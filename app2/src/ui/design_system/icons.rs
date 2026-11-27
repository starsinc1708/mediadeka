//! Command icons drawn from programmatic vector geometry.
//!
//! The Lucide font cannot change shape, so the command icons that carry a state
//! are built here as unit-square polygons and painted with `paint_path`. A morph
//! only moves vertices: no image is decoded, no layout is rebuilt and no extra
//! GPU pipeline is created.
//!
//! Procedural glyphs use unit-square fills and strokes. The favorite glyph
//! paints the same SVG pair in the controls and the gallery. Interaction
//! styling never changes the control's width, height or hitbox.
//!
//! Animation state is keyed by a stable control id and held in an
//! [`IconAnimations`] global installed by [`install`]. A transition is
//! interruptible, commits immediately under reduced motion and settles exactly
//! once its duration expires, so [`active`] stops asking for frames afterwards.

use std::{
    cell::RefCell,
    f32::consts::{PI, TAU},
    time::{Duration, Instant},
};

#[cfg(feature = "capture")]
use gpui::canvas;
use gpui::{
    App, ElementId, Global, Hsla, PathBuilder, Pixels, Point, Window, point, prelude::*, px,
};

use super::motion::{self, MOTION, Spring};

mod buttons;
mod catalogue;
mod like;

#[cfg(feature = "capture")]
pub(in crate::ui) use buttons::ghost_icon_button_with_state;
pub(in crate::ui) use buttons::{
    ghost_icon_button, hero_play, hero_play_icon, secondary_action, text_action,
};

/// Gallery cells show the glyph larger than a toolbar button does.
#[cfg(feature = "capture")]
const GALLERY_GLYPH_SHARE: f32 = 0.84;
/// Main stroke width in unit-square coordinates.
const STROKE: f32 = 0.072;
/// Hairline stroke for secondary marks and small details.
const THIN_STROKE: f32 = 0.054;
/// Radius of the settled-state indicator dot.
const DOT_RADIUS: f32 = 0.032;
/// Keep strokes visible on very small controls.
const MIN_STROKE_PX: f32 = 0.75;
/// Optical stroke range in DIP. The unit-square width is scaled by the glyph
/// box, so a small control would otherwise draw a visibly thinner line than a
/// large one; the main stroke is clamped and the hairline marks follow it.
const STROKE_RANGE: (f32, f32) = (1.5, 2.0);
/// Bounded so a long-lived session cannot accumulate one entry per page.
const MAX_TRACKED_ICONS: usize = 192;
/// A target change arriving after a long idle must not fast-forward the morph
/// with the time the window spent asleep.
const MAX_INTERRUPT_DELTA: f32 = 0.05;

// Motion map. Play/Pause keeps the established ~220 ms beat; Shuffle gets the
// longer 280–400 ms window the choreography needs. Everything else reuses the
// shared motion tokens instead of carrying private literals.
const PLAY_PAUSE_MORPH: Duration = MOTION.normal();
const SHUFFLE_MORPH: Duration = Duration::from_millis(330);
const REPEAT_MORPH: Duration = Duration::from_millis(300);
const LIKE_MORPH: Duration = Duration::from_millis(240);
const QUEUE_MORPH: Duration = Duration::from_millis(280);
const MORE_MORPH: Duration = Duration::from_millis(180);
const FULLSCREEN_MORPH: Duration = MOTION.normal();
const VOLUME_MORPH: Duration = MOTION.normal();
const FILTER_MORPH: Duration = MOTION.normal();
const SORT_MORPH: Duration = MOTION.normal();
const PIN_MORPH: Duration = MOTION.normal();
const COPY_MORPH: Duration = MOTION.normal();
const PULSE_DURATION: Duration = Duration::from_millis(160);

/// A command whose geometry depends on application state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Glyph {
    /// Triangle morphing into two pause bars.
    PlayPause,
    /// Two crossing lines redraw in sequence while their arrows travel.
    Shuffle,
    /// Loop arrows nudge as the repeat mode changes.
    Repeat,
    /// [`Self::Repeat`] with the "1" of repeat-one fading in.
    RepeatOne,
    /// Heart outline receiving its fill, with a brief scale bump.
    Like,
    /// Queue lines shifting one after another.
    Queue,
    /// Three dots spreading for an open menu.
    More,
    /// Four corner brackets spreading for fullscreen.
    Fullscreen,
    /// Skip-next arrow with a short rightward nudge.
    Next,
    /// Skip-previous arrow with a short leftward nudge.
    Previous,
    /// Speaker waves that collapse into a mute slash (`target` = muted).
    Volume,
    /// Muted variant included alongside Volume in the capture gallery.
    #[cfg_attr(not(any(feature = "capture", test)), allow(dead_code))]
    Mute,
    /// Slider lines whose knobs move to a new arrangement.
    Filter,
    /// Three lines reordering themselves.
    #[cfg_attr(not(any(feature = "capture", test)), allow(dead_code))]
    Sort,
    /// Pin rotating into its settled pinned angle.
    Pin,
    /// Two copy sheets separating while a success check appears.
    Copy,
    Lyrics,
    ChevronDown,
}

impl Glyph {
    /// Every glyph, for a gallery or a completeness test.
    #[cfg(any(feature = "capture", test))]
    pub(in crate::ui) const ALL: [Glyph; 18] = [
        Glyph::PlayPause,
        Glyph::Shuffle,
        Glyph::Repeat,
        Glyph::RepeatOne,
        Glyph::Like,
        Glyph::Queue,
        Glyph::More,
        Glyph::Fullscreen,
        Glyph::Next,
        Glyph::Previous,
        Glyph::Volume,
        Glyph::Mute,
        Glyph::Filter,
        Glyph::Sort,
        Glyph::Pin,
        Glyph::Copy,
        Glyph::Lyrics,
        Glyph::ChevronDown,
    ];

    fn morph_duration(self) -> Duration {
        match self {
            Self::PlayPause => PLAY_PAUSE_MORPH,
            Self::Shuffle => SHUFFLE_MORPH,
            Self::Repeat | Self::RepeatOne => REPEAT_MORPH,
            Self::Like => LIKE_MORPH,
            Self::Queue => QUEUE_MORPH,
            Self::More => MORE_MORPH,
            Self::Fullscreen => FULLSCREEN_MORPH,
            Self::Next | Self::Previous => PULSE_DURATION,
            Self::Volume | Self::Mute => VOLUME_MORPH,
            Self::Filter => FILTER_MORPH,
            Self::Sort => SORT_MORPH,
            Self::Pin => PIN_MORPH,
            Self::Copy => COPY_MORPH,
            Self::ChevronDown => MORE_MORPH,
            Self::Lyrics => QUEUE_MORPH,
        }
    }

    /// Whether an active target should tint the glyph with the accent.
    fn target_is_emphasis(self) -> bool {
        matches!(
            self,
            Self::Shuffle
                | Self::Repeat
                | Self::RepeatOne
                | Self::Like
                | Self::Queue
                | Self::More
                | Self::Fullscreen
                | Self::Filter
                | Self::Sort
                | Self::Pin
                | Self::Copy
                | Self::Lyrics
        )
    }

    /// Whether the glyph gains the small settled-state dot. Glyphs whose
    /// geometry already changes (heart fill, pin rotation, corner spread,
    /// check mark) do not need it.
    fn wants_indicator(self) -> bool {
        matches!(
            self,
            Self::Shuffle | Self::Repeat | Self::RepeatOne | Self::Queue | Self::Lyrics
        )
    }

    /// One-shot skip commands animate through [`pulse`] instead of a
    /// persistent target.
    fn is_impulse(self) -> bool {
        matches!(self, Self::Next | Self::Previous)
    }

    /// ARIA toggled semantics are only honest for real toggle commands.
    fn announces_toggle(self) -> bool {
        matches!(
            self,
            Self::PlayPause
                | Self::Shuffle
                | Self::Repeat
                | Self::RepeatOne
                | Self::Like
                | Self::Queue
                | Self::Lyrics
                | Self::Fullscreen
                | Self::Volume
                | Self::Mute
                | Self::Filter
                | Self::Sort
                | Self::Pin
        )
    }
}

/// Visual semantics that do not follow from the glyph geometry alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(in crate::ui) enum GlyphState {
    /// Ordinary resting control.
    #[default]
    Rest,
    /// Selected, enabled or toggled on. Adds the accent emphasis and, for the
    /// glyphs that have one, the settled-state indicator.
    #[cfg_attr(not(feature = "capture"), allow(dead_code))]
    Active,
    /// Dimmed, out of the tab order and described as unavailable.
    Disabled,
    /// Kept visible with an activity ring; callers own click gating.
    Loading,
}

impl GlyphState {
    fn is_enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    fn is_loading(self) -> bool {
        matches!(self, Self::Loading)
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// A glyph placed at one point of its animation, ready to paint from any
/// canvas. `progress` is the settled 0..1 state; `pulse` is the transient
/// impulse used by one-shot glyphs such as skip-next.
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct AnimatedIcon {
    glyph: Glyph,
    progress: f32,
    pulse: f32,
    active: bool,
    repeat_one: Option<f32>,
}

impl AnimatedIcon {
    pub(in crate::ui) fn at(glyph: Glyph, progress: f32) -> Self {
        Self {
            glyph,
            progress: progress.clamp(0., 1.),
            pulse: 0.,
            active: false,
            repeat_one: None,
        }
    }

    pub(in crate::ui) fn with_pulse(mut self, pulse: f32) -> Self {
        self.pulse = pulse.clamp(0., 1.);
        self
    }

    pub(in crate::ui) fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub(in crate::ui) fn paint(
        self,
        window: &mut Window,
        origin: Point<Pixels>,
        side: Pixels,
        color: Hsla,
        cx: &App,
    ) {
        if self.glyph == Glyph::Like {
            like::paint(window, origin, side, self.progress, color, cx);
            return;
        }
        let glyph_shapes = if let Some(one) = self.repeat_one {
            repeat_shapes(self.progress, self.active, Some(one))
        } else {
            shapes(self.glyph, self.progress, self.pulse, self.active)
        };
        for shape in glyph_shapes {
            match shape {
                Shape::Fill(points, alpha) if alpha > 0.001 => {
                    paint_polygon(window, &points, origin, side, tinted(color, alpha));
                }
                Shape::Stroke(points, width, alpha) if alpha > 0.001 => {
                    paint_stroke(window, &points, width, origin, side, tinted(color, alpha));
                }
                _ => {}
            }
        }
    }
}

/// Gallery hook: paint any glyph at an explicit progress with no global state.
/// A gallery cell can call this from its own canvas, or use [`gallery_icon`].
#[cfg(feature = "capture")]
pub(in crate::ui) fn paint_glyph_for_gallery(
    window: &mut Window,
    glyph: Glyph,
    origin: Point<Pixels>,
    side: Pixels,
    progress: f32,
    color: impl Into<Hsla>,
    cx: &App,
) {
    AnimatedIcon::at(glyph, progress).paint(window, origin, side, color.into(), cx);
}

/// Gallery hook that also resolves enabled/active/disabled/loading styling.
#[cfg(feature = "capture")]
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn paint_glyph_state_for_gallery(
    window: &mut Window,
    glyph: Glyph,
    origin: Point<Pixels>,
    side: Pixels,
    progress: f32,
    state: GlyphState,
    accent: impl Into<Hsla>,
    color: impl Into<Hsla>,
    cx: &App,
) {
    let accent = accent.into();
    let color = color.into();
    let active = state.is_active();
    let paint = match state {
        GlyphState::Disabled => tinted(color, 0.55),
        GlyphState::Active => accent,
        GlyphState::Rest | GlyphState::Loading => color,
    };
    if state.is_loading() {
        for shape in loading_shapes() {
            match shape {
                Shape::Fill(points, alpha) => {
                    paint_polygon(window, &points, origin, side, tinted(paint, alpha));
                }
                Shape::Stroke(points, width, alpha) => {
                    paint_stroke(window, &points, width, origin, side, tinted(paint, alpha));
                }
            }
        }
        return;
    }
    if state == GlyphState::Rest {
        paint_glyph_for_gallery(window, glyph, origin, side, progress, color, cx);
        return;
    }
    AnimatedIcon::at(glyph, progress)
        .with_active(active)
        .paint(window, origin, side, paint, cx);
}

/// A self-contained gallery cell for one glyph at a given progress.
#[cfg(feature = "capture")]
pub(in crate::ui) fn gallery_icon(
    glyph: Glyph,
    side: f32,
    progress: f32,
    color: impl Into<Hsla>,
) -> impl gpui::IntoElement {
    let side = side.max(1.);
    let color = color.into();
    canvas(
        |_, _, _| (),
        move |bounds, (), window, cx| {
            let glyph_side = bounds.size.width.min(bounds.size.height) * GALLERY_GLYPH_SHARE;
            let origin = point(
                bounds.origin.x + (bounds.size.width - glyph_side) * 0.5,
                bounds.origin.y + (bounds.size.height - glyph_side) * 0.5,
            );
            paint_glyph_state_for_gallery(
                window,
                glyph,
                origin,
                glyph_side,
                progress,
                GlyphState::Rest,
                color,
                color,
                cx,
            );
        },
    )
    .size(px(side))
}

/// Per-control animation state. Progress advances on the frame that asks for
/// it, so a finished morph costs nothing and no timer stays alive.
#[derive(Default)]
pub(in crate::ui) struct IconAnimations {
    states: RefCell<Vec<Entry>>,
}

struct Entry {
    id: ElementId,
    spring: Spring,
    last: Option<Instant>,
    target: bool,
    /// Seconds since the target last changed, capped by `duration`.
    elapsed: f32,
    duration: f32,
    pulse_started: Option<Instant>,
}

/// The progress and transient impulse of one glyph in one frame.
#[derive(Clone, Copy, Debug, Default)]
struct GlyphFrame {
    progress: f32,
    pulse: f32,
    moving: bool,
}

impl GlyphFrame {
    fn settled(target: bool) -> Self {
        Self {
            progress: if target { 1. } else { 0. },
            pulse: 0.,
            moving: false,
        }
    }
}

impl IconAnimations {
    /// Progress of the control `id` towards `target` (0 = first state,
    /// 1 = second state). Interrupting a morph keeps its velocity and its
    /// position; a target that is already settled costs nothing.
    fn frame_at(
        &self,
        id: impl Into<ElementId>,
        target: bool,
        glyph: Glyph,
        now: Instant,
        reduced: bool,
    ) -> GlyphFrame {
        let mut states = self.states.borrow_mut();
        let index = Self::entry_index(&mut states, id.into(), target);
        let entry = &mut states[index];
        entry.duration = glyph.morph_duration().as_secs_f32();

        let delta = entry
            .last
            .replace(now)
            .map_or(0., |last| now.saturating_duration_since(last).as_secs_f32());
        let changed = entry.target != target;
        entry.target = target;
        entry.spring.set_target(if target { 1. } else { 0. });

        if reduced {
            entry.spring.snap();
            entry.elapsed = entry.duration;
            entry.pulse_started = None;
            return GlyphFrame::settled(target);
        }

        if changed {
            entry.elapsed = 0.;
        }
        // A target toggled after the window was idle must start from the
        // current shape, not advance by the whole idle interval at once.
        if (!changed || delta <= MAX_INTERRUPT_DELTA) && delta > 0. {
            entry.elapsed += delta;
            entry.spring.advance(delta);
        }
        if entry.elapsed >= entry.duration {
            entry.spring.snap();
        }

        let progress = entry.spring.value().clamp(0., 1.);
        let pulse = entry.pulse_at(now);
        GlyphFrame {
            progress,
            pulse,
            moving: !entry.spring.settled() || entry.pulse_started.is_some(),
        }
    }

    fn progress_at(&self, id: &'static str, target: bool, now: Instant, reduced: bool) -> f32 {
        self.frame_at(id, target, Glyph::PlayPause, now, reduced)
            .progress
    }

    /// Begin a one-shot impulse. Used by glyphs that have no persistent state
    /// (skip-next, skip-previous); the impulse decays to zero on its own.
    fn start_pulse(&self, id: &'static str, now: Instant) {
        let mut states = self.states.borrow_mut();
        let index = Self::entry_index(&mut states, id.into(), false);
        states[index].pulse_started = Some(now);
    }

    fn active(&self, now: Instant) -> bool {
        let mut states = self.states.borrow_mut();
        let mut active = false;
        for entry in states.iter_mut() {
            if let Some(start) = entry.pulse_started {
                if now.saturating_duration_since(start) < PULSE_DURATION {
                    active = true;
                } else {
                    entry.pulse_started = None;
                }
            }
            if entry.spring.settled() {
                continue;
            }
            // A control can disappear mid-morph (navigation, a closed menu).
            // Advance what is left by wall time so an abandoned spring settles
            // and stops the frame pump instead of running forever.
            if let Some(last) = entry.last {
                let delta = now.saturating_duration_since(last).as_secs_f32();
                if delta > 0. {
                    entry.last = Some(now);
                    entry.elapsed += delta;
                    entry.spring.advance(delta);
                    if entry.duration > 0. && entry.elapsed >= entry.duration {
                        entry.spring.snap();
                    }
                }
            }
            if !entry.spring.settled() {
                active = true;
            }
        }
        active
    }

    fn entry_index(states: &mut Vec<Entry>, id: ElementId, target: bool) -> usize {
        if let Some(index) = states.iter().position(|entry| entry.id == id) {
            return index;
        }
        if states.len() >= MAX_TRACKED_ICONS {
            let evict = states
                .iter()
                .position(|entry| entry.spring.settled() && entry.pulse_started.is_none())
                .unwrap_or(0);
            states.remove(evict);
        }
        states.push(Entry {
            id,
            spring: Spring::at(if target { 1. } else { 0. }),
            last: None,
            target,
            elapsed: 0.,
            duration: 0.,
            pulse_started: None,
        });
        states.len() - 1
    }
}

impl Entry {
    fn pulse_at(&mut self, now: Instant) -> f32 {
        let Some(start) = self.pulse_started else {
            return 0.;
        };
        let progress =
            now.saturating_duration_since(start).as_secs_f32() / PULSE_DURATION.as_secs_f32();
        if progress >= 1. {
            self.pulse_started = None;
            return 0.;
        }
        (PI * motion::smoothstep(progress)).sin().max(0.)
    }
}

impl Global for IconAnimations {}

pub(in crate::ui) fn install(cx: &mut App) {
    cx.set_global(IconAnimations::default());
    cx.set_global(guise::IconRenderer(catalogue::render));
}

pub(in crate::ui) fn forget_motion(cx: &App, id: &'static str) {
    if let Some(icons) = cx.try_global::<IconAnimations>() {
        let id = ElementId::from(id);
        icons.states.borrow_mut().retain(|state| state.id != id);
    }
}

/// Current morph progress of a control. Without the global (tests, previews)
/// the icon simply shows its target state. Custom canvases can use this when
/// they cannot call the button constructors.
#[allow(dead_code)] // Public canvas hook alongside `paint_glyph_for_gallery`.
pub(in crate::ui) fn progress(cx: &App, id: &'static str, target: bool) -> f32 {
    cx.try_global::<IconAnimations>().map_or_else(
        || f32::from(u8::from(target)),
        |icons| icons.progress_at(id, target, Instant::now(), cx.reduce_motion()),
    )
}

/// Kick a one-shot impulse for a glyph without a persistent target, such as
/// skip-next. Requests the first frame itself; subsequent frames are driven by
/// [`active`] and stop as soon as the impulse decays.
pub(in crate::ui) fn pulse(window: &mut Window, cx: &App, id: &'static str) {
    if cx.reduce_motion() || crate::ui::theme::fluid_high_contrast(cx) {
        return;
    }
    if let Some(icons) = cx.try_global::<IconAnimations>() {
        icons.start_pulse(id, Instant::now());
        window.refresh();
    }
}

/// Whether any icon still needs a frame. Owners ask this after building the
/// content and request one more animation frame if it is true.
pub(in crate::ui) fn active(cx: &App) -> bool {
    cx.try_global::<IconAnimations>()
        .is_some_and(|icons| icons.active(Instant::now()))
}

fn frame(cx: &App, id: impl Into<ElementId>, target: bool, glyph: Glyph) -> GlyphFrame {
    cx.try_global::<IconAnimations>().map_or_else(
        || GlyphFrame::settled(target),
        |icons| {
            icons.frame_at(
                id,
                target,
                glyph,
                Instant::now(),
                cx.reduce_motion() || crate::ui::theme::fluid_high_contrast(cx),
            )
        },
    )
}

#[derive(IntoElement)]
pub(in crate::ui) struct AnimatedGlyph {
    glyph: Glyph,
    frame: GlyphFrame,
    side: Pixels,
    ink: Option<Hsla>,
    active: bool,
    loading: bool,
    repeat_one: Option<GlyphFrame>,
}

impl AnimatedGlyph {
    fn new(glyph: Glyph, frame: GlyphFrame, side: f32, ink: Option<Hsla>) -> Self {
        Self {
            glyph,
            frame,
            side: px(side),
            ink,
            active: false,
            loading: false,
            repeat_one: None,
        }
    }
}

impl RenderOnce for AnimatedGlyph {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        if self.frame.moving || self.repeat_one.is_some_and(|one| one.moving) {
            window.request_animation_frame();
        }
        let ink = self.ink.unwrap_or_else(|| window.text_style().color);
        if self.loading {
            catalogue::icon_canvas(self.side, ink, |window, origin, side, ink, _| {
                paint_loading(window, origin, side, ink)
            })
            .into_any_element()
        } else {
            let phase = self
                .repeat_one
                .filter(|one| !self.frame.moving && one.moving)
                .map_or(self.frame.progress, |one| one.progress);
            let mut icon = AnimatedIcon::at(self.glyph, phase)
                .with_pulse(self.frame.pulse)
                .with_active(self.active);
            icon.repeat_one = self.repeat_one.map(|one| one.progress);
            catalogue::canvas_icon(icon, self.side, ink).into_any_element()
        }
    }
}

pub(in crate::ui) fn animated_glyph(
    id: impl Into<ElementId>,
    glyph: Glyph,
    target: bool,
    side: f32,
    ink: impl Into<Option<Hsla>>,
    cx: &App,
) -> AnimatedGlyph {
    AnimatedGlyph::new(glyph, frame(cx, id, target, glyph), side, ink.into())
}

pub(in crate::ui) fn settled_like(active: bool, side: f32) -> AnimatedGlyph {
    AnimatedGlyph::new(Glyph::Like, GlyphFrame::settled(active), side, None)
}

pub(in crate::ui) fn repeat_glyph(
    id: &'static str,
    enabled: bool,
    one: bool,
    side: f32,
    cx: &App,
) -> AnimatedGlyph {
    let mut icon = animated_glyph(id, Glyph::Repeat, enabled, side, None, cx);
    icon.repeat_one = Some(frame(cx, (id, 1usize), one, Glyph::RepeatOne));
    icon
}

fn tinted(color: Hsla, alpha: f32) -> Hsla {
    color.opacity(color.a * alpha)
}

fn paint_polygon(
    window: &mut Window,
    points: &[[f32; 2]],
    origin: Point<Pixels>,
    side: Pixels,
    color: Hsla,
) {
    if points.len() < 3 {
        return;
    }
    let mut builder = PathBuilder::fill();
    for (index, point) in points.iter().enumerate() {
        let vertex = vertex(origin, side, *point);
        if index == 0 {
            builder.move_to(vertex);
        } else {
            builder.line_to(vertex);
        }
    }
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_stroke(
    window: &mut Window,
    points: &[[f32; 2]],
    width: f32,
    origin: Point<Pixels>,
    side: Pixels,
    color: Hsla,
) {
    if points.len() < 2 {
        return;
    }
    // Scale the hairline marks with the clamped main stroke so the hierarchy
    // between a command outline and its detail survives on small controls.
    let main = (f32::from(side) * STROKE).clamp(STROKE_RANGE.0, STROKE_RANGE.1);
    let pixels = (main * width / STROKE).max(MIN_STROKE_PX);
    let mut builder = PathBuilder::stroke(px(pixels));
    for (index, point) in points.iter().enumerate() {
        let vertex = vertex(origin, side, *point);
        if index == 0 {
            builder.move_to(vertex);
        } else {
            builder.line_to(vertex);
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_loading(window: &mut Window, origin: Point<Pixels>, side: Pixels, color: Hsla) {
    for shape in loading_shapes() {
        match shape {
            Shape::Fill(points, alpha) => {
                paint_polygon(window, &points, origin, side, tinted(color, alpha));
            }
            Shape::Stroke(points, width, alpha) => {
                paint_stroke(window, &points, width, origin, side, tinted(color, alpha));
            }
        }
    }
}

fn vertex(origin: Point<Pixels>, side: Pixels, [x, y]: [f32; 2]) -> Point<Pixels> {
    point(origin.x + side * x, origin.y + side * y)
}

fn loading_shapes() -> Vec<Shape> {
    let center = [0.5, 0.5];
    let start = -PI * 0.5;
    let end = PI;
    let radius = 0.40;
    let tip = [
        center[0] + radius * end.cos(),
        center[1] + radius * end.sin(),
    ];
    vec![
        Shape::Stroke(arc_points(center, radius, start, end, 28), THIN_STROKE, 1.),
        Shape::Fill(circle_points(tip, 0.052, 12), 1.),
    ]
}

fn shapes(glyph: Glyph, progress: f32, pulse: f32, active: bool) -> Vec<Shape> {
    let progress = progress.clamp(0., 1.);
    let pulse = pulse.clamp(0., 1.);
    match glyph {
        Glyph::PlayPause => play_pause_shapes(progress),
        Glyph::Shuffle => shuffle_shapes(progress, active),
        Glyph::Repeat => repeat_shapes(progress, active, None),
        Glyph::RepeatOne => repeat_shapes(progress, active, Some(progress)),
        Glyph::Like => unreachable!("the favorite glyph is an SVG"),
        Glyph::Queue => queue_shapes(progress, active),
        Glyph::More => more_shapes(progress),
        Glyph::Fullscreen => fullscreen_shapes(progress),
        Glyph::Next => skip_shapes(1., pulse),
        Glyph::Previous => skip_shapes(-1., pulse),
        Glyph::Volume | Glyph::Mute => volume_shapes(progress),
        Glyph::Filter => filter_shapes(progress),
        Glyph::Sort => sort_shapes(progress),
        Glyph::Pin => pin_shapes(progress),
        Glyph::Copy => copy_shapes(progress),
        Glyph::Lyrics => {
            let offset = 0.045 * (PI * progress).sin();
            let mut shapes = vec![
                closed_stroke(
                    &[[0.16, 0.24], [0.84, 0.24], [0.84, 0.74], [0.16, 0.74]],
                    STROKE,
                    1.,
                ),
                line(
                    [0.30 + offset, 0.45],
                    [0.46 + offset, 0.45],
                    THIN_STROKE,
                    1.,
                ),
                line([0.58, 0.45], [0.70, 0.45], THIN_STROKE, 1.),
                line([0.30, 0.58], [0.70 - offset, 0.58], THIN_STROKE, 1.),
            ];
            shapes.extend(indicator_dot(active));
            shapes
        }
        Glyph::ChevronDown => vec![stroke_path(
            &rotate_around(
                &[[0.26, 0.40], [0.50, 0.62], [0.74, 0.40]],
                [0.5, 0.5],
                PI * progress,
            ),
            STROKE,
            1.,
        )],
    }
}

// --- Shapes -----------------------------------------------------------------

/// One paint primitive in unit-square coordinates. Fills are polygons; strokes
/// are open polylines whose width is a share of the glyph box.
#[derive(Clone, Debug, PartialEq)]
enum Shape {
    Fill(Vec<[f32; 2]>, f32),
    Stroke(Vec<[f32; 2]>, f32, f32),
}

impl Shape {
    fn fill(points: Vec<[f32; 2]>, alpha: f32) -> Self {
        Self::Fill(points, alpha)
    }

    fn stroke(points: Vec<[f32; 2]>, width: f32, alpha: f32) -> Self {
        Self::Stroke(points, width, alpha)
    }

    #[cfg(test)]
    fn points(&self) -> &[[f32; 2]] {
        match self {
            Self::Fill(points, _) | Self::Stroke(points, _, _) => points,
        }
    }

    #[cfg(test)]
    fn alpha(&self) -> f32 {
        match self {
            Self::Fill(_, alpha) | Self::Stroke(_, _, alpha) => *alpha,
        }
    }
}

fn line(from: [f32; 2], to: [f32; 2], width: f32, alpha: f32) -> Shape {
    Shape::stroke(vec![from, to], width, alpha)
}

fn stroke_path(points: &[[f32; 2]], width: f32, alpha: f32) -> Shape {
    Shape::stroke(points.to_vec(), width, alpha)
}

fn closed_stroke(points: &[[f32; 2]], width: f32, alpha: f32) -> Shape {
    let mut closed = points.to_vec();
    if let Some(first) = closed.first().copied() {
        closed.push(first);
    }
    Shape::stroke(closed, width, alpha)
}

fn circle_shape(center: [f32; 2], radius: f32, alpha: f32) -> Shape {
    Shape::fill(circle_points(center, radius, 20), alpha)
}

fn circle_points(center: [f32; 2], radius: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..steps)
        .map(|index| {
            let angle = index as f32 / steps as f32 * TAU;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

fn arc_points(center: [f32; 2], radius: f32, start: f32, end: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|index| {
            let angle = start + (end - start) * index as f32 / steps as f32;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

fn arrow_head(tip: [f32; 2], direction: f32, length: f32, alpha: f32) -> Shape {
    let [x, y] = tip;
    let back = x - direction * length;
    stroke_path(&[[back, y - 0.11], tip, [back, y + 0.11]], STROKE, alpha)
}

fn mix(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

fn mix_point(from: [f32; 2], to: [f32; 2], progress: f32) -> [f32; 2] {
    [mix(from[0], to[0], progress), mix(from[1], to[1], progress)]
}

fn shift(points: &[[f32; 2]], dx: f32, dy: f32) -> Vec<[f32; 2]> {
    points.iter().map(|[x, y]| [x + dx, y + dy]).collect()
}

fn rotate_around(points: &[[f32; 2]], center: [f32; 2], radians: f32) -> Vec<[f32; 2]> {
    let (sin, cos) = radians.sin_cos();
    points
        .iter()
        .map(|[x, y]| {
            let dx = x - center[0];
            let dy = y - center[1];
            [
                center[0] + dx * cos - dy * sin,
                center[1] + dx * sin + dy * cos,
            ]
        })
        .collect()
}

/// A 0→1→0 window over a slice of the main progress. At both ends the value is
/// zero, so a callback interruption simply reverses the choreography.
fn window(progress: f32, start: f32, end: f32) -> f32 {
    let t = stage(progress, start, end, motion::smoothstep);
    (PI * t).sin().max(0.)
}

fn stage(progress: f32, start: f32, end: f32, curve: fn(f32) -> f32) -> f32 {
    if end <= start {
        return f32::from(u8::from(progress >= end));
    }
    curve(((progress - start) / (end - start)).clamp(0., 1.))
}

fn indicator_dot(active: bool) -> Option<Shape> {
    active.then(|| circle_shape([0.5, 0.93], DOT_RADIUS, 0.9))
}

// --- Glyphs -----------------------------------------------------------------

fn play_pause_shapes(progress: f32) -> Vec<Shape> {
    let play_left = lerp(PLAY_LEFT, PAUSE_LEFT, progress);
    let play_right = lerp(PLAY_RIGHT, PAUSE_RIGHT, progress);
    let [x, y] = polygon_centroid(&[play_left, play_right]);
    vec![
        Shape::fill(shift(&play_left, 0.5 - x, 0.5 - y), 1.),
        Shape::fill(shift(&play_right, 0.5 - x, 0.5 - y), 1.),
    ]
}

fn polygon_centroid(polygons: &[[[f32; 2]; 4]; 2]) -> [f32; 2] {
    let (mut area, mut x, mut y) = (0., 0., 0.);
    for polygon in polygons {
        for (index, &[x1, y1]) in polygon.iter().enumerate() {
            let [x2, y2] = polygon[(index + 1) % polygon.len()];
            let cross = x1 * y2 - x2 * y1;
            area += cross;
            x += (x1 + x2) * cross;
            y += (y1 + y2) * cross;
        }
    }
    [x / (3. * area), y / (3. * area)]
}

fn lerp(from: [[f32; 2]; 4], to: [[f32; 2]; 4], progress: f32) -> [[f32; 2]; 4] {
    std::array::from_fn(|index| mix_point(from[index], to[index], progress))
}

fn shuffle_shapes(progress: f32, active: bool) -> Vec<Shape> {
    // Two crossing lines, redrawn one after the other while their arrowheads
    // travel in the direction of playback.
    let first = window(progress, 0.0, 0.60);
    let second = window(progress, 0.22, 0.84);
    let mut shapes = Vec::with_capacity(5);

    let paths = [
        (
            [[0.14, 0.28], [0.34, 0.28], [0.66, 0.72], [0.82, 0.72]],
            0.72,
            first,
        ),
        (
            [[0.14, 0.72], [0.34, 0.72], [0.66, 0.28], [0.82, 0.28]],
            0.28,
            second,
        ),
    ];
    for (path, tip_y, bump) in paths {
        let slide = 0.035 * bump;
        let alpha = 1. - 0.6 * bump;
        shapes.push(stroke_path(&shift(&path, slide, 0.), 0.080, alpha));
        shapes.push(arrow_head([0.82 + slide, tip_y], 1., 0.12, alpha));
    }
    shapes.extend(indicator_dot(active));
    shapes
}

fn repeat_shapes(progress: f32, active: bool, one: Option<f32>) -> Vec<Shape> {
    let bump = (PI * motion::smoothstep(progress)).sin().max(0.);
    let travel = 0.04 * bump;
    let upper = 0.84 + travel;
    let lower = 0.16 - travel;
    let mut shapes = vec![
        stroke_path(
            &[[0.16, 0.48], [0.16, 0.37], [0.21, 0.31], [upper, 0.31]],
            STROKE,
            1.,
        ),
        arrow_head([upper, 0.31], 1., 0.12, 1.),
        stroke_path(
            &[[0.84, 0.48], [0.84, 0.62], [0.79, 0.68], [lower, 0.68]],
            STROKE,
            1.,
        ),
        arrow_head([lower, 0.68], -1., 0.12, 1.),
    ];
    if let Some(one) = one {
        let alpha = stage(one, 0.25, 1.0, motion::smoothstep);
        shapes.push(line([0.53, 0.42], [0.53, 0.60], THIN_STROKE, alpha));
        shapes.push(line([0.47, 0.46], [0.53, 0.42], THIN_STROKE, alpha));
        shapes.push(line([0.45, 0.60], [0.61, 0.60], THIN_STROKE, alpha));
    }
    shapes.extend(indicator_dot(active));
    shapes
}

fn queue_shapes(progress: f32, active: bool) -> Vec<Shape> {
    let rows = [(0.30, 0.13, 0.66), (0.49, 0.13, 0.60), (0.68, 0.13, 0.47)];
    let mut shapes = Vec::with_capacity(4);
    for (index, (y, from, to)) in rows.into_iter().enumerate() {
        let bump = window(progress, index as f32 * 0.15, 0.55 + index as f32 * 0.15);
        let slide = 0.05 * bump;
        let alpha = 1. - 0.3 * bump;
        shapes.push(line([from + slide, y], [to + slide, y], STROKE, alpha));
    }
    shapes.push(line([0.83, 0.33], [0.83, 0.65], STROKE, 1.));
    shapes.push(closed_stroke(
        &circle_points([0.73, 0.65], 0.10, 18),
        THIN_STROKE,
        1.,
    ));
    shapes.extend(indicator_dot(active));
    shapes
}

fn more_shapes(progress: f32) -> Vec<Shape> {
    // The outer dots spread first and last; the middle one stays put.
    let left = stage(progress, 0.0, 0.65, motion::out_cubic);
    let right = stage(progress, 0.25, 1.0, motion::out_cubic);
    let dots = [
        ([0.28 - 0.055 * left, 0.50], 0.065),
        ([0.50, 0.50], 0.065),
        ([0.72 + 0.055 * right, 0.50], 0.065),
    ];
    dots.into_iter()
        .map(|(center, radius)| circle_shape(center, radius, 1.))
        .collect()
}

fn fullscreen_shapes(progress: f32) -> Vec<Shape> {
    let top = stage(progress, 0.0, 0.7, motion::out_cubic);
    let bottom = stage(progress, 0.3, 1.0, motion::out_cubic);
    let mut shapes = Vec::with_capacity(4);
    for (index, corner) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let t = if index < 2 { top } else { bottom };
        let inset = mix(0.22, 0.12, t);
        let arm = mix(0.15, 0.21, t);
        let horizontal = if corner.0 == 0. { 1. } else { -1. };
        let vertical = if corner.1 == 0. { 1. } else { -1. };
        let x = if corner.0 == 0. { inset } else { 1. - inset };
        let y = if corner.1 == 0. { inset } else { 1. - inset };
        shapes.push(stroke_path(
            &[[x, y + vertical * arm], [x, y], [x + horizontal * arm, y]],
            STROKE,
            1.,
        ));
    }
    shapes
}

fn skip_shapes(direction: f32, pulse: f32) -> Vec<Shape> {
    let travel = 0.105 * pulse;
    let dx = direction * travel;
    let (triangle, bar) = if direction > 0. {
        (
            vec![[0.19, 0.19], [0.19, 0.81], [0.68, 0.50]],
            vec![[0.72, 0.19], [0.81, 0.19], [0.81, 0.81], [0.72, 0.81]],
        )
    } else {
        (
            vec![[0.81, 0.19], [0.81, 0.81], [0.32, 0.50]],
            vec![[0.28, 0.19], [0.19, 0.19], [0.19, 0.81], [0.28, 0.81]],
        )
    };
    vec![
        Shape::fill(shift(&triangle, dx, 0.), 1.),
        Shape::fill(shift(&bar, dx, 0.), 1.),
    ]
}

fn volume_shapes(progress: f32) -> Vec<Shape> {
    let speaker = vec![
        [0.12, 0.40],
        [0.28, 0.40],
        [0.46, 0.26],
        [0.46, 0.74],
        [0.28, 0.60],
        [0.12, 0.60],
    ];
    let mut shapes = vec![closed_stroke(&speaker, STROKE, 1.)];
    let wave_alpha = 1. - progress;
    if wave_alpha > 0.001 {
        for radius in [0.16, 0.28] {
            let radius = radius * (1. - 0.45 * progress);
            shapes.push(stroke_path(
                &arc_points([0.46, 0.50], radius, -1.0, 1.0, 12),
                THIN_STROKE,
                wave_alpha,
            ));
        }
    }
    let slash = stage(progress, 0.2, 1.0, motion::smoothstep);
    if slash > 0.001 {
        shapes.push(line([0.72, 0.24], [0.34, 0.76], STROKE, slash));
    }
    shapes
}

fn filter_shapes(progress: f32) -> Vec<Shape> {
    let rows = [(0.28, 0.36, 0.56), (0.50, 0.62, 0.38), (0.72, 0.46, 0.68)];
    let mut shapes = Vec::with_capacity(6);
    for (index, (y, rest, active)) in rows.into_iter().enumerate() {
        shapes.push(line([0.14, y], [0.86, y], THIN_STROKE, 1.));
        let t = stage(
            progress,
            index as f32 * 0.15,
            0.7 + index as f32 * 0.15,
            motion::out_cubic,
        );
        shapes.push(circle_shape([mix(rest, active, t), y], 0.06, 1.));
    }
    shapes
}

fn sort_shapes(progress: f32) -> Vec<Shape> {
    // Descending and ascending orders swap places; the middle line anchors the
    // composition while the others travel one after the other.
    let top = stage(progress, 0.0, 0.7, motion::out_cubic);
    let bottom = stage(progress, 0.3, 1.0, motion::out_cubic);
    vec![
        line([0.16, 0.28], [mix(0.80, 0.50, top), 0.28], STROKE, 1.),
        line([0.16, 0.50], [0.64, 0.50], STROKE, 1.),
        line([0.16, 0.72], [mix(0.50, 0.80, bottom), 0.72], STROKE, 1.),
    ]
}

fn pin_shapes(progress: f32) -> Vec<Shape> {
    let head = [
        [0.32, 0.20],
        [0.68, 0.20],
        [0.63, 0.48],
        [0.75, 0.60],
        [0.75, 0.66],
        [0.25, 0.66],
        [0.25, 0.60],
        [0.37, 0.48],
    ];
    let tip = [[0.50, 0.66], [0.50, 0.88]];
    let angle = 0.57 * (1. - progress);
    vec![
        closed_stroke(&rotate_around(&head, [0.5, 0.5], angle), STROKE, 1.),
        stroke_path(&rotate_around(&tip, [0.5, 0.5], angle), STROKE, 1.),
    ]
}

fn copy_shapes(progress: f32) -> Vec<Shape> {
    let shift_amount = 0.022 * progress;
    let back = shift(
        &[[0.18, 0.18], [0.58, 0.18], [0.58, 0.58], [0.18, 0.58]],
        -shift_amount,
        -shift_amount,
    );
    let front = shift(
        &[[0.38, 0.38], [0.82, 0.38], [0.82, 0.82], [0.38, 0.82]],
        shift_amount,
        shift_amount,
    );
    let check = stage(progress, 0.35, 1.0, motion::smoothstep);
    let mut shapes = vec![
        closed_stroke(&back, THIN_STROKE, 1.),
        closed_stroke(&front, THIN_STROKE, 1.),
    ];
    if check > 0.001 {
        shapes.push(stroke_path(
            &[[0.50, 0.60], [0.58, 0.68], [0.72, 0.50]],
            0.07,
            check,
        ));
    }
    shapes
}

// Vertex order stays clockwise in both states, so a plain lerp keeps the
// polygons convex all the way through the morph.
const fn unit_quad(mut points: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let mut index = 0;
    while index < 4 {
        points[index][0] /= 24.;
        points[index][1] /= 24.;
        index += 1;
    }
    points
}

const PLAY_LEFT: [[f32; 2]; 4] = unit_quad([[7.5, 4.5], [12., 7.], [12., 17.], [7.5, 19.5]]);
const PAUSE_LEFT: [[f32; 2]; 4] = unit_quad([[6., 4.5], [10.5, 4.5], [10.5, 19.5], [6., 19.5]]);
const PLAY_RIGHT: [[f32; 2]; 4] = unit_quad([[12., 7.], [21., 12.], [12., 17.], [12., 17.]]);
const PAUSE_RIGHT: [[f32; 2]; 4] = unit_quad([[13.5, 4.5], [18., 4.5], [18., 19.5], [13.5, 19.5]]);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn procedural_glyphs() -> impl Iterator<Item = Glyph> {
        Glyph::ALL.into_iter().filter(|glyph| *glyph != Glyph::Like)
    }

    fn area(points: &[[f32; 2]]) -> f32 {
        let mut sum = 0.;
        for index in 0..points.len() {
            let [x1, y1] = points[index];
            let [x2, y2] = points[(index + 1) % points.len()];
            sum += x1 * y2 - x2 * y1;
        }
        (sum * 0.5).abs()
    }

    fn assert_inside(glyph: Glyph, progress: f32, pulse: f32, active: bool) {
        let shapes = shapes(glyph, progress, pulse, active);
        assert!(!shapes.is_empty(), "{glyph:?} has no geometry");
        for shape in &shapes {
            for [x, y] in shape.points() {
                assert!(
                    x.is_finite() && y.is_finite(),
                    "{glyph:?} produced a non-finite point at {progress}"
                );
                assert!(
                    (-0.001..=1.001).contains(x) && (-0.001..=1.001).contains(y),
                    "{glyph:?} left its box at {progress}: [{x}, {y}]"
                );
            }
            match shape {
                Shape::Fill(points, _) => {
                    assert!(
                        points.len() >= 3,
                        "{glyph:?} fill has fewer than three vertices"
                    );
                    assert!(
                        area(points) > 0.0001,
                        "{glyph:?} produced a degenerate fill at {progress}"
                    );
                }
                Shape::Stroke(points, width, _) => {
                    assert!(points.len() >= 2, "{glyph:?} stroke has no segment");
                    assert!(
                        *width > 0. && *width < 0.2,
                        "{glyph:?} has an invalid stroke width {width}"
                    );
                }
            }
            assert!(
                (0. ..=1.).contains(&shape.alpha()),
                "{glyph:?} alpha escaped 0..1 at {progress}"
            );
        }
    }

    #[test]
    fn procedural_glyphs_paint_inside_their_boxes_at_every_phase() {
        for glyph in procedural_glyphs() {
            for step in 0..=24 {
                let progress = step as f32 / 24.;
                for pulse in [0., 0.5, 1.] {
                    for active in [false, true] {
                        assert_inside(glyph, progress, pulse, active);
                    }
                }
            }
        }
    }

    #[test]
    fn procedural_glyphs_have_geometry_at_both_resting_states() {
        for glyph in procedural_glyphs() {
            for progress in [0., 1.] {
                assert_inside(glyph, progress, 0., false);
            }
        }
    }

    #[test]
    fn loading_ring_stays_inside_the_glyph_box() {
        for shape in loading_shapes() {
            for [x, y] in shape.points() {
                assert!((0. ..=1.).contains(x) && (0. ..=1.).contains(y));
            }
            if let Shape::Fill(points, _) = shape {
                assert!(area(&points) > 0.0001);
            }
        }
    }

    #[test]
    fn the_triangle_becomes_two_bars_without_leaving_the_box() {
        let start = area(&PLAY_LEFT) + area(&PLAY_RIGHT);
        let end = area(&PAUSE_LEFT) + area(&PAUSE_RIGHT);
        assert!(end > start * 1.2, "pause bars cover {end}, play {start}");
        for step in 0..=20 {
            let progress = step as f32 / 20.;
            let shapes = play_pause_shapes(progress);
            let polygons = std::array::from_fn(|index| {
                shapes[index]
                    .points()
                    .try_into()
                    .expect("transport glyph quad")
            });
            let [x, y] = polygon_centroid(&polygons);
            assert!(
                (x - 0.5).abs() < 1e-5 && (y - 0.5).abs() < 1e-5,
                "{progress}: [{x}, {y}]"
            );
            for polygon in [
                lerp(PLAY_LEFT, PAUSE_LEFT, progress),
                lerp(PLAY_RIGHT, PAUSE_RIGHT, progress),
            ] {
                for [x, y] in polygon {
                    assert!((0. ..=1.).contains(&x) && (0. ..=1.).contains(&y));
                }
                assert!(area(&polygon) > 0., "degenerate polygon at {progress}");
            }
        }
    }

    #[test]
    fn required_durations_stay_in_the_product_windows() {
        assert_eq!(PLAY_PAUSE_MORPH, Duration::from_millis(220));
        assert!((Duration::from_millis(280)..=Duration::from_millis(400)).contains(&SHUFFLE_MORPH));
        for glyph in Glyph::ALL {
            assert!(glyph.morph_duration() >= PULSE_DURATION);
            assert!(glyph.morph_duration() <= Duration::from_millis(400));
        }
    }

    #[test]
    fn a_morph_interrupts_from_where_it_is_and_settles() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        assert_eq!(icons.progress_at("probe", false, start, false), 0.);
        let first = icons.progress_at("probe", true, start + Duration::from_millis(16), false);
        let middle = icons.progress_at("probe", true, start + Duration::from_millis(90), false);
        assert!(middle > first);
        let reversed = icons.progress_at("probe", false, start + Duration::from_millis(120), false);
        assert!(
            reversed < middle && reversed > 0.,
            "reversal must keep position"
        );
        let settled = icons.progress_at("probe", false, start + Duration::from_millis(1200), false);
        assert_eq!(settled, 0.);
        assert!(!icons.active(start + Duration::from_millis(1200)));
    }

    #[test]
    fn a_toggle_after_a_long_idle_does_not_jump() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        assert_eq!(icons.progress_at("idle", false, start, false), 0.);
        let after_idle = icons.progress_at("idle", true, start + Duration::from_secs(10), false);
        assert_eq!(
            after_idle, 0.,
            "a stale frame must not fast-forward the morph"
        );
        let moving = icons.progress_at(
            "idle",
            true,
            start + Duration::from_secs(10) + Duration::from_millis(16),
            false,
        );
        assert!(moving > 0.);
    }

    #[test]
    fn reduced_motion_commits_the_state_without_animating() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        assert_eq!(icons.progress_at("probe", true, start, true), 1.);
        assert!(!icons.active(start));
    }

    #[test]
    fn every_glyph_reaches_its_target_within_its_declared_duration() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        for (index, glyph) in Glyph::ALL.into_iter().enumerate() {
            let id = ("settle", index);
            assert_eq!(icons.frame_at(id, false, glyph, start, false).progress, 0.);
            let moving = icons.frame_at(id, true, glyph, start + Duration::from_millis(16), false);
            assert!(moving.progress > 0., "{glyph:?} did not start moving");
            let end = start + glyph.morph_duration() + Duration::from_millis(17);
            assert_eq!(
                icons.frame_at(id, true, glyph, end, false).progress,
                1.,
                "{glyph:?} did not settle within its declared duration"
            );
        }
        assert!(!icons.active(start + Duration::from_secs(2)));
    }

    #[test]
    fn simultaneous_controls_keep_independent_progress() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        icons.frame_at("first", false, Glyph::Shuffle, start, false);
        icons.frame_at("second", false, Glyph::Like, start, false);
        let first = icons.frame_at(
            "first",
            true,
            Glyph::Shuffle,
            start + Duration::from_millis(16),
            false,
        );
        let second = icons.frame_at(
            "second",
            false,
            Glyph::Like,
            start + Duration::from_millis(16),
            false,
        );
        assert!(first.progress > 0.);
        assert_eq!(second.progress, 0.);
    }

    #[test]
    fn an_abandoned_morph_settles_and_stops_the_frame_pump() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        icons.frame_at("gone", false, Glyph::Like, start, false);
        icons.frame_at(
            "gone",
            true,
            Glyph::Like,
            start + Duration::from_millis(16),
            false,
        );
        assert!(icons.active(start + Duration::from_millis(32)));
        let end = start + LIKE_MORPH + Duration::from_millis(50);
        assert!(
            !icons.active(end),
            "an unrendered control must not spin the frame pump"
        );
        let settled = icons.frame_at(
            "gone",
            true,
            Glyph::Like,
            end + Duration::from_millis(10),
            false,
        );
        assert_eq!(settled.progress, 1.);
    }

    #[test]
    fn a_pulse_returns_to_rest_and_stops_requesting_frames() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        icons.start_pulse("nudge", start);
        assert!(icons.active(start));
        let moving = icons.frame_at(
            "nudge",
            false,
            Glyph::Next,
            start + Duration::from_millis(30),
            false,
        );
        assert!(moving.pulse > 0.);
        let end = start + PULSE_DURATION + Duration::from_millis(1);
        assert!(!icons.active(end));
        let settled = icons.frame_at("nudge", false, Glyph::Next, end, false);
        assert_eq!(settled.pulse, 0.);
    }

    #[test]
    fn reduced_motion_cancels_an_existing_pulse() {
        let icons = IconAnimations::default();
        let start = Instant::now();
        icons.start_pulse("nudge", start);
        assert!(icons.active(start));
        let frame = icons.frame_at("nudge", false, Glyph::Next, start, true);
        assert_eq!(frame.pulse, 0.);
        assert!(!icons.active(start));
    }

    #[test]
    fn indicators_only_appear_on_their_owning_glyphs() {
        for glyph in procedural_glyphs() {
            let plain = shapes(glyph, 0.8, 0., false);
            let active = shapes(glyph, 0.8, 0., true);
            if glyph.wants_indicator() {
                assert_eq!(
                    active.len(),
                    plain.len() + 1,
                    "{glyph:?} should add one indicator dot"
                );
            } else {
                assert_eq!(
                    active.len(),
                    plain.len(),
                    "{glyph:?} should not add an indicator"
                );
            }
        }
    }
}
