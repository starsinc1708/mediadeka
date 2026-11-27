mod shelf;

use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, IntoElement, Role, SharedString, Stateful, div,
    linear_color_stop, linear_gradient, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use crate::{
    music::{ObjectKind, ObjectRef},
    providers::{Home, HomeItem, HomeSection, HomeSectionPage, ProviderId},
    state::{HomeContent, HomeView, Issue},
    ui::{
        assets::provider_mark,
        components::{
            entity_link,
            glass_control::{Material, Tone},
            music::card_title,
            track_row,
        },
        design_system::controls::ControlVariant,
        fluid_glass::{artwork as fluid_artwork, tokens},
        music::detail::hero_motion::{cover_span, shared_artwork},
        playlist::navigation::Destination,
        theme::{
            HOME_COVER_PALETTE, HOME_COVER_SHADE, HOME_ITEM_HOVER, HOME_ROW_BACKGROUND,
            NOTICE_BACKGROUND, navigation_accent as accent, secondary,
        },
        workspace::MusicTarget,
    },
};

use super::{
    Activate, Feed, FeedContext, Layout, MainView, SOUNDCLOUD_ACTIVITY, artwork, button,
    open_track, openable,
};
use shelf::ShelfSurface;

const CARD_LABEL_HEIGHT: f32 = 50.;
const RELEASE_LABEL_HEIGHT: f32 = 56.;
const COLLECTION_LABEL_HEIGHT: f32 = 46.;

#[derive(Clone, Copy)]
enum ItemStyle {
    Card,
    Artist,
    Release,
    Collection,
    Quick,
    Row,
}

impl ItemStyle {
    fn square_cover(self) -> bool {
        matches!(
            self,
            Self::Card | Self::Artist | Self::Release | Self::Collection
        )
    }

    fn label_height(self) -> f32 {
        match self {
            Self::Release => RELEASE_LABEL_HEIGHT,
            Self::Collection => COLLECTION_LABEL_HEIGHT,
            _ => CARD_LABEL_HEIGHT,
        }
    }
}

impl MainView {
    pub(super) fn music_content(
        &self,
        view: &HomeView,
        source: FeedContext<'_>,
        feed: Feed<'_>,
        layout: Layout,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let collection = feed.snapshot().map(|snapshot| &snapshot.home);
        let mut content = home_section(view, self.home_notice_rule(cx))
            .children(self.feed_notice(source, feed, cx));
        content = if source.provider == ProviderId::SoundCloud {
            content.child(self.soundcloud_home(collection, source, layout, cx))
        } else {
            content.child(self.feed(collection, source, ShelfSurface::Home, layout, cx))
        };
        content.when(
            collection.is_some_and(|home| home.items.is_empty()),
            |body| {
                if layout.glass.is_some() {
                    body.child(crate::ui::components::content_state::empty_state(
                        "home-empty-tracks",
                        "В подборке пока ничего нет",
                        None,
                        IconName::Music2,
                        cx,
                    ))
                } else {
                    body.child(label("home-empty-tracks", "В подборке пока ничего нет."))
                }
            },
        )
    }

    pub(super) fn combined_music_content(
        &self,
        view: &HomeView,
        mut feeds: Vec<(FeedContext<'_>, Feed<'_>)>,
        layout: Layout,
        cx: &Context<Self>,
    ) -> Div {
        feeds.sort_by_key(|(source, feed)| {
            (
                std::cmp::Reverse(feed.snapshot().map(|snapshot| snapshot.received_at_ms)),
                source.provider,
            )
        });
        let has_content = feeds.iter().any(|(_, feed)| {
            feed.snapshot()
                .is_some_and(|snapshot| !snapshot.home.items.is_empty())
        });
        let busy = feeds.iter().any(|(_, feed)| feed.refreshing);
        home_section(view, self.home_notice_rule(cx))
            .children(
                feeds
                    .iter()
                    .filter_map(|(source, feed)| self.feed_notice(*source, *feed, cx)),
            )
            .child(
                self.combined_feed(
                    feeds
                        .iter()
                        .filter_map(|(source, feed)| Some((*source, &feed.snapshot()?.home))),
                    layout,
                    cx,
                ),
            )
            .when(!has_content && !busy, |body| {
                body.child(crate::ui::components::content_state::empty_state(
                    "home-empty-tracks",
                    "В подборках пока ничего нет.",
                    Some("Подборки появятся, когда площадки вернут рекомендации."),
                    guise::IconName::Music2,
                    cx,
                ))
            })
    }

    /// SoundCloud keeps the platform's own shelf order: recommendations,
    /// the subscription feed and the library shelves.
    fn soundcloud_home(
        &self,
        collection: Option<&Home>,
        source: FeedContext<'_>,
        layout: Layout,
        cx: &Context<Self>,
    ) -> Div {
        let activity = collection.is_some_and(soundcloud_activity_visible);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(layout.section_gap()))
            .child(self.feed(collection, source, ShelfSurface::SoundCloudHome, layout, cx))
            .when(activity, |body| {
                body.children(
                    collection
                        .map(|home| soundcloud_activity(self, home, source, layout.glass, cx)),
                )
            })
            .child(self.feed(
                collection,
                source,
                ShelfSurface::SoundCloudLibrary,
                layout,
                cx,
            ))
    }

    fn feed_notice(
        &self,
        source: FeedContext<'_>,
        feed: Feed<'_>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let issue = feed.issue();
        let rule = self.home_notice_rule(cx);
        if !source.combined && feed.refreshing && issue.is_none() {
            if self.uses_fluid_glass() && feed.snapshot().is_some() {
                return None;
            }
            return Some(notice(
                "home-refreshing".into(),
                "Обновляем подборку…".into(),
                rule,
            ));
        }
        let issue = issue?;
        let (id, detail) = match feed.content? {
            HomeContent::Partial { .. } => (
                "home-partial-notice",
                format!("Часть разделов не обновилась. {issue}"),
            ),
            HomeContent::Cached { .. } => (
                "home-cache-notice",
                format!("Показываем сохранённую подборку. {issue}"),
            ),
            _ => ("home-empty-notice", issue.to_string()),
        };
        let (id, detail) = if source.combined {
            (
                format!("{id}-{}", source.provider.as_str()),
                format!("{} · {detail}", source.provider.display_name()),
            )
        } else {
            (id.to_owned(), detail)
        };
        let retry = source.key();
        let reconnect = retry.profile.is_some()
            && matches!(issue, Issue::Provider(error) if error.needs_sign_in());
        let label = if reconnect {
            "Войти снова"
        } else {
            "Повторить"
        };
        let retry_id = if source.combined {
            format!("home-retry-{}", source.provider.as_str())
        } else {
            "home-retry".into()
        };
        Some(notice(id, detail, rule).when(!feed.refreshing, |row| {
            row.child(
                guise::Button::new(SharedString::from(retry_id), label)
                    .variant(guise::Variant::Default)
                    .accessible_name(label)
                    .size(guise::Size::Xs)
                    .tab_order(31)
                    .when_some(self.glass_controls(cx), |button, material| {
                        ControlVariant::TextAction.text_button(button, material)
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if reconnect {
                            view.show_music_page(crate::ui::workspace::Page::Settings, window, cx);
                            view.open_settings_connection(retry.provider, window, cx);
                        } else {
                            view.refresh_home_source(retry.clone(), cx);
                        }
                    })),
            )
        }))
    }

    fn home_notice_rule(&self, cx: &Context<Self>) -> Option<Hsla> {
        self.uses_fluid_glass().then(|| {
            rgb(if self.fluid_high_contrast(cx) {
                tokens::contrast::RULE
            } else {
                tokens::RULE
            })
            .into()
        })
    }
}

fn home_section(view: &HomeView, rule: Option<Hsla>) -> Div {
    section().when_some(view.switch_issue.as_ref(), |body, issue| {
        body.child(notice("home-switch-notice".into(), issue.to_string(), rule))
    })
}

fn notice(id: String, detail: String, rule: Option<Hsla>) -> Div {
    let row = div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(9.))
        .child(label(SharedString::from(id), detail).min_w_0().flex_1());
    if let Some(rule) = rule {
        row.pl(px(11.)).py(px(7.)).border_l_1().border_color(rule)
    } else {
        row.p(px(11.)).rounded(px(6.)).bg(rgb(NOTICE_BACKGROUND))
    }
}

fn section_items<'a>(home: &'a Home, group: &HomeSection) -> Option<&'a [HomeItem]> {
    home.items
        .get(group.start..group.start.checked_add(group.len)?)
}

/// The subscription feed appears on the SoundCloud home when it has tracks.
/// An empty collection is announced once by the common empty label instead.
fn soundcloud_activity_visible(home: &Home) -> bool {
    if home.sections.is_empty() {
        !home.items.is_empty()
    } else {
        home.sections
            .iter()
            .any(|section| section.page == HomeSectionPage::Activity && section.len > 0)
    }
}

fn soundcloud_activity(
    view: &MainView,
    home: &Home,
    source: FeedContext<'_>,
    glass: Option<Material>,
    cx: &Context<MainView>,
) -> Stateful<Div> {
    let legacy = home.sections.is_empty().then_some(home.items.as_slice());
    let tracks = legacy
        .into_iter()
        .chain(
            home.sections
                .iter()
                .filter(|group| group.page == HomeSectionPage::Activity)
                .filter_map(|group| section_items(home, group)),
        )
        .flatten();
    section()
        .id("home-activity-feed")
        .accessibility_id("home-activity-feed")
        .role(Role::Group)
        .aria_label(SOUNDCLOUD_ACTIVITY)
        .child(
            section_title(SOUNDCLOUD_ACTIVITY)
                .id("home-activity-title")
                .accessibility_id("home-activity-title")
                .role(Role::Heading)
                .aria_label(SOUNDCLOUD_ACTIVITY),
        )
        .children(tracks.enumerate().map(|(index, track)| {
            track_item(
                view,
                source,
                index,
                track,
                (ItemStyle::Row, glass),
                true,
                cx,
            )
        }))
}

fn section() -> Div {
    div().w_full().flex().flex_col().gap(px(9.))
}

pub(super) fn section_title(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_size(px(tokens::text::SECTION_TITLE))
        .line_height(px(28.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

fn label(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Stateful<Div> {
    let id = id.into();
    let text = text.into();
    div()
        .id(id.clone())
        .accessibility_id(id)
        .role(Role::Label)
        .aria_value(text.clone())
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(secondary())
        .child(text)
}

fn track_item(
    view: &MainView,
    source: FeedContext<'_>,
    index: usize,
    item: &HomeItem,
    (style, glass): (ItemStyle, Option<Material>),
    interactive: bool,
    cx: &Context<MainView>,
) -> AnyElement {
    let identity = source.id("home-item", index);
    let order = (40 + index) as isize;
    let object = ObjectRef::from_item(source.provider, item);
    let current = glass.and_then(|_| {
        view.player
            .read(cx)
            .catalogue_track_status(object.as_ref()?, source.profile)
    });
    let origin_span = glass
        .filter(|_| interactive)
        .and(object.as_ref())
        .filter(|object| {
            matches!(
                object.kind,
                ObjectKind::Album | ObjectKind::Artist | ObjectKind::Playlist | ObjectKind::Track
            )
        })
        .map(|_| cover_span(("home-feed-artwork", &identity, &item.id)));
    let opens_page = openable(source.provider, item);
    let card = style.square_cover();
    let body = track_layout(
        source,
        index,
        item,
        style,
        opens_page,
        glass.is_some(),
        origin_span,
    );
    let control = if !interactive {
        div().id(identity.clone()).border_1()
    } else if opens_page {
        let item = item.clone();
        let origin = source.key();
        let control = button(identity.clone(), item.title.clone(), order).aria_description(
            format!("Открыть в Mediadeka · {}", source.provider.display_name()),
        );
        if let Some(material) = glass {
            material.paint(control, Tone::Quiet)
        } else {
            control.hover(|style| style.bg(rgb(HOME_ITEM_HOVER)))
        }
        .on_activate(cx.listener(move |view, _, window, cx| {
            if let Some(span) = origin_span {
                view.navigate_from_artwork(span, window, cx, |view, window, cx| {
                    open_track(&origin, item.clone(), view, window, cx)
                });
            } else {
                open_track(&origin, item.clone(), view, window, cx)
            }
        }))
    } else {
        div()
            .id(identity.clone())
            .accessibility_id(identity.clone())
            .role(Role::ListItem)
            .aria_label(item.title.clone())
            .border_1()
    };
    let row_style = track_row::Style::new(cx, view.fluid_high_contrast(cx));
    let control = row_style
        .surface(control, current.is_some())
        .when_some(current, |tile, status| {
            tile.aria_description(track_row::current_description(status))
        })
        .min_w_0()
        .relative()
        .w_full()
        .rounded(px(6.))
        .overflow_hidden()
        .when(!card && glass.is_none(), |item| {
            item.bg(rgb(HOME_ROW_BACKGROUND))
        })
        .child(body);
    if !interactive {
        return control.into_any_element();
    }
    let hint = object.map(|object| {
        Destination::Object(MusicTarget {
            object,
            profile: source.profile.map(str::to_owned),
            title: item.title.clone(),
            artwork: item.artwork.clone(),
        })
    });
    let visibility = view.prefetch_visibility_probe(hint.iter().cloned());
    entity_link::hint(control.children(visibility), &identity, order, hint, cx)
}

fn track_layout(
    source: FeedContext<'_>,
    index: usize,
    item: &HomeItem,
    style: ItemStyle,
    opens_page: bool,
    glass: bool,
    origin: Option<gpui::SpanId>,
) -> Div {
    let feature_tile = matches!(style, ItemStyle::Release | ItemStyle::Collection);
    let centered = !glass && matches!(style, ItemStyle::Artist);
    let title_size = match style {
        ItemStyle::Release => 15.,
        ItemStyle::Collection => 14.,
        _ => 13.,
    };
    let description = div()
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            card_title(item.title.clone())
                .when(!glass, |title| {
                    title.text_size(px(title_size)).line_height(px(17.))
                })
                .when(glass && !style.square_cover(), |title| {
                    title.text_size(px(tokens::text::BODY))
                })
                .when(glass, |title| title.text_color(rgb(tokens::PRIMARY)))
                .when(centered, |title| title.text_center()),
        )
        .when(!item.subtitle.is_empty(), |body| {
            body.child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(secondary())
                    .when(centered, |subtitle| subtitle.text_center())
                    .child(item.subtitle.clone()),
            )
        });
    let details = div()
        .w_full()
        .min_w_0()
        .flex_1()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(description)
        .child(provider_mark(source.provider, 14.))
        .when(opens_page && !style.square_cover(), |body| {
            body.child(
                Icon::new(IconName::ChevronRight)
                    .pixel_size(px(13.))
                    .color(secondary()),
            )
        });
    let art = cover(source, index, item, style, glass);
    let art = if let Some(span) = origin {
        shared_artwork(art, span, None)
    } else {
        art.into_any_element()
    };
    match style {
        ItemStyle::Card | ItemStyle::Artist | ItemStyle::Release | ItemStyle::Collection => div()
            .w_full()
            .flex()
            .flex_col()
            .when(feature_tile, |tile| tile.p(px(10.)))
            .child(art)
            .child(details.px(px(8.)).py(px(7.)).h(px(style.label_height()))),
        ItemStyle::Quick | ItemStyle::Row => div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(9.))
            .px(px(6.))
            .py(px(5.))
            .child(art)
            .child(details),
    }
}

fn cover(
    source: FeedContext<'_>,
    index: usize,
    item: &HomeItem,
    style: ItemStyle,
    glass: bool,
) -> Stateful<Div> {
    let palette = HOME_COVER_PALETTE;
    let card = style.square_cover();
    let is_wave = item.id.starts_with("wave:");
    let artist_tile = matches!(style, ItemStyle::Artist);
    let circular = !glass && (artist_tile || item.is_artist() || is_wave);
    let edge = if matches!(style, ItemStyle::Quick) {
        42.
    } else {
        34.
    };
    let art = div()
        .id(source.id("track-artwork", index))
        .accessibility_id(source.id("home-artwork", index))
        .role(Role::Image)
        .aria_label("Обложка")
        .relative()
        .flex_none()
        .flex()
        .justify_center()
        .items_center()
        .overflow_hidden()
        .when(card, |art| art.w_full().aspect_ratio(1.).rounded(px(6.)))
        .when(!card, |art| art.size(px(edge)).rounded(px(5.)))
        .when(circular, |art| art.rounded_full());
    let art = if glass {
        art.bg(rgb(tokens::SURFACE))
    } else {
        art.bg(linear_gradient(
            140.,
            linear_color_stop(rgb(palette[index % palette.len()]), 0.),
            linear_color_stop(rgb(HOME_COVER_SHADE), 1.),
        ))
    };
    art.when(!is_wave || item.artwork.is_none(), |art| {
        art.child(
            Icon::new(if artist_tile {
                IconName::UserRound
            } else {
                IconName::Headphones
            })
            .pixel_size(px(if card { 31. } else { 15. }))
            .color(if glass {
                rgb(tokens::SECONDARY).into()
            } else {
                accent()
            }),
        )
    })
    .when(glass, |art| {
        art.child(fluid_artwork::thumbnail(
            item.artwork.as_deref(),
            card.then(|| ObjectRef::from_item(source.provider, item))
                .flatten()
                .as_ref()
                .map(fluid_artwork::Seed::object),
            px(if card { 6. } else { 5. }),
            card,
        ))
    })
    .when_some(item.artwork.as_deref().filter(|_| !glass), |art, url| {
        art.child(artwork::remote_rounded(
            url,
            circular,
            px(if card { 6. } else { 5. }),
            card,
        ))
    })
}
