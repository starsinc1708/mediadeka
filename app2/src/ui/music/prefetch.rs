mod backoff;
mod efficiency;
mod viewport;

use std::{
    cmp::Reverse,
    collections::{HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{Context, Task};

use crate::{
    music::{
        ObjectKind, ObjectRef, PageReceipt, ScopedPage,
        library::{Entry, EntryAction, PlaylistReceipt},
        prefetch_page,
    },
    playlist::{
        PlaylistKey,
        library::{ReadError, prefetch_playlist},
    },
    providers::{ErrorKind, ProviderId},
    telemetry,
    ui::{
        MainView,
        components::artwork::{self, ArtworkPrefetch},
        playlist::navigation::{self, Destination},
        workspace::{MusicTarget, Route},
    },
};

use efficiency::Efficiency;

const INTENT_DELAY: Duration = Duration::from_millis(200);
const UPCOMING_TRACKS: usize = 2;
const RELATED_ARTISTS: usize = 2;
const RECENT_LIMIT: usize = 32;
const RETRY_DELAY: Duration = Duration::from_secs(20);
const RECHECK_DELAY: Duration = Duration::from_secs(5 * 60);
const READ_AHEAD_DELAY: Duration = Duration::from_millis(120);
const METADATA_READ_AHEAD_DELAY: Duration = Duration::from_millis(200);
const NEARBY_LIST_TARGETS: usize = 2;

pub(super) fn entry_target(entry: &Entry) -> Option<Destination> {
    let target = match &entry.action {
        EntryAction::Track(key, track) => navigation::track_target(key, track)?,
        EntryAction::Object {
            object, profile, ..
        } => MusicTarget {
            object: object.clone(),
            profile: Some(profile.clone()),
            title: entry.title.clone(),
            artwork: entry.artwork.clone(),
        },
        EntryAction::LocalPlaylist { .. } | EntryAction::LocalGroup(_) => return None,
    };
    Some(Destination::Object(target))
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    object: ObjectRef,
    profile: Option<String>,
}

enum WarmResult {
    Ready {
        receipt: Receipt,
        cover: Option<String>,
    },
    FailedNetwork,
    Unavailable,
}

#[derive(Clone, PartialEq, Eq)]
enum Receipt {
    Page(PageReceipt),
    Playlist(Arc<PlaylistReceipt>),
}

impl Key {
    fn from_target(target: &MusicTarget) -> Option<Self> {
        matches!(
            target.object.kind,
            ObjectKind::Track | ObjectKind::Artist | ObjectKind::Album | ObjectKind::Playlist
        )
        .then(|| Self {
            object: target.object.clone(),
            profile: target.profile.clone(),
        })
    }

    fn from_destination(target: Destination) -> Option<Self> {
        match target {
            Destination::Object(target) => Self::from_target(&target),
            Destination::Playlist(target) => {
                Self::from_playlist(&target.key).filter(|key| key.object.is_valid())
            }
            _ => None,
        }
    }

    fn from_playlist(key: &PlaylistKey) -> Option<Self> {
        let PlaylistKey::Source {
            provider,
            profile,
            id,
        } = key
        else {
            return None;
        };
        Some(Self {
            object: ObjectRef {
                provider: *provider,
                kind: ObjectKind::Playlist,
                id: id.clone(),
            },
            profile: profile.clone(),
        })
    }

    fn warm(&self, cancelled: &AtomicBool) -> WarmResult {
        if self.object.kind == ObjectKind::Playlist {
            let result = prefetch_playlist(
                &PlaylistKey::Source {
                    provider: self.object.provider,
                    profile: self.profile.clone(),
                    id: self.object.id.clone(),
                },
                cancelled,
            );
            match result {
                Ok(receipt) => WarmResult::Ready {
                    receipt: Receipt::Playlist(Arc::new(receipt)),
                    cover: None,
                },
                Err(ReadError::Provider(error)) if error.kind == ErrorKind::Network => {
                    WarmResult::FailedNetwork
                }
                Err(_) => WarmResult::Unavailable,
            }
        } else {
            match prefetch_page(&self.object, self.profile.as_deref(), cancelled) {
                Ok(Some(ScopedPage {
                    page,
                    receipt: Some(receipt),
                    ..
                })) => WarmResult::Ready {
                    receipt: Receipt::Page(receipt),
                    cover: page.artwork.clone(),
                },
                Err(error) if error.kind == ErrorKind::Network => WarmResult::FailedNetwork,
                _ => WarmResult::Unavailable,
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    Maintenance,
    Speculative,
    Nearby,
    Imminent,
    Visible,
    Critical,
}

impl Priority {
    fn label(self) -> &'static str {
        match self {
            Self::Maintenance => "Фоновая",
            Self::Speculative => "Прогноз",
            Self::Nearby => "Ближайшая",
            Self::Imminent => "Следующее действие",
            Self::Visible => "Видимое",
            Self::Critical => "Текущее",
        }
    }
}

#[derive(Default)]
pub(in crate::ui) struct PrefetchReport {
    pub(in crate::ui) waiting: usize,
    pub(in crate::ui) active: Option<&'static str>,
    pub(in crate::ui) completed: u64,
    pub(in crate::ui) cancelled: u64,
    pub(in crate::ui) network_completed: u64,
    pub(in crate::ui) useful: u64,
    pub(in crate::ui) unused_response_bytes: u64,
}

struct Candidate {
    key: Key,
    priority: Priority,
}

struct Active {
    key: Key,
    priority: Priority,
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct CatalogPrefetch {
    intent: Option<Key>,
    dwell: Option<(Key, Task<()>)>,
    read_ahead: Option<Task<()>>,
    viewport: viewport::Viewport,
    route: Option<Key>,
    page: Vec<Candidate>,
    queue: Vec<Candidate>,
    maintenance: Vec<Candidate>,
    active: Option<Active>,
    retiring: Option<Active>,
    wake: Option<(Instant, Task<()>)>,
    recent: VecDeque<(Key, Instant)>,
    last_provider: Option<ProviderId>,
    backoff: backoff::Backoff,
    completed: u64,
    cancelled: u64,
    efficiency: Efficiency<Receipt>,
}

impl CatalogPrefetch {
    fn candidates(&self) -> impl Iterator<Item = (&Key, Priority)> {
        self.queue
            .iter()
            .chain(&self.page)
            .chain(&self.maintenance)
            .map(|item| (&item.key, item.priority))
            .chain(
                self.viewport
                    .visible()
                    .iter()
                    .map(|key| (key, Priority::Visible)),
            )
            .chain(self.intent.iter().map(|key| (key, Priority::Imminent)))
    }

    fn intent(&mut self, key: Key, entered: bool, cx: &mut Context<MainView>) {
        if !entered {
            if self
                .dwell
                .as_ref()
                .is_some_and(|(waiting, _)| *waiting == key)
            {
                self.dwell = None;
            }
            if self.intent.as_ref() == Some(&key) {
                self.intent = None;
            }
            // Keep a request that already started. A click can join its flight.
            return;
        }
        if self.route.as_ref() == Some(&key)
            || self.intent.as_ref() == Some(&key)
            || self
                .dwell
                .as_ref()
                .is_some_and(|(waiting, _)| *waiting == key)
        {
            return;
        }
        let waiting = key.clone();
        let task = cx.spawn(async move |view, cx| {
            cx.background_executor().timer(INTENT_DELAY).await;
            let _ = view.update(cx, |view, cx| {
                let scheduler = &mut view.music.prefetch;
                scheduler.dwell = None;
                scheduler.intent = Some(waiting);
                scheduler.drive(cx);
            });
        });
        self.dwell = Some((key, task));
    }

    pub(super) fn route_changed(&mut self, route: &Route, cx: &mut Context<MainView>) {
        self.route = match route {
            Route::Object(target) => Key::from_target(target),
            Route::Playlist(target) => Key::from_playlist(&target.key),
            _ => None,
        };
        self.dwell = None;
        self.read_ahead = None;
        self.viewport = Default::default();
        self.intent = None;
        self.page.clear();
        self.cancel_obsolete();
        self.drive(cx);
    }

    fn cancel_obsolete(&mut self) {
        if let Some(active) = &self.active
            && self.route.as_ref() != Some(&active.key)
            && !self.candidates().any(|(key, _)| *key == active.key)
        {
            self.cancel_active();
        }
    }

    fn cancel_active(&mut self) {
        if let Some(active) = &self.active
            && !active.cancelled.swap(true, Ordering::Relaxed)
        {
            self.cancelled += 1;
        }
    }

    fn next(&self, now: Instant) -> (Option<Candidate>, Option<Instant>) {
        let mut best = None;
        let mut wake: Option<Instant> = None;
        for (index, (key, priority)) in self.candidates().enumerate() {
            if self.route.as_ref() == Some(key)
                || self.active.as_ref().is_some_and(|job| &job.key == key)
                || self.retiring.as_ref().is_some_and(|job| &job.key == key)
            {
                continue;
            }
            let recent = self
                .recent
                .iter()
                .find_map(|(seen, until)| (seen == key && *until > now).then_some(*until));
            let provider = self.backoff.paused_until(key.object.provider, now);
            if let Some(ready_at) = recent.max(provider) {
                wake = Some(wake.map_or(ready_at, |old| old.min(ready_at)));
                continue;
            }
            let score = (
                priority,
                self.last_provider != Some(key.object.provider),
                Reverse(index),
            );
            if best.is_none_or(|(_, old)| score > old) {
                best = Some((key, score));
            }
        }
        (
            best.map(|(key, (priority, _, _))| Candidate {
                key: key.clone(),
                priority,
            }),
            wake,
        )
    }

    fn wait_until(&mut self, deadline: Option<Instant>, cx: &mut Context<MainView>) {
        if self.wake.as_ref().map(|(at, _)| *at) == deadline {
            return;
        }
        self.wake = deadline.map(|at| {
            let task = cx.spawn(async move |view, cx| {
                cx.background_executor()
                    .timer(at.saturating_duration_since(Instant::now()))
                    .await;
                let _ = view.update(cx, |view, cx| {
                    let scheduler = &mut view.music.prefetch;
                    scheduler.wake = None;
                    scheduler.drive(cx);
                });
            });
            (at, task)
        });
    }

    fn active_priority(&self, active: &Active) -> Priority {
        if self.route.as_ref() == Some(&active.key) {
            Priority::Critical
        } else {
            self.candidates()
                .filter_map(|(key, priority)| (key == &active.key).then_some(priority))
                .max()
                .unwrap_or(active.priority)
        }
    }

    fn may_start(&mut self, priority: Priority) -> bool {
        let Some(active) = &self.active else {
            return true;
        };
        if self.retiring.is_some()
            || (!active.cancelled.load(Ordering::Relaxed)
                && priority <= self.active_priority(active))
        {
            return false;
        }
        // A synchronous HTTP call may still be unwinding. Keep a single
        // cancelled flight while giving higher-priority work the active slot.
        self.cancel_active();
        self.retiring = self.active.take();
        true
    }

    fn drive(&mut self, cx: &mut Context<MainView>) {
        let now = Instant::now();
        self.recent.retain(|(_, until)| *until > now);
        let (candidate, wake) = self.next(now);
        self.wait_until(wake, cx);
        let Some(Candidate { key, priority }) = candidate else {
            return;
        };
        if !self.may_start(priority) {
            return;
        }
        self.last_provider = Some(key.object.provider);
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&cancelled);
        let active_key = key.clone();
        let worker_key = key.clone();
        let epoch = self.efficiency.epoch();
        let task = cx.spawn(async move |view, cx| {
            let worker_flag = Arc::clone(&stopped);
            let (result, bytes) = cx
                .background_executor()
                .spawn(
                    async move { telemetry::prefetch_body_bytes(|| worker_key.warm(&worker_flag)) },
                )
                .await;
            let _ = view.update(cx, |view, cx| {
                let scheduler = &mut view.music.prefetch;
                let receipt = match &result {
                    WarmResult::Ready { receipt, .. } => Some(receipt.clone()),
                    WarmResult::FailedNetwork | WarmResult::Unavailable => None,
                };
                scheduler.efficiency.finished(epoch, bytes, receipt);
                if scheduler
                    .retiring
                    .as_ref()
                    .is_some_and(|job| Arc::ptr_eq(&job.cancelled, &stopped))
                {
                    scheduler.retiring = None;
                    scheduler.drive(cx);
                    return;
                }
                let cancelled = stopped.load(Ordering::Relaxed);
                scheduler.active = None;
                if !cancelled {
                    let loaded = matches!(&result, WarmResult::Ready { .. });
                    let network_failed = matches!(&result, WarmResult::FailedNetwork);
                    scheduler
                        .backoff
                        .finish(active_key.object.provider, loaded, network_failed);
                    scheduler.completed += u64::from(loaded);
                    if let WarmResult::Ready {
                        cover: Some(cover), ..
                    } = result
                    {
                        artwork::prefetch(&cover, ArtworkPrefetch::Hero, 1., cx);
                    }
                    let delay = if network_failed {
                        RETRY_DELAY
                    } else {
                        RECHECK_DELAY
                    };
                    scheduler
                        .recent
                        .push_back((active_key, Instant::now() + delay));
                    if scheduler.recent.len() > RECENT_LIMIT {
                        scheduler.recent.pop_front();
                    }
                }
                scheduler.drive(cx);
            });
        });
        self.active = Some(Active {
            key,
            priority,
            cancelled,
            _task: task,
        });
    }

    fn observe(&mut self, receipt: Receipt) {
        self.efficiency
            .observed(receipt, self.active.is_some() || self.retiring.is_some());
    }
}

impl MainView {
    pub(in crate::ui) fn prefetch_pins(
        &mut self,
        targets: impl IntoIterator<Item = MusicTarget>,
        cx: &mut Context<Self>,
    ) {
        const PINNED_LIMIT: usize = 2;
        let scheduler = &mut self.music.prefetch;
        scheduler.maintenance = targets
            .into_iter()
            .filter_map(|target| Key::from_target(&target))
            .take(PINNED_LIMIT)
            .map(|key| Candidate {
                key,
                priority: Priority::Maintenance,
            })
            .collect();
        scheduler.cancel_obsolete();
        scheduler.drive(cx);
    }

    pub(in crate::ui) fn prefetch_report(&self) -> PrefetchReport {
        let scheduler = &self.music.prefetch;
        let mut waiting = Vec::new();
        let now = Instant::now();
        for (key, _) in scheduler.candidates() {
            if scheduler.route.as_ref() != Some(key)
                && scheduler.active.as_ref().is_none_or(|job| &job.key != key)
                && !scheduler
                    .recent
                    .iter()
                    .any(|(seen, until)| seen == key && *until > now)
                && !waiting.contains(&key)
            {
                waiting.push(key);
            }
        }
        PrefetchReport {
            waiting: waiting.len(),
            active: scheduler
                .active
                .as_ref()
                .map(|job| scheduler.active_priority(job).label()),
            completed: scheduler.completed,
            cancelled: scheduler.cancelled,
            network_completed: scheduler.efficiency.completed,
            useful: scheduler.efficiency.useful,
            unused_response_bytes: scheduler.efficiency.unused_bytes,
        }
    }

    pub(super) fn observe_prefetched_page(&mut self, data: &ScopedPage) {
        if let Some(receipt) = &data.receipt {
            self.music.prefetch.observe(Receipt::Page(receipt.clone()));
        }
    }

    pub(in crate::ui) fn observe_prefetched_playlist(&mut self, receipt: PlaylistReceipt) {
        self.music
            .prefetch
            .observe(Receipt::Playlist(Arc::new(receipt)));
    }

    pub(in crate::ui) fn reset_list_prefetch(&mut self, cx: &mut Context<Self>) {
        let scheduler = &mut self.music.prefetch;
        scheduler.read_ahead = None;
        scheduler.viewport = Default::default();
        scheduler.page.clear();
        scheduler.cancel_obsolete();
        scheduler.drive(cx);
    }

    pub(in crate::ui) fn prefetch_list_window(
        &mut self,
        covers: Vec<String>,
        targets: impl IntoIterator<Item = Destination>,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        let mut keys = Vec::new();
        for key in targets.into_iter().filter_map(Key::from_destination) {
            if keys.contains(&key) {
                continue;
            }
            keys.push(key);
            if keys.len() == NEARBY_LIST_TARGETS {
                break;
            }
        }
        let scheduler = &mut self.music.prefetch;
        scheduler.page.retain(|pending| keys.contains(&pending.key));
        scheduler.cancel_obsolete();
        scheduler.drive(cx);
        scheduler.read_ahead = (!covers.is_empty() || !keys.is_empty()).then(|| {
            let delay = if keys.is_empty() {
                READ_AHEAD_DELAY
            } else {
                METADATA_READ_AHEAD_DELAY
            };
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(delay).await;
                let _ = view.update(cx, |view, cx| {
                    let mut seen = HashSet::new();
                    for cover in &covers {
                        if seen.insert(cover) {
                            artwork::prefetch(cover, ArtworkPrefetch::Row, scale, cx);
                        }
                    }
                    let scheduler = &mut view.music.prefetch;
                    scheduler.read_ahead = None;
                    scheduler.page = keys
                        .into_iter()
                        .map(|key| Candidate {
                            key,
                            priority: Priority::Nearby,
                        })
                        .collect();
                    scheduler.drive(cx);
                });
            })
        });
    }

    pub(in crate::ui) fn hint_music_target(
        &mut self,
        target: Destination,
        entered: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = Key::from_destination(target) {
            self.music.prefetch.intent(key, entered, cx);
        }
    }

    pub(in crate::ui) fn playback_prefetch_changed(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        let mut queue = Vec::new();
        for (position, entry) in player
            .current_entry()
            .into_iter()
            .chain(player.upcoming().take(UPCOMING_TRACKS))
            .enumerate()
        {
            let key = &entry.playlist;
            let track = &entry.track;
            let current = position == 0;
            let recording = navigation::track_target(key, track)
                .map(Destination::Object)
                .into_iter()
                .map(|target| {
                    (
                        target,
                        if current {
                            Priority::Critical
                        } else {
                            Priority::Nearby
                        },
                    )
                });
            let related = navigation::album(key, track)
                .into_iter()
                .chain(navigation::artists(key, track).take(RELATED_ARTISTS))
                .map(|(_, target)| {
                    (
                        target,
                        if current {
                            Priority::Visible
                        } else {
                            Priority::Speculative
                        },
                    )
                });
            queue.extend(recording.chain(related).filter_map(|(target, priority)| {
                Key::from_destination(target).map(|key| Candidate { key, priority })
            }));
        }
        self.music.prefetch.queue = queue;
        self.music.prefetch.cancel_obsolete();
        self.music.prefetch.drive(cx);
    }

    pub(super) fn prefetch_music_relations(&mut self, data: &ScopedPage, cx: &mut Context<Self>) {
        let page = &data.page;
        let scope = super::detail::playlist_key(data);
        let related = navigation::credits(&page.artists, "", &page.artist_credits)
            .take(RELATED_ARTISTS)
            .filter_map(|(name, reference)| {
                navigation::related(&scope, ObjectKind::Artist, name, reference)
            })
            .filter_map(Key::from_destination)
            .map(|key| Candidate {
                key,
                priority: Priority::Nearby,
            });
        self.music.prefetch.page = related.collect();
        self.music.prefetch.cancel_obsolete();
        self.music.prefetch.drive(cx);
    }

    pub(in crate::ui) fn clear_music_prefetch(&mut self) {
        let scheduler = &mut self.music.prefetch;
        scheduler.dwell = None;
        scheduler.read_ahead = None;
        scheduler.viewport = Default::default();
        scheduler.wake = None;
        scheduler.intent = None;
        scheduler.route = None;
        scheduler.page.clear();
        scheduler.queue.clear();
        scheduler.maintenance.clear();
        scheduler.recent.clear();
        scheduler.backoff = Default::default();
        scheduler.efficiency.invalidate();
        scheduler.cancel_obsolete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> Key {
        Key {
            object: ObjectRef {
                provider: ProviderId::Jellyfin,
                kind: ObjectKind::Track,
                id: id.into(),
            },
            profile: Some("listener".into()),
        }
    }

    fn active(key: Key, priority: Priority) -> Active {
        Active {
            key,
            priority,
            cancelled: Arc::new(AtomicBool::new(false)),
            _task: Task::ready(()),
        }
    }

    #[test]
    fn strong_intent_can_start_while_one_cancelled_request_finishes() {
        let low = key("pin");
        let mut scheduler = CatalogPrefetch {
            active: Some(active(low.clone(), Priority::Maintenance)),
            maintenance: vec![Candidate {
                key: low,
                priority: Priority::Maintenance,
            }],
            ..Default::default()
        };
        assert!(scheduler.may_start(Priority::Imminent));
        assert_eq!(scheduler.cancelled, 1);
        assert!(
            scheduler
                .retiring
                .as_ref()
                .unwrap()
                .cancelled
                .load(Ordering::Relaxed)
        );
        scheduler.active = Some(active(key("hover"), Priority::Imminent));
        assert!(!scheduler.may_start(Priority::Critical));
        scheduler.retiring = None;
        assert!(scheduler.may_start(Priority::Critical));
        assert_eq!(scheduler.cancelled, 2);
    }

    #[test]
    fn existing_flight_is_promoted_when_its_track_becomes_current() {
        let chosen = key("current");
        let mut scheduler = CatalogPrefetch {
            active: Some(active(chosen.clone(), Priority::Maintenance)),
            queue: vec![Candidate {
                key: chosen.clone(),
                priority: Priority::Critical,
            }],
            ..Default::default()
        };
        assert!(scheduler.next(Instant::now()).0.is_none());
        assert!(!scheduler.may_start(Priority::Imminent));
        scheduler.queue.clear();
        scheduler.route = Some(chosen);
        assert!(!scheduler.may_start(Priority::Visible));
        assert_eq!(scheduler.cancelled, 0);
    }

    #[test]
    fn quiet_view_has_a_wake_deadline_and_retries_when_it_opens() {
        let key = key("failed");
        let now = Instant::now();
        let deadline = now + RETRY_DELAY;
        let mut scheduler = CatalogPrefetch {
            queue: vec![Candidate {
                key: key.clone(),
                priority: Priority::Critical,
            }],
            recent: [(key.clone(), deadline)].into(),
            ..Default::default()
        };
        scheduler.backoff.finish(key.object.provider, false, true);
        let (next, wake) = scheduler.next(now);
        assert!(next.is_none());
        assert_eq!(wake, Some(deadline));
        let (next, wake) = scheduler.next(deadline);
        assert!(next.is_some_and(|item| item.key == key));
        assert!(wake.is_none());
    }
}
