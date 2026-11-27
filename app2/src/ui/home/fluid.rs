//! The first screen of the Fluid Glass home, above the existing music shelves.

use gpui::{AnyElement, Context, Div, FontWeight, Role, Stateful, div, prelude::*, px, rgb};
use guise::{Icon, IconName};

use crate::ui::{
    assets::provider_mark,
    components::{glass_control::Tone, music::fluid_cover},
    design_system::controls::{ControlState, ControlVariant},
    fluid_glass::tokens::{self, space},
};

use super::{Activate, Layout, MainView, Page, button, listening::Tile};

const SPOTLIGHT_WIDTH: f32 = 820.;
const SPOTLIGHT_ACTION_GROUP: &str = "home-spotlight-action";

pub(super) fn heading(layout: Layout) -> Stateful<Div> {
    div()
        .id("home-title")
        .accessibility_id("home-title")
        .role(Role::Heading)
        .aria_label("Главная")
        .min_w_0()
        .truncate()
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(if layout.short {
            tokens::text::PAGE_TITLE_COMPACT
        } else {
            tokens::text::PAGE_TITLE
        }))
        .child("Главная")
}

pub(super) fn section_heading(title: &'static str) -> AnyElement {
    div()
        .id(format!("fluid-home-section-{title}"))
        .role(Role::Heading)
        .aria_label(title)
        .min_w_0()
        .truncate()
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(tokens::text::SECTION_TITLE))
        .line_height(px(28.))
        .child(title)
        .into_any_element()
}

pub(super) fn spotlight(
    control: Stateful<Div>,
    layout: Layout,
    mut content: Tile<'_>,
    subtitle: &'static str,
    action: &'static str,
    progress: Option<Stateful<Div>>,
    loading: bool,
) -> Stateful<Div> {
    let (title_label, detail_label) = content.labels();
    let compact = layout.short || layout.content_width < 500.;
    let edge = if compact { 88. } else { 176. };
    let control = control
        .group(SPOTLIGHT_ACTION_GROUP)
        .w_full()
        .max_w(px(SPOTLIGHT_WIDTH))
        .rounded(px(8.))
        .gap(px(if compact { space::LG } else { space::XL }))
        .p(px(if compact { space::XS } else { space::SM }));
    let control = if let Some(material) = layout.glass {
        material.paint(control, Tone::Quiet)
    } else {
        control
    };
    control
        .child(fluid_cover(content.artwork, content.fallback, edge, 6.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(if compact { 3. } else { 7. }))
                .child(
                    div()
                        .text_size(px(tokens::text::CAPTION))
                        .truncate()
                        .child(subtitle),
                )
                .child(
                    div()
                        .text_color(rgb(tokens::PRIMARY))
                        .text_size(px(if compact { 23. } else { 40. }))
                        .line_height(px(if compact { 28. } else { 46. }))
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_clamp(if compact { 1 } else { 2 })
                        .text_ellipsis()
                        .child(title_label),
                )
                .when(!content.detail.is_empty(), |details| {
                    details.child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(px(6.))
                            .children(
                                content
                                    .provider
                                    .map(|provider| provider_mark(provider, 13.)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(tokens::text::BODY))
                                    .child(detail_label),
                            ),
                    )
                })
                .children(progress)
                .child(spotlight_action(action, compact, loading, layout)),
        )
}

fn spotlight_action(
    action: &'static str,
    compact: bool,
    loading: bool,
    layout: Layout,
) -> Stateful<Div> {
    let mut label = div()
        .id("home-spotlight-cta-face")
        .self_start()
        .h(px(if compact { 28. } else { 36. }))
        .px(px(space::MD))
        .flex()
        .items_center()
        .gap(px(space::SM))
        .text_size(px(tokens::text::BODY))
        .font_weight(FontWeight::SEMIBOLD)
        .child(
            Icon::new(if loading {
                IconName::LoaderCircle
            } else {
                IconName::Play
            })
            .pixel_size(px(16.)),
        )
        .child(action);
    if let Some(material) = layout.glass {
        let state = ControlState {
            loading,
            ..Default::default()
        };
        let rest = ControlVariant::DialogAction.resolve(material, state);
        let hover = ControlVariant::DialogAction.resolve(
            material,
            ControlState {
                hovered: true,
                ..state
            },
        );
        let down = ControlVariant::DialogAction.resolve(
            material,
            ControlState {
                pressed: true,
                ..state
            },
        );
        label = label
            .rounded(px(rest.radius))
            .bg(rest.bg)
            .text_color(rest.fg)
            .group_hover(SPOTLIGHT_ACTION_GROUP, move |label| label.bg(hover.bg))
            .group_active(SPOTLIGHT_ACTION_GROUP, move |label| label.bg(down.bg));
    }
    label
}

impl MainView {
    pub(super) fn home_library_intro(&self, layout: Layout, cx: &Context<Self>) -> Div {
        let open = button("home-open-library", "Открыть медиатеку", 37)
            .self_start()
            .gap(px(7.))
            .h(px(36.))
            .px(px(10.))
            .rounded(px(6.))
            .text_size(px(tokens::text::BODY))
            .child(Icon::new(IconName::Music).pixel_size(px(16.)))
            .child("Открыть медиатеку");
        let open = if let Some(material) = layout.glass {
            ControlVariant::SecondaryButton.paint(open, material, ControlState::default())
        } else {
            open
        };
        div().flex().child(
            open.flex_none()
                .on_activate(cx.listener(|view, _, window, cx| {
                    view.show_music_page(Page::Tracks, window, cx);
                })),
        )
    }
}
