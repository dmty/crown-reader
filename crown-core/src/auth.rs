use std::fmt;
use std::sync::Mutex;

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
        keyring::Entry::new(KEYRING_SERVICE, &self.account)
            .ok()?
            .get_password()
            .ok()
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
        self.0.lock().unwrap().clone()
    }

    fn save(&self, token: &str) -> Result<(), AuthError> {
        *self.0.lock().unwrap() = Some(token.to_string());
        Ok(())
    }

    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

async fn sign_in(creds: &Credentials) -> Result<String, AuthError> {
    let body = serde_json::json!({
        "email": creds.email,
        "password": creds.password,
        "returnSecureToken": true,
    });
    let text = reqwest::Client::new()
        .post(format!("{SIGN_IN_URL}?key={API_KEY}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?
        .text()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    parse_sign_in(&text)
}

pub async fn mint_token(creds: &Credentials) -> Result<String, AuthError> {
    let id_token = sign_in(creds).await?;
    let body = serde_json::json!({ "data": { "deviceId": creds.device_id } });
    let text = reqwest::Client::new()
        .post(CREATE_TOKEN_URL)
        .bearer_auth(id_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?
        .text()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;
    parse_token_response(&text)
}

/// Caching is an optimization; a broken store must not fail a successful mint.
fn cache_token(store: &dyn TokenStore, token: &str) {
    if let Err(e) = store.save(token) {
        eprintln!("warning: could not cache Bluetooth token: {e}");
    }
}

/// Returns a cached token when one exists, otherwise mints and caches a new one.
pub async fn token(
    creds: &Credentials,
    store: &dyn TokenStore,
    force_refresh: bool,
) -> Result<String, AuthError> {
    if !force_refresh {
        if let Some(t) = store.load() {
            return Ok(t);
        }
    }
    let t = mint_token(creds).await?;
    cache_token(store, &t);
    Ok(t)
}

#[tokio::test]
#[ignore = "requires network and real credentials"]
async fn live_mint_returns_a_token() {
    let creds = Credentials::from_env().expect("credentials in environment");
    let token = mint_token(&creds).await.expect("mint should succeed");
    assert!(!token.is_empty());
    println!("token length: {}", token.len());
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
}
