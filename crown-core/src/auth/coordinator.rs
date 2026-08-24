use std::sync::Arc;

use super::provider::{AuthProvider, PasswordAuthProvider};
use super::{AuthError, AuthProfile, KeyringStore, TokenStore};

pub struct TokenCoordinator {
    provider: Arc<dyn AuthProvider>,
    store: Arc<dyn TokenStore>,
}

impl TokenCoordinator {
    pub fn new(provider: Arc<dyn AuthProvider>, store: Arc<dyn TokenStore>) -> Self {
        Self { provider, store }
    }

    pub fn from_profile(profile: AuthProfile) -> Self {
        let provider: Arc<dyn AuthProvider> = Arc::new(PasswordAuthProvider::from_profile(profile));
        let store = Arc::new(KeyringStore::new(provider.cache_identity()));
        Self::new(provider, store)
    }

    pub async fn token(&self, force_refresh: bool) -> Result<String, AuthError> {
        if !force_refresh {
            if let Some(t) = self.store.load() {
                return Ok(t);
            }
        }
        let t = self.provider.mint_ble_token().await?;
        if let Err(e) = self.store.save(&t) {
            eprintln!("warning: could not cache Bluetooth token: {e}");
        }
        Ok(t)
    }

    pub fn clear_cache(&self) -> Result<(), AuthError> {
        self.store.clear()
    }

    pub fn device_id(&self) -> &str {
        self.provider.device_id()
    }

    pub fn cache_identity(&self) -> &str {
        self.provider.cache_identity()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::TokenCoordinator;
    use crate::auth::{AuthError, AuthFuture, AuthProvider, MemoryStore, TokenStore};

    struct FakeProvider {
        token: String,
        calls: AtomicUsize,
    }

    impl FakeProvider {
        fn new(token: impl Into<String>) -> Self {
            Self {
                token: token.into(),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl AuthProvider for FakeProvider {
        fn cache_identity(&self) -> &str {
            "fake@example.com"
        }

        fn device_id(&self) -> &str {
            "device-1"
        }

        fn mint_ble_token(&self) -> AuthFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let token = self.token.clone();
            Box::pin(async move { Ok(token) })
        }
    }

    struct FailingSaveStore;

    impl TokenStore for FailingSaveStore {
        fn load(&self) -> Option<String> {
            None
        }

        fn save(&self, _token: &str) -> Result<(), AuthError> {
            Err(AuthError::Store("keychain locked".into()))
        }

        fn clear(&self) -> Result<(), AuthError> {
            Ok(())
        }
    }

    struct FailingClearStore;

    impl TokenStore for FailingClearStore {
        fn load(&self) -> Option<String> {
            None
        }

        fn save(&self, _token: &str) -> Result<(), AuthError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), AuthError> {
            Err(AuthError::Store("locked".into()))
        }
    }

    #[test]
    fn coordinator_forwards_provider_identity() {
        let coordinator = TokenCoordinator::new(
            Arc::new(FakeProvider::new("unused")),
            Arc::new(MemoryStore::default()),
        );
        assert_eq!(coordinator.device_id(), "device-1");
        assert_eq!(coordinator.cache_identity(), "fake@example.com");
    }

    #[tokio::test]
    async fn cache_hit_does_not_call_provider() {
        let provider = Arc::new(FakeProvider::new("minted"));
        let store = Arc::new(MemoryStore::default());
        store.save("cached").unwrap();
        let coordinator = TokenCoordinator::new(provider.clone(), store);
        assert_eq!(coordinator.token(false).await.unwrap(), "cached");
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn forced_refresh_mints_and_replaces_cache() {
        let provider = Arc::new(FakeProvider::new("fresh"));
        let store = Arc::new(MemoryStore::default());
        store.save("stale").unwrap();
        let coordinator = TokenCoordinator::new(provider.clone(), store.clone());
        assert_eq!(coordinator.token(true).await.unwrap(), "fresh");
        assert_eq!(store.load().as_deref(), Some("fresh"));
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn minted_token_is_returned_when_cache_write_fails() {
        let provider = Arc::new(FakeProvider::new("minted"));
        let coordinator = TokenCoordinator::new(provider, Arc::new(FailingSaveStore));
        assert_eq!(coordinator.token(false).await.unwrap(), "minted");
    }

    #[test]
    fn clear_cache_reports_store_failure() {
        let coordinator = TokenCoordinator::new(
            Arc::new(FakeProvider::new("unused")),
            Arc::new(FailingClearStore),
        );
        assert!(matches!(
            coordinator.clear_cache(),
            Err(AuthError::Store(_))
        ));
    }
}
