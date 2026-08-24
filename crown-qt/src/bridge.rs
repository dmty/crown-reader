#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;

        include!("cxx-qt-lib/qpointf.h");
        type QPointF = cxx_qt_lib::QPointF;
        include!("cxx-qt-lib/qlist.h");
        type QList_QPointF = cxx_qt_lib::QList<QPointF>;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, connection)]
        #[qproperty(f64, calm)]
        #[qproperty(f64, focus)]
        #[qproperty(i32, dropped)]
        /// Milliseconds the metric stream has fallen behind, or -1 before
        /// the first metric arrives. Negative rather than optional because
        /// QML has no ergonomic Option; the QML side treats <0 as "unknown".
        ///
        /// One word deliberately: cxx-qt 0.9 does not convert case, so a
        /// snake_case field would have to be spelled the same way in QML.
        #[qproperty(i32, staleness)]
        #[qproperty(QString, recording)]
        #[qproperty(bool, raw)]
        // `ConnectionState::is_active()`, republished so QML can gate the
        // raw toggle on a fact rather than string-matching `connection`
        // against `label()`'s output.
        #[qproperty(bool, active)]
        // Whether `Live` has a configured device, i.e. whether starting a
        // recording is possible right now.
        #[qproperty(bool, ready)]
        // The message from the most recent session-ending failure, empty
        // when there isn't one. `supervise` deliberately never prints a
        // terminal error itself so there is exactly one place a human sees
        // it; for this front end, this property is that place. Cleared at
        // the start of the next `start()` call, not on every tick, since a
        // `Failed` session doesn't retry on its own — the message should
        // stay put until the user acts on it.
        #[qproperty(QString, error)]
        // Bumped by `tick()` only when it actually refreshed the cached
        // snapshot. QML bindings that read invokables backed by that
        // snapshot (`channels()`, `quality()`, `band()`, `waveform()`) read
        // `rev` too, purely to pick up its NOTIFY signal — that's what makes
        // them re-evaluate, since QML can't otherwise see that an invokable's
        // result changed.
        #[qproperty(i32, rev)]
        type CrownBridge = super::CrownBridgeRust;

        /// Pulls a snapshot and republishes it as Qt properties. Returns
        /// `true` when it actually refreshed (so QML knows the
        /// invokable-backed bindings are worth re-evaluating), `false` when
        /// nothing had changed since the last call.
        #[qinvokable]
        fn tick(self: Pin<&mut Self>, width: i32) -> bool;

        /// Starts the BLE supervisor. Safe to call more than once.
        #[qinvokable]
        fn start(self: Pin<&mut Self>);

        /// Channel names in device order, for QML to iterate against `quality`.
        #[qinvokable]
        fn channels(&self) -> QStringList;

        /// Contact-quality label for the channel at `channel`'s position in
        /// `channels()`.
        #[qinvokable]
        fn quality(&self, channel: i32) -> QString;

        /// Mean power across channels for the named band.
        #[qinvokable]
        fn band(&self, name: &QString) -> f64;

        /// One channel's decimated envelope as a polyline, scaled to `height`.
        #[qinvokable]
        fn waveform(&self, channel: i32, height: f64) -> QList_QPointF;

        /// Starts a recording against the currently configured device, or
        /// stops the active one if there is one.
        #[qinvokable]
        #[cxx_name = "toggleRecording"]
        fn toggle_recording(self: Pin<&mut Self>);

        /// Flips the raw-stream choice for the *next* session. Has no effect
        /// on a session already running — see the method's doc comment.
        #[qinvokable]
        #[cxx_name = "toggleRaw"]
        fn toggle_raw(self: Pin<&mut Self>);
    }
}

use core::pin::Pin;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QPointF, QString, QStringList};

use crown_core::auth::{Credentials, KeyringStore};
use crown_core::record::Recorder;
use crown_core::state::{ConnectionState, Live};

pub struct CrownBridgeRust {
    connection: QString,
    calm: f64,
    focus: f64,
    dropped: i32,
    staleness: i32,
    recording: QString,
    // Display only: what the raw button should currently read. Outcome
    // (`Snapshot::raw_enabled`) while a session is active, `raw_requested`
    // while idle — see `tick()`. Never read by `start()`; `raw_requested`
    // below is the actual source of truth for what the next session asks
    // for, so a stale outcome left behind by a session that already ended
    // (or never started) can never overwrite what the user asked for.
    raw: bool,
    active: bool,
    ready: bool,
    error: QString,
    rev: i32,
    live: Arc<Mutex<Live>>,
    recorder: Arc<Mutex<Option<Recorder>>>,
    runtime: Option<tokio::runtime::Runtime>,
    handle: Option<tokio::task::JoinHandle<()>>,
    // The user's actual pending choice for the *next* session, set only by
    // `toggle_raw`. This is what `start()` reads — never the `raw` property,
    // which `tick()` overwrites with the current session's outcome while
    // active, and would otherwise clobber a request the user made while an
    // earlier session's outcome was still sitting in `Live::raw_enabled`.
    raw_requested: bool,
    // Written by the spawned `supervise` task on a terminal error, read and
    // republished by `tick()` as the `error` property. A separate lock from
    // `live` rather than a new `Live` field: this is GUI presentation state
    // (a formatted message for a specific front end), not part of the
    // core-to-UI contract `Live`/`Snapshot` define.
    last_error: Arc<Mutex<Option<String>>>,
    // `None` until the first tick, distinct from a real revision (which
    // starts at 0 too) so the very first call always refreshes rather than
    // reading as "unchanged" by coincidence.
    last_rev: Option<u64>,
    snapshot: Option<crown_core::state::Snapshot>,
    // Names `quality()` has already warned about missing from the quality
    // map, so a persistent device-info/signal-quality mismatch is reported
    // once per name instead of once per tick.
    warned_missing_quality: RefCell<HashSet<String>>,
}

impl Default for CrownBridgeRust {
    fn default() -> Self {
        Self {
            connection: QString::from("Disconnected"),
            calm: 0.0,
            focus: 0.0,
            dropped: 0,
            staleness: -1,
            recording: QString::from(""),
            raw: false,
            active: false,
            ready: false,
            error: QString::from(""),
            rev: 0,
            live: Arc::new(Mutex::new(Live::new())),
            recorder: Arc::new(Mutex::new(None)),
            runtime: None,
            handle: None,
            last_error: Arc::new(Mutex::new(None)),
            raw_requested: false,
            last_rev: None,
            snapshot: None,
            warned_missing_quality: RefCell::new(HashSet::new()),
        }
    }
}

fn label(s: ConnectionState) -> &'static str {
    match s {
        ConnectionState::Disconnected => "Disconnected",
        ConnectionState::Scanning => "Scanning",
        ConnectionState::Connecting => "Connecting",
        ConnectionState::Authenticating => "Authenticating",
        ConnectionState::Streaming => "Streaming",
        ConnectionState::Reconnecting => "Reconnecting",
        ConnectionState::Failed => "Failed",
    }
}

impl qobject::CrownBridge {
    /// `Some(runtime)` used to mean "a session was ever started", which left
    /// Connect permanently inert after `supervise` returns on its own for a
    /// terminal auth error: the runtime was still `Some`, so clicking
    /// Connect again silently did nothing. The guard now keys on whether the
    /// spawned task is still running, not on whether one was ever spawned,
    /// so a finished session can be restarted.
    pub fn start(mut self: Pin<&mut Self>) {
        let already_running = self.handle.as_ref().is_some_and(|h| !h.is_finished());
        if already_running {
            return;
        }

        // A fresh attempt starts with a clean slate: any message from a
        // previous terminal failure no longer describes the current state.
        *crown_core::sync::lock(&self.last_error) = None;
        self.as_mut().set_error(QString::from(""));

        let creds = match Credentials::from_env() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("{e}");
                eprintln!("crown-qt: {msg}");
                self.as_mut().set_error(QString::from(msg));
                return;
            }
        };

        if self.runtime.is_none() {
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            self.as_mut().rust_mut().runtime = Some(runtime);
        }

        let live = self.live.clone();
        let store = Arc::new(KeyringStore {
            account: creds.email.clone(),
        });
        let recorder = self.recorder.clone();
        // Not `self.raw`: that property can currently be showing this
        // session's own outcome-in-progress (see the field's doc comment)
        // rather than the user's request. `raw_requested` is only ever
        // written by `toggle_raw`, so it survives a session that fails
        // before ever reaching the point where raw is attempted.
        let raw_enabled = self.raw_requested;
        let last_error = self.last_error.clone();
        // `supervise` never prints a terminal error itself (see its doc
        // comment) so that there is exactly one place a human sees it; for
        // this front end, that place is `last_error`, republished by
        // `tick()` as the `error` property, plus this stderr line for
        // anyone running the GUI from a terminal.
        let handle = self
            .runtime
            .as_ref()
            .expect("runtime just ensured present")
            .spawn(async move {
                if let Err(e) =
                    crown_core::backoff::supervise(live, creds, store, raw_enabled, recorder).await
                {
                    let msg = format!("{e:#}");
                    eprintln!("crown-qt: {msg}");
                    *crown_core::sync::lock(&last_error) = Some(msg);
                }
            });
        self.as_mut().rust_mut().handle = Some(handle);
    }

    pub fn tick(mut self: Pin<&mut Self>, width: i32) -> bool {
        // Checked unconditionally, ahead of the snapshot-unchanged early
        // return below: `error` is written by a different task on its own
        // schedule, not by anything that also bumps `Live::rev`, so tying
        // its refresh to "did the snapshot change" could leave a freshly
        // set message unpublished for however long `Live` next happens to
        // change (in the GUI's idle `Failed` state, `Live` may never change
        // again — the message would never appear). A plain property with
        // its own NOTIFY, so QML re-evaluates on the signal regardless of
        // what `tick()` returns; guarded by a compare so an unchanged ""
        // doesn't refire that signal every 33ms.
        let error = crown_core::sync::lock(&self.last_error)
            .clone()
            .unwrap_or_default();
        let error = QString::from(error);
        if error != *self.error() {
            self.as_mut().set_error(error);
        }

        // Building a Snapshot decimates every channel's ring; skip that
        // work entirely, not just the property writes, when nothing changed.
        let snap = {
            let live = crown_core::sync::lock(&self.live);
            if self.last_rev == Some(live.rev()) {
                return false;
            }
            live.snapshot(width.max(0) as usize)
        };
        self.as_mut().rust_mut().last_rev = Some(snap.rev);

        // Read everything the setters below need out of `snap` before it
        // moves into `self.snapshot`. Setters emit their NOTIFY signal
        // synchronously, so if `self.snapshot` were assigned after them, a
        // binding that reads a NOTIFY property and a snapshot-backed
        // invokable in the same evaluation could run in between and see
        // this tick's properties paired with last tick's snapshot.
        let connection = label(snap.connection);
        let calm = snap.calm as f64;
        let focus = snap.focus as f64;
        let dropped = snap.dropped_frames as i32;
        // Saturating: a session left running long enough to exceed i32
        // milliseconds (~24 days) should pin the display at "very stale"
        // rather than wrap into a small or negative number that reads as
        // healthy.
        let staleness = snap
            .metric_staleness_ms
            .map(|ms| ms.clamp(0, i32::MAX as i64) as i32)
            .unwrap_or(-1);
        // Derived from `Snapshot.recording`, like every other displayed
        // value, rather than cached separately: `Live::recording` is the
        // one place recording start/stop and the transport's clear-on-
        // failure path (see `ble::clear_recording_indicator`) both write,
        // so this is the only way the label can't disagree with reality.
        let recording = match &snap.recording {
            Some(dir) => QString::from(dir.display().to_string()),
            None => QString::from(""),
        };
        let active = snap.connection.is_active();
        let ready = snap.device_name.is_some();
        // While a session is active, `raw` publishes `Snapshot::raw_enabled`
        // — `true` for the life of the session unless the OSC listener's
        // bind failed, in which case it flips to `false` once. It means the
        // listener is running, not that samples are arriving: a device
        // that's powered on but not broadcasting still reads `true`.
        // Toggling the control (which QML disables while active anyway)
        // couldn't take effect either way. Before a session starts (or
        // after one ends), this instead republishes
        // `raw_requested` — never the outcome an *earlier* session may have
        // just written into this same property, which `start()` correctly
        // never reads but a human looking at the button still would.
        let raw = if active {
            snap.raw_enabled
        } else {
            self.raw_requested
        };

        self.as_mut().rust_mut().snapshot = Some(snap);

        self.as_mut().set_connection(QString::from(connection));
        self.as_mut().set_calm(calm);
        self.as_mut().set_focus(focus);
        self.as_mut().set_dropped(dropped);
        self.as_mut().set_staleness(staleness);
        self.as_mut().set_recording(recording);
        self.as_mut().set_active(active);
        self.as_mut().set_ready(ready);
        self.as_mut().set_raw(raw);
        let next_rev = self.rev().wrapping_add(1);
        self.as_mut().set_rev(next_rev);
        true
    }

    pub fn channels(&self) -> QStringList {
        let mut list = QStringList::default();
        if let Some(s) = &self.snapshot {
            for name in &s.channel_names {
                list.append(QString::from(name));
            }
        }
        list
    }

    /// All three failure paths render the same "unknown" grey tile in QML —
    /// deliberately, since the display shouldn't invent a fake status — but
    /// each is a distinct failure with a distinct cause, so each is reported
    /// to stderr separately rather than collapsed into one silent grey.
    pub fn quality(&self, channel: i32) -> QString {
        let Some(s) = &self.snapshot else {
            eprintln!("crown-qt: quality({channel}) called before any snapshot arrived");
            return QString::from("unknown");
        };
        let Some(idx) = usize::try_from(channel).ok() else {
            eprintln!("crown-qt: quality() got a negative channel index: {channel}");
            return QString::from("unknown");
        };
        let Some(name) = s.channel_names.get(idx) else {
            eprintln!(
                "crown-qt: quality({channel}) is out of range for {} channels",
                s.channel_names.len()
            );
            return QString::from("unknown");
        };
        match s.quality.get(name) {
            Some(q) => QString::from(format!("{:?}", q.status)),
            None => {
                // Reachable in practice, unlike the two paths above: the
                // device-info and signal-quality streams name channels
                // independently and can race or disagree. Warn once per
                // name rather than every tick, so a real mismatch is
                // diagnosable without flooding stderr at tick rate.
                if self
                    .warned_missing_quality
                    .borrow_mut()
                    .insert(name.clone())
                {
                    eprintln!(
                        "crown-qt: channel '{name}' is in device-info but has no entry in the quality map"
                    );
                }
                QString::from("unknown")
            }
        }
    }

    /// Mean power across channels for one band. QML asks by name so adding a
    /// band later needs no bridge change.
    pub fn band(&self, name: &QString) -> f64 {
        let Some(s) = &self.snapshot else { return 0.0 };
        let Some(b) = &s.bands else { return 0.0 };
        let values = match name.to_string().as_str() {
            "delta" => &b.delta,
            "theta" => &b.theta,
            "alpha" => &b.alpha,
            "beta" => &b.beta,
            "gamma" => &b.gamma,
            _ => return 0.0,
        };
        if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        }
    }

    /// One channel's decimated envelope as a polyline. Each column contributes
    /// two points (min then max), so the zigzag paints a filled-looking trace.
    /// Y is pre-scaled here because QML has no cheap way to map thousands of
    /// points itself.
    ///
    /// The scale is shared across every channel (recomputed from
    /// `s.waveform` on each call), not taken from this channel's own
    /// min/max: a per-channel scale makes a quiet channel's noise fill the
    /// height and makes amplitude incomparable between electrodes, which
    /// defeats the point of a multi-electrode display. One shared scale
    /// means a loud channel compresses the others instead — the correct
    /// trade for spotting which electrode looks wrong.
    pub fn waveform(&self, channel: i32, height: f64) -> QList<QPointF> {
        let mut out = QList::<QPointF>::default();
        let Some(s) = &self.snapshot else { return out };
        let Some(idx) = usize::try_from(channel).ok() else {
            return out;
        };
        let Some(column) = s.waveform.get(idx) else {
            return out;
        };
        if column.is_empty() {
            return out;
        }

        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for col in &s.waveform {
            for &(min, max) in col {
                lo = lo.min(min);
                hi = hi.max(max);
            }
        }
        // Degenerate when nothing finite was seen (shouldn't happen, since
        // `column` above is non-empty and is itself part of `s.waveform`)
        // or every sample across every channel is identical (`hi <= lo`).
        // Either way there is nothing to scale against, so center the
        // trace rather than dividing by zero or producing NaN.
        let span = (hi - lo) as f64;
        let degenerate = !span.is_finite() || span <= 0.0;

        let map = |v: f32| -> f64 {
            if degenerate {
                height / 2.0
            } else {
                height - ((v - lo) as f64 / span) * height
            }
        };
        for (x, &(min, max)) in column.iter().enumerate() {
            out.append(QPointF::new(x as f64, map(min)));
            out.append(QPointF::new(x as f64, map(max)));
        }
        out
    }

    /// Stops the active recording if one is running, otherwise starts one
    /// against the device `Live` currently has configured.
    ///
    /// Never holds `recorder`'s lock and `live`'s lock at the same time —
    /// each is fully acquired and released before the other is ever taken,
    /// the same discipline `ble::run` documents and depends on. This
    /// doesn't just set the bridge's own `recording` property: it writes
    /// `Live::recording` itself, since that's the one field `tick()` reads
    /// to publish it (see `tick`'s comment) and the one field the transport
    /// clears on a disk-write failure. Writing anywhere else would let the
    /// two disagree.
    ///
    /// Ordering rule, same on both paths even though they run it in
    /// opposite directions: whichever of the two facts (`Live::recording`,
    /// the `recorder` slot) is made *false* second, and made *true* first,
    /// is the one a racing transport thread could act on and disagree with.
    /// So stop clears the private fact (`recorder`) first, then the public
    /// one (`Live::recording`) — a transport thread that squeezes a write
    /// in between still writes to a real `Recorder` that's simply about to
    /// be dropped, harmless. Start publishes the public fact
    /// (`Live::recording`) first, then installs the private one
    /// (`recorder`) — see that branch's comment for why the reverse order
    /// there can latch a lie permanently.
    pub fn toggle_recording(self: Pin<&mut Self>) {
        let mut slot = crown_core::sync::lock(&self.recorder);
        if slot.is_some() {
            *slot = None; // dropping flushes and closes both writers
            drop(slot);
            let mut live = crown_core::sync::lock(&self.live);
            live.recording = None;
            live.touch();
            return;
        }
        drop(slot);

        let info = {
            let live = crown_core::sync::lock(&self.live);
            live.device.clone()
        };
        let Some(info) = info else {
            eprintln!("crown-qt: no device info yet, can't start a recording");
            return;
        };

        let root = dirs_home().join("CrownSessions");
        let name = Recorder::session_name();
        match Recorder::start(&root, &info, &name) {
            Ok(rec) => {
                let dir = rec.dir().to_path_buf();
                // Publish `Live::recording` *before* installing the
                // recorder, not after: if the recorder were installed
                // first, a transport thread could fail its very first
                // write in the gap before the line below runs, clear the
                // (already-installed) recorder slot back to `None`, and
                // call `clear_recording_indicator` — which this line would
                // then overwrite with `Some(dir)` right afterward. That
                // indicator would never self-correct, since every recorder
                // write path only ever touches a `Some` slot, and the slot
                // would be `None` from then on. With `Live::recording` set
                // first, the transport can only ever observe "recording, no
                // recorder yet" (which it silently no-ops on) — never
                // "recorder failed, indicator stuck on".
                let mut live = crown_core::sync::lock(&self.live);
                live.recording = Some(dir);
                live.touch();
                drop(live);
                *crown_core::sync::lock(&self.recorder) = Some(rec);
            }
            Err(e) => eprintln!("crown-qt: failed to start recording: {e}"),
        }
    }

    /// Flips the raw-stream choice for the *next* session.
    ///
    /// `supervise` takes `raw_enabled` by value and reads it once, before
    /// the reconnect loop starts, to decide whether the OSC listener runs
    /// for the life of that session; it is never re-read. So this has no
    /// effect on a session already running — QML disables the control while
    /// connected so that's never a surprise, rather than something a user
    /// discovers by watching a flat trace.
    pub fn toggle_raw(mut self: Pin<&mut Self>) {
        // QML only allows this while idle (`enabled: !crown.active`), so
        // `raw`'s displayed value is already the pending choice here, not
        // an active session's outcome — but `raw_requested`, not `raw`, is
        // still the one write that matters: it is what `start()` reads.
        let next = !self.raw_requested;
        self.as_mut().rust_mut().raw_requested = next;
        self.as_mut().set_raw(next);
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
