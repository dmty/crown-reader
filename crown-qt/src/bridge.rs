#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, connection)]
        #[qproperty(f64, calm)]
        #[qproperty(f64, focus)]
        #[qproperty(i32, dropped)]
        type CrownBridge = super::CrownBridgeRust;

        /// Pulls a snapshot and republishes it as Qt properties.
        #[qinvokable]
        fn tick(self: Pin<&mut Self>, width: i32);

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
    }
}

use core::pin::Pin;
use std::sync::{Arc, Mutex};

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QString, QStringList};

use crown_core::auth::{Credentials, KeyringStore};
use crown_core::state::{ConnectionState, Live};

pub struct CrownBridgeRust {
    connection: QString,
    calm: f64,
    focus: f64,
    dropped: i32,
    live: Arc<Mutex<Live>>,
    runtime: Option<tokio::runtime::Runtime>,
    handle: Option<tokio::task::JoinHandle<()>>,
    last_rev: u64,
    snapshot: Option<crown_core::state::Snapshot>,
}

impl Default for CrownBridgeRust {
    fn default() -> Self {
        Self {
            connection: QString::from("Disconnected"),
            calm: 0.0,
            focus: 0.0,
            dropped: 0,
            live: Arc::new(Mutex::new(Live::new())),
            runtime: None,
            handle: None,
            last_rev: 0,
            snapshot: None,
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
        let recorder = Arc::new(Mutex::new(None));
        // `supervise` returns `anyhow::Result<()>`; the bridge only ever
        // needs to know whether the task is still running (`is_finished`),
        // not why it ended, so the result is discarded here rather than
        // pulling `anyhow` into this crate just to name the type.
        let handle = self.runtime.as_ref().expect("runtime just ensured present").spawn(
            async move {
                let _ = crown_core::backoff::supervise(live, creds, store, false, recorder).await;
            },
        );
        self.as_mut().rust_mut().handle = Some(handle);
    }

    pub fn tick(mut self: Pin<&mut Self>, width: i32) {
        // Building a Snapshot decimates every channel's ring; skip that
        // work entirely, not just the property writes, when nothing changed.
        let snap = {
            let live = self.live.lock().unwrap();
            if live.rev() == self.last_rev {
                return;
            }
            live.snapshot(width.max(0) as usize)
        };
        self.as_mut().rust_mut().last_rev = snap.rev;
        self.as_mut().set_connection(QString::from(label(snap.connection)));
        self.as_mut().set_calm(snap.calm as f64);
        self.as_mut().set_focus(snap.focus as f64);
        self.as_mut().set_dropped(snap.dropped_frames as i32);
        self.as_mut().rust_mut().snapshot = Some(snap);
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

    pub fn quality(&self, channel: i32) -> QString {
        let Some(s) = &self.snapshot else {
            return QString::from("unknown");
        };
        let Some(name) = s.channel_names.get(channel.max(0) as usize) else {
            return QString::from("unknown");
        };
        match s.quality.get(name) {
            Some(q) => QString::from(&format!("{:?}", q.status)),
            None => QString::from("unknown"),
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
}
