use std::sync::{Arc, LazyLock};

use gpui::{Context, FontFeatures, IntoElement, WeakEntity, Window, div, prelude::*, px, rgb};

use super::PlayerBar;
use crate::{
    playback::Playback,
    ui::{
        components::{
            music::time_label,
            slider::{Slider, SliderPreview, slider, slider_with_preview},
        },
        design_system::materials,
        fluid_glass::tokens::{contrast, space, text},
        theme::{SECONDARY, fluid_high_contrast, playlist},
    },
};

static TABULAR_NUMERALS: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

pub(super) struct Progress {
    player: WeakEntity<PlayerBar>,
    state: State,
    preview: Option<f32>,
    #[cfg(feature = "capture")]
    track_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
}

#[derive(Default, PartialEq)]
struct State {
    position: f64,
    duration: Option<f64>,
    seekable: bool,
}

impl Progress {
    pub(super) fn new(player: WeakEntity<PlayerBar>) -> Self {
        Self {
            player,
            state: State::default(),
            preview: None,
            #[cfg(feature = "capture")]
            track_bounds: Default::default(),
        }
    }

    pub(super) fn sync(&mut self, playback: &Playback, cx: &mut Context<Self>) {
        let state = State {
            position: playback.position(),
            duration: playback.duration(),
            seekable: playback.seekable(),
        };
        if self.state != state {
            if !state.seekable || state.duration != self.state.duration {
                self.preview = None;
            }
            self.state = state;
            cx.notify();
        }
    }

    fn set_preview(&mut self, fraction: Option<f32>, cx: &mut Context<Self>) {
        if self.preview != fraction {
            self.preview = fraction;
            cx.notify();
        }
    }

    fn seek(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let player = self.player.clone();
        cx.defer(move |cx| {
            let _ = player.update(cx, |player, cx| player.seek(seconds, cx));
        });
    }

    #[cfg(feature = "capture")]
    pub(super) fn seek_for_capture(&self) -> (gpui::Bounds<gpui::Pixels>, Option<f64>) {
        (
            self.track_bounds.get(),
            self.preview
                .zip(self.state.duration)
                .map(|(fraction, duration)| f64::from(fraction) * duration),
        )
    }
}

impl Render for Progress {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let State {
            position,
            duration,
            seekable,
        } = self.state;
        let (playlist_presentation, fluid) =
            self.player.upgrade().map_or((false, false), |player| {
                let player = player.read(cx);
                (player.compact_chrome(), player.fluid_chrome())
            });
        let duration_label = duration.map_or_else(|| "—".to_owned(), time_label);
        let time_width = (duration_label.len() as f32 * 7.).max(34.);
        let seek = Slider {
            min: 0.,
            value: position,
            max: duration.unwrap_or(0.),
            step: 5.,
            enabled: seekable && (!fluid || duration.is_some_and(|duration| duration > 0.)),
            playlist_presentation,
            fluid,
        };
        let seek_id = "player-progress";
        let seek_title = "Позиция воспроизведения";
        let position_preview = self
            .preview
            .zip(duration)
            .filter(|_| fluid && seek.enabled)
            .map(|(fraction, duration)| f64::from(fraction) * duration);
        let slider = if fluid {
            slider_with_preview(
                seek_id,
                seek_title,
                606,
                seek,
                Self::seek,
                SliderPreview {
                    fraction: self.preview,
                    update: Self::set_preview,
                },
                cx,
            )
            .into_any_element()
        } else {
            slider(seek_id, seek_title, 606, seek, Self::seek, cx).into_any_element()
        };
        let row = div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(if fluid { space::XS } else { 6. }))
            .text_color(rgb(if playlist_presentation {
                playlist::INK_SECONDARY
            } else {
                SECONDARY
            }))
            .text_size(px(if fluid {
                text::CAPTION
            } else if playlist_presentation {
                11.
            } else {
                10.
            }))
            .when(fluid, |progress| {
                progress.font_features(TABULAR_NUMERALS.clone())
            })
            .child(
                div()
                    .flex_none()
                    .when(fluid, |time| time.w(px(time_width)).text_right())
                    .when(position_preview.is_some(), |time| {
                        let color = if fluid_high_contrast(cx) {
                            rgb(contrast::ACCENT).into()
                        } else {
                            materials::accent(cx)
                        };
                        time.text_color(color)
                    })
                    .child(time_label(position_preview.unwrap_or(position))),
            )
            .child(slider)
            .child(
                div()
                    .flex_none()
                    .when(fluid, |time| time.w(px(time_width)))
                    .child(duration_label),
            );
        #[cfg(feature = "capture")]
        let row = {
            let bounds = self.track_bounds.clone();
            row.on_children_prepainted(move |children, _, _| {
                if let Some(track) = children.get(1) {
                    bounds.set(*track);
                }
            })
        };
        row
    }
}
