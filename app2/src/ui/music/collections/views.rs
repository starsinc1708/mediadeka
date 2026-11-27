use gpui::{Context, Div, Role, Toggled, div, prelude::*, px};

use crate::{
    storage::{CollectionDisplay, CollectionDisplayChange, CollectionKind, CollectionView, Store},
    ui::{
        MainView,
        components::{
            Activate, button, paint_section_tab,
            slider::{Slider, slider},
        },
        music::status,
        theme::secondary,
        workspace::LibraryTab,
    },
};

#[derive(Default)]
pub(in crate::ui::music) struct CollectionViews {
    albums: Option<CollectionDisplay>,
    playlists: Option<CollectionDisplay>,
    error: Option<String>,
}

impl LibraryTab {
    pub(super) fn collection_kind(self) -> Option<CollectionKind> {
        match self {
            Self::Albums => Some(CollectionKind::Albums),
            Self::Playlists => Some(CollectionKind::Playlists),
            Self::Tracks | Self::Artists => None,
        }
    }
}

impl CollectionViews {
    pub(super) fn get(&self, kind: CollectionKind) -> CollectionDisplay {
        match kind {
            CollectionKind::Albums => self.albums,
            CollectionKind::Playlists => self.playlists,
        }
        .unwrap_or_default()
    }

    fn slot(&mut self, kind: CollectionKind) -> &mut Option<CollectionDisplay> {
        match kind {
            CollectionKind::Albums => &mut self.albums,
            CollectionKind::Playlists => &mut self.playlists,
        }
    }

    pub(super) fn load(&mut self, kind: CollectionKind) {
        if self.slot(kind).is_some() {
            return;
        }
        match Store::open_default().and_then(|store| store.collection_display(kind)) {
            Ok(display) => {
                *self.slot(kind) = Some(display);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Не удалось прочитать вид коллекции. {error}")),
        }
    }
}

impl MainView {
    fn update_collection_display(
        &mut self,
        kind: CollectionKind,
        change: CollectionDisplayChange,
        cx: &mut Context<Self>,
    ) {
        let views = &mut self.music.collection_views;
        let current = views.get(kind);
        let same = match change {
            CollectionDisplayChange::View(view) => current.view == view,
            CollectionDisplayChange::CardEdge(edge) => current.card_edge == edge,
        };
        if same {
            return;
        }
        match Store::open_default().and_then(|store| store.save_collection_display(kind, change)) {
            Ok(display) => {
                *views.slot(kind) = Some(display);
                views.error = None;
            }
            Err(error) => {
                views.error = Some(format!("Не удалось сохранить вид коллекции. {error}"))
            }
        }
        cx.notify();
    }

    fn set_collection_card_edge(&mut self, value: f64, cx: &mut Context<Self>) {
        let Some(kind) = self
            .music_page()
            .library_tab()
            .and_then(LibraryTab::collection_kind)
        else {
            return;
        };
        let min = f64::from(CollectionDisplay::MIN_CARD_EDGE);
        let step = f64::from(CollectionDisplay::CARD_EDGE_STEP);
        let edge = min + ((value - min) / step).round() * step;
        let edge = (edge as u16).clamp(
            CollectionDisplay::MIN_CARD_EDGE,
            CollectionDisplay::MAX_CARD_EDGE,
        );
        self.update_collection_display(kind, CollectionDisplayChange::CardEdge(edge), cx);
    }

    pub(super) fn collection_view_controls(&self, kind: CollectionKind, cx: &Context<Self>) -> Div {
        let views = &self.music.collection_views;
        let selected = views.get(kind);
        let material = crate::ui::design_system::controls::material(cx);
        let mode = div()
            .id("collection-view-modes")
            .accessibility_id("collection-view-modes")
            .role(Role::RadioGroup)
            .aria_label("Вид коллекции")
            .flex()
            .flex_none()
            .flex_wrap()
            .gap(px(2.))
            .children(
                [
                    (CollectionView::Grid, "Сетка"),
                    (CollectionView::List, "Список"),
                    (CollectionView::Compact, "Компактный"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (view, label))| {
                    let picked = view == selected.view;
                    paint_section_tab(
                        button(
                            format!("collection-view-{}", view.as_str()),
                            label,
                            33 + index as isize,
                        )
                        .role(Role::RadioButton)
                        .aria_selected(picked)
                        .aria_toggled(Toggled::from(picked))
                        .h(px(30.))
                        .px(px(8.))
                        .text_size(px(13.))
                        .child(label),
                        material,
                        picked,
                    )
                    .on_activate(cx.listener(move |main, _, _, cx| {
                        main.update_collection_display(
                            kind,
                            CollectionDisplayChange::View(view),
                            cx,
                        );
                    }))
                }),
            );
        let size = || {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(secondary())
                        .child("Размер обложек"),
                )
                .child(div().flex().w(px(96.)).child(slider(
                    "collection-card-size",
                    "Размер обложек",
                    36,
                    Slider {
                        value: f64::from(selected.card_edge),
                        min: f64::from(CollectionDisplay::MIN_CARD_EDGE),
                        max: f64::from(CollectionDisplay::MAX_CARD_EDGE),
                        step: f64::from(CollectionDisplay::CARD_EDGE_STEP),
                        enabled: true,
                        playlist_presentation: false,
                        fluid: true,
                    },
                    Self::set_collection_card_edge,
                    cx,
                )))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(14.))
                    .child(mode)
                    .children((selected.view == CollectionView::Grid).then(size)),
            )
            .children(
                views
                    .error
                    .as_ref()
                    .map(|error| status("collection-layout-error", error.clone())),
            )
    }
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn saved_collection_view_for_capture(&self) -> Option<(&'static str, u16)> {
        let kind = self.music_page().library_tab()?.collection_kind()?;
        let saved = Store::open_default().ok()?.collection_display(kind).ok()?;
        Some((saved.view.as_str(), saved.card_edge))
    }
}
