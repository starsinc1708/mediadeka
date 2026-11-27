//! A single account or guest feed refresh, independent of the active session.

use crate::{
    providers::{self, HomeUpdate, ProviderId},
    storage::{Account, Snapshot, Store},
};

use super::{
    ActiveSession, HomeContent, Intent, Issue, Request, home::read_snapshot, input_issue,
    missing_account,
};

#[derive(Clone)]
pub(crate) struct HomeSourceRequest(Request);

impl Default for HomeSourceRequest {
    fn default() -> Self {
        Self(Request::new(Intent::Refresh, None, None))
    }
}

impl HomeSourceRequest {
    pub(crate) fn for_startup() -> Self {
        Self(Request::new(Intent::Startup, None, None))
    }

    pub(crate) fn cancel(&self) {
        self.0.retire();
    }

    pub(crate) fn cached(
        &self,
        store: &Store,
        provider: ProviderId,
        profile: Option<&str>,
    ) -> Result<Option<Snapshot>, Issue> {
        self.0.check_cancelled()?;
        let account = current_account(store, provider, profile)?;
        let session = account
            .as_ref()
            .map_or(ActiveSession::Guest, ActiveSession::from);
        let snapshot = read_snapshot(store, &session)?;
        current_account(store, provider, profile)?;
        self.0.check_cancelled()?;
        Ok(snapshot)
    }

    pub(crate) fn refresh(
        &self,
        store: &Store,
        provider: ProviderId,
        profile: Option<&str>,
    ) -> Result<HomeContent, Issue> {
        self.0.check_cancelled()?;
        let content = if let Some(account) = current_account(store, provider, profile)? {
            self.0.restore_account_content(store, account)?.1
        } else {
            let result = providers::guest_home(self.0.cancel_flag()).map(HomeUpdate::Complete);
            self.0.check_cancelled()?;
            self.0.home_content(store, &ActiveSession::Guest, result)
        };
        current_account(store, provider, profile)?;
        self.0.check_cancelled()?;
        Ok(content)
    }
}

fn current_account(
    store: &Store,
    provider: ProviderId,
    profile: Option<&str>,
) -> Result<Option<Account>, Issue> {
    if let Some(profile) = profile {
        return store
            .account(provider, profile)?
            .map(Some)
            .ok_or_else(missing_account);
    }
    if provider != ProviderId::YouTubeMusic {
        return Err(input_issue(
            "Гостевая подборка доступна только в YouTube Music.",
        ));
    }
    if store.preferred_account(provider)?.is_some() {
        return Err(missing_account());
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::{Capabilities, ErrorKind, Home},
        storage::{CacheScope, Startup},
    };
    use rusqlite::{Connection, params};

    fn snapshot(title: &str, received_at_ms: i64) -> Snapshot {
        Snapshot {
            home: Home {
                title: title.into(),
                items: Vec::new(),
                sections: Vec::new(),
            },
            received_at_ms,
        }
    }

    #[test]
    fn source_cache_is_scoped_and_a_connected_youtube_blocks_guest_updates() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let provider = ProviderId::YouTubeMusic;
        let guest = snapshot("Guest", 1);
        let signed_in = snapshot("Connected YouTube", 2);
        assert!(store.use_guest(|| Some(())).unwrap());
        store
            .save_session_cache(CacheScope::Guest, &guest, || Some(()))
            .unwrap();
        let guest_request = HomeSourceRequest::default();
        assert_eq!(
            guest_request.cached(&store, provider, None).unwrap(),
            Some(guest.clone())
        );

        let database = Connection::open(directory.path().join("mediadeka.sqlite3")).unwrap();
        let capabilities = serde_json::to_string(&Capabilities {
            home: true,
            profile_selection: true,
            device_code: false,
        })
        .unwrap();
        database.execute(
            "INSERT INTO account (provider, profile_id, display_name, capabilities, credential_ref)
             VALUES (?1, 'own', 'YouTube', ?2, 'test-reference')",
            params![provider.as_str(), capabilities],
        ).unwrap();
        database
            .execute(
                "INSERT INTO account_source (provider, profile_id) VALUES (?1, 'own')",
                [provider.as_str()],
            )
            .unwrap();
        store
            .save_session_cache(
                CacheScope::Account(provider, "own"),
                &signed_in,
                || Some(()),
            )
            .unwrap();

        store
            .save_cache(
                CacheScope::Account(provider, "another"),
                &snapshot("Other profile", 3),
                || Some(()),
            )
            .unwrap();

        let signed_in_request = HomeSourceRequest::default();
        assert_eq!(
            signed_in_request
                .cached(&store, provider, Some("own"))
                .unwrap(),
            Some(signed_in)
        );
        assert!(
            signed_in_request
                .cached(&store, provider, Some("another"))
                .is_err()
        );
        assert!(
            signed_in_request
                .refresh(&store, provider, Some("another"))
                .is_err()
        );
        assert!(guest_request.cached(&store, provider, None).is_err());
        assert!(guest_request.refresh(&store, provider, None).is_err());
        store
            .save_session_cache(CacheScope::Guest, &snapshot("Late guest", 3), || Some(()))
            .unwrap();
        assert_eq!(store.cache(CacheScope::Guest).unwrap(), Some(guest));
        assert!(matches!(store.startup().unwrap(), Startup::Guest));
    }

    #[test]
    fn cancelled_request_does_not_start_or_publish_a_feed() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let request = HomeSourceRequest::default();
        let worker = request.clone();
        request.cancel();
        assert!(
            matches!(worker.cached(&store, ProviderId::YouTubeMusic, None), Err(Issue::Provider(error)) if error.kind == ErrorKind::Cancelled)
        );
        assert!(
            matches!(worker.refresh(&store, ProviderId::YouTubeMusic, None), Err(Issue::Provider(error)) if error.kind == ErrorKind::Cancelled)
        );
        assert!(matches!(store.startup().unwrap(), Startup::Welcome));
    }
}
