//! The pinned group in the sidebar: pinned playlists, albums and artists.
//!
//! Ordering is changed only by dragging an item or by Alt+↑/Alt+↓; a click or
//! Enter opens the item and never reorders it.

mod commands;

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, Context, Div, FocusHandle, FontWeight, IntoElement, ListAlignment, ListState,
    Pixels, Render, Role, SharedString, SpanId, Stateful, Task, Window, accesskit::Toggled, div,
    list, point, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::PlaylistKey,
    storage::{PinId, PinnedKind, PinnedObject, Store},
    ui::{
        MainView, MovePinDown, MovePinUp, PIN_KEY_CONTEXT,
        assets::provider_mark,
        components::{
            Activate, button,
            music::cover,
            scroll_viewport::{Density, FluidScrollViewport},
            scrollbar::ScrollSurface,
        },
        design_system::motion::{ExitingSpans, MOTION, out_cubic},
        music::detail::hero_motion::{HeroMotion, cover_span, painted_fragment},
        theme::{ACCENT, ELEVATED, NAV_LINK_HOVER, secondary, surface_border},
    },
};

use super::{MusicTarget, Route, Target};
#[cfg(feature = "capture")]
use crate::storage::NewPin;
use commands::PinCommand;
pub(in crate::ui) use commands::PinRequest;

const PIN_ROW_HEIGHT: f32 = 34.;
const PIN_ROW_HEIGHT_COMPACT: f32 = 30.;
const PIN_ART_SIZE: f32 = 22.;
const PIN_GROUP_TAB: isize = 17;

fn pin_row_height(compact: bool) -> f32 {
    if compact {
        PIN_ROW_HEIGHT_COMPACT
    } else {
        PIN_ROW_HEIGHT
    }
}

/// The value carried by a drag of a pinned item.
#[derive(Clone)]
struct PinDrag {
    id: PinId,
    title: String,
}

pub(in crate::ui) struct Pins {
    committed: Vec<PinnedObject>,
    items: Vec<PinnedObject>,
    item_focus: HashMap<PinId, FocusHandle>,
    list: ListState,
    focus: gpui::FocusHandle,
    load: Option<Task<()>>,
    write: Option<Task<()>>,
    pending: VecDeque<PinCommand>,
    refresh_after_write: bool,
    appearing: Option<(PinId, Instant)>,
    departure: Option<PinDeparture>,
    exits: ExitingSpans,
    #[cfg(feature = "capture")]
    motion_sample: Option<Duration>,
    #[cfg(feature = "capture")]
    capture_bounds: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
}

struct PinDeparture {
    started: Instant,
    origins: RefCell<HashMap<SpanId, Option<HeroMotion>>>,
}

impl Pins {
    pub(in crate::ui) fn new(cx: &mut Context<MainView>) -> Self {
        Self {
            committed: Vec::new(),
            items: Vec::new(),
            item_focus: HashMap::new(),
            list: ListState::new(0, ListAlignment::Top, px(100.)),
            focus: cx.focus_handle(),
            load: None,
            write: None,
            pending: VecDeque::new(),
            refresh_after_write: false,
            appearing: None,
            departure: None,
            exits: ExitingSpans::default(),
            #[cfg(feature = "capture")]
            motion_sample: None,
            #[cfg(feature = "capture")]
            capture_bounds: std::rc::Rc::default(),
        }
    }

    fn contains(&self, id: &PinId) -> bool {
        self.items.iter().any(|pin| &pin.id == id)
    }

    fn sampled_motion(&self) -> Option<Duration> {
        #[cfg(feature = "capture")]
        {
            self.motion_sample
        }
        #[cfg(not(feature = "capture"))]
        {
            None
        }
    }

    fn animate_departure(&mut self, id: &PinId, items: &[PinnedObject]) {
        if self.contains(id) && items.iter().all(|pin| &pin.id != id) {
            let started = Instant::now();
            self.exits.start(pin_span(id), started);
            self.departure = Some(PinDeparture {
                started,
                origins: RefCell::default(),
            });
        }
    }

    fn replace(&mut self, items: Vec<PinnedObject>, cx: &Context<MainView>) {
        if self
            .items
            .iter()
            .map(|pin| &pin.id)
            .eq(items.iter().map(|pin| &pin.id))
        {
            self.items = items;
            return;
        }
        let old_len = self.items.len();
        if !self.pending.is_empty()
            && items.len() > old_len
            && let Some(added) = items
                .iter()
                .find(|pin| !self.item_focus.contains_key(&pin.id))
        {
            self.appearing = Some((added.id.clone(), Instant::now()));
        }
        let mut offset = self.list.logical_scroll_top();
        if let Some(top) = self.items.get(offset.item_ix) {
            offset.item_ix = items
                .iter()
                .position(|pin| pin.id == top.id)
                .unwrap_or_else(|| offset.item_ix.min(items.len().saturating_sub(1)));
        }
        let handles: HashMap<_, _> = items
            .iter()
            .enumerate()
            .map(|(position, pin)| {
                let focus = self
                    .item_focus
                    .remove(&pin.id)
                    .unwrap_or_else(|| cx.focus_handle())
                    .tab_index(position as isize)
                    .tab_stop(true);
                (pin.id.clone(), focus)
            })
            .collect();
        self.list.splice_focusable(
            0..old_len,
            items.iter().map(|pin| handles.get(&pin.id).cloned()),
        );
        self.list.scroll_to(offset);
        self.item_focus = handles;
        self.items = items;
    }
}

impl MainView {
    /// Read the pinned group once at startup.
    pub(in crate::ui) fn load_pins(&mut self, cx: &mut Context<Self>) {
        if self.pins.load.is_none() {
            self.refresh_pins(cx);
        }
    }

    /// Re-read the group after a local playlist was renamed or deleted.
    pub(in crate::ui) fn refresh_pins(&mut self, cx: &mut Context<Self>) {
        if self.pins.write.is_some() {
            self.pins.refresh_after_write = true;
            return;
        }
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async { Store::open_default().and_then(|store| store.pinned_objects()) })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.pins.load = None;
                match result {
                    Ok(items) => {
                        view.pins.committed = items.clone();
                        view.pins.replace(items, cx);
                        let targets = view
                            .pins
                            .items
                            .iter()
                            .filter_map(pin_music_target)
                            .take(2)
                            .collect::<Vec<_>>();
                        view.prefetch_pins(targets, cx);
                        cx.notify();
                    }
                    Err(error) => {
                        eprintln!("Не удалось прочитать закреплённое Mediadeka: {error}");
                    }
                }
            });
        });
        self.pins.load = Some(task);
    }

    pub(in crate::ui) fn is_pinned(&self, id: &PinId) -> bool {
        self.pins.contains(id)
    }

    fn move_pin(&mut self, id: PinId, up: bool, cx: &mut Context<Self>) {
        self.queue_pin(PinCommand::Move(id, up), cx);
    }

    fn move_pin_to(&mut self, id: PinId, position: i64, cx: &mut Context<Self>) {
        self.queue_pin(PinCommand::MoveTo(id, position), cx);
    }

    pub(super) fn has_pins(&self) -> bool {
        !self.pins.items.is_empty() || !self.pins.exits.is_empty()
    }

    /// The pinned item that owns the current route, if any.
    pub(super) fn pinned_current(&self) -> Option<&PinnedObject> {
        let id = self.current_pin_id()?;
        self.pins.items.iter().find(|pin| pin.id == id)
    }

    fn current_pin_id(&self) -> Option<PinId> {
        match self.workspace.routes.current() {
            Route::Playlist(target) => Some(pin_id_of_target(target)),
            Route::Object(target) | Route::Station(target) => pin_id_of_object(target),
            _ => None,
        }
    }

    pub(super) fn pinned_group(
        &self,
        compact: bool,
        show_label: bool,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let has_items = !self.pins.items.is_empty();
        let leaving = !self.pins.exits.is_empty();
        if !has_items && !leaving {
            return None;
        }
        #[cfg(feature = "capture")]
        self.pins
            .capture_bounds
            .borrow_mut()
            .resize(self.pins.items.len(), Default::default());
        let current = self.current_pin_id();
        let view = cx.weak_entity();
        let list = div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(FluidScrollViewport::new(
                div()
                    .relative()
                    .size_full()
                    .when(leaving, |body| {
                        body.child(self.pins.exits.layer(
                            !self.reduced_motion && !cx.reduce_motion(),
                            self.pins.sampled_motion(),
                        ))
                    })
                    .child(
                        list(self.pins.list.clone(), move |position, window, cx| {
                            view.update(cx, |view, cx| {
                                let pin = &view.pins.items[position];
                                div()
                                    .pb(px(2.))
                                    .child(view.pinned_item(
                                        position,
                                        pin,
                                        current.as_ref(),
                                        compact,
                                        window,
                                        cx,
                                    ))
                                    .into_any_element()
                            })
                            .unwrap_or_else(|_| div().into_any_element())
                        })
                        .size_full(),
                    ),
                ScrollSurface::VirtualList(self.pins.list.clone()),
                self.uses_fluid_glass(),
                Density::Panel,
            ))
            .map(|list| {
                // Preserve the open group's footprint for the last painted row,
                // and remove its controls from the accessibility tree immediately.
                if !has_items {
                    return list.into_any_element();
                }
                list.id("home-pinned")
                    .accessibility_id("home-pinned")
                    .role(Role::List)
                    .aria_label("Закреплённое")
                    .aria_size_of_set(self.pins.items.len())
                    .track_focus(&self.pins.focus)
                    .tab_index(PIN_GROUP_TAB)
                    .tab_group()
                    .tab_stop(false)
                    .into_any_element()
            });
        let label_opacity = if has_items {
            1.
        } else {
            self.pins.departure.as_ref().map_or(0., |departure| {
                1. - out_cubic(
                    self.pins
                        .sampled_motion()
                        .unwrap_or_else(|| departure.started.elapsed())
                        .as_secs_f32()
                        / MOTION.normal().as_secs_f32(),
                )
            })
        };
        Some(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .when(show_label, |group| {
                    group.child(pin_label(compact).opacity(label_opacity))
                })
                .child(list),
        )
    }

    fn pinned_item(
        &self,
        position: usize,
        pin: &PinnedObject,
        current: Option<&PinId>,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = current == Some(&pin.id);
        let dragged = PinDrag {
            id: pin.id.clone(),
            title: pin.title.clone(),
        };
        let action_id = pin.id.clone();
        let open_id = pin.id.clone();
        let span = pin_span(&pin.id);
        let sample = self.pins.sampled_motion();
        let motion = self
            .pins
            .departure
            .as_ref()
            .filter(|departure| {
                self.uses_fluid_glass()
                    && !self.reduced_motion
                    && !cx.reduce_motion()
                    && sample.unwrap_or_else(|| departure.started.elapsed()) < MOTION.normal()
            })
            .and_then(|departure| {
                *departure
                    .origins
                    .borrow_mut()
                    .entry(span)
                    .or_insert_with(|| {
                        window.rendered_span(span).map(|bounds| {
                            HeroMotion::with_start(bounds, span, departure.started, MOTION.normal())
                        })
                    })
            });
        #[cfg(feature = "capture")]
        let motion = motion.map(|mut motion| {
            if let Some(sample) = sample {
                motion.sample(sample);
            }
            motion
        });
        if !self.pins.exits.is_empty() {
            self.pins.exits.forget(span);
        }
        let appearing = self.pins.appearing.as_ref().filter(|(id, started)| {
            id == &pin.id
                && started.elapsed() < MOTION.normal()
                && self.uses_fluid_glass()
                && !self.reduced_motion
                && !cx.reduce_motion()
        });
        if appearing.is_some() && window.is_visible() && window.is_window_active() {
            window.request_animation_frame();
        }
        let item = button(
            ("home-pinned-item", position),
            format!("{} · {}", pin.title, kind_label(pin.id.kind)),
            position as isize,
        )
        .accessibility_id(format!("home-pinned-{position}"))
        .track_focus(&self.pins.item_focus[&pin.id])
        .key_context(PIN_KEY_CONTEXT)
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .when(selected, |item| item.aria_description("Текущий раздел"))
        .w_full()
        .h(px(pin_row_height(compact)))
        .gap(px(9.))
        .rounded(px(9.))
        .when_some(appearing, |item, (_, started)| {
            let progress =
                out_cubic(started.elapsed().as_secs_f32() / MOTION.normal().as_secs_f32());
            item.opacity(0.28 + 0.72 * progress)
                .layer_translate(point(px(-4. * (1. - progress)), Pixels::ZERO))
        })
        .when(compact, |item| {
            item.justify_center()
                .tooltip(guise::tooltip(pin.title.clone()))
        })
        .when(!compact, |item| item.px(px(9.)))
        .when(selected, |item| item.bg(rgb(ELEVATED)))
        .hover(|style| style.bg(rgb(NAV_LINK_HOVER)))
        .child(pin_art(pin))
        .when(!compact, |item| {
            item.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(pin.title.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(secondary())
                    .child(kind_label(pin.id.kind)),
            )
        })
        .on_drag(dragged, |value: &PinDrag, _, _, cx| {
            cx.new(|_| PinDragPreview {
                title: SharedString::from(value.title.clone()),
            })
        })
        .on_drop(cx.listener(move |view, value: &PinDrag, _, cx| {
            view.move_pin_to(value.id.clone(), position as i64, cx);
        }))
        .on_action(cx.listener({
            let id = action_id.clone();
            move |view, _: &MovePinUp, _, cx| view.move_pin(id.clone(), true, cx)
        }))
        .on_action(cx.listener(move |view, _: &MovePinDown, _, cx| {
            view.move_pin(action_id.clone(), false, cx)
        }))
        .on_activate(
            cx.listener(move |view, _, window, cx| view.open_pinned(open_id.clone(), window, cx)),
        );
        #[cfg(feature = "capture")]
        let item = {
            let bounds = self.pins.capture_bounds.clone();
            div()
                .on_children_prepainted(move |children, _, _| {
                    if let (Some(&child), Some(slot)) =
                        (children.first(), bounds.borrow_mut().get_mut(position))
                    {
                        *slot = child;
                    }
                })
                .child(item)
                .into_any_element()
        };
        #[cfg(not(feature = "capture"))]
        let item = item.into_any_element();
        if self.uses_fluid_glass() {
            painted_fragment(item, span, motion)
        } else {
            item
        }
    }

    fn open_pinned(&mut self, id: PinId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pin) = self.pins.items.iter().find(|pin| pin.id == id).cloned() else {
            return;
        };
        match pin.id.kind {
            PinnedKind::Playlist => {
                let key = match pin.id.provider {
                    Some(provider) => PlaylistKey::Source {
                        provider,
                        profile: (!pin.id.profile.is_empty()).then(|| pin.id.profile.clone()),
                        id: pin.id.object_id.clone(),
                    },
                    None => {
                        let Ok(local) = pin.id.object_id.parse() else {
                            return;
                        };
                        PlaylistKey::Local(local)
                    }
                };
                self.navigate(
                    Route::Playlist(Target {
                        key,
                        name: pin.title.clone(),
                        artwork: pin.artwork.clone(),
                        link: pin.link.clone(),
                    }),
                    window,
                    cx,
                );
            }
            PinnedKind::Album | PinnedKind::Artist | PinnedKind::Station => {
                if let Some(target) = pin_music_target(&pin) {
                    self.open_music_target(target, window, cx);
                }
            }
        }
    }
}

fn pin_music_target(pin: &PinnedObject) -> Option<MusicTarget> {
    let provider = pin.id.provider?;
    let kind = match pin.id.kind {
        PinnedKind::Album => ObjectKind::Album,
        PinnedKind::Artist => ObjectKind::Artist,
        PinnedKind::Station => ObjectKind::Station,
        PinnedKind::Playlist => ObjectKind::Playlist,
    };
    let object = ObjectRef {
        provider,
        kind,
        id: pin.id.object_id.clone(),
    };
    Some(MusicTarget {
        object,
        profile: (!pin.id.profile.is_empty()).then(|| pin.id.profile.clone()),
        title: pin.title.clone(),
        artwork: pin.artwork.clone(),
    })
}

fn pin_span(id: &PinId) -> SpanId {
    cover_span(("sidebar-pin", id))
}

/// Build the identity of a playlist route target.
pub(in crate::ui) fn pin_id_of_target(target: &Target) -> PinId {
    match &target.key {
        PlaylistKey::Local(id) => PinId {
            kind: PinnedKind::Playlist,
            provider: None,
            profile: String::new(),
            object_id: id.to_string(),
        },
        PlaylistKey::Source {
            provider,
            profile,
            id,
        } => PinId {
            kind: PinnedKind::Playlist,
            provider: Some(*provider),
            profile: profile.clone().unwrap_or_default(),
            object_id: id.clone(),
        },
        PlaylistKey::Station {
            provider,
            profile,
            id,
        } => PinId {
            kind: PinnedKind::Station,
            provider: Some(*provider),
            profile: profile.clone().unwrap_or_default(),
            object_id: id.clone(),
        },
    }
}

/// Build the identity of a pinnable object route. Tracks and collections are not pinnable.
pub(in crate::ui) fn pin_id_of_object(target: &MusicTarget) -> Option<PinId> {
    let kind = match target.object.kind {
        ObjectKind::Playlist => PinnedKind::Playlist,
        ObjectKind::Album => PinnedKind::Album,
        ObjectKind::Artist => PinnedKind::Artist,
        ObjectKind::Station => PinnedKind::Station,
        ObjectKind::Track | ObjectKind::Collection => return None,
    };
    Some(PinId {
        kind,
        provider: Some(target.object.provider),
        profile: target.profile.clone().unwrap_or_default(),
        object_id: target.object.id.clone(),
    })
}

pub(in crate::ui) fn pin_of_target(target: &Target) -> PinRequest {
    PinRequest {
        id: pin_id_of_target(target),
        title: target.name.clone(),
        artwork: target.artwork.clone(),
        link: target.link.clone(),
    }
}

pub(in crate::ui) fn pin_of_object(target: &MusicTarget) -> Option<PinRequest> {
    let id = pin_id_of_object(target)?;
    Some(PinRequest {
        id,
        title: target.title.clone(),
        artwork: target.artwork.clone(),
        link: None,
    })
}

fn kind_label(kind: PinnedKind) -> &'static str {
    match kind {
        PinnedKind::Playlist => "Плейлист",
        PinnedKind::Album => "Альбом",
        PinnedKind::Artist => "Артист",
        PinnedKind::Station => "Станция",
    }
}

fn pin_label(compact: bool) -> Stateful<Div> {
    div()
        .id("home-pinned-label")
        .accessibility_id("home-pinned-label")
        .role(Role::Label)
        .aria_label("Закреплённое")
        .aria_value("Закреплённое")
        .h(px(if compact { 22. } else { 28. }))
        .flex_none()
        .flex()
        .items_center()
        .when(compact, |label| {
            label
                .justify_center()
                .child(div().w(px(24.)).h(px(1.)).bg(surface_border()))
        })
        .when(!compact, |label| {
            label
                .px(px(12.))
                .text_size(px(11.))
                .text_color(secondary())
                .child("Закреплённое")
        })
}

fn pin_art(pin: &PinnedObject) -> Div {
    div()
        .flex_none()
        .size(px(PIN_ART_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .when_some(pin.artwork.clone(), |art, artwork| {
            art.child(cover(Some(&artwork), PIN_ART_SIZE, 5., false))
        })
        .when(pin.artwork.is_none(), |art| {
            art.child(match pin.id.provider {
                Some(provider) => provider_mark(provider, PIN_ART_SIZE).into_any_element(),
                None => Icon::new(IconName::ListMusic)
                    .pixel_size(px(17.))
                    .color(gpui::Hsla::from(rgb(ACCENT)))
                    .into_any_element(),
            })
        })
}

#[cfg(feature = "capture")]
impl MainView {
    #[cfg(feature = "capture")]
    pub fn sample_pin_departure_for_capture(
        &mut self,
        millis: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        self.pins.motion_sample = millis.map(Duration::from_millis);
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn pin_departure_alpha_for_capture(&self) -> f32 {
        self.pins.exits.strongest_alpha()
    }

    pub fn pinned_bounds_for_capture(&self, index: usize) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.pins
            .capture_bounds
            .borrow()
            .get(index)
            .copied()
            .filter(|bounds| bounds.size.width > px(0.))
    }

    pub fn pinned_order_for_capture(&self) -> Vec<String> {
        self.pins
            .items
            .iter()
            .map(|pin| pin.title.clone())
            .collect()
    }

    /// Read the persisted order straight from SQLite for the drop assertion.
    pub fn pinned_order_from_store_for_capture(&self) -> gpui::Result<Vec<String>> {
        let store = Store::open_default()?;
        Ok(store
            .pinned_objects()?
            .into_iter()
            .map(|pin| pin.title)
            .collect())
    }

    /// Seed the pinned group of the isolated capture profile.
    pub fn seed_pins_for_capture(&mut self, local_playlist: i64, cx: &mut Context<Self>) {
        let seed = || -> gpui::Result<()> {
            let store = Store::open_default()?;
            store.use_guest(|| Some(()))?;
            store.pin_object(&NewPin {
                id: PinId {
                    kind: PinnedKind::Playlist,
                    provider: None,
                    profile: String::new(),
                    object_id: local_playlist.to_string(),
                },
                title: None,
                artwork: None,
                link: None,
            })?;
            store.pin_object(&NewPin {
                id: PinId {
                    kind: PinnedKind::Playlist,
                    provider: Some(crate::providers::ProviderId::Spotify),
                    profile: "capture".to_owned(),
                    object_id: "37i9dQZF1DXcBWIGoYBM5M".to_owned(),
                },
                title: Some("SET::0.1".to_owned()),
                artwork: None,
                link: None,
            })?;
            Ok(())
        };
        if let Err(error) = seed() {
            eprintln!("Не удалось засеять закреплённое для кадра: {error}");
        }
        self.refresh_pins(cx);
    }
}

struct PinDragPreview {
    title: SharedString,
}

impl Render for PinDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(180.))
            .px(px(9.))
            .h(px(PIN_ROW_HEIGHT))
            .flex()
            .items_center()
            .rounded(px(9.))
            .border_1()
            .border_color(surface_border())
            .bg(rgb(ELEVATED))
            .text_size(px(13.))
            .child(div().min_w_0().truncate().child(self.title.clone()))
    }
}
