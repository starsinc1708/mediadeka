use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::{
    providers::{self, ErrorKind, ProviderError, ProviderId, Session},
    storage::{Account, Store},
};

use super::{
    ActiveSession, HomeContent, Intent, Issue, Request, Screen,
    home::{is_temporary, saved_snapshot},
    missing_account,
};

const SPOTIFY_FEED_TTL: Duration = Duration::from_secs(5 * 60);
pub(super) const RENEWAL_LEAD: Duration = Duration::from_secs(5 * 60);

impl Request {
    pub(super) fn restore_account(&self, store: &Store, account: Account) -> Result<Screen, Issue> {
        let (session, content) = self.restore_account_content(store, account)?;
        Ok(self.home_with_content(store, session, content))
    }

    pub(super) fn restore_account_content(
        &self,
        store: &Store,
        account: Account,
    ) -> Result<(ActiveSession, HomeContent), Issue> {
        let active = ActiveSession::from(&account);
        let session = match restore_connection(store, &account, self.cancel_flag(), false) {
            Ok(session) => session,
            Err(Issue::Provider(error)) => return self.restore_error(store, active, error),
            Err(issue) => return Err(issue),
        };
        self.check_cancelled()?;
        if matches!(self.intent, Intent::Startup | Intent::SwitchAccount)
            && account.provider == ProviderId::Spotify
            && let Some(snapshot) = saved_snapshot(store, &active)
            && snapshot.is_recent(SPOTIFY_FEED_TTL)
        {
            return Ok((ActiveSession::from(session), HomeContent::Fresh(snapshot)));
        }
        let result = providers::load_home(&session, self.cancel_flag());
        self.check_cancelled()?;
        if result
            .as_ref()
            .is_err_and(|error| error.kind == ErrorKind::Revoked)
            && let Some(current) = store.account(session.provider, &session.profile.id)?
            && store.credentials(&current)?.as_ref() == Some(&session.credentials)
        {
            store.forget_music_account(&current)?;
        }
        let active = ActiveSession::from(session);
        let content = self.home_content(store, &active, result);
        Ok((active, content))
    }

    pub(super) fn restore_error(
        &self,
        store: &Store,
        active: ActiveSession,
        error: ProviderError,
    ) -> Result<(ActiveSession, HomeContent), Issue> {
        if error.kind == ErrorKind::Cancelled {
            return Err(error.into());
        }
        if matches!(
            self.intent,
            Intent::Startup | Intent::Guest | Intent::Disconnect(_)
        ) || is_temporary(&error)
            || error.needs_sign_in()
        {
            // Keep the source picker available when the active account needs to sign in again.
            let content = self.home_content(store, &active, Err(error));
            return Ok((active, content));
        }
        Err(error.into())
    }
}

/// Confirm identity and persist renewed credentials without reading a feed or changing the route.
pub(super) fn restore_connection(
    store: &Store,
    account: &Account,
    cancelled: &AtomicBool,
    proactive: bool,
) -> Result<Session, Issue> {
    let provider = account.provider;
    if cancelled.load(Ordering::Relaxed) {
        return Err(ProviderError {
            kind: ErrorKind::Cancelled,
            message: "Проверка подключения отменена.".into(),
        }
        .into());
    }
    let _lock = if matches!(
        provider,
        ProviderId::Spotify | ProviderId::YandexMusic | ProviderId::SoundCloud
    ) {
        Some(
            store
                .lock_connection(provider, cancelled)?
                .ok_or_else(|| ProviderError {
                    kind: if cancelled.load(Ordering::Relaxed) {
                        ErrorKind::Cancelled
                    } else {
                        ErrorKind::Unavailable
                    },
                    message: "Подключение уже обновляется.".into(),
                })?,
        )
    } else {
        None
    };
    let account = store
        .account(provider, &account.profile.id)?
        .ok_or_else(missing_account)?;
    if store.account_revoked(&account)? {
        return Err(revoked_connection(provider).into());
    }
    let credentials = store.credentials(&account)?.ok_or_else(|| ProviderError {
        kind: ErrorKind::Expired,
        message: format!(
            "Подключение к {} истекло. Войдите снова.",
            provider.display_name()
        ),
    })?;
    let result = providers::restore_in_profile(
        provider,
        &account.profile,
        credentials.clone(),
        store.directory(),
        cancelled,
    );
    let mut session = match result {
        Ok(session) => session,
        Err(error) => {
            if error.kind == ErrorKind::Revoked && !cancelled.load(Ordering::Relaxed) {
                store.forget_music_account(&account)?;
            }
            return Err(error.into());
        }
    };
    check_identity(&account, &session)?;

    let deadline = session.credentials.renewal_deadline();
    let due = proactive
        && deadline == credentials.renewal_deadline()
        && deadline
            .zip(providers::epoch_seconds())
            .is_some_and(|(expiry, now)| expiry <= now.saturating_add(RENEWAL_LEAD.as_secs()));
    let renewal_error = if due && !cancelled.load(Ordering::Relaxed) {
        match providers::renew(session.clone(), cancelled) {
            Ok(renewed) => {
                check_identity(&account, &renewed)?;
                session = renewed;
                None
            }
            // The current session just passed verification. A failed early exchange
            // leaves it usable and gets a later retry.
            Err(error) => Some(ProviderError {
                kind: if error.needs_sign_in() {
                    ErrorKind::Unavailable
                } else {
                    error.kind
                },
                message: error.message,
            }),
        }
    } else {
        None
    };

    let changed = session.credentials != credentials
        || session.profile != account.profile
        || session.capabilities != account.capabilities;
    if changed {
        // Commit a rotated grant even after a late view cancellation, conditional on the old binding.
        if !store.update_account(&session, &account, || Some(()))? {
            return Err(missing_account());
        }
    } else if store.account(provider, &account.profile.id)?.as_ref() != Some(&account) {
        return Err(missing_account());
    }
    if let Some(error) = renewal_error {
        return Err(error.into());
    }
    Ok(session)
}

fn check_identity(account: &Account, session: &Session) -> Result<(), ProviderError> {
    if session.provider == account.provider && session.profile.id == account.profile.id {
        return Ok(());
    }
    Err(ProviderError {
        kind: ErrorKind::Expired,
        message: "Сервис вернул другой профиль. Войдите снова в сохранённый аккаунт.".into(),
    })
}

fn revoked_connection(provider: ProviderId) -> ProviderError {
    ProviderError {
        kind: ErrorKind::Revoked,
        message: format!(
            "Доступ к {} отозван. Войдите снова.",
            provider.display_name()
        ),
    }
}
