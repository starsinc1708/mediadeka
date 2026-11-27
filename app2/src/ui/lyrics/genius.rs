use gpui::{
    App, Context, Div, FontWeight, IntoElement, Role, Stateful, Task, Window, div, prelude::*, px,
    rgb,
};
use guise::{Icon, IconName};

use crate::{
    music::lyrics::{
        Line, Recording, Source,
        genius::{self, Access, Annotation, Song},
    },
    storage::Store,
    ui::{
        components::{Activate, button, music::compact_control},
        design_system::controls::{self, ControlState, ControlVariant},
        fluid_glass::tokens::{size, space, text},
        theme::{ACCENT, ELEVATED, primary, secondary},
    },
};

use super::{Job, LyricsView};

#[derive(Default)]
pub(super) struct Genius {
    pub(super) song: Option<Song>,
    pub(super) open_note: Option<(usize, u64)>,
    pub(super) rows: Vec<Vec<usize>>,
    job: Option<Job>,
    writing: Option<Task<()>>,
    started: bool,
    failed: bool,
    pending_access: Option<Access>,
}

impl Genius {
    pub(super) fn is_pending(&self) -> bool {
        self.job.is_some() || self.writing.is_some()
    }

    #[cfg(feature = "capture")]
    pub(super) fn failed(&self) -> bool {
        self.failed
    }
}

impl LyricsView {
    pub(super) fn start_genius(&mut self, cx: &mut Context<Self>) {
        if !self.visible || self.genius.started || self.genius.is_pending() {
            return;
        }
        let Some(entry) = self.entry.as_ref() else {
            return;
        };
        let recording = Recording::from_entry(entry);
        self.genius.started = true;
        self.genius.failed = false;
        self.genius.job = Some(Job::run(
            cx,
            move |flag, sender| {
                let store = Store::open_default().map_err(|error| error.to_string())?;
                let song = genius::load(&store, &recording, flag, |cached| {
                    if cached.is_current(&store) {
                        let _ = sender.send_blocking(Some(cached));
                    }
                })?;
                let _ = sender.send_blocking(song.filter(|song| song.is_current(&store)));
                Ok(())
            },
            |view, song, cx| {
                view.genius.song = song;
                view.indices_changed(cx);
            },
            |view, result, _| {
                view.genius.job = None;
                view.genius.failed = result.is_err();
            },
        ));
    }

    fn change_access(&mut self, access: Access, cx: &mut Context<Self>) {
        let Some(song) = self.genius.song.clone() else {
            return;
        };
        if access == Access::Link {
            if let Some(current) = self.genius.song.as_mut() {
                current.access = Access::Link;
                current.revoked = true;
                current.lyrics = None;
                current.annotations.clear();
            }
            self.genius.open_note = None;
            self.indices_changed(cx);
        }
        self.genius.job = None;
        self.genius.pending_access = None;
        self.genius.failed = false;
        let (sender, receiver) = async_channel::bounded(1);
        // An explicit permission change must persist even if the user closes
        // the panel before SQLite completes it. Only delivery to the old view
        // is cancelled when the track changes.
        cx.background_executor()
            .spawn(async move {
                let result = Store::open_default()
                    .map_err(|error| error.to_string())
                    .and_then(|store| genius::set_access(&store, &song, access));
                let _ = sender.send(result).await;
            })
            .detach();
        self.genius.writing = Some(cx.spawn(async move |view, cx| {
            if let Ok(result) = receiver.recv().await {
                let _ = view.update(cx, |view, cx| {
                    view.genius.writing = None;
                    view.genius.failed = result.is_err();
                    if result.is_ok() {
                        view.genius.started = false;
                        view.start_genius(cx);
                    }
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }

    pub(super) fn remap_annotations(&mut self) {
        self.genius.rows = match (self.current_text(), self.genius.song.as_ref()) {
            (Some(text), Some(song)) => annotation_rows(&text.lines, &song.annotations),
            _ => Vec::new(),
        };
        if let Some((row, id)) = self.genius.open_note
            && !self
                .genius
                .rows
                .get(row)
                .into_iter()
                .flatten()
                .any(|&index| {
                    self.genius
                        .song
                        .as_ref()
                        .is_some_and(|song| song.annotations[index].id == id)
                })
        {
            self.genius.open_note = None;
        }
    }

    pub(super) fn notes_for_line(&self, row: usize, cx: &Context<Self>) -> impl IntoElement {
        let Some(song) = &self.genius.song else {
            return div();
        };
        div().flex_none().flex().items_start().children(
            self.genius
                .rows
                .get(row)
                .into_iter()
                .flatten()
                .filter_map(|&index| {
                    let note = song.annotations.get(index)?;
                    let id = note.id;
                    let opens = self.genius.open_note != Some((row, id));
                    let toggle = cx.listener(move |view, _: &(), _, cx| {
                        view.genius.open_note = opens.then_some((row, id));
                        cx.notify();
                    });
                    let element_id = format!("lyrics-note-{row}-{id}");
                    const LABEL: &str = "Пояснение Genius";
                    Some(if self.fluid {
                        ControlVariant::GhostIcon
                            .paint(
                                button(element_id, LABEL, 570)
                                    .size(px(size::XS))
                                    .justify_center()
                                    .tooltip(guise::tooltip(LABEL)),
                                controls::material(cx),
                                ControlState {
                                    active: !opens,
                                    ..Default::default()
                                },
                            )
                            .child(Icon::new(IconName::MessageSquareText).pixel_size(px(space::LG)))
                            .on_toggle(!opens, toggle)
                            .into_any_element()
                    } else {
                        compact_control(
                            element_id,
                            LABEL,
                            IconName::MessageSquareText,
                            570,
                            true,
                            !opens,
                        )
                        .on_click(move |_, window, cx| toggle(&(), window, cx))
                        .into_any_element()
                    })
                }),
        )
    }

    pub(super) fn annotation_body(&self, row: usize) -> Option<Div> {
        let (open_row, id) = self.genius.open_note?;
        if open_row != row {
            return None;
        }
        let note = self
            .genius
            .song
            .as_ref()?
            .annotations
            .iter()
            .find(|note| note.id == id)?;
        Some(
            div().w_full().child(
                div()
                    .id(format!("lyrics-note-body-{row}"))
                    .accessibility_id(format!("lyrics-note-body-{row}"))
                    .role(Role::Group)
                    .aria_label("Пояснение Genius")
                    .text_size(px(13.))
                    .line_height(px(19.))
                    .m(px(3.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(rgb(ELEVATED))
                    .text_color(primary())
                    .child(note.text.clone()),
            ),
        )
    }

    pub(super) fn genius_footer(&self, cx: &Context<Self>) -> Option<Div> {
        let Some(song) = &self.genius.song else {
            return (self.genius.failed && self.selection == Some(Source::Genius)).then(|| {
                div()
                    .text_color(secondary())
                    .child("Genius сейчас недоступен")
            });
        };
        let href = song.url.as_ref().filter(|url| {
            url::Url::parse(url).is_ok_and(|url| {
                url.scheme() == "https"
                    && matches!(url.host_str(), Some("genius.com" | "www.genius.com"))
            })
        });
        let busy = self.genius.is_pending();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(5.))
                .mt(px(12.))
                .p(px(10.))
                .rounded(px(8.))
                .bg(rgb(ELEVATED))
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(secondary())
                .child(
                    div()
                        .text_color(primary())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Сведения Genius"),
                )
                .when(!song.writers.is_empty(), |section| {
                    section.child(format!("Авторы · {}", song.writers.join(", ")))
                })
                .when(!song.producers.is_empty(), |section| {
                    section.child(format!("Продюсеры · {}", song.producers.join(", ")))
                })
                .children(href.map(|url| {
                    let url = url.clone();
                    let label = format!("Открыть «{}» · {} на Genius", song.title, song.artist);
                    self.paint_genius_action(button("lyrics-genius-url", label, 575), cx)
                        .child("Открыть на Genius")
                        .on_activate(move |_, _, cx| cx.open_url(&url))
                }))
                .when(busy, |section| section.child("Обновляем сведения Genius…"))
                .when(self.genius.failed || song.issue.is_some(), |section| {
                    section.child("Не удалось обновить сведения Genius")
                })
                .when(!busy && href.is_some(), |section| {
                    section.child(self.access_controls(song, cx))
                }),
        )
    }

    fn access_controls(&self, song: &Song, cx: &Context<Self>) -> Div {
        if let Some(access) = self.genius.pending_access {
            let description = match access {
                Access::Read => "У меня есть право читать полный текст в Mediadeka.",
                Access::Save => "У меня есть право читать и сохранять полный текст локально.",
                Access::Link => unreachable!("revocation requires no confirmation"),
            };
            return div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(description)
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(self.access_button(
                            "lyrics-genius-confirm",
                            "Подтвердить",
                            cx.listener(move |view, _, _, cx| view.change_access(access, cx)),
                            cx,
                        ))
                        .child(self.access_button(
                            "lyrics-genius-cancel",
                            "Отмена",
                            cx.listener(|view, _, _, cx| {
                                view.genius.pending_access = None;
                                cx.notify();
                            }),
                            cx,
                        )),
                );
        }
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(8.))
            .gap_y(px(2.))
            .when(song.access != Access::Read, |row| {
                row.child(self.access_button(
                    "lyrics-genius-read",
                    "Разрешить чтение",
                    cx.listener(|view, _, _, cx| {
                        view.genius.pending_access = Some(Access::Read);
                        cx.notify();
                    }),
                    cx,
                ))
            })
            .when(song.access != Access::Save, |row| {
                row.child(self.access_button(
                    "lyrics-genius-save",
                    "Разрешить хранение",
                    cx.listener(|view, _, _, cx| {
                        view.genius.pending_access = Some(Access::Save);
                        cx.notify();
                    }),
                    cx,
                ))
            })
            .when(
                !song.revoked && (song.access != Access::Link || !song.annotations.is_empty()),
                |row| {
                    row.child(self.access_button(
                        "lyrics-genius-revoke",
                        "Отозвать для песни",
                        cx.listener(|view, _, _, cx| view.change_access(Access::Link, cx)),
                        cx,
                    ))
                },
            )
    }

    fn access_button(
        &self,
        id: &'static str,
        label: &'static str,
        listener: impl Fn(&(), &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> Stateful<Div> {
        self.paint_genius_action(button(id, label, 576), cx)
            .child(label)
            .on_activate(listener)
    }

    fn paint_genius_action(&self, action: Stateful<Div>, cx: &App) -> Stateful<Div> {
        if self.fluid {
            ControlVariant::TextAction.paint(
                action
                    .flex_none()
                    .self_start()
                    .h(px(size::XS))
                    .px(px(space::SM))
                    .text_size(px(text::CAPTION)),
                controls::material(cx),
                ControlState::default(),
            )
        } else {
            action.text_color(rgb(ACCENT))
        }
    }
}

fn normalize(text: &str) -> String {
    let mut out = String::new();
    for part in text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(part.chars().flat_map(char::to_lowercase));
    }
    out
}

/// The note stays beside the start of its fragment even when the fragment spans
/// several lyric rows. Exact word boundaries distinguish a word from a suffix.
fn annotation_rows(lines: &[Line], annotations: &[Annotation]) -> Vec<Vec<usize>> {
    let mut body = String::new();
    let mut positions = Vec::with_capacity(lines.len());
    for line in lines {
        body.push(' ');
        positions.push(body.len());
        body.push_str(&normalize(&line.text));
    }
    let mut rows = vec![Vec::new(); lines.len()];
    for (index, annotation) in annotations.iter().enumerate() {
        let fragment = normalize(&annotation.fragment);
        if fragment.is_empty() {
            continue;
        }
        for (start, _) in body.match_indices(&fragment) {
            let end = start + fragment.len();
            if body.as_bytes().get(start - 1) != Some(&b' ')
                || body.as_bytes().get(end).is_some_and(|&byte| byte != b' ')
            {
                continue;
            }
            let row = positions.partition_point(|&position| position <= start) - 1;
            if rows[row].last() != Some(&index) {
                rows[row].push(index);
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_can_match_the_start_of_a_multi_line_fragment() {
        let lines =
            ["Сначала свет.", "Горят огни", "🎵 Снова свет", "светлячок"].map(|text| Line {
                text: text.into(),
                time_ms: None,
                words: Vec::new(),
            });
        let notes = ["СВЕТ\nгорят огни", "свет"].map(|fragment| Annotation {
            id: 1,
            fragment: fragment.into(),
            text: "Пояснение".into(),
        });
        assert_eq!(
            annotation_rows(&lines, &notes),
            [vec![0, 1], vec![], vec![1], vec![]]
        );
    }
}
