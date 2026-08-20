use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::auth::{AuthError, Credentials, TokenStore};
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

/// Whether a given `AuthError` means retrying is pointless: the same input
/// (env, credentials) will fail the same way every time, so looping only
/// burns cycles and, for `Remote`, hammers the identity service.
///
/// - `MissingEnv`: an env var absent at process start will not appear
///   mid-run. Terminal.
/// - `Remote`: the identity service actively rejected the request (wrong
///   password, unknown email, ...). No retry fixes a bad credential.
///   Terminal.
/// - `Malformed`: the response parsed as HTTP but not into the shape we
///   expect. This is a contract mismatch (wrong endpoint/region, an API
///   change) rather than noise — the same well-formed response will fail
///   to parse the same way every time. Terminal.
/// - `Http`: a transport failure. The network can come back on its own.
///   Transient.
/// - `Store`: a token-cache read/write failure. Not actually reachable
///   through `token()` today — `TokenStore::load`/`save` failures are
///   swallowed to a warning before they become an `AuthError` a caller
///   sees — but if that ever changes: it describes local system state
///   (locked keychain, a permissions hiccup), which can clear up without
///   user action. Transient.
fn is_terminal(err: &AuthError) -> bool {
    matches!(err, AuthError::MissingEnv(_) | AuthError::Remote(_) | AuthError::Malformed(_))
}

/// Same question as [`is_terminal`], for the `anyhow::Error` shape
/// `ble::run` actually returns. Anything that isn't an `AuthError` at all —
/// a Bluetooth/adapter failure, say — is transient: the headset can be
/// turned back on or brought back into range.
fn error_is_terminal(err: &anyhow::Error) -> bool {
    err.downcast_ref::<AuthError>().is_some_and(is_terminal)
}

/// Runs the BLE session forever, reconnecting with backoff, until a
/// terminal auth error ends it for good.
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
///
/// A terminal error (see [`is_terminal`]) is the one case that does not
/// loop: it sets `Failed` (not `Reconnecting`), names the actual problem on
/// stderr, and returns, since retrying a bad password or a missing env var
/// cannot succeed and would otherwise hammer the identity service forever.
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

        if let Err(e) = &result {
            if error_is_terminal(e) {
                eprintln!("session failed: {e:#}");
                let mut l = live.lock().unwrap();
                l.connection = ConnectionState::Failed;
                l.touch();
                return;
            }
        }

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

    #[test]
    fn missing_env_var_is_terminal() {
        assert!(is_terminal(&AuthError::MissingEnv("NEUROSITY_PASSWORD".into())));
    }

    #[test]
    fn a_rejection_from_the_identity_service_is_terminal() {
        assert!(is_terminal(&AuthError::Remote("INVALID_LOGIN_CREDENTIALS".into())));
    }

    #[test]
    fn a_response_that_does_not_parse_as_expected_is_terminal() {
        assert!(is_terminal(&AuthError::Malformed("no idToken field".into())));
    }

    #[test]
    fn a_transport_failure_is_transient() {
        assert!(!is_terminal(&AuthError::Http("connection reset".into())));
    }

    #[test]
    fn a_token_store_failure_is_transient() {
        assert!(!is_terminal(&AuthError::Store("keychain locked".into())));
    }

    #[test]
    fn an_auth_error_wrapped_in_anyhow_is_still_classified() {
        let err: anyhow::Error = AuthError::Remote("INVALID_LOGIN_CREDENTIALS".into()).into();
        assert!(error_is_terminal(&err));
    }

    #[test]
    fn a_non_auth_error_is_treated_as_transient() {
        // e.g. a Bluetooth/adapter failure from `ble::run`, which has
        // nothing to do with `AuthError` at all: the headset can be turned
        // back on or brought back into range.
        let err = anyhow::anyhow!("no Bluetooth adapter available");
        assert!(!error_is_terminal(&err));
    }
}
