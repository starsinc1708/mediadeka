use gpui::{ClipboardItem, Context, Div, IntoElement, Role, Stateful, div, prelude::*, px, rgb};
use guise::{Icon, IconName, Size, Variant};

use crate::{
    music::{
        ConfirmedLink, ObjectKind, ObjectRef, is_soundcloud_set, links::public_link, search_url,
    },
    providers::ProviderId,
    ui::{
        MainView,
        assets::{mark, provider_mark},
        components::{Activate, button, glass_control::Tone},
        fluid_glass::tokens::text,
        playlist::popup::MenuKind,
        theme::{ACTIVE, ELEVATED, SURFACE, navigation_accent, secondary, surface_border},
    },
};

use super::{detail::Detail, detail_style::Ink, heading};

impl MainView {
    pub(super) fn music_link_actions(
        &self,
        detail: &Detail,
        title: &str,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let query = detail
            .data
            .as_ref()
            .filter(|data| !data.page.artists.is_empty())
            .map_or_else(
                || title.to_owned(),
                |data| format!("{} {}", title, data.page.artists.join(" ")),
            );
        let fallback = ProviderId::ALL
            .into_iter()
            .filter_map(|provider| {
                if detail
                    .links
                    .iter()
                    .any(|link| link.platform == provider.as_str())
                {
                    return None;
                }
                Some((provider, search_url(provider, &query)?.to_string()))
            })
            .collect::<Vec<_>>();
        let can_search = detail.data.is_some()
            && detail.target.object.kind != ObjectKind::Collection
            && !fallback.is_empty();
        let controls = self.glass_controls(cx);
        let glass = controls.is_some();
        let ink = Ink::new(glass, self.fluid_high_contrast(cx));
        let needs_sign_in = detail
            .error
            .as_ref()
            .filter(|error| glass && error.needs_sign_in);
        if detail.links.is_empty() && !can_search && needs_sign_in.is_none() {
            return None;
        }
        let provider = detail.target.object.provider;
        let recovery_anchor = detail
            .links
            .iter()
            .position(|link| link.platform == provider.as_str());
        let mut recovery = needs_sign_in.map(|error| {
            self.provider_recovery_action("music-object-connect", provider, 43, cx)
                .size(Size::Xs)
                .variant(Variant::Subtle)
                .color(ink.secondary)
                .accessible_description(error.message.clone())
        });
        let kind = detail.target.object.kind;
        let copied = detail.copied_platform.as_deref();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(if glass { 5. } else { 8. }))
                        .when(!detail.links.is_empty(), |row| {
                            row.child(
                                heading(
                                    "music-confirmed-links",
                                    if glass {
                                        "Доступно на площадках"
                                    } else {
                                        "Площадки"
                                    },
                                )
                                .text_size(px(if glass { text::CAPTION } else { 14. }))
                                .when(glass, |label| label.text_color(ink.secondary))
                                .aria_label("Подтверждённые ссылки на площадки")
                                .pr(px(4.)),
                            )
                        })
                        .children(detail.links.iter().enumerate().map(|(index, link)| {
                            if glass {
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(px(3.))
                                    .child(self.music_link_label(index, link, kind, cx))
                                    .when(recovery_anchor == Some(index), |badge| {
                                        badge.children(recovery.take())
                                    })
                                    .into_any_element()
                            } else {
                                self.legacy_link_chip(
                                    index,
                                    link,
                                    kind,
                                    copied == Some(&link.platform),
                                    cx,
                                )
                                .into_any_element()
                            }
                        }))
                        .children(recovery)
                        .when(glass && !detail.links.is_empty(), |row| {
                            let label = copied.map_or_else(
                                || "Действия со ссылками на площадки".to_owned(),
                                |platform| {
                                    format!(
                                        "Действия со ссылками · ссылка на {} скопирована",
                                        platform_name(platform)
                                    )
                                },
                            );
                            row.child(self.playlist_menu_trigger(
                                "music-platform-menu",
                                label,
                                if copied.is_some() {
                                    IconName::Check
                                } else {
                                    IconName::Ellipsis
                                },
                                73,
                                MenuKind::ObjectLinks { can_search },
                                cx,
                            ))
                        })
                        .when(can_search && (!glass || detail.links.is_empty()), |row| {
                            row.child(self.music_search_toggle(detail.search_open, cx))
                        }),
                )
                .when(can_search && detail.search_open, |group| {
                    group.child(
                        div().flex().flex_wrap().gap(px(7.)).children(
                            fallback
                                .into_iter()
                                .enumerate()
                                .map(|(index, (provider, url))| {
                                    button(
                                        ("music-platform-search", index),
                                        format!("Найти на площадке {}", provider.display_name()),
                                        75 + index as isize,
                                    )
                                    .h(px(35.))
                                    .gap(px(7.))
                                    .px(px(9.))
                                    .rounded(px(8.))
                                    .bg(rgb(SURFACE))
                                    .border_color(surface_border())
                                    .when(controls.is_none(), |link| {
                                        link.hover(|style| style.bg(rgb(ELEVATED)))
                                    })
                                    .when_some(controls, |link, material| {
                                        material.paint(link, Tone::Outlined)
                                    })
                                    .child(provider_mark(provider, 16.))
                                    .child(provider.display_name())
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .pixel_size(px(13.))
                                            .color(secondary()),
                                    )
                                    .on_activate(move |_, _, cx| cx.open_url(&url))
                                }),
                        ),
                    )
                }),
        )
    }

    fn music_search_toggle(&self, expanded: bool, cx: &Context<Self>) -> impl IntoElement {
        let controls = self.glass_controls(cx);
        button("music-search-platforms", "Найти на площадке", 74)
            .h(px(35.))
            .px(px(8.))
            .gap(px(7.))
            .rounded(px(8.))
            .text_color(secondary())
            .text_size(px(13.))
            .when(controls.is_none(), |link| {
                link.hover(|style| style.bg(rgb(ELEVATED)))
            })
            .when_some(controls, |link, material| material.paint(link, Tone::Quiet))
            .child(
                Icon::new(IconName::Search)
                    .pixel_size(px(15.))
                    .color(secondary()),
            )
            .child("Найти на площадке")
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .pixel_size(px(13.))
                .color(secondary()),
            )
            .on_toggle(
                expanded,
                cx.listener(|view, _, _, cx| {
                    view.toggle_music_platform_search(cx);
                }),
            )
    }

    fn legacy_link_chip(
        &self,
        index: usize,
        link: &ConfirmedLink,
        kind: ObjectKind,
        copied: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let name = platform_name(&link.platform);
        let copy = link.url.clone();
        let platform = link.platform.clone();
        let order = 44 + index as isize * 2;
        let ink = Ink::new(false, false);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(1.))
            .rounded(px(10.))
            .border_1()
            .border_color(surface_border())
            .bg(rgb(SURFACE))
            .child(self.music_link_label(index, link, kind, cx))
            .child(div().w(px(1.)).h(px(19.)).bg(ink.rule))
            .child(
                button(
                    ("music-copy-link", index),
                    if copied {
                        format!("Ссылка на {name} скопирована")
                    } else {
                        format!("Копировать ссылку на {name}")
                    },
                    order + 1,
                )
                .role(Role::Button)
                .size(px(36.))
                .justify_center()
                .rounded(px(8.))
                .hover(|style| style.bg(rgb(ELEVATED)))
                .active(|style| style.bg(rgb(ACTIVE)))
                .child(if copied {
                    Icon::new(IconName::Check)
                        .pixel_size(px(15.))
                        .color(navigation_accent())
                        .into_any_element()
                } else {
                    Icon::new(IconName::Copy)
                        .pixel_size(px(15.))
                        .color(secondary())
                        .into_any_element()
                })
                .on_activate(cx.listener(move |view, _, _, cx| {
                    view.copy_music_link(platform.clone(), copy.clone(), cx);
                })),
            )
    }

    fn music_link_label(
        &self,
        index: usize,
        link: &ConfirmedLink,
        kind: ObjectKind,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let name = platform_name(&link.platform);
        let open = link.url.clone();
        let native = ObjectRef::from_url(&open).and_then(|mut object| {
            if kind == ObjectKind::Album && is_soundcloud_set(&object) {
                object.kind = ObjectKind::Album;
            }
            (object.kind == kind && object.is_valid()).then_some(object)
        });
        let glass = self.glass_controls(cx);
        let ink = Ink::new(glass.is_some(), self.fluid_high_contrast(cx));
        button(
            ("music-confirmed-link", index),
            if native.is_some() {
                format!("Открыть в Mediadeka · {name}")
            } else {
                format!("Открыть на площадке · {name}")
            },
            44 + index as isize * 2,
        )
        .role(Role::Link)
        .h(px(if glass.is_some() { 32. } else { 36. }))
        .gap(px(if glass.is_some() { 6. } else { 7. }))
        .px(px(if glass.is_some() { 6. } else { 9. }))
        .rounded(px(if glass.is_some() { 6. } else { 8. }))
        .text_size(px(13.))
        .aria_description(link.evidence.clone())
        .when(glass.is_none(), |link| {
            link.hover(|style| style.bg(rgb(ELEVATED)))
                .active(|style| style.bg(rgb(ACTIVE)))
        })
        .when_some(glass, |link, material| {
            material.paint(link, Tone::Quiet).text_color(ink.primary)
        })
        .child(platform_icon(&link.platform))
        .child(name.to_owned())
        .on_activate(cx.listener(move |view, _, window, cx| {
            if let Some(object) = &native {
                view.open_search_address(object.clone(), open.clone(), window, cx);
            } else if let Some(link) = public_link(&open) {
                cx.open_url(&link.url);
            }
        }))
    }

    pub(in crate::ui) fn music_link_copy_entries(
        &self,
    ) -> impl Iterator<Item = (String, &ConfirmedLink)> {
        self.music.object.iter().flat_map(|detail| {
            detail.links.iter().map(|link| {
                let name = platform_name(&link.platform);
                (format!("Копировать ссылку · {name}"), link)
            })
        })
    }

    pub(in crate::ui) fn music_platform_search_label(&self) -> &'static str {
        if self
            .music
            .object
            .as_ref()
            .is_some_and(|detail| detail.search_open)
        {
            "Скрыть поиск по площадкам"
        } else {
            "Найти на другой площадке"
        }
    }

    pub(in crate::ui) fn toggle_music_platform_search(&mut self, cx: &mut Context<Self>) {
        if let Some(detail) = self.music.object.as_mut() {
            detail.search_open = !detail.search_open;
            cx.notify();
        }
    }

    pub(in crate::ui) fn copy_music_link(
        &mut self,
        platform: String,
        url: String,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = &mut self.music.object else {
            return;
        };
        if detail
            .links
            .iter()
            .any(|link| link.platform == platform && link.url == url)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(url));
            detail.copied_platform = Some(platform);
            cx.notify();
        }
    }
}

fn platform_name(platform: &str) -> &str {
    native_provider(platform)
        .map(ProviderId::display_name)
        .unwrap_or(match platform {
            "apple-music" => "Apple Music",
            "deezer" => "Deezer",
            "tidal" => "TIDAL",
            "qobuz" => "Qobuz",
            _ => platform,
        })
}

fn native_provider(platform: &str) -> Option<ProviderId> {
    ProviderId::ALL
        .into_iter()
        .find(|provider| provider.as_str() == platform)
}

fn platform_icon(platform: &str) -> gpui::AnyElement {
    if let Some(provider) = native_provider(platform) {
        return provider_mark(provider, 17.).into_any_element();
    }
    match platform {
        "apple-music" => mark("applemusic.svg", px(17.)).into_any_element(),
        "deezer" => mark("deezer.svg", px(17.)).into_any_element(),
        "tidal" => mark("tidal.svg", px(17.)).into_any_element(),
        _ => Icon::new(IconName::Link)
            .pixel_size(px(16.))
            .color(navigation_accent())
            .into_any_element(),
    }
}
