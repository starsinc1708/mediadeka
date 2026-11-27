mod feedback;
#[cfg(all(test, windows, feature = "capture"))]
mod native;
mod page;

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{Context, Task, Window};

use feedback::Event;

use crate::{
    music::{
        ObjectKind, ObjectRef,
        station::{self, Feed, MY_WAVE},
    },
    playback::{QueueEntry, StationTrackEnded, Status, can_resolve},
    playlist::PlaylistKey,
    providers::{ProviderId, yandex::station::MAX_RECENT_TRACKS},
};

use super::{
    MainView,
    connections::SourceFailure,
    workspace::{MusicTarget, Route},
};

#[derive(Default)]
pub(super) struct StationUi {
    revision: u64,
    starting: Option<Starting>,
    refill: Option<Job>,
    active: Option<Active>,
    failure: Option<(PlaylistKey, SourceFailure)>,
}

struct Starting {
    key: PlaylistKey,
    previous_key: Option<PlaylistKey>,
    queue_replacement: u64,
    _job: Job,
}

pub(super) struct Job {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

#[cfg(test)]
impl Job {
    pub(super) fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

struct Active {
    key: PlaylistKey,
    revision: u64,
    supplied: usize,
    radio: Option<Radio>,
}

struct Radio {
    session: String,
    recent: VecDeque<String>,
    batches: BTreeMap<usize, String>,
    feedback: feedback::Worker,
}

impl Active {
    fn owns(&self, entry: &QueueEntry) -> bool {
        entry.playlist == self.key && entry.playlist_revision == Some(self.revision)
    }

    fn entries(&mut self, feed: Feed) -> Vec<QueueEntry> {
        let batch = feed.rotor.map(|rotor| rotor.batch_id);
        feed.tracks
            .into_iter()
            .filter_map(|track| {
                if !can_resolve(&self.key, &track) {
                    return None;
                }
                let index = self.supplied;
                self.supplied += 1;
                if let Some(radio) = self.radio.as_mut()
                    && let Some(batch) = &batch
                {
                    radio.recent.push_back(track.id.clone());
                    radio.batches.insert(index, batch.clone());
                    if radio.recent.len() > MAX_RECENT_TRACKS {
                        radio.recent.pop_front();
                        radio.batches.pop_first();
                    }
                }
                Some(QueueEntry {
                    playlist: self.key.clone(),
                    playlist_revision: Some(self.revision),
                    recommendations: Vec::new(),
                    index,
                    track,
                })
            })
            .collect()
    }
}

impl MainView {
    pub(in crate::ui) fn open_and_start_station(
        &mut self,
        target: MusicTarget,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.routes.current() != &Route::Station(target.clone()) {
            self.open_music_target(target.clone(), window, cx);
        }
        let key = self.station_key(&target);
        let already_active = self
            .station
            .active
            .as_ref()
            .is_some_and(|active| active.key == key)
            && matches!(
                self.player_snapshot.status,
                Status::Loading | Status::Playing | Status::Paused
            );
        if !already_active
            && matches!(self.workspace.routes.current(), Route::Station(current) if current == &target)
        {
            self.start_station(target, cx);
        }
    }

    pub(in crate::ui) fn open_yandex_wave(
        &mut self,
        profile: Option<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.open_and_start_station(my_wave_target(profile), window, cx);
    }

    pub(super) fn station_route_changed(&mut self, route: &Route) {
        if let Some(starting) = &self.station.starting
            && !matches!(route, Route::Station(target) if self.station_key(target) == starting.key)
        {
            self.station.starting = None;
        }
    }

    fn start_station(&mut self, target: MusicTarget, cx: &mut Context<Self>) {
        let key = self.station_key(&target);
        if self
            .station
            .starting
            .as_ref()
            .is_some_and(|starting| starting.key == key)
        {
            return;
        }
        self.station.failure = None;
        let work_key = key.clone();
        let result_key = key.clone();
        let queue_replacement = self.player.read(cx).queue_replacement_revision();
        let job = run(
            cx,
            move |cancelled| station::start(&work_key, cancelled),
            move |view, result, cx| {
                view.station.starting = None;
                if view.player.read(cx).queue_replacement_revision() != queue_replacement {
                    return;
                }
                match result {
                    Ok(feed) => view.play_station_feed(result_key, feed, cx),
                    Err(error) => {
                        view.station.failure = Some((result_key, station_failure(&error)));
                    }
                }
            },
        );
        self.station.starting = Some(Starting {
            key,
            previous_key: self.player_snapshot.key.clone(),
            queue_replacement,
            _job: job,
        });
        cx.notify();
    }

    fn play_station_feed(&mut self, key: PlaylistKey, feed: Feed, cx: &mut Context<Self>) {
        if !feed.tracks.iter().any(|track| can_resolve(&key, track)) {
            self.station.failure = Some((
                key,
                SourceFailure {
                    message: "Станция не вернула треки с доступным звуком.".into(),
                    needs_sign_in: false,
                },
            ));
            return;
        }
        self.station.refill = None;
        self.station.failure = None;
        self.station.revision = self.station.revision.wrapping_add(1);
        let radio = feed.rotor.as_ref().map(|rotor| Radio {
            session: rotor.session_id.clone(),
            recent: VecDeque::new(),
            batches: BTreeMap::new(),
            feedback: feedback::Worker::new(&key, &rotor.session_id, cx),
        });
        let mut active = Active {
            key: key.clone(),
            revision: self.station.revision,
            supplied: 0,
            radio,
        };
        let entries = active.entries(feed);
        self.station.active = Some(active);
        self.ensure_playlist_favorites(&key, cx);
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.play(entries, 0));
        });
    }

    pub(super) fn station_player_changed(&mut self, cx: &Context<Self>) {
        let player = self.player.read(cx);
        if let Some(starting) = &self.station.starting
            && (starting.previous_key != self.player_snapshot.key
                || starting.queue_replacement != player.queue_replacement_revision())
        {
            self.station.starting = None;
        }
        let active = self.station.active.as_ref();
        if active.is_some_and(|active| player.current().is_none_or(|entry| !active.owns(entry))) {
            self.station.active = None;
            self.station.refill = None;
        }
    }

    pub(super) fn provider_station_running(&self) -> bool {
        self.station.starting.is_some() || self.station.active.is_some()
    }

    pub(super) fn station_track_started(&mut self, entry: &QueueEntry, cx: &mut Context<Self>) {
        self.send_station_feedback(entry, Event::Started);
        if self.station.refill.is_some() {
            return;
        }
        let Some(active) = self
            .station
            .active
            .as_ref()
            .filter(|active| active.owns(entry))
        else {
            return;
        };
        if active.supplied > entry.index.saturating_add(1) {
            return;
        }
        let Some(radio) = active.radio.as_ref() else {
            return;
        };
        let session = radio.session.clone();
        let recent = radio.recent.iter().cloned().collect::<Vec<_>>();
        let work_key = active.key.clone();
        let revision = active.revision;
        let job = run(
            cx,
            move |cancelled| station::refill(&work_key, &session, &recent, cancelled),
            move |view, result, cx| {
                view.station.refill = None;
                let Some(active) = view
                    .station
                    .active
                    .as_mut()
                    .filter(|active| active.revision == revision)
                else {
                    return;
                };
                if !view
                    .player
                    .read(cx)
                    .current()
                    .is_some_and(|entry| active.owns(entry))
                {
                    return;
                }
                match result {
                    Ok(feed) => {
                        let entries = active.entries(feed);
                        if !entries.is_empty() {
                            view.player.update(cx, |bar, cx| {
                                bar.command(cx, |playback| playback.append_continuation(entries));
                            });
                        }
                    }
                    Err(error) => {
                        let failure = station_failure(&error);
                        let message = failure.message.clone();
                        view.station.failure = Some((active.key.clone(), failure));
                        view.player
                            .update(cx, |bar, cx| bar.show_notice(message, cx));
                    }
                }
            },
        );
        self.station.refill = Some(job);
    }

    pub(super) fn station_track_ended(&self, event: &StationTrackEnded) {
        self.send_station_feedback(
            &event.entry,
            Event::Ended {
                seconds: event.seconds,
                skipped: event.skipped,
            },
        );
    }

    pub(super) fn station_like(&self, key: &PlaylistKey, id: &str, cx: &Context<Self>) {
        if let Some(entry) = self
            .player
            .read(cx)
            .current()
            .filter(|entry| &entry.playlist == key && entry.track.id == id)
        {
            self.send_station_feedback(entry, Event::Like);
        }
    }

    fn dislike_station_track(&self, cx: &Context<Self>) {
        let bar = self.player.read(cx);
        if let Some(entry) = bar.current() {
            self.send_station_feedback(
                entry,
                Event::Dislike {
                    seconds: bar.position(),
                },
            );
        }
    }

    fn send_station_feedback(&self, entry: &QueueEntry, event: Event) {
        if let Some(active) = self
            .station
            .active
            .as_ref()
            .filter(|active| active.owns(entry))
            && let Some(radio) = &active.radio
            && let Some(batch) = radio.batches.get(&entry.index)
        {
            radio
                .feedback
                .send(batch.clone(), entry.track.id.clone(), event);
        }
    }
}

pub(in crate::ui) fn yandex_radio_target(
    object: &ObjectRef,
    profile: Option<&str>,
    title: &str,
    artwork: Option<&str>,
) -> Option<MusicTarget> {
    if object.provider != ProviderId::YandexMusic
        || !matches!(object.kind, ObjectKind::Track | ObjectKind::Artist)
    {
        return None;
    }
    let profile = profile?.to_owned();
    let id = format!("{}:{}", object.kind.path(), object.id);
    if !crate::providers::yandex::station::valid_seed(&id) {
        return None;
    }
    Some(MusicTarget {
        object: ObjectRef {
            provider: object.provider,
            kind: ObjectKind::Station,
            id,
        },
        profile: Some(profile),
        title: format!("Радио · {title}"),
        artwork: artwork.map(str::to_owned),
    })
}

impl MainView {
    fn station_key(&self, target: &MusicTarget) -> PlaylistKey {
        let provider = target.object.provider;
        PlaylistKey::Station {
            provider,
            profile: target
                .profile
                .as_deref()
                .or_else(|| self.connected_provider_profile(provider))
                .map(str::to_owned),
            id: target.object.id.clone(),
        }
    }
}

fn station_failure(error: &station::StationError) -> SourceFailure {
    SourceFailure {
        message: error.to_string(),
        needs_sign_in: error.needs_sign_in(),
    }
}

fn my_wave_target(profile: Option<String>) -> MusicTarget {
    MusicTarget {
        object: ObjectRef {
            provider: ProviderId::YandexMusic,
            kind: ObjectKind::Station,
            id: MY_WAVE.into(),
        },
        profile,
        title: "Моя волна".into(),
        artwork: None,
    }
}

pub(super) fn run<T: Send + 'static, E: Send + 'static>(
    cx: &mut Context<MainView>,
    work: impl FnOnce(&AtomicBool) -> Result<T, E> + Send + 'static,
    finish: impl FnOnce(&mut MainView, Result<T, E>, &mut Context<MainView>) + 'static,
) -> Job {
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let worker_flag = flag.clone();
    let task = cx.spawn(async move |view, cx| {
        let result = cx
            .background_executor()
            .spawn(async move { work(&worker_flag) })
            .await;
        if !flag.load(Ordering::Relaxed) {
            let _ = view.update(cx, |view, cx| {
                finish(view, result, cx);
                cx.notify();
            });
        }
    });
    Job {
        cancelled,
        _task: task,
    }
}
