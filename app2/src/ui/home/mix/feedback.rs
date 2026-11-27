use gpui::{
    AnyElement, App, Context, Div, IntoElement, Role, Stateful, Task, Window, div, prelude::*, px,
};
use guise::{Button, Size, Variant};

use crate::{
    music::{
        likes::LikeIdentity,
        recommendations::{Reaction, set_reaction},
    },
    playback::QueueEntry,
    storage::Store,
    ui::{
        MainView, components::Activate, design_system::icons, fluid_glass::tokens::text,
        theme::secondary,
    },
};

#[derive(Default)]
pub(super) struct FeedbackUi {
    task: Option<Task<()>>,
    notice: Option<(LikeIdentity, String)>,
}

fn queue_action(
    id: &'static str,
    label: &'static str,
    order: isize,
    enabled: bool,
    fluid: bool,
    cx: &App,
    activate: impl Fn(&(), &mut Window, &mut App) + 'static,
) -> AnyElement {
    if fluid {
        return icons::text_action(id, label, None, false, text::CAPTION, order, enabled, cx)
            .when(!enabled, |button| {
                button.a11y_synthetic_children(|tree| tree.parent_node().set_disabled())
            })
            .when(enabled, |button| button.on_activate(activate))
            .into_any_element();
    }
    Button::new(id, label)
        .size(Size::Xs)
        .variant(Variant::Default)
        .tab_order(order)
        .disabled(!enabled)
        .on_click(move |_, window, cx| activate(&(), window, cx))
        .into_any_element()
}

impl FeedbackUi {
    pub(super) fn is_saving(&self) -> bool {
        self.task.is_some()
    }
}

fn feedback_status(id: &'static str, message: &str) -> Stateful<Div> {
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Status)
        .aria_label(message.to_owned())
        .text_size(px(11.))
        .text_color(secondary())
        .child(message.to_owned())
}

impl MainView {
    pub(in crate::ui) fn recommendation_reaction_pending(&self) -> bool {
        self.home.mix.feedback.is_saving()
    }

    pub(in crate::ui) fn queue_recommendation_controls(&self, cx: &Context<Self>) -> Option<Div> {
        let running = self.recommendation_wave_running();
        let radio = self.contextual_radio_running();
        let radio_notice = self.home.mix.radio_notice.as_deref();
        let recommended = self
            .player
            .read(cx)
            .current()
            .filter(|entry| !entry.recommendations.is_empty());
        if !running && !radio && radio_notice.is_none() && recommended.is_none() {
            return None;
        }
        let mix = &self.home.mix;
        let busy = mix.feedback.is_saving() || mix.starting.is_some();
        let fluid = self.uses_fluid_glass();
        let actions = recommended.into_iter().flat_map(|entry| {
            [
                (
                    "queue-recommendation-more",
                    "Больше такого",
                    Reaction::MoreLikeThis,
                ),
                ("queue-recommendation-hide", "Скрыть", Reaction::Hide),
            ]
            .into_iter()
            .enumerate()
            .map(move |(index, (id, label, reaction))| {
                let target = entry.clone();
                queue_action(
                    id,
                    label,
                    610 + index as isize,
                    !busy,
                    fluid,
                    cx,
                    cx.listener(move |view, _, _, cx| {
                        view.save_queue_reaction(target.clone(), reaction, cx)
                    }),
                )
            })
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .pb(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(actions)
                        .when(running, |row| {
                            row.child(queue_action(
                                "queue-wave-stop",
                                "Остановить волну",
                                612,
                                true,
                                fluid,
                                cx,
                                cx.listener(|view, _, window, cx| {
                                    view.stop_recommendation_wave(cx);
                                    window.focus(&view.focus, cx);
                                }),
                            ))
                        })
                        .when(radio, |row| {
                            row.child(queue_action(
                                "queue-radio-stop",
                                "Остановить радио",
                                613,
                                true,
                                fluid,
                                cx,
                                cx.listener(|view, _, window, cx| {
                                    view.stop_contextual_radio(cx);
                                    window.focus(&view.focus, cx);
                                }),
                            ))
                        }),
                )
                .children(radio_notice.map(|notice| feedback_status("queue-radio-status", notice)))
                .children(
                    mix.feedback
                        .notice
                        .as_ref()
                        .filter(|(identity, _)| {
                            recommended
                                .and_then(|entry| {
                                    LikeIdentity::from_track(&entry.playlist, &entry.track)
                                })
                                .as_ref()
                                == Some(identity)
                        })
                        .map(|(_, message)| {
                            feedback_status("queue-recommendation-status", message)
                        }),
                ),
        )
    }

    fn save_queue_reaction(
        &mut self,
        target: QueueEntry,
        reaction: Reaction,
        cx: &mut Context<Self>,
    ) {
        let mix = &mut self.home.mix;
        if mix.feedback.is_saving() || mix.starting.is_some() {
            return;
        }
        let Some(identity) = LikeIdentity::from_track(&target.playlist, &target.track) else {
            return;
        };
        mix.wave.cancel_refill();
        mix.feedback.notice = None;
        mix.feedback.task = Some(cx.spawn(async move |view, cx| {
            let stored = cx
                .background_executor()
                .spawn(async move { set_reaction(&Store::open_default()?, &target, reaction) })
                .await;
            let _ = view.update(cx, |view, cx| {
                let feedback = &mut view.home.mix.feedback;
                feedback.task = None;
                feedback.notice = Some((
                    identity,
                    match stored {
                        Ok(()) => match reaction {
                            Reaction::MoreLikeThis => "Сохранено · Больше такого",
                            Reaction::Hide => "Сохранено · Скрыть",
                        },
                        Err(error) => {
                            eprintln!("Не удалось сохранить реакцию Mediadeka: {error}");
                            "Не удалось сохранить реакцию."
                        }
                    }
                    .into(),
                ));
                view.recommendation_player_changed(cx);
                view.autoplay_player_changed(cx);
                cx.notify();
            });
        }));
        self.autoplay_player_changed(cx);
        cx.notify();
    }
}
