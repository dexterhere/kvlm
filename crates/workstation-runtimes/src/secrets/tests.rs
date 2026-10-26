// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

/// In-memory store with scripted failures; the real keyring is never contacted.
struct Fake {
    saved: Mutex<Option<Token>>,
    fail: Option<SecretError>,
}

impl SecretStore for Fake {
    fn get(&self) -> BoxFuture<Result<Option<Token>, SecretError>> {
        let result = match self.fail {
            Some(error) => Err(error),
            None => Ok(self.saved.lock().unwrap().clone()),
        };
        Box::pin(async move { result })
    }
    fn put(&self, token: Token) -> BoxFuture<Result<(), SecretError>> {
        let result = match self.fail {
            Some(error) => Err(error),
            None => {
                *self.saved.lock().unwrap() = Some(token);
                Ok(())
            }
        };
        Box::pin(async move { result })
    }
    fn clear(&self) -> BoxFuture<Result<(), SecretError>> {
        let result = match self.fail {
            Some(error) => Err(error),
            None => {
                *self.saved.lock().unwrap() = None;
                Ok(())
            }
        };
        Box::pin(async move { result })
    }
}

fn service(fail: Option<SecretError>) -> (TokenService, Arc<Fake>) {
    let store = Arc::new(Fake {
        saved: Mutex::new(None),
        fail,
    });
    (
        TokenService::new(store.clone(), TokenSlot::default()),
        store,
    )
}

#[test]
fn tokens_are_validated_trimmed_and_never_printed() {
    let token = Token::parse("  hf_AbC123-_.x\n").unwrap();
    assert_eq!(token.expose(), "hf_AbC123-_.x");
    assert_eq!(format!("{token:?} {token}"), "Token([redacted]) [redacted]");
    assert!(!format!("{:?}", Some(token)).contains("AbC123"));
    for bad in [
        "",
        "   ",
        "has space",
        "new\nline",
        "quote\"",
        "é",
        &"a".repeat(257),
    ] {
        assert_eq!(Token::parse(bad).err(), Some(InvalidToken), "{bad:?}");
        assert!(!InvalidToken.to_string().contains(bad) || bad.is_empty());
    }
}

#[tokio::test]
async fn save_load_and_clear_keep_the_store_and_the_slot_in_step() {
    let (service, store) = service(None);
    assert_eq!(service.load().await, Ok(false));
    service.save(Token::parse("hf_one").unwrap()).await.unwrap();
    assert_eq!(service.slot().get().unwrap().expose(), "hf_one");
    assert_eq!(
        store.saved.lock().unwrap().as_ref().unwrap().expose(),
        "hf_one"
    );

    service.slot().set(None);
    assert_eq!(service.load().await, Ok(true));
    assert!(service.slot().is_set());

    service.clear().await.unwrap();
    assert!(!service.slot().is_set());
    assert!(store.saved.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_failing_keyring_never_blocks_use_and_never_claims_success() {
    for error in [
        SecretError::Unavailable,
        SecretError::Locked,
        SecretError::Denied,
        SecretError::Timeout,
    ] {
        let (service, _) = service(Some(error));
        // Not saved, but usable for this session; the caller sees the error.
        assert_eq!(
            service.save(Token::parse("hf_two").unwrap()).await,
            Err(error)
        );
        assert!(service.slot().is_set());
        assert_eq!(service.load().await, Err(error));
        // Removal drops the in-memory token even when the saved copy cannot be removed.
        assert_eq!(service.clear().await, Err(error));
        assert!(!service.slot().is_set());
    }
}

#[tokio::test]
async fn a_hung_keyring_call_times_out() {
    let hung = timed_for::<()>(Duration::from_millis(30), std::future::pending());
    assert_eq!(hung.await, Err(SecretError::Timeout));
}

#[test]
fn error_messages_carry_no_secret_material() {
    for error in [
        SecretError::Unavailable,
        SecretError::Locked,
        SecretError::Denied,
        SecretError::Timeout,
        SecretError::Failed,
    ] {
        assert!(!error.to_string().contains("hf_"));
    }
}
