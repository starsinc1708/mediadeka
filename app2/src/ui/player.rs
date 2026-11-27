#[cfg(feature = "capture")]
mod acceptance;
mod controls;
mod fluid_cover;
mod layout;
mod panels;
mod progress;
mod queue_resume;
mod sound_details;

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, App, AppContext, Context, Div, Entity, EventEmitter, FocusHandle,
    FontWeight, KeyDownEvent, Role, Stateful, Task, Window, div, prelude::*, px, relative, rgb,
};
use guise::{ActionIcon, IconName};

use super::components::slider::{Slider, slider};
use super::listening::reveal;
use super::music::detail::hero_motion::paint_offset;
use super::{
    components::{
        artwork, entity_link,
        music::{compact_control, control, cover},
        overlay::TriggerBounds,
    },
    design_system::icons::{self, Glyph},
    fluid_glass::{Geometry, artwork::Seed, tokens as fluid},
    likes::{Gesture, LikeAction, like_button},
    playlist::{
        artwork::track_cover,
        navigation::{self, Destination},
    },
    theme::{DIVIDER, PRIMARY, SECONDARY, SURFACE, playlist, playlist::metrics},
};
pub(super) use controls::ControlKind;
use controls::{Controls, LISTENING_COVER_ORDER};
use fluid_cover::FluidCover;
use layout::Layout;
use progress::Progress;
use sound_details::SoundPopover;

use crate::{
    music::{ObjectRef, likes::Subject, recording::Recording},
    playback::{
        AudioQuality, CollectionProgressUpdate, ContinuationPoint, ListeningSample, Playback,
        PollChange, QueueEntry, Repeat, SoundCacheChanged, SoundSource, SoundStarted,
        StationTrackEnded, Status,
    },
    playlist::{PlaylistKey, PlaylistTrack},
};

pub(super) const PLAYBACK_POLL_INTERVAL: Duration = Duration::from_millis(150);
const COVER_TRANSITION: Duration = Duration::from_millis(240);
const AMBILIGHT_TRANSITION: Duration = Duration::from_millis(300);
const LISTENING_SEEK_MAX: f32 = 900.;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Snapshot {
    pub key: Option<PlaylistKey>,
    pub index: Option<usize>,
    pub playlist_revision: Option<u64>,
    pub status: Status,
    pub soloist_setup_required: bool,
    pub queue_len: usize,
    pub queue_revision: u64,
    pub queue_position: Option<usize>,
    pub shuffle: bool,
    pub continuation: Option<ContinuationPoint>,
    pub(super) sound_source: Option<SoundSource>,
    pub(super) audio_quality: Option<AudioQuality>,
}

impl Snapshot {
    pub(super) fn highlights(
        &self,
        key: &PlaylistKey,
        revision: Option<u64>,
        index: usize,
    ) -> bool {
        self.identifies(key, revision, index)
            && matches!(self.status, Status::Playing | Status::Paused)
    }

    pub(super) fn identifies(
        &self,
        key: &PlaylistKey,
        revision: Option<u64>,
        index: usize,
    ) -> bool {
        self.key.as_ref() == Some(key)
            && self.playlist_revision == revision
            && self.index == Some(index)
    }
}

pub(super) struct FavoriteRequested {
    pub subject: Subject,
    pub gesture: Gesture,
}

/// The playback queue lives in the right panel, so the player only asks for it.
pub(super) struct QueueRequested;
pub(super) struct TrackRequested;
pub(super) struct LyricsRequested;
pub(super) struct ListeningRequested;
pub(super) struct EntityRequested(pub Destination);
pub(super) struct TrackMenuRequested(pub gpui::Bounds<gpui::Pixels>);
pub(super) struct SeekRequested(pub f64);
pub(super) struct PositionChanged(pub f64);
pub(super) struct ListeningChanged(pub Option<ListeningSample>);

struct CoverTransition {
    previous: Option<String>,
    revision: usize,
    started: Option<Instant>,
}

pub(super) struct PlayerBar {
    playback: Playback,
    queue_resume: Option<queue_resume::Tracking>,
    listening: Option<ListeningSample>,
    progress: Entity<Progress>,
    controls: Entity<Controls>,
    liked: Option<LikeAction>,
    notice: Option<String>,
    polling: Option<Task<()>>,
    listening_focus: FocusHandle,
    devices_open: bool,
    devices_focus: FocusHandle,
    devices_anchor: TriggerBounds,
    devices_return: Option<FocusHandle>,
    sound_popover: SoundPopover,
    queue_focus: FocusHandle,
    skip_focus: [FocusHandle; 2],
    queue_active: bool,
    inspector_active: bool,
    lyrics_active: bool,
    current_cover: Option<String>,
    cover_transition: Option<CoverTransition>,
    cover_revision: usize,
    fluid_cover: FluidCover,
    reduced_motion: bool,
    audible_volume: f64,
    playlist_presentation: bool,
    listening_presentation: bool,
    listening_artwork_motion: Option<super::music::detail::hero_motion::HeroMotion>,
    listening_reveal: Option<super::music::detail::hero_motion::HeroMotion>,
    listening_reveal_frames: Option<Task<()>>,
    fluid: bool,
    fluid_motion: bool,
    #[cfg(feature = "capture")]
    layout_probe: acceptance::LayoutProbe,
}

impl EventEmitter<FavoriteRequested> for PlayerBar {}
impl EventEmitter<QueueRequested> for PlayerBar {}
impl EventEmitter<TrackRequested> for PlayerBar {}
impl EventEmitter<LyricsRequested> for PlayerBar {}
impl EventEmitter<ListeningRequested> for PlayerBar {}
impl EventEmitter<EntityRequested> for PlayerBar {}
impl EventEmitter<TrackMenuRequested> for PlayerBar {}
impl EventEmitter<SeekRequested> for PlayerBar {}
impl EventEmitter<PositionChanged> for PlayerBar {}
impl EventEmitter<ListeningChanged> for PlayerBar {}
impl EventEmitter<SoundCacheChanged> for PlayerBar {}
impl EventEmitter<SoundStarted> for PlayerBar {}
impl EventEmitter<StationTrackEnded> for PlayerBar {}
impl EventEmitter<CollectionProgressUpdate> for PlayerBar {}

impl PlayerBar {
    pub(super) fn navigation_handler(
        cx: &Context<Self>,
    ) -> impl Fn(Destination, &mut Window, &mut App) + Clone + 'static {
        let player = cx.entity().downgrade();
        move |destination, _, cx| {
            player
                .update(cx, |_, cx| cx.emit(EntityRequested(destination)))
                .ok();
        }
    }

    pub(super) fn new(reduced_motion: bool, fluid: bool, cx: &mut App) -> Entity<Self> {
        let fluid_motion = fluid && super::fluid_glass::transition_motion_configured();
        cx.new(|cx| Self {
            playback: Playback::new(),
            queue_resume: None,
            listening: None,
            progress: {
                let player = cx.weak_entity();
                cx.new(|_| Progress::new(player))
            },
            controls: {
                let player = cx.entity();
                cx.new(|cx| Controls::new(&player, cx))
            },
            liked: None,
            notice: None,
            polling: None,
            listening_focus: cx
                .focus_handle()
                .tab_index(LISTENING_COVER_ORDER)
                .tab_stop(true),
            devices_open: false,
            devices_focus: cx.focus_handle(),
            devices_anchor: TriggerBounds::default(),
            devices_return: None,
            sound_popover: SoundPopover::new(cx),
            queue_focus: cx.focus_handle().tab_index(610).tab_stop(true),
            skip_focus: [602, 604].map(|order| cx.focus_handle().tab_index(order).tab_stop(true)),
            queue_active: false,
            inspector_active: false,
            lyrics_active: false,
            current_cover: None,
            cover_transition: None,
            cover_revision: 0,
            fluid_cover: FluidCover::new(fluid_motion),
            reduced_motion,
            audible_volume: 0.75,
            playlist_presentation: false,
            listening_presentation: false,
            listening_artwork_motion: None,
            listening_reveal: None,
            listening_reveal_frames: None,
            fluid,
            fluid_motion,
            #[cfg(feature = "capture")]
            layout_probe: acceptance::LayoutProbe::default(),
        })
    }

    pub(super) fn current(&self) -> Option<&QueueEntry> {
        self.playback.current()
    }

    pub(in crate::ui) fn recording_status(
        &self,
        key: &PlaylistKey,
        track: &PlaylistTrack,
    ) -> Option<&Status> {
        let entry = self.current()?;
        let status = self.playback.status();
        if *status == Status::Idle {
            return None;
        }
        let same_row_id = &entry.playlist == key
            && entry.track.id == track.id
            && !track.id.is_empty()
            && !track.id.starts_with("missing:")
            && entry.track.media == track.media;
        (same_row_id
            || Recording::from_track(&entry.playlist, &entry.track)
                .zip(Recording::from_track(key, track))
                .is_some_and(|(active, row)| active.matches(&row)))
        .then_some(status)
    }

    pub(in crate::ui) fn catalogue_track_status(
        &self,
        reference: &ObjectRef,
        profile: Option<&str>,
    ) -> Option<&Status> {
        let entry = self.current()?;
        let status = self.playback.status();
        if *status == Status::Idle {
            return None;
        }
        Recording::from_track(&entry.playlist, &entry.track)?
            .matches(&Recording::from_reference(reference, profile)?)
            .then_some(status)
    }

    pub(super) fn upcoming(&self) -> impl Iterator<Item = &QueueEntry> {
        self.playback.upcoming()
    }

    pub(super) fn queue_focus(&self) -> &FocusHandle {
        &self.queue_focus
    }

    pub(super) fn current_artwork(&self) -> Option<&str> {
        self.current_cover.as_deref()
    }

    pub(super) fn fluid_track_position(&self) -> Option<(u64, usize)> {
        self.playback
            .current_index()
            .map(|index| (self.playback.queue_replacement_revision(), index))
    }

    #[cfg(feature = "capture")]
    pub(super) fn set_artwork_for_capture(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.current_cover.as_deref() != Some(url) {
            self.current_cover = Some(url.to_owned());
            self.cover_revision = self.cover_revision.wrapping_add(1);
        }
        self.cover_transition = None;
        cx.notify();
    }

    pub(super) fn current_entry(&self) -> Option<&QueueEntry> {
        self.playback.current()
    }

    pub(super) fn current_index(&self) -> Option<usize> {
        self.playback.current_index()
    }

    pub(super) fn queue_len(&self) -> usize {
        self.playback.entries().len()
    }

    pub(super) fn queued(&self, index: usize) -> Option<(usize, &QueueEntry)> {
        self.playback.queued(index)
    }

    pub(super) fn next_queue_entry_id(&self) -> usize {
        self.playback.next_queue_entry_id()
    }

    pub(super) fn queue_revision(&self) -> u64 {
        self.playback.queue_revision()
    }

    pub(super) fn move_upcoming(
        &mut self,
        revision: u64,
        id: usize,
        source: usize,
        target: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.playback.move_upcoming(revision, id, source, target) {
            return false;
        }
        self.publish_queue_resume(true, cx);
        cx.notify();
        true
    }

    pub(super) fn remove_upcoming(
        &mut self,
        revision: u64,
        id: usize,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.playback.remove_upcoming(revision, id, index) {
            return false;
        }
        self.publish_queue_resume(true, cx);
        cx.notify();
        true
    }

    pub(super) fn clear_upcoming(&mut self, revision: u64, cx: &mut Context<Self>) -> bool {
        if !self.playback.clear_upcoming(revision) {
            return false;
        }
        self.publish_queue_resume(true, cx);
        cx.notify();
        true
    }

    pub(super) fn queue_replacement_revision(&self) -> u64 {
        self.playback.queue_replacement_revision()
    }

    pub(super) fn entries(&self) -> impl ExactSizeIterator<Item = &QueueEntry> {
        self.playback.entries()
    }

    pub(super) fn has_queued_next(&self) -> bool {
        self.playback.has_queued_next()
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            key: self.current().map(|entry| entry.playlist.clone()),
            index: self.current().map(|entry| entry.index),
            playlist_revision: self.current().and_then(|entry| entry.playlist_revision),
            status: self.playback.status().clone(),
            soloist_setup_required: self.playback.soloist_setup_required(),
            queue_len: self.playback.entries().len(),
            queue_revision: self.playback.queue_revision(),
            queue_position: self.playback.current_index(),
            shuffle: self.playback.shuffle(),
            continuation: self.playback.continuation_point(),
            sound_source: self.playback.sound_source().copied(),
            audio_quality: self.playback.audio_quality().cloned(),
        }
    }

    pub(super) fn position(&self) -> f64 {
        self.playback.position()
    }

    pub(super) fn current_sound_source(&self) -> Option<SoundSource> {
        self.playback.sound_source().copied()
    }

    pub(super) fn is_playing(&self) -> bool {
        matches!(self.playback.status(), Status::Playing)
    }

    pub(super) fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    pub(super) fn lyrics_seekable(&self) -> bool {
        self.playback.seekable()
    }

    pub(super) fn seek(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if self.playback.seekable() && seconds.is_finite() {
            let seconds = seconds
                .max(0.)
                .min(self.playback.duration().unwrap_or(f64::MAX));
            self.command(cx, |playback| playback.seek(seconds));
            cx.emit(SeekRequested(seconds));
        }
    }

    fn set_volume(&mut self, volume: f64, cx: &mut Context<Self>) {
        self.command(cx, |playback| playback.set_volume(volume));
    }

    pub(super) fn collection_checkpoint(&mut self) -> Option<CollectionProgressUpdate> {
        self.playback.collection_checkpoint()
    }

    pub(super) fn command(&mut self, cx: &mut Context<Self>, action: impl FnOnce(&mut Playback)) {
        self.cancel_queue_restore();
        action(&mut self.playback);
        self.publish_queue_resume(true, cx);
        self.publish_listening(cx);
        if let Some(ended) = self.playback.take_station_ended() {
            cx.emit(ended);
        }
        if let Some(progress) = self.playback.take_collection_progress() {
            cx.emit(progress);
        }
        self.notice = None;
        self.refresh_cover();
        self.sync_progress(cx);
        self.start_polling(cx);
        cx.notify();
    }

    pub(super) fn set_liked(&mut self, liked: Option<LikeAction>, cx: &mut Context<Self>) {
        if self.liked != liked {
            self.liked = liked;
            cx.notify();
        }
    }

    pub(super) fn set_panel_activity(&mut self, queue: bool, lyrics: bool, cx: &mut Context<Self>) {
        if (self.queue_active, self.lyrics_active) != (queue, lyrics) {
            self.queue_active = queue;
            self.lyrics_active = lyrics;
            cx.notify();
        }
    }

    pub(super) fn set_inspector_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.inspector_active != active {
            self.inspector_active = active;
            cx.notify();
        }
    }

    pub(super) fn set_playlist_presentation(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.playlist_presentation != active {
            self.playlist_presentation = active;
            cx.notify();
        }
    }

    pub(super) fn set_listening_presentation(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.listening_presentation != active {
            self.listening_presentation = active;
            cx.notify();
        }
    }

    pub(super) fn set_listening_artwork_motion(
        &mut self,
        motion: Option<super::music::detail::hero_motion::HeroMotion>,
        cx: &mut Context<Self>,
    ) {
        if motion.is_some() || self.listening_artwork_motion.is_some() {
            self.listening_artwork_motion = motion;
            cx.notify();
        }
    }

    /// Drives the staged reveal of the immersive player. The cover keeps its
    /// own shared-artwork motion; this only phases the progress and transport.
    pub(super) fn set_listening_reveal(
        &mut self,
        reveal: Option<super::music::detail::hero_motion::HeroMotion>,
        cx: &mut Context<Self>,
    ) {
        self.listening_reveal = reveal;
        self.listening_reveal_frames.take();
        if reveal.is_some_and(|motion| motion.active() && !motion.sampled()) {
            self.listening_reveal_frames = Some(cx.spawn(async move |bar, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    let active = bar.update(cx, |bar, cx| {
                        let active = bar
                            .listening_reveal
                            .is_some_and(|motion| motion.active() && !motion.sampled());
                        if active {
                            cx.notify();
                        }
                        active
                    });
                    if !active.unwrap_or(false) {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    pub(super) fn displayed_cover_image(&self) -> Option<std::sync::Arc<gpui::RenderImage>> {
        self.fluid_cover.image_for_revision(self.cover_revision)
    }

    fn compact_chrome(&self) -> bool {
        self.playlist_presentation || self.listening_presentation
    }

    fn fluid_chrome(&self) -> bool {
        self.fluid
    }

    fn skip_button(&self, forward: bool, compact: bool, cx: &Context<Self>) -> ActionIcon {
        let (id, label, icon, order) = if forward {
            ("player-next", "Следующий трек", IconName::SkipForward, 604)
        } else {
            ("player-prev", "Предыдущий трек", IconName::SkipBack, 602)
        };
        let available: fn(&Playback) -> bool = if forward {
            Playback::can_next
        } else {
            Playback::can_previous
        };
        let skip: fn(&mut Playback) = if forward {
            Playback::next
        } else {
            Playback::previous
        };
        let enabled = available(&self.playback);
        let button = compact_control(id, label, icon, order, enabled, false);
        let button = self.control_glyph(
            button,
            id,
            if forward {
                Glyph::Next
            } else {
                Glyph::Previous
            },
            false,
            ControlKind::Transport.dimensions(compact).1,
            cx,
        );
        if !enabled {
            return button;
        }
        let index = usize::from(forward);
        let button = button.when(self.fluid_chrome(), |button| {
            button.track_focus(&self.skip_focus[index])
        });
        button.on_click(cx.listener(move |bar, _, window, cx| {
            let focused = bar.fluid_chrome() && bar.skip_focus[index].is_focused(window);
            if bar.fluid_chrome() {
                icons::pulse(window, cx, id);
            }
            bar.command(cx, skip);
            if focused && !available(&bar.playback) {
                window.focus(&bar.queue_focus, cx);
            }
        }))
    }

    pub(super) fn set_reduced_motion(&mut self, reduced: bool, cx: &mut Context<Self>) {
        if self.reduced_motion != reduced {
            self.reduced_motion = reduced;
            if reduced {
                self.cover_transition = None;
            }
            cx.notify();
        }
    }

    pub(super) fn show_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    pub(super) fn clear_notice(&mut self, expected: &str, cx: &mut Context<Self>) {
        if self.notice.as_deref() == Some(expected) {
            self.notice = None;
            cx.notify();
        }
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.polling.is_some() || !self.playback.needs_poll() {
            return;
        }
        self.polling = Some(cx.spawn(async move |bar, cx| {
            loop {
                cx.background_executor().timer(PLAYBACK_POLL_INTERVAL).await;
                let needs_poll = bar
                    .update(cx, |bar, cx| {
                        let cache_revision = bar.playback.cache_revision();
                        let could_go_back = bar.playback.can_previous();
                        let change = bar.playback.poll();
                        if change != PollChange::None {
                            bar.sync_progress(cx);
                        }
                        if change == PollChange::Transport
                            || could_go_back != bar.playback.can_previous()
                        {
                            bar.refresh_cover();
                            cx.notify();
                        } else if change == PollChange::Position {
                            cx.emit(PositionChanged(bar.playback.position()));
                        }
                        if bar.playback.cache_revision() != cache_revision {
                            cx.emit(SoundCacheChanged);
                        }
                        if let Some(started) = bar.playback.take_sound_started() {
                            cx.emit(started);
                        }
                        bar.publish_listening(cx);
                        if let Some(ended) = bar.playback.take_station_ended() {
                            cx.emit(ended);
                        }
                        if let Some(progress) = bar.playback.take_collection_progress() {
                            cx.emit(progress);
                        }
                        let needs_poll = bar.playback.needs_poll();
                        if change != PollChange::None || !needs_poll {
                            let flush =
                                !needs_poll || matches!(bar.playback.status(), Status::Paused);
                            bar.publish_queue_resume(flush, cx);
                        }
                        if !needs_poll {
                            bar.polling.take();
                        }
                        needs_poll
                    })
                    .unwrap_or(false);
                if !needs_poll {
                    break;
                }
            }
        }));
    }

    fn sync_progress(&self, cx: &mut Context<Self>) {
        self.progress
            .update(cx, |progress, cx| progress.sync(&self.playback, cx));
    }

    fn publish_listening(&mut self, cx: &mut Context<Self>) {
        if let Some(completed) = self.playback.take_completed_listening() {
            cx.emit(ListeningChanged(Some(completed)));
        }
        let sample = self.playback.listening_sample();
        if self.listening != sample {
            self.listening = sample;
            cx.emit(ListeningChanged(sample));
        }
    }

    fn refresh_cover(&mut self) {
        let next = self
            .current()
            .and_then(|entry| entry.track.artwork.as_ref());
        if next != self.current_cover.as_ref() {
            let next = next.cloned();
            let previous = std::mem::replace(&mut self.current_cover, next);
            self.cover_revision = self.cover_revision.wrapping_add(1);
            self.cover_transition = (!self.reduced_motion).then_some(CoverTransition {
                previous,
                revision: self.cover_revision,
                started: None,
            });
        }
    }

    fn track_details(
        &mut self,
        compact: bool,
        glass: Option<Geometry>,
        layout: Option<Layout>,
        listening_tool: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let edge = glass.map_or_else(
            || match (self.compact_chrome(), compact) {
                (true, true) => metrics::THUMB,
                (true, false) => metrics::ROW,
                _ => cover_edge(compact),
            },
            Geometry::player_cover,
        );
        let current = self.playback.current();
        let title = current.map_or("Сейчас ничего не играет", |entry| {
            entry.track.title.as_str()
        });
        let detail = current.map_or(
            if glass.is_some() {
                "Выберите трек"
            } else {
                "Выберите трек в плейлисте"
            },
            |entry| entry.track.artist.as_str(),
        );
        let (image, labels) = if glass.is_some() {
            let frame = self.fluid_cover.render(
                fluid_cover::Source {
                    revision: self.cover_revision,
                    track: self
                        .playback
                        .current_index()
                        .and_then(|index| self.playback.queued(index).map(|(id, _)| id)),
                    url: self.current_cover.as_deref(),
                    fallback: current.and_then(|entry| Seed::track(&entry.track)),
                    title,
                    artist: detail,
                },
                edge,
                self.reduced_motion,
                window,
                cx,
            );
            (frame.cover, Some(frame.labels))
        } else {
            (self.legacy_cover(edge), None)
        };
        let image = if glass.is_some() && !self.listening_presentation {
            use super::music::detail::hero_motion::{PLAYER_ARTWORK_SPAN, shared_artwork};
            div().size(px(edge)).child(shared_artwork(
                image,
                PLAYER_ARTWORK_SPAN,
                self.listening_artwork_motion,
            ))
        } else if glass.is_some() {
            let replacement = div().size(px(edge)).flex().items_center().justify_center();
            replacement.when(!listening_tool, |replacement| {
                replacement.child(
                    guise::Icon::new(IconName::Minimize2)
                        .pixel_size(px(18.))
                        .color(gpui::Hsla::from(rgb(fluid::PRIMARY))),
                )
            })
        } else {
            image
        };
        let artwork = self.listening_cover(image, edge, listening_tool, cx);
        let favorite = self.current().map(|_| {
            like_button(
                "player-favorite",
                600,
                "текущий трек ",
                self.liked,
                self.compact_chrome(),
                self.fluid_chrome().then_some(cx as &App),
                cx.listener(|bar, gesture, _, cx| {
                    if let Some(entry) = bar.current()
                        && let Some(subject) = Subject::new(&entry.playlist, &entry.track)
                    {
                        cx.emit(FavoriteRequested {
                            subject,
                            gesture: *gesture,
                        });
                    }
                }),
            )
        });
        let title_size = match (self.fluid_chrome(), compact) {
            (true, true) => fluid::text::BODY,
            (true, false) => fluid::text::TRACK_TITLE,
            (false, true) => 12.,
            (false, false) => 14.,
        };
        let mut headline = div()
            .id("player-track")
            .accessibility_id("player-track")
            .role(Role::Label)
            .aria_label(title.to_owned())
            .min_w_0()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(title_size));
        let settled = labels.as_ref().is_none_or(|labels| {
            matches!(labels.as_slice(), [(visible_title, visible_artist, opacity)]
                if visible_title == title && visible_artist == detail && *opacity >= 0.999)
        });
        let current_link = settled.then_some(current).flatten();
        let mut subtitle = if let Some(labels) = labels {
            let title_height = if compact { 18. } else { 20. };
            let detail_height = 15.;
            headline = headline
                .relative()
                .h(px(title_height))
                .overflow_hidden()
                .line_height(px(title_height))
                // Keep the link and the like control adjacent while the visible
                // title crossfades in the same box.
                .child(div().truncate().opacity(0.).child(title.to_owned()))
                .children(labels.iter().map(|(title, _, opacity)| {
                    div()
                        .absolute()
                        .inset_0()
                        .truncate()
                        .opacity(*opacity)
                        .child(title.clone())
                }));
            (!labels.iter().all(|(_, artist, _)| artist.trim().is_empty())).then(|| {
                div()
                    .relative()
                    .h(px(detail_height))
                    .overflow_hidden()
                    .text_size(px(fluid::text::CAPTION))
                    .line_height(px(detail_height))
                    .text_color(rgb(SECONDARY))
                    .children(labels.into_iter().filter_map(|(_, artist, opacity)| {
                        (!artist.trim().is_empty()).then(|| {
                            div()
                                .absolute()
                                .inset_0()
                                .truncate()
                                .opacity(opacity)
                                .child(artist)
                        })
                    }))
            })
        } else {
            headline = headline.truncate().child(title.to_owned());
            (!detail.trim().is_empty()).then(|| {
                div()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(rgb(SECONDARY))
                    .child(detail.to_owned())
            })
        };
        if let Some(entry) = current_link {
            let open = Self::navigation_handler(cx);
            if let Some(target) = navigation::track_destination(&entry.playlist, &entry.track) {
                let dispatch = open.clone();
                headline =
                    entity_link::activate(headline, title.to_owned(), 599, move |window, cx| {
                        dispatch(target.clone(), window, cx);
                    });
            }
            if !detail.trim().is_empty() {
                subtitle = Some(entity_link::artist_list(
                    div()
                        .text_size(px(if glass.is_some() {
                            fluid::text::CAPTION
                        } else {
                            11.
                        }))
                        .line_height(px(15.))
                        .h(px(15.))
                        .text_color(rgb(SECONDARY)),
                    "player-artist",
                    599,
                    navigation::artists(&entry.playlist, &entry.track),
                    |node, id, name, destination, order| {
                        entity_link::text_with(
                            node,
                            id,
                            name,
                            Some(destination),
                            order,
                            open.clone(),
                        )
                    },
                ));
            }
        }
        let (headline, trailing_favorite) = if glass.is_some() || self.compact_chrome() {
            (
                div()
                    .flex()
                    .items_center()
                    .h(px(match (glass.is_some(), compact) {
                        (true, true) => 22.,
                        (true, false) => fluid::size::SM,
                        (false, true) => 18.,
                        (false, false) => 20.,
                    }))
                    .gap(px(metrics::GAP_SMALL))
                    .child(headline.tooltip(guise::tooltip(title.to_owned())))
                    .children(favorite)
                    .into_any_element(),
                None,
            )
        } else {
            (headline.into_any_element(), favorite)
        };
        let metadata = layout.and_then(|layout| {
            let album = current_link
                .and_then(|entry| navigation::album(&entry.playlist, &entry.track))
                .map(|(name, destination)| {
                    entity_link::text_with(
                        div().min_w_0().flex_1().truncate(),
                        "player-album",
                        name,
                        Some(destination),
                        599,
                        Self::navigation_handler(cx),
                    )
                });
            let status = if layout.compact {
                self.transport_status(compact, cx)
            } else {
                None
            };
            let sound = self.sound_control(
                layout.labeled_metadata || (album.is_none() && status.is_none()),
                cx,
            );
            (album.is_some() || status.is_some() || sound.is_some()).then(|| {
                div()
                    .flex()
                    .min_w_0()
                    .h(px(fluid::space::XL))
                    .items_center()
                    .gap(px(fluid::space::SM))
                    .text_size(px(fluid::text::CAPTION))
                    .text_color(super::theme::secondary())
                    .children(sound)
                    .children(
                        status
                            .map(|status| status.flex_1().into_any_element())
                            .or(album),
                    )
            })
        });
        div()
            .min_w_0()
            .when(glass.is_some() || self.compact_chrome(), |wing| {
                wing.flex_1()
            })
            .when(glass.is_none() && !self.compact_chrome(), |wing| {
                wing.w(relative(0.28)).flex_none()
            })
            .flex()
            .gap(px(if glass.is_some() {
                if compact {
                    fluid::space::SM
                } else {
                    fluid::space::MD
                }
            } else if self.compact_chrome() {
                metrics::COVER_GAP
            } else {
                8.
            }))
            .items_center()
            .child(artwork)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when_some(layout, |labels, layout| {
                        labels.max_w(px(layout.metadata_width()))
                    })
                    .child(headline)
                    .children(subtitle)
                    .children(metadata)
                    .when(glass.is_none(), |labels| {
                        labels.children(self.sound_details(true))
                    }),
            )
            .children(trailing_favorite)
    }

    fn transport_controls(&self, compact: bool, cx: &Context<Self>) -> Div {
        let status = self.playback.status();
        let playing = matches!(status, Status::Playing);
        let loading = matches!(status, Status::Loading);
        let enabled = !loading && (self.current().is_some() || self.playback.can_next());
        let shuffle = self.playback.shuffle();
        let repeat = self.playback.repeat() != Repeat::Off;
        let (repeat_label, repeat_icon) = match self.playback.repeat() {
            Repeat::Off => ("Повтор выключен", IconName::Repeat),
            Repeat::All => ("Повтор очереди", IconName::Repeat),
            Repeat::One => ("Повтор текущего трека", IconName::Repeat1),
        };
        let play_label = if playing {
            "Пауза"
        } else if loading {
            "Загрузка аудио"
        } else {
            "Воспроизвести"
        };
        let play_icon = if playing {
            IconName::Pause
        } else if loading {
            IconName::LoaderCircle
        } else {
            IconName::Play
        };
        let play = control(
            "player-play",
            play_label,
            play_icon,
            603,
            enabled,
            true,
            false,
        );
        let play = if loading {
            play
        } else {
            self.control_glyph(
                play,
                "player-play",
                Glyph::PlayPause,
                playing,
                ControlKind::Primary.dimensions(compact).1,
                cx,
            )
        };
        let play = if enabled {
            play.on_click(cx.listener(|bar, _, _, cx| bar.command(cx, Playback::toggle_pause)))
        } else {
            play
        };
        let tool_glyph_size = ControlKind::Tool.dimensions(compact).1;
        let shuffle_button = player_button(
            "player-shuffle",
            "Перемешать очередь",
            IconName::Shuffle,
            601,
            (self.playback.entries().len() > 1, shuffle),
            Playback::toggle_shuffle,
            cx,
        )
        .when(self.fluid_chrome() || self.compact_chrome(), |button| {
            button.toggled(shuffle)
        });
        let repeat_button = player_button(
            "player-repeat",
            repeat_label,
            repeat_icon,
            605,
            (self.playback.entries().len() > 0, repeat),
            Playback::cycle_repeat,
            cx,
        )
        .when(self.fluid_chrome() || self.compact_chrome(), |button| {
            button.toggled(repeat)
        });
        let repeat_one = self.playback.repeat() == Repeat::One;
        let repeat_button = repeat_button.when(self.fluid_chrome(), |button| {
            button.glyph_content(icons::repeat_glyph(
                "player-repeat",
                repeat,
                repeat_one,
                tool_glyph_size,
                cx,
            ))
        });
        let buttons = [
            (
                self.control_glyph(
                    shuffle_button,
                    "player-shuffle",
                    Glyph::Shuffle,
                    shuffle,
                    tool_glyph_size,
                    cx,
                ),
                !shuffle,
                ControlKind::toggle(shuffle),
            ),
            (
                self.skip_button(false, compact, cx),
                false,
                ControlKind::Transport,
            ),
            (play, false, ControlKind::Primary),
            (
                self.skip_button(true, compact, cx),
                false,
                ControlKind::Transport,
            ),
            (repeat_button, !repeat, ControlKind::toggle(repeat)),
        ];
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(if self.fluid_chrome() {
                if compact {
                    fluid::space::XS
                } else {
                    fluid::space::SM
                }
            } else if self.compact_chrome() {
                metrics::GAP
            } else if compact {
                6.
            } else {
                10.
            }))
            .children(buttons.map(|(button, muted, kind)| {
                let button = button.when(self.compact_chrome(), |button| {
                    playlist_player_icon(button, metrics::CONTROL_SMALL, muted)
                });
                self.control_feedback(button, kind, compact, cx).when(
                    self.fluid_chrome()
                        && matches!(kind, ControlKind::Tool | ControlKind::Selected),
                    |button| button.pixel_size(px(ControlKind::Transport.dimensions(compact).0)),
                )
            }))
    }

    fn legacy_cover(&self, edge: f32) -> Div {
        let artwork = |url| {
            if self.compact_chrome() {
                track_cover(url, edge).bg(rgb(playlist::PAPER))
            } else {
                cover(url, edge, 7., false)
            }
        };
        let image = artwork(self.current_cover.as_deref());
        let frame = div().size(px(edge)).flex_none().relative();
        if let Some(transition) = &self.cover_transition {
            frame.child(artwork(transition.previous.as_deref())).child(
                image.absolute().top_0().left_0().with_animation(
                    ("player-cover", transition.revision),
                    Animation::new(COVER_TRANSITION),
                    |image, value| image.opacity(value),
                ),
            )
        } else {
            frame.child(image)
        }
    }

    fn transport_status(&self, compact: bool, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let message = match self.playback.status() {
            Status::Failed(error) => Some(error.as_str()),
            Status::Loading => Some("Загружаем аудио…"),
            _ => self.notice.as_deref().or_else(|| self.playback.error()),
        }?;
        Some(
            div()
                .id("player-message")
                .accessibility_id("player-message")
                .role(Role::Status)
                .aria_label(message.to_owned())
                .tooltip(guise::tooltip(message.to_owned()))
                .min_w_0()
                .truncate()
                .text_size(px(if self.fluid_chrome() {
                    fluid::text::CAPTION
                } else {
                    10.
                }))
                .when(self.fluid_chrome() && compact, |status| {
                    status.line_height(px(15.))
                })
                .text_color(
                    if self.fluid_chrome()
                        && matches!(self.playback.status(), Status::Failed(_))
                        && !super::theme::fluid_high_contrast(cx)
                    {
                        rgb(fluid::ERROR).into()
                    } else {
                        super::theme::secondary()
                    },
                )
                .child(message.to_owned()),
        )
    }

    fn transport(&self, compact: bool, layout: Option<Layout>, cx: &Context<Self>) -> Div {
        let reveal = self.listening_reveal.filter(|motion| motion.active());
        let controls = self.controls.clone().cached(Controls::style(
            self.compact_chrome(),
            layout.map(|_| compact),
        ));
        let controls = if let Some(motion) = reveal {
            let (opacity, _) = reveal::transport(Some(motion));
            // Opacity only: the transport keeps its working coordinates while
            // it appears.
            div().opacity(opacity).child(controls).into_any_element()
        } else {
            controls.into_any_element()
        };
        let progress = self.progress.clone();
        let progress = if let Some(motion) = reveal {
            let (opacity, offset) = reveal::progress(Some(motion));
            paint_offset(div().opacity(opacity).child(progress), px(offset))
        } else {
            progress.into_any_element()
        };
        let stack = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(if layout.is_some() {
                if compact { 0. } else { fluid::space::XS }
            } else {
                2.
            }))
            .when(layout.is_none() && self.compact_chrome(), |transport| {
                let width = if self.listening_presentation {
                    LISTENING_SEEK_MAX
                } else {
                    metrics::SEEK_MAX
                };
                transport.max_w(px(width)).gap(px(metrics::GAP_SMALL))
            })
            .child(controls)
            .child(progress);
        if let Some(layout) = layout {
            div()
                .relative()
                .w(px(layout.transport_width))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .child(stack)
                .when(!compact, |transport| {
                    transport.children(self.transport_status(compact, cx).map(|status| {
                        status
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .text_center()
                    }))
                })
        } else {
            stack.flex_1().children(
                self.transport_status(compact, cx)
                    .map(|status| status.w_full().text_center()),
            )
        }
    }

    fn tools(
        &self,
        compact: bool,
        layout: Option<Layout>,
        listening_tool: bool,
        cx: &Context<Self>,
    ) -> Div {
        let volume = self.playback.volume();
        let icon_size = if compact {
            metrics::CONTROL_SMALL
        } else {
            metrics::CONTROL
        };
        let style = |button: ActionIcon, muted, selected| {
            let button = button.when(self.compact_chrome(), |button| {
                playlist_player_icon(button, icon_size, muted)
            });
            self.control_feedback(button, ControlKind::toggle(selected), compact, cx)
        };
        let glyph_size = ControlKind::Tool.dimensions(compact).1;
        let glyph = |button: ActionIcon, id, glyph, target| {
            self.control_glyph(button, id, glyph, target, glyph_size, cx)
        };
        let mute = compact_control(
            "player-mute",
            if volume > 0. {
                "Выключить звук"
            } else {
                "Включить звук"
            },
            if volume > 0. {
                IconName::Volume2
            } else {
                IconName::VolumeX
            },
            607,
            true,
            volume == 0.,
        )
        .on_click(cx.listener(|bar, _, _, cx| {
            let volume = bar.playback.volume();
            let target = if volume > 0. {
                bar.audible_volume = volume;
                0.
            } else {
                bar.audible_volume
            };
            bar.command(cx, |playback| playback.set_volume(target));
        }));
        let queue = compact_control(
            "player-queue",
            "Очередь воспроизведения",
            IconName::ListMusic,
            610,
            true,
            self.queue_active,
        )
        .track_focus(&self.queue_focus)
        .on_click(cx.listener(|_, _, _, cx| cx.emit(QueueRequested)));
        let lyrics = compact_control(
            "player-lyrics",
            "Текст песни",
            IconName::Captions,
            611,
            true,
            self.lyrics_active,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.emit(LyricsRequested)));
        let inspector = (self.fluid_chrome() && !self.listening_presentation).then(|| {
            compact_control(
                "player-track-inspector",
                "Информация о треке",
                IconName::Info,
                610,
                self.current().is_some(),
                self.inspector_active,
            )
            .when(self.current().is_some(), |button| {
                button.on_click(cx.listener(|_, _, _, cx| cx.emit(TrackRequested)))
            })
        });
        let volume_width = layout.map_or_else(
            || match (self.compact_chrome(), compact) {
                (true, true) => metrics::VOLUME_COMPACT,
                (true, false) => metrics::VOLUME,
                _ => 101.,
            },
            Layout::volume_width,
        );
        div()
            .min_w_0()
            .when(self.fluid_chrome() || self.compact_chrome(), |wing| {
                wing.flex_1()
            })
            .when(!self.fluid_chrome() && !self.compact_chrome(), |wing| {
                wing.w(relative(0.28)).flex_none()
            })
            .flex()
            .items_center()
            .justify_end()
            .gap(px(if self.fluid_chrome() {
                if compact {
                    fluid::space::XS
                } else {
                    fluid::space::SM
                }
            } else if self.compact_chrome() {
                metrics::GAP_SMALL
            } else if compact {
                3.
            } else {
                8.
            }))
            .child(style(
                glyph(mute, "player-mute", Glyph::Volume, volume == 0.),
                false,
                volume == 0.,
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .max_w(px(volume_width))
                    .child(slider(
                        "player-volume",
                        "Громкость",
                        608,
                        Slider {
                            value: volume,
                            min: 0.,
                            max: 1.,
                            step: 0.05,
                            enabled: true,
                            playlist_presentation: self.compact_chrome(),
                            fluid: self.fluid_chrome(),
                        },
                        Self::set_volume,
                        cx,
                    )),
            )
            .child(
                self.devices_anchor
                    .capture(
                        style(
                            compact_control(
                                "player-devices",
                                "Устройство вывода",
                                IconName::MonitorSpeaker,
                                609,
                                self.playback.can_select_device(),
                                self.devices_open,
                            ),
                            !self.devices_open,
                            self.devices_open,
                        )
                        .tooltip_enabled(!self.devices_open)
                        .when(
                            self.playback.can_select_device(),
                            |control| {
                                control.on_click(
                                    cx.listener(|bar, _, window, cx| bar.show_devices(window, cx)),
                                )
                            },
                        ),
                    )
                    .when(self.fluid_chrome(), |devices| {
                        devices.mr(px(fluid::space::XS))
                    }),
            )
            .child(style(
                glyph(queue, "player-queue", Glyph::Queue, self.queue_active),
                !self.queue_active,
                self.queue_active,
            ))
            .children(
                inspector.map(|inspector| {
                    style(inspector, !self.inspector_active, self.inspector_active)
                }),
            )
            .child(style(
                glyph(lyrics, "player-lyrics", Glyph::Lyrics, self.lyrics_active),
                !self.lyrics_active,
                self.lyrics_active,
            ))
            .when(listening_tool, |tools| {
                tools.child(style(
                    glyph(
                        self.listening_tool(cx),
                        "player-listening",
                        Glyph::Fullscreen,
                        self.listening_presentation,
                    ),
                    false,
                    false,
                ))
            })
    }
}

impl Render for PlayerBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(transition) = &mut self.cover_transition {
            let now = Instant::now();
            // The animation schedules its own frames. Discard the old artwork as soon as the
            // longer crossfade ends, including after restoring a minimized window.
            if super::native::minimized(window)
                || now.duration_since(*transition.started.get_or_insert(now))
                    >= AMBILIGHT_TRANSITION
            {
                self.cover_transition = None;
            }
        }
        let glass = self.fluid_chrome();
        let geometry = glass.then(|| Geometry::for_window(window));
        let layout = glass.then(|| Layout::for_window(window));
        let compact = layout.map_or_else(
            || window.viewport_size().height < px(600.),
            |layout| layout.compact,
        );
        let edge = px(cover_edge(compact));
        let listening_tool = self.place_listening_control(window);
        let row = div()
            .relative()
            .flex()
            .items_center()
            .w_full()
            .gap(px(9.))
            .when(layout.is_none() && self.compact_chrome(), |row| {
                row.gap(px(if compact {
                    metrics::GAP
                } else {
                    metrics::GAP_WIDE
                }))
            })
            .when_some(layout, |row, layout| row.h_full().gap(px(layout.gap())))
            .child(self.track_details(compact, geometry, layout, listening_tool, window, cx))
            .child(self.transport(compact, layout, cx))
            .child(self.tools(compact, layout, listening_tool, cx));
        #[cfg(feature = "capture")]
        let row = self.layout_probe.record(row);
        let glow = self
            .current()
            .filter(|_| !self.compact_chrome() && !glass)
            .map(|_| {
                let glow = div().absolute().inset_0().child(artwork::ambilight(
                    self.current_cover.as_deref(),
                    edge,
                    false,
                ));
                if let Some(transition) = &self.cover_transition {
                    div()
                        .absolute()
                        .inset_0()
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(artwork::ambilight(
                                    transition.previous.as_deref(),
                                    edge,
                                    false,
                                ))
                                .with_animation(
                                    ("player-ambilight-out", transition.revision),
                                    Animation::new(AMBILIGHT_TRANSITION),
                                    |glow, progress| glow.opacity(1. - progress),
                                ),
                        )
                        .child(glow.with_animation(
                            ("player-ambilight-in", transition.revision),
                            Animation::new(AMBILIGHT_TRANSITION),
                            |glow, progress| glow.opacity(progress),
                        ))
                        .into_any_element()
                } else {
                    glow.into_any_element()
                }
            });
        div()
            .id("home-transport")
            .accessibility_id("home-transport")
            .role(Role::Group)
            .aria_label("Состояние воспроизведения")
            .when(
                !self.listening_presentation && self.current().is_some(),
                |bar| {
                    bar.on_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "f10" && event.keystroke.modifiers.shift {
                            let bounds = window
                                .rendered_span(
                                    super::music::detail::hero_motion::PLAYER_ARTWORK_SPAN,
                                )
                                .unwrap_or_else(|| {
                                    gpui::Bounds::new(
                                        window.mouse_position(),
                                        gpui::size(px(1.), px(1.)),
                                    )
                                });
                            cx.stop_propagation();
                            cx.emit(TrackMenuRequested(bounds));
                        }
                    }))
                },
            )
            .min_h(px(if compact { 58. } else { 76. }))
            .relative()
            .flex_none()
            .px(px(if compact { 9. } else { 14. }))
            .py(px(if compact { 3.5 } else { 6. }))
            .gap(px(3.))
            .flex()
            .flex_col()
            .justify_center()
            .when(self.compact_chrome(), |bar| {
                bar.min_h(px(if compact {
                    metrics::PLAYER_COMPACT
                } else {
                    metrics::PLAYER
                }))
                .px(px(metrics::PLAYER_INSET))
                .border_t_1()
                .border_color(rgb(playlist::RULE))
            })
            .when(!self.compact_chrome(), |bar| {
                bar.border_t_1().border_color(rgb(DIVIDER))
            })
            .text_color(rgb(PRIMARY))
            .bg(rgb(if self.compact_chrome() {
                playlist::PANEL
            } else {
                SURFACE
            }))
            .when(self.listening_presentation, |bar| {
                bar.bg(gpui::Hsla::from(rgb(playlist::PANEL)).alpha(0.82))
            })
            .when_some(geometry.zip(layout), |bar, (geometry, layout)| {
                let height = px(geometry.player());
                bar.h(height)
                    .min_h(height)
                    .px(px(layout.inset()))
                    .py_0()
                    .border_t_0()
                    .text_color(rgb(fluid::PRIMARY))
                    .bg(gpui::transparent_black())
            })
            .children(glow)
            .child(row)
            .children(self.sound_popover_panel(window, cx))
            .children(self.devices_panel(window, cx))
    }
}

pub(super) fn cover_edge(compact: bool) -> f32 {
    if compact { 37. } else { 47. }
}

fn playlist_player_icon(button: ActionIcon, size: f32, muted: bool) -> ActionIcon {
    button
        .pixel_size(px(size))
        .variant(guise::Variant::Subtle)
        .color(gpui::Hsla::from(rgb(if muted {
            playlist::INK_SECONDARY
        } else {
            playlist::SIGNAL
        })))
}

fn player_button(
    id: &'static str,
    label: &'static str,
    glyph: IconName,
    order: isize,
    (enabled, active): (bool, bool),
    action: impl Fn(&mut Playback) + 'static,
    cx: &Context<PlayerBar>,
) -> ActionIcon {
    let button = compact_control(id, label, glyph, order, enabled, active);
    if enabled {
        button.on_click(cx.listener(move |bar, _, _, cx| bar.command(cx, &action)))
    } else {
        button
    }
}
