use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    providers::{
        Credentials, ErrorKind, Profile, ProviderError, ProviderId, Session,
        refresh_spotify_library_access,
    },
    storage::{Account, Store, StoreError},
};

#[derive(Debug)]
pub(crate) enum AccessError {
    Store(StoreError),
    Provider(ProviderError),
}

impl From<StoreError> for AccessError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Provider(error) => error.fmt(formatter),
        }
    }
}

pub(crate) fn prepare(
    store: &Store,
    profile: &str,
    cancelled: &AtomicBool,
) -> Result<(Account, Credentials), AccessError> {
    prepare_with(store, profile, cancelled, refresh_spotify_library_access)
}

fn prepare_with(
    store: &Store,
    profile: &str,
    cancelled: &AtomicBool,
    refresh: impl FnOnce(&mut Credentials, &Profile, &AtomicBool) -> Result<bool, ProviderError>,
) -> Result<(Account, Credentials), AccessError> {
    check_cancelled(cancelled)?;
    let Some(_exchange) = store.lock_connection(ProviderId::Spotify, cancelled)? else {
        check_cancelled(cancelled)?;
        return Err(AccessError::Provider(ProviderError {
            kind: ErrorKind::Unavailable,
            message: "Доступ Spotify уже обновляется. Повторите запрос чуть позже.".into(),
        }));
    };
    check_cancelled(cancelled)?;
    let (account, mut credentials) = current_access(store, profile)?;
    let updated =
        refresh(&mut credentials, &account.profile, cancelled).map_err(AccessError::Provider)?;

    if !updated {
        return Ok((account, credentials));
    }
    let session = Session {
        provider: account.provider,
        profile: account.profile.clone(),
        capabilities: account.capabilities,
        credentials,
    };
    // The exchange has already succeeded. Save any rotated access before
    // returning cancellation for the favorite request.
    if !store.update_account(&session, &account, || Some(()))? {
        return Err(changed_connection());
    }
    check_cancelled(cancelled)?;
    let (saved, credentials) = current_access(store, profile)?;
    if credentials != session.credentials {
        return Err(changed_connection());
    }
    Ok((saved, credentials))
}

fn current_access(store: &Store, profile: &str) -> Result<(Account, Credentials), AccessError> {
    let provider = ProviderId::Spotify;
    let account = store
        .account(provider, profile)?
        .ok_or_else(changed_connection)?;
    let credentials = store.credentials(&account)?.ok_or_else(|| {
        AccessError::Provider(ProviderError {
            kind: ErrorKind::Expired,
            message: "Подтвердите вход в Spotify заново.".into(),
        })
    })?;
    // The vault lookup may have opened a newer reference than the account
    // read. Use the pair only if the same binding is still current.
    if store.account(provider, profile)?.as_ref() != Some(&account) {
        return Err(changed_connection());
    }
    Ok((account, credentials))
}

fn changed_connection() -> AccessError {
    AccessError::Provider(ProviderError {
        kind: ErrorKind::InvalidInput,
        message: "Подключение Spotify изменилось.".into(),
    })
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), AccessError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(AccessError::Provider(ProviderError {
            kind: ErrorKind::Cancelled,
            message: "Запрос Spotify отменён.".into(),
        }));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::providers::{Capabilities, SpotifySessionCredentials};

    const PROFILE: &str = "spotify-library-fixture";

    fn session(access: &str, refresh: &str, expiration: u64) -> Session {
        Session {
            provider: ProviderId::Spotify,
            profile: Profile {
                id: PROFILE.into(),
                name: "Spotify library fixture".into(),
            },
            capabilities: Capabilities {
                home: true,
                profile_selection: false,
                device_code: false,
            },
            credentials: Credentials::Spotify {
                client_id: "fixture-client".into(),
                access_token: access.into(),
                refresh_token: refresh.into(),
                access_expires_at: Some(expiration),
                librespot: Some(SpotifySessionCredentials {
                    username: PROFILE.into(),
                    auth_data: vec![1],
                }),
            },
        }
    }

    struct Fixture {
        store: Store,
        directory: tempfile::TempDir,
    }

    impl Fixture {
        fn new(session: &Session) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open(directory.path()).unwrap();
            assert!(store.use_account(session, || Some(()), || {}).unwrap());
            Self { store, directory }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self
                .store
                .disconnect_account(ProviderId::Spotify, PROFILE, || Some(()));
        }
    }

    #[test]
    fn saves_both_rotated_tokens_and_returns_the_current_account_scope() {
        let before = session("old-access", "old-refresh", 1);
        let next = session("new-access", "new-refresh", 2);
        let fixture = Fixture::new(&before);
        let store = &fixture.store;
        let previous = store
            .account(ProviderId::Spotify, PROFILE)
            .unwrap()
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let (account, used) =
            prepare_with(store, PROFILE, &cancelled, |credentials, profile, _| {
                assert_eq!(profile.id, PROFILE);
                assert!(*credentials == before.credentials);
                *credentials = next.credentials.clone();
                Ok(true)
            })
            .unwrap();

        let reopened = Store::open(fixture.directory.path()).unwrap();
        assert_ne!(account, previous);
        assert_eq!(
            reopened.account(ProviderId::Spotify, PROFILE).unwrap(),
            Some(account.clone())
        );
        assert!(reopened.credentials(&account).unwrap().unwrap() == next.credentials);
        assert!(used == next.credentials);
        assert!(
            !store
                .save_music_snapshot(
                    ProviderId::Spotify,
                    Some(&previous),
                    "FR",
                    "saved-albums",
                    &Vec::<String>::new()
                )
                .unwrap()
        );
        assert!(
            store
                .save_music_snapshot(
                    ProviderId::Spotify,
                    Some(&account),
                    "FR",
                    "saved-albums",
                    &Vec::<String>::new()
                )
                .unwrap()
        );

        let cancelled_grant = session("third-access", "third-refresh", 3);
        let result = prepare_with(store, PROFILE, &cancelled, |credentials, _, cancelled| {
            assert!(*credentials == next.credentials);
            *credentials = cancelled_grant.credentials.clone();
            cancelled.store(true, Ordering::Relaxed);
            Ok(true)
        });
        assert!(matches!(
            result,
            Err(AccessError::Provider(ProviderError {
                kind: ErrorKind::Cancelled,
                ..
            }))
        ));
        let account = reopened
            .account(ProviderId::Spotify, PROFILE)
            .unwrap()
            .unwrap();
        assert!(reopened.credentials(&account).unwrap().unwrap() == cancelled_grant.credentials);
    }

    #[test]
    fn discards_a_grant_when_the_account_is_disconnected_during_the_exchange() {
        let fixture = Fixture::new(&session("old-access", "old-refresh", 1));
        let store = &fixture.store;
        let result = prepare_with(
            store,
            PROFILE,
            &AtomicBool::new(false),
            |credentials, _, _| {
                assert!(
                    store
                        .disconnect_account(ProviderId::Spotify, PROFILE, || Some(()))
                        .unwrap()
                        .is_some()
                );
                *credentials = session("new-access", "new-refresh", 2).credentials;
                Ok(true)
            },
        );
        assert!(matches!(
            result,
            Err(AccessError::Provider(ProviderError {
                kind: ErrorKind::InvalidInput,
                ..
            }))
        ));
        assert!(
            store
                .account(ProviderId::Spotify, PROFILE)
                .unwrap()
                .is_none()
        );
    }
}
