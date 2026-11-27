use std::{collections::BTreeMap, sync::Arc};

use gpui::{Context, Task};

use crate::{
    providers::{ErrorKind, ProviderId},
    state::{ActiveSession, ConnectionCheck, HomeContent, HomeSourceRequest, HomeView, Issue},
    storage::{Snapshot, Store},
};

use super::MainView;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct FeedSource {
    pub(super) provider: ProviderId,
    pub(super) profile: Option<String>,
}

impl FeedSource {
    pub(super) fn active(home: &HomeView) -> Self {
        match &home.session {
            ActiveSession::Guest => Self {
                provider: ProviderId::YouTubeMusic,
                profile: None,
            },
            ActiveSession::Account {
                provider, profile, ..
            } => Self {
                provider: *provider,
                profile: Some(profile.id.clone()),
            },
        }
    }

    pub(super) fn available(home: &HomeView) -> Vec<Self> {
        let mut sources: Vec<_> = home
            .accounts
            .iter()
            .map(|account| Self {
                provider: account.provider,
                profile: Some(account.profile.id.clone()),
            })
            .collect();
        if !sources
            .iter()
            .any(|source| source.provider == ProviderId::YouTubeMusic)
        {
            sources.push(Self {
                provider: ProviderId::YouTubeMusic,
                profile: None,
            });
        }
        sources
    }
}

#[derive(Default)]
pub(super) struct Feeds(BTreeMap<FeedSource, Entry>);

struct Entry {
    content: HomeContent,
    job: Option<Job>,
}

struct Job {
    request: Arc<HomeSourceRequest>,
    _task: Task<()>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.request.cancel();
    }
}

#[derive(Clone, Copy)]
pub(super) struct Feed<'a> {
    pub(super) content: Option<&'a HomeContent>,
    pub(super) refreshing: bool,
}

impl<'a> Feed<'a> {
    pub(super) fn status(self) -> &'static str {
        if self.refreshing || matches!(self.content, Some(HomeContent::Loading)) {
            return "Обновляется";
        }
        match self.issue() {
            Some(Issue::Provider(error)) if error.needs_sign_in() => "Нужен вход",
            Some(Issue::Provider(error)) if error.kind == ErrorKind::Network => "Нет сети",
            Some(_) => "Ошибка обновления",
            None => "Подключено",
        }
    }

    pub(super) fn snapshot(self) -> Option<&'a Snapshot> {
        self.content.and_then(snapshot)
    }

    pub(super) fn issue(self) -> Option<&'a Issue> {
        match self.content? {
            HomeContent::Partial { issue, .. }
            | HomeContent::Cached { issue, .. }
            | HomeContent::Empty(issue) => Some(issue),
            HomeContent::Loading | HomeContent::Fresh(_) => None,
        }
    }
}

fn snapshot(content: &HomeContent) -> Option<&Snapshot> {
    match content {
        HomeContent::Fresh(snapshot)
        | HomeContent::Partial { snapshot, .. }
        | HomeContent::Cached { snapshot, .. } => Some(snapshot),
        HomeContent::Loading | HomeContent::Empty(_) => None,
    }
}

fn can_retry_feed(content: &HomeContent) -> bool {
    match content {
        HomeContent::Partial { issue, .. }
        | HomeContent::Cached { issue, .. }
        | HomeContent::Empty(issue) => {
            matches!(issue, Issue::Provider(error) if error.kind != ErrorKind::Revoked)
        }
        _ => false,
    }
}

impl Feeds {
    #[cfg(feature = "capture")]
    pub(super) fn set_for_capture(&mut self, source: FeedSource, content: HomeContent) {
        self.0.insert(source, Entry { content, job: None });
    }

    pub(super) fn get<'a>(
        &'a self,
        source: &FeedSource,
        active: &FeedSource,
        view: &'a HomeView,
        refreshing: bool,
    ) -> Feed<'a> {
        if source == active {
            return Feed {
                content: Some(&view.content),
                refreshing,
            };
        }
        let entry = self.0.get(source);
        Feed {
            content: entry.map(|entry| &entry.content),
            refreshing: entry.is_none_or(|entry| entry.job.is_some()),
        }
    }
}

impl MainView {
    pub(in crate::ui) fn apply_connection_checks(
        &mut self,
        checks: Vec<ConnectionCheck>,
        cx: &mut Context<Self>,
    ) {
        for check in checks {
            if check
                .problem
                .as_ref()
                .is_some_and(|error| error.kind == ErrorKind::Revoked)
            {
                self.forget_account_playlists(
                    check.account.provider,
                    Some(&check.account.profile.id),
                    cx,
                );
            }
            let Some((home, busy)) = self.state.home_view() else {
                return;
            };
            if busy
                || !home.accounts.iter().any(|account| {
                    account.provider == check.account.provider
                        && account.profile.id == check.account.profile.id
                })
            {
                continue;
            }
            let active = FeedSource::active(home);
            let source = FeedSource {
                provider: check.account.provider,
                profile: Some(check.account.profile.id.clone()),
            };
            if let Some(error) = check.problem {
                if source == active {
                    if self.state.note_connection_problem(
                        check.account.provider,
                        &check.account.profile.id,
                        error,
                    ) {
                        cx.notify();
                    }
                } else if let Some(entry) = self.home.feeds.0.get_mut(&source)
                    && entry.job.is_none()
                    && entry.content.connection_problem(error)
                {
                    cx.notify();
                }
            } else if self
                .home
                .feeds
                .get(&source, &active, home, busy)
                .content
                .is_some_and(can_retry_feed)
            {
                let update_active = source == active;
                self.request_home_source(source, HomeSourceRequest::default(), update_active, cx);
            }
        }
    }
    pub(in crate::ui) fn invalidate_home_source(&mut self, provider: ProviderId) {
        self.home
            .feeds
            .0
            .retain(|source, _| source.provider != provider);
    }

    pub(in crate::ui) fn sync_home_sources(&mut self, cx: &mut Context<Self>) {
        if self.waiting_for_workspace_frame() {
            return;
        }
        let Some((view, _)) = self.state.home_view() else {
            return;
        };
        let active = FeedSource::active(view);
        let mut sources = FeedSource::available(view);
        sources.retain(|source| source != &active);
        self.home
            .feeds
            .0
            .retain(|source, _| sources.contains(source));
        if self.home.filter.is_some_and(|provider| {
            active.provider != provider && !sources.iter().any(|source| source.provider == provider)
        }) {
            self.home.filter = None;
        }
        if self.music_page() == super::Page::Home {
            if self.history.needs_load() {
                self.load_history(cx);
            }
            for source in sources {
                if !self.home.feeds.0.contains_key(&source) {
                    self.request_home_source(source, HomeSourceRequest::for_startup(), false, cx);
                }
            }
        }
    }

    pub(super) fn refresh_home_source(&mut self, source: FeedSource, cx: &mut Context<Self>) {
        let Some((home, busy)) = self.state.home_view() else {
            return;
        };
        if source == FeedSource::active(home) {
            if !busy {
                self.run(crate::state::Intent::Refresh, cx);
            }
            return;
        }
        self.request_home_source(source, HomeSourceRequest::default(), false, cx);
    }

    fn request_home_source(
        &mut self,
        source: FeedSource,
        request: HomeSourceRequest,
        update_active: bool,
        cx: &mut Context<Self>,
    ) {
        let entry = self.home.feeds.0.entry(source.clone()).or_insert(Entry {
            content: HomeContent::Loading,
            job: None,
        });
        if entry.job.is_some() {
            return;
        }
        let request = Arc::new(request);
        let work = request.clone();
        let query = source.clone();
        let task = cx.spawn(async move |view, cx| {
            let (sender, receiver) = async_channel::bounded(1);
            let cached_request = work.clone();
            let result = cx.background_executor().spawn(async move {
                let store = Store::open_default().map_err(Issue::Storage)?;
                if !update_active && let Ok(Some(snapshot)) =
                    cached_request.cached(&store, query.provider, query.profile.as_deref())
                {
                    let _ = sender.try_send(snapshot);
                }
                drop(sender);
                cached_request.refresh(&store, query.provider, query.profile.as_deref())
            });
            if let Ok(snapshot) = receiver.recv().await {
                let _ = view.update(cx, |view, cx| {
                    if let Some(entry) = view.home.feeds.entry_for(&source, &work)
                        && matches!(entry.content, HomeContent::Loading)
                    {
                        entry.content = HomeContent::Fresh(snapshot);
                        cx.notify();
                    }
                });
            }
            let result = result.await;
            let _ = view.update(cx, |view, cx| {
                if update_active {
                    if view.home.feeds.entry_for(&source, &work).is_some() {
                        view.home.feeds.0.remove(&source);
                        if let (Ok(content), Some(profile)) = (result, source.profile.as_deref())
                            && view.state.replace_connection_feed(source.provider, profile, content)
                        {
                            cx.notify();
                        }
                    }
                    return;
                }
                if let Some(entry) = view.home.feeds.entry_for(&source, &work) {
                    let next = match result {
                        Ok(content) => content,
                        Err(issue) => HomeContent::Empty(issue),
                    };
                    entry.content = match (
                        next,
                        std::mem::replace(&mut entry.content, HomeContent::Loading),
                    ) {
                        (HomeContent::Empty(issue), before)
                            if !matches!(&issue, Issue::Provider(error) if error.kind == ErrorKind::Revoked) => match before {
                            HomeContent::Fresh(snapshot)
                            | HomeContent::Cached { snapshot, .. }
                            | HomeContent::Partial { snapshot, .. } => {
                                HomeContent::Cached { snapshot, issue }
                            }
                            _ => HomeContent::Empty(issue),
                        },
                        (content, _) => content,
                    };
                    entry.job = None;
                    cx.notify();
                }
            });
        });
        entry.job = Some(Job {
            request,
            _task: task,
        });
        cx.notify();
    }
}

impl Feeds {
    fn entry_for(
        &mut self,
        source: &FeedSource,
        request: &Arc<HomeSourceRequest>,
    ) -> Option<&mut Entry> {
        self.0.get_mut(source).filter(|entry| {
            entry
                .job
                .as_ref()
                .is_some_and(|job| Arc::ptr_eq(&job.request, request))
        })
    }
}
