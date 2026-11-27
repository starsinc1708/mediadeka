//! Calendar headings and the compact layout for the existing virtual history.

use gpui::{AnyElement, Context, Div, FontWeight, Hsla, Role, div, prelude::*, px, rgb};
use guise::{Icon, IconName};

use crate::ui::{
    design_system::controls::{ControlState, ControlVariant},
    fluid_glass::tokens,
    labels::{counted, month_name},
};

use super::{Activate, HEADER_ITEMS, HistoryRow, MainView, Page, button, status};

#[derive(Clone, Copy)]
pub(super) struct RowStyle {
    pub current: bool,
    pub show_source: bool,
}

impl MainView {
    pub(super) fn fluid_history_item(
        &self,
        index: usize,
        margin: f32,
        compact: bool,
        width: f32,
        current: Option<i64>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let high_contrast = self.fluid_high_contrast(cx);
        let secondary = ink_secondary(high_contrast);
        let rule = if high_contrast {
            rgb(tokens::contrast::RULE).into()
        } else {
            Hsla::from(rgb(tokens::RULE)).alpha(0.48)
        };
        let entry = if index == 0 {
            self.fluid_history_header(compact, secondary, cx)
        } else {
            let row_index = index - HEADER_ITEMS;
            let row = &self.history.rows[row_index];
            let date = date_key(row);
            let starts_day = row_index == 0 || date_key(&self.history.rows[row_index - 1]) != date;
            let date_heading = starts_day.then(|| {
                let (year, month, day) = date;
                let label = format!("{day} {} {year}", month_name(month).unwrap_or_default());
                div()
                    .id(("history-day", row.id as usize))
                    .accessibility_id(format!("history-day-{year}-{month:02}-{day:02}"))
                    .role(Role::Heading)
                    .aria_label(label.clone())
                    .pl(px(9.))
                    .pt(px(if compact { 9. } else { 18. }))
                    .pb(px(7.))
                    .border_b_1()
                    .border_color(rule)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(tokens::text::CAPTION))
                    .text_color(secondary)
                    .child(label)
            });
            div().flex().flex_col().children(date_heading).child(
                div()
                    .border_b_1()
                    .border_color(rule)
                    .child(self.history_row(
                        row_index,
                        Some(RowStyle {
                            current: current == Some(row.id),
                            show_source: width >= 520.,
                        }),
                        cx,
                    )),
            )
        };
        div()
            .w_full()
            .px(px(margin))
            .child(div().max_w(px(1140.)).child(entry))
            .into_any_element()
    }

    fn fluid_history_header(&self, compact: bool, secondary: Hsla, cx: &Context<Self>) -> Div {
        let history = &self.history;
        div()
            .w_full()
            .pt(px(if compact { 8. } else { 26. }))
            .pb(px(if compact { 5. } else { 16. }))
            .flex()
            .flex_col()
            .gap(px(if compact { 5. } else { 10. }))
            .child(
                div()
                    .id("history-title")
                    .accessibility_id("history-title")
                    .role(Role::Heading)
                    .aria_label("История")
                    .text_size(px(if compact {
                        tokens::text::PAGE_TITLE_COMPACT
                    } else {
                        tokens::text::PAGE_TITLE
                    }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("История"),
            )
            .children(
                history
                    .error
                    .as_ref()
                    .map(|error| status("history-error", error.clone())),
            )
            .when(!history.rows.is_empty(), |page| {
                page.child(
                    div()
                        .text_color(secondary)
                        .text_size(px(tokens::text::BODY))
                        .child(counted(
                            history.rows.len(),
                            "прослушивание",
                            "прослушивания",
                            "прослушиваний",
                        )),
                )
            })
            .when(history.rows.is_empty() && history.error.is_none(), |page| {
                if history.loading {
                    page.child(crate::ui::components::content_state::loading_state(
                        "history-loading", "Загружается история…", cx,
                    ))
                } else {
                    let open = button("history-open-library", "Открыть медиатеку", 49)
                        .h(px(36.))
                        .px(px(10.))
                        .gap(px(7.))
                        .rounded(px(6.))
                        .child(Icon::new(IconName::Music).pixel_size(px(16.)))
                        .child("Открыть медиатеку");
                    let open = if let Some(material) = self.glass_controls(cx) {
                        ControlVariant::SecondaryButton.paint(
                            open, material, ControlState::default(),
                        )
                    } else {
                        open
                    };
                    page.child(
                        crate::ui::components::content_state::empty_state(
                            "history-empty", "Здесь появятся прослушанные записи.",
                            Some("Начните воспроизведение в медиатеке, чтобы добавить запись в историю."),
                            IconName::History, cx,
                        ).child(open.on_activate(cx.listener(|view, _, window, cx| {
                            view.show_music_page(Page::Tracks, window, cx);
                        }))),
                    )
                }
            })
    }
}

fn date_key(row: &HistoryRow) -> (u32, u32, u32) {
    (row.time.year, row.time.month, row.time.day)
}

pub(super) fn ink_secondary(high_contrast: bool) -> Hsla {
    rgb(if high_contrast {
        tokens::contrast::SECONDARY
    } else {
        tokens::SECONDARY
    })
    .into()
}
