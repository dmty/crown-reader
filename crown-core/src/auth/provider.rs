use super::{mint_token, AuthError, AuthMethod, AuthProfile, Credentials};

pub type AuthFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, AuthError>> + Send + 'a>>;

pub trait AuthProvider: Send + Sync {
    fn cache_identity(&self) -> &str;
    fn device_id(&self) -> &str;
    fn mint_ble_token(&self) -> AuthFuture<'_>;
}

pub struct PasswordAuthProvider {
    credentials: Credentials,
}

impl PasswordAuthProvider {
    pub fn from_profile(profile: AuthProfile) -> Self {
        let (device_id, method) = profile.into_parts();
        let AuthMethod::Password(password) = method;
        let (email, password) = password.into_parts();
        Self {
            credentials: Credentials {
                email,
                password,
                device_id,
            },
        }
    }
}

impl AuthProvider for PasswordAuthProvider {
    fn cache_identity(&self) -> &str {
        &self.credentials.email
    }

    fn device_id(&self) -> &str {
        &self.credentials.device_id
    }

    fn mint_ble_token(&self) -> AuthFuture<'_> {
        Box::pin(mint_token(&self.credentials))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_provider_uses_profile_identity() {
        let profile = AuthProfile::password(
            "reader@example.com".into(),
            "secret".into(),
            "device-1".into(),
        )
        .unwrap();
        let provider = PasswordAuthProvider::from_profile(profile);
        assert_eq!(provider.cache_identity(), "reader@example.com");
        assert_eq!(provider.device_id(), "device-1");
    }
}
