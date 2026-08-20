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
        #[qproperty(QString, recording)]
        #[qproperty(bool, raw)]
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
    recording: QString,
    raw: bool,
    rev: i32,
    live: Arc<Mutex<Live>>,
    recorder: Arc<Mutex<Option<Recorder>>>,
    runtime: Option<tokio::runtime::Runtime>,
    handle: Option<tokio::task::JoinHandle<()>>,
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
            recording: QString::from(""),
            raw: false,
            rev: 0,
            live: Arc::new(Mutex::new(Live::new())),
            recorder: Arc::new(Mutex::new(None)),
            runtime: None,
            handle: None,
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

        let creds = match Credentials::from_env() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("crown-qt: {e}");
                self.as_mut().set_connection(QString::from(format!("{e}")));
                return;
            }
        };

        if self.runtime.is_none() {
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            self.as_mut().rust_mut().runtime = Some(runtime);
        }

        let live = self.live.clone();
        let store = Arc::new(KeyringStore { account: creds.email.clone() });
        let recorder = self.recorder.clone();
        let raw_enabled = self.raw;
        // `supervise` returns `anyhow::Result<()>`; the bridge only ever
        // needs to know whether the task is still running (`is_finished`),
        // not why it ended, so the result is discarded here rather than
        // pulling `anyhow` into this crate just to name the type.
        let handle = self.runtime.as_ref().expect("runtime just ensured present").spawn(
            async move {
                let _ =
                    crown_core::backoff::supervise(live, creds, store, raw_enabled, recorder)
                        .await;
            },
        );
        self.as_mut().rust_mut().handle = Some(handle);
    }

    pub fn tick(mut self: Pin<&mut Self>, width: i32) -> bool {
        // Building a Snapshot decimates every channel's ring; skip that
        // work entirely, not just the property writes, when nothing changed.
        let snap = {
            let live = self.live.lock().unwrap();
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
        // Derived from `Snapshot.recording`, like every other displayed
        // value, rather than cached separately: `Live::recording` is the
        // one place recording start/stop and the transport's clear-on-
        // failure path (see `ble::clear_recording_indicator`) both write,
        // so this is the only way the label can't disagree with reality.
        let recording = match &snap.recording {
            Some(dir) => QString::from(dir.display().to_string()),
            None => QString::from(""),
        };

        self.as_mut().rust_mut().snapshot = Some(snap);

        self.as_mut().set_connection(QString::from(connection));
        self.as_mut().set_calm(calm);
        self.as_mut().set_focus(focus);
        self.as_mut().set_dropped(dropped);
        self.as_mut().set_recording(recording);
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
                if self.warned_missing_quality.borrow_mut().insert(name.clone()) {
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
        let Some(idx) = usize::try_from(channel).ok() else { return out };
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
        let mut slot = self.recorder.lock().unwrap();
        if slot.is_some() {
            *slot = None; // dropping flushes and closes both writers
            drop(slot);
            let mut live = self.live.lock().unwrap();
            live.recording = None;
            live.touch();
            return;
        }
        drop(slot);

        let info = {
            let live = self.live.lock().unwrap();
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
                // indicator would never self-correct, since
                // `record_raw_samples`/`record_derived_line` only act on a
                // `Some` slot, and the slot would be `None` from then on.
                // With `Live::recording` set first, the transport can only
                // ever observe "recording, no recorder yet" (which it
                // silently no-ops on) — never "recorder failed, indicator
                // stuck on".
                let mut live = self.live.lock().unwrap();
                live.recording = Some(dir);
                live.touch();
                drop(live);
                *self.recorder.lock().unwrap() = Some(rec);
            }
            Err(e) => eprintln!("crown-qt: failed to start recording: {e}"),
        }
    }

    /// Flips the raw-stream choice for the *next* session.
    ///
    /// `supervise` takes `raw_enabled` by value and reads it once, at spawn
    /// time, to fix the BLE subscription set for the life of that session;
    /// it is never re-read. So this has no effect on a session already
    /// running — QML disables the control while connected so that's never a
    /// surprise, rather than something a user discovers by watching a flat
    /// trace.
    pub fn toggle_raw(mut self: Pin<&mut Self>) {
        let next = !*self.raw();
        self.as_mut().set_raw(next);
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
