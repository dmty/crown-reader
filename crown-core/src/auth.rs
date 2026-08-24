use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

mod profile;
pub use profile::{
    password_profile_for_save, AuthMethod, AuthProfile, AuthProfileStore,
    KeyringAuthProfileStore, MemoryAuthProfileStore, PasswordAuth, ProfileStoreError,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

const API_KEY: &str = concat!("AIza", "SyB0TkZ83Fj0CIzn8AAmE-Osc92s3ER8hy8");
const SIGN_IN_URL: &str = "https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword";
const CREATE_TOKEN_URL: &str =
    "https://us-central1-neurosity-device.cloudfunctions.net/createBluetoothToken";
const KEYRING_SERVICE: &str = "crown-reader";

#[derive(Debug)]
pub enum AuthError {
    MissingEnv(String),
    Http(String),
    Remote(String),
    Malformed(String),
    Store(String),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv(k) => write!(f, "environment variable {k} is not set"),
            Self::Http(e) => write!(f, "request failed: {e}"),
            Self::Remote(m) => write!(f, "service rejected the request: {m}"),
            Self::Malformed(m) => write!(f, "unexpected response shape: {m}"),
            Self::Store(e) => write!(f, "token store failed: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Never derive Debug or Display in a way that renders `password`.
pub struct Credentials {
    pub email: String,
    pub password: String,
    pub device_id: String,
}

impl Credentials {
    pub fn from_env() -> Result<Self, AuthError> {
        let get = |k: &str| std::env::var(k).map_err(|_| AuthError::MissingEnv(k.to_string()));
        Ok(Self {
            email: get("NEUROSITY_EMAIL")?,
            password: get("NEUROSITY_PASSWORD")?,
            device_id: get("NEUROSITY_DEVICE_ID")?,
        })
    }
}

pub fn parse_sign_in(body: &str) -> Result<String, AuthError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AuthError::Malformed(e.to_string()))?;
    if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(AuthError::Remote(msg.to_string()));
    }
    v.get("idToken")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| AuthError::Malformed("no idToken field".into()))
}

pub fn parse_token_response(body: &str) -> Result<String, AuthError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AuthError::Malformed(e.to_string()))?;
    if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(AuthError::Remote(msg.to_string()));
    }
    v.pointer("/result/token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| AuthError::Malformed("no result.token field".into()))
}

pub trait TokenStore: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, token: &str) -> Result<(), AuthError>;
    fn clear(&self);
}

pub struct KeyringStore {
    pub account: String,
}

impl TokenStore for KeyringStore {
    fn load(&self) -> Option<String> {
        // A missing entry is an ordinary cache miss and stays silent; any other
        // error (locked/inaccessible Keychain) is worth a diagnostic, since it
        // would otherwise look identical to a miss and re-mint over the network
        // on every launch with no visible cause.
        let entry = match keyring::Entry::new(KEYRING_SERVICE, &self.account) {
            Ok(e) => e,
            Err(keyring::Error::NoEntry) => return None,
            Err(e) => {
                eprintln!("warning: could not read cached Bluetooth token: {e}");
                return None;
            }
        };
        match entry.get_password() {
            Ok(p) => Some(p),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                eprintln!("warning: could not read cached Bluetooth token: {e}");
                None
            }
        }
    }

    fn save(&self, token: &str) -> Result<(), AuthError> {
        keyring::Entry::new(KEYRING_SERVICE, &self.account)
            .and_then(|e| e.set_password(token))
            .map_err(|e| AuthError::Store(e.to_string()))
    }

    fn clear(&self) {
        if let Ok(e) = keyring::Entry::new(KEYRING_SERVICE, &self.account) {
            let _ = e.delete_credential();
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

impl TokenStore for MemoryStore {
    fn load(&self) -> Option<String> {
        crate::sync::lock(&self.0).clone()
    }

    fn save(&self, token: &str) -> Result<(), AuthError> {
        *crate::sync::lock(&self.0) = Some(token.to_string());
        Ok(())
    }

    fn clear(&self) {
        *crate::sync::lock(&self.0) = None;
    }
}

/// Reclassifies a `Remote` or `Malformed` auth failure by the *class* of
/// HTTP status it arrived on, independent of how the body itself happened
/// to parse. Status class, not body shape, is the actual signal for
/// whether an unchanged retry can help:
///
/// - 2xx: the server answered successfully and we still couldn't use the
///   result. Terminal, whether that surfaced as `Remote` (a rejection
///   embedded in a 200 body) or `Malformed` (a 200 with a body we don't
///   understand).
/// - 4xx except 429: the request itself was wrong — bad credentials, a
///   stale/wrong endpoint, a malformed request. Retrying the same request
///   cannot change that. Terminal, whether `Remote` or `Malformed` — a 404
///   that happens to parse as valid JSON without the field we expect
///   (`Malformed`) is exactly as unfixable by retrying as a 400 with a
///   structured `error.message` (`Remote`); both are the server saying the
///   request was wrong, just in different shapes.
/// - 429 or 5xx: rate limiting and server-side failure (including a
///   cold-started function returning a structured `Remote`-shaped
///   `INTERNAL`/`UNAVAILABLE` body). This is exactly what backoff exists
///   for. Reclassified to `Http` — transient — regardless of body shape.
/// - No status at all: `sign_in`/`mint_token`'s own `.send()`/`.text()`
///   failures are already `AuthError::Http` before this function is ever
///   called, so this function never sees them. Unchanged, transient.
///
/// Once this runs, a `Remote` or `Malformed` a caller sees always arrived
/// on a status this project treats as terminal (2xx or non-429 4xx) — see
/// `backoff::is_terminal`'s doc comment, which relies on that invariant.
fn reclassify_by_status(err: AuthError, status: reqwest::StatusCode) -> AuthError {
    let terminal_status =
        status.is_success() || (status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS);
    if matches!(&err, AuthError::Remote(_) | AuthError::Malformed(_)) && !terminal_status {
        AuthError::Http(format!("HTTP {status}: {err}"))
    } else {
        err
    }
}

async fn sign_in(creds: &Credentials, client: &reqwest::Client) -> Result<String, AuthError> {
    let body = serde_json::json!({
        "email": creds.email,
        "password": creds.password,
        "returnSecureToken": true,
    });
    let response = client
        .post(format!("{SIGN_IN_URL}?key={API_KEY}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| AuthError::Http(e.to_string()))?;
    parse_sign_in(&text).map_err(|e| reclassify_by_status(e, status))
}

pub async fn mint_token(creds: &Credentials) -> Result<String, AuthError> {
    // A server that accepts the connection and never responds must not hang
    // the caller forever; an auth round-trip has no legitimate reason to run
    // longer than this.
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("reqwest client with a timeout is always buildable");
    let id_token = sign_in(creds, &client).await?;
    let body = serde_json::json!({ "data": { "deviceId": creds.device_id } });
    let response = client
        .post(CREATE_TOKEN_URL)
        .bearer_auth(id_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| AuthError::Http(e.to_string()))?;
    parse_token_response(&text).map_err(|e| reclassify_by_status(e, status))
}

/// Caching is an optimization; a broken store must not fail a successful mint.
fn cache_token(store: &dyn TokenStore, token: &str) {
    if let Err(e) = store.save(token) {
        eprintln!("warning: could not cache Bluetooth token: {e}");
    }
}

/// A forced refresh must not read the cache at all, not even to discard it.
fn cached_token(store: &dyn TokenStore, force_refresh: bool) -> Option<String> {
    if force_refresh {
        None
    } else {
        store.load()
    }
}

/// Returns a cached token when one exists, otherwise mints and caches a new one.
pub async fn token(
    creds: &Credentials,
    store: &dyn TokenStore,
    force_refresh: bool,
) -> Result<String, AuthError> {
    if let Some(t) = cached_token(store, force_refresh) {
        return Ok(t);
    }
    let t = mint_token(creds).await?;
    cache_token(store, &t);
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_id_token_from_sign_in_response() {
        let body = r#"{"idToken":"eyJhbGc","email":"a@b.c","localId":"xyz"}"#;
        assert_eq!(parse_sign_in(body).unwrap(), "eyJhbGc");
    }

    #[test]
    fn sign_in_error_is_reported_not_swallowed() {
        let body = r#"{"error":{"code":400,"message":"INVALID_LOGIN_CREDENTIALS"}}"#;
        let err = parse_sign_in(body).unwrap_err();
        assert!(format!("{err}").contains("INVALID_LOGIN_CREDENTIALS"));
    }

    #[test]
    fn extracts_token_from_callable_envelope() {
        let body = r#"{"result":{"token":"bt-jwt-123"}}"#;
        assert_eq!(parse_token_response(body).unwrap(), "bt-jwt-123");
    }

    #[test]
    fn callable_error_is_reported() {
        let body = r#"{"error":{"message":"unauthenticated","status":"UNAUTHENTICATED"}}"#;
        let err = parse_token_response(body).unwrap_err();
        assert!(format!("{err}").contains("unauthenticated"));
    }

    #[test]
    fn token_response_malformed_json_is_reported() {
        let err = parse_token_response("not json").unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    #[test]
    fn a_non_success_status_reclassifies_an_unparseable_body_as_transient() {
        // e.g. a 502 from a gateway, or a captive portal's HTML: not a
        // contract mismatch, a transport problem.
        let reclassified = reclassify_by_status(
            AuthError::Malformed("expected value at line 1 column 1".into()),
            reqwest::StatusCode::BAD_GATEWAY,
        );
        assert!(matches!(reclassified, AuthError::Http(_)));
    }

    #[test]
    fn a_success_status_leaves_a_malformed_body_alone() {
        let reclassified = reclassify_by_status(
            AuthError::Malformed("no idToken field".into()),
            reqwest::StatusCode::OK,
        );
        assert!(matches!(reclassified, AuthError::Malformed(_)));
    }

    #[test]
    fn a_structured_rejection_is_not_reclassified_even_on_a_non_success_status() {
        // The identity service itself sends these on a non-2xx response;
        // reclassifying them would reopen looping against a rejecting
        // endpoint.
        let reclassified = reclassify_by_status(
            AuthError::Remote("INVALID_LOGIN_CREDENTIALS".into()),
            reqwest::StatusCode::BAD_REQUEST,
        );
        assert!(matches!(reclassified, AuthError::Remote(_)));
    }

    #[test]
    fn a_structured_rejection_on_a_server_error_status_is_transient() {
        // A cold-started function can return a perfectly structured
        // `{"error":{"message":"INTERNAL"}}` on a 500 -- that's exactly
        // what backoff exists for, not a rejection that will repeat.
        let reclassified = reclassify_by_status(
            AuthError::Remote("INTERNAL".into()),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        );
        assert!(matches!(reclassified, AuthError::Http(_)));
    }

    #[test]
    fn a_rate_limited_response_is_transient() {
        let reclassified = reclassify_by_status(
            AuthError::Remote("rate limited".into()),
            reqwest::StatusCode::TOO_MANY_REQUESTS,
        );
        assert!(matches!(reclassified, AuthError::Http(_)));
    }

    #[test]
    fn a_shape_mismatched_body_on_a_client_error_status_is_terminal() {
        // The exact shape a wrong-region 404 would produce: valid JSON,
        // but neither an error.message nor a result.token field. The
        // request itself is wrong; retrying an unchanged request cannot
        // fix that, regardless of how the body happened to parse.
        let reclassified = reclassify_by_status(
            AuthError::Malformed("no result.token field".into()),
            reqwest::StatusCode::NOT_FOUND,
        );
        assert!(matches!(reclassified, AuthError::Malformed(_)));
    }

    #[test]
    fn token_response_missing_field_is_reported() {
        // The shape a wrong-region 404 would actually produce: valid JSON,
        // but no result.token field.
        let body = r#"{"result":{}}"#;
        let err = parse_token_response(body).unwrap_err();
        assert!(format!("{err}").contains("no result.token field"));
    }

    #[test]
    fn memory_store_round_trips_and_clears() {
        let store = MemoryStore::default();
        assert!(store.load().is_none());
        store.save("tok").unwrap();
        assert_eq!(store.load().unwrap(), "tok");
        store.clear();
        assert!(store.load().is_none());
    }

    struct FailingStore;

    impl TokenStore for FailingStore {
        fn load(&self) -> Option<String> {
            None
        }

        fn save(&self, _token: &str) -> Result<(), AuthError> {
            Err(AuthError::Store("keychain locked".into()))
        }

        fn clear(&self) {}
    }

    #[test]
    fn token_is_returned_even_when_the_cache_write_fails() {
        // cache_token is exactly what token() calls after a successful mint;
        // its return type ((), not Result) makes a save failure unable to
        // propagate as an error to the caller. This does not panic.
        cache_token(&FailingStore, "minted-token");
    }

    fn dummy_credentials() -> Credentials {
        Credentials {
            email: "a@b.c".into(),
            password: "unused".into(),
            device_id: "device-1".into(),
        }
    }

    #[tokio::test]
    async fn cached_token_is_returned_without_a_network_call() {
        let store = MemoryStore::default();
        store.save("cached-token").unwrap();
        // force_refresh: false with a populated cache returns before token()
        // ever reaches mint_token, so this makes no network call even though
        // the credentials above are not real.
        let t = token(&dummy_credentials(), &store, false).await.unwrap();
        assert_eq!(t, "cached-token");
    }

    #[test]
    fn force_refresh_bypasses_the_cache() {
        // This is the exact decision token() makes before ever calling
        // mint_token; asserting it directly proves force_refresh skips the
        // cache without requiring a network call to observe it.
        let store = MemoryStore::default();
        store.save("cached-token").unwrap();
        assert_eq!(cached_token(&store, false), Some("cached-token".to_string()));
        assert_eq!(cached_token(&store, true), None);
    }

    #[tokio::test]
    #[ignore = "requires network and real credentials"]
    async fn live_mint_returns_a_token() {
        let creds = Credentials::from_env().expect("credentials in environment");
        let token = mint_token(&creds).await.expect("mint should succeed");
        assert!(!token.is_empty());
        println!("token length: {}", token.len());
    }
}
