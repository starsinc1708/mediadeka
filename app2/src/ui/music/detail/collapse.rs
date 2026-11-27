use gpui::{Context, Div, IntoElement, Role, div, prelude::*, px};
use guise::{Button, IconName, Size, Variant};

use crate::{
    music::ObjectKind,
    playback::{Status, can_resolve},
    ui::{
        MainView,
        components::{
            Activate, entity_link,
            music::control,
            scrollbar::{ScrollSurface, scroll_offset},
        },
        design_system::{controls, icons},
        fluid_glass::{
            artist_portrait::ArtistPortrait,
            artwork::{self, Seed},
        },
        playlist::popup::MenuKind,
    },
};

use super::super::{PlaybackAction, detail_style};
use super::{
    hero_motion::{HERO_ARTWORK_SPAN, cover_span, shared_artwork},
    playlist_key,
    sticky_hero::{CollapsingMediaHero, HeroHeader, blend_hero_artwork, scroll_to_hero_top},
};

impl MainView {
    pub(in crate::ui) fn object_hero_scroll(
        &self,
        width: f32,
        compact: bool,
        cx: &Context<Self>,
    ) -> Option<CollapsingMediaHero> {
        let detail = self.displayed_music_object()?;
        let page = &detail.data.as_ref()?.page;
        if !self.uses_fluid_glass()
            || detail.hidden
            || !matches!(
                page.reference.kind,
                ObjectKind::Album
                    | ObjectKind::Artist
                    | ObjectKind::Playlist
                    | ObjectKind::Collection
            )
        {
            return None;
        }
        let size = match page.reference.kind {
            ObjectKind::Album => detail_style::HeroSize::release(
                width,
                compact,
                detail_style::ReleasePresentation::for_page(Some(page)),
            ),
            kind => detail_style::HeroSize::fluid(width, compact, kind == ObjectKind::Artist),
        };
        let cover = size.cover.min(340.);
        Some(CollapsingMediaHero::new(
            cover + size.vertical_inset * 2. + if compact { 8. } else { 0. },
            cover,
            size.title.min(64.),
            scroll_offset(&self.scroll),
            self.reduced_motion || self.fluid_high_contrast(cx),
        ))
    }

    pub(in crate::ui) fn object_collapsing_header(
        &self,
        hero: CollapsingMediaHero,
        width: f32,
        margin: f32,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let detail = self.displayed_music_object()?;
        let data = detail.data.as_ref()?;
        let page = &data.page;
        let target = &detail.target;
        let kind = page.reference.kind;
        let artist = kind == ObjectKind::Artist;
        let title = if page.title.is_empty() {
            target.title.as_str()
        } else {
            &page.title
        };
        let kind_label = self.displayed_kind_label(kind, Some(page));
        let key = playlist_key(data);
        let byline = entity_link::page_artists(div(), page, &key, cx);
        let audience = page
            .artist_details
            .as_ref()
            .and_then(detail_style::monthly_audience)
            .map(|label| {
                div()
                    .id("music-artist-audience")
                    .accessibility_id("music-artist-audience")
                    .role(Role::Label)
                    .aria_label(label.clone())
                    .min_w_0()
                    .truncate()
                    .child(label)
                    .into_any_element()
            });
        let subtitle = div()
            .flex()
            .items_center()
            .min_w_0()
            .gap(px(6.))
            .when_some(byline, |row, links| {
                row.child(links).child(div().child("·"))
            })
            .child(
                audience.unwrap_or_else(|| div().flex_none().child(kind_label).into_any_element()),
            )
            .into_any_element();
        let playable = page.tracks.iter().any(|track| can_resolve(&key, track));
        let status = (self.player_snapshot.key.as_ref() == Some(&key))
            .then_some(&self.player_snapshot.status);
        let playing = matches!(status, Some(Status::Playing));
        let paused = matches!(status, Some(Status::Paused));
        let loading = matches!(status, Some(Status::Loading));
        let (label, state) = if loading {
            ("Загрузка…", icons::GlyphState::Loading)
        } else if playing {
            ("Пауза", icons::GlyphState::Rest)
        } else {
            ("Слушать", icons::GlyphState::Rest)
        };
        let radius = hero.artwork_radius();
        let edge = hero.artwork_size();
        let high_contrast = self.fluid_high_contrast(cx);
        let image = page.artwork.as_deref().or(target.artwork.as_deref());
        let panorama = page
            .artist_details
            .as_ref()
            .and_then(|details| details.banner.as_deref());
        let seed = Seed::object(&target.object);
        let artwork = div().relative().size(px(edge)).child(blend_hero_artwork(
            cover_span(("object-hero", &target.object)),
            (image.map(str::to_owned), panorama.map(str::to_owned)),
            !self.reduced_motion && !high_contrast,
            move |(image, panorama), previous| {
                if artist && (image.is_some() || panorama.is_some()) {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ArtistPortrait {
                            portrait: image.as_deref().map(Into::into),
                            panorama: panorama.as_deref().map(Into::into),
                            fallback: seed,
                            width: edge,
                            height: edge,
                            compact: edge < 160.,
                            high_contrast,
                            decorative: previous,
                            decode_edge: Some(artwork::HERO_IMAGE_EDGE),
                        })
                        .into_any_element()
                } else {
                    div()
                        .size_full()
                        .relative()
                        .child(artwork::hero_thumbnail(
                            image.as_deref(),
                            Some(seed),
                            px(radius),
                        ))
                        .into_any_element()
                }
            },
        ));
        let artwork = if artist {
            artwork.into_any_element()
        } else {
            artwork
                .id("music-object-artwork")
                .accessibility_id("music-object-artwork")
                .role(Role::Image)
                .aria_label("Обложка")
                .into_any_element()
        };
        let artwork = shared_artwork(artwork, HERO_ARTWORK_SPAN, self.music_hero_motion());
        let controls = self.glass_controls(cx);
        let compact = hero.play_size() < 50.;
        let mut secondary = Vec::new();
        if playable {
            secondary.push(
                control(
                    "music-object-shuffle",
                    "Перемешать и слушать",
                    IconName::Shuffle,
                    38,
                    true,
                    false,
                    false,
                )
                .pixel_size(px(32.))
                .when_some(controls, detail_style::secondary_icon)
                .on_click(
                    cx.listener(|view, _, _, cx| {
                        view.play_object(None, PlaybackAction::Shuffle, cx)
                    }),
                )
                .into_any_element(),
            );
        }
        secondary.push(
            self.playlist_menu_trigger(
                "music-object-hero-menu",
                "Действия страницы",
                IconName::Ellipsis,
                39,
                MenuKind::ObjectHero { playable },
                cx,
            )
            .into_any_element(),
        );
        let actions = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .when(playable, |actions| {
                actions.child(
                    icons::hero_play_icon(
                        "music-object-play",
                        label,
                        playing,
                        hero.play_size(),
                        37,
                        state,
                        cx,
                    )
                    .when(!loading, |button| {
                        button.on_activate(cx.listener(move |view, _, _, cx| {
                            if playing || paused {
                                view.player.update(cx, |bar, cx| {
                                    bar.command(cx, crate::playback::Playback::toggle_pause)
                                });
                            } else {
                                view.play_object(None, PlaybackAction::Play, cx);
                            }
                        }))
                    }),
                )
            });
        let actions = if let Some(material) = controls {
            actions.child(controls::action_cluster(
                "music-object-secondary-actions",
                material,
                compact,
                secondary,
            ))
        } else {
            actions.children(secondary)
        };
        let ink = detail_style::Ink::new(true, high_contrast);
        let facts = page
            .year
            .map(|year| year.to_string())
            .into_iter()
            .chain(
                (!page.tracks.is_empty())
                    .then(|| crate::ui::labels::track_count(page.tracks.len())),
            )
            .chain(
                page.tracks
                    .iter()
                    .map(|track| track.duration.map(u64::from))
                    .sum::<Option<u64>>()
                    .filter(|total| *total > 0)
                    .map(detail_style::duration_label),
            )
            .collect::<Vec<_>>()
            .join(" · ");
        let station = crate::ui::station::yandex_radio_target(
            &target.object,
            data.profile.as_deref(),
            title,
            image,
        );
        let extras = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(!facts.is_empty(), |extras| {
                extras.child(
                    div()
                        .text_size(px(13.))
                        .text_color(ink.secondary)
                        .child(facts),
                )
            })
            .children(station.map(|target| {
                Button::new("music-object-station", "Станция Яндекс Музыки")
                    .size(Size::Xs)
                    .variant(Variant::Subtle)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_and_start_station(target.clone(), window, cx)
                    }))
            }))
            .when(!self.has_selected_release(), |extras| {
                extras.children(self.music_link_actions(detail, title, cx))
            });
        let surface = ScrollSurface::Area(self.scroll.clone());
        Some(hero.render(
            HeroHeader {
                id: "music-object-hero",
                title_id: "music-object-title",
                title,
                subtitle: Some(subtitle),
                width,
                margin,
                action_width: if playable {
                    hero.play_size() + 84.
                } else {
                    36.
                },
                high_contrast,
            },
            artwork,
            actions,
            extras,
            move |_, window, cx| scroll_to_hero_top(surface.clone(), window, cx),
            cx,
        ))
    }

    pub(in crate::ui) fn enqueue_open_object(&mut self, cx: &mut Context<Self>) {
        self.play_object(None, PlaybackAction::Append, cx);
    }

    pub(in crate::ui) fn object_radio_available(&self) -> bool {
        self.music
            .object
            .as_ref()
            .and_then(|detail| detail.data.as_ref())
            .is_some_and(|data| {
                data.page.reference.kind == ObjectKind::Artist && self.can_start_artist_radio(data)
            })
    }

    pub(in crate::ui) fn start_open_object_radio(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.object_radio_available()
            && let Some(data) = self
                .music
                .object
                .as_ref()
                .and_then(|detail| detail.data.clone())
        {
            self.start_artist_radio(data, window, cx);
        }
    }
}
