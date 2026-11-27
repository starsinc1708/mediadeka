use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{
    AnyElement, App, Context, Div, FocusHandle, IntoElement, Role, SpanId, Stateful, Subscription,
    WeakEntity, Window, div, prelude::*, px,
};

use crate::{
    music::{MusicPage, ObjectKind},
    playlist::{PlaylistKey, PlaylistTrack},
    ui::{
        MainView,
        playlist::navigation::{self, Destination, artists, track_destination},
        theme,
    },
};

use super::Activate;

pub(in crate::ui) fn activate(
    node: Stateful<Div>,
    name: impl Into<gpui::SharedString>,
    order: isize,
    open: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    link_action(
        node.hover(|link| link.text_color(theme::primary()))
            .active(|link| link.text_color(theme::navigation_accent()))
            .focus_visible(|link| link.underline()),
        name,
        order,
        open,
    )
}

fn link_action(
    node: Stateful<Div>,
    name: impl Into<gpui::SharedString>,
    order: isize,
    open: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    node.role(Role::Link)
        .aria_label(name)
        .tab_index(order)
        .cursor_pointer()
        .on_activate(move |_, window, cx| {
            cx.stop_propagation();
            open(window, cx);
        })
}

pub(in crate::ui) fn text(
    node: Div,
    id: &str,
    name: &str,
    destination: Option<Destination>,
    order: isize,
    cx: &Context<MainView>,
) -> AnyElement {
    let view = cx.entity().downgrade();
    let intent = destination.clone();
    let link = link_text(
        node,
        id,
        name,
        destination,
        order,
        move |destination, window, cx| {
            view.update(cx, |view, cx| {
                view.follow_playlist_link(destination, window, cx)
            })
            .ok();
        },
    );
    hint(link, id, order, intent, cx)
}

pub(in crate::ui) fn text_with(
    node: Div,
    id: &str,
    name: &str,
    destination: Option<Destination>,
    order: isize,
    open: impl Fn(Destination, &mut Window, &mut App) + 'static,
) -> AnyElement {
    link_text(node, id, name, destination, order, open).into_any_element()
}

pub(in crate::ui) fn hint(
    link: Stateful<Div>,
    id: &str,
    order: isize,
    destination: Option<Destination>,
    cx: &Context<MainView>,
) -> AnyElement {
    let Some(destination) = destination.filter(|entry| match entry {
        Destination::Object(_) => true,
        Destination::Playlist(target) => target.key.provider().is_some(),
        _ => false,
    }) else {
        return link.into_any_element();
    };
    HintLink {
        link,
        id: format!("entity-intent-{id}"),
        order,
        destination,
        view: cx.entity().downgrade(),
    }
    .into_any_element()
}

#[derive(IntoElement)]
struct HintLink {
    link: Stateful<Div>,
    id: String,
    order: isize,
    destination: Destination,
    view: WeakEntity<MainView>,
}

struct HintState {
    focus: FocusHandle,
    signal: Rc<HintSignal>,
    _focus: Subscription,
    _blur: Subscription,
}

struct HintSignal {
    destination: RefCell<Destination>,
    view: WeakEntity<MainView>,
    hovered: Cell<bool>,
}

impl HintSignal {
    fn set(&self, entered: bool, cx: &mut App) {
        let destination = self.destination.borrow().clone();
        let _ = self.view.update(cx, |view, cx| {
            view.hint_music_target(destination, entered, cx)
        });
    }
}

impl RenderOnce for HintLink {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |window, cx| {
            let focus = cx.focus_handle().tab_index(self.order).tab_stop(true);
            let signal = Rc::new(HintSignal {
                destination: RefCell::new(self.destination.clone()),
                view: self.view,
                hovered: Cell::new(false),
            });
            let enter = signal.clone();
            let leave = signal.clone();
            let _focus = window.on_focus_in(&focus, cx, move |window, cx| {
                if window.last_input_was_keyboard() {
                    enter.set(true, cx);
                }
            });
            let _blur = window.on_focus_out(&focus, cx, move |_, _, cx| {
                if !leave.hovered.get() {
                    leave.set(false, cx);
                }
            });
            HintState {
                focus,
                signal,
                _focus,
                _blur,
            }
        });
        let state = state.read(cx);
        let focus = state.focus.clone().tab_index(self.order);
        let signal = state.signal.clone();
        signal.destination.replace(self.destination);
        let hover_focus = focus.clone();
        self.link
            .track_focus(&focus)
            .on_hover(move |entered, window, cx| {
                signal.hovered.set(*entered);
                if *entered || !hover_focus.is_focused(window) {
                    signal.set(*entered, cx);
                }
            })
    }
}

pub(in crate::ui) fn artwork(
    cover: AnyElement,
    id: &str,
    label: &str,
    target: Destination,
    origin: Option<SpanId>,
    order: isize,
    cx: &Context<MainView>,
) -> AnyElement {
    let view = cx.entity().downgrade();
    let link = super::button(id.to_owned(), target.accessible_name(label), order)
        .relative()
        .p_0()
        .flex_none()
        .self_start()
        .child(cover)
        .hover(|link| link.opacity(0.86));
    let intent = target.clone();
    let link = link_action(
        link,
        target.accessible_name(label),
        order,
        move |window, cx| {
            view.update(cx, |view, cx| {
                if let Some(origin) = origin {
                    view.navigate_from_artwork(origin, window, cx, |view, window, cx| {
                        view.follow_playlist_link(target.clone(), window, cx);
                    });
                } else {
                    view.follow_playlist_link(target.clone(), window, cx);
                }
            })
            .ok();
        },
    );
    hint(link, id, order, Some(intent), cx)
}

fn link_text(
    node: Div,
    id: &str,
    name: &str,
    destination: Option<Destination>,
    order: isize,
    open: impl Fn(Destination, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let node = node
        .child(name.to_owned())
        .id(id.to_owned())
        .accessibility_id(id.to_owned());
    let Some(destination) = destination else {
        return node.role(Role::Label).aria_label(name.to_owned());
    };
    let label = destination.accessible_name(name);
    activate(node, label, order, move |window, cx| {
        open(destination.clone(), window, cx)
    })
}

pub(in crate::ui) fn track(
    node: Div,
    id: &str,
    key: &PlaylistKey,
    track: &PlaylistTrack,
    order: isize,
    cx: &Context<MainView>,
) -> AnyElement {
    text(
        node,
        id,
        &track.title,
        track_destination(key, track),
        order,
        cx,
    )
}

pub(in crate::ui) fn artist_names(
    node: Div,
    id: &str,
    key: &PlaylistKey,
    track: &PlaylistTrack,
    order: isize,
    cx: &Context<MainView>,
) -> Div {
    artist_list(
        node,
        id,
        order,
        artists(key, track),
        |node, id, name, target, order| text(node, id, name, Some(target), order, cx),
    )
}

pub(in crate::ui) fn page_artists(
    node: Div,
    page: &MusicPage,
    key: &PlaylistKey,
    cx: &Context<MainView>,
) -> Option<Div> {
    let mut credits = navigation::credits(&page.artists, "", &page.artist_credits)
        .filter_map(|(name, reference)| {
            navigation::related(key, ObjectKind::Artist, name, reference)
                .map(|target| (name, target))
        })
        .peekable();
    credits.peek()?;
    Some(artist_list(
        node,
        "music-object-artist",
        35,
        credits,
        |node, id, name, destination, order| text(node, id, name, Some(destination), order, cx),
    ))
}

pub(in crate::ui) fn artist_list<'a>(
    node: Div,
    id: &str,
    order: isize,
    items: impl IntoIterator<Item = (&'a str, Destination)>,
    mut link: impl FnMut(Div, &str, &str, Destination, isize) -> AnyElement,
) -> Div {
    let mut items = items.into_iter().enumerate().peekable();
    node.flex()
        .items_center()
        .min_w_0()
        .overflow_hidden()
        .gap(px(3.))
        .children(std::iter::from_fn(|| {
            let (index, (name, target)) = items.next()?;
            Some(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .child(link(
                        div().truncate().min_w_0(),
                        &format!("{id}-{index}"),
                        name,
                        target,
                        order,
                    ))
                    .when(items.peek().is_some(), |node| node.child(div().child(","))),
            )
        }))
}
