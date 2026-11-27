//! In-page artist navigation over the workspace's existing scroll surface.

use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, Div, FocusHandle, IntoElement, Pixels, Role, ScrollHandle, Stateful, Window,
    div, prelude::*, px, rgb,
};

use crate::ui::{
    components::{
        Activate, button,
        glass_control::Material,
        scrollbar::{scroll_offset, set_handle_offset},
    },
    design_system::controls::{ControlState, ControlVariant},
    fluid_glass::tokens::{self, size, space, text},
};

const NAV_ORDER: isize = 80;

#[derive(Clone, Copy)]
enum Section {
    Tracks,
    Releases,
    Related,
    Biography,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Self::Tracks => "Треки",
            Self::Releases => "Релизы",
            Self::Related => "Похожие исполнители",
            Self::Biography => "Об исполнителе",
        }
    }

    fn destination(self) -> &'static str {
        match self {
            Self::Tracks => "artist-section-tracks-content",
            Self::Releases => "artist-section-releases-content",
            Self::Related => "artist-section-related-content",
            Self::Biography => "artist-section-biography-content",
        }
    }

    fn link(self) -> &'static str {
        match self {
            Self::Tracks => "artist-section-tracks",
            Self::Releases => "artist-section-releases",
            Self::Related => "artist-section-related",
            Self::Biography => "artist-section-biography",
        }
    }

    fn first_tab(self) -> isize {
        // Place the programmatic destination before its existing actions.
        match self {
            Self::Tracks => 89,
            Self::Releases => 374,
            Self::Related => 349,
            Self::Biography => 800,
        }
    }
}

struct Anchor {
    section: Section,
    top: Rc<Cell<Option<Pixels>>>,
    focus: FocusHandle,
}

impl Anchor {
    fn body(&self, body: AnyElement, material: Material) -> Stateful<Div> {
        let top = self.top.clone();
        let focus_color = rgb(if material.contrast() {
            tokens::contrast::PRIMARY
        } else {
            tokens::PRIMARY
        });
        div()
            .on_children_prepainted(move |children, _, _| {
                top.set(children.first().map(|bounds| bounds.top()));
            })
            .id(self.section.destination())
            .accessibility_id(self.section.destination())
            .role(Role::Group)
            .aria_label(self.section.label())
            .track_focus(&self.focus)
            .tab_stop(false)
            .min_w_0()
            .border_t_1()
            .border_color(gpui::transparent_black())
            .focus_visible(move |body| body.border_color(focus_color))
            .child(body)
    }

    fn link(
        &self,
        index: usize,
        material: Material,
        scroll: ScrollHandle,
        clearance: Pixels,
    ) -> Stateful<Div> {
        let top = self.top.clone();
        let focus = self.focus.clone();
        section_link(self.section.link(), self.section.label(), index, material).on_activate(
            move |_, window, cx| {
                if let Some(top) = top.get() {
                    let target = scroll_offset(&scroll) + top - scroll.bounds().top() - clearance;
                    set_handle_offset(&scroll, target);
                    window.focus(&focus, cx);
                    window.refresh();
                }
            },
        )
    }
}

#[derive(IntoElement)]
pub(super) struct ArtistSections {
    pub tracks: Option<AnyElement>,
    pub related: Option<AnyElement>,
    pub releases: Option<AnyElement>,
    pub biography: Option<AnyElement>,
    pub notice: Option<Div>,
    pub compact: bool,
    pub section_gap: Pixels,
    pub scroll: ScrollHandle,
    pub clearance: Pixels,
    pub material: Material,
}

impl RenderOnce for ArtistSections {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window.use_keyed_state("artist-section-focus", cx, |_, cx| {
            [
                Section::Tracks,
                Section::Releases,
                Section::Related,
                Section::Biography,
            ]
            .map(|section| {
                cx.focus_handle()
                    .tab_index(section.first_tab())
                    .tab_stop(false)
            })
        });
        let mut links = Vec::new();
        let mut section = |kind, body: Option<AnyElement>| {
            body.map(|body| {
                let anchor = Anchor {
                    section: kind,
                    top: Rc::new(Cell::new(None)),
                    focus: focus.read(cx)[kind as usize].clone(),
                };
                links.push(anchor.link(
                    links.len() + 1,
                    self.material,
                    self.scroll.clone(),
                    self.clearance + px(space::SM),
                ));
                anchor.body(body, self.material)
            })
        };
        let tracks = section(Section::Tracks, self.tracks);
        let releases = section(Section::Releases, self.releases);
        let related = section(Section::Related, self.related);
        let biography = section(Section::Biography, self.biography);
        div()
            .flex()
            .flex_col()
            .gap(px(if self.compact { space::SM } else { space::LG }))
            .when(!links.is_empty(), |body| {
                body.child(
                    div()
                        .id("artist-section-navigation")
                        .accessibility_id("artist-section-navigation")
                        .role(Role::Navigation)
                        .aria_label("Разделы исполнителя")
                        .flex()
                        .flex_wrap()
                        .gap(px(space::XS))
                        .border_b_1()
                        .border_color(rgb(if self.material.contrast() {
                            tokens::contrast::RULE
                        } else {
                            tokens::RULE
                        }))
                        .child(
                            section_link("artist-section-overview", "Обзор", 0, self.material)
                                .on_activate(move |_, window, _| {
                                    if set_handle_offset(&self.scroll, Pixels::ZERO) {
                                        window.refresh();
                                    }
                                }),
                        )
                        .children(links),
                )
            })
            .when(tracks.is_some() || related.is_some(), |body| {
                body.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(self.section_gap)
                        .when(self.compact, |row| row.flex_col())
                        .children(
                            tracks.map(|track| {
                                track.flex_1().when(self.compact, |track| track.w_full())
                            }),
                        )
                        .children(related.map(|related| {
                            related
                                .flex_none()
                                .when(self.compact, |related| related.w_full())
                        })),
                )
            })
            .children(self.notice)
            .children(releases)
            .children(biography)
    }
}

fn section_link(
    id: &'static str,
    label: &'static str,
    index: usize,
    material: Material,
) -> Stateful<Div> {
    ControlVariant::TextAction.paint(
        button(id, label, NAV_ORDER + index as isize)
            .role(Role::Link)
            .h(px(size::XS))
            .px(px(space::SM))
            .text_size(px(text::NAVIGATION))
            .child(label),
        material,
        ControlState::default(),
    )
}
