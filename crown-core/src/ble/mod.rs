use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use uuid::Uuid;

use crate::auth::{token, Credentials, TokenStore};
use crate::record::Recorder;
use crate::state::{ConnectionState, Live};
use crate::stitch::Stitcher;
use crate::streams::{Awareness, DeviceInfo, PowerByBand, SignalQuality};

pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x00001803_0000_1000_8000_00805f9b34fb);
pub const CHAR_AUTH: Uuid = Uuid::from_u128(0x7f9f1a35_9816_471b_bf67_2ec6f2295a1d);
pub const CHAR_DEVICE_INFO: Uuid = Uuid::from_u128(0x97b81f68_04cf_4650_a044_14924f11b9ee);
pub const CHAR_POWER_BY_BAND: Uuid = Uuid::from_u128(0x2f6236dd_215a_427f_b94c_ab5df71937af);
pub const CHAR_FOCUS: Uuid = Uuid::from_u128(0x8e12baf1_81bb_4a1b_8948_9e68a4457d2a);
pub const CHAR_CALM: Uuid = Uuid::from_u128(0x7d47617d_a60a_41d1_8df6_cfb78d02ffeb);
pub const CHAR_SIGNAL_QUALITY: Uuid = Uuid::from_u128(0xcf28ed0c_20cd_48ed_93c5_ee2fb265099a);

const NAME_PREFIXES: [&str; 2] = ["Crown-", "Notion-"];

mod probe;

/// Host clock in epoch milliseconds, matching how `record.rs` stamps its
/// clock anchor. Paired with a metric's own device timestamp to measure how
/// far the metric stream has fallen behind.
fn host_epoch_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

use probe::{dump_gatt, MetricProbe};

/// btleplug's CoreBluetooth backend resolves a characteristic read's
/// underlying future only from a `didUpdateValueForCharacteristic` callback
/// or a disconnect sweep — there is no timeout inside btleplug itself, so a
/// read that never gets acknowledged hangs forever without one of these.
/// Two separate bounds below guard two different things, deliberately not
/// shared: one is a request/response round trip, the other is "wait for the
/// device to volunteer its identity blob" on a link that was just
/// authenticated and may still be settling its notify pipeline.
///
/// Bound on the auth characteristic's read reply.
const AUTH_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on the wait for the one-shot deviceInfo notification after
/// subscribing to it. Longer than `AUTH_READ_TIMEOUT`: firing this early on
/// a device that's simply slow to start notifying degrades a working
/// headset to `channels=0`, which is a worse failure mode than waiting a
/// few extra seconds.
const DEVICE_INFO_TIMEOUT: Duration = Duration::from_secs(15);

/// Bound on `Peripheral::disconnect()`, called on every exit from
/// `try_run`. Not the same class of "device is slow" timeout as the two
/// above — this one exists because btleplug 0.12's CoreBluetooth backend
/// has a real gap, not because a real disconnect can legitimately take
/// this long. `disconnect_peripheral` in btleplug's internal event loop is
/// `if let Some(p) = self.peripherals.get_mut(...) { ... }` with **no
/// `else` branch** — contrast `is_connected`'s handler in the same file,
/// which does have one, with a comment naming exactly this hazard
/// ("rather than hanging the future forever"). A device-initiated
/// disconnect (the headset switching off) reaches that same internal map
/// first via `on_peripheral_disconnect`, which removes the entry *before*
/// our own `disconnect()` call can run — so the reply future our call
/// awaits is never fulfilled, and a bare `.await` hangs forever on exactly
/// the "headset switched off" path the reconnect supervisor exists to
/// recover from. A short timeout, with expiry treated as success (the
/// peripheral already being gone is exactly the case that hangs), is the
/// only way to bound this without patching btleplug itself. Do not delete
/// this timeout as unneeded caution — it is load-bearing.
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

// These bounds cover the auth read, the deviceInfo wait, and the
// disconnect specifically — they are not a general guarantee that
// `Authenticating` (or any other state) is bounded. `connect()`,
// `discover_services()`, `write_jwt`'s `p.write()`, and every
// `subscribe()` call below are the same kind of unbounded btleplug await,
// with the same "only a value or a disconnect resolves it, nothing else"
// shape, and none of them have a timeout here. A device that acknowledges
// the connection but never completes one of those calls still hangs the
// session indefinitely. Known limitation, not addressed in this round —
// adding timeouts to every btleplug call is a larger change than fits
// here.

/// Builds the one `Adapter` a caller should hold for the life of a
/// reconnecting session, rather than a fresh one per attempt.
///
/// `Manager::adapters()` is not free: btleplug 0.12's CoreBluetooth backend
/// spawns a dedicated thread running `loop { cbi.wait_for_message().await; }`
/// with no break, so the `CBCentralManager` and the thread backing it live
/// until the process exits — there is no `Drop` impl that ever tears it
/// down. Calling this once per `supervise` lifetime (see that function)
/// instead of once per reconnect attempt is what keeps a headset that's
/// off or out of range from leaking a thread and a central manager on
/// every retry.
pub async fn first_adapter() -> Result<Adapter> {
    let manager = Manager::new().await?;
    manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter available"))
}

/// Clears `adapter`'s btleplug-internal peripheral cache before a scan.
///
/// Needed only because `adapter` is now reused across every reconnect
/// attempt (see `first_adapter`) rather than rebuilt fresh each time.
/// btleplug's `AdapterManager` — the cache backing `adapter.peripherals()`
/// — removes an entry when a peripheral actually disconnects or the
/// adapter powers off (both dispatch a `DeviceDisconnected` event that
/// `AdapterManager::emit` acts on), but *not* when a connection attempt
/// simply fails: the CoreBluetooth backend's
/// `on_peripheral_connection_failed` only fails the pending `connect()`
/// future, it never dispatches any disconnect event. A peripheral that
/// fails to connect once — an ordinary, common occurrence on real
/// hardware, not a bug — therefore stays cached forever on a reused
/// adapter. The next time it's rediscovered, `AdapterManager`'s
/// `add_peripheral` does `assert!(!self.peripherals.contains_key(...),
/// "Adding a peripheral that's already in the map.")`, which panics
/// btleplug's own background event-pump task — silently breaking every
/// future scan and connect on this adapter, with no error surfaced to us.
/// A fresh adapter per attempt (the old, leaking shape) never hit this,
/// since its cache started empty every time; reuse is what exposed it.
/// `clear_peripherals` exists on the `Central` trait specifically for
/// this and is never called internally by btleplug itself. Best-effort: a
/// failure here is logged, not propagated — the scan that follows can
/// still succeed against whatever the cache already holds, and failing
/// the whole attempt over a cache-clear error would be worse than the
/// (still merely possible, not certain) assert this is guarding against.
async fn clear_stale_cache(adapter: &Adapter) {
    if let Err(e) = adapter.clear_peripherals().await {
        eprintln!("warning: failed to clear the Bluetooth adapter's peripheral cache: {e}");
    }
}

/// Scans for a headset by advertised name. The service UUID is not used as a
/// scan filter because the device does not reliably advertise it.
///
/// Clears `adapter`'s peripheral cache before every scan — see
/// `clear_stale_cache`'s doc comment for why a reused adapter needs this and
/// a fresh one never did.
///
/// Always stops the scan before returning, on every exit path including an
/// error from `adapter.peripherals()`/`p.properties()` partway through —
/// best-effort, logged rather than allowed to replace the real result. This
/// matters more than it used to: `adapter` is now built once per `supervise`
/// session (see `first_adapter`) and reused across every reconnect attempt,
/// rather than a fresh, throwaway one per attempt, so a scan left running by
/// an early return here would carry over into the *next* attempt's
/// `start_scan` on that same adapter instead of dying with a discarded one.
/// CoreBluetooth tolerates a redundant `start_scan`, so this was never a
/// correctness bug — but there is no reason to lean on that tolerance when
/// stopping cleanly is this cheap.
pub async fn find_crown(adapter: &Adapter) -> Result<Peripheral> {
    clear_stale_cache(adapter).await;
    adapter.start_scan(ScanFilter::default()).await?;
    let result = scan_for_crown(adapter).await;
    if let Err(e) = adapter.stop_scan().await {
        eprintln!("warning: failed to stop the Bluetooth scan: {e}");
    }
    result
}

async fn scan_for_crown(adapter: &Adapter) -> Result<Peripheral> {
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            let name = match p.properties().await? {
                Some(props) => props.local_name.unwrap_or_default(),
                None => continue,
            };
            if NAME_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
                return Ok(p);
            }
        }
    }
    Err(anyhow!("no Crown or Notion device found within 10 seconds"))
}

async fn characteristic(
    p: &Peripheral,
    uuid: Uuid,
) -> Result<btleplug::api::Characteristic> {
    p.characteristics()
        .into_iter()
        .find(|c| c.uuid == uuid)
        .ok_or_else(|| anyhow!("characteristic {uuid} not found on device"))
}

/// Parses `line` as `T` and applies it to `Live` via `apply` on success.
/// Returns whether the parse succeeded; callers count a `false` as a
/// dropped frame rather than treating it as fatal.
fn apply_json<T: serde::de::DeserializeOwned>(line: &str, apply: impl FnOnce(T)) -> bool {
    match serde_json::from_str(line) {
        Ok(v) => {
            apply(v);
            true
        }
        Err(_) => false,
    }
}

/// Records a successfully-parsed derived-metric line to the active
/// recorder, if any. Returns `true` if this call was the one that just
/// turned recording off, so the caller can clear `Live::recording` to
/// match — deliberately not done in here: locking `live` while still
/// holding `recorder`'s guard would invert the lock order `run`'s doc
/// comment establishes. The caller locks `recorder` (via this function),
/// lets the guard drop, and only then locks `live`. Only `calm`, `focus`,
/// `powerByBand`, and `signalQuality` are derived streams; `deviceInfo`
/// (and anything else routed through this dispatch) is metadata already
/// captured once in `meta.json`, and is not written again here.
///
/// Called only after the caller's `live` lock has already been released —
/// see `run`'s doc comment for the lock-ordering rule this keeps. A write
/// failure — a disk problem — is not a streaming problem: recording is
/// secondary to the live connection, so it must not propagate into
/// `stream_session`'s `Result` and tear the session down. Latches off on
/// the first write failure rather than retried: a disk failure persists, so
/// retrying every line would only add noise.
fn record_derived_line(recorder: &Mutex<Option<Recorder>>, uuid: Uuid, line: &str) -> bool {
    let name = match uuid {
        CHAR_CALM => "calm",
        CHAR_FOCUS => "focus",
        CHAR_POWER_BY_BAND => "powerByBand",
        CHAR_SIGNAL_QUALITY => "signalQuality",
        _ => return false,
    };
    let mut guard = crate::sync::lock(recorder);
    let Some(r) = guard.as_mut() else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return false };
    if let Err(e) = r.write_derived(name, &v) {
        eprintln!("warning: recorder stopped, failed to write derived sample: {e}");
        *guard = None;
        return true;
    }
    false
}

/// Clears `Live::recording` after a recorder write failure has already
/// turned the recorder itself off. Called with `recorder`'s lock already
/// released (`record_derived_line` only returns `true` after dropping its
/// guard), so this is the only place in the derived recording path that
/// locks `live` — never nested inside a `recorder` lock.
fn clear_recording_indicator(live: &Mutex<Live>) {
    let mut l = crate::sync::lock(live);
    l.recording = None;
    l.touch();
}

/// Writes `jwt` to the auth characteristic.
///
/// A single write is tried first. If it fails with what looks like a length
/// or MTU error, we fall back to writing MTU-sized chunks with-response,
/// since CoreBluetooth enforces a hard per-write length cap and doesn't
/// always perform a queued long write the way Web Bluetooth does. btleplug's
/// CoreBluetooth backend reports any write failure as `Error::RuntimeError`
/// carrying whatever text the OS gave it, so detection here is a
/// best-effort keyword match rather than a typed error — only real hardware
/// can confirm which path this device actually needs. If the
/// keyword match is a false positive and the chunked fallback also fails,
/// the returned error carries both failures rather than only the second, so
/// the real cause isn't masked.
async fn write_jwt(p: &Peripheral, auth: &btleplug::api::Characteristic, jwt: &str) -> Result<()> {
    let bytes = jwt.as_bytes();
    let Err(first_err) = p.write(auth, bytes, WriteType::WithResponse).await else {
        return Ok(());
    };
    let msg = first_err.to_string().to_lowercase();
    let looks_like_a_length_error =
        ["length", "mtu", "too long", "exceed"].iter().any(|kw| msg.contains(kw));
    if !looks_like_a_length_error {
        return Err(first_err.into());
    }
    let mtu = match p.mtu() {
        // 512 was a guess and can fail for the same reason the single write
        // did. 20 is the guaranteed default ATT payload — every BLE
        // connection supports at least a 23-byte ATT MTU — so it is safe
        // rather than merely likely, at the cost of more round trips.
        0 => 23,
        n => n as usize,
    };
    let chunk_size = mtu.saturating_sub(3).max(1);
    for chunk in bytes.chunks(chunk_size) {
        if let Err(chunk_err) = p.write(auth, chunk, WriteType::WithResponse).await {
            return Err(anyhow!(
                "JWT write failed both as a single write ({first_err}) and chunked ({chunk_err})"
            ));
        }
    }
    Ok(())
}

/// Outcome of one auth-characteristic exchange.
///
/// A timeout is not evidence the token was bad — only the device actually
/// answering `false` is — so `NoAnswer` is kept distinct from `Rejected`
/// rather than folded into it: a caller must not clear a cached token, and
/// must not spend its one retry, on a device that simply didn't respond in
/// time.
pub enum AuthOutcome {
    /// The device accepted the token; carries the response's `expiresIn`,
    /// if it sent one.
    Accepted(Option<f64>),
    /// The device answered with `isAuthenticated: false`.
    Rejected,
    /// No answer arrived within `AUTH_READ_TIMEOUT`.
    NoAnswer,
}

/// The device rejected the Bluetooth token twice in a row: once on the
/// token `stream_session` started with, and again after a forced re-mint. A typed
/// error rather than a string, so a caller (`supervise`) can classify it as
/// terminal by type rather than by matching on message text: no amount of
/// retrying fixes a token the device itself refuses to authenticate twice.
#[derive(Debug)]
pub struct DeviceRejectedToken;

impl std::fmt::Display for DeviceRejectedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "device rejected the Bluetooth token twice")
    }
}

impl std::error::Error for DeviceRejectedToken {}

/// Writes the minted JWT and reads back `[isAuthenticated, expiresIn]`.
///
/// The read is bounded by `AUTH_READ_TIMEOUT`: btleplug has no timeout of its
/// own on a characteristic read (see that constant's doc comment), and
/// this characteristic is never subscribed, so — unlike the deviceInfo read
/// this codebase deliberately avoids (see `stream_session`) — there is no
/// notification stream whose data an abandoned read future could steal.
pub async fn authenticate(p: &Peripheral, jwt: &str) -> Result<AuthOutcome> {
    let auth = characteristic(p, CHAR_AUTH).await?;
    write_jwt(p, &auth, jwt).await?;
    let raw = match tokio::time::timeout(AUTH_READ_TIMEOUT, p.read(&auth)).await {
        Ok(read) => read?,
        Err(_) => return Ok(AuthOutcome::NoAnswer),
    };
    let parsed: serde_json::Value = serde_json::from_slice(&raw)?;
    let ok = parsed
        .get(0)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow!("auth response missing boolean: {parsed}"))?;
    let expires_in = parsed.get(1).and_then(|v| v.as_f64());
    Ok(if ok { AuthOutcome::Accepted(expires_in) } else { AuthOutcome::Rejected })
}

type Notifications = Pin<Box<dyn futures::Stream<Item = btleplug::api::ValueNotification> + Send>>;

/// Waits for the next notification, treating a device-level disconnect as
/// the end of the stream.
///
/// btleplug's notification stream does not end on its own here: its sender
/// lives inside `Peripheral`'s shared state, which the caller holds for the
/// whole connection, so the stream can never observe its own sender
/// dropping. Without this check, `notifications.next()` blocks forever once
/// the headset is turned off or walks out of range: `Disconnected` is never
/// reached and `run` never returns, which would leave a caller with no way
/// to notice the session ended. Polling `is_connected` is simpler than
/// watching `CentralEvent::DeviceDisconnected` and doesn't require the
/// adapter to be kept around past the initial scan.
///
/// `liveness` is owned by the caller and threaded through every call rather
/// than created fresh here: a freshly-made `Interval`'s first `tick()`
/// always fires immediately, so recreating one per call would mean an
/// `is_connected` round trip on nearly every notification under raw
/// streaming instead of once every couple of seconds.
async fn next_or_disconnected(
    notifications: &mut Notifications,
    peripheral: &Peripheral,
    liveness: &mut tokio::time::Interval,
) -> Option<btleplug::api::ValueNotification> {
    loop {
        tokio::select! {
            n = notifications.next() => return n,
            _ = liveness.tick() => {
                // A transient error here is treated the same as a confirmed
                // disconnect rather than assumed-still-connected: the other
                // way round risks reintroducing the hang this function
                // exists to prevent, and ending a session on a false
                // disconnect is a far cheaper mistake than never ending one.
                if !peripheral.is_connected().await.unwrap_or(false) {
                    return None;
                }
            }
        }
    }
}

/// Connects, authenticates, subscribes, and pumps notifications into `live`
/// until the connection drops. Returns `Ok(())` on a clean disconnect.
///
/// This is a thin wrapper around `try_run` whose only job is to make sure
/// `Live::connection` reflects a `Failed` state on *any* error return —
/// scan failure, connect failure, subscribe failure, auth rejection, all of
/// it — rather than leaving it at whatever the last successful step set.
///
/// `adapter` is built once by the caller (`supervise`) and reused across
/// every reconnect attempt — see `first_adapter`'s doc comment for why a
/// fresh one per attempt leaks.
///
/// `recorder` starts (and typically stays) `None`; a caller flips it to
/// `Some` to turn recording on mid-session and back to `None` to turn it
/// off, without needing to restart the connection. This is stronger than a
/// consistent lock order: everywhere below that touches both, one guard is
/// fully acquired *and released* before the other is ever taken — `live` then
/// `recorder` when recording a derived-metric line, `recorder` then `live`
/// when a write failure needs `Live::recording` cleared to match — so the two
/// locks are never held at the same time in either direction. There is no
/// ordering for a caller elsewhere (e.g. a future UI thread) to invert into a
/// cycle, because there is no window in which this code holds one while
/// waiting on the other. Neither lock is ever held across an `.await`.
pub async fn run(
    adapter: &Adapter,
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    recorder: Arc<Mutex<Option<Recorder>>>,
) -> Result<()> {
    let live_for_failure = live.clone();
    let result = try_run(adapter, live, creds, store, recorder).await;
    if result.is_err() {
        let mut l = crate::sync::lock(&live_for_failure);
        l.connection = ConnectionState::Failed;
        l.touch();
    }
    result
}

async fn try_run(
    adapter: &Adapter,
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    recorder: Arc<Mutex<Option<Recorder>>>,
) -> Result<()> {
    // `streaming_since` is managed here, alongside `connection`, rather than
    // by the caller: `Scanning` marks the start of a fresh attempt and
    // clears state left over from a previous run on this same `Live` (it is
    // not recreated per attempt) back to `None`, and the rest of this
    // function only ever sets it back to something truthful for *this*
    // attempt. `Streaming` is the one transition that stamps
    // `streaming_since`. Every other state (`Connecting`, `Authenticating`,
    // `Disconnected`) leaves it alone — in particular, `Disconnected` must
    // not clear `streaming_since`, since a caller reading `Live` after
    // `run()` returns (to measure how long the session actually streamed)
    // needs it to still be there.
    let set = |s: ConnectionState| {
        let mut l = crate::sync::lock(&live);
        l.connection = s;
        match s {
            ConnectionState::Scanning => {
                l.streaming_since = None;
                l.forget_metric_clock();
            }
            ConnectionState::Streaming => l.streaming_since = Some(Instant::now()),
            _ => {}
        }
        l.touch();
    };

    set(ConnectionState::Scanning);
    let peripheral = find_crown(adapter).await?;

    // Every exit from here down — a clean disconnect, an error partway
    // through, or the terminal double-rejection path alike — must release
    // the link: the Crown accepts only one connection at a time, and
    // btleplug 0.12's CoreBluetooth backend has no `Drop` impl on
    // `Peripheral` to do this automatically (see `first_adapter`'s doc
    // comment for the adapter side of the same class of leak). Best-effort
    // and time-bounded (see `DISCONNECT_TIMEOUT`'s doc comment): a
    // disconnect failure — or a timeout, which is the *expected* shape of
    // "the headset already disconnected itself" — is logged and never
    // allowed to replace the real result from `stream_session`; `run`'s
    // caller needs that result to classify the failure and decide whether
    // to retry.
    let result =
        stream_session(&peripheral, live.clone(), creds, store, recorder, &set).await;
    match tokio::time::timeout(DISCONNECT_TIMEOUT, peripheral.disconnect()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("warning: failed to disconnect from the headset: {e}"),
        Err(_) => {
            // Expected, not a failure: see `DISCONNECT_TIMEOUT`'s doc
            // comment for why btleplug never resolves this future when the
            // peripheral disconnected on its own first.
        }
    }
    result
}

/// Authenticates, subscribes, and pumps notifications from an already-found
/// `peripheral` into `live` until the connection drops. Split out of
/// `try_run` so that its caller can guarantee `peripheral.disconnect()` runs
/// no matter which way this returns.
async fn stream_session(
    peripheral: &Peripheral,
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    recorder: Arc<Mutex<Option<Recorder>>>,
    // `Send + Sync` on the trait object, not just the underlying closure: a
    // bare `dyn Fn(ConnectionState)` erases that the closure only captures a
    // `&Arc<Mutex<Live>>` (itself `Send + Sync`), which would otherwise make
    // this whole future `!Send` and reject it at `tokio::spawn`.
    set: &(dyn Fn(ConnectionState) + Send + Sync),
) -> Result<()> {
    set(ConnectionState::Connecting);
    peripheral.connect().await?;
    peripheral.discover_services().await?;
    dump_gatt(peripheral);

    set(ConnectionState::Authenticating);
    let mut jwt = token(&creds, store.as_ref(), false).await?;
    let mut retried = false;
    loop {
        match authenticate(peripheral, &jwt).await? {
            AuthOutcome::Accepted(_) => break,
            AuthOutcome::NoAnswer => {
                // Not a rejection: the cached token may be fine and the
                // device just didn't answer in time. Leave the cache alone
                // and fail outright rather than spending the one retry on
                // an inconclusive result.
                return Err(anyhow!(
                    "device did not answer the auth read within {AUTH_READ_TIMEOUT:?}"
                ));
            }
            AuthOutcome::Rejected if !retried => {
                // A cached token can outlive its validity; mint once more, then give up.
                let _ = store.clear();
                jwt = token(&creds, store.as_ref(), true).await?;
                retried = true;
            }
            AuthOutcome::Rejected => {
                return Err(DeviceRejectedToken.into());
            }
        }
    }

    // The notification stream must exist before any subscribe call: btleplug
    // only delivers notifications that arrive after this call, and a
    // one-shot notification sent right on subscribe would otherwise be lost.
    let mut notifications = peripheral.notifications().await?;
    let mut liveness = tokio::time::interval(Duration::from_secs(2));
    // Default `Burst` behavior replays every missed tick back-to-back once
    // notifications stop winning the select race for a while, which would
    // turn "check every couple of seconds" into an actual burst of
    // `is_connected` calls. `Delay` collapses a backlog of missed ticks into
    // a single one instead.
    liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // deviceInfo gates everything else: every per-channel structure in
    // `Live` is sized from its channel count. It is resolved by subscribing
    // to it alone and waiting, before any other characteristic is
    // subscribed. That ordering matters: btleplug buffers at most 16
    // notifications and silently discards the oldest past that (a lagged
    // notification is dropped, not surfaced), so subscribing the other four
    // characteristics first — any of which can emit before anything drains
    // the buffer — risks evicting the one-shot deviceInfo notification
    // before it is ever read.
    //
    // A direct `read()` on this characteristic looks like the obvious way to
    // sidestep that race — an earlier version of this function did exactly
    // that — but it does not work on btleplug 0.12's CoreBluetooth backend.
    // Reads and notifications share one delegate callback there
    // (`on_characteristic_read`'s own source comment: "Reads and
    // notifications both return the same callback"), matched to whichever
    // read future is oldest in a per-characteristic queue, not to the
    // specific request that produced the value. A read that is never
    // acknowledged — not readable, or the callback simply never fires —
    // leaves its future queued forever with no timeout inside btleplug to
    // end it, and wrapping it in `tokio::time::timeout` does not help:
    // dropping our side of the future does not deregister it from that
    // queue, so the *next* real deviceInfo notification gets popped off the
    // queue and used to fulfill the abandoned read instead of ever reaching
    // us as a notification — silently, since nothing is awaiting that read
    // any more. Subscribing and draining the notification stream has no
    // equivalent hazard: `StreamExt::next` only borrows the stream, so a
    // cancelled wait loses nothing already queued in it, which is what makes
    // wrapping the drain below in a timeout safe where wrapping the read
    // was not.
    let device_info_char = characteristic(peripheral, CHAR_DEVICE_INFO).await?;
    let mut stitchers: std::collections::HashMap<Uuid, Stitcher> = Default::default();

    peripheral.subscribe(&device_info_char).await?;
    match tokio::time::timeout(DEVICE_INFO_TIMEOUT, async {
        let mut configured = false;
        while !configured {
            let Some(n) =
                next_or_disconnected(&mut notifications, peripheral, &mut liveness).await
            else {
                return None; // disconnected before deviceInfo ever arrived
            };
            if n.uuid != CHAR_DEVICE_INFO {
                continue;
            }
            for line in stitchers.entry(n.uuid).or_default().push(&n.value) {
                let mut l = crate::sync::lock(&live);
                let parsed = apply_json(&line, |d: DeviceInfo| l.configure(d));
                configured = l.device.is_some();
                if !parsed {
                    l.dropped_frames += 1;
                    l.touch();
                }
            }
        }
        Some(())
    })
    .await
    {
        Ok(None) => {
            set(ConnectionState::Disconnected);
            return Ok(());
        }
        Ok(Some(())) => {}
        Err(_) => {
            // No deviceInfo within DEVICE_INFO_TIMEOUT: degrade instead of
            // hanging. The other four characteristics still get subscribed
            // below and the session still reaches Streaming — just without a
            // channel count to size anything from. Counted here since there
            // is no logging dependency in this workspace: a human watching
            // the CLI sees `channels=0` hold forever instead of populating,
            // alongside this one-off bump in `dropped_frames`.
            let mut l = crate::sync::lock(&live);
            l.dropped_frames += 1;
            l.touch();
        }
    };

    for uuid in [CHAR_POWER_BY_BAND, CHAR_CALM, CHAR_FOCUS, CHAR_SIGNAL_QUALITY] {
        peripheral.subscribe(&characteristic(peripheral, uuid).await?).await?;
    }

    set(ConnectionState::Streaming);

    let mut metric_probe = MetricProbe::new();

    loop {
        let Some(n) = next_or_disconnected(&mut notifications, peripheral, &mut liveness).await
        else {
            break;
        };

        for line in stitchers.entry(n.uuid).or_default().push(&n.value) {
            let mut calm_timestamp = None;
            // Read before the lock: a clock read is cheap, but nothing that
            // does not need the guard belongs under it.
            let host_ms = host_epoch_ms();
            let parsed = {
                let mut l = crate::sync::lock(&live);
                let parsed = match n.uuid {
                    CHAR_DEVICE_INFO => apply_json(&line, |d: DeviceInfo| l.configure(d)),
                    CHAR_POWER_BY_BAND => apply_json(&line, |b: PowerByBand| {
                        l.bands = Some(b);
                        l.touch();
                    }),
                    CHAR_CALM => apply_json(&line, |a: Awareness| {
                        l.calm = a.probability as f32;
                        l.note_metric_time(a.timestamp, host_ms);
                        l.touch();
                        calm_timestamp = Some(a.timestamp);
                    }),
                    CHAR_FOCUS => apply_json(&line, |a: Awareness| {
                        l.focus = a.probability as f32;
                        l.note_metric_time(a.timestamp, host_ms);
                        l.touch();
                    }),
                    CHAR_SIGNAL_QUALITY => apply_json(&line, |q: SignalQuality| {
                        l.quality = q;
                        l.touch();
                    }),
                    _ => true,
                };
                if !parsed {
                    l.dropped_frames += 1;
                    l.touch();
                }
                parsed
            }; // `live`'s lock is dropped here, before `recorder` is ever touched.

            if let Some(ts) = calm_timestamp {
                metric_probe.observe(ts);
            }

            if parsed && record_derived_line(&recorder, n.uuid, &line) {
                clear_recording_indicator(&live);
            }
        }
    }

    set(ConnectionState::Disconnected);
    Ok(())
}
