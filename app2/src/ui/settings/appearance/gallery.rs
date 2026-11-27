//! Capture-only samples used to compare the shared controls and their motion.

use gpui::{Context, Div, FontWeight, Role, div, prelude::*, px};
use guise::theme;

use super::{PALETTE_ORDER, rows, transport};
use crate::ui::{
    MainView,
    components::Activate,
    design_system::{
        controls::{self, ControlVariant},
        icons::{self, Glyph, GlyphState},
    },
};

const GALLERY_IDS: [&str; Glyph::ALL.len()] = [
    "appearance-icon-play",
    "appearance-icon-shuffle",
    "appearance-icon-repeat",
    "appearance-icon-repeat-one",
    "appearance-icon-like",
    "appearance-icon-queue",
    "appearance-icon-more",
    "appearance-icon-fullscreen",
    "appearance-icon-next",
    "appearance-icon-previous",
    "appearance-icon-volume",
    "appearance-icon-mute",
    "appearance-icon-filter",
    "appearance-icon-sort",
    "appearance-icon-pin",
    "appearance-icon-copy",
    "appearance-icon-lyrics",
    "appearance-icon-dropdown",
];
const GALLERY_LABELS: [&str; Glyph::ALL.len()] = [
    "Play/Pause",
    "Shuffle",
    "Repeat",
    "Repeat one",
    "Like",
    "Queue",
    "More",
    "Fullscreen",
    "Next",
    "Previous",
    "Volume",
    "Mute",
    "Filter",
    "Sort",
    "Pin",
    "Copy",
    "Текст",
    "Селектор",
];
const GALLERY_DISABLED_IDS: [&str; 4] = [
    "appearance-icon-disabled-shuffle",
    "appearance-icon-disabled-like",
    "appearance-icon-disabled-repeat",
    "appearance-icon-disabled-queue",
];
const GALLERY_LOADING_IDS: [&str; 4] = [
    "appearance-icon-loading-more",
    "appearance-icon-loading-copy",
    "appearance-icon-loading-filter",
    "appearance-icon-loading-pin",
];

impl MainView {
    /// The internal Animated Component Gallery. Every cell uses the same
    /// programmatic glyph painter as the real controls; clicking a cell
    /// retargets its global spring, so the whole set can be compared on the
    /// active palette and at the current window/DPI without a video pipeline.
    pub(super) fn icon_gallery_section(&self, compact: bool, cx: &Context<Self>) -> Div {
        let palette = theme(cx);
        let size = if compact { 34. } else { 42. };
        let mut grid = div()
            .id("appearance-icon-gallery")
            .accessibility_id("appearance-icon-gallery")
            .role(Role::Group)
            .aria_label("Галерея анимированных иконок")
            .flex()
            .flex_wrap()
            .gap(px(if compact { 8. } else { 14. }));
        for (index, glyph) in Glyph::ALL.into_iter().enumerate() {
            let active = self.icon_gallery[index];
            let one_shot = matches!(glyph, Glyph::Next | Glyph::Previous | Glyph::Copy);
            let button = icons::ghost_icon_button_with_state(
                GALLERY_IDS[index],
                GALLERY_LABELS[index],
                glyph,
                active,
                size,
                PALETTE_ORDER + 40 + index as isize,
                GlyphState::Rest,
                cx,
            )
            .on_click(cx.listener(move |view, _, window, cx| {
                view.icon_gallery[index] = !view.icon_gallery[index];
                if one_shot {
                    icons::pulse(window, cx, GALLERY_IDS[index]);
                }
                cx.notify();
            }));
            grid = grid.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .w(px(size + 20.))
                    .child(button)
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(palette.dimmed())
                            .child(GALLERY_LABELS[index]),
                    ),
            );
        }
        let mut samples = div().flex().flex_wrap().items_center().gap(px(10.));
        for (index, glyph) in [Glyph::Shuffle, Glyph::Like, Glyph::Repeat, Glyph::Queue]
            .into_iter()
            .enumerate()
        {
            samples = samples.child(icons::ghost_icon_button_with_state(
                GALLERY_DISABLED_IDS[index],
                "Недоступная иконка",
                glyph,
                false,
                30.,
                PALETTE_ORDER + 70 + index as isize,
                GlyphState::Disabled,
                cx,
            ));
        }
        for (index, glyph) in [Glyph::More, Glyph::Copy, Glyph::Filter, Glyph::Pin]
            .into_iter()
            .enumerate()
        {
            samples = samples.child(icons::ghost_icon_button_with_state(
                GALLERY_LOADING_IDS[index],
                "Иконка загружается",
                glyph,
                false,
                30.,
                PALETTE_ORDER + 80 + index as isize,
                GlyphState::Loading,
                cx,
            ));
        }
        let mut phases = div().flex().flex_wrap().items_center().gap(px(8.));
        for (glyph, progress) in [
            (Glyph::Like, 0.),
            (Glyph::Like, 0.5),
            (Glyph::Like, 1.),
            (Glyph::Copy, 0.),
            (Glyph::Copy, 1.),
        ] {
            phases = phases.child(icons::gallery_icon(glyph, 22., progress, palette.primary()));
        }
        let text_action = icons::text_action(
            "appearance-icon-text-action",
            if self.icon_gallery[0] {
                "Пауза"
            } else {
                "Слушать"
            },
            Some(Glyph::PlayPause),
            self.icon_gallery[0],
            13.,
            PALETTE_ORDER + 90,
            true,
            cx,
        )
        .on_click(cx.listener(|view, _, _, cx| {
            view.icon_gallery[0] = !view.icon_gallery[0];
            cx.notify();
        }));
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.dimmed())
                    .child("Анимированные иконки · нажмите, чтобы переключить состояние"),
            )
            .child(grid)
            .child(transport::gallery(self, compact, cx))
            .child(self.gallery_inputs.render(controls::material(cx), compact))
            .child(rows::gallery(cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(palette.dimmed())
                    .child("Фазы морфа: Like 0/50/100%, Copy 0/100%."),
            )
            .child(phases)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        icons::hero_play(
                            "appearance-icon-primary",
                            if self.icon_gallery[0] {
                                "Пауза"
                            } else {
                                "Слушать"
                            },
                            self.icon_gallery[0],
                            if compact { 44. } else { 56. },
                            PALETTE_ORDER + 89,
                            GlyphState::Rest,
                            cx,
                        )
                        .on_activate(cx.listener(|view, _, _, cx| {
                            view.icon_gallery[0] = !view.icon_gallery[0];
                            cx.notify();
                        })),
                    )
                    .child(text_action)
                    .child(icons::ghost_icon_button(
                        "appearance-icon-ghost",
                        "Ghost",
                        Glyph::More,
                        false,
                        30.,
                        PALETTE_ORDER + 91,
                        true,
                        cx,
                    ))
                    .child(icons::ghost_icon_button_with_state(
                        "appearance-icon-active-like",
                        "Активное сердце",
                        Glyph::Like,
                        true,
                        30.,
                        PALETTE_ORDER + 92,
                        GlyphState::Active,
                        cx,
                    ))
                    .child(
                        ControlVariant::DialogAction.dialog_button(
                            guise::Button::new("appearance-dialog-confirm", "Готово")
                                .tab_order(PALETTE_ORDER + 93)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.icon_gallery[0] = false;
                                    cx.notify();
                                })),
                            controls::material(cx),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(palette.dimmed())
                    .child("TextAction и Ghost; нижний ряд — Disabled и Loading."),
            )
            .child(samples)
    }
}
