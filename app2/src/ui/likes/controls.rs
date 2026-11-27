use std::{rc::Rc, time::Duration};

use gpui::{App, Context, Div, ElementId, KeyDownEvent, MouseButton, Window, div, prelude::*};
use guise::IconName;

use crate::{
    music::likes::{LikeIdentity, Subject},
    playlist::{PlaylistKey, PlaylistTrack},
    ui::{
        MainView,
        components::music::compact_control,
        design_system::{
            controls::{self, ControlState, ControlVariant},
            icons::{self, Glyph},
        },
        theme::playlist,
        workspace::Page,
    },
};

use super::{Gesture, LikeAction, Provider, effective_defaults};

const PRESS_DURATION: Duration = Duration::from_millis(450);

impl MainView {
    pub(in crate::ui) fn track_like_action(
        &self,
        key: &PlaylistKey,
        track: &PlaylistTrack,
        known_liked: &[Provider],
    ) -> Option<LikeAction> {
        let identity = LikeIdentity::from_track(key, track)?;
        let marks = self.likes.cached_marks.get(&identity);
        let selected = effective_defaults(&self.likes.defaults, |id| {
            id.is_none_or(|provider| self.likes.accounts.contains_key(&provider))
        });
        let action = LikeAction::for_destinations(selected, |destination| {
            self.likes
                .pending_mark(&identity, destination, self.likes.profile(destination))
                .or_else(|| marks.and_then(|marks| marks.get(&destination).copied()))
                .or_else(|| known_liked.contains(&destination).then_some(true))
                .or_else(|| {
                    self.playlist
                        .favorites
                        .destination_mark(&identity, destination)
                })
        });
        Some(if self.likes.has_pending(&identity) {
            LikeAction::Pending {
                marked: action == LikeAction::Remove,
            }
        } else {
            action
        })
    }

    pub(in crate::ui) fn like_control(
        &self,
        id: impl Into<ElementId>,
        order: isize,
        key: &PlaylistKey,
        track: &PlaylistTrack,
        cx: &Context<Self>,
    ) -> Div {
        self.like_control_with_mark(id, order, key, track, &[], cx)
    }

    pub(in crate::ui) fn like_control_with_mark(
        &self,
        id: impl Into<ElementId>,
        order: isize,
        key: &PlaylistKey,
        track: &PlaylistTrack,
        known_liked: &[Provider],
        cx: &Context<Self>,
    ) -> Div {
        let subject = Subject::new(key, track);
        let liked = self.track_like_action(key, track, known_liked);
        like_button(
            id,
            order,
            "",
            liked,
            self.music_page() == Page::Playlist,
            self.uses_fluid_glass().then_some(cx as &App),
            cx.listener(move |view, gesture, window, cx| {
                if let Some(subject) = &subject {
                    view.like_gesture(subject.clone(), *gesture, window, cx);
                }
            }),
        )
    }

    pub(in crate::ui) fn like_gesture(
        &mut self,
        subject: Subject,
        gesture: Gesture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match gesture {
            Gesture::BeginPress => {
                self.likes.held = None;
                self.likes.pressed = Some(cx.spawn_in(window, async move |view, cx| {
                    cx.background_executor().timer(PRESS_DURATION).await;
                    let _ = view.update_in(cx, |view, window, cx| {
                        view.likes.held = Some(subject.identity.clone());
                        view.open_like_destinations(subject, false, window, cx);
                    });
                }));
            }
            Gesture::EndPress => self.likes.pressed = None,
            Gesture::Open => {
                self.likes.pressed = None;
                self.open_like_destinations(subject, false, window, cx);
            }
            Gesture::Activate => {
                self.likes.pressed = None;
                if self.likes.held.take().as_ref() != Some(&subject.identity) {
                    self.open_like_destinations(subject, true, window, cx);
                }
            }
        }
    }
}

pub(in crate::ui) fn like_button(
    id: impl Into<ElementId>,
    order: isize,
    subject_label: &str,
    liked: Option<LikeAction>,
    playlist_presentation: bool,
    fluid: Option<&App>,
    action: impl Fn(&Gesture, &mut Window, &mut App) + 'static,
) -> Div {
    let enabled = liked.is_some();
    let active = matches!(
        liked,
        Some(LikeAction::Remove | LikeAction::Pending { marked: true })
    );
    let pending = matches!(liked, Some(LikeAction::Pending { .. }));
    let id = id.into();
    let label = match liked {
        Some(LikeAction::Remove) => format!("Убрать {subject_label}из избранного"),
        Some(LikeAction::Save) => format!("Добавить {subject_label}в избранное"),
        Some(LikeAction::Check) => "Изменить отметку в избранном".into(),
        Some(LikeAction::Pending { .. }) => "Избранное · ожидает подтверждения".into(),
        None => "Избранное недоступно".into(),
    };
    let action = Rc::new(action);
    let click = action.clone();
    let right = action.clone();
    let hold = action.clone();
    let up = action.clone();
    let outside = action.clone();
    div()
        .flex_none()
        .child(
            compact_control(id.clone(), label, IconName::Heart, order, enabled, active)
                .when(playlist_presentation, |button| {
                    button
                        .pixel_size(gpui::px(playlist::metrics::CONTROL_SMALL))
                        .color(gpui::Hsla::from(gpui::rgb(if active {
                            playlist::SIGNAL
                        } else {
                            playlist::INK_SECONDARY
                        })))
                })
                .when(pending || fluid.is_some(), |button| button.toggled(active))
                .when(fluid.is_none(), |button| {
                    button.glyph_content(icons::settled_like(active, 16.))
                })
                .when_some(fluid, |button, cx| {
                    button
                        .surface_style(ControlVariant::GhostIcon.surface(
                            controls::material(cx),
                            ControlState {
                                active,
                                ..ControlState::default()
                            },
                        ))
                        .glyph_content(icons::animated_glyph(
                            id,
                            Glyph::Like,
                            active,
                            17.,
                            None,
                            cx,
                        ))
                })
                .when(enabled, |button| {
                    button
                        .accessible_description(
                            "Меню площадок открывается правой кнопкой или Shift+F10",
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            if !pending {
                                click(&Gesture::Activate, window, cx);
                            }
                        })
                }),
        )
        .when(enabled, |wrapper| {
            wrapper
                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    cx.stop_propagation();
                    right(&Gesture::Open, window, cx);
                })
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    hold(&Gesture::BeginPress, window, cx)
                })
                .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                    up(&Gesture::EndPress, window, cx)
                })
                .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                    outside(&Gesture::EndPress, window, cx)
                })
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    if (event.keystroke.key == "f10" && event.keystroke.modifiers.shift)
                        || event.keystroke.key == "menu"
                    {
                        cx.stop_propagation();
                        action(&Gesture::Open, window, cx);
                    }
                })
        })
}
