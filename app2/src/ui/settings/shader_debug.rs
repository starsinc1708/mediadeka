use gpui::{App, Context, Div, Role, Window, div, prelude::*, px};
use guise::theme;

use crate::{
    ui::{
        MainView,
        components::{
            Activate, button,
            glass_control::Tone,
            overlay::{Select, SelectChoice, SelectLabel},
        },
        fluid_glass::{DEBUG_EFFECTS, DebugPass, comparison_flags},
    },
    visual::{VisualFlags, VisualSettings},
};

const PHASES: [f32; 3] = [0., 2.5, 7.5];
#[cfg(feature = "capture")]
const DEBUG_STUDIO_SPAN: gpui::SpanId = 0x6d65646961736473;

pub(in crate::ui) struct ShaderDebug {
    expanded: bool,
    effect: usize,
    pass: DebugPass,
    phase: usize,
    fixed_palette: bool,
    effect_menu: Select,
    pass_menu: Select,
}

impl ShaderDebug {
    pub(in crate::ui) fn new(cx: &App) -> Self {
        Self {
            expanded: false,
            effect: 0,
            pass: DebugPass::Composite,
            phase: 2,
            fixed_palette: true,
            effect_menu: Select::new(922, cx),
            pass_menu: Select::new(930, cx),
        }
    }
}

impl MainView {
    #[cfg(feature = "capture")]
    pub fn reveal_shader_debug_for_capture(&self, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(bounds) = window.rendered_span(DEBUG_STUDIO_SPAN) else {
            return false;
        };
        let target = bounds.top() - px(210.) - self.scroll.offset().y;
        self.set_home_scroll_offset_for_capture(f32::from(target), cx);
        true
    }

    pub(super) fn shader_debug_studio(
        &self,
        visual: VisualSettings,
        interactive: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let studio = &self.shader_debug;
        let material = self.glass_controls(cx);
        let control = |id, label, order| {
            let item = button(id, label, order)
                .h(px(32.))
                .px(px(12.))
                .rounded(px(8.))
                .text_size(px(12.5))
                .child(label)
                .tab_stop(interactive);
            match material {
                Some(glass) => glass.paint_enabled(item, Tone::Quiet, interactive),
                None => item,
            }
        };
        let toggle = control("shader-debug-toggle", "Shader Debug Studio", 920)
            .aria_expanded(studio.expanded)
            .when(interactive, |item| {
                item.on_activate(cx.listener(|view, _, _, cx| {
                    let studio = &mut view.shader_debug;
                    studio.expanded = !studio.expanded;
                    studio.effect_menu.reset();
                    studio.pass_menu.reset();
                    cx.notify();
                }))
            });
        let section = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .child(toggle);
        if !studio.expanded {
            return section;
        }
        let reduced = self.reduced_motion || self.fluid_high_contrast(cx);
        let (flag, effect_label) = DEBUG_EFFECTS[studio.effect];
        let selectors = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                studio.effect_menu.view(
                    SelectLabel {
                        trigger: "shader-debug-effect",
                        menu: "shader-debug-effects",
                        label: "Эффект для сравнения",
                        value: effect_label.into(),
                        enabled: interactive,
                    },
                    DEBUG_EFFECTS
                        .iter()
                        .enumerate()
                        .map(|(index, (flag, label))| {
                            SelectChoice::new(
                                format!("shader-debug-effect-{flag}"),
                                *label,
                                index == studio.effect,
                            )
                        })
                        .collect(),
                    |view| &mut view.shader_debug.effect_menu,
                    |view, index, _, _| view.shader_debug.effect = index,
                    material,
                    !reduced,
                    window,
                    cx,
                ),
            )
            .child(
                studio.pass_menu.view(
                    SelectLabel {
                        trigger: "shader-debug-pass",
                        menu: "shader-debug-passes",
                        label: "Диагностическое представление",
                        value: studio.pass.label().into(),
                        enabled: interactive,
                    },
                    DebugPass::ALL
                        .iter()
                        .enumerate()
                        .map(|(index, pass)| {
                            SelectChoice::new(
                                format!("shader-debug-pass-{index}"),
                                pass.label(),
                                *pass == studio.pass,
                            )
                        })
                        .collect(),
                    |view| &mut view.shader_debug.pass_menu,
                    |view, index, _, _| view.shader_debug.pass = DebugPass::ALL[index],
                    material,
                    !reduced,
                    window,
                    cx,
                ),
            );
        let phase_label = ["Фаза 0 с", "Фаза 2,5 с", "Фаза 7,5 с"][studio.phase];
        let palette_label = if studio.fixed_palette {
            "Solar (тест)"
        } else {
            "Текущая палитра"
        };
        let selectors = selectors
            .child(
                control("shader-debug-phase", phase_label, 938).when(interactive, |item| {
                    item.on_activate(cx.listener(|view, _, _, cx| {
                        view.shader_debug.phase = (view.shader_debug.phase + 1) % PHASES.len();
                        cx.notify();
                    }))
                }),
            )
            .child(
                control("shader-debug-palette", palette_label, 939).when(interactive, |item| {
                    item.on_activate(cx.listener(|view, _, _, cx| {
                        view.shader_debug.fixed_palette = !view.shader_debug.fixed_palette;
                        cx.notify();
                    }))
                }),
            );
        let (before, after) = comparison_flags(flag);
        let tile = |label, flags| {
            let (view, uniforms) = self.shader_debug_tile(
                visual,
                flags,
                studio.pass,
                PHASES[studio.phase],
                studio.fixed_palette,
                cx,
            );
            (
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .child(label)
                    .child(view),
                uniforms,
            )
        };
        let (before, _) = tile("До", before);
        let (after, uniforms) = tile("После", after);
        let palette = theme(cx);
        let note = match flag {
            VisualFlags::DISPERSION => Some("В обоих кадрах включено преломление."),
            VisualFlags::GLITCH => {
                Some("Сцена использует фиксированный импульс. В Eco и Static глитч отключён.")
            }
            _ => None,
        };
        section
            .child(selectors)
            .child(
                div()
                    .id("shader-debug-comparison")
                    .accessibility_id("shader-debug-comparison")
                    .role(Role::Group)
                    .aria_label("Сравнение эффекта до и после")
                    .w_full()
                    .max_w(px(720.))
                    .relative()
                    .flex()
                    .gap(px(8.))
                    .child(before)
                    .child(after)
                    .when(cfg!(feature = "capture"), |view| {
                        view.children(self.shader_debug_capture_span())
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.dimmed())
                    .child(uniforms)
                    .children(note),
            )
    }

    fn shader_debug_capture_span(&self) -> Option<gpui::AnyElement> {
        #[cfg(feature = "capture")]
        return Some(
            gpui::canvas(
                |_, _, _| (),
                |bounds, (), window, _| window.with_paint_span(DEBUG_STUDIO_SPAN, bounds, |_| {}),
            )
            .absolute()
            .size_full()
            .into_any_element(),
        );
        #[cfg(not(feature = "capture"))]
        None
    }
}
