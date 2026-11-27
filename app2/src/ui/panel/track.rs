//! The track tab of the right panel: the selected playlist row, otherwise
//! the sounding record.

mod controls;

use std::time::Instant;

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, Role, SpanId, Stateful, Window, div,
    prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use crate::{
    playback::{QueueEntry, SoundDetails, SoundSource, can_resolve},
    playlist::PlaylistKey,
    ui::{
        MainView,
        assets::provider_mark,
        components::{
            Activate, button,
            content_state::empty_state,
            entity_link,
            glass_control::Material,
            music::{cover, fluid_cover, time_label},
            scroll_viewport::{Density, FluidScrollViewport},
            scrollbar::ScrollSurface,
        },
        design_system::{
            controls::{ControlState, ControlVariant},
            motion::{MOTION, out_quad},
        },
        fluid_glass::{
            artwork::Seed,
            tokens::{size, space, text},
        },
        labels::readable_date,
        music::{
            detail::hero_motion::{HeroMotion, cover_span, shared_artwork},
            detail_style::Ink,
        },
        playlist::{
            artwork::track_cover,
            navigation::{self, Destination, track_target},
        },
        theme::{
            ACCENT, ELEVATED, SURFACE, playlist, playlist::metrics, secondary, surface_border,
        },
        workspace::Page,
    },
};

const INSPECTOR_ARTWORK_SPAN: SpanId = 0x6d65646961696e73;

pub(in crate::ui) fn playlist_track_artwork_span(key: &PlaylistKey, index: usize) -> SpanId {
    cover_span(("playlist-track", Seed::collection(key), index))
}

#[derive(Default)]
pub(super) struct InspectorTransition {
    started: Option<Instant>,
    artwork: Option<HeroMotion>,
    source: Option<SpanId>,
}

impl MainView {
    pub(in crate::ui) fn point_inspector_at_playlist_track(
        &mut self,
        source: SpanId,
        use_artwork: bool,
        window: &Window,
        cx: &Context<Self>,
    ) {
        self.panel.follow_playing_track = false;
        if self.reduced_motion
            || cx.reduce_motion()
            || !self.uses_fluid_glass()
            || self.fluid_high_contrast(cx)
            || self.capture_time.is_some()
        {
            return;
        }
        let already_open = self.panel.open && self.panel.tab == crate::storage::PanelTab::Track;
        let artwork = use_artwork
            .then(|| {
                window.rendered_span(if already_open {
                    INSPECTOR_ARTWORK_SPAN
                } else {
                    source
                })
            })
            .flatten()
            .map(|origin| {
                let duration = if already_open {
                    MOTION.fast()
                } else {
                    MOTION.slow()
                };
                HeroMotion::with_duration(origin, INSPECTOR_ARTWORK_SPAN, duration)
            });
        self.panel.inspector = InspectorTransition {
            started: Some(Instant::now()),
            artwork,
            source: artwork.filter(|_| !already_open).map(|_| source),
        };
    }

    pub(in crate::ui) fn inspector_is_moving(&self, source: SpanId) -> bool {
        self.panel.open
            && self.panel.inspector.source == Some(source)
            && self.panel.inspector.artwork.is_some_and(HeroMotion::active)
    }

    pub(super) fn track_section(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let player = self.player.read(cx);
        let playlist_view = self.music_page() == Page::Playlist;
        let controls = self.glass_controls(cx);
        let glass = controls.is_some();
        let ink = Ink::new(glass, self.fluid_high_contrast(cx));
        let selected = if self.panel.follow_playing_track {
            None
        } else {
            self.selected_playlist_track()
        };
        let subject = selected
            .map(|(key, revision, index, track)| (key, revision, index, track, false))
            .or_else(|| {
                self.panel
                    .inspected_track
                    .as_ref()
                    .filter(|_| !self.panel.follow_playing_track)
                    .map(|entry| {
                        (
                            &entry.playlist,
                            entry.playlist_revision,
                            entry.index,
                            &entry.track,
                            false,
                        )
                    })
            })
            .or_else(|| {
                player.current_entry().map(|entry| {
                    (
                        &entry.playlist,
                        entry.playlist_revision,
                        entry.index,
                        &entry.track,
                        true,
                    )
                })
            });
        let Some((key, revision, index, track, playing)) = subject else {
            return empty_state(
                "right-panel-track-empty",
                "Трек не выбран",
                Some("Включите трек или откройте сведения из списка."),
                IconName::Music2,
                cx,
            )
            .into_any_element();
        };
        let playable = can_resolve(key, track);
        let sounding = player.current_entry().is_some_and(|entry| {
            &entry.playlist == key && entry.playlist_revision == revision && entry.index == index
        });
        let url: Option<String> = track_target(key, track).and_then(|target| target.object.url());
        let entry = selected.is_none().then(|| QueueEntry {
            playlist: key.clone(),
            playlist_revision: revision,
            recommendations: Vec::new(),
            index,
            track: track.clone(),
        });
        let compact = window.viewport_size().height < px(500.);
        let (artwork_edge, title_size, title_line) = match (playlist_view && !glass, compact) {
            (true, true) => (metrics::COVER / 2., 16., 20.),
            (true, false) => (metrics::COVER, 19., 24.),
            (false, true) => (76., 19., 24.),
            (false, false) => (155., 23., 30.),
        };
        let artwork = if glass {
            fluid_cover(
                track.artwork.as_deref(),
                Seed::track(track),
                artwork_edge,
                8.,
            )
        } else if playlist_view && track.artwork.is_none() {
            track_cover(None, artwork_edge).bg(rgb(playlist::PAPER))
        } else {
            cover(
                track.artwork.as_deref(),
                artwork_edge,
                if playlist_view { playlist::RADIUS } else { 12. },
                true,
            )
            .when(playlist_view, |art| art.bg(rgb(playlist::PAPER)))
        };
        let transition = &self.panel.inspector;
        let artwork = if glass {
            shared_artwork(artwork, INSPECTOR_ARTWORK_SPAN, transition.artwork)
        } else {
            artwork.into_any_element()
        };
        let artwork_target = navigation::album(key, track)
            .filter(|(_, target)| matches!(target, Destination::Object(_)))
            .or_else(|| {
                navigation::track_destination(key, track)
                    .map(|target| (track.title.as_str(), target))
            });
        let artwork = if let Some((name, target)) = artwork_target {
            entity_link::artwork(
                artwork,
                "playlist-inspector-artwork-link",
                name,
                target,
                glass.then_some(INSPECTOR_ARTWORK_SPAN),
                593,
                cx,
            )
        } else {
            artwork
        };
        let opacity = transition.started.map_or(1., |started| {
            let phase = started.elapsed().as_secs_f32() / MOTION.normal().as_secs_f32();
            if phase < 1. && window.is_visible() && window.is_window_active() {
                window.request_animation_frame();
            }
            0.78 + 0.22 * out_quad(phase)
        });
        let short = glass && compact;
        let artwork = div()
            .flex_none()
            .pt(px(if short { 0. } else { space::XS }))
            .when(playlist_view && !glass, |art| {
                art.pt_0().flex().justify_center()
            })
            .child(artwork);
        let text = div()
            .min_w_0()
            .flex()
            .flex_col()
            .opacity(opacity)
            .gap(px(space::XS))
            .child(entity_link::text(
                div()
                    .line_clamp(2)
                    .text_size(px(title_size))
                    .line_height(px(title_line))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(glass, |title| title.text_color(ink.primary)),
                "playlist-inspector-title",
                &track.title,
                navigation::track_destination(key, track),
                594,
                cx,
            ))
            .when(!track.artist.trim().is_empty(), |info| {
                info.child(entity_link::artist_names(
                    div()
                        .text_size(px(text::METADATA))
                        .line_height(px(17.))
                        .text_color(ink.secondary),
                    "playlist-inspector-artist-link",
                    key,
                    track,
                    594,
                    cx,
                ))
            })
            .when(sounding && !glass, |info| {
                info.children(player.sound_details(false))
            });
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(if glass { space::SM } else { metrics::GAP }))
            .child(self.inspector_play(playlist_view, playable, playing, index, entry.as_ref(), cx))
            .child(self.like_control("playlist-inspector-like", 595, key, track, cx))
            .child(self.inspector_enqueue(playlist_view, playable, entry, index, cx));
        let overview = div()
            .min_w_0()
            .flex()
            .gap(px(space::MD))
            .when(short, |overview| overview.items_start())
            .when(!short, |overview| overview.flex_col())
            .child(artwork)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(space::SM))
                    .child(text)
                    .child(actions),
            );
        let details = div()
            .border_t_1()
            .opacity(opacity)
            .border_color(surface_border())
            .pt(px(space::MD))
            .when(playlist_view && !glass, |details| {
                details.border_color(rgb(playlist::RULE))
            })
            .when(glass, |details| details.border_color(ink.rule))
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(space::SM))
            .children(navigation::album(key, track).map(|(name, target)| {
                property(
                    "Альбом",
                    entity_link::text(
                        div().truncate(),
                        "playlist-inspector-album-link",
                        name,
                        Some(target),
                        596,
                        cx,
                    ),
                    ink,
                )
            }))
            .children(
                track
                    .duration
                    .map(|duration| property("Длительность", time_label(f64::from(duration)), ink)),
            )
            .children(
                track
                    .added
                    .as_deref()
                    .map(|value| property("Добавлено", readable_date(value), ink)),
            )
            .children(key.provider().map(|provider| {
                property(
                    "Площадка",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::XS))
                        .child(provider_mark(provider, 16.))
                        .child(provider.display_name()),
                    ink,
                )
            }));
        let audio = if glass && sounding {
            let (source, report) = player.sound_report();
            source.map(|source| current_sound(source, report, ink))
        } else {
            None
        };
        let split_details =
            short && !super::inline(window) && window.viewport_size().width >= px(760.);
        let metadata = div()
            .flex()
            .gap(px(space::LG))
            .when(!split_details, |metadata| metadata.flex_col())
            .child(details.when(split_details, |column| column.flex_1()))
            .children(audio.map(|audio| audio.when(split_details, |column| column.flex_1())));
        let mut panel = div()
            .id("playlist-inspector")
            .accessibility_id("playlist-inspector")
            .role(Role::Group)
            .aria_label(if playing {
                "Сейчас играет"
            } else {
                "Выбранный трек"
            })
            .relative()
            .flex()
            .flex_col()
            .gap(px(if compact { 8. } else { 12. }))
            .p(px(if compact { 12. } else { 15. }))
            .w_full()
            .bg(rgb(SURFACE))
            .rounded(px(14.))
            .border_1()
            .border_color(surface_border())
            .when(playlist_view, |panel| {
                panel
                    .gap(px(metrics::PAD))
                    .p(px(metrics::PAD))
                    .bg(rgb(playlist::PANEL))
                    .rounded_none()
                    .border_0()
            })
            .when(glass, |panel| {
                panel
                    .gap(px(if compact { space::SM } else { space::LG }))
                    .p(px(space::MD))
                    .bg(gpui::transparent_black())
                    .rounded_none()
                    .border_0()
            })
            .when(glass, |panel| {
                panel.child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(ink.secondary)
                        .child(if playing {
                            "Сейчас играет"
                        } else {
                            "Выбранный трек"
                        }),
                )
            })
            .child(overview)
            .child(metadata);
        if let Some(url) = url {
            panel = panel.child(
                div()
                    .border_t_1()
                    .border_color(surface_border())
                    .pt(px(5.))
                    .when(playlist_view, |links| {
                        links
                            .pt(px(metrics::GAP_SMALL))
                            .border_color(rgb(playlist::RULE))
                    })
                    .when(glass, |links| links.border_color(ink.rule))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        jump(
                            "playlist-inspector-open-source",
                            "Открыть на площадке",
                            IconName::ExternalLink,
                            600,
                            controls,
                        )
                        .on_activate(move |_, _, cx| cx.open_url(&url)),
                    ),
            );
        }
        let body = div()
            .id("right-panel-track-body")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.panel.track_scroll)
            .px(px(if glass {
                space::XS
            } else if playlist_view {
                metrics::GAP
            } else {
                10.
            }))
            .pb(px(if glass {
                space::MD
            } else if playlist_view {
                metrics::PAD
            } else {
                10.
            }))
            .child(panel);
        FluidScrollViewport::new(
            body,
            ScrollSurface::Area(self.panel.track_scroll.clone()),
            glass,
            Density::Panel,
        )
        .into_any_element()
    }

    fn restart_current(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| {
                if let Some(index) = playback.current_index() {
                    playback.play_queued(index);
                }
            })
        });
    }

    fn enqueue_entry(&mut self, entry: QueueEntry, cx: &mut Context<Self>) {
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.append(vec![entry]))
        });
    }
}

fn property(label: &'static str, value: impl IntoElement, ink: Ink) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(13.))
        .child(
            div()
                .w(px(104.))
                .flex_none()
                .text_color(ink.secondary)
                .child(label),
        )
        .child(div().flex_1().min_w_0().truncate().child(value))
}

fn current_sound(source: SoundSource, details: &SoundDetails, ink: Ink) -> Stateful<Div> {
    let label = details.quality.as_ref().map_or_else(
        || source.label().to_owned(),
        |quality| format!("{} · {}", source.label(), quality.label()),
    );
    let rows =
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(property("Источник", source.label(), ink));
    div()
        .id("track-panel-sound-details")
        .accessibility_id("track-panel-sound-details")
        .role(Role::Group)
        .aria_label(label)
        .min_w_0()
        .border_t_1()
        .border_color(ink.rule)
        .pt(px(space::MD))
        .child(rows.when_some(details.quality.as_ref(), |rows, quality| {
            rows.child(property("Формат", quality.codec_label(), ink))
                .children(
                    quality
                        .bitrate_label()
                        .map(|bitrate| property("Битрейт", bitrate, ink)),
                )
                .child(property("Частота", quality.sample_rate_label(), ink))
        }))
}

fn jump(
    id: &'static str,
    label: &'static str,
    glyph: IconName,
    order: isize,
    controls: Option<Material>,
) -> Stateful<Div> {
    let link = button(id, label, order)
        .w_full()
        .h(px(29.))
        .px(px(6.))
        .gap(px(8.))
        .rounded(px(6.))
        .text_size(px(13.));
    let icon = Icon::new(glyph).pixel_size(px(15.));
    let chevron = Icon::new(IconName::ChevronRight).pixel_size(px(14.));
    let (link, icon, chevron) = if let Some(controls) = controls {
        (
            ControlVariant::TextAction.paint(
                link.h(px(size::SM)).px(px(space::SM)).gap(px(space::SM)),
                controls,
                ControlState::default(),
            ),
            icon,
            chevron,
        )
    } else {
        (
            link.hover(|style| style.bg(rgb(ELEVATED))),
            icon.color(gpui::Hsla::from(rgb(ACCENT))),
            chevron.color(secondary()),
        )
    };
    link.child(icon)
        .child(div().flex_1().min_w_0().truncate().child(label))
        .child(chevron)
}
