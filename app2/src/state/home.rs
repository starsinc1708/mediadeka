use crate::{
    providers::{
        ErrorKind, Home, HomeUpdate, ProviderError, ProviderId, Session, SoundCloudUpdate,
    },
    storage::{Snapshot, Store, StoreError},
};

use super::{ActiveSession, HomeContent, HomeView, Issue, Request, Screen};

impl HomeContent {
    pub(crate) fn connection_problem(&mut self, error: ProviderError) -> bool {
        if matches!(self, Self::Cached { issue: Issue::Provider(issue), .. } | Self::Empty(Issue::Provider(issue)) if *issue == error)
        {
            return false;
        }
        *self = match std::mem::replace(self, Self::Loading) {
            Self::Fresh(snapshot)
            | Self::Partial { snapshot, .. }
            | Self::Cached { snapshot, .. }
                if error.kind != ErrorKind::Revoked =>
            {
                Self::Cached {
                    snapshot,
                    issue: error.into(),
                }
            }
            _ => Self::Empty(error.into()),
        };
        true
    }
}

impl Request {
    pub(super) fn guest_home(&self, store: &Store, result: Result<Home, ProviderError>) -> Screen {
        self.home(
            store,
            ActiveSession::Guest,
            result.map(HomeUpdate::Complete),
        )
    }

    pub(super) fn account_home(
        &self,
        store: &Store,
        session: Session,
        result: Result<HomeUpdate, ProviderError>,
    ) -> Screen {
        self.home(store, ActiveSession::from(session), result)
    }

    pub(super) fn home(
        &self,
        store: &Store,
        session: ActiveSession,
        result: Result<HomeUpdate, ProviderError>,
    ) -> Screen {
        let content = self.home_content(store, &session, result);
        self.home_with_content(store, session, content)
    }

    pub(super) fn home_content(
        &self,
        store: &Store,
        session: &ActiveSession,
        result: Result<HomeUpdate, ProviderError>,
    ) -> HomeContent {
        match result {
            Ok(HomeUpdate::Complete(home)) => self.save_complete_home(store, session, home),
            Ok(HomeUpdate::SoundCloud(update)) if update.is_partial() => {
                self.partial_soundcloud(store, session, &update)
            }
            Ok(HomeUpdate::SoundCloud(update)) => {
                self.save_complete_home(store, session, update.apply(None).0)
            }
            Err(error) => fallback(store, session, error),
        }
    }

    fn save_complete_home(
        &self,
        store: &Store,
        session: &ActiveSession,
        home: Home,
    ) -> HomeContent {
        let snapshot = Snapshot::received(home);
        if !self.is_cancelled()
            && let Err(error) =
                store.save_session_cache(session.cache_scope(), &snapshot, || self.commit_permit())
        {
            report_cache_write_failure(error);
        }
        HomeContent::Fresh(snapshot)
    }

    fn partial_soundcloud(
        &self,
        store: &Store,
        session: &ActiveSession,
        update: &SoundCloudUpdate,
    ) -> HomeContent {
        if update.may_update_cache() && !self.is_cancelled() {
            match store.merge_session_cache(
                session.cache_scope(),
                |previous| update.apply(previous),
                || self.commit_permit(),
            ) {
                Ok((snapshot, issue)) => return partial_result(snapshot, issue),
                Err(error) => report_cache_write_failure(error),
            }
        }
        let previous = saved_snapshot(store, session);
        let (home, issue) = update.apply(previous.as_ref().map(|snapshot| &snapshot.home));
        partial_result(Snapshot::received(home), issue)
    }

    pub(super) fn home_with_content(
        &self,
        store: &Store,
        session: ActiveSession,
        content: HomeContent,
    ) -> Screen {
        Screen::Home(self.home_view(store, session, content))
    }

    pub(super) fn home_view(
        &self,
        store: &Store,
        session: ActiveSession,
        content: HomeContent,
    ) -> HomeView {
        let (accounts, switch_issue) = match store.accounts() {
            Ok(accounts) => (accounts, None),
            Err(error) => (Vec::new(), Some(Box::new(Issue::Storage(error)))),
        };
        HomeView {
            session,
            content,
            accounts,
            switch_issue,
        }
    }
}

fn partial_result(snapshot: Snapshot, issue: Option<ProviderError>) -> HomeContent {
    match issue {
        Some(error) => HomeContent::Partial {
            snapshot,
            issue: error.into(),
        },
        None => HomeContent::Fresh(snapshot),
    }
}

fn fallback(store: &Store, session: &ActiveSession, error: ProviderError) -> HomeContent {
    if (is_temporary(&error)
        || (matches!(session, ActiveSession::Account { .. }) && error.kind == ErrorKind::Expired))
        && let Some(snapshot) = saved_snapshot(store, session)
    {
        return HomeContent::Cached {
            snapshot,
            issue: Issue::Provider(error),
        };
    }
    HomeContent::Empty(Issue::Provider(error))
}

pub(super) fn saved_snapshot(store: &Store, session: &ActiveSession) -> Option<Snapshot> {
    match read_snapshot(store, session) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("Не удалось прочитать музыкальную подборку Mediadeka: {error}");
            None
        }
    }
}

pub(super) fn read_snapshot(
    store: &Store,
    session: &ActiveSession,
) -> Result<Option<Snapshot>, StoreError> {
    let spotify = matches!(
        session,
        ActiveSession::Account {
            provider: ProviderId::Spotify,
            ..
        }
    );
    Ok(store
        .cache(session.cache_scope())?
        .filter(|snapshot| !spotify || !snapshot.home.sections.is_empty()))
}

pub(super) fn is_temporary(error: &ProviderError) -> bool {
    matches!(error.kind, ErrorKind::Network | ErrorKind::Unavailable)
}

fn report_cache_write_failure(error: StoreError) {
    eprintln!("Не удалось сохранить музыкальную подборку Mediadeka: {error}");
}
