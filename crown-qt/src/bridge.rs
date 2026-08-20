#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
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
    }
}

use core::pin::Pin;
use std::sync::{Arc, Mutex};

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;

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
    }
}
