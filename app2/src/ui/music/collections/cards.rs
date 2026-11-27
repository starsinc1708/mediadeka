use std::borrow::Cow;

use gpui::{Context, Div, IntoElement, SharedString, Toggled, Window, div, prelude::*, px, rgb};

use crate::{
    music::{
        ObjectKind,
        library::{Entry, EntryAction, LibraryRow, Scope},
    },
    playlist::PlaylistKey,
    providers::ProviderId,
    ui::{
        MainView,
        components::{
            Activate, button, entity_link,
            music::{card_title, cover, fluid_cover},
        },
        design_system::{
            controls::{ControlState, ControlVariant},
            icons::{Glyph, animated_glyph},
        },
        fluid_glass::{
            artwork::Seed,
            tokens::{self, radius},
        },
        labels::track_count,
        playlist::artwork::playlist_cover,
        theme::{ELEVATED, secondary},
        workspace::{
            MusicTarget, Target,
            pins::{PinRequest, pin_of_object, pin_of_target},
        },
    },
};

use super::{super::prefetch::entry_target, list::source_marks, motion::Surface};
use crate::ui::music::detail::hero_motion::{cover_span, painted_fragment, shared_artwork};

const TILE_PADDING: f32 = 8.;
const TILE_INSET: f32 = TILE_PADDING + 1.;
const GAP: f32 = 12.;
const CAPTION_HEIGHT: f32 = 72.;
const PIN_HITBOX: f32 = 28.;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct Grid {
    pub(super) columns: usize,
    edge: u16,
}

impl Grid {
    pub(super) fn new(width: f32, preferred_edge: u16) -> Self {
        let edge = preferred_edge.min((width - 2. * TILE_INSET).floor().max(1.) as u16);
        let tile = f32::from(edge) + 2. * TILE_INSET;
        let columns = (((width + GAP) / (tile + GAP)).floor() as usize).max(1);
        Self { columns, edge }
    }

    pub(super) fn row_height(self) -> f32 {
        f32::from(self.edge) + CAPTION_HEIGHT + 2. * TILE_INSET
    }
}

impl MainView {
    #[cfg(feature = "capture")]
    pub fn first_album_tile_cover_for_capture(
        &self,
        window: &gpui::Window,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        let library = &self.music.collections;
        let row = library.index.rows.get(*library.results.visible.first()?)?;
        let entry = row.entry(library.location.source.as_ref());
        window.rendered_span(cover_span(("library", &entry.key)))
    }

    pub(super) fn collection_tiles(
        &self,
        row: usize,
        grid: Grid,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let library = &self.music.collections;
        div()
            .flex()
            .gap(px(GAP))
            .pt(px(8.))
            .h(px(grid.row_height()))
            .children(
                library
                    .results
                    .visible
                    .iter()
                    .enumerate()
                    .skip(row * grid.columns)
                    .take(grid.columns)
                    .map(|(index, &position)| {
                        self.collection_tile(index, &library.index.rows[position], grid, window, cx)
                    }),
            )
    }

    fn collection_tile(
        &self,
        index: usize,
        row: &LibraryRow,
        grid: Grid,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let entry = row.entry(self.music.collections.location.source.as_ref());
        let byline = collection_subtitle(entry);
        let year = entry.year.map(|year| year.to_string()).unwrap_or_default();
        let subtitle = [byline.as_ref(), year.as_str()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let marks = row
            .scopes()
            .map(Scope::label)
            .collect::<Vec<_>>()
            .join(", ");
        let label = [entry.title.as_str(), &subtitle, &marks]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let action = entry.action.clone();
        let title = entry.title.clone();
        let artwork = entry.artwork.clone();
        let origin_id = cover_span(("library", &entry.key));
        let (layout_motion, opacity) =
            self.collection_motion(origin_id, Surface::Cover, window, cx);
        let edge = f32::from(grid.edge);
        let order = 102 + index as isize * 3;
        let id = match &entry.action {
            EntryAction::LocalPlaylist { id, .. } => format!("local-playlist-{id}"),
            EntryAction::Object { object, .. } if object.kind == ObjectKind::Playlist => {
                format!("library-playlist-{index}")
            }
            _ => format!("library-album-{index}"),
        };
        let hint = entry_target(entry);
        let visibility = self.prefetch_visibility_probe(hint.iter().cloned());
        let material = self.glass_controls(cx);
        let card = button(SharedString::from(id.clone()), label, order)
            .flex_none()
            .opacity(opacity)
            .flex_col()
            .items_start()
            .justify_start()
            .w(px(edge + 2. * TILE_INSET))
            .min_w_0()
            .p(px(TILE_PADDING))
            .gap(px(4.))
            .rounded(px(radius::CONTROL))
            .when(material.is_none(), |card| {
                card.hover(|style| style.bg(rgb(ELEVATED)))
            })
            .when_some(material, |card, material| {
                ControlVariant::TextAction
                    .paint(card, material, ControlState::default())
                    .text_color(rgb(if material.contrast() {
                        tokens::contrast::PRIMARY
                    } else {
                        tokens::PRIMARY
                    }))
            })
            .child(if self.uses_fluid_glass() {
                let cover = self.collection_cover(entry, edge, 6.);
                if let Some(motion) = self.music_hero_motion() {
                    shared_artwork(cover, origin_id, Some(motion))
                } else {
                    // Reflow stays inside the collection's scroll mask.
                    painted_fragment(cover, origin_id, layout_motion)
                }
            } else {
                cover(entry.artwork.as_deref(), edge, 6., false).into_any_element()
            })
            .child(card_title(entry.title.clone()).flex_none().h(px(19.)))
            .child(
                div()
                    .flex_none()
                    .h(px(17.))
                    .line_height(px(17.))
                    .w_full()
                    .truncate()
                    .text_color(secondary())
                    .text_size(px(12.))
                    .child(subtitle),
            )
            .child(source_marks(row).flex_none().h(px(16.)))
            .on_activate(cx.listener(move |view, _, window, cx| {
                view.navigate_from_artwork(origin_id, window, cx, |view, window, cx| {
                    view.activate_library_entry(&action, &title, artwork.as_deref(), window, cx);
                });
            }));
        let pin = self
            .uses_fluid_glass()
            .then(|| self.collection_pin_control(entry, index, order + 1, cx))
            .flatten();
        div()
            .id(("collection-card", index))
            .group("collection-card")
            .relative()
            .flex_none()
            .child(entity_link::hint(card, &id, order, hint, cx))
            .children(visibility)
            .children(pin)
    }

    fn collection_pin_control(
        &self,
        entry: &Entry,
        index: usize,
        order: isize,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let pin = collection_pin(entry)?;
        let pinned = self.is_pinned(&pin.id);
        let label = if pinned {
            "Открепить"
        } else {
            "Закрепить"
        };
        let id = format!("collection-card-pin-{index}");
        let status = self.pin_status(&pin.id);
        Some(
            ControlVariant::GhostIcon
                .paint(
                    button(id.clone(), format!("{label} · {}", entry.title), order)
                        .absolute()
                        .right(px(TILE_PADDING))
                        .bottom(px(4.))
                        .size(px(PIN_HITBOX))
                        .justify_center()
                        .aria_toggled(Toggled::from(pinned))
                        .when_some(status, |button, status| button.aria_description(status))
                        .child(animated_glyph(id, Glyph::Pin, pinned, 16., None, cx)),
                    self.glass_controls(cx)?,
                    ControlState {
                        active: pinned,
                        ..ControlState::default()
                    },
                )
                .when(!pinned, |pin| {
                    pin.opacity(0.)
                        .group_hover("collection-card", |style| style.opacity(1.))
                        .focus_visible(|style| style.opacity(1.))
                })
                .tooltip(guise::tooltip(status.unwrap_or(label)))
                .on_activate(cx.listener(move |view, _, _, cx| view.toggle_pin(pin.clone(), cx))),
        )
    }

    pub(super) fn collection_cover(&self, entry: &Entry, edge: f32, radius: f32) -> Div {
        let url = entry.artwork.as_deref();
        let key = match &entry.action {
            EntryAction::LocalPlaylist { id, .. } => PlaylistKey::Local(*id),
            EntryAction::Object {
                profile, object, ..
            } if object.kind == ObjectKind::Playlist => PlaylistKey::Source {
                provider: object.provider,
                profile: Some(profile.clone()),
                id: object.id.clone(),
            },
            _ => return fluid_cover(url, Seed::entry(&entry.action), edge, radius),
        };
        playlist_cover(
            url,
            self.playlist_artwork_candidates(&key),
            Seed::collection(&key),
            edge,
            radius,
        )
    }
}

pub(super) fn collection_subtitle(entry: &Entry) -> Cow<'_, str> {
    match &entry.action {
        EntryAction::LocalPlaylist {
            track_count: count, ..
        } => Cow::Owned(track_count(*count)),
        // Older saved Spotify collections may carry an owner username in this field.
        EntryAction::Object { object, .. }
            if object.provider == ProviderId::Spotify && object.kind == ObjectKind::Playlist =>
        {
            Cow::Borrowed("")
        }
        _ => Cow::Borrowed(&entry.subtitle),
    }
}

fn collection_pin(entry: &Entry) -> Option<PinRequest> {
    match &entry.action {
        EntryAction::LocalPlaylist { id, .. } => {
            Some(pin_of_target(&Target::local(*id, entry.title.clone())))
        }
        EntryAction::Object {
            profile, object, ..
        } => pin_of_object(&MusicTarget {
            object: object.clone(),
            profile: Some(profile.clone()),
            title: entry.title.clone(),
            artwork: entry.artwork.clone(),
        }),
        _ => None,
    }
}
