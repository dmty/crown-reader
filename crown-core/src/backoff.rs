use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::auth::{Credentials, TokenStore};
use crate::record::Recorder;
use crate::state::{ConnectionState, Live};

/// 1s, 2s, 4s, 8s, 16s, then 30s forever.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_secs(secs.min(30))
}

/// A run shorter than this is treated as a failure to reconnect even when it
/// returns `Ok`: a device that connects, authenticates, and then drops the
/// link right away would otherwise reset `attempt` to 0 every cycle and get
/// retried every ~1-2s forever — the "always resets hammers a broken device"
/// case, just laundered through a clean disconnect instead of an error. This
/// is comfortably above what a real connect+auth round trip costs, so a
/// session that reaches `Streaming` and runs for any meaningful time still
/// resets the delay.
const MIN_SESSION_FOR_RESET: Duration = Duration::from_secs(10);

/// Runs the BLE session forever, reconnecting with backoff.
///
/// `ble::run` returns `Ok(())` only via a clean disconnect (leaving
/// `Live::connection == Disconnected`, set by `run` itself) and `Err` only
/// via a hard failure before `Streaming` was ever reached (`run` sets
/// `Failed` on any error path before returning it — see its doc comment).
/// Once `Streaming` is reached, `run` can only return by disconnecting, so
/// `Streaming` is never the observed state here: checking the `Result` is
/// the direct signal and reading `Live::connection` afterward would only
/// recover the same information one step removed. `Err` always climbs the
/// backoff. `Ok` resets it too, but only if the run lasted at least
/// `MIN_SESSION_FOR_RESET` — see that constant's doc comment for why a bare
/// `Ok` is not enough on its own.
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
        let started = Instant::now();
        let result = crate::ble::run(
            live.clone(),
            creds_clone,
            store.clone(),
            raw_enabled,
            recorder.clone(),
        )
        .await;
        let ran_long_enough = started.elapsed() >= MIN_SESSION_FOR_RESET;

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

        attempt = if result.is_ok() && ran_long_enough { 0 } else { attempt + 1 };
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
