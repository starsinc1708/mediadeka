use gpui::{
    Context, Div, FontWeight, Hsla, Role, Window, canvas, div, fill, linear_color_stop,
    linear_gradient, prelude::*, px, rgb,
};
use guise::IconName;

use super::{
    ACCENT, BASE, CONTROL, LINE, MUTED, SELECTED, TEXT, button, button_base, glyph, sleeve,
    surface, title,
};
use crate::music_concept::{Browse, Concept, TrackId, remaining_track_scroll};

impl Concept {
    pub(super) fn release(
        &self,
        selection: TrackId,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let release = &self.releases[selection.release];
        let artwork = if compact { 142. } else { 452. };
        let count = release.info.tracks.len();
        let mut metadata = format!("{}  ·  {count} {}", release.info.artist, track_word(count),);
        if let Some(year) = &release.info.year {
            metadata.push_str(&format!("  ·  {year}"));
        }
        surface()
            .flex()
            .flex_col()
            .px(px(if compact { 22. } else { 65. }))
            .pt(px(if compact { 10. } else { 68. }))
            .pb(px(if compact { 6. } else { 68. }))
            .gap(px(if compact { 9. } else { 45. }))
            .child(
                button("concept-back", "Назад", 6)
                    .self_start()
                    .flex_none()
                    .h(px(if compact { 24. } else { 32. }))
                    .gap(px(7.))
                    .text_color(rgb(MUTED))
                    .child(glyph(IconName::ChevronLeft, MUTED))
                    .child(if self.release_from_search {
                        "К поиску"
                    } else {
                        "Медиатека"
                    })
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.back();
                        if view.browse == Browse::Search {
                            view.query.update(cx, |field, cx| field.focus(window, cx));
                        } else {
                            window.focus(&view.focus, cx);
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_start()
                    .gap(px(if compact { 24. } else { 86. }))
                    .child(sleeve(release.image.clone(), artwork))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .min_h_0()
                            .max_w(px(714.))
                            .flex()
                            .flex_col()
                            .gap(px(if compact { 6. } else { 11. }))
                            .child(
                                title(
                                    "concept-release-title",
                                    &release.info.title,
                                    if compact { 28. } else { 62. },
                                )
                                .flex_none()
                                .truncate(),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(if compact { 12. } else { 15. }))
                                    .text_color(rgb(MUTED))
                                    .child(metadata),
                            )
                            .child(self.release_actions(selection, compact, cx))
                            .child(self.release_tracks(selection, compact, window, cx)),
                    ),
            )
    }

    fn release_actions(&self, selection: TrackId, compact: bool, cx: &Context<Self>) -> Div {
        let height = px(if compact { 30. } else { 38. });
        div()
            .flex_none()
            .flex()
            .gap(px(8.))
            .pt(px(if compact { 2. } else { 6. }))
            .child(
                button_base("concept-play", "Слушать выбранный трек", 40)
                    .h(height)
                    .px(px(11.))
                    .gap(px(7.))
                    .bg(rgb(ACCENT))
                    .text_color(rgb(BASE))
                    .hover(|style| style.bg(rgb(TEXT)))
                    .child(glyph(IconName::Play, BASE))
                    .child("Слушать")
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.start_release(selection);
                        cx.notify();
                    })),
            )
            .child(
                button("concept-add-next", "Поставить выбранный трек следующим", 41)
                    .h(height)
                    .px(px(10.))
                    .gap(px(7.))
                    .border_color(rgb(LINE))
                    .bg(Hsla::from(rgb(CONTROL)).alpha(0.62))
                    .child(glyph(IconName::ListPlus, ACCENT))
                    .child("Следующим")
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.add_next(selection);
                        cx.notify();
                    })),
            )
    }

    fn release_tracks(
        &self,
        selection: TrackId,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let scroll_for_fade = self.track_scroll.clone();
        let release = &self.releases[selection.release];
        let rows = release
            .info
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                let selected = selection.index == index;
                let id = TrackId { index, ..selection };
                let playing = self.current == id;
                button(
                    format!("concept-row-{index}"),
                    track.title.clone(),
                    100 + index as isize,
                )
                .track_focus(&self.row_focus[id.release][id.index])
                .role(Role::ListItem)
                .aria_selected(selected)
                .when(playing, |row| {
                    row.aria_description(if self.paused {
                        "Текущий трек · пауза"
                    } else {
                        "Сейчас играет"
                    })
                })
                .h(px(if compact { 33. } else { 48. }))
                .w_full()
                .flex_none()
                .px(px(8.))
                .gap(px(if compact { 8. } else { 14. }))
                .rounded(px(4.))
                .relative()
                .when(selected, |row| {
                    row.bg(Hsla::from(rgb(SELECTED)).alpha(0.90))
                })
                .child(
                    div()
                        .w(px(23.))
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .when(playing, |number| {
                            number.child(glyph(IconName::AudioLines, ACCENT))
                        })
                        .when(!playing, |number| number.child((index + 1).to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(playing, |label| label.font_weight(FontWeight::SEMIBOLD))
                        .child(track.title.clone()),
                )
                .when(!compact, |row| {
                    row.child(
                        div()
                            .w(px(132.))
                            .flex_none()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(track.artist.clone()),
                    )
                })
                .child(
                    div()
                        .w(px(40.))
                        .flex_none()
                        .text_right()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(track.duration.clone()),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(rgb(LINE)),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.browse = Browse::Release(id);
                    cx.notify();
                }))
            });
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .mt(px(if compact { 1. } else { 7. }))
            .border_t_1()
            .border_color(rgb(LINE))
            .overflow_hidden()
            .child(
                div()
                    .id("concept-track-list")
                    .accessibility_id("concept-track-list")
                    .role(Role::List)
                    .aria_label("Треки релиза")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.track_scroll)
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .when(self.focused_row(window, cx).is_none(), |list| {
                list.child(
                    canvas(
                        move |_, _, _| remaining_track_scroll(&scroll_for_fade),
                        |bounds, remaining, window, _| {
                            if remaining <= px(1.) {
                                return;
                            }
                            let strength = (remaining / px(35.)).clamp(0., 1.);
                            let ink = Hsla::from(rgb(BASE));
                            window.paint_quad(fill(
                                bounds,
                                linear_gradient(
                                    180.,
                                    linear_color_stop(ink.alpha(0.), 0.),
                                    linear_color_stop(ink.alpha(0.94 * strength), 1.),
                                ),
                            ));
                        },
                    )
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(if compact { 43. } else { 112. })),
                )
            })
    }
}

fn track_word(count: usize) -> &'static str {
    match (count % 100, count % 10) {
        (11..=14, _) => "треков",
        (_, 1) => "трек",
        (_, 2..=4) => "трека",
        _ => "треков",
    }
}
