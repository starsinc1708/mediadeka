#[cfg(windows)]
mod mpv;
#[cfg(windows)]
mod soloist;
#[cfg(windows)]
pub(crate) use soloist::probe_managed_soloist;
mod worker;

mod collection;
mod quality;
mod queue;
mod queue_resume;

#[cfg(test)]
mod tests;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::playlist::{PlaylistKey, PlaylistTrack};
use crate::providers::ProviderId;
use collection::CollectionListening;
pub(crate) use collection::{
    CollectionKey, CollectionPlan, CollectionProgress, CollectionProgressUpdate,
};
pub(crate) use quality::AudioQuality;
use queue::{Placement, Queue};
pub(crate) use queue_resume::{QueueResume, QueueResumePlan, QueueResumeUpdate};
use worker::{Command, Frame, WORKER_STOPPED, Worker};

const DEFAULT_AUDIO_DEVICE: &str = "auto";
const UNAVAILABLE_TRACK: &str = "Звук этой записи недоступен.";
const PREFETCH_TAIL: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct RecommendationOrigin {
    pub provider: ProviderId,
    pub playlist_id: String,
    pub playlist: String,
}

#[derive(Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct QueueEntry {
    pub playlist: PlaylistKey,
    pub playlist_revision: Option<u64>,
    pub index: usize,
    pub track: PlaylistTrack,
    pub recommendations: Vec<RecommendationOrigin>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContinuationPoint {
    pub queue: u64,
    generation: u64,
}

impl QueueEntry {
    pub(crate) fn is_playable(&self) -> bool {
        can_resolve(&self.playlist, &self.track)
    }

    #[cfg(windows)]
    pub(crate) fn same_recording(&self, other: &Self) -> bool {
        self.playlist.connected_source() == other.playlist.connected_source()
            && self.track.id == other.track.id
            && self.track.media == other.track.media
    }
}

pub(crate) fn can_resolve(key: &PlaylistKey, track: &PlaylistTrack) -> bool {
    track.media.is_some() || crate::music::sound::recording(key, track).is_some()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SoundSource {
    LocalFile,
    Cached,
    Download(ProviderId),
    Soloist,
    Stream(ProviderId),
    Link,
}

impl SoundSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::LocalFile => "Локальный файл",
            Self::Cached => "Кэш профиля",
            Self::Download(ProviderId::Jellyfin) => "Оригинал Jellyfin",
            Self::Download(ProviderId::SoundCloud) => "Оригинал SoundCloud",
            Self::Download(provider) => provider.display_name(),
            Self::Soloist => "Spotify Soloist",
            Self::Stream(ProviderId::Spotify) => "Поток Spotify",
            Self::Stream(ProviderId::YandexMusic) => "Поток Яндекс Музыки",
            Self::Stream(ProviderId::SoundCloud) => "Поток SoundCloud",
            Self::Stream(ProviderId::YouTubeMusic) => "Поток YouTube Music",
            Self::Stream(ProviderId::Jellyfin) => "Поток Jellyfin",
            Self::Link => "Поток по ссылке",
        }
    }

    pub(crate) fn provider(self) -> Option<ProviderId> {
        match self {
            Self::Stream(provider) | Self::Download(provider) => Some(provider),
            Self::Soloist => Some(ProviderId::Spotify),
            Self::LocalFile | Self::Cached | Self::Link => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct SoundDetails {
    pub quality: Option<AudioQuality>,
    pub time_to_sound_ms: Option<u64>,
    pub reason: Option<String>,
}

/// The first frame of a generation that actually produced sound.
#[derive(Clone)]
pub(crate) struct SoundStarted {
    pub generation: u64,
    pub entry: QueueEntry,
    pub source: SoundSource,
    pub details: SoundDetails,
}

pub(crate) struct SoundCacheChanged;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ListeningSample {
    pub generation: u64,
    pub phase: ListeningPhase,
    pub position: f64,
    pub duration: Option<f64>,
    pub listened: Duration,
    pub seek_revision: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ListeningPhase {
    Playing,
    Paused,
    Buffering,
    Ended,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PollChange {
    None,
    Position,
    Transport,
}

pub(crate) struct StationTrackEnded {
    pub entry: QueueEntry,
    pub seconds: f64,
    pub skipped: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Status {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AudioDevice {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Default)]
pub(crate) struct Playback {
    worker: Option<Worker>,
    queue: Queue,
    frame: Frame,
    generation: u64,
    sequence: u64,
    repeat: Repeat,
    reported_generation: Option<u64>,
    started: Option<SoundStarted>,
    completed_listening: Option<ListeningSample>,
    station_ended: Option<StationTrackEnded>,
    collection: Option<CollectionListening>,
    collection_update: Option<CollectionProgressUpdate>,
    prefetched_near_end: bool,
    resume_position: Option<f64>,
}

impl Playback {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn status(&self) -> &Status {
        &self.frame.status
    }

    pub(crate) fn sound_source(&self) -> Option<&SoundSource> {
        self.frame.source.as_ref()
    }

    pub(crate) fn audio_quality(&self) -> Option<&AudioQuality> {
        self.frame.sound.quality.as_ref()
    }

    pub(crate) fn sound_details(&self) -> &SoundDetails {
        &self.frame.sound
    }

    pub(crate) fn cache_revision(&self) -> u64 {
        self.frame.cache_revision
    }

    pub(crate) fn current(&self) -> Option<&QueueEntry> {
        self.queue.current()
    }

    pub(crate) fn upcoming(&self) -> impl Iterator<Item = &QueueEntry> {
        self.queue.upcoming(self.repeat)
    }

    pub(crate) fn current_index(&self) -> Option<usize> {
        self.queue.current_index()
    }

    pub(crate) fn entries(&self) -> impl ExactSizeIterator<Item = &QueueEntry> {
        self.queue.entries()
    }

    pub(crate) fn queue_revision(&self) -> u64 {
        self.queue.revision()
    }

    pub(crate) fn queued(&self, index: usize) -> Option<(usize, &QueueEntry)> {
        self.queue.at(index)
    }

    pub(crate) fn next_queue_entry_id(&self) -> usize {
        self.queue.next_entry_id()
    }

    pub(crate) fn move_upcoming(
        &mut self,
        revision: u64,
        id: usize,
        source: usize,
        target: usize,
    ) -> bool {
        let moved = self.queue.move_upcoming(revision, id, source, target);
        if moved {
            self.update_prefetch(false);
        }
        moved
    }

    pub(crate) fn remove_upcoming(&mut self, revision: u64, id: usize, position: usize) -> bool {
        let removed = self.queue.remove_upcoming(revision, id, position);
        if removed {
            self.update_prefetch(false);
        }
        removed
    }

    pub(crate) fn clear_upcoming(&mut self, revision: u64) -> bool {
        let cleared = self.queue.clear_upcoming(revision);
        if cleared {
            self.update_prefetch(false);
        }
        cleared
    }

    pub(crate) fn queue_replacement_revision(&self) -> u64 {
        self.queue.replacement_revision()
    }

    pub(crate) fn position(&self) -> f64 {
        if matches!(
            self.frame.status,
            Status::Idle | Status::Loading | Status::Failed(_)
        ) {
            self.resume_position.unwrap_or(self.frame.position)
        } else {
            self.frame.position
        }
    }

    pub(crate) fn duration(&self) -> Option<f64> {
        self.frame
            .duration
            .or_else(|| self.current()?.track.duration.map(f64::from))
    }

    pub(crate) fn volume(&self) -> f64 {
        self.frame.volume
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.frame.error.as_deref()
    }

    pub(crate) fn soloist_setup_required(&self) -> bool {
        self.frame.soloist_setup_required
    }

    pub(crate) fn seekable(&self) -> bool {
        self.frame.seekable && matches!(self.frame.status, Status::Playing | Status::Paused)
    }

    pub(crate) fn can_next(&self) -> bool {
        self.queue.next(self.repeat == Repeat::All).is_some()
    }

    pub(crate) fn can_previous(&self) -> bool {
        self.queue.previous(self.repeat == Repeat::All).is_some()
            || self.seekable() && self.position() > 0.
    }

    pub(crate) fn shuffle(&self) -> bool {
        self.queue.shuffle()
    }

    pub(crate) fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub(crate) fn devices(&self) -> &[AudioDevice] {
        &self.frame.devices
    }

    pub(crate) fn audio_device(&self) -> &str {
        &self.frame.device
    }

    pub(crate) fn can_select_device(&self) -> bool {
        self.frame.output.as_deref() != Some("soloist")
    }

    pub(crate) fn play(&mut self, entries: Vec<QueueEntry>, index: usize) {
        self.play_inner(entries, index, None, 0.);
    }

    pub(crate) fn play_collection(
        &mut self,
        entries: Vec<QueueEntry>,
        index: usize,
        plan: Arc<CollectionPlan>,
    ) {
        self.play_inner(
            entries,
            index,
            Some(CollectionListening::new(plan, Default::default())),
            0.,
        );
    }

    pub(crate) fn resume_collection(
        &mut self,
        plan: Arc<CollectionPlan>,
        progress: CollectionProgress,
    ) {
        let Some(index) = progress.resume_index(&plan) else {
            self.frame.error = Some(UNAVAILABLE_TRACK.into());
            return;
        };
        let position = if progress.at_end {
            0.
        } else {
            progress.position_ms as f64 / 1_000.
        };
        let entries = plan.entries.clone();
        self.play_inner(
            entries,
            index,
            Some(CollectionListening::new(plan, progress)),
            position,
        );
    }

    fn play_inner(
        &mut self,
        entries: Vec<QueueEntry>,
        index: usize,
        collection: Option<CollectionListening>,
        position: f64,
    ) {
        if entries.get(index).is_none_or(|entry| !entry.is_playable()) {
            self.frame.error = Some(UNAVAILABLE_TRACK.into());
            return;
        }
        self.report_collection_progress(false, true);
        self.collection = collection;
        self.queue.replace(entries, index);
        self.start_current(position);
    }

    pub(crate) fn play_shuffled(&mut self, entries: Vec<QueueEntry>) {
        self.shuffle_inner(entries, None);
    }

    pub(crate) fn shuffle_collection(
        &mut self,
        entries: Vec<QueueEntry>,
        plan: Arc<CollectionPlan>,
    ) {
        self.shuffle_inner(
            entries,
            Some(CollectionListening::new(plan, Default::default())),
        );
    }

    fn shuffle_inner(&mut self, entries: Vec<QueueEntry>, collection: Option<CollectionListening>) {
        if entries.iter().all(|entry| !entry.is_playable()) {
            self.frame.error = Some(UNAVAILABLE_TRACK.into());
            return;
        }
        self.report_collection_progress(false, true);
        self.collection = collection;
        self.queue.replace_shuffled(entries);
        self.start_current(0.);
    }

    pub(crate) fn play_queued(&mut self, index: usize) {
        self.report_collection_progress(false, true);
        if self.queue.select(index) {
            self.start_current(0.);
        }
    }

    pub(crate) fn insert_next(&mut self, entries: Vec<QueueEntry>) {
        self.queue.insert(entries, Placement::Next);
        self.update_prefetch(false);
    }

    pub(crate) fn append(&mut self, entries: Vec<QueueEntry>) {
        self.queue.insert(entries, Placement::ManualEnd);
        self.update_prefetch(false);
    }

    pub(crate) fn append_continuation(&mut self, entries: Vec<QueueEntry>) {
        self.queue.insert(entries, Placement::FeedEnd);
        self.update_prefetch(false);
        if self.frame.completed {
            self.advance();
        }
    }

    pub(crate) fn continuation_point(&self) -> Option<ContinuationPoint> {
        (self.frame.completed && self.repeat == Repeat::Off).then_some(ContinuationPoint {
            queue: self.queue.replacement_revision(),
            generation: self.generation,
        })
    }

    pub(crate) fn has_queued_next(&self) -> bool {
        self.queue.next(false).is_some()
    }

    pub(crate) fn continue_after(&mut self, point: ContinuationPoint, entries: Vec<QueueEntry>) {
        if self.continuation_point() == Some(point) {
            self.queue.insert(entries, Placement::AutoplayEnd);
            self.advance();
        }
    }

    pub(crate) fn toggle_pause(&mut self) {
        match self.frame.status {
            Status::Playing => {
                self.send(Command::Pause(true));
            }
            Status::Paused => {
                self.send(Command::Pause(false));
            }
            Status::Failed(_) if self.soloist_setup_required() && self.current().is_some() => {
                self.start_current(self.position());
            }
            Status::Idle | Status::Failed(_) if self.current().is_some() => {
                self.play_current_or_resume()
            }
            Status::Idle | Status::Failed(_) => self.next(),
            Status::Loading => {}
        }
    }

    pub(crate) fn previous(&mut self) {
        if let Some(previous) = self.queue.previous(self.repeat == Repeat::All) {
            self.play_queued(previous);
        } else {
            self.seek(0.);
        }
    }

    pub(crate) fn next(&mut self) {
        if let Some(next) = self.queue.next(self.repeat == Repeat::All) {
            if !self.frame.completed {
                self.report_station_end(true);
            }
            self.play_queued(next);
        }
    }

    fn advance(&mut self) {
        if let Some(next) = self.queue.next(self.repeat == Repeat::All) {
            self.play_queued(next);
        }
    }

    pub(crate) fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() && self.seekable() {
            let seconds = seconds.max(0.).min(self.duration().unwrap_or(f64::MAX));
            self.send(Command::Seek(seconds));
        }
    }

    pub(crate) fn set_volume(&mut self, volume: f64) {
        if volume.is_finite() {
            self.send(Command::Volume(volume.clamp(0., 1.)));
        }
    }

    pub(crate) fn request_devices(&mut self) {
        self.send(Command::Devices);
    }

    pub(crate) fn select_device(&mut self, id: String) {
        self.send(Command::Device(id));
    }

    pub(crate) fn toggle_shuffle(&mut self) {
        self.queue.toggle_shuffle();
        self.update_prefetch(false);
    }

    pub(crate) fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
        self.update_prefetch(false);
    }

    pub(crate) fn needs_poll(&self) -> bool {
        matches!(
            self.frame.status,
            Status::Loading | Status::Playing | Status::Paused
        ) || self.sequence != self.frame.sequence
    }

    pub(crate) fn poll(&mut self) -> PollChange {
        let Some(worker) = &self.worker else {
            return PollChange::None;
        };
        let frame = match worker.frames.try_recv() {
            Ok(frame) if frame.generation == self.generation => frame,
            Err(async_channel::TryRecvError::Closed) => {
                self.engine_failed(WORKER_STOPPED);
                return PollChange::Transport;
            }
            _ => return PollChange::None,
        };
        let changed = self.frame.change(&frame);
        let completed = frame.completed && !self.frame.completed;
        let paused = matches!(frame.status, Status::Paused)
            && (self.frame.status != frame.status || self.frame.position != frame.position);
        self.frame = frame;
        let first_sound = self.reported_generation != Some(self.generation);
        self.report_start();
        self.report_collection_progress(completed, paused || first_sound);
        if completed {
            self.completed_listening = self.listening_sample_in(ListeningPhase::Ended);
            self.report_station_end(false);
            if self.repeat == Repeat::One {
                self.start_current(0.);
            } else {
                self.advance();
            }
            return PollChange::Transport;
        }
        if !self.prefetched_near_end
            && self.frame.status == Status::Playing
            && self
                .duration()
                .is_some_and(|duration| duration - self.position() <= PREFETCH_TAIL.as_secs_f64())
            && self.queue.upcoming(self.repeat).next().is_some()
        {
            self.prefetched_near_end = true;
            self.update_prefetch(true);
            if matches!(self.frame.status, Status::Failed(_)) {
                return PollChange::Transport;
            }
        }
        changed
    }

    /// Take the first sound of the current generation. Pauses, resumes and
    /// seeks reuse the generation, so they never produce a second message.
    pub(crate) fn take_sound_started(&mut self) -> Option<SoundStarted> {
        self.started.take()
    }

    pub(crate) fn take_completed_listening(&mut self) -> Option<ListeningSample> {
        self.completed_listening.take()
    }

    pub(crate) fn listening_sample(&self) -> Option<ListeningSample> {
        let phase = match self.status() {
            Status::Playing => ListeningPhase::Playing,
            Status::Paused => ListeningPhase::Paused,
            Status::Loading => ListeningPhase::Buffering,
            Status::Idle | Status::Failed(_) => return None,
        };
        self.listening_sample_in(phase)
    }

    fn listening_sample_in(&self, phase: ListeningPhase) -> Option<ListeningSample> {
        (self.reported_generation == Some(self.generation)).then(|| ListeningSample {
            generation: self.generation,
            phase,
            position: self.position(),
            duration: self.duration(),
            listened: self.frame.listened,
            seek_revision: self.frame.seek_revision,
        })
    }

    pub(crate) fn take_station_ended(&mut self) -> Option<StationTrackEnded> {
        self.station_ended.take()
    }

    pub(crate) fn take_collection_progress(&mut self) -> Option<CollectionProgressUpdate> {
        self.collection_update.take()
    }

    pub(crate) fn collection_checkpoint(&mut self) -> Option<CollectionProgressUpdate> {
        self.poll();
        self.report_collection_progress(false, true);
        self.take_collection_progress()
    }

    fn report_collection_progress(&mut self, completed: bool, force: bool) {
        if self.reported_generation != Some(self.generation)
            || !completed && !matches!(self.frame.status, Status::Playing | Status::Paused)
        {
            return;
        }
        let duration = self.duration();
        if let (Some(collection), Some(entry)) = (self.collection.as_mut(), self.queue.current())
            && let Some(update) =
                collection.observe(entry, self.frame.position, duration, completed, force)
        {
            self.collection_update = Some(update);
        }
    }

    fn report_station_end(&mut self, skipped: bool) {
        if self.reported_generation != Some(self.generation) {
            return;
        }
        let Some(entry) = self
            .current()
            .filter(|entry| matches!(entry.playlist, PlaylistKey::Station { .. }))
        else {
            return;
        };
        self.station_ended = Some(StationTrackEnded {
            entry: entry.clone(),
            seconds: self.position(),
            skipped,
        });
    }

    fn report_start(&mut self) {
        if self.reported_generation == Some(self.generation) {
            return;
        }
        // The worker keeps a confirmed source on every later frame of this play.
        let Some(source) = self.frame.source else {
            return;
        };
        let Some(entry) = self.current().cloned() else {
            return;
        };
        self.reported_generation = Some(self.generation);
        self.resume_position = None;
        self.started = Some(SoundStarted {
            generation: self.generation,
            source,
            entry,
            details: self.frame.sound.clone(),
        });
    }

    fn start_current(&mut self, position: f64) {
        let requested_at = Instant::now();
        let Some(entry) = self.current().cloned() else {
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        self.resume_position = None;
        self.prefetched_near_end = false;
        self.frame.status = Status::Loading;
        self.frame.position = position;
        self.frame.duration = None;
        self.frame.seekable = false;
        self.frame.error = None;
        self.frame.completed = false;
        self.frame.soloist_setup_required = false;
        self.frame.source = None;
        self.frame.sound = SoundDetails::default();
        self.send(Command::Load {
            entry: Box::new(entry),
            position,
            requested_at,
        });
        self.update_prefetch(false);
    }

    fn update_prefetch(&mut self, near_end: bool) {
        if self.worker.is_some() {
            self.send(Command::Prefetch {
                entries: self.upcoming().cloned().collect(),
                near_end,
            });
        }
    }

    fn send(&mut self, command: Command) {
        let result = if self.worker.is_none() {
            Worker::spawn(self.frame.volume).map(|worker| self.worker.insert(worker))
        } else {
            self.worker
                .as_mut()
                .ok_or("Движок воспроизведения недоступен.")
        };
        let result = result.and_then(|worker| {
            let sequence = self.sequence.wrapping_add(1);
            worker.send(sequence, self.generation, command)?;
            self.sequence = sequence;
            Ok(())
        });
        if let Err(error) = result {
            self.engine_failed(error);
        }
    }

    fn engine_failed(&mut self, error: &str) {
        self.worker = None;
        self.frame.sequence = self.sequence;
        self.frame.soloist_setup_required = false;
        if matches!(
            self.frame.status,
            Status::Loading | Status::Playing | Status::Paused
        ) {
            self.frame.status = Status::Failed(error.to_owned());
            self.frame.seekable = false;
            self.frame.output = None;
            self.frame.source = None;
            self.frame.sound = SoundDetails::default();
            self.frame.error = None;
        } else {
            self.frame.error = Some(error.to_owned());
        }
    }
}
