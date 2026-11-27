//! Layout and actions shared by Fluid Glass musical detail pages.

use gpui::{App, Div, FontWeight, Hsla, SharedString, Stateful, Window, div, prelude::*, px, rgb};
use guise::{Button, IconName};

use crate::{
    music::{ArtistDetails, MusicPage},
    providers::ProviderId,
    ui::{
        assets::provider_mark,
        components::{
            artwork::cover_colors,
            button,
            glass_control::{Kind, Material, Tone},
            music::fluid_cover_slot,
        },
        design_system::{
            controls::{ControlState, ControlVariant},
            materials::{self, Interaction},
        },
        fluid_glass::{
            artwork::{self, Seed},
            effects_enabled,
            tokens::{self, contrast, size, space, text},
        },
        theme,
    },
};

#[derive(Clone, Copy)]
pub(in crate::ui) struct Ink {
    pub primary: Hsla,
    pub secondary: Hsla,
    pub rule: Hsla,
}

impl Ink {
    pub(in crate::ui) fn new(fluid: bool, high_contrast: bool) -> Self {
        let (primary, secondary, rule) = match (fluid, high_contrast) {
            (true, true) => (contrast::PRIMARY, contrast::SECONDARY, contrast::RULE),
            (true, false) => (tokens::PRIMARY, tokens::SECONDARY, tokens::RULE),
            (false, _) => (theme::PRIMARY, theme::SECONDARY, theme::DIVIDER),
        };
        Self {
            primary: rgb(primary).into(),
            secondary: rgb(secondary).into(),
            rule: rgb(rule).into(),
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct HeroSize {
    pub cover: f32,
    pub title: f32,
    pub gap: f32,
    pub vertical_inset: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReleasePresentation {
    Single,
    Ep,
    Album,
}

impl ReleasePresentation {
    pub(super) fn for_page(page: Option<&MusicPage>) -> Self {
        let Some(page) = page else { return Self::Album };
        if page.single || page.tracks.len() == 1 {
            Self::Single
        } else if (2..=6).contains(&page.tracks.len())
            && (page.title.ends_with(" - EP")
                || page
                    .tracks
                    .iter()
                    .map(|track| track.duration.map(u64::from))
                    .sum::<Option<u64>>()
                    .is_some_and(|seconds| seconds <= 30 * 60))
        {
            Self::Ep
        } else {
            Self::Album
        }
    }
}

impl HeroSize {
    pub(in crate::ui) fn album(width: f32, compact: bool) -> Self {
        Self::release(width, compact, ReleasePresentation::Album)
    }

    pub(super) fn release(width: f32, compact: bool, release: ReleasePresentation) -> Self {
        if compact {
            return Self {
                cover: match release {
                    ReleasePresentation::Single => 88.,
                    ReleasePresentation::Ep => 96.,
                    ReleasePresentation::Album => 104.,
                },
                title: text::PAGE_TITLE_COMPACT,
                gap: space::LG,
                vertical_inset: space::SM,
            };
        }
        let room = hero_room(width);
        let (cover, title, inset) = match release {
            ReleasePresentation::Single => (176. + 48. * room, 40. + 10. * room, space::LG),
            ReleasePresentation::Ep => (208. + 48. * room, 44. + 10. * room, space::LG),
            ReleasePresentation::Album => {
                (224. + 56. * room, text::HERO_TITLE + 12. * room, space::XL)
            }
        };
        Self {
            cover,
            title,
            gap: space::XL + room * space::SM,
            vertical_inset: inset,
        }
    }

    pub(in crate::ui) fn playlist(width: f32, compact: bool, count: usize) -> Self {
        if compact {
            return Self::fluid(width, true, false);
        }
        let room = hero_room(width);
        let long = count > 8;
        Self {
            cover: if long {
                216. + 48. * room
            } else {
                184. + 48. * room
            },
            title: 44. + 12. * room,
            gap: space::XL + room * space::SM,
            vertical_inset: space::LG + room * space::SM,
        }
    }

    pub(in crate::ui) fn fluid(width: f32, compact: bool, portrait: bool) -> Self {
        if compact {
            return Self {
                cover: if portrait { 96. } else { 100. },
                title: text::PAGE_TITLE_COMPACT,
                gap: space::LG,
                vertical_inset: space::SM,
            };
        }

        let room = ((width - 700.) / 700.).clamp(0., 1.);
        let (cover, title) = if portrait {
            (224. + room * 110., text::HERO_TITLE + room * 12.)
        } else {
            (224. + room * 72., text::HERO_TITLE + room * 12.)
        };
        Self {
            cover,
            title,
            gap: space::XL + room * space::SM,
            vertical_inset: space::LG + room * space::SM,
        }
    }

    /// The quieter hero used by track and station pages, where the page is
    /// about one recording rather than a catalogue object.
    pub(in crate::ui) fn detail(width: f32, compact: bool) -> Self {
        if compact {
            return Self {
                cover: 82.,
                title: 28.,
                gap: 16.,
                vertical_inset: 8.,
            };
        }
        let room = ((width - 680.) / 640.).clamp(0., 1.);
        Self {
            cover: 144. + room * 104.,
            title: text::PAGE_TITLE + room * 18.,
            gap: 24. + room * 14.,
            vertical_inset: 20. + room * 13.,
        }
    }
}

fn hero_room(width: f32) -> f32 {
    ((width - 750.) / 650.).clamp(0., 1.)
}

pub(in crate::ui) fn title_size(nominal: f32, full_title: &str) -> f32 {
    let length = full_title.chars().count() as f32;
    if length <= 34. {
        nominal
    } else {
        nominal * (34. / length).powf(0.24).max(0.72)
    }
}

pub(in crate::ui) fn title(element: Stateful<Div>, size: f32, lines: usize) -> Stateful<Div> {
    element
        .font_family(tokens::FONT)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(size))
        .line_height(px(size * 1.04))
        .letter_spacing(if size >= 48. { -0.018 } else { -0.008 })
        .line_clamp(lines)
        .text_ellipsis()
}

pub(in crate::ui) fn cover(url: Option<&str>, fallback: Option<Seed>, size: HeroSize) -> Div {
    let radius = (size.cover * 0.035).clamp(8., 16.);
    fluid_cover_slot(size.cover, radius)
        .child(artwork::thumbnail(url, fallback, px(radius), true))
        .child(
            // A hairline keeps the large cover from dissolving into the field
            // without reading as an outline.
            div()
                .absolute()
                .inset_0()
                .rounded(px(radius))
                .border_1()
                .border_color(Hsla::from(rgb(tokens::ACCENT)).alpha(0.10)),
        )
}

/// The uppercase section marker above a display title.
pub(in crate::ui) fn eyebrow(label: impl Into<SharedString>, ink: Ink, compact: bool) -> Div {
    let label = label.into();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(if compact { 10. } else { 11. }))
        .line_height(px(if compact { 12. } else { 14. }))
        .font_weight(FontWeight::MEDIUM)
        .letter_spacing(if compact { 0.12 } else { 0.18 })
        .text_color(ink.secondary)
        .when(!compact, |row| {
            row.child(
                div()
                    .w(px(20.))
                    .h(px(1.))
                    .flex_none()
                    .bg(ink.secondary.opacity(0.55)),
            )
        })
        .child(label.as_ref().to_uppercase())
}

/// A quiet catalogue column that turns bare metadata into spatial composition.
pub(in crate::ui) fn catalogue(rows: Vec<(&'static str, String)>, ink: Ink) -> Div {
    div()
        .flex_none()
        .w(px(196.))
        .pl(px(24.))
        .border_l_1()
        .border_color(ink.rule.opacity(0.55))
        .flex()
        .flex_col()
        .gap(px(16.))
        .children(rows.into_iter().map(|(label, value)| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .letter_spacing(0.16)
                        .text_color(ink.secondary)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(20.))
                        .line_height(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ink.primary)
                        .child(value),
                )
        }))
}

pub(in crate::ui) fn duration_label(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{} ч {} мин", seconds / 3600, seconds / 60 % 60)
    } else {
        format!("{} мин", (seconds / 60).max(1))
    }
}

pub(in crate::ui) fn group_number(value: u64) -> String {
    let digits = value.to_string();
    let first = (digits.len() - 1) % 3 + 1;
    let mut number = digits[..first].to_owned();
    for group in digits.as_bytes()[first..].chunks(3) {
        number.push('\u{202f}');
        number.extend(group.iter().copied().map(char::from));
    }
    number
}

pub(in crate::ui) fn monthly_audience(details: &ArtistDetails) -> Option<String> {
    details.monthly_listeners.map_or_else(
        || details.monthly_listeners_caption.clone(),
        |count| Some(format!("{} слушателей за месяц", group_number(count))),
    )
}

#[derive(IntoElement)]
pub(super) struct PagePrimary {
    button: Button,
    optics: bool,
    artwork: Option<SharedString>,
}

pub(super) fn page_primary(button: Button, optics: bool, artwork: Option<&str>) -> PagePrimary {
    PagePrimary {
        button,
        optics,
        artwork: artwork.map(SharedString::from),
    }
}

impl RenderOnce for PagePrimary {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.optics || !effects_enabled() {
            return self.button;
        }
        // The page's own artwork drives the accent; the palette is the fallback.
        let accent = self
            .artwork
            .as_deref()
            .and_then(|url| cover_colors(url, window, cx).flatten())
            .and_then(|colors| colors.swatches().first().copied())
            .map_or_else(
                || materials::accent(cx),
                |(color, _)| Hsla::from(rgb(color)),
            );
        let porcelain = Material::new(Kind::Porcelain, accent);
        self.button.backgrounds(
            porcelain.style(Interaction::Rest).face,
            porcelain.style(Interaction::Hover).face,
            porcelain.style(Interaction::Pressed).face,
        )
    }
}

pub(in crate::ui) fn primary_icon(icon: IconName) -> guise::Icon {
    guise::Icon::new(icon)
        .pixel_size(px(16.))
        .color(Hsla::from(rgb(tokens::CANVAS)))
}

pub(in crate::ui) fn secondary_icon(
    button: guise::ActionIcon,
    material: Material,
) -> guise::ActionIcon {
    // Editorial secondary action: the icon sits on the page, the wash appears
    // only under the pointer.
    button.surface_style(material.icon_surface(Tone::Quiet))
}

pub(super) fn radio_action(
    id: &'static str,
    label: &'static str,
    order: isize,
    compact: bool,
    material: Material,
    provider: Option<ProviderId>,
) -> Stateful<Div> {
    let control = button(id, label, order)
        .h(px(size::MD))
        .flex_none()
        .justify_center()
        .gap(px(space::SM))
        .when(compact, |control| control.w(px(size::MD)))
        .when(!compact, |control| control.px(px(space::MD)))
        .text_size(px(text::BODY))
        .font_weight(FontWeight::MEDIUM)
        .tooltip(guise::tooltip(label))
        .child(guise::Icon::new(IconName::Radio).pixel_size(px(16.)))
        .when(!compact, |control| {
            control
                .child(label)
                .children(provider.map(|source| provider_mark(source, 14.)))
        });
    ControlVariant::SecondaryButton.paint(control, material, ControlState::default())
}
