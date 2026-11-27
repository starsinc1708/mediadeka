use gpui::{Context, Div, IntoElement, Role, div, prelude::*, px};

use crate::{
    music::library::{EntryAction, LibraryGroup},
    playback::{Playback, Status},
    playlist::PlaylistKey,
    ui::{
        MainView,
        components::{Activate, music::fluid_cover_slot},
        design_system::icons::{self, Glyph, GlyphState},
        fluid_glass::{artwork, artwork::Seed},
        labels::track_count,
        music::{
            PlaybackAction,
            detail::{
                hero_motion::{HERO_ARTWORK_SPAN, cover_span, shared_artwork},
                sticky_hero::{CollapsingMediaHero, HeroHeader, blend_hero_artwork},
            },
            detail_style::{self, HeroSize},
        },
    },
};

const ACTION_EDGE: f32 = 32.;
const ACTION_GAP: f32 = 8.;

impl MainView {
    pub(super) fn local_group_hero(
        &self,
        width: f32,
        compact: bool,
        cx: &Context<Self>,
    ) -> Option<CollapsingMediaHero> {
        let library = &self.music.collections;
        let group = library
            .location
            .group
            .as_ref()
            .filter(|_| self.uses_fluid_glass())?;
        let size = match group {
            LibraryGroup::Album { .. } => HeroSize::album(width, compact),
            LibraryGroup::Artist(_) => HeroSize::fluid(width, compact, false),
        };
        let cover = size.cover.min(340.);
        let expanded_height = cover + 2. * size.vertical_inset;
        Some(CollapsingMediaHero::new(
            expanded_height,
            cover,
            size.title.min(64.),
            library.group_scroll_offset(px(expanded_height)),
            self.reduced_motion || cx.reduce_motion() || self.fluid_high_contrast(cx),
        ))
    }

    pub(super) fn local_group_header(
        &self,
        hero: CollapsingMediaHero,
        width: f32,
        margin: f32,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let library = &self.music.collections;
        let group = library.location.group.as_ref()?;
        let subtitle = match group {
            LibraryGroup::Album { artist, .. } if !artist.is_empty() => {
                format!("{artist} · Локальный альбом")
            }
            LibraryGroup::Album { .. } => "Локальный альбом".to_owned(),
            LibraryGroup::Artist(_) => "Исполнитель · Локальные файлы".to_owned(),
        };
        let seed = Seed::entry(&EntryAction::LocalGroup(group.clone()));
        let radius = hero.artwork_radius();
        let high_contrast = self.fluid_high_contrast(cx);
        let artwork = blend_hero_artwork(
            cover_span(("library-group-artwork", group)),
            library.group_artwork().map(str::to_owned),
            !self.reduced_motion && !cx.reduce_motion() && !high_contrast,
            move |url, _| {
                artwork::hero_thumbnail(url.as_deref(), seed, px(radius)).into_any_element()
            },
        );
        let cover = fluid_cover_slot(hero.artwork_size(), radius)
            .id("library-group-artwork")
            .accessibility_id("library-group-artwork")
            .role(Role::Image)
            .aria_label("Обложка локальной коллекции")
            .child(artwork);
        let cover = shared_artwork(cover, HERO_ARTWORK_SPAN, self.music_hero_motion());
        let available = library.results.downloaded > 0;
        let current = self.player.read(cx).current_entry().is_some_and(|entry| {
            matches!(&entry.playlist, PlaylistKey::Local(_)) && group.contains(&entry.track)
        });
        let playing = current && matches!(&self.player_snapshot.status, Status::Playing);
        let paused = current && matches!(&self.player_snapshot.status, Status::Paused);
        let loading = current && matches!(&self.player_snapshot.status, Status::Loading);
        let label = if playing {
            "Пауза"
        } else if loading {
            "Загрузка…"
        } else {
            "Слушать"
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(px(ACTION_GAP))
            .when(available, |actions| {
                actions
                    .child(
                        icons::hero_play_icon(
                            "library-group-play",
                            label,
                            playing,
                            hero.play_size(),
                            29,
                            if loading {
                                GlyphState::Loading
                            } else {
                                GlyphState::Rest
                            },
                            cx,
                        )
                        .when(!loading, |control| {
                            control.on_activate(cx.listener(move |view, _, _, cx| {
                                if playing || paused {
                                    view.player.update(cx, |bar, cx| {
                                        bar.command(cx, Playback::toggle_pause)
                                    });
                                } else {
                                    view.play_local_group(PlaybackAction::Play, cx);
                                }
                            }))
                        }),
                    )
                    .child(
                        icons::ghost_icon_button(
                            "library-group-shuffle",
                            "Перемешать и слушать",
                            Glyph::Shuffle,
                            false,
                            ACTION_EDGE,
                            30,
                            true,
                            cx,
                        )
                        .on_activate(cx.listener(|view, _, _, cx| {
                            view.play_local_group(PlaybackAction::Shuffle, cx);
                        })),
                    )
                    .child(
                        icons::ghost_icon_button(
                            "library-group-queue",
                            "Добавить в очередь",
                            Glyph::Queue,
                            false,
                            ACTION_EDGE,
                            32,
                            true,
                            cx,
                        )
                        .on_activate(cx.listener(|view, _, _, cx| {
                            view.play_local_group(PlaybackAction::Append, cx);
                        })),
                    )
            });
        let details = div()
            .text_size(px(13.))
            .text_color(detail_style::Ink::new(true, high_contrast).secondary)
            .child(track_count(library.results.visible.len()));
        Some(hero.render(
            HeroHeader {
                id: "library-group-hero",
                title_id: "music-liked-title",
                title: group.title(),
                subtitle: (!subtitle.is_empty()).then(|| div().child(subtitle).into_any_element()),
                width,
                margin,
                action_width: if available {
                    hero.play_size() + 2. * (ACTION_EDGE + ACTION_GAP)
                } else {
                    0.
                },
                high_contrast,
            },
            cover,
            actions,
            details,
            cx.listener(|view, _, window, cx| view.scroll_library_hero_to_top(window, cx)),
            cx,
        ))
    }

    fn play_local_group(&mut self, action: PlaybackAction, cx: &mut Context<Self>) {
        let library = &self.music.collections;
        let source = library.location.source.as_ref();
        let entries: Vec<_> = library
            .results
            .visible
            .iter()
            .filter_map(|&index| {
                let entry = library.index.rows[index].entry(source);
                match &entry.action {
                    EntryAction::Track(key @ PlaylistKey::Local(_), track) if entry.downloaded => {
                        Some((key.clone(), track.clone()))
                    }
                    _ => None,
                }
            })
            .collect();
        if matches!(action, PlaybackAction::Play)
            && let Some((_, first)) = entries.first()
            && self.play_local_album_track(first, cx)
        {
            return;
        }
        self.play_music_tracks(
            entries.iter().map(|(key, track)| (key.clone(), track)),
            action,
            cx,
        );
    }
}
