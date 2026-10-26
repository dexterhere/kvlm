// SPDX-License-Identifier: GPL-3.0-or-later
//! The Hugging Face access token: a redacted, zeroized value, a Secret Service store, and
//! the in-memory slot the HTTP clients read.
//!
//! The token is never logged, put in an error, an event, SQLite or a settings file. There is
//! no file or environment-variable fallback: without a working Secret Service the token can
//! still be used for this session but is not saved. Nothing here runs at startup.

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_LEN: usize = 256;
const LABEL: &str = "Local AI Workstation: Hugging Face access token";
const ATTRIBUTES: [(&str, &str); 2] = [
    ("application", "org.gnome.LocalAIWorkstation"),
    ("service", "huggingface"),
];

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A Hugging Face access token. `Debug` and `Display` never show it.
#[derive(Clone)]
pub struct Token(Zeroizing<String>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidToken;

impl fmt::Display for InvalidToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("That is not a Hugging Face token: use only letters, digits, '_', '-' and '.' (at most 256 characters).")
    }
}

impl Token {
    /// Surrounding whitespace from a paste is dropped; anything else odd is refused, which
    /// also keeps the value safe to place in an HTTP header.
    pub fn parse(text: &str) -> Result<Self, InvalidToken> {
        let text = text.trim();
        let valid = !text.is_empty()
            && text.len() <= MAX_LEN
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        valid
            .then(|| Self(Zeroizing::new(text.to_owned())))
            .ok_or(InvalidToken)
    }

    /// For the one place that builds an `Authorization` header or a keyring item.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token([redacted])")
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// The token the HTTP clients attach, shared by every client of one application.
#[derive(Clone, Default)]
pub struct TokenSlot(Arc<Mutex<Option<Token>>>);

impl TokenSlot {
    pub fn set(&self, token: Option<Token>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }

    pub fn get(&self) -> Option<Token> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn is_set(&self) -> bool {
        self.get().is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// No Secret Service answered on the session bus.
    Unavailable,
    Locked,
    /// The user dismissed the keyring prompt.
    Denied,
    Timeout,
    Failed,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "No keyring (Secret Service) is available, so the token cannot be saved. It can still be used until you close the application.",
            Self::Locked => "The keyring is locked. Unlock it and retry.",
            Self::Denied => "The keyring request was dismissed. Retry and approve it.",
            Self::Timeout => "The keyring did not answer in time. If it is showing a prompt, answer it and retry.",
            Self::Failed => "The keyring could not complete the request.",
        })
    }
}

impl std::error::Error for SecretError {}

/// Where the token is kept between sessions.
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self) -> BoxFuture<Result<Option<Token>, SecretError>>;
    fn put(&self, token: Token) -> BoxFuture<Result<(), SecretError>>;
    fn clear(&self) -> BoxFuture<Result<(), SecretError>>;
}

/// The desktop keyring over the Secret Service D-Bus API. Only the encrypted transport is
/// used, and the sandbox file backend is not compiled into this path.
pub struct KeyringStore;

fn classify(error: oo7::dbus::Error) -> SecretError {
    use oo7::dbus::{Error, ServiceError};
    match error {
        Error::Dismissed => SecretError::Denied,
        Error::Service(ServiceError::IsLocked(_)) => SecretError::Locked,
        Error::ZBus(_) | Error::IO(_) => SecretError::Unavailable,
        _ => SecretError::Failed,
    }
}

async fn timed<T>(work: impl Future<Output = Result<T, SecretError>>) -> Result<T, SecretError> {
    timed_for(TIMEOUT, work).await
}

async fn timed_for<T>(
    limit: Duration,
    work: impl Future<Output = Result<T, SecretError>>,
) -> Result<T, SecretError> {
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or(Err(SecretError::Timeout))
}

async fn collection() -> Result<oo7::dbus::Collection, SecretError> {
    let service = oo7::dbus::Service::encrypted().await.map_err(classify)?;
    let collection = service.default_collection().await.map_err(classify)?;
    if collection.is_locked().await.map_err(classify)? {
        collection.unlock(None).await.map_err(classify)?;
    }
    Ok(collection)
}

impl SecretStore for KeyringStore {
    fn get(&self) -> BoxFuture<Result<Option<Token>, SecretError>> {
        Box::pin(timed(async {
            let items = collection()
                .await?
                .search_items(&ATTRIBUTES)
                .await
                .map_err(classify)?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            let secret = item.secret().await.map_err(classify)?;
            // A value this application did not write is reported, never shown or deleted.
            let text = std::str::from_utf8(secret.as_bytes()).map_err(|_| SecretError::Failed)?;
            Token::parse(text)
                .map(Some)
                .map_err(|_| SecretError::Failed)
        }))
    }

    fn put(&self, token: Token) -> BoxFuture<Result<(), SecretError>> {
        Box::pin(timed(async move {
            collection()
                .await?
                .create_item(
                    LABEL,
                    &ATTRIBUTES,
                    oo7::Secret::text(token.expose()),
                    true,
                    None,
                )
                .await
                .map(|_| ())
                .map_err(classify)
        }))
    }

    fn clear(&self) -> BoxFuture<Result<(), SecretError>> {
        Box::pin(timed(async {
            let items = collection()
                .await?
                .search_items(&ATTRIBUTES)
                .await
                .map_err(classify)?;
            for item in items {
                item.delete(None).await.map_err(classify)?;
            }
            Ok(())
        }))
    }
}

/// Keeps the keyring and the in-memory slot consistent. "Saved" and "cleared" are reported
/// only after the store confirms, and the slot never holds a token the user removed.
pub struct TokenService {
    store: Arc<dyn SecretStore>,
    slot: TokenSlot,
}

impl TokenService {
    pub fn new(store: Arc<dyn SecretStore>, slot: TokenSlot) -> Self {
        Self { store, slot }
    }

    pub fn keyring(slot: TokenSlot) -> Self {
        Self::new(Arc::new(KeyringStore), slot)
    }

    pub fn slot(&self) -> &TokenSlot {
        &self.slot
    }

    /// Save to the keyring, then use it. If the keyring fails the token is still used for
    /// this session, which the error says.
    pub async fn save(&self, token: Token) -> Result<(), SecretError> {
        let saved = self.store.put(token.clone()).await;
        self.slot.set(Some(token));
        saved
    }

    /// Read the saved token into memory. `Ok(false)` means nothing is saved.
    pub async fn load(&self) -> Result<bool, SecretError> {
        match self.store.get().await? {
            Some(token) => {
                self.slot.set(Some(token));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Forget the token at once, then remove the saved copy.
    pub async fn clear(&self) -> Result<(), SecretError> {
        self.slot.set(None);
        self.store.clear().await
    }
}

#[cfg(test)]
mod tests;
