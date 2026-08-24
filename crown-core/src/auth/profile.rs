use std::fmt;
use std::sync::Mutex;

const PROFILE_VERSION: u32 = 1;
const AUTH_KIND_PASSWORD: &str = "password";
const PROFILE_KEYRING_SERVICE: &str = "crown-reader.credentials";
const PROFILE_KEYRING_ACCOUNT: &str = "neurosity";

pub enum AuthMethod {
    Password(PasswordAuth),
}

pub struct PasswordAuth {
    email: String,
    password: String,
}

pub struct AuthProfile {
    device_id: String,
    method: AuthMethod,
}

fn required_identity(value: &str, field: &'static str) -> Result<String, ProfileStoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(ProfileStoreError::InvalidInput(field))
    } else {
        Ok(trimmed.to_string())
    }
}

impl AuthProfile {
    pub fn password(
        email: String,
        password: String,
        device_id: String,
    ) -> Result<Self, ProfileStoreError> {
        let email = required_identity(&email, "email")?;
        if password.is_empty() {
            return Err(ProfileStoreError::InvalidInput("password"));
        }
        Ok(Self {
            device_id: required_identity(&device_id, "device_id")?,
            method: AuthMethod::Password(PasswordAuth { email, password }),
        })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn cache_identity(&self) -> &str {
        let AuthMethod::Password(password) = &self.method;
        password.email()
    }

    pub fn method(&self) -> &AuthMethod {
        &self.method
    }

    #[allow(dead_code)]
    pub(crate) fn into_parts(self) -> (String, AuthMethod) {
        (self.device_id, self.method)
    }
}

impl PasswordAuth {
    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    #[allow(dead_code)]
    pub(crate) fn into_parts(self) -> (String, String) {
        (self.email, self.password)
    }
}

#[derive(Debug)]
pub enum ProfileStoreError {
    Unavailable(String),
    Malformed(String),
    UnsupportedVersion(u32),
    UnsupportedMethod(String),
    Write(String),
    Delete(String),
    InvalidInput(&'static str),
}

impl fmt::Display for ProfileStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(_) => write!(f, "profile store unavailable"),
            Self::Malformed(_) => write!(f, "malformed profile"),
            Self::UnsupportedVersion(_) => write!(f, "unsupported profile version"),
            Self::UnsupportedMethod(_) => write!(f, "unsupported auth method"),
            Self::Write(_) => write!(f, "failed to write profile"),
            Self::Delete(_) => write!(f, "failed to delete profile"),
            Self::InvalidInput(field) => write!(f, "invalid {field}"),
        }
    }
}

pub fn password_profile_for_save(
    existing: Option<AuthProfile>,
    email: &str,
    password: &str,
    device_id: &str,
) -> Result<AuthProfile, ProfileStoreError> {
    let password = if password.is_empty() {
        match existing {
            Some(AuthProfile {
                method: AuthMethod::Password(stored),
                ..
            }) => stored.password,
            None => return Err(ProfileStoreError::InvalidInput("password")),
        }
    } else {
        password.to_string()
    };
    AuthProfile::password(email.to_string(), password, device_id.to_string())
}

pub trait AuthProfileStore: Send + Sync {
    fn load(&self) -> Result<Option<AuthProfile>, ProfileStoreError>;
    fn save(&self, profile: &AuthProfile) -> Result<(), ProfileStoreError>;
    fn clear(&self) -> Result<(), ProfileStoreError>;
}

#[derive(Default)]
pub struct MemoryAuthProfileStore(Mutex<Option<String>>);

#[derive(Default)]
pub struct KeyringAuthProfileStore;

impl AuthProfileStore for MemoryAuthProfileStore {
    fn load(&self) -> Result<Option<AuthProfile>, ProfileStoreError> {
        crate::sync::lock(&self.0)
            .as_deref()
            .map(decode)
            .transpose()
    }

    fn save(&self, profile: &AuthProfile) -> Result<(), ProfileStoreError> {
        *crate::sync::lock(&self.0) = Some(encode(profile)?);
        Ok(())
    }

    fn clear(&self) -> Result<(), ProfileStoreError> {
        *crate::sync::lock(&self.0) = None;
        Ok(())
    }
}

impl AuthProfileStore for KeyringAuthProfileStore {
    fn load(&self) -> Result<Option<AuthProfile>, ProfileStoreError> {
        let entry = match keyring::Entry::new(PROFILE_KEYRING_SERVICE, PROFILE_KEYRING_ACCOUNT) {
            Ok(entry) => entry,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(ProfileStoreError::Unavailable(e.to_string())),
        };
        match entry.get_password() {
            Ok(encoded) => decode(&encoded).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(ProfileStoreError::Unavailable(e.to_string())),
        }
    }

    fn save(&self, profile: &AuthProfile) -> Result<(), ProfileStoreError> {
        let encoded = encode(profile)?;
        match keyring::Entry::new(PROFILE_KEYRING_SERVICE, PROFILE_KEYRING_ACCOUNT) {
            Ok(entry) => entry
                .set_password(&encoded)
                .map_err(|e| ProfileStoreError::Write(e.to_string())),
            Err(e) => Err(ProfileStoreError::Unavailable(e.to_string())),
        }
    }

    fn clear(&self) -> Result<(), ProfileStoreError> {
        let entry = match keyring::Entry::new(PROFILE_KEYRING_SERVICE, PROFILE_KEYRING_ACCOUNT) {
            Ok(entry) => entry,
            Err(keyring::Error::NoEntry) => return Ok(()),
            Err(e) => return Err(ProfileStoreError::Unavailable(e.to_string())),
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(ProfileStoreError::Delete(e.to_string())),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredProfile {
    version: u32,
    device_id: String,
    auth: serde_json::Value,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredPassword {
    kind: String,
    email: String,
    password: String,
}

fn encode(profile: &AuthProfile) -> Result<String, ProfileStoreError> {
    let AuthMethod::Password(password) = &profile.method;
    let auth = serde_json::to_value(StoredPassword {
        kind: AUTH_KIND_PASSWORD.to_string(),
        email: password.email.clone(),
        password: password.password.clone(),
    })
    .map_err(|e| ProfileStoreError::Malformed(e.to_string()))?;
    serde_json::to_string(&StoredProfile {
        version: PROFILE_VERSION,
        device_id: profile.device_id.clone(),
        auth,
    })
    .map_err(|e| ProfileStoreError::Malformed(e.to_string()))
}

fn decode(json: &str) -> Result<AuthProfile, ProfileStoreError> {
    let stored: StoredProfile =
        serde_json::from_str(json).map_err(|e| ProfileStoreError::Malformed(e.to_string()))?;
    if stored.version != PROFILE_VERSION {
        return Err(ProfileStoreError::UnsupportedVersion(stored.version));
    }
    let kind = stored
        .auth
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProfileStoreError::Malformed("missing auth.kind".into()))?;
    if kind != AUTH_KIND_PASSWORD {
        return Err(ProfileStoreError::UnsupportedMethod(kind.to_string()));
    }
    let password: StoredPassword = serde_json::from_value(stored.auth)
        .map_err(|e| ProfileStoreError::Malformed(e.to_string()))?;
    AuthProfile::password(password.email, password.password, stored.device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password_profile() -> AuthProfile {
        AuthProfile::password(
            "reader@example.com".into(),
            "secret".into(),
            "device-1".into(),
        )
        .unwrap()
    }

    #[test]
    fn version_one_password_profile_round_trips() {
        let encoded = encode(&password_profile()).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.device_id(), "device-1");
        assert_eq!(decoded.cache_identity(), "reader@example.com");
        let AuthMethod::Password(password) = decoded.method();
        assert_eq!(password.email(), "reader@example.com");
        assert_eq!(password.password(), "secret");
    }

    #[test]
    fn rejects_unknown_version() {
        let json = r#"{"version":2,"device_id":"d","auth":{"kind":"password","email":"e","password":"p"}}"#;
        assert!(matches!(
            decode(json),
            Err(ProfileStoreError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn rejects_unknown_auth_method() {
        let json = r#"{"version":1,"device_id":"d","auth":{"kind":"oauth","token":"x"}}"#;
        assert!(matches!(
            decode(json),
            Err(ProfileStoreError::UnsupportedMethod(kind)) if kind == "oauth"
        ));
    }

    #[test]
    fn trims_identity_fields_and_rejects_blank_required_values() {
        let profile = AuthProfile::password(" a@b.c ".into(), "p".into(), " dev ".into()).unwrap();
        assert_eq!(profile.cache_identity(), "a@b.c");
        assert_eq!(profile.device_id(), "dev");
        assert!(matches!(
            AuthProfile::password(" ".into(), "p".into(), "d".into()),
            Err(ProfileStoreError::InvalidInput("email"))
        ));
    }

    #[test]
    fn blank_edit_password_keeps_stored_password() {
        let updated =
            password_profile_for_save(Some(password_profile()), "new@example.com", "", "device-2")
                .unwrap();
        let AuthMethod::Password(password) = updated.method();
        assert_eq!(password.password(), "secret");
    }

    #[test]
    fn first_save_requires_password() {
        assert!(matches!(
            password_profile_for_save(None, "a@b.c", "", "device-1"),
            Err(ProfileStoreError::InvalidInput("password"))
        ));
    }

    #[test]
    fn memory_store_replaces_and_clears_profile() {
        let store = MemoryAuthProfileStore::default();
        assert!(store.load().unwrap().is_none());
        store.save(&password_profile()).unwrap();
        assert_eq!(store.load().unwrap().unwrap().device_id(), "device-1");
        store.clear().unwrap();
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}
