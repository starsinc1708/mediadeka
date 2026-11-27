use gpui::{
    Context, Div, FocusHandle, FontWeight, MouseButton, Role, Stateful, Window, accesskit::Toggled,
    div, hsla, prelude::*, px, rgb,
};
use guise::{Button, Icon, IconName, Variant};

use crate::{
    providers::ProviderId,
    ui::{
        MainView,
        assets::provider_mark,
        components::{Activate, button, heading, music::time_label, overlay},
        design_system::controls::{ControlState, ControlVariant},
        theme::{ACCENT, ACTIVE, ELEVATED, SURFACE, secondary, surface_border},
    },
};

use super::{Arc, Destination, Mutation, Provider, State};

impl MainView {
    pub(in crate::ui) fn like_menu_focus(&self) -> Option<&FocusHandle> {
        self.likes.dialog.as_ref().filter(|dialog| dialog.visible)?;
        self.likes.focus.as_ref()
    }

    pub(super) fn focus_likes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.likes.focus.get_or_insert_with(|| cx.focus_handle());
        window.focus(focus, cx);
    }

    pub(in crate::ui) fn close_like_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .likes
            .dialog
            .as_ref()
            .is_none_or(|dialog| !dialog.visible)
        {
            return false;
        }
        if let Some(dialog) = self.likes.dialog.take()
            && let Some(restore) = dialog.return_focus
        {
            window.focus(&restore, cx);
        }
        self.likes.held = None;
        cx.notify();
        true
    }

    pub(in crate::ui) fn like_overlay(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Stateful<Div>> {
        let dialog = self.likes.dialog.as_ref().filter(|dialog| dialog.visible)?;
        let max_height = (f32::from(window.viewport_size().height) - 58.).max(285.);
        let material = self.glass_controls(cx);
        let overlay = div()
            .id("like-destinations-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.56))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    view.close_like_menu(window, cx);
                    cx.stop_propagation();
                }),
            );
        let panel = div()
            .id("like-destinations-menu")
            .debug_selector(|| "like-destinations-menu".into())
            .accessibility_id("like-destinations-menu")
            .role(Role::Menu)
            .aria_label("Куда сохранить лайк")
            .when_some(self.likes.focus.as_ref(), |panel, focus| {
                panel.track_focus(focus)
            })
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(440.))
            .max_h(px(max_height))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(9.))
            .rounded(px(13.))
            .border_1()
            .border_color(surface_border())
            .bg(rgb(ELEVATED))
            .when_some(material, overlay::menu_surface)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(heading("Куда сохранить лайк", 21.))
                    .child(
                        Button::new("like-menu-close", "Закрыть")
                            .variant(Variant::Subtle)
                            .tab_order(751)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.close_like_menu(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(secondary())
                    .child(format!(
                        "{} · {}",
                        dialog.subject.track.title, dialog.subject.track.artist
                    )),
            )
            .child(
                div()
                    .id("like-menu-scroll")
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .min_h_0()
                    .overflow_y_scroll()
                    .when_some(dialog.error.as_ref(), |body, error| {
                        body.child(notice("like-menu-error", error.clone()))
                    })
                    .when(dialog.choices.is_none() && dialog.error.is_none(), |body| {
                        body.child(notice("like-loading", "Читаем сохранённые отметки…".into()))
                    })
                    .children(
                        dialog
                            .choices
                            .iter()
                            .flatten()
                            .enumerate()
                            .map(|(index, row)| self.like_destination_row(index, row, cx)),
                    ),
            );
        Some(overlay.child(panel))
    }

    fn like_destination_row(&self, index: usize, row: &Destination, cx: &Context<Self>) -> Div {
        let Some(dialog) = self.likes.dialog.as_ref() else {
            return div();
        };
        let provider = row.provider;
        let pending =
            self.likes
                .pending_mark(&dialog.subject.identity, provider, row.profile.as_deref());
        let searching = dialog
            .search
            .as_ref()
            .is_some_and(|(active, _)| Some(*active) == provider);
        let (caption, liked) = match (pending, &row.state) {
            (Some(true), _) => ("Сохраняем…", Some(true)),
            (Some(false), _) => ("Убираем…", Some(false)),
            _ if searching => ("Ищем запись…", None),
            (
                _,
                State::Local { liked }
                | State::Ready {
                    liked: Some(liked), ..
                },
            ) => (
                if *liked {
                    "Сохранено"
                } else {
                    "Сохранить"
                },
                Some(*liked),
            ),
            (_, State::Ready { liked: None, .. }) => ("Сохранить", None),
            (_, State::Unmatched) => ("Найти запись", None),
            (_, State::Error(_)) => ("Недоступно", None),
        };
        let name = display_name(provider);
        let id = format!("like-destination-{}", slug(provider));
        let data = row.clone();
        let subject = dialog.subject.clone();
        let material = self.glass_controls(cx);
        let enabled = pending.is_none() && !searching && !matches!(row.state, State::Error(_));
        let row_button = button(id, format!("{name} · {caption}"), 760 + index as isize)
            .role(Role::MenuItem)
            .when_some(liked, |button, liked| {
                button.aria_toggled(Toggled::from(liked))
            })
            .when(provider.is_none(), |button| {
                button.debug_selector(|| "like-destination-mediadeka".into())
            })
            .when(
                pending.is_none() && matches!(row.state, State::Ready { liked: None, .. }),
                |button| button.aria_description("Текущая отметка пока неизвестна"),
            )
            .px(px(9.))
            .gap(px(8.))
            .min_h(px(35.))
            .justify_between()
            .w_full()
            .rounded(px(7.))
            .when(material.is_none(), |row| {
                row.bg(rgb(SURFACE)).hover(|row| row.bg(rgb(ACTIVE)))
            })
            .when_some(material, |row, material| {
                ControlVariant::MenuItem.paint(
                    row,
                    material,
                    ControlState {
                        disabled: !enabled,
                        ..ControlState::default()
                    },
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(destination_icon(provider))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(13.))
                    .child(div().text_color(secondary()).child(caption))
                    .children(liked.filter(|liked| *liked).map(|_| {
                        Icon::new(IconName::Check)
                            .pixel_size(px(15.))
                            .color(gpui::Hsla::from(rgb(ACCENT)))
                    })),
            )
            .when(enabled, |element| {
                element.on_activate(cx.listener(move |view, _, window, cx| match &data.state {
                    State::Local { liked } => view.save_like(
                        subject.clone(),
                        data.clone(),
                        Mutation {
                            recording: None,
                            liked: !liked,
                            explicit_match: false,
                        },
                        window,
                        cx,
                    ),
                    State::Ready { recording, liked } => view.save_like(
                        subject.clone(),
                        data.clone(),
                        Mutation {
                            recording: Some(recording.clone()),
                            liked: *liked != Some(true),
                            explicit_match: false,
                        },
                        window,
                        cx,
                    ),
                    State::Unmatched => view.search_like_tracks(data.clone(), cx),
                    State::Error(_) => {}
                }))
            });
        let error = dialog.named_errors.get(&row.provider).or(match &row.state {
            State::Error(error) => Some(error),
            _ => None,
        });
        div()
            .w_full()
            .child(row_button)
            .when_some(error, |entry, error| {
                entry.child(notice(
                    format!("like-error-{}", slug(provider)),
                    error.clone(),
                ))
            })
            .when_some(
                dialog
                    .candidates
                    .as_ref()
                    .filter(|(search, _)| search.provider == provider),
                |entry, (search, candidates)| {
                    entry.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .pl(px(12.))
                            .when(candidates.is_empty(), |group| {
                                group.child(notice(
                                    "like-empty-candidates",
                                    "Совпадения не найдены.".into(),
                                ))
                            })
                            .children(candidates.iter().enumerate().map(|(index, candidate)| {
                                let info = [
                                    &candidate.track.artist,
                                    candidate.track.album.as_deref().unwrap_or(""),
                                ]
                                .into_iter()
                                .filter(|text| !text.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · ");
                                let info = match candidate.track.duration {
                                    Some(duration) => {
                                        format!("{info} · {}", time_label(f64::from(duration)))
                                    }
                                    None => info,
                                };
                                let title = candidate.track.title.clone();
                                let recording = candidate.clone();
                                let row = search.clone();
                                let subject = dialog.subject.clone();
                                button(
                                    ("like-candidate", index),
                                    format!("{title} · {info}"),
                                    790 + index as isize,
                                )
                                .role(Role::MenuItem)
                                .w_full()
                                .px(px(7.))
                                .py(px(5.))
                                .flex_col()
                                .items_start()
                                .gap(px(1.))
                                .rounded(px(6.))
                                .when(material.is_none(), |item| {
                                    item.hover(|state| state.bg(rgb(ACTIVE)))
                                })
                                .when_some(material, |item, material| {
                                    ControlVariant::MenuItem.paint(
                                        item,
                                        material,
                                        ControlState::default(),
                                    )
                                })
                                .child(div().w_full().truncate().text_size(px(13.)).child(title))
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_color(secondary())
                                        .text_size(px(11.))
                                        .child(info),
                                )
                                .on_activate(cx.listener(
                                    move |view, _, window, cx| {
                                        if let Some(dialog) = view.likes.dialog.as_mut() {
                                            dialog.candidates = None;
                                        }
                                        view.save_like(
                                            subject.clone(),
                                            row.clone(),
                                            Mutation {
                                                recording: Some(Arc::new(recording.clone())),
                                                liked: true,
                                                explicit_match: true,
                                            },
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                            })),
                    )
                },
            )
    }
}

pub(super) fn display_name(provider: Provider) -> &'static str {
    provider.map_or("Mediadeka", ProviderId::display_name)
}

pub(super) fn slug(provider: Provider) -> &'static str {
    provider.map_or("mediadeka", ProviderId::as_str)
}

pub(super) fn destination_icon(provider: Provider) -> gpui::AnyElement {
    match provider {
        Some(provider) => provider_mark(provider, 17.).into_any_element(),
        None => Icon::new(IconName::Library)
            .pixel_size(px(16.))
            .color(gpui::Hsla::from(rgb(ACCENT)))
            .into_any_element(),
    }
}

pub(super) fn notice(id: impl Into<gpui::ElementId>, message: String) -> Stateful<Div> {
    let id = id.into();
    let automation_id = id.to_string();
    div()
        .id(id)
        .accessibility_id(automation_id)
        .role(Role::Status)
        .aria_label(message.clone())
        .px(px(9.))
        .py(px(4.))
        .w_full()
        .text_size(px(12.))
        .text_color(secondary())
        .child(message)
}
