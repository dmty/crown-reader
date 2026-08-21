use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::auth::{AuthError, Credentials, TokenStore};
use crate::ble::DeviceRejectedToken;
use crate::record::Recorder;
use crate::state::{ConnectionState, Live};

/// 1s, 2s, 4s, 8s, 16s, then 30s forever.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_secs(secs.min(30))
}

/// Advances the retry-attempt counter and returns how long to wait before
/// the next attempt. `should_reset` reflects whether the run that just
/// ended proved the link works (see `supervise`'s reset rule).
///
/// The returned delay always corresponds to `*attempt` *before* it is
/// incremented: an earlier version of this loop incremented first and slept
/// second, which meant the first retry after a fresh failure waited
/// `backoff_delay(1)` == 2s instead of the spec's 1s, and `backoff_delay(0)`
/// was reachable only through the reset branch. Pulling this into its own
/// function makes that sequencing a thing the test below can pin down
/// without running the supervisor loop at all.
fn next_delay(attempt: &mut u32, should_reset: bool) -> Duration {
    if should_reset {
        *attempt = 0;
    }
    let delay = backoff_delay(*attempt);
    if !should_reset {
        *attempt += 1;
    }
    delay
}

/// A session must have streamed for at least this long, not merely
/// *run* for this long, before a clean disconnect resets the backoff.
///
/// This measures `Live::streaming_since` (set by `ble::run` only on the
/// transition into `Streaming`), not the run's whole wall time. Whole wall
/// time was tried first and was wrong in both directions: `find_crown` alone
/// can take up to ~10s, so a run that spent most of its time scanning and
/// only streamed briefly would still reset (never escalating against a link
/// that connects but can't hold), while a run that scanned quickly and then
/// streamed solidly for, say, 8s would *not* reset merely because 8s is
/// under a wall-clock floor chosen to be safely above scan time — pinning a
/// perfectly good link at the backoff ceiling forever. Gating on time spent
/// in `Streaming` specifically fixes both: scan time never counts for or
/// against the reset, only actual streaming time does.
const MIN_STREAMING_FOR_RESET: Duration = Duration::from_secs(10);

/// Whether a given `AuthError` means retrying is pointless: the same input
/// (env, credentials) will fail the same way every time, so looping only
/// burns cycles and, for `Remote`, hammers the identity service.
///
/// - `MissingEnv`: an env var absent at process start will not appear
///   mid-run. Terminal.
/// - `Remote` and `Malformed`: by the time either reaches this function,
///   `auth::reclassify_by_status` has already sorted them by HTTP status
///   *class*, not by which of the two variants the body happened to
///   produce — see that function's doc comment for the full rule. In
///   short: a `Remote` or `Malformed` a caller sees here always arrived on
///   a 2xx or a non-429 4xx, i.e. the server either answered successfully
///   and we still couldn't use it, or told us the request itself was
///   wrong. Neither improves by retrying the same request. Terminal. (A
///   429 or 5xx — rate limiting, a cold-started function, any other
///   server-side failure — is reclassified to `Http` before it gets here,
///   precisely so it is *not* terminal.)
/// - `Http`: a transport failure, a status this project cannot otherwise
///   explain, or one of the reclassified 429/5xx cases above. The
///   underlying condition can resolve on its own. Transient.
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
/// `ble::run` actually returns. Two typed causes are terminal:
/// - an `AuthError` for which [`is_terminal`] says so, or
/// - `DeviceRejectedToken`: the device itself refused the Bluetooth token
///   twice in a row, which is not an `AuthError` (it never touches the
///   identity service) but is exactly as unfixable by retrying.
///
/// Anything else — a Bluetooth/adapter failure, a scan timeout — is
/// transient: the headset can be turned back on or brought back into range.
fn error_is_terminal(err: &anyhow::Error) -> bool {
    err.downcast_ref::<AuthError>().is_some_and(is_terminal)
        || err.downcast_ref::<DeviceRejectedToken>().is_some()
}

/// Runs the BLE session forever, reconnecting with backoff, until a
/// terminal error ends it for good.
///
/// `ble::run` returns `Ok(())` only via a clean disconnect (leaving
/// `Live::connection == Disconnected`, set by `run` itself) and `Err` only
/// via a hard failure before `Streaming` was ever reached (`run` sets
/// `Failed` on any error path before returning it — see its doc comment).
/// Once `Streaming` is reached, `run` can only return by disconnecting, so
/// `Streaming` is never the observed state here: checking the `Result` is
/// the direct signal and reading `Live::connection` afterward would only
/// recover the same information one step removed. `Err` always climbs the
/// backoff. `Ok` resets it too, but only if `Live::streaming_since` shows
/// the run actually streamed for at least `MIN_STREAMING_FOR_RESET` — see
/// that constant's doc comment for why.
///
/// A terminal error (see [`error_is_terminal`]) is the one case that does
/// not loop: it sets `Failed` (not `Reconnecting`) and returns the error
/// instead, since retrying a bad password, a missing env var, or a token
/// the device itself refuses cannot succeed and would otherwise hammer the
/// identity service forever. The caller (the CLI today) is responsible for
/// reporting that error and exiting non-zero — `supervise` itself does not
/// print it, so there is exactly one place a human sees the message rather
/// than two.
///
/// The `Adapter` is built once, here, before the loop — not inside `ble::run`
/// on every attempt. `ble::first_adapter`'s doc comment has the leak this
/// avoids: btleplug 0.12's CoreBluetooth backend spawns a thread and a
/// `CBCentralManager` per `Adapter` with no teardown, so building a fresh one
/// per reconnect leaked both on every retry. Building it here means a
/// missing/disabled adapter is a hard failure of `supervise` itself rather
/// than something the backoff loop retries forever — unlike a headset that's
/// merely off or out of range, there is nothing about waiting 30 more
/// seconds that makes an adapter appear, and the previous "retry forever"
/// behavior was incidental to where the code happened to live rather than a
/// deliberate choice. `Failed` is set the same way the terminal-error branch
/// below sets it, so a caller watching `Live::connection` sees a consistent
/// picture regardless of which failure path ended the session.
pub async fn supervise(
    live: Arc<Mutex<Live>>,
    creds: Credentials,
    store: Arc<dyn TokenStore>,
    _raw_enabled: bool,
    recorder: Arc<Mutex<Option<Recorder>>>,
) -> anyhow::Result<()> {
    let adapter = match crate::ble::first_adapter().await {
        Ok(a) => a,
        Err(e) => {
            let mut l = crate::sync::lock(&live);
            l.connection = ConnectionState::Failed;
            l.touch();
            return Err(e);
        }
    };

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
            &adapter,
            live.clone(),
            creds_clone,
            store.clone(),
            recorder.clone(),
        )
        .await;

        if result.as_ref().is_err_and(error_is_terminal) {
            let mut l = crate::sync::lock(&live);
            l.connection = ConnectionState::Failed;
            l.touch();
            return Err(result.unwrap_err());
        }

        let streamed_long_enough = {
            let l = crate::sync::lock(&live);
            l.streaming_since.is_some_and(|since| since.elapsed() >= MIN_STREAMING_FOR_RESET)
        };

        // Logged before the state flips to Reconnecting below, so a human
        // watching stderr can tell a failure-triggered retry from a clean-
        // disconnect retry even though both leave the same Reconnecting
        // state behind for anything polling Live::connection.
        match &result {
            Ok(()) => eprintln!("session ended: device disconnected, reconnecting"),
            Err(e) => eprintln!("session ended: {e:#}, reconnecting"),
        }

        {
            let mut l = crate::sync::lock(&live);
            l.connection = ConnectionState::Reconnecting;
            l.touch();
        }

        let should_reset = result.is_ok() && streamed_long_enough;
        tokio::time::sleep(next_delay(&mut attempt, should_reset)).await;
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
    fn the_retry_sequence_starts_at_one_second_and_doubles_to_the_cap() {
        let mut attempt = 0u32;
        let delays: Vec<Duration> = (0..7).map(|_| next_delay(&mut attempt, false)).collect();
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
                Duration::from_secs(30),
            ]
        );
    }

    #[test]
    fn a_reset_drops_the_next_delay_back_to_one_second_and_the_sequence_restarts() {
        let mut attempt = 0u32;
        for _ in 0..4 {
            next_delay(&mut attempt, false);
        }
        // Without a reset the next delay would be backoff_delay(4) == 16s.
        assert_eq!(next_delay(&mut attempt, true), Duration::from_secs(1));
        // The reset put the streak back at 0, so the next non-resetting
        // outcome is the *first* failure of a fresh streak and gets the
        // same 1s delay any first failure gets -- not 2s, which would only
        // be right if the reset call had already used up that first slot.
        assert_eq!(next_delay(&mut attempt, false), Duration::from_secs(1));
        assert_eq!(next_delay(&mut attempt, false), Duration::from_secs(2));
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
    fn a_device_side_token_rejection_is_terminal() {
        let err: anyhow::Error = DeviceRejectedToken.into();
        assert!(error_is_terminal(&err));
    }

    #[test]
    fn a_non_auth_error_is_treated_as_transient() {
        // e.g. a Bluetooth/adapter failure from `ble::run`, which has
        // nothing to do with `AuthError` or `DeviceRejectedToken` at all:
        // the headset can be turned back on or brought back into range.
        let err = anyhow::anyhow!("no Bluetooth adapter available");
        assert!(!error_is_terminal(&err));
    }
}
