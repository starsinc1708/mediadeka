use std::{io, path::Path};

use gpui::{App, Context, Pixels, Point, Window, point, px};
use serde_json::{Value, json};

use super::MainView;
use crate::{
    music::lyrics::{self, Lyrics, LyricsVersions, Source, parse_lrc},
    playback::QueueEntry,
    playlist::PlaylistKey,
    storage::Store,
    ui::{components::artwork, listening::ListeningMode},
};

impl MainView {
    pub fn seed_lyrics_versions_for_capture(
        profile: &Path,
        playlist: i64,
        synchronized: &str,
        plain: &str,
    ) -> gpui::Result<()> {
        let store = Store::open(profile)?;
        let track = store
            .local_playlist(playlist)?
            .and_then(|playlist| playlist.tracks.into_iter().next())
            .ok_or_else(|| io::Error::other("Тестовый плейлист не содержит трек."))?;
        let entry = QueueEntry {
            playlist: PlaylistKey::Local(playlist),
            playlist_revision: None,
            index: 0,
            recommendations: Vec::new(),
            track,
        };
        let synchronized = parse_lrc(Source::Lrclib, synchronized)
            .ok_or_else(|| io::Error::other("Тестовый LRC не содержит меток времени."))?;
        let plain = Lyrics::plain(Source::Lrclib, plain);
        let versions = LyricsVersions::from_source(synchronized, Some(plain));
        lyrics::seed_lrclib_versions_for_capture(&store, &entry, &versions)
            .map_err(io::Error::other)?;
        Ok(())
    }

    pub fn sample_lyrics_emphasis_for_capture(&self, ms: Option<u64>, cx: &mut Context<Self>) {
        self.lyrics.update(cx, |lyrics, cx| {
            lyrics
                .emphasis
                .sample(ms.map(std::time::Duration::from_millis));
            cx.notify();
        });
    }

    pub fn lyrics_emphasis_for_capture(&self, line: usize, cx: &App) -> Option<(f32, f32)> {
        let lyrics = self.lyrics.read(cx);
        let row = lyrics.scroll.bounds_for_item(line)?;
        let amount =
            lyrics
                .emphasis
                .amount(line, std::time::Instant::now(), lyrics.reduced_motion(cx));
        Some((amount, row.size.height.into()))
    }

    pub fn lyrics_accent_for_capture(&self, cx: &App) -> [u8; 3] {
        let [_, red, green, blue] = self.lyrics.read(cx).accent(cx).to_be_bytes();
        [red, green, blue]
    }

    pub fn lyrics_state_for_capture(&self, cx: &App) -> Value {
        let view = self.lyrics.read(cx);
        let text = view.current_text();
        let player = self.player.read(cx);
        let snapshot = player.snapshot();
        let mode = self.listening.mode();
        let (upcoming, completed, prepared) = view.prefetch.counts();
        json!({
            "source": text.map(|text| text.source.label()),
            "selection": view.selection.map_or("Авто", Source::label),
            "display_plain": view.display_plain && view.plain_source().is_some(),
            "plain_available": view.plain_source().is_some(),
            "timed_lines": text.map_or(0, |text| text.lines.iter().filter(|line| line.time_ms.is_some()).count()),
            "loading": view.load.is_some() || view.genius.is_pending(),
            "track": view.entry.as_ref().map(|entry| entry.index),
            "lyrics_track": view.entry.as_ref().map(|entry| entry.track.id.as_str()),
            "follows_playback": view.follows_playback,
            "seekable": view.seekable,
            "lines": text.map_or(0, |text| text.lines.len()),
            "line": view.highlight.line,
            "word": view.highlight.word,
            "clock_ms": view.clock.position_ms(),
            "scroll_y": f32::from(view.scroll.offset().y),
            "player_position": player.position(),
            "playing": player.is_playing(),
            "fullscreen": mode == Some(ListeningMode::Lyrics),
            "listening_mode": mode.map(|mode| match mode {
                ListeningMode::Cover => "cover",
                ListeningMode::Lyrics => "lyrics",
            }),
            "current_track": player.current().map(|entry| entry.track.id.as_str()),
            "visible": view.visible,
            "prefetch": {
                "upcoming": upcoming,
                "completed": completed,
                "prepared": prepared,
            },
            "queue": {
                "entries": player.entries().map(|entry| entry.track.id.as_str()).collect::<Vec<_>>(),
                "position": snapshot.queue_position,
                "revision": snapshot.queue_revision,
            },
            "genius_failed": view.genius.failed(),
            "genius": view.genius.song.as_ref().map(|song| json!({
                "annotations": song.annotations.len(),
                "writers": song.writers.len(),
                "marked_lines": view.genius.rows.iter().filter(|row| !row.is_empty()).count(),
                "open_note": view.genius.open_note.map(|(line, _)| line),
            })),
        })
    }

    pub fn seek_lyrics_playback_for_capture(&mut self, seconds: f64, cx: &mut Context<Self>) {
        self.player.update(cx, |bar, cx| bar.seek(seconds, cx));
    }

    pub fn lyrics_line_target_for_capture(&self, index: usize, cx: &App) -> Option<Point<Pixels>> {
        let view = self.lyrics.read(cx);
        let bounds = view.scroll.bounds_for_item(index)?;
        let center = bounds.center() + view.scroll.offset();
        (view.scroll.bounds().contains(&center)).then_some(center)
    }

    pub fn lyrics_note_target_for_capture(&self, index: usize, cx: &App) -> Option<Point<Pixels>> {
        let view = self.lyrics.read(cx);
        view.genius.rows.get(index)?.last()?;
        let bounds = view.scroll.bounds_for_item(index)?;
        let target = point(bounds.right() - px(17.), bounds.top() + px(17.)) + view.scroll.offset();
        (view.scroll.bounds().contains(&target)).then_some(target)
    }

    pub fn set_listening_artwork_for_capture(&mut self, artwork: &str, cx: &mut Context<Self>) {
        self.player
            .update(cx, |bar, cx| bar.set_artwork_for_capture(artwork, cx));
    }

    pub fn listening_artwork_decoded_for_capture(&self, window: &Window, cx: &mut App) -> bool {
        let viewport = window.viewport_size();
        let edge = if self.uses_fluid_glass() {
            self.fluid_listening_artwork_edge(window)
        } else {
            Some(viewport.width.max(viewport.height))
        };
        let artwork = self.player.read(cx).current_artwork().map(str::to_owned);
        artwork
            .zip(edge)
            .is_some_and(|(url, edge)| artwork::decoded_edge_for_capture(&url, edge, window, cx))
    }

    pub fn choose_local_lyrics_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lyrics.update(cx, |view, cx| {
            view.choose_source(Some(Source::Local), window, cx)
        });
    }
}
