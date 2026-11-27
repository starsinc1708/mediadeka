use std::time::Duration;

use gpui::{Context, Window};

use super::audio::ENVELOPE_POINTS;
use crate::ui::MainView;

use super::{Geometry, Sample, graphics};
use crate::{appearance::GlassPalette, visual::VisualSettings};

impl MainView {
    pub fn sample_presentation_for_capture(&mut self, time: Duration, cx: &mut Context<Self>) {
        self.capture_time = Some(time);
        cx.notify();
    }

    pub fn set_fluid_shader_diagnostics_for_capture(
        &self,
        pass: usize,
        test_pulse: bool,
        cx: &mut Context<Self>,
    ) {
        graphics::studio::set_capture(graphics::studio::Pass::ALL[pass], test_pulse, cx);
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |_, cx| cx.notify());
        }
    }

    pub fn fluid_glass_geometry_for_capture(&self, window: &Window) -> [f32; 7] {
        let geometry = Geometry::for_window(window);
        [
            geometry.inset(),
            geometry.rail(),
            geometry.player(),
            geometry.inner_radius,
            geometry.margin(),
            geometry.content_bottom(),
            geometry.content_width(window.viewport_size().width.into()),
        ]
    }

    pub fn set_fluid_glass_full_edge_for_capture(&self, full: bool, cx: &mut Context<Self>) {
        graphics::set_full_edge_for_capture(full, cx);
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |_, cx| cx.notify());
        }
    }

    pub fn fluid_glass_edge_work_for_capture(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> (usize, u64, u64) {
        let dock = self
            .fluid_backdrop
            .as_ref()
            .map_or(0., |backdrop| backdrop.read(cx).dock.width);
        graphics::edge::work(Geometry::for_window(window), dock, window)
    }

    pub fn fluid_glass_quality_for_capture(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> Option<(&'static str, f32, u32, bool)> {
        let backdrop = self.fluid_backdrop.as_ref()?.read(cx);
        let quality = backdrop.quality.effective(window, cx.reduce_motion());
        backdrop.has_optics(cx).then(|| {
            (
                quality.name(),
                quality.raster_scale(),
                quality.frames_per_second(),
                quality.reacts_to_audio(),
            )
        })
    }

    /// Applies a palette and the independent visual layers without touching the
    /// saved profile, for the native pattern and palette matrix.
    pub fn set_visual_for_capture(
        &mut self,
        palette: GlassPalette,
        visual: VisualSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.palette = palette;
        self.appearance.visual = visual;
        crate::ui::appearance::set_palette(cx, palette);
        crate::ui::appearance::set_visual(cx, visual);
        let artwork = self.player.read(cx).current_artwork().map(str::to_owned);
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.set_palette(palette, artwork.as_deref(), cx);
                backdrop.set_visual(visual, window, cx);
            });
        }
        cx.notify();
    }

    pub fn visual_for_capture(&self) -> VisualSettings {
        self.appearance.visual
    }

    pub fn appearance_error_for_capture(&self) -> Option<&str> {
        self.appearance_error.as_deref()
    }

    pub fn fluid_frame_interval_for_capture(&self, cx: &gpui::App) -> f32 {
        self.fluid_backdrop
            .as_ref()
            .map_or(0., |backdrop| backdrop.read(cx).frame_ms())
    }

    pub fn fluid_icon_motion_active_for_capture(&self, cx: &gpui::App) -> bool {
        crate::ui::design_system::icons::active(cx)
    }

    pub fn fluid_glass_pattern_for_capture(
        &self,
        cx: &gpui::App,
    ) -> Option<(&'static str, &'static str, f32, f32, u32)> {
        let backdrop = self.fluid_backdrop.as_ref()?.read(cx);
        let pattern = backdrop.last_pattern();
        Some((
            pattern.a.as_str(),
            pattern.b.as_str(),
            pattern.mix,
            pattern.params.intensity,
            pattern.params.seed,
        ))
    }

    pub fn set_fluid_high_contrast_for_capture(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.set_fluid_high_contrast(enabled, cx);
    }

    pub fn start_fluid_glass_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.start_desktop(std::time::Instant::now(), window, cx);
    }

    pub fn sample_fluid_glass_for_capture(&mut self, time: Duration, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.sample(
                    Sample {
                        wall: time,
                        active: None,
                        live_audio: false,
                        shader_phase: None,
                    },
                    cx,
                );
            });
        }
    }

    pub fn sample_fluid_glass_activity_for_capture(
        &mut self,
        elapsed: Duration,
        active: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.sample(
                    Sample {
                        wall: elapsed,
                        active: Some(active),
                        live_audio: false,
                        shader_phase: None,
                    },
                    cx,
                );
            });
        }
    }

    pub fn sample_fluid_glass_audio_for_capture(&mut self, wall: Duration, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.sample(
                    Sample {
                        wall,
                        active: Some(true),
                        live_audio: true,
                        shader_phase: None,
                    },
                    cx,
                );
            });
        }
    }

    pub fn fluid_glass_audio_for_capture(&self, cx: &gpui::App) -> [f32; 3] {
        self.fluid_backdrop
            .as_ref()
            .map_or([0.; 3], |backdrop| backdrop.read(cx).audio.bands())
    }

    pub fn fluid_glass_audio_status_for_capture(
        &self,
        cx: &gpui::App,
    ) -> Option<(&'static str, bool)> {
        let backdrop = self.fluid_backdrop.as_ref()?.read(cx);
        Some((
            backdrop.audio.capture_status(),
            backdrop.process_audio_playing,
        ))
    }

    pub fn fluid_glass_waveform_for_capture(
        &self,
        cx: &gpui::App,
    ) -> (f32, [f32; ENVELOPE_POINTS]) {
        self.fluid_backdrop
            .as_ref()
            .and_then(|backdrop| backdrop.read(cx).audio.waveform())
            .map_or((0., [0.; ENVELOPE_POINTS]), |trace| {
                (trace.opacity, trace.envelope)
            })
    }

    pub fn show_fluid_glass_waveform_for_capture(&self, visible: bool, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.hide_waveform_for_capture = !visible;
                cx.notify();
            });
        }
    }

    pub fn fluid_glass_row_launch_origin_for_capture(&self, cx: &gpui::App) -> Option<[f32; 2]> {
        let point = self.fluid_backdrop.as_ref()?.read(cx).row_launch.origin()?;
        Some([point.x.into(), point.y.into()])
    }

    pub fn show_fluid_glass_row_launch_for_capture(&self, visible: bool, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.hide_row_launch_for_capture = !visible;
                cx.notify();
            });
        }
    }

    pub fn sample_fluid_glass_palette_for_capture(
        &mut self,
        wall: Duration,
        shader_phase: Duration,
        cx: &mut Context<Self>,
    ) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.sample(
                    Sample {
                        wall,
                        active: None,
                        live_audio: false,
                        shader_phase: Some(shader_phase),
                    },
                    cx,
                );
            });
        }
    }

    pub fn resume_fluid_glass_for_capture(&self, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                backdrop.sample = None;
                cx.notify();
            });
        }
    }

    pub fn sample_fluid_glass_track_change_for_capture(
        &mut self,
        wall: Duration,
        shader_phase: Duration,
        cx: &mut Context<Self>,
    ) {
        self.sample_fluid_glass_palette_for_capture(wall, shader_phase, cx);
        self.player.update(cx, |player, cx| {
            player.sample_fluid_cover_for_capture(wall, cx)
        });
    }

    pub fn fluid_glass_palette_ready_for_capture(&self, url: &str, cx: &gpui::App) -> bool {
        self.fluid_backdrop
            .as_ref()
            .is_some_and(|backdrop| backdrop.read(cx).cover.settled_for(url))
    }

    pub fn fluid_glass_target_colors_for_capture(&self, cx: &gpui::App) -> [[f32; 3]; 4] {
        self.fluid_backdrop
            .as_ref()
            .map_or([[0.; 3]; 4], |backdrop| {
                backdrop
                    .read(cx)
                    .target
                    .colors
                    .map(|color| [color.r, color.g, color.b])
            })
    }

    pub fn fluid_glass_page_palette_for_capture(&self, cx: &gpui::App) -> Option<[[f32; 3]; 4]> {
        let backdrop = self.fluid_backdrop.as_ref()?.read(cx);
        Some(
            backdrop
                .contextual
                .palette()?
                .colors
                .map(|color| [color.r, color.g, color.b]),
        )
    }

    pub fn set_fluid_glass_cover_for_capture(&self, url: &str, cx: &mut Context<Self>) {
        self.player
            .update(cx, |player, cx| player.set_artwork_for_capture(url, cx));
    }

    pub fn uses_fluid_glass_for_capture(&self) -> bool {
        self.uses_fluid_glass()
    }

    pub fn saved_appearance_for_capture(&self) -> (&'static str, &'static str) {
        (
            self.appearance.shell.as_str(),
            self.appearance.palette.as_str(),
        )
    }

    pub fn fluid_glass_theme_for_capture(&self, cx: &gpui::App) -> Option<&'static str> {
        self.fluid_backdrop
            .as_ref()
            .map(|backdrop| backdrop.read(cx).theme.as_str())
    }
}
