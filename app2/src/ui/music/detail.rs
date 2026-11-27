mod artist_sections;
mod collapse;
mod hero;
pub(in crate::ui) mod hero_motion;
mod local_track;
#[cfg(all(test, windows, feature = "capture"))]
mod related_acceptance;
pub(in crate::ui) mod sticky_hero;
pub(super) mod track_release;

use std::{cell::Cell, sync::Arc};

use gpui::{Context, FontWeight, IntoElement, Role, accesskit::Toggled, div, prelude::*, px, rgb};
use guise::{Button, Icon, IconName, Size, Variant};

use crate::{
    music::{ConfirmedLink, ObjectKind, PageUpdate, ScopedPage, load_page},
    playback::{CollectionPlan, QueueEntry, can_resolve},
    playlist::{PlaylistKey, PlaylistTrack},
    ui::{
        MainView,
        components::{
            Activate, button,
            content_state::{error_state, loading_state, refresh_indicator},
            entity_link,
            glass_control::Tone,
            music::{control, time_label},
        },
        connections::SourceFailure,
        design_system::icons,
        fluid_glass::{PageAtmosphere, tokens},
        playlist::{navigation, popup::MenuKind},
        theme::{DIVIDER, ELEVATED, WINDOW, primary, secondary, surface_border},
        workspace::{MusicTarget, Route, pins::pin_of_object},
    },
};

use super::{PlaybackAction, detail_style, heading, run_job, status};
use hero::{Hero, HeroArtwork};
use sticky_hero::CollapsingMediaHero;

pub(super) struct Detail {
    pub(super) target: MusicTarget,
    pub(super) data: Option<ScopedPage>,
    pub(super) links: Vec<ConfirmedLink>,
    pub(super) error: Option<SourceFailure>,
    hidden: bool,
    pub(super) copied_platform: Option<String>,
    pub(super) search_open: bool,
    pub(super) show_all_tracks: bool,
    pub(super) singles_filter: Option<bool>,
}

impl Detail {
    pub(super) fn new(target: MusicTarget) -> Self {
        Self {
            target,
            data: None,
            links: Vec::new(),
            error: None,
            hidden: false,
            copied_platform: None,
            search_open: false,
            show_all_tracks: false,
            singles_filter: None,
        }
    }

    fn has_cross_provider_data(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|data| data.page.reference.provider != self.target.object.provider)
    }

    fn source_hints_visible(&self) -> bool {
        !self.hidden && !self.has_cross_provider_data()
    }
}

pub(super) fn playlist_key(data: &ScopedPage) -> PlaylistKey {
    let reference = &data.page.reference;
    PlaylistKey::Source {
        provider: reference.provider,
        profile: data.profile.clone(),
        id: format!("{}:{}", reference.kind.path(), reference.id),
    }
}

pub(super) fn object_entry(key: &PlaylistKey, index: usize, track: &PlaylistTrack) -> QueueEntry {
    QueueEntry {
        playlist: key.clone(),
        playlist_revision: None,
        index,
        track: track.clone(),
        recommendations: Vec::new(),
    }
}

impl MainView {
    pub(in crate::ui) fn object_atmosphere(&self) -> Option<PageAtmosphere<'_>> {
        let detail = self.displayed_music_object();
        let target = match self.workspace.routes.current() {
            _ if self.has_selected_release() => &detail?.target,
            Route::Object(target) => target,
            _ => return None,
        };
        let detail = detail.filter(|detail| &detail.target == target);
        let page = detail.and_then(|detail| detail.data.as_ref());
        let artwork = page
            .and_then(|data| {
                data.page
                    .artwork
                    .as_deref()
                    .or_else(|| data.page.artist_details.as_ref()?.banner.as_deref())
            })
            .or_else(|| {
                (detail.is_none_or(Detail::source_hints_visible))
                    .then_some(target.artwork.as_deref())
                    .flatten()
            });
        Some(PageAtmosphere::object(&target.object, artwork))
    }

    pub(in crate::ui) fn object_radio_entry(&self, index: usize) -> Option<QueueEntry> {
        let data = self.displayed_music_object()?.data.as_ref()?;
        let track = data.page.tracks.get(index)?;
        Some(object_entry(&playlist_key(data), index, track))
    }

    pub(in crate::ui) fn forget_artist_discography(&mut self) {
        self.music.artist_discography = None;
    }

    pub(super) fn load_music_object(&mut self, target: MusicTarget, cx: &mut Context<Self>) {
        if self.music.request.is_some() {
            return;
        }
        match self.music.object.as_mut() {
            Some(detail) if detail.target == target => detail.error = None,
            _ => self.music.object = Some(Detail::new(target.clone())),
        }
        let cross_provider_loaded = Cell::new(false);
        self.music.request = Some(run_job(
            cx,
            move |cancelled, tx| {
                load_page(
                    &target.object,
                    target.profile.as_deref(),
                    cancelled,
                    |update| {
                        let _ = tx.try_send(update);
                    },
                );
                Ok(())
            },
            move |view, update, cx| {
                let Some(detail) = &mut view.music.object else {
                    return;
                };
                match update {
                    PageUpdate::Data(page) => {
                        if page.page.reference.kind == ObjectKind::Artist
                            && !page.page.releases.is_empty()
                        {
                            view.music.artist_discography = Some(page.clone());
                        }
                        detail.hidden = false;
                        detail.error = None;
                        detail.data = Some(page.clone());
                        cross_provider_loaded.set(detail.has_cross_provider_data());
                        view.refresh_genius_credits(cx);
                        view.observe_prefetched_page(&page);
                        view.prefetch_music_relations(&page, cx);
                        view.refresh_track_release(cx);
                    }
                    PageUpdate::Links(links) => detail.links = links,
                    PageUpdate::Error {
                        message,
                        hide,
                        needs_sign_in,
                    } => {
                        detail.error = Some(SourceFailure {
                            message,
                            needs_sign_in,
                        });
                        if hide {
                            let provider = detail.target.object.provider;
                            if cross_provider_loaded.get() {
                                detail
                                    .links
                                    .retain(|link| link.platform != provider.as_str());
                                if detail.copied_platform.as_deref() == Some(provider.as_str()) {
                                    detail.copied_platform = None;
                                }
                            } else {
                                detail.hidden = true;
                                detail.data = None;
                                detail.links.clear();
                            }
                            view.forget_account_playlists(provider, None, cx);
                            view.forget_artist_discography();
                            view.refresh_genius_credits(cx);
                            view.refresh_track_release(cx);
                        }
                    }
                }
                cx.notify();
            },
            |_, _, _| {},
        ));
        cx.notify();
    }

    pub(in crate::ui) fn object_view(
        &self,
        compact: bool,
        content_width: f32,
        short: bool,
        collapsing: Option<CollapsingMediaHero>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut content = div()
            .id("music-object-page")
            .accessibility_id("music-object-page")
            .role(Role::Group)
            .aria_label("Музыкальная страница")
            .flex()
            .flex_col()
            .gap(px(15.));
        let in_release = self.has_selected_release();
        if let Route::LocalTrack(target) = self.workspace.routes.current()
            && !in_release
        {
            return content
                .child(self.local_track_detail(target, compact, content_width, short, cx))
                .children(self.track_credits_block(&target.track, cx));
        }
        let Some(detail) = self.displayed_music_object() else {
            return content;
        };
        let target = &detail.target;
        let kind = target.object.kind;
        let glass = self.uses_fluid_glass();
        let controls = self.glass_controls(cx);
        let high_contrast = self.fluid_high_contrast(cx);
        let ink = detail_style::Ink::new(glass, high_contrast);
        let compact = compact || (glass && short);
        let glass_compact = glass && compact;
        content = content.when(glass_compact, |content| content.gap(px(8.)));
        let page = detail.data.as_ref().map(|data| data.page.as_ref());
        let recording = (kind == ObjectKind::Track).then_some(page).flatten();
        let playing_recording = recording.is_some()
            && self
                .player
                .read(cx)
                .current_entry()
                .and_then(|entry| navigation::track_target(&entry.playlist, &entry.track))
                .is_some_and(|playing| {
                    playing.object == target.object
                        && playing.profile.as_deref()
                            == detail
                                .data
                                .as_ref()
                                .and_then(|data| data.profile.as_deref())
                });
        let release = detail_style::ReleasePresentation::for_page(page);
        let key = detail.data.as_ref().map(playlist_key);
        let title = page
            .map(|page| page.title.as_str())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                (detail.source_hints_visible() && !target.title.is_empty())
                    .then_some(target.title.as_str())
            })
            .unwrap_or(kind.label());
        let artwork = page.and_then(|page| page.artwork.as_deref()).or_else(|| {
            detail
                .source_hints_visible()
                .then_some(target.artwork.as_deref())
                .flatten()
        });
        let banner = page
            .and_then(|page| page.artist_details.as_ref())
            .and_then(|details| details.banner.as_deref());
        let hero = Hero::new(
            kind,
            content_width,
            compact,
            short,
            artwork.is_some() || banner.is_some(),
            glass,
            release,
        );
        let feature = hero.is_feature();
        content = content.when(feature, |content| content.gap(px(22.)));
        let kind_label = match kind {
            ObjectKind::Artist if glass => "Исполнитель",
            _ => self.displayed_kind_label(kind, page),
        };
        if let Some(collapsing) = collapsing {
            let columns = self.object_hero_columns_height();
            content = content.child(div().h(collapsing.expanded_height() + columns).flex_none());
        } else {
            let mut station = detail
                .data
                .as_ref()
                .and_then(|data| {
                    crate::ui::station::yandex_radio_target(
                        &data.page.reference,
                        data.profile.as_deref(),
                        title,
                        artwork,
                    )
                })
                .map(|target| {
                    if let Some(material) = controls {
                        detail_style::radio_action(
                            "music-object-station",
                            "Станция Яндекс Музыки",
                            44,
                            compact,
                            material,
                            Some(crate::providers::ProviderId::YandexMusic),
                        )
                        .on_activate(cx.listener(move |view, _, window, cx| {
                            view.open_and_start_station(target.clone(), window, cx);
                        }))
                        .into_any_element()
                    } else {
                        Button::new("music-object-station", "Станция Яндекс Музыки")
                            .variant(Variant::Default)
                            .when(feature, quiet_action)
                            .size(Size::Xs)
                            .tab_order(44)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.open_and_start_station(target.clone(), window, cx);
                            }))
                            .into_any_element()
                    }
                });
            let mut artist_radio = detail
                .data
                .as_ref()
                .filter(|data| kind == ObjectKind::Artist && self.can_start_artist_radio(data))
                .map(|data| {
                    let data = data.clone();
                    if let Some(material) = controls {
                        detail_style::radio_action(
                            "music-object-artist-radio",
                            "Радио по исполнителю",
                            43,
                            compact,
                            material,
                            None,
                        )
                        .on_activate(cx.listener(move |view, _, window, cx| {
                            view.start_artist_radio(data.clone(), window, cx);
                        }))
                        .into_any_element()
                    } else {
                        Button::new("music-object-artist-radio", "Радио по исполнителю")
                            .variant(Variant::Default)
                            .when(feature, quiet_action)
                            .size(Size::Xs)
                            .tab_order(43)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.start_artist_radio(data.clone(), window, cx);
                            }))
                            .into_any_element()
                    }
                });
            let track_radio = kind == ObjectKind::Track
                && self
                    .object_radio_entry(0)
                    .is_some_and(|entry| entry.is_playable() && self.can_start_track_radio(&entry));
            let mut subtitle = page
                .map(|page| {
                    let mut values = Vec::new();
                    values.extend(page.year.map(|year| year.to_string()));
                    values.extend(recording.and_then(|track| track.version.clone()));
                    values.extend(page.duration.map(|ms| time_label(f64::from(ms) / 1000.)));
                    if kind == ObjectKind::Album && !page.tracks.is_empty() {
                        values.push(crate::ui::labels::track_count(page.tracks.len()));
                        if let Some(total) = page
                            .tracks
                            .iter()
                            .map(|track| track.duration.map(u64::from))
                            .sum::<Option<u64>>()
                        {
                            values.push(time_label(total as f64));
                        }
                    }
                    if let Some(audience) = page
                        .artist_details
                        .as_ref()
                        .and_then(detail_style::monthly_audience)
                    {
                        values.push(audience);
                    }
                    values.join(" · ")
                })
                .unwrap_or_default();
            if glass_compact {
                subtitle = if subtitle.is_empty() {
                    kind_label.to_owned()
                } else {
                    format!("{kind_label} · {subtitle}")
                };
            }
            let kind_glyph = match kind {
                ObjectKind::Artist => IconName::UserRound,
                ObjectKind::Album | ObjectKind::Collection | ObjectKind::Playlist => {
                    IconName::ListMusic
                }
                ObjectKind::Station | ObjectKind::Track => IconName::AudioLines,
            };
            let pin_control = pin_of_object(target).map(|pin| {
                let pin =
                    pin.with_title(page.map_or(target.title.as_str(), |page| page.title.as_str()));
                let pinned = self.is_pinned(&pin.id);
                button(
                    "pin-toggle",
                    if pinned {
                        "Открепить"
                    } else {
                        "Закрепить"
                    },
                    42,
                )
                .when_some(self.pin_status(&pin.id), |pin, status| {
                    pin.aria_description(status)
                })
                .aria_toggled(if pinned {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .h(px(32.))
                .px(px(11.))
                .gap(px(7.))
                .rounded(px(9.))
                .border_color(surface_border())
                .bg(rgb(ELEVATED))
                .when(feature, |pin| {
                    pin.bg(gpui::transparent_black()).text_color(secondary())
                })
                .when(controls.is_none(), |pin| {
                    pin.hover(|style| style.bg(rgb(DIVIDER)))
                })
                .when_some(controls, |pin, material| {
                    material.paint(pin, if pinned { Tone::Selected } else { Tone::Quiet })
                })
                .when(glass_compact, |pin| {
                    pin.size(px(tokens::size::SM))
                        .px_0()
                        .justify_center()
                        .tooltip(guise::tooltip(if pinned {
                            "Открепить"
                        } else {
                            "Закрепить"
                        }))
                })
                .child(if glass {
                    icons::animated_glyph(
                        "music-object-pin-icon",
                        icons::Glyph::Pin,
                        pinned,
                        18.,
                        None,
                        cx,
                    )
                    .into_any_element()
                } else {
                    Icon::new(if pinned {
                        IconName::PinOff
                    } else {
                        IconName::Pin
                    })
                    .pixel_size(px(15.))
                    .color(gpui::Hsla::from(rgb(crate::ui::theme::ACCENT)))
                    .into_any_element()
                })
                .when(!glass_compact, |pin| {
                    pin.child(div().text_size(px(12.)).child(if pinned {
                        "Открепить"
                    } else {
                        "Закрепить"
                    }))
                })
                .on_activate(cx.listener(move |view, _, _, cx| {
                    view.toggle_pin(pin.clone(), cx);
                }))
            });
            let playable = page
                .zip(key.as_ref())
                .is_some_and(|(page, key)| page.tracks.iter().any(|track| can_resolve(key, track)));
            let optical = glass && matches!(kind, ObjectKind::Artist | ObjectKind::Album);
            let (inline_pin, pin_control) = if optical && playable {
                (pin_control, None)
            } else {
                (None, pin_control)
            };
            let stage = hero.stage();
            let total_seconds = page.and_then(|page| {
                page.tracks
                    .iter()
                    .map(|track| track.duration.map(u64::from))
                    .sum::<Option<u64>>()
                    .filter(|total| *total > 0)
                    .or_else(|| page.duration.map(|millis| u64::from(millis) / 1000))
            });
            let track_total = page
                .map(|page| page.tracks.len())
                .filter(|count| *count > 0);
            let byline = page.zip(key.as_ref()).and_then(|(page, key)| {
                entity_link::page_artists(
                    div()
                        .text_size(px(if stage && !compact { 16. } else { 12. }))
                        .line_height(px(if stage && !compact { 22. } else { 16. }))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if stage {
                            ink.primary.opacity(0.90)
                        } else {
                            ink.secondary
                        }),
                    page,
                    key,
                    cx,
                )
            });
            let album_link = page.zip(key.as_ref()).and_then(|(page, key)| {
                let name = page
                    .album
                    .as_deref()
                    .filter(|_| kind == ObjectKind::Track)?;
                let destination =
                    navigation::related(key, ObjectKind::Album, name, page.album_ref.as_ref());
                Some(entity_link::text(
                    div().truncate().text_color(ink.secondary),
                    "music-object-album",
                    name,
                    destination,
                    36,
                    cx,
                ))
            });
            let mut facts = Vec::new();
            if let Some(page) = page.filter(|_| stage) {
                if let Some(year) = page.year {
                    facts.push(year.to_string());
                }
                if let Some(count) = track_total {
                    facts.push(crate::ui::labels::track_count(count));
                }
                if let Some(seconds) = total_seconds {
                    facts.push(detail_style::duration_label(seconds));
                }
            }
            let ledger = stage
                && !compact
                && kind != ObjectKind::Artist
                && content_width >= 1260.
                && (kind != ObjectKind::Album
                    || release == detail_style::ReleasePresentation::Album)
                && page.is_some();
            let aside = ledger.then(|| {
                let value = |text: Option<String>| text.unwrap_or_else(|| "—".to_owned());
                let mut rows = Vec::new();
                if kind == ObjectKind::Album {
                    rows.push((
                        "ГОД",
                        value(page.and_then(|page| page.year).map(|year| year.to_string())),
                    ));
                }
                rows.push(("ТРЕКИ", value(track_total.map(|count| count.to_string()))));
                rows.push((
                    "ДЛИТЕЛЬНОСТЬ",
                    value(total_seconds.map(detail_style::duration_label)),
                ));
                detail_style::catalogue(rows, ink)
            });
            let artist_details = page.and_then(|page| page.artist_details.as_ref());
            let listeners = artist_details.and_then(|details| details.monthly_listeners);
            let audience = artist_details
                .and_then(|details| details.monthly_listeners_caption.as_ref())
                .filter(|_| listeners.is_none() && stage && kind == ObjectKind::Artist);
            let hero_body = hero
                .body()
                .when(stage, |body| {
                    body.child(detail_style::eyebrow(
                        kind_label.to_uppercase(),
                        ink,
                        compact,
                    ))
                })
                .when(!stage && !glass_compact, |body| {
                    body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(tokens::text::CAPTION))
                            .text_color(ink.secondary)
                            .when(
                                glass && matches!(kind, ObjectKind::Artist | ObjectKind::Album),
                                |label| label.letter_spacing(0.12).child(kind_label.to_uppercase()),
                            )
                            .when(
                                !glass || !matches!(kind, ObjectKind::Artist | ObjectKind::Album),
                                |label| {
                                    label
                                        .child(
                                            Icon::new(kind_glyph)
                                                .pixel_size(px(15.))
                                                .color(ink.secondary),
                                        )
                                        .child(kind_label)
                                },
                            ),
                    )
                })
                .child({
                    let title_control =
                        hero.title(heading("music-object-title", title.to_owned()), title);
                    if glass {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                title_control
                                    .min_w_0()
                                    .text_color(ink.primary)
                                    .tooltip(guise::tooltip(title.to_owned())),
                            )
                            .when(page.is_some(), |title| {
                                title.child(div().w(px(24.)).flex_none().when(
                                    self.music.request.is_some(),
                                    |slot| {
                                        slot.child(refresh_indicator(
                                            "music-object-loading",
                                            "Обновляем данные…",
                                            cx,
                                        ))
                                    },
                                ))
                            })
                            .into_any_element()
                    } else {
                        title_control.into_any_element()
                    }
                })
                .children(byline)
                .children(album_link)
                .when(playing_recording, |body| {
                    body.children(self.player.read(cx).sound_details(false))
                })
                .children(recording.and_then(|track| track.isrc.as_ref()).map(|isrc| {
                    div()
                        .text_color(ink.secondary)
                        .text_size(px(tokens::text::CAPTION))
                        .child(format!("ISRC · {isrc}"))
                }))
                .children(
                    recording
                        .and_then(|_| self.object_radio_entry(0))
                        .map(|entry| div().flex().child(self.track_lyrics_action(entry, cx))),
                )
                .when_some(
                    listeners.filter(|_| stage && kind == ObjectKind::Artist),
                    |body, listeners| {
                        body.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(if compact { 7. } else { 11. }))
                                .child(
                                    div()
                                        .text_size(px(if compact { 18. } else { 30. }))
                                        .line_height(px(if compact { 20. } else { 34. }))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ink.primary)
                                        .child(detail_style::group_number(listeners)),
                                )
                                .child(
                                    div()
                                        .text_size(px(if compact { 10. } else { 12. }))
                                        .text_color(ink.secondary)
                                        .child("слушателей за месяц"),
                                ),
                        )
                    },
                )
                .when_some(audience, |body, caption| {
                    body.child(
                        div()
                            .text_size(px(if compact { 12. } else { 16. }))
                            .text_color(ink.secondary)
                            .child(caption.clone()),
                    )
                })
                .when(stage && !facts.is_empty() && !ledger, |body| {
                    body.child(
                        div()
                            .text_size(px(if compact { 11. } else { 13. }))
                            .line_height(px(if compact { 14. } else { 17. }))
                            .text_color(ink.secondary)
                            .line_clamp(1)
                            .text_ellipsis()
                            .child(facts.join(" · ")),
                    )
                })
                .when(!stage && !subtitle.is_empty(), |body| {
                    body.child(
                        div()
                            .text_color(ink.secondary)
                            .when(feature, |subtitle| subtitle.text_size(px(15.)))
                            .child(subtitle),
                    )
                })
                .when(playable, |body| {
                    body.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .pt(px(if stage {
                                if compact {
                                    tokens::space::XS
                                } else {
                                    tokens::space::SM
                                }
                            } else if feature {
                                6.
                            } else {
                                3.
                            }))
                            .child({
                                let click = cx.listener(|view, _, _, cx| {
                                    view.play_object(None, PlaybackAction::Play, cx)
                                });
                                if glass {
                                    let current = key == self.player_snapshot.key && key.is_some();
                                    let status = current.then_some(&self.player_snapshot.status);
                                    let playing =
                                        matches!(status, Some(crate::playback::Status::Playing));
                                    let paused =
                                        matches!(status, Some(crate::playback::Status::Paused));
                                    let loading =
                                        matches!(status, Some(crate::playback::Status::Loading));
                                    let (label, state) = if loading {
                                        ("Загрузка…", icons::GlyphState::Loading)
                                    } else if playing {
                                        ("Пауза", icons::GlyphState::Rest)
                                    } else {
                                        ("Слушать", icons::GlyphState::Rest)
                                    };
                                    icons::hero_play(
                                        "music-object-play",
                                        label,
                                        playing,
                                        if compact { 40. } else { 56. },
                                        37,
                                        state,
                                        cx,
                                    )
                                    .when(!loading, |button| {
                                        button.on_activate(cx.listener(move |view, _, _, cx| {
                                            if playing || paused {
                                                view.player.update(cx, |bar, cx| {
                                                    bar.command(
                                                        cx,
                                                        crate::playback::Playback::toggle_pause,
                                                    )
                                                });
                                            } else {
                                                view.play_object(None, PlaybackAction::Play, cx);
                                            }
                                        }))
                                    })
                                    .into_any_element()
                                } else {
                                    detail_style::page_primary(
                                        Button::new("music-object-play", "Слушать")
                                            .variant(Variant::Light)
                                            .tab_order(37)
                                            .when(feature && !glass, |play| {
                                                play.variant(Variant::Filled)
                                                    .color(primary())
                                                    .radius(Size::Sm)
                                                    .left_section(
                                                        Icon::new(IconName::Play)
                                                            .pixel_size(px(15.))
                                                            .color(gpui::Hsla::from(rgb(WINDOW))),
                                                    )
                                            })
                                            .when(!feature && !glass, |play| {
                                                play.right_section(
                                                    Icon::new(IconName::Play)
                                                        .pixel_size(px(15.))
                                                        .color(primary()),
                                                )
                                            })
                                            .on_click(click),
                                        false,
                                        artwork.or(banner),
                                    )
                                    .into_any_element()
                                }
                            })
                            .when(kind != ObjectKind::Track, |bar| {
                                let shuffle = if glass && !compact && kind != ObjectKind::Artist {
                                    icons::secondary_action(
                                        "music-object-shuffle",
                                        "Перемешать",
                                        Some(icons::Glyph::Shuffle),
                                        false,
                                        38,
                                        true,
                                        cx,
                                    )
                                    .on_activate(cx.listener(|view, _, _, cx| {
                                        view.play_object(None, PlaybackAction::Shuffle, cx);
                                    }))
                                    .into_any_element()
                                } else {
                                    control(
                                        "music-object-shuffle",
                                        "Перемешать и слушать",
                                        IconName::Shuffle,
                                        38,
                                        true,
                                        false,
                                        false,
                                    )
                                    .when(feature || glass, feature_action)
                                    .when_some(controls, detail_style::secondary_icon)
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.play_object(None, PlaybackAction::Shuffle, cx)
                                    }))
                                    .into_any_element()
                                };
                                bar.child(shuffle)
                            })
                            .child(
                                control(
                                    "music-object-enqueue",
                                    "Добавить в очередь",
                                    IconName::ListEnd,
                                    39,
                                    true,
                                    false,
                                    false,
                                )
                                .when(feature || glass, feature_action)
                                .when_some(controls, detail_style::secondary_icon)
                                .on_click(cx.listener(
                                    |view, _, _, cx| {
                                        view.play_object(None, PlaybackAction::Append, cx)
                                    },
                                )),
                            )
                            .children(
                                recording
                                    .and_then(|track| track.tracks.first())
                                    .zip(key.as_ref())
                                    .map(|(track, key)| {
                                        self.like_control(
                                            "music-object-track-like",
                                            40,
                                            key,
                                            track,
                                            cx,
                                        )
                                    }),
                            )
                            .when(track_radio || recording.is_some(), |bar| {
                                bar.child(self.playlist_menu_trigger(
                                    "music-object-track-menu",
                                    "Действия трека",
                                    IconName::Ellipsis,
                                    40,
                                    MenuKind::ObjectTrack(0),
                                    cx,
                                ))
                            })
                            .when(glass, |bar| {
                                bar.children(artist_radio.take()).children(station.take())
                            })
                            .children(inline_pin),
                    )
                })
                .when(
                    pin_control.is_some() || artist_radio.is_some() || station.is_some(),
                    |body| {
                        body.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(7.))
                                .pt(px(if stage {
                                    if compact { 1. } else { 6. }
                                } else if feature {
                                    6.
                                } else {
                                    3.
                                }))
                                .children(pin_control)
                                .children(artist_radio)
                                .children(station),
                        )
                    },
                )
                .when(glass && !detail.hidden && !in_release, |body| {
                    body.children(self.music_link_actions(detail, title, cx))
                });
            let hero_frame = hero.frame(
                HeroArtwork {
                    image: artwork,
                    banner,
                    fallback: crate::ui::fluid_glass::artwork::Seed::object(&target.object),
                    scale: 1.,
                },
                hero_body,
                aside,
                high_contrast,
                self.music_hero_motion(),
            );
            content = content.child(hero_frame);
        }
        let loading = (self.music.request.is_some() && !(glass && page.is_some())).then(|| {
            if glass {
                loading_state("music-object-loading", "Загружаем страницу…", cx)
            } else {
                status(
                    "music-object-loading",
                    if page.is_some() {
                        "Обновляем данные…"
                    } else {
                        "Загружаем страницу…"
                    },
                )
            }
        });
        let error = detail
            .error
            .as_ref()
            .filter(|error| !glass || detail.hidden || !error.needs_sign_in)
            .map(|error| {
                let message = status("music-object-error", error.message.clone());
                let action = if error.needs_sign_in {
                    self.provider_recovery_action(
                        "music-object-connect",
                        target.object.provider,
                        39,
                        cx,
                    )
                    .variant(Variant::Light)
                } else {
                    let target = target.clone();
                    Button::new("music-object-retry", "Повторить")
                        .variant(Variant::Default)
                        .when(feature || glass, quiet_action)
                        .tab_order(39)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.load_music_object(target.clone(), cx);
                        }))
                };
                if glass && page.is_none() {
                    div().child(
                        error_state("music-object-error", error.message.clone(), None, cx)
                            .child(action.size(Size::Xs)),
                    )
                } else if feature || glass_compact {
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(if glass_compact { 8. } else { 12. }))
                        .child(message.text_size(px(13.)))
                        .child(action.size(Size::Xs))
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(15.))
                        .child(message)
                        .child(div().flex().child(action))
                }
            });
        let show_tracks_first = glass_compact
            && kind != ObjectKind::Track
            && page.is_some_and(|page| !page.tracks.is_empty());
        let mut status = (loading.is_some() || error.is_some()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(if glass {
                    5.
                } else if feature {
                    22.
                } else {
                    15.
                }))
                .children(loading)
                .children(error)
        });
        if !show_tracks_first || detail.hidden {
            content = content.children(status.take());
        }
        if detail.hidden {
            return content;
        }
        if !glass && !show_tracks_first && !in_release {
            content = content.children(self.music_link_actions(detail, title, cx));
        }
        if let Some((data, key)) = detail.data.as_ref().zip(key) {
            let related = self.related_artists(data, compact, feature, cx);
            let track_width = if related.is_some() && !compact {
                content_width - 238. - if feature { 28. } else { 13. }
            } else {
                content_width
            };
            let tracks = self.object_tracks(
                detail,
                &data.page,
                &key,
                (compact, track_width),
                collapsing.map(|hero| hero.height() + self.object_hero_columns_height()),
                cx,
            );
            if let Some(material) = controls.filter(|_| kind == ObjectKind::Artist) {
                return content.child(artist_sections::ArtistSections {
                    tracks: tracks.map(IntoElement::into_any_element),
                    related: related.map(IntoElement::into_any_element),
                    releases: self
                        .object_releases(detail, data, compact, cx)
                        .map(IntoElement::into_any_element),
                    biography: self
                        .artist_biography(&data.page, cx)
                        .map(IntoElement::into_any_element),
                    notice: status.take(),
                    compact,
                    section_gap: px(if feature { 28. } else { 13. }),
                    scroll: self.scroll.clone(),
                    clearance: collapsing.map_or(gpui::Pixels::ZERO, |_| {
                        CollapsingMediaHero::collapsed_height()
                    }),
                    material,
                });
            }
            content = content
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(if feature { 28. } else { 13. }))
                        .when(compact, |section| section.flex_col())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(compact, |section| section.w_full())
                                .when(feature && kind == ObjectKind::Album, |section| {
                                    section.max_w(px(super::super::design_system::adaptive_layout::AdaptiveContent::resolve(track_width, None).table_width()))
                                })
                                .children(tracks),
                        )
                        .children(related),
                )
                .children(status.take())
                .children(self.object_releases(detail, data, compact, cx))
                .children(self.artist_biography(&data.page, cx));
        }
        content
            .children(recording.and_then(|page| self.track_credits_block(page.tracks.first()?, cx)))
    }

    pub(super) fn play_object(
        &mut self,
        selected: Option<usize>,
        action: PlaybackAction,
        cx: &mut Context<Self>,
    ) {
        let Some(data) = self
            .displayed_music_object()
            .and_then(|detail| detail.data.clone())
        else {
            return;
        };
        let page = &data.page;
        let key = playlist_key(&data);
        if page.reference.kind == ObjectKind::Album && !matches!(action, PlaybackAction::Append) {
            let plan = Arc::new(CollectionPlan::new(
                key.clone(),
                &page.title,
                page.artwork.as_deref(),
                &page.tracks,
            ));
            let first = selected.unwrap_or_default();
            let entries = plan
                .entries
                .iter()
                .filter(|entry| entry.index >= first)
                .cloned()
                .collect::<Vec<_>>();
            if entries.is_empty() {
                return;
            }
            self.ensure_playlist_favorites(&key, cx);
            self.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| match action {
                    PlaybackAction::Shuffle => playback.shuffle_collection(entries, plan),
                    _ => playback.play_collection(entries, 0, plan),
                });
            });
            return;
        }
        self.play_music_tracks(
            page.tracks
                .iter()
                .skip(selected.unwrap_or_default())
                .map(|track| (key.clone(), track)),
            action,
            cx,
        );
    }
}

fn feature_action(control: guise::ActionIcon) -> guise::ActionIcon {
    control
        .pixel_size(px(36.))
        .radius(Size::Sm)
        .variant(Variant::Subtle)
        .color(primary())
}

fn quiet_action(button: Button) -> Button {
    button.variant(Variant::Subtle).color(secondary())
}
