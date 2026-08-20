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
    pub fn start(mut self: Pin<&mut Self>) {
        if self.runtime.is_some() {
            return;
        }
        let creds = match Credentials::from_env() {
            Ok(c) => c,
            Err(e) => {
                self.as_mut().set_connection(QString::from(format!("{e}")));
                return;
            }
        };
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let live = self.live.clone();
        let store = Arc::new(KeyringStore { account: creds.email.clone() });
        let recorder = Arc::new(Mutex::new(None));
        runtime.spawn(crown_core::backoff::supervise(live, creds, store, false, recorder));
        self.as_mut().rust_mut().runtime = Some(runtime);
    }

    pub fn tick(mut self: Pin<&mut Self>, width: i32) {
        let snap = {
            let live = self.live.lock().unwrap();
            live.snapshot(width.max(0) as usize)
        };
        if snap.rev == self.last_rev {
            return;
        }
        self.as_mut().rust_mut().last_rev = snap.rev;
        self.as_mut().set_connection(QString::from(label(snap.connection)));
        self.as_mut().set_calm(snap.calm as f64);
        self.as_mut().set_focus(snap.focus as f64);
        self.as_mut().set_dropped(snap.dropped_frames as i32);
    }
}
