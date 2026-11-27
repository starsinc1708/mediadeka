mod clock;
mod emphasis;
mod follow;
mod genius;
mod karaoke;
mod prefetch;
mod view;

#[cfg(feature = "capture")]
mod acceptance;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, ScrollHandle, Subscription, Task,
    Window, point, px,
};

use crate::{
    music::lyrics::{self, Lyrics, LyricsVersions, Source},
    playback::{QueueEntry, SoundSource},
    storage::Store,
};

use super::{
    MainView,
    components::overlay::Select,
    player::{PlayerBar, PositionChanged, SeekRequested},
};
use clock::{Clock, Highlight};
use emphasis::Emphasis;
use follow::{FollowScroll, ScrollIntent};
use genius::Genius;
use prefetch::Prefetch;

const MOTION_TICK: Duration = Duration::from_millis(33);
const SOURCE_MENU_WIDTH: f32 = 280.;

pub(super) struct FullscreenRequested;

pub(super) struct LyricsView {
    player: Entity<PlayerBar>,
    _playback: Subscription,
    _position: Subscription,
    _seek: Subscription,
    focus: FocusHandle,
    change_source_focus: FocusHandle,
    picker: Select,
    scroll: ScrollHandle,
    entry: Option<QueueEntry>,
    inspection: Option<QueueEntry>,
    follows_playback: bool,
    sound: Option<SoundSource>,
    selection: Option<Source>,
    text: Option<LyricsVersions>,
    display_plain: bool,
    load: Option<Job>,
    prefetch: Prefetch,
    failed: bool,
    genius: Genius,
    visible: bool,
    presented: bool,
    fullscreen: bool,
    fluid: bool,
    follow: bool,
    follow_scroll: FollowScroll,
    seekable: bool,
    clock: Clock,
    highlight: Highlight,
    emphasis: Emphasis,
    frame_pending: bool,
    sweep_moving: bool,
}

impl EventEmitter<FullscreenRequested> for LyricsView {}

impl LyricsView {
    pub(super) fn new(player: Entity<PlayerBar>, fluid: bool, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let playback = cx.observe(&player, |view: &mut Self, _, cx| view.player_changed(cx));
            let position = cx.subscribe(
                &player,
                |view: &mut Self, _, event: &PositionChanged, cx| {
                    if view.visible && view.follows_playback {
                        view.clock.sample(event.0, view.clock.playing());
                        view.update_highlight(cx);
                    }
                },
            );
            let seek = cx.subscribe(&player, |view: &mut Self, _, event: &SeekRequested, cx| {
                if view.visible && view.follows_playback {
                    view.clock.seek(event.0);
                    view.follow_current_line(cx);
                }
            });
            let mut view = Self {
                player,
                _playback: playback,
                _position: position,
                _seek: seek,
                focus: cx.focus_handle(),
                change_source_focus: cx.focus_handle().tab_index(570).tab_stop(true),
                picker: Select::new(560, cx).with_menu_width(SOURCE_MENU_WIDTH),
                scroll: ScrollHandle::new(),
                entry: None,
                inspection: None,
                follows_playback: true,
                sound: None,
                selection: None,
                text: None,
                display_plain: false,
                load: None,
                prefetch: Prefetch::default(),
                failed: false,
                genius: Genius::default(),
                visible: false,
                presented: false,
                fullscreen: false,
                fluid,
                follow: true,
                follow_scroll: FollowScroll::default(),
                seekable: false,
                clock: Clock::default(),
                highlight: Highlight::default(),
                emphasis: Emphasis::default(),
                frame_pending: false,
                sweep_moving: false,
            };
            view.player_changed(cx);
            view
        })
    }

    pub(super) fn set_visibility(
        &mut self,
        visible: bool,
        presented: bool,
        cx: &mut Context<Self>,
    ) {
        if self.presented != presented {
            self.presented = presented;
            if presented && self.follow && self.highlight.line.is_some() {
                self.follow_scroll.request(ScrollIntent::Snap);
            }
            cx.notify();
        }
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.player_changed(cx);
            if self.load.is_none() && self.failed && self.text.is_none() {
                self.start_load(cx, false);
            }
            if self.load.is_none() || self.text.is_some() {
                self.start_genius(cx);
            }
        } else {
            self.picker.reset();
        }
        cx.notify();
    }

    pub(super) fn set_immersive(&mut self, immersive: bool, cx: &mut Context<Self>) {
        if self.fullscreen != immersive {
            self.fullscreen = immersive;
            self.picker.reset();
            self.follow_scroll = FollowScroll::default();
            if self.follow && self.highlight.line.is_some() {
                self.follow_scroll.request(ScrollIntent::Snap);
            }
            cx.notify();
        }
    }

    pub(in crate::ui) fn set_inspection(
        &mut self,
        entry: Option<QueueEntry>,
        cx: &mut Context<Self>,
    ) {
        if self.inspection != entry {
            self.inspection = entry;
            self.player_changed(cx);
        }
    }

    fn player_changed(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        let playing_entry = player.current();
        let entry = self.inspection.as_ref().or(playing_entry);
        let follows_playback = self.inspection.as_ref().is_none_or(|inspected| {
            playing_entry.is_some_and(|playing| inspected.same_recording(playing))
        });
        let prepare_playing = playing_entry.filter(|_| !follows_playback);
        self.prefetch
            .reconcile(entry, prepare_playing.into_iter().chain(player.upcoming()));
        let entry_changed = self.entry.as_ref() != entry;
        let follows_playback_changed = self.follows_playback != follows_playback;
        self.follows_playback = follows_playback;
        if follows_playback_changed {
            self.clock = Clock::default();
        }
        let sound = player.current_sound_source().filter(|_| follows_playback);
        let origin = || {
            let entry = entry?;
            crate::music::sound::recording(&entry.playlist, &entry.track)
                .map(|reference| reference.provider)
        };
        let source_provider =
            |source: Option<SoundSource>| source.and_then(SoundSource::provider).or_else(origin);
        let provider_changed =
            self.sound != sound && source_provider(self.sound) != source_provider(sound);
        self.sound = sound;
        if entry_changed {
            self.entry = entry.cloned();
            self.clear_track();
        }
        if entry_changed || provider_changed && self.selection.is_none() {
            self.start_load(cx, false);
        }
        self.start_upcoming(cx);
        if !self.visible {
            return;
        }
        let player = self.player.read(cx);
        let playing = follows_playback && player.is_playing();
        let playing_changed = self.clock.playing() != playing;
        let seekable = follows_playback && player.lyrics_seekable();
        let seekable_changed = self.seekable != seekable;
        self.seekable = seekable;
        if follows_playback {
            self.clock.sample(player.position(), playing);
        }
        self.update_highlight(cx);
        if playing_changed || seekable_changed || follows_playback_changed {
            cx.notify();
        }
    }

    fn clear_track(&mut self) {
        self.selection = None;
        self.load = None;
        self.text = None;
        self.display_plain = false;
        self.genius = Genius::default();
        self.failed = false;
        self.clock = Clock::default();
        self.highlight = Highlight::default();
        self.emphasis = Emphasis::default();
        self.sweep_moving = false;
        self.picker.reset();
        self.follow = true;
        self.follow_scroll = FollowScroll::default();
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn choose_source(
        &mut self,
        source: Option<Source>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.close(window, cx);
        if self.selection != source {
            self.picker.focus_trigger(window, cx);
            self.selection = source;
            self.text = None;
            self.display_plain = false;
            self.reset_presentation();
            self.start_load(cx, false);
            self.indices_changed(cx);
        } else if self.display_plain {
            self.picker.focus_trigger(window, cx);
            self.display_plain = false;
            self.reset_presentation();
            self.indices_changed(cx);
            self.follow_current_line(cx);
        }
        cx.notify();
    }

    fn choose_plain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.plain_source() else {
            return;
        };
        self.picker.close(window, cx);
        self.picker.focus_trigger(window, cx);
        if !self.display_plain {
            self.selection = Some(source);
            self.display_plain = true;
            self.load = None;
            self.failed = false;
            self.reset_presentation();
            self.indices_changed(cx);
            self.start_upcoming(cx);
        }
        cx.notify();
    }

    fn reset_presentation(&mut self) {
        self.genius.open_note = None;
        self.follow = true;
        self.follow_scroll = FollowScroll::default();
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn plain_source(&self) -> Option<Source> {
        Some(self.text.as_ref()?.alternate_plain()?.source)
    }

    fn start_load(&mut self, cx: &mut Context<Self>, retry: bool) {
        self.load = None;
        self.failed = false;
        let Some(entry) = self.entry.clone() else {
            cx.notify();
            return;
        };
        let source = self.selection;
        if source == Some(Source::Genius) {
            self.start_genius(cx);
            cx.notify();
            return;
        }
        let sound = self.sound;
        self.load = Some(Job::run(
            cx,
            move |flag, sender| {
                let store = Store::open_default().map_err(|error| error.to_string())?;
                let mut emit = |text| {
                    let _ = sender.send_blocking(text);
                };
                if retry {
                    lyrics::reload(&store, &entry, sound.as_ref(), source, flag, &mut emit)
                } else {
                    lyrics::load(&store, &entry, sound.as_ref(), source, flag, &mut emit)
                }
            },
            |view, text, cx| {
                view.text = Some(text);
                if view.visible {
                    view.indices_changed(cx);
                    view.start_genius(cx);
                }
            },
            |view, result, cx| {
                view.load = None;
                view.failed = result.is_err();
                view.start_genius(cx);
                view.start_upcoming(cx);
            },
        ));
        cx.notify();
    }

    fn retry_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.entry.is_some() && self.load.is_none() {
            self.genius = Genius::default();
            self.start_load(cx, true);
        }
    }

    fn current_text(&self) -> Option<&Lyrics> {
        self.text
            .as_ref()
            .map(|versions| {
                if self.display_plain
                    && let Some(plain) = versions.alternate_plain()
                {
                    plain
                } else {
                    versions.preferred()
                }
            })
            .or_else(|| {
                if self.selection.is_none() || self.selection == Some(Source::Genius) {
                    self.genius.song.as_ref()?.lyrics.as_ref()
                } else {
                    None
                }
            })
    }

    fn indices_changed(&mut self, cx: &mut Context<Self>) {
        self.remap_annotations();
        self.update_highlight(cx);
        cx.notify();
    }

    fn update_highlight(&mut self, cx: &mut Context<Self>) {
        let next = if self.follows_playback {
            self.current_text()
                .map_or_else(Highlight::default, |lyrics| {
                    Highlight::at(lyrics, self.clock.position_ms())
                })
        } else {
            Highlight::default()
        };
        if self.highlight == next {
            return;
        }
        if self.fluid && self.highlight.line != next.line {
            self.emphasis.select(next.line, Instant::now());
        }
        if self.follow && self.highlight.line != next.line {
            if next.line.is_some() {
                self.follow_scroll.request(ScrollIntent::Glide);
            } else {
                self.follow_scroll = FollowScroll::default();
            }
        }
        self.highlight = next;
        cx.notify();
    }

    fn follow_current_line(&mut self, cx: &mut Context<Self>) {
        self.follow = true;
        self.update_highlight(cx);
        if self.highlight.line.is_some() {
            self.follow_scroll.request(ScrollIntent::Snap);
        }
        cx.notify();
    }

    fn seek_line(&mut self, time_ms: u64, cx: &mut Context<Self>) {
        if !self.seekable {
            return;
        }
        self.player.update(cx, |bar, cx| {
            bar.seek(time_ms as f64 / 1000., cx);
        });
    }

    fn schedule_frame(&mut self, window: &Window, cx: &mut Context<Self>) {
        let reduced = self.reduced_motion(cx);
        let emphasis = !reduced && self.emphasis.running(Instant::now());
        let sync = self.clock.playing()
            && self.current_text().is_some_and(|text| {
                text.lines
                    .first()
                    .is_some_and(|line| line.time_ms.is_some())
            });
        if self.frame_pending
            || !self.presented
            || (!sync && !self.follow_scroll.busy() && !emphasis)
            || super::native::minimized(window)
        {
            return;
        }
        self.frame_pending = true;
        let timer = cx.background_executor().timer(MOTION_TICK);
        cx.spawn_in(window, async move |view, cx| {
            timer.await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.frame_pending = false;
                if !view.presented || super::native::minimized(window) {
                    return;
                }
                view.update_highlight(cx);
                let reduced = view.reduced_motion(cx);
                let scrolled =
                    view.follow_scroll
                        .advance(&view.scroll, view.highlight.line, reduced);
                let moving = !reduced
                    && view.clock.playing()
                    && view.active_sweep(false).is_some_and(|sweep| sweep.moving());
                let final_sweep_frame = view.sweep_moving != moving;
                view.sweep_moving = moving;
                if scrolled || moving || final_sweep_frame || emphasis {
                    cx.notify();
                }
                view.schedule_frame(window, cx);
            });
        })
        .detach();
    }

    fn reduced_motion(&self, cx: &App) -> bool {
        cx.reduce_motion() || self.player.read(cx).reduced_motion()
    }

    fn active_sweep(&self, reduced: bool) -> Option<karaoke::Sweep> {
        let index = self.highlight.line?;
        let text = self.current_text()?;
        let line = text.lines.get(index)?;
        (!line.words.is_empty()).then(|| {
            let next = text.lines.get(index + 1).and_then(|line| line.time_ms);
            karaoke::Sweep::at(line, self.clock.position_ms(), next, reduced)
        })
    }
}

struct Job {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Job {
    fn run<T: Send + 'static, O: Send + 'static>(
        cx: &mut Context<LyricsView>,
        work: impl FnOnce(&AtomicBool, async_channel::Sender<T>) -> Result<O, String> + Send + 'static,
        receive: impl Fn(&mut LyricsView, T, &mut Context<LyricsView>) + 'static,
        finish: impl FnOnce(&mut LyricsView, Result<O, String>, &mut Context<LyricsView>) + 'static,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let check = Arc::clone(&cancelled);
        let task = cx.spawn(async move |view, cx| {
            let (tx, rx) = async_channel::unbounded();
            let flag = Arc::clone(&check);
            let worker = cx
                .background_executor()
                .spawn(async move { work(&flag, tx) });
            while let Ok(update) = rx.recv().await {
                if check.load(Ordering::Relaxed)
                    || view
                        .update(cx, |view, cx| {
                            receive(view, update, cx);
                        })
                        .is_err()
                {
                    return;
                }
            }
            let result = worker.await;
            if !check.load(Ordering::Relaxed) {
                let _ = view.update(cx, |view, cx| {
                    finish(view, result, cx);
                    cx.notify();
                });
            }
        });
        Self {
            cancelled,
            _task: task,
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl MainView {
    pub(super) fn lyrics_accounts_changed(&mut self, cx: &mut Context<Self>) {
        self.lyrics.update(cx, |view, cx| {
            view.prefetch = Prefetch::default();
            view.text = None;
            view.start_load(cx, false);
            view.player_changed(cx);
            view.indices_changed(cx);
        });
    }

    pub(super) fn dismiss_lyrics_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.lyrics.update(cx, |view, cx| {
            if view.picker.close(window, cx) {
                cx.notify();
                true
            } else {
                false
            }
        })
    }

    pub(in crate::ui) fn genius_settings_changed(&mut self, cx: &mut Context<Self>) {
        self.reset_genius_credits(cx);
        self.lyrics.update(cx, |view, cx| {
            view.genius = Genius::default();
            view.indices_changed(cx);
            if view.visible && (view.current_text().is_some() || view.load.is_none()) {
                view.start_genius(cx);
            }
        });
    }
}
