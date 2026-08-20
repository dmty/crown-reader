use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::auth::{Credentials, TokenStore};
use crate::record::Recorder;
use crate::state::{ConnectionState, Live};

/// 1s, 2s, 4s, 8s, 16s, then 30s forever.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_secs(secs.min(30))
}

/// Runs the BLE session forever, reconnecting with backoff.
///
/// `ble::run` returns `Ok(())` only via a clean disconnect (leaving
/// `Live::connection == Disconnected`, set by `run` itself) and `Err` only
/// via a hard failure before `Streaming` was ever reached (`run` sets
/// `Failed` on any error path before returning it — see its doc comment).
/// Once `Streaming` is reached, `run` can only return by disconnecting, so
/// `Streaming` is never the observed state here: checking the `Result` is
/// the direct signal and reading `Live::connection` afterward would only
/// recover the same information one step removed. So the delay resets on
/// `Ok`, not on inferring "reached streaming" from state: this still means
/// a session that drops before deviceInfo ever arrives resets the counter
/// too, but that is a real (if early) disconnect, not the kind of
/// persistent problem (bad credentials, no adapter, device unreachable)
/// that `Err` represents — and only `Err` climbs the backoff.
pub async fn supervise(
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    raw_enabled: bool,
    recorder: Arc<Mutex<Option<Recorder>>>,
) {
    let mut attempt = 0u32;
    loop {
        // Credentials deliberately has no Clone (it holds a password); clone
        // field by field instead.
        let creds_clone = Credentials {
            email: creds.email.clone(),
            password: creds.password.clone(),
            device_id: creds.device_id.clone(),
        };
        let result = crate::ble::run(
            live.clone(),
            creds_clone,
            store.clone(),
            raw_enabled,
            recorder.clone(),
        )
        .await;

        // Logged before the state flips to Reconnecting below, so a human
        // watching stderr can tell a failure-triggered retry from a clean-
        // disconnect retry even though both leave the same Reconnecting
        // state behind for anything polling Live::connection.
        match &result {
            Ok(()) => eprintln!("session ended: device disconnected, reconnecting"),
            Err(e) => eprintln!("session ended: {e:#}, reconnecting"),
        }

        {
            let mut l = live.lock().unwrap();
            l.connection = ConnectionState::Reconnecting;
            l.touch();
        }

        attempt = if result.is_ok() { 0 } else { attempt + 1 };
        tokio::time::sleep(backoff_delay(attempt)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_one_second() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
    }

    #[test]
    fn caps_at_thirty_seconds() {
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(50), Duration::from_secs(30));
    }
}
