use gpui::{Context, Div, IntoElement, div, prelude::*, px};
use guise::{Button, Icon, IconName, Size, Variant};

use crate::{
    music::ObjectKind,
    playback::{Playback, QueueEntry, Status, can_resolve},
    playlist::Media,
    ui::{
        MainView,
        components::{Activate, button, entity_link, music::time_label},
        design_system::icons::{GlyphState, hero_play},
        fluid_glass::artwork::Seed,
        music::{
            PlaybackAction, TRACK_LYRICS_ORDER,
            detail_style::{Ink, ReleasePresentation},
            heading,
        },
        playlist::navigation,
        workspace::LocalTrackTarget,
    },
};

use super::hero::{Hero, HeroArtwork};

impl MainView {
    pub(super) fn track_lyrics_action(
        &self,
        entry: QueueEntry,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        Button::new("music-object-lyrics", "Текст песни")
            .variant(Variant::Subtle)
            .size(Size::Xs)
            .tab_order(TRACK_LYRICS_ORDER)
            .track_focus(&self.music.track_lyrics_focus)
            .left_section(Icon::new(IconName::Captions).pixel_size(px(15.)))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.show_track_lyrics(entry.clone(), window, cx)
            }))
    }

    pub(super) fn local_track_detail(
        &self,
        target: &LocalTrackTarget,
        compact: bool,
        content_width: f32,
        short: bool,
        cx: &Context<Self>,
    ) -> Div {
        let key = &target.key;
        let track = &target.track;
        let glass = self.uses_fluid_glass();
        let kind = match &track.media {
            Some(Media::File(_)) => "Локальный трек",
            Some(Media::Url(_)) => "Импортированный трек",
            _ => "Трек",
        };
        let high_contrast = self.fluid_high_contrast(cx);
        let ink = Ink::new(glass, high_contrast);
        let hero = Hero::new(
            ObjectKind::Track,
            content_width,
            compact,
            short,
            track.artwork.is_some(),
            glass,
            ReleasePresentation::Album,
        );
        let current = self.player.read(cx).recording_status(key, track).is_some();
        let playing = current && self.player_snapshot.status == Status::Playing;
        let paused = current && self.player_snapshot.status == Status::Paused;
        let label = if playing {
            "Пауза"
        } else if paused {
            "Продолжить"
        } else {
            "Слушать"
        };
        let transport = if glass {
            hero_play(
                "music-object-play",
                label,
                playing,
                if compact || short { 40. } else { 56. },
                37,
                GlyphState::Rest,
                cx,
            )
        } else {
            button("music-object-play", label, 37)
                .h(px(32.))
                .px(px(8.))
                .child(label)
        };
        let selection = target.clone();
        let body = hero
            .body()
            .child(div().text_color(ink.secondary).child(kind))
            .child(hero.title(
                heading("music-object-title", track.title.clone()),
                &track.title,
            ))
            .child(entity_link::artist_names(
                div().text_color(ink.secondary),
                "local-track-artist",
                key,
                track,
                34,
                cx,
            ))
            .children(navigation::album(key, track).map(|(name, destination)| {
                entity_link::text(
                    div().truncate().text_color(ink.secondary),
                    "local-track-album",
                    name,
                    Some(destination),
                    35,
                    cx,
                )
            }))
            .children(track.duration.map(|seconds| {
                div()
                    .text_color(ink.secondary)
                    .child(time_label(f64::from(seconds)))
            }))
            .when(current, |body| {
                body.children(self.player.read(cx).sound_details(false))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(can_resolve(key, track), |controls| {
                        controls.child(transport.on_activate(cx.listener(move |view, _, _, cx| {
                            if playing || paused {
                                view.player
                                    .update(cx, |bar, cx| bar.command(cx, Playback::toggle_pause));
                            } else {
                                view.play_music_tracks(
                                    [(selection.key.clone(), &selection.track)],
                                    PlaybackAction::Play,
                                    cx,
                                );
                            }
                        })))
                    })
                    .child(self.like_control("local-track-like", 38, key, track, cx))
                    .child(self.track_lyrics_action(super::object_entry(key, 0, track), cx)),
            );
        hero.frame(
            HeroArtwork {
                image: track.artwork.as_deref(),
                banner: None,
                fallback: Seed::queue_track(key, track),
                scale: 1.,
            },
            body,
            None,
            high_contrast,
            self.music_hero_motion(),
        )
    }
}
