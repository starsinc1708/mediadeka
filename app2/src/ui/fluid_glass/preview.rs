//! Live preview card for the appearance settings and the shader gallery.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, App, Context, Corners, Div, Pixels, Role, Stateful, Window, canvas,
    div, prelude::*, px, rgb,
};

#[cfg(feature = "capture")]
use crate::visual::VisualFlags;
use crate::{
    ui::MainView,
    visual::{PatternId, PatternParams, PatternSelection, QualityChoice, VisualSettings},
};

#[cfg(feature = "capture")]
use super::DebugPass;
use super::{
    Backdrop, contextual::PatternMix, graphics, palette::Palette, quality::Quality, tokens,
};

const LOOP: Duration = Duration::from_secs(24);

fn mix(selection: PatternSelection, seed: u32, visual: VisualSettings) -> PatternMix {
    let pattern = match selection {
        PatternSelection::Auto => PatternId::ThermalFlow,
        PatternSelection::Fixed(pattern) => pattern,
    };
    PatternMix {
        a: pattern,
        b: pattern,
        mix: 0.,
        params: PatternParams::for_plan(pattern, seed, visual.intensity, visual.speed),
    }
}

/// Paints one pattern into a rounded card. The palette is the one the shell is
/// currently using, so Auto follows the current artwork instead of guessing.
pub(in crate::ui) fn card(
    palette: Palette,
    visual: VisualSettings,
    auto: PatternId,
    seed: u32,
    reduced_motion: bool,
    cx: &App,
) -> Stateful<Div> {
    let selection = match visual.pattern {
        PatternSelection::Auto => PatternSelection::Fixed(auto),
        selection => selection,
    };
    let pattern = mix(selection, seed, visual);
    let flags = visual.effects;
    let quality = Quality::preview(visual.quality);
    let high_contrast = crate::ui::theme::fluid_high_contrast(cx);
    let flat = high_contrast || visual.surface.neutral || !super::effects_enabled();
    let animate = !flat && !reduced_motion && visual.quality != QualityChoice::Static;
    let label = if flat {
        if high_contrast {
            "Высокий контраст"
        } else {
            "Нейтральный фон"
        }
        .to_owned()
    } else {
        match visual.pattern {
            PatternSelection::Auto => format!("Авто · {}", auto.label()),
            fixed => fixed.label().to_owned(),
        }
    };
    let mode = if animate {
        "Динамический"
    } else {
        "Статичный"
    };
    let palette_label = crate::ui::appearance::palette(cx).label();
    let paint = move |bounds: gpui::Bounds<Pixels>, window: &mut Window, seconds: f32| {
        let Some(shader) = graphics::preview_shader() else {
            return;
        };
        let instance = graphics::studio::live_preview(
            palette,
            pattern,
            flags,
            quality,
            seconds,
            animate,
            visual.surface,
        );
        window.paint_shader(bounds, Corners::all(px(14.)), shader, instance);
    };
    div()
        .id("fluid-glass-live-preview-card")
        .accessibility_id("fluid-glass-live-preview-card")
        .role(Role::Group)
        .aria_label("Живой предпросмотр Fluid Glass")
        .relative()
        .w_full()
        .h(px(196.))
        .rounded(px(14.))
        .overflow_hidden()
        .bg(rgb(if high_contrast {
            tokens::contrast::CANVAS
        } else {
            tokens::CANVAS
        }))
        .when(flat, |card| {
            card.border_1().border_color(rgb(if high_contrast {
                tokens::contrast::RULE
            } else {
                tokens::RULE
            }))
        })
        .when(!flat, |card| {
            card.child(if animate {
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| paint(bounds, window, 0.),
                )
                .size_full()
                .with_animation(
                    "fluid-glass-live-preview",
                    Animation::new(LOOP).repeat(),
                    move |_, t| {
                        let seconds = t * LOOP.as_secs_f32();
                        canvas(
                            |_, _, _| (),
                            move |bounds, (), window, _| paint(bounds, window, seconds),
                        )
                        .size_full()
                    },
                )
                .into_any_element()
            } else {
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| paint(bounds, window, 0.),
                )
                .size_full()
                .into_any_element()
            })
        })
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .px(px(14.))
                .pt(px(28.))
                .pb(px(11.))
                .flex()
                .items_end()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .text_size(px(12.))
                        .text_color(gpui::white())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(label))
                        .when(!flat, |caption| {
                            caption.child(div().text_color(gpui::white().opacity(0.72)).child(
                                format!(
                                    "{palette_label} · {}",
                                    quality_preview_label(visual.quality)
                                ),
                            ))
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(gpui::white().opacity(0.62))
                        .child(mode),
                ),
        )
}

pub(in crate::ui) fn quality_preview_label(choice: QualityChoice) -> &'static str {
    match choice {
        QualityChoice::Auto => "Авто → High",
        QualityChoice::Ultra => "Ultra",
        QualityChoice::High => "High",
        QualityChoice::Balanced => "Balanced",
        QualityChoice::Eco => "Eco",
        QualityChoice::Static => "Static",
    }
}

impl MainView {
    #[cfg(feature = "capture")]
    pub(in crate::ui) fn shader_debug_tile(
        &self,
        visual: VisualSettings,
        flags: VisualFlags,
        pass: DebugPass,
        seconds: f32,
        fixed_palette: bool,
        cx: &Context<Self>,
    ) -> (Div, String) {
        let backdrop = self.fluid_backdrop.as_ref().map(|view| view.read(cx));
        let selection = match visual.pattern {
            PatternSelection::Auto => PatternSelection::Fixed(
                backdrop.map_or(PatternId::ThermalFlow, Backdrop::preview_auto),
            ),
            fixed => fixed,
        };
        let pattern = mix(selection, 0, visual);
        let palette = if fixed_palette {
            crate::appearance::GlassPalette::Solar.palette(None)
        } else {
            backdrop.map_or_else(
                || crate::ui::appearance::palette(cx).palette(None),
                Backdrop::preview_palette,
            )
        };
        let quality = Quality::preview(visual.quality);
        graphics::studio::tile(palette, pattern, flags, quality, seconds, pass)
    }

    /// The live preview card reads the palette the backdrop has resolved, so
    /// an automatic palette shows the same light as the shell.
    pub(in crate::ui) fn fluid_glass_preview(
        &self,
        visual: VisualSettings,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let backdrop = self
            .fluid_backdrop
            .as_ref()
            .map(|backdrop| backdrop.read(cx));
        let palette = backdrop
            .map(|backdrop| backdrop.preview_palette())
            .unwrap_or_else(|| crate::ui::appearance::palette(cx).palette(None));
        let auto = backdrop.map_or(PatternId::ThermalFlow, |backdrop| backdrop.preview_auto());
        let seed = backdrop.map_or(0, |backdrop| backdrop.preview_seed());
        card(palette, visual, auto, seed, cx.reduce_motion(), cx)
    }

    pub(in crate::ui) fn fluid_gallery_thumb(
        &self,
        pattern: PatternId,
        visual: VisualSettings,
        seconds: f32,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let palette = self
            .fluid_backdrop
            .as_ref()
            .map(|backdrop| backdrop.read(cx).preview_palette())
            .unwrap_or_else(|| crate::ui::appearance::palette(cx).palette(None));
        thumbnail(palette, pattern, visual, seconds)
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn fluid_frame_interval(&self, cx: &Context<Self>) -> f32 {
        self.fluid_backdrop
            .as_ref()
            .map_or(0., |backdrop| backdrop.read(cx).frame_ms())
    }
}

/// A still frame of one pattern for the shader gallery. It is the same GPU
/// shader, not a cached image; only the time is fixed so nine cards stay light.
pub(in crate::ui) fn thumbnail(
    palette: Palette,
    pattern: PatternId,
    visual: VisualSettings,
    seconds: f32,
) -> Stateful<Div> {
    let mix = mix(PatternSelection::Fixed(pattern), 0, visual);
    let flags = visual.effects;
    let quality = Quality::preview(visual.quality);
    div()
        .id(gpui::ElementId::Name(
            format!("fluid-glass-gallery-{}", pattern.as_str()).into(),
        ))
        .relative()
        .w_full()
        .h(px(88.))
        .rounded(px(8.))
        .overflow_hidden()
        .bg(rgb(tokens::SURFACE))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let Some(shader) = graphics::preview_shader() else {
                        return;
                    };
                    let instance =
                        graphics::preview_instance(palette, mix, flags, quality, seconds);
                    window.paint_shader(bounds, Corners::all(px(8.)), shader, instance);
                },
            )
            .size_full(),
        )
}

impl Backdrop {
    #[cfg(feature = "capture")]
    pub(super) fn frame_ms(&self) -> f32 {
        self.frame_ms
    }

    pub(super) fn preview_palette(&self) -> Palette {
        self.last_field.player
    }

    pub(super) fn preview_auto(&self) -> PatternId {
        self.contextual
            .auto_pattern(self.scene != super::Scene::Workspace)
    }

    pub(super) fn preview_seed(&self) -> u32 {
        self.contextual.plan_seed()
    }

    #[cfg(feature = "capture")]
    pub(super) fn last_pattern(&self) -> PatternMix {
        self.last_pattern
    }
}
