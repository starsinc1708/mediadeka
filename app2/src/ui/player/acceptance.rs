use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{Bounds, Context, Div, Pixels, Window};

use super::{PlayerBar, PositionChanged};

use crate::{playback::QueueEntry, playlist::PlaylistKey, storage::Store, ui::MainView};

#[derive(Default)]
pub(super) struct LayoutProbe(Rc<Cell<[Bounds<Pixels>; 3]>>);

impl LayoutProbe {
    pub(super) fn record(&self, row: Div) -> Div {
        let bounds = self.0.clone();
        row.on_children_prepainted(move |children, _, _| {
            if let [track, transport, tools] = children.as_slice() {
                bounds.set([*track, *transport, *tools]);
            }
        })
    }
}

impl PlayerBar {
    pub(in crate::ui) fn sample_fluid_cover_for_capture(
        &mut self,
        time: Duration,
        cx: &mut Context<Self>,
    ) {
        self.fluid_cover.sample(time);
        cx.notify();
    }
}

impl MainView {
    pub fn player_layout_for_capture(
        &self,
        cx: &gpui::App,
    ) -> ([Bounds<Pixels>; 3], Bounds<Pixels>, Option<f64>) {
        let player = self.player.read(cx);
        let (seek, preview) = player.progress.read(cx).seek_for_capture();
        (player.layout_probe.0.get(), seek, preview)
    }

    pub fn fluid_track_labels_for_capture(&self, cx: &gpui::App) -> Vec<(String, String, f32)> {
        self.player.read(cx).fluid_cover.labels_for_capture()
    }

    pub fn observe_player_ticks_for_capture(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn() -> (usize, usize) + use<> {
        let observers = Rc::new(Cell::new(0));
        let ticks = Rc::new(Cell::new(0));
        let observed = observers.clone();
        let transport = cx.observe(&self.player, move |_, _, _| {
            observed.set(observed.get() + 1);
        });
        let positions = ticks.clone();
        let position = cx.subscribe(&self.player, move |_, _, _: &PositionChanged, _| {
            positions.set(positions.get() + 1);
        });
        move || {
            let _hold_subscriptions = (&transport, &position);
            (ticks.get(), observers.get())
        }
    }

    pub fn sound_state_for_capture(&self, cx: &gpui::App) -> Option<serde_json::Value> {
        let playback = &self.player.read(cx).playback;
        let source = playback.sound_source()?;
        let quality = playback.audio_quality()?;
        let details = playback.sound_details();
        Some(serde_json::json!({
            "source": source.label(),
            "quality": quality.label(),
            "codec": quality.codec,
            "bitrate_bps": quality.bitrate_bps,
            "sample_rate_hz": quality.sample_rate_hz,
            "position": playback.position(),
            "duration": playback.duration(),
            "time_to_sound_ms": details.time_to_sound_ms,
            "reason": details.reason,
        }))
    }

    pub fn reset_player_probe_focus_for_capture(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
    }

    pub fn play_local_with_artwork_for_capture(
        &mut self,
        id: i64,
        artwork: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Result<()> {
        self.play_local_cover_fixture(id, [None, Some(artwork)], cx)
    }

    pub fn play_fluid_glass_transition_fixture_for_capture(
        &mut self,
        id: i64,
        first_artwork: &str,
        second_artwork: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Result<()> {
        self.play_local_cover_fixture(id, [Some(first_artwork), Some(second_artwork)], cx)
    }

    fn play_local_cover_fixture(
        &mut self,
        id: i64,
        artworks: [Option<&str>; 2],
        cx: &mut Context<Self>,
    ) -> gpui::Result<()> {
        let source = Store::open_default()?
            .local_playlist(id)?
            .ok_or_else(|| std::io::Error::other("Нет локального плейлиста для проверки."))?;
        let entries = source
            .tracks
            .into_iter()
            .enumerate()
            .map(|(index, mut track)| {
                if let Some(Some(artwork)) = artworks.get(index) {
                    track.artwork = Some((*artwork).to_owned());
                }
                QueueEntry {
                    playlist: PlaylistKey::Local(id),
                    playlist_revision: None,
                    recommendations: Vec::new(),
                    index,
                    track,
                }
            })
            .collect();
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.play(entries, 0));
        });
        Ok(())
    }

    pub fn player_state_for_capture(
        &self,
        cx: &gpui::App,
    ) -> (bool, bool, Option<usize>, bool, Option<&'static str>, usize) {
        let bar = self.player.read(cx);
        (
            bar.reduced_motion,
            bar.cover_transition.is_some(),
            bar.playback.current_index(),
            bar.playback.seekable(),
            if bar.devices_open {
                Some("devices")
            } else {
                self.panel_is_open_on_queue().then_some("queue")
            },
            bar.playback.devices().len(),
        )
    }
}
