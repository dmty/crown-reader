use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use uuid::Uuid;

use crate::auth::{token, Credentials, TokenStore};
use crate::raw::RawDecoder;
use crate::state::{ConnectionState, Live};
use crate::stitch::Stitcher;
use crate::streams::{Awareness, DeviceInfo, PowerByBand, SignalQuality};

pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x00001803_0000_1000_8000_00805f9b34fb);
pub const CHAR_AUTH: Uuid = Uuid::from_u128(0x7f9f1a35_9816_471b_bf67_2ec6f2295a1d);
pub const CHAR_DEVICE_INFO: Uuid = Uuid::from_u128(0x97b81f68_04cf_4650_a044_14924f11b9ee);
pub const CHAR_RAW: Uuid = Uuid::from_u128(0x009cf0bb_b68d_4af1_a0e5_625f2eb964a6);
pub const CHAR_POWER_BY_BAND: Uuid = Uuid::from_u128(0x2f6236dd_215a_427f_b94c_ab5df71937af);
pub const CHAR_FOCUS: Uuid = Uuid::from_u128(0x8e12baf1_81bb_4a1b_8948_9e68a4457d2a);
pub const CHAR_CALM: Uuid = Uuid::from_u128(0x7d47617d_a60a_41d1_8df6_cfb78d02ffeb);
pub const CHAR_SIGNAL_QUALITY: Uuid = Uuid::from_u128(0xcf28ed0c_20cd_48ed_93c5_ee2fb265099a);

const NAME_PREFIXES: [&str; 2] = ["Crown-", "Notion-"];

/// Scans for a headset by advertised name. The service UUID is not used as a
/// scan filter because the device does not reliably advertise it.
pub async fn find_crown(adapter: &Adapter) -> Result<Peripheral> {
    adapter.start_scan(ScanFilter::default()).await?;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            let name = match p.properties().await? {
                Some(props) => props.local_name.unwrap_or_default(),
                None => continue,
            };
            if NAME_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
                adapter.stop_scan().await?;
                return Ok(p);
            }
        }
    }
    adapter.stop_scan().await?;
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

/// A direct characteristic read returns one complete value, not a notify
/// fragment, so it bypasses the newline-delimited `Stitcher` used for the
/// subscribed stream and is parsed directly instead.
fn parse_direct_read(bytes: &[u8]) -> Option<DeviceInfo> {
    let text = std::str::from_utf8(bytes).ok()?;
    serde_json::from_str(text.trim()).ok()
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

/// Writes the minted JWT and reads back `[isAuthenticated, expiresIn]`.
pub async fn authenticate(p: &Peripheral, jwt: &str) -> Result<(bool, Option<f64>)> {
    let auth = characteristic(p, CHAR_AUTH).await?;
    write_jwt(p, &auth, jwt).await?;
    let raw = p.read(&auth).await?;
    let parsed: serde_json::Value = serde_json::from_slice(&raw)?;
    let ok = parsed
        .get(0)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow!("auth response missing boolean: {parsed}"))?;
    let expires_in = parsed.get(1).and_then(|v| v.as_f64());
    Ok((ok, expires_in))
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
pub async fn run(
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    raw_enabled: bool,
) -> Result<()> {
    let live_for_failure = live.clone();
    let result = try_run(live, creds, store, raw_enabled).await;
    if result.is_err() {
        let mut l = live_for_failure.lock().unwrap();
        l.connection = ConnectionState::Failed;
        l.touch();
    }
    result
}

async fn try_run(
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    raw_enabled: bool,
) -> Result<()> {
    let set = |s: ConnectionState| {
        let mut l = live.lock().unwrap();
        l.connection = s;
        l.touch();
    };

    set(ConnectionState::Scanning);
    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter available"))?;
    let peripheral = find_crown(&adapter).await?;

    set(ConnectionState::Connecting);
    peripheral.connect().await?;
    peripheral.discover_services().await?;

    set(ConnectionState::Authenticating);
    let mut jwt = token(&creds, store.as_ref(), false).await?;
    let (mut ok, _) = authenticate(&peripheral, &jwt).await?;
    if !ok {
        // A cached token can outlive its validity; mint once more, then give up.
        store.clear();
        jwt = token(&creds, store.as_ref(), true).await?;
        (ok, _) = authenticate(&peripheral, &jwt).await?;
    }
    if !ok {
        return Err(anyhow!("device rejected the Bluetooth token twice"));
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

    // deviceInfo gates everything else: raw has no delimiter or checksum, so
    // decoding it needs the channel count from deviceInfo first, and every
    // per-channel structure in `Live` is sized from it too. It is resolved
    // completely — by direct read if possible, otherwise by subscribing to
    // it alone and waiting — before any other characteristic is subscribed.
    // That ordering matters: btleplug buffers at most 16 notifications and
    // silently discards the oldest past that (a lagged read is dropped, not
    // surfaced), so subscribing the other four characteristics first — any
    // of which can emit before anything drains the buffer — risks evicting
    // the one-shot deviceInfo notification before it is ever read, wedging
    // raw off for the life of the connection with nothing to show why.
    let device_info_char = characteristic(&peripheral, CHAR_DEVICE_INFO).await?;
    let mut stitchers: std::collections::HashMap<Uuid, Stitcher> = Default::default();

    // A direct read sidesteps the eviction race entirely, so try it first.
    // Not every characteristic is readable — a failure here just falls
    // through to the subscribe-and-wait path below.
    let mut device_info_configured = match peripheral.read(&device_info_char).await {
        Ok(bytes) => match parse_direct_read(&bytes) {
            Some(info) => {
                let mut l = live.lock().unwrap();
                l.configure(info);
                l.device.is_some()
            }
            None => false,
        },
        Err(_) => false,
    };

    peripheral.subscribe(&device_info_char).await?;
    while !device_info_configured {
        let Some(n) = next_or_disconnected(&mut notifications, &peripheral, &mut liveness).await
        else {
            set(ConnectionState::Disconnected);
            return Ok(());
        };
        if n.uuid != CHAR_DEVICE_INFO {
            continue;
        }
        for line in stitchers.entry(n.uuid).or_default().push(&n.value) {
            let mut l = live.lock().unwrap();
            let parsed = apply_json(&line, |d: DeviceInfo| l.configure(d));
            device_info_configured = l.device.is_some();
            if !parsed {
                l.dropped_frames += 1;
                l.touch();
            }
        }
    }

    for uuid in [CHAR_POWER_BY_BAND, CHAR_CALM, CHAR_FOCUS, CHAR_SIGNAL_QUALITY] {
        peripheral.subscribe(&characteristic(&peripheral, uuid).await?).await?;
    }

    // Raw is opportunistic: a device without the characteristic, or a
    // subscribe that fails for any other reason, just leaves raw off rather
    // than tearing down an otherwise healthy session. `Live::raw_enabled`
    // staying false is itself the visible record of that — there is no
    // logging dependency in this workspace.
    //
    // Subscribed last, immediately before we start draining the stream: raw
    // is the one characteristic where an evicted notification can't self-heal
    // (see the comment below), so unlike the four JSON streams above, the gap
    // between "notifications can arrive" and "something is reading them" has
    // to be kept as close to zero as it can be.
    if raw_enabled {
        let subscribed = match characteristic(&peripheral, CHAR_RAW).await {
            Ok(raw_char) => peripheral.subscribe(&raw_char).await.is_ok(),
            Err(_) => false,
        };
        let mut l = live.lock().unwrap();
        l.raw_enabled = subscribed;
        l.touch();
    }

    set(ConnectionState::Streaming);

    // A lost raw notification permanently offsets the byte stream: there is
    // no delimiter or checksum to resynchronize on, and btleplug drops a
    // lagged notification rather than surfacing it, so this layer has no way
    // to detect or recover from one. Contrast a lost JSON line, which
    // self-heals at the next `\n`. A resulting desync shows up as absurd
    // values in the CLI's alignment diagnostic — currently the only place
    // it's visible. Also worth remembering: the `Live` mutex locked all
    // through this loop is contended with the UI/CLI thread's `snapshot()`
    // call, so a slow render is itself a contributor to notification lag.
    let mut raw_decoder = RawDecoder::default();

    loop {
        let Some(n) = next_or_disconnected(&mut notifications, &peripheral, &mut liveness).await
        else {
            break;
        };

        if n.uuid == CHAR_RAW {
            let channels = {
                let l = live.lock().unwrap();
                l.device.as_ref().map(|d| d.channels).unwrap_or(0)
            };
            if channels == 0 {
                // Belt-and-braces: raw is only ever subscribed once deviceInfo
                // has configured `Live`, so this should be unreachable.
                continue;
            }
            let samples = raw_decoder.push(&n.value, channels);
            let mut l = live.lock().unwrap();
            for s in &samples {
                l.push_raw(s);
            }
            continue;
        }

        for line in stitchers.entry(n.uuid).or_default().push(&n.value) {
            let mut l = live.lock().unwrap();
            let parsed = match n.uuid {
                CHAR_DEVICE_INFO => apply_json(&line, |d: DeviceInfo| l.configure(d)),
                CHAR_POWER_BY_BAND => apply_json(&line, |b: PowerByBand| {
                    l.bands = Some(b);
                    l.touch();
                }),
                CHAR_CALM => apply_json(&line, |a: Awareness| {
                    l.calm = a.probability as f32;
                    l.touch();
                }),
                CHAR_FOCUS => apply_json(&line, |a: Awareness| {
                    l.focus = a.probability as f32;
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
        }
    }

    set(ConnectionState::Disconnected);
    Ok(())
}
