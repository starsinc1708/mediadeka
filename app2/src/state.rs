
mod home;
mod home_sources;
mod maintenance;
mod recovery;
#[cfg(all(test, windows))]
mod recovery_tests;

pub(crate) use home_sources::HomeSourceRequest;
pub(crate) use maintenance::{ConnectionCheck, ConnectionMaintenance};

use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock, PoisonError,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    providers::{self, Capabilities, ErrorKind, Profile, ProviderError, ProviderId, Session},
    storage::{Account, AccountSummary, CacheScope, Snapshot, Startup, Store, StoreError},
};

use self::home::saved_snapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intent {
    Startup,
    Guest,
    SignIn(ProviderId),
    Connect(ProviderId),
    Disconnect(ProviderId),
    SwitchAccount,
    Refresh,
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    intent: Intent,
    control: Arc<RequestControl>,
    selected_account: Option<(ProviderId, String)>,
    active_session: Option<ActiveSession>,
}

#[derive(Debug, Default)]
struct RequestControl {
    cancelled: AtomicBool,
    connection_committed: AtomicBool,
    reconnect_account: OnceLock<Account>,
    commit: Mutex<()>,
}

#[derive(Debug, Default)]
pub(crate) enum Screen {
    #[default]
    Welcome,
    Pending(Intent),
    Cancelled(Intent),
    Home(HomeView),
    Failed(Failure),
}

#[derive(Debug)]
pub(crate) struct HomeView {
    pub session: ActiveSession,
    pub content: HomeContent,
    pub accounts: Vec<AccountSummary>,
    pub switch_issue: Option<Box<Issue>>,
}

#[derive(Clone, Debug)]
pub(crate) enum ActiveSession {
    Guest,
    Account {
        provider: ProviderId,
        profile: Profile,
        capabilities: Capabilities,
    },
}

#[derive(Debug)]
pub(crate) enum HomeContent {
    Loading,
    Fresh(Snapshot),
    Partial { snapshot: Snapshot, issue: Issue },
    Cached { snapshot: Snapshot, issue: Issue },
    Empty(Issue),
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub intent: Intent,
    pub issue: Issue,
}

#[derive(Debug)]
pub(crate) enum Issue {
    Provider(ProviderError),
    Storage(StoreError),
}

#[derive(Default)]
pub(crate) struct State {
    screen: Screen,
    pending: Option<Request>,
    home_preview: Option<HomeView>,
    previous_home: Option<HomeView>,
}

impl State {
    pub(crate) fn note_connection_problem(
        &mut self,
        provider: ProviderId,
        profile: &str,
        error: ProviderError,
    ) -> bool {
        let Screen::Home(view) = &mut self.screen else {
            return false;
        };
        if view.session.cache_scope() != CacheScope::Account(provider, profile) {
            return false;
        }
        view.content.connection_problem(error)
    }

    pub(crate) fn replace_connection_feed(
        &mut self,
        provider: ProviderId,
        profile: &str,
        content: HomeContent,
    ) -> bool {
        let Screen::Home(view) = &mut self.screen else {
            return false;
        };
        if view.session.cache_scope() != CacheScope::Account(provider, profile) {
            return false;
        }
        view.content = content;
        true
    }
    pub(crate) fn screen(&self) -> &Screen {
        &self.screen
    }

    pub(crate) fn home_view(&self) -> Option<(&HomeView, bool)> {
        match &self.screen {
            Screen::Home(home) => Some((home, false)),
            Screen::Pending(Intent::Startup) => self.home_preview.as_ref().map(|home| (home, true)),
            Screen::Pending(Intent::SwitchAccount | Intent::Guest) => self
                .home_preview
                .as_ref()
                .or(self.previous_home.as_ref())
                .map(|home| (home, true)),
            Screen::Pending(Intent::Refresh | Intent::Disconnect(_)) => {
                self.previous_home.as_ref().map(|home| (home, true))
            }
            Screen::Pending(Intent::Connect(_))
            | Screen::Cancelled(Intent::Connect(_))
            | Screen::Failed(Failure {
                intent: Intent::Connect(_),
                ..
            }) => self.previous_home.as_ref().map(|home| (home, false)),
            _ => None,
        }
    }

    pub(crate) fn set_home_preview(&mut self, request: &Request, home: HomeView) -> bool {
        if !matches!(
            request.intent,
            Intent::Startup | Intent::SwitchAccount | Intent::Guest
        ) || !self.accepts(request)
        {
            return false;
        }
        self.home_preview = Some(home);
        true
    }

    pub(crate) fn keeps_cached_preview(&self, next: &Screen) -> bool {
        self.home_preview.as_ref().is_some_and(|preview| {
            matches!(preview.content, HomeContent::Fresh(_))
                && match next {
                    Screen::Home(home) => {
                        preview.session.cache_scope() == home.session.cache_scope()
                    }
                    _ => matches!(self.screen, Screen::Pending(Intent::Startup)),
                }
        })
    }

    pub(crate) fn has_youtube_account(&self) -> bool {
        let home = self.home_view().map(|(home, _)| home);
        home.is_some_and(|home| {
            matches!(
                home.session,
                ActiveSession::Account {
                    provider: ProviderId::YouTubeMusic,
                    ..
                }
            ) || home
                .accounts
                .iter()
                .any(|account| account.provider == ProviderId::YouTubeMusic)
        })
    }

    pub(crate) fn begin(&mut self, intent: Intent) -> Request {
        self.begin_request(intent, None)
    }

    pub(crate) fn begin_account(&mut self, provider: ProviderId, profile_id: String) -> Request {
        self.begin_request(Intent::SwitchAccount, Some((provider, profile_id)))
    }

    fn begin_request(
        &mut self,
        intent: Intent,
        selected_account: Option<(ProviderId, String)>,
    ) -> Request {
        self.cancel();
        if matches!(
            intent,
            Intent::Refresh
                | Intent::SwitchAccount
                | Intent::Guest
                | Intent::Connect(_)
                | Intent::Disconnect(_)
        ) && matches!(self.screen, Screen::Home(_))
            && let Screen::Home(home) = std::mem::take(&mut self.screen)
        {
            self.previous_home = Some(home);
        }
        let selected_account = match intent {
            Intent::Connect(provider) | Intent::Disconnect(provider) => {
                self.previous_home.as_ref().and_then(|home| {
                    home.accounts
                        .iter()
                        .find(|account| account.provider == provider)
                        .map(|account| (provider, account.profile.id.clone()))
                })
            }
            _ => selected_account,
        };
        let request = Request::new(
            intent,
            selected_account,
            matches!(
                intent,
                Intent::Guest | Intent::Connect(_) | Intent::Disconnect(_)
            )
            .then(|| self.previous_home.as_ref().map(|home| home.session.clone()))
            .flatten(),
        );
        self.screen = Screen::Pending(intent);
        self.pending = Some(request.clone());
        request
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(request) = self.pending.take() {
            request.retire();
            let preview = self.home_preview.take().filter(|home| {
                request.intent == Intent::Startup && matches!(home.content, HomeContent::Fresh(_))
            });
            self.screen = self
                .previous_home
                .take()
                .or(preview)
                .map_or(Screen::Cancelled(request.intent), Screen::Home);
        }
    }

    pub(crate) fn dismiss_connection(&mut self) -> bool {
        if let Some(request) = self.pending.as_ref()
            && matches!(request.intent, Intent::Connect(_))
            && !request.cancel_connection()
        {
            // Let the completed connection reach the UI before closing its modal.
            return false;
        }
        if matches!(
            self.screen,
            Screen::Pending(Intent::Connect(_))
                | Screen::Cancelled(Intent::Connect(_))
                | Screen::Failed(Failure {
                    intent: Intent::Connect(_),
                    ..
                })
        ) {
            self.cancel();
            if let Some(home) = self.previous_home.take() {
                self.screen = Screen::Home(home);
            }
        }
        true
    }

    pub(crate) fn can_leave_login(&self) -> bool {
        self.pending.as_ref().is_none_or(|request| {
            !matches!(request.intent, Intent::SignIn(_) | Intent::Connect(_))
                || request.cancel_connection()
        })
    }

    pub(crate) fn reset(&mut self) {
        self.cancel();
        self.previous_home = None;
        self.screen = Screen::Welcome;
    }

    pub(crate) fn accepts(&self, request: &Request) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            Arc::ptr_eq(&pending.control, &request.control) && !request.is_cancelled()
        })
    }

    pub(crate) fn finish(&mut self, request: &Request, next: Screen) -> bool {
        if !self.accepts(request) {
            return false;
        }
        request.retire();
        self.pending = None;
        if matches!(request.intent, Intent::Connect(_)) && !matches!(next, Screen::Home(_)) {
            self.screen = next;
            return true;
        }
        let preview = self
            .home_preview
            .take()
            .filter(|_| request.intent == Intent::Startup);
        let previous = self.previous_home.take().or(preview);
        self.screen = match (previous, next) {
            (Some(previous), Screen::Home(mut home))
                if matches!(request.intent, Intent::Connect(_) | Intent::Disconnect(_))
                    && !request.reconnects(&previous.session)
                    && previous.session.cache_scope() == home.session.cache_scope() =>
            {
                home.content = previous.content;
                Screen::Home(home)
            }
            (Some(home), Screen::Cancelled(_)) if !matches!(home.content, HomeContent::Loading) => {
                Screen::Home(home)
            }
            (Some(mut home), Screen::Failed(failure))
                if matches!(request.intent, Intent::Startup | Intent::Refresh) =>
            {
                home.content = match home.content {
                    HomeContent::Fresh(snapshot)
                    | HomeContent::Partial { snapshot, .. }
                    | HomeContent::Cached { snapshot, .. } => HomeContent::Cached {
                        snapshot,
                        issue: failure.issue,
                    },
                    HomeContent::Loading | HomeContent::Empty(_) => {
                        HomeContent::Empty(failure.issue)
                    }
                };
                Screen::Home(home)
            }
            (Some(mut home), Screen::Failed(failure)) => {
                home.switch_issue = Some(Box::new(failure.issue));
                Screen::Home(home)
            }
            (_, next) => next,
        };
        true
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Request {
    pub(crate) fn is_startup(&self) -> bool {
        self.intent == Intent::Startup
    }

    fn new(
        intent: Intent,
        selected_account: Option<(ProviderId, String)>,
        active_session: Option<ActiveSession>,
    ) -> Self {
        Self {
            intent,
            control: Arc::new(RequestControl::default()),
            selected_account,
            active_session,
        }
    }

    pub(crate) fn cancel_flag(&self) -> &AtomicBool {
        &self.control.cancelled
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag().load(Ordering::SeqCst)
    }

    fn retire(&self) {
        let _commit = self.lock_commit();
        self.cancel_flag().store(true, Ordering::SeqCst);
    }

    fn cancel_connection(&self) -> bool {
        let _commit = self.lock_commit();
        if self.control.connection_committed.load(Ordering::Relaxed) {
            return false;
        }
        self.cancel_flag().store(true, Ordering::SeqCst);
        true
    }

    fn lock_commit(&self) -> MutexGuard<'_, ()> {
        self.control
            .commit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn commit_permit(&self) -> Option<MutexGuard<'_, ()>> {
        // Preparation, I/O wait for the write transaction, and vault writes happen before this
        // guard. Cancellation and the final SQLite commit share one ordering point.
        let commit = self.lock_commit();
        (!self.is_cancelled()).then_some(commit)
    }

    pub(crate) fn store_failure(&self, error: StoreError) -> Screen {
        self.failed(Issue::Storage(error))
    }

    pub(crate) fn login_preparation_failure(&self, store: &Store) -> Option<Screen> {
        if matches!(self.intent, Intent::Connect(_))
            && self.selected_account.is_some()
            && self.control.reconnect_account.get().is_none()
        {
            match self.requested_account(store) {
                Ok(account) => {
                    let _ = self.control.reconnect_account.set(account);
                }
                Err(issue) => return Some(self.failed(issue)),
            }
        }
        None
    }

    pub(crate) fn reconnected_provider(&self) -> Option<ProviderId> {
        self.control
            .reconnect_account
            .get()
            .filter(|_| self.control.connection_committed.load(Ordering::Relaxed))
            .map(|account| account.provider)
    }

    fn reconnects(&self, active: &ActiveSession) -> bool {
        self.control.reconnect_account.get().is_some_and(|account| {
            active.cache_scope() == CacheScope::Account(account.provider, &account.profile.id)
        })
    }

    pub(crate) fn home_preview(&self, store: &Store) -> Option<HomeView> {
        if self.is_cancelled() {
            return None;
        }
        let account = match self.intent {
            Intent::Startup => match store.startup().ok()? {
                Startup::Welcome => return None,
                Startup::Guest => store.preferred_account(ProviderId::YouTubeMusic).ok()?,
                Startup::Account(account) => Some(account),
            },
            Intent::SwitchAccount => Some(self.requested_account(store).ok()?),
            Intent::Guest if self.active_session.is_some() => {
                store.preferred_account(ProviderId::YouTubeMusic).ok()?
            }
            _ => return None,
        };
        let session = account
            .as_ref()
            .map_or(ActiveSession::Guest, ActiveSession::from);
        let snapshot = saved_snapshot(store, &session).filter(|_| {
            account.as_ref().is_none_or(|account| {
                store
                    .credentials(account)
                    .is_ok_and(|credentials| credentials.is_some())
            })
        });
        let content = snapshot.map_or(HomeContent::Loading, HomeContent::Fresh);
        (!self.is_cancelled()).then(|| self.home_view(store, session, content))
    }

    pub(crate) fn run(&self, store: &Store) -> Screen {
        self.resolve(|| match self.intent {
            Intent::Startup => {
                store.retry_credential_cleanup();
                self.restore_route(store)
            }
            Intent::Refresh => self.restore_route(store),
            Intent::Guest => self.select_guest(store),
            Intent::SwitchAccount => self.switch_account(store),
            Intent::Disconnect(provider) => self.disconnect_account(store, provider),
            Intent::SignIn(_) | Intent::Connect(_) => {
                Err(input_issue("Ожидается завершение входа через сервис."))
            }
        })
    }

    pub(crate) fn finish_login(
        &self,
        store: &Store,
        result: Result<Session, ProviderError>,
    ) -> Screen {
        self.resolve(|| {
            let session = result?;
            if !matches!(self.intent, Intent::SignIn(provider) | Intent::Connect(provider) if provider == session.provider)
                || session.profile.id.trim().is_empty()
            {
                return Err(invalid_response(
                    "Профиль сервиса не соответствует запросу входа.",
                ));
            }
            if self.intent == Intent::Connect(session.provider) {
                let committed = || self.control.connection_committed.store(true, Ordering::Relaxed);
                let route = if self.selected_account.is_some() {
                    let previous = self.control.reconnect_account.get().ok_or_else(missing_account)?;
                    if previous.profile.id != session.profile.id {
                        return Err(input_issue("Войдите в тот же аккаунт, чтобы восстановить подключение."));
                    }
                    store.reconnect_account(&session, previous, || self.commit_permit(), committed)?
                } else {
                    store.connect_account(&session, || self.commit_permit(), committed)?
                };
                let Some(route) = route else {
                    self.check_cancelled()?;
                    return Err(missing_account());
                };
                if self.selected_account.is_some()
                    && matches!(&route, Startup::Account(account) if account.provider == session.provider && account.profile.id == session.profile.id)
                {
                    let result = providers::load_home(&session, self.cancel_flag());
                    return Ok(self.account_home(store, session, result));
                }
                return self.home_after_account_change(store, route);
            }
            if !store.use_account(
                &session,
                || self.commit_permit(),
                || self.control.connection_committed.store(true, Ordering::Relaxed),
            )? {
                return Ok(Screen::Cancelled(self.intent));
            }
            let result = providers::load_home(&session, self.cancel_flag());
            Ok(self.account_home(store, session, result))
        })
    }

    fn disconnect_account(&self, store: &Store, provider: ProviderId) -> Result<Screen, Issue> {
        let Some((selected, id)) = &self.selected_account else {
            return Err(missing_account());
        };
        if *selected != provider {
            return Err(missing_account());
        }
        let _soundcloud_access = if provider == ProviderId::SoundCloud {
            let Some(lock) = store.lock_connection(provider, self.cancel_flag())? else {
                self.check_cancelled()?;
                return Err(ProviderError {
                    kind: ErrorKind::Unavailable,
                    message: "Подключение SoundCloud обновляется. Повторите выход чуть позже."
                        .into(),
                }
                .into());
            };
            Some(lock)
        } else {
            None
        };
        let Some(route) = store.disconnect_account(provider, id, || self.commit_permit())? else {
            self.check_cancelled()?;
            return Err(missing_account());
        };
        if provider == ProviderId::SoundCloud
            && let Err(error) = providers::clear_soundcloud_browser_session(
                store.directory(),
                &AtomicBool::new(false),
            )
        {
            eprintln!("Не удалось очистить браузерную сессию SoundCloud: {error}");
        }
        self.home_after_account_change(store, route)
    }

    fn home_after_account_change(&self, store: &Store, route: Startup) -> Result<Screen, Issue> {
        let route = if matches!(route, Startup::Welcome) {
            Startup::Guest
        } else {
            route
        };
        let session = match &route {
            Startup::Account(account) => ActiveSession::from(account),
            Startup::Guest | Startup::Welcome => ActiveSession::Guest,
        };
        if self
            .active_session
            .as_ref()
            .is_some_and(|active| active.cache_scope() == session.cache_scope())
        {
            // State carries forward the current collection at the same time as these accounts.
            return Ok(self.home_with_content(store, session, HomeContent::Loading));
        }
        // Once the account change has committed, subsequent load failures belong to the
        // fallback source. The old account must not reappear in the UI.
        Ok(match self.load_route(store, route) {
            Err(issue) => self.home_with_content(store, session, HomeContent::Empty(issue)),
            Ok(screen) => screen,
        })
    }

    fn switch_account(&self, store: &Store) -> Result<Screen, Issue> {
        self.select_existing_account(store, self.requested_account(store)?)
    }

    fn requested_account(&self, store: &Store) -> Result<Account, Issue> {
        let Some((provider, id)) = &self.selected_account else {
            return Err(input_issue("Выберите подключённый аккаунт."));
        };
        store.account(*provider, id)?.ok_or_else(missing_account)
    }

    fn select_existing_account(&self, store: &Store, account: Account) -> Result<Screen, Issue> {
        let provider = account.provider;
        let id = account.profile.id.clone();
        let home = self.restore_account(store, account)?;
        self.check_cancelled()?;
        if !store.select_account(provider, &id, || self.commit_permit())? {
            self.check_cancelled()?;
            return Err(missing_account());
        }
        Ok(home)
    }

    fn select_guest(&self, store: &Store) -> Result<Screen, Issue> {
        if let Some(account) = store.preferred_account(ProviderId::YouTubeMusic)? {
            return self.select_existing_account(store, account);
        }
        let result = providers::guest_home(self.cancel_flag());
        self.check_cancelled()?;
        if store.use_guest(|| self.commit_permit())? {
            return Ok(self.guest_home(store, result));
        }
        self.check_cancelled()?;
        let account = store
            .preferred_account(ProviderId::YouTubeMusic)?
            .ok_or_else(missing_account)?;
        self.select_existing_account(store, account)
    }

    fn restore_route(&self, store: &Store) -> Result<Screen, Issue> {
        let mut route = store.startup()?;
        if matches!(route, Startup::Guest)
            && store.preferred_account(ProviderId::YouTubeMusic)?.is_some()
        {
            store.upgrade_guest_route(|| self.commit_permit())?;
            self.check_cancelled()?;
            route = store.startup()?;
        }
        self.load_route(store, route)
    }

    fn load_route(&self, store: &Store, route: Startup) -> Result<Screen, Issue> {
        match route {
            Startup::Welcome => Ok(Screen::Welcome),
            Startup::Guest => {
                let result = providers::guest_home(self.cancel_flag());
                self.check_cancelled()?;
                Ok(self.guest_home(store, result))
            }
            Startup::Account(account) => self.restore_account(store, account),
        }
    }

    fn resolve(&self, run: impl FnOnce() -> Result<Screen, Issue>) -> Screen {
        let result = self.check_cancelled().and_then(|()| run());
        if self.is_cancelled() {
            return Screen::Cancelled(self.intent);
        }
        match result {
            Ok(screen) => screen,
            Err(Issue::Provider(error)) if error.kind == ErrorKind::Cancelled => {
                Screen::Cancelled(self.intent)
            }
            Err(issue) => self.failed(issue),
        }
    }

    fn check_cancelled(&self) -> Result<(), Issue> {
        if self.is_cancelled() {
            return Err(Issue::Provider(ProviderError {
                kind: ErrorKind::Cancelled,
                message: "Действие отменено.".into(),
            }));
        }
        Ok(())
    }

    fn failed(&self, issue: Issue) -> Screen {
        self.failed_for(self.intent, issue)
    }

    fn failed_for(&self, intent: Intent, issue: Issue) -> Screen {
        Screen::Failed(Failure { intent, issue })
    }
}

impl ActiveSession {
    fn cache_scope(&self) -> CacheScope<'_> {
        match self {
            Self::Guest => CacheScope::Guest,
            Self::Account {
                provider, profile, ..
            } => CacheScope::Account(*provider, &profile.id),
        }
    }
}

impl From<&Account> for ActiveSession {
    fn from(account: &Account) -> Self {
        Self::Account {
            provider: account.provider,
            profile: account.profile.clone(),
            capabilities: account.capabilities,
        }
    }
}

impl From<Session> for ActiveSession {
    fn from(session: Session) -> Self {
        Self::Account {
            provider: session.provider,
            profile: session.profile,
            capabilities: session.capabilities,
        }
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(error) => formatter.write_str(&error.message),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl From<StoreError> for Issue {
    fn from(error: StoreError) -> Self {
        Self::Storage(error)
    }
}

impl From<ProviderError> for Issue {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

fn invalid_response(message: &str) -> Issue {
    Issue::Provider(ProviderError {
        kind: ErrorKind::InvalidResponse,
        message: message.into(),
    })
}

fn input_issue(message: &str) -> Issue {
    Issue::Provider(ProviderError {
        kind: ErrorKind::InvalidInput,
        message: message.into(),
    })
}

fn missing_account() -> Issue {
    input_issue("Подключение изменилось. Выберите аккаунт снова.")
}

#[cfg(test)]
#[path = "state/tests.rs"]
mod tests;
