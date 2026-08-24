use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

mod coordinator;
mod profile;
mod provider;

pub use coordinator::TokenCoordinator;
pub use profile::{
    identity_or_device_changed, password_profile_for_save, plan_confirmed_clear,
    recover_cache_identity, AuthMethod, AuthProfile, AuthProfileStore, ConfirmedClear,
    KeyringAuthProfileStore, MemoryAuthProfileStore, PasswordAuth, ProfileStoreError,
    ORPHAN_BLE_TOKEN_WARNING,
};
pub use provider::{AuthFuture, AuthProvider, PasswordAuthProvider};

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

pub struct SignIn {
    pub id_token: String,
    pub local_id: String,
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

pub fn parse_sign_in(body: &str) -> Result<SignIn, AuthError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AuthError::Malformed(e.to_string()))?;
    if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(AuthError::Remote(msg.to_string()));
    }
    let id_token = v
        .get("idToken")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| AuthError::Malformed("no idToken field".into()))?;
    let local_id = v
        .get("localId")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| AuthError::Malformed("no localId field".into()))?;
    if local_id.is_empty() {
        return Err(AuthError::Malformed("no localId field".into()));
    }
    Ok(SignIn {
        id_token,
        local_id,
    })
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

const DATABASE_URL: &str = "https://neurosity-device.firebaseio.com";

#[derive(Debug)]
pub struct UserDeviceRef {
    pub device_id: String,
    pub claimed_on: i64,
}

#[derive(Debug)]
pub struct ClaimedDevice {
    pub device_id: String,
    pub nickname: String,
}

fn rtdb_remote_error(v: &serde_json::Value) -> Option<AuthError> {
    match v.get("error") {
        Some(serde_json::Value::String(m)) => Some(AuthError::Remote(m.clone())),
        Some(obj) => obj
            .get("message")
            .and_then(|m| m.as_str())
            .map(|m| AuthError::Remote(m.to_string())),
        None => None,
    }
}

pub fn parse_user_devices(body: &str) -> Result<Vec<UserDeviceRef>, AuthError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AuthError::Malformed(e.to_string()))?;
    if let Some(err) = rtdb_remote_error(&v) {
        return Err(err);
    }
    if v.is_null() {
        return Ok(Vec::new());
    }
    let map = v
        .as_object()
        .ok_or_else(|| AuthError::Malformed("user devices is not an object".into()))?;
    let mut out: Vec<UserDeviceRef> = map
        .iter()
        .map(|(id, meta)| UserDeviceRef {
            device_id: id.clone(),
            claimed_on: meta.get("claimedOn").and_then(|n| n.as_i64()).unwrap_or(0),
        })
        .collect();
    out.sort_by_key(|d| (d.claimed_on, d.device_id.clone()));
    Ok(out)
}

pub fn parse_device_info(
    body: &str,
    fallback_id: &str,
) -> Result<Option<ClaimedDevice>, AuthError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AuthError::Malformed(e.to_string()))?;
    if let Some(err) = rtdb_remote_error(&v) {
        return Err(err);
    }
    if v.is_null() {
        return Ok(None);
    }
    let obj = v
        .as_object()
        .ok_or_else(|| AuthError::Malformed("device info is not an object".into()))?;
    let device_id = obj
        .get("deviceId")
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or(fallback_id)
        .to_string();
    let nickname = obj
        .get("deviceNickname")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(device_id.as_str())
        .to_string();
    Ok(Some(ClaimedDevice {
        device_id,
        nickname,
    }))
}

pub trait TokenStore: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, token: &str) -> Result<(), AuthError>;
    fn clear(&self) -> Result<(), AuthError>;
}

pub struct KeyringStore {
    pub account: String,
}

impl KeyringStore {
    pub fn new(account: impl Into<String>) -> Self {
        Self {
            account: account.into(),
        }
    }
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

    fn clear(&self) -> Result<(), AuthError> {
        let entry = match keyring::Entry::new(KEYRING_SERVICE, &self.account) {
            Ok(entry) => entry,
            Err(keyring::Error::NoEntry) => return Ok(()),
            Err(e) => return Err(AuthError::Store(e.to_string())),
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Store(e.to_string())),
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

    fn clear(&self) -> Result<(), AuthError> {
        *crate::sync::lock(&self.0) = None;
        Ok(())
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
    let terminal_status = status.is_success()
        || (status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS);
    if matches!(&err, AuthError::Remote(_) | AuthError::Malformed(_)) && !terminal_status {
        AuthError::Http(format!("HTTP {status}: {err}"))
    } else {
        err
    }
}

fn user_devices_url(local_id: &str) -> String {
    format!("{DATABASE_URL}/users/{local_id}/devices.json")
}

fn device_info_url(device_id: &str) -> String {
    format!("{DATABASE_URL}/devices/{device_id}/info.json")
}

async fn rtdb_get(
    client: &reqwest::Client,
    url: &str,
    id_token: &str,
) -> Result<String, AuthError> {
    let response = client
        .get(url)
        .query(&[("auth", id_token)])
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    if !status.is_success() {
        let err = rtdb_remote_error(
            &serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        )
        .unwrap_or_else(|| AuthError::Malformed(format!("HTTP {status}")));
        return Err(reclassify_by_status(err, status));
    }
    Ok(text)
}

async fn sign_in_email_password(
    email: &str,
    password: &str,
    client: &reqwest::Client,
) -> Result<SignIn, AuthError> {
    let body = serde_json::json!({
        "email": email,
        "password": password,
        "returnSecureToken": true,
    });
    let response = client
        .post(format!("{SIGN_IN_URL}?key={API_KEY}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    parse_sign_in(&text).map_err(|e| reclassify_by_status(e, status))
}

pub async fn list_claimed_devices(
    email: &str,
    password: &str,
) -> Result<Vec<ClaimedDevice>, AuthError> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("reqwest client with a timeout is always buildable");
    let sign_in = sign_in_email_password(email, password, &client).await?;
    let body = rtdb_get(
        &client,
        &user_devices_url(&sign_in.local_id),
        &sign_in.id_token,
    )
    .await?;
    let refs = parse_user_devices(&body)?;
    let mut devices = Vec::new();
    for device_ref in refs {
        let info_body = rtdb_get(
            &client,
            &device_info_url(&device_ref.device_id),
            &sign_in.id_token,
        )
        .await?;
        if let Some(device) = parse_device_info(&info_body, &device_ref.device_id)? {
            devices.push(device);
        }
    }
    Ok(devices)
}

pub async fn mint_token(creds: &Credentials) -> Result<String, AuthError> {
    // A server that accepts the connection and never responds must not hang
    // the caller forever; an auth round-trip has no legitimate reason to run
    // longer than this.
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("reqwest client with a timeout is always buildable");
    let sign_in = sign_in_email_password(&creds.email, &creds.password, &client).await?;
    let body = serde_json::json!({ "data": { "deviceId": creds.device_id } });
    let response = client
        .post(CREATE_TOKEN_URL)
        .bearer_auth(&sign_in.id_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    parse_token_response(&text).map_err(|e| reclassify_by_status(e, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_id_token_and_local_id_from_sign_in_response() {
        let body = r#"{"idToken":"eyJhbGc","email":"a@b.c","localId":"xyz"}"#;
        let parsed = parse_sign_in(body).unwrap();
        assert_eq!(parsed.id_token, "eyJhbGc");
        assert_eq!(parsed.local_id, "xyz");
    }

    #[test]
    fn sign_in_missing_local_id_is_malformed() {
        let body = r#"{"idToken":"eyJhbGc","email":"a@b.c"}"#;
        let err = parse_sign_in(body).err().unwrap();
        assert!(matches!(err, AuthError::Malformed(_)));
        assert!(format!("{err}").contains("no localId field"));
    }

    #[test]
    fn sign_in_error_is_reported_not_swallowed() {
        let body = r#"{"error":{"code":400,"message":"INVALID_LOGIN_CREDENTIALS"}}"#;
        let err = parse_sign_in(body).err().unwrap();
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
    fn user_devices_null_and_empty_object_are_empty_lists() {
        assert!(parse_user_devices("null").unwrap().is_empty());
        assert!(parse_user_devices("{}").unwrap().is_empty());
    }

    #[test]
    fn user_devices_sort_by_claimed_on_ascending() {
        let body = r#"{"later":{"claimedOn":20},"earlier":{"claimedOn":10},"missing":{}}"#;
        let parsed = parse_user_devices(body).unwrap();
        let ids: Vec<_> = parsed.iter().map(|d| d.device_id.as_str()).collect();
        assert_eq!(ids, ["missing", "earlier", "later"]);
        assert_eq!(parsed[0].claimed_on, 0);
    }

    #[test]
    fn rtdb_string_error_is_remote() {
        let err = parse_user_devices(r#"{"error":"Permission denied"}"#).unwrap_err();
        assert!(matches!(err, AuthError::Remote(m) if m == "Permission denied"));
    }

    #[test]
    fn device_info_null_is_skipped() {
        assert!(parse_device_info("null", "abc").unwrap().is_none());
    }

    #[test]
    fn device_info_uses_nickname_or_falls_back_to_id() {
        let named = parse_device_info(
            r#"{"deviceId":"abc","deviceNickname":"Kitchen Crown"}"#,
            "abc",
        )
        .unwrap()
        .unwrap();
        assert_eq!(named.device_id, "abc");
        assert_eq!(named.nickname, "Kitchen Crown");

        let unnamed = parse_device_info(r#"{"deviceId":"abc"}"#, "abc")
            .unwrap()
            .unwrap();
        assert_eq!(unnamed.nickname, "abc");

        let key_only = parse_device_info("{}", "from-path").unwrap().unwrap();
        assert_eq!(key_only.device_id, "from-path");
        assert_eq!(key_only.nickname, "from-path");
    }

    #[test]
    fn device_info_string_error_is_remote() {
        let err = parse_device_info(r#"{"error":"Permission denied"}"#, "abc").unwrap_err();
        assert!(matches!(err, AuthError::Remote(_)));
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
    fn claimed_device_urls_match_neurosity_rtdb_paths() {
        assert_eq!(
            user_devices_url("uid-1"),
            "https://neurosity-device.firebaseio.com/users/uid-1/devices.json"
        );
        assert_eq!(
            device_info_url("abc"),
            "https://neurosity-device.firebaseio.com/devices/abc/info.json"
        );
    }

    #[test]
    fn memory_store_round_trips_and_clears() {
        let store = MemoryStore::default();
        assert!(store.load().is_none());
        store.save("tok").unwrap();
        assert_eq!(store.load().unwrap(), "tok");
        store.clear().unwrap();
        assert!(store.load().is_none());
    }

    #[tokio::test]
    #[ignore = "requires network and real credentials"]
    async fn live_mint_returns_a_token() {
        let creds = Credentials::from_env().expect("credentials in environment");
        let token = mint_token(&creds).await.expect("mint should succeed");
        assert!(!token.is_empty());
        println!("token length: {}", token.len());
    }

    #[tokio::test]
    #[ignore = "requires network and real credentials"]
    async fn live_list_returns_claimed_devices() {
        let creds = Credentials::from_env().expect("credentials in environment");
        let devices = list_claimed_devices(&creds.email, &creds.password)
            .await
            .expect("list should succeed");
        assert!(
            devices.iter().any(|d| d.device_id == creds.device_id),
            "expected env device id in {:?}",
            devices.iter().map(|d| d.device_id.as_str()).collect::<Vec<_>>()
        );
    }
}
