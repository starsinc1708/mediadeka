use std::{borrow::Cow, io, sync::Arc};

#[path = "layout/release.rs"]
mod release;
#[path = "layout/transport.rs"]
mod transport;

use gpui::{
    Context, Div, FontWeight, Hsla, ObjectFit, RenderImage, Role, Stateful, Window, div, img,
    linear_color_stop, linear_gradient, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use super::{Browse, Concept, Release, TrackId};

const BASE: u32 = 0x100f0e;
const RAIL: u32 = 0x161513;
const CONTROL: u32 = 0x282522;
const LINE: u32 = 0x48413d;
const TEXT: u32 = 0xf4f0eb;
const MUTED: u32 = 0xc3bab1;
const ACCENT: u32 = 0xe4bb98;
const SELECTED: u32 = 0x3b322d;
pub(super) const FONT: &str = "Golos Text";

pub(super) fn install_theme(cx: &mut gpui::App) -> gpui::Result<()> {
    let text = cx.text_system();
    text.add_fonts(vec![Cow::Borrowed(include_bytes!(
        "../../../assets/fonts/GolosText.ttf"
    ))])?;
    if !text.all_font_names().iter().any(|family| family == FONT) {
        return Err(io::Error::other("GPUI не зарегистрировала Golos Text.").into());
    }
    let mut theme = guise::Theme::dark()
        .with_body(rgb(BASE))
        .with_surface(rgb(CONTROL))
        .with_primary(rgb(ACCENT))
        .with_text(rgb(TEXT))
        .with_dimmed(rgb(MUTED))
        .with_border(rgb(LINE));
    theme.font_family = FONT.into();
    theme.init(cx);
    Ok(())
}

impl Render for Concept {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = window.viewport_size().width < px(1120.);
        let body = if self.queue_open {
            self.queue(compact)
        } else {
            match self.browse {
                Browse::Library | Browse::Search => self.library(compact, cx),
                Browse::Release(selection) => self.release(selection, compact, window, cx),
            }
        };
        let backdrop = if self.queue_open {
            Some(&self.releases[self.current.release])
        } else {
            match self.browse {
                Browse::Library => self.releases.first(),
                Browse::Search => None,
                Browse::Release(selection) => self.releases.get(selection.release),
            }
        };
        div()
            .size_full()
            .id("music-concept")
            .track_focus(&self.focus)
            .bg(rgb(BASE))
            .text_color(rgb(TEXT))
            .font_family(FONT)
            .text_size(px(15.))
            .relative()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "tab" => {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                        if let Some((_, index)) = view.focused_row(window, cx) {
                            view.track_scroll.scroll_to_item(index);
                        }
                    }
                    "escape" => {
                        view.back();
                        cx.notify();
                    }
                    "k" if event.keystroke.modifiers.control => {
                        view.open_search(window, cx);
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .children(backdrop.map(|release| image_field(release, compact)))
            .child(self.header(compact, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!compact, |row| row.child(self.navigation(cx)))
                    .child(body),
            )
            .child(self.transport(compact, cx))
    }
}

impl Concept {
    fn header(&self, compact: bool, cx: &Context<Self>) -> Div {
        let search = !self.queue_open && self.browse == Browse::Search;
        let search_width = px(if compact { 210. } else { 342. });
        div()
            .flex_none()
            .h(px(if compact { 51. } else { 73. }))
            .px(px(if compact { 18. } else { 32. }))
            .border_b_1()
            .border_color(rgb(LINE))
            .bg(Hsla::from(rgb(RAIL)).alpha(0.52))
            .flex()
            .items_center()
            .gap(px(if compact { 14. } else { 36. }))
            .child(
                div()
                    .text_size(px(if compact { 18. } else { 23. }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Mediadeka"),
            )
            .when(compact, |bar| bar.child(self.library_button(cx)))
            .when(search, |bar| {
                bar.child(
                    div()
                        .w(search_width)
                        .h(px(if compact { 31. } else { 36. }))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .px(px(10.))
                        .border_1()
                        .border_color(rgb(ACCENT))
                        .rounded(px(5.))
                        .bg(rgb(CONTROL))
                        .child(glyph(IconName::Search, ACCENT))
                        .child(div().flex_1().min_w_0().child(self.query.clone())),
                )
            })
            .when(!search, |bar| {
                bar.child(
                    button("concept-search", "Поиск по медиатеке", 2)
                        .w(search_width)
                        .h(px(if compact { 31. } else { 36. }))
                        .border_1()
                        .border_color(rgb(LINE))
                        .px(px(10.))
                        .gap(px(9.))
                        .child(glyph(IconName::Search, MUTED))
                        .child(div().flex_1().child("Поиск"))
                        .when(!compact, |button| {
                            button.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child("Ctrl+K"),
                            )
                        })
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.open_search(window, cx);
                        })),
                )
            })
            .child(div().flex_1())
    }

    fn library_button(&self, cx: &Context<Self>) -> Stateful<Div> {
        button("concept-library", "Медиатека", 1)
            .px(px(10.))
            .gap(px(9.))
            .h(px(38.))
            .child(glyph(IconName::Library, ACCENT))
            .child("Медиатека")
            .aria_toggled(gpui::accesskit::Toggled::from(
                !self.queue_open && self.browse != Browse::Search,
            ))
            .when(!self.queue_open && self.browse != Browse::Search, |item| {
                item.bg(rgb(SELECTED))
            })
            .on_click(cx.listener(|view, _, _, cx| {
                if view.queue_open {
                    view.queue_open = false;
                } else {
                    view.browse = Browse::Library;
                }
                cx.notify();
            }))
    }

    fn navigation(&self, cx: &Context<Self>) -> Div {
        div()
            .w(px(184.))
            .flex_none()
            .bg(Hsla::from(rgb(BASE)).alpha(0.88))
            .rounded_tr(px(19.))
            .p(px(16.))
            .pt(px(27.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(self.library_button(cx))
            .child(
                button("concept-nav-search", "Поиск по медиатеке", 3)
                    .h(px(38.))
                    .px(px(10.))
                    .gap(px(9.))
                    .text_color(rgb(MUTED))
                    .aria_toggled(gpui::accesskit::Toggled::from(
                        !self.queue_open && self.browse == Browse::Search,
                    ))
                    .when(!self.queue_open && self.browse == Browse::Search, |item| {
                        item.bg(rgb(SELECTED)).text_color(rgb(TEXT))
                    })
                    .child(glyph(IconName::Search, MUTED))
                    .child("Поиск")
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_search(window, cx);
                    })),
            )
            .child(div().h(px(1.)).mt(px(13.)).mb(px(8.)).bg(rgb(LINE)))
            .child(
                button("concept-nav-queue", "Очередь", 4)
                    .h(px(38.))
                    .px(px(10.))
                    .gap(px(9.))
                    .text_color(rgb(MUTED))
                    .aria_toggled(gpui::accesskit::Toggled::from(self.queue_open))
                    .when(self.queue_open, |item| {
                        item.bg(rgb(SELECTED)).text_color(rgb(TEXT))
                    })
                    .child(glyph(IconName::ListMusic, MUTED))
                    .child("Очередь")
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.queue_open = !view.queue_open;
                        cx.notify();
                    })),
            )
    }

    fn library(&self, compact: bool, cx: &Context<Self>) -> Div {
        let search = self.browse == Browse::Search;
        let query = self.query.read(cx).text().trim().to_lowercase();
        let releases = self
            .releases
            .iter()
            .enumerate()
            .filter_map(|(release, item)| {
                let index = if !search
                    || query.is_empty()
                    || [&item.info.title, &item.info.artist]
                        .iter()
                        .any(|value| matches(value, &query))
                {
                    0
                } else {
                    item.info.tracks.iter().position(|track| {
                        matches(&track.title, &query) || matches(&track.artist, &query)
                    })?
                };
                Some((TrackId { release, index }, item))
            })
            .collect::<Vec<_>>();
        let size = if compact {
            if search { 88. } else { 122. }
        } else {
            286.
        };
        surface().child(
            div()
                .id("concept-library-scroll")
                .size_full()
                .overflow_y_scroll()
                .px(px(if compact { 22. } else { 66. }))
                .py(px(if compact { 17. } else { 67. }))
                .flex()
                .flex_col()
                .gap(px(if compact { 12. } else { 32. }))
                .child(title(
                    "concept-library-title",
                    if search {
                        "Поиск"
                    } else {
                        "Медиатека"
                    },
                    if compact { 28. } else { 62. },
                ))
                .when(releases.is_empty(), |pane| {
                    pane.child(
                        div()
                            .id("concept-no-results")
                            .role(Role::Status)
                            .text_color(rgb(MUTED))
                            .child("Ничего не найдено"),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(if compact { 20. } else { 40. }))
                        .children(releases.into_iter().map(|(selection, release)| {
                            let metadata = if selection.index > 0 {
                                self.track(selection).title.clone()
                            } else {
                                release.info.artist.clone()
                            };
                            button(
                                format!("concept-release-{}", selection.release),
                                release.info.title.clone(),
                                20 + selection.release as isize,
                            )
                            .w(px(size))
                            .flex_col()
                            .items_start()
                            .gap(px(if compact { 4. } else { 10. }))
                            .child(sleeve(release.image.clone(), size))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(release.info.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(rgb(MUTED))
                                    .child(metadata),
                            )
                            .on_click(cx.listener(
                                move |view, _, window, cx| {
                                    view.open_release(selection);
                                    window.focus(&view.focus, cx);
                                    cx.notify();
                                },
                            ))
                        })),
                ),
        )
    }

    fn queue(&self, compact: bool) -> Div {
        let current = self.track(self.current);
        let release = &self.releases[self.current.release];
        surface().child(
            div()
                .id("concept-queue-scroll")
                .size_full()
                .overflow_y_scroll()
                .px(px(if compact { 22. } else { 66. }))
                .py(px(if compact { 14. } else { 57. }))
                .flex()
                .flex_col()
                .gap(px(if compact { 14. } else { 32. }))
                .child(title(
                    "concept-queue-title",
                    "Очередь",
                    if compact { 30. } else { 60. },
                ))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(if compact { 32. } else { 92. }))
                        .child(
                            div()
                                .w(px(if compact { 215. } else { 456. }))
                                .flex_none()
                                .flex()
                                .flex_col()
                                .gap(px(if compact { 6. } else { 14. }))
                                .child(div().text_color(rgb(MUTED)).text_size(px(13.)).child(
                                    if self.paused {
                                        "На паузе"
                                    } else {
                                        "Сейчас играет"
                                    },
                                ))
                                .child(sleeve(
                                    release.image.clone(),
                                    if compact { 88. } else { 292. },
                                ))
                                .child(
                                    div()
                                        .text_size(px(if compact { 19. } else { 32. }))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(current.title.clone()),
                                )
                                .child(div().text_color(rgb(MUTED)).child(current.artist.clone())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .max_w(px(710.))
                                .flex()
                                .flex_col()
                                .gap(px(if compact { 6. } else { 14. }))
                                .child(title(
                                    "concept-following-title",
                                    "Дальше",
                                    if compact { 20. } else { 32. },
                                ))
                                .when(self.following.is_empty(), |list| {
                                    list.child(div().text_color(rgb(MUTED)).child("Очередь пуста"))
                                })
                                .children(self.following.iter().enumerate().map(|(index, id)| {
                                    let track = self.track(*id);
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(12.))
                                        .h(px(if compact { 43. } else { 64. }))
                                        .border_b_1()
                                        .border_color(rgb(LINE))
                                        .child(
                                            div()
                                                .w(px(23.))
                                                .text_color(rgb(MUTED))
                                                .child((index + 1).to_string()),
                                        )
                                        .child(sleeve(
                                            self.releases[id.release].image.clone(),
                                            if compact { 27. } else { 39. },
                                        ))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(div().truncate().child(track.title.clone()))
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(12.))
                                                        .text_color(rgb(MUTED))
                                                        .child(track.artist.clone()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(rgb(MUTED))
                                                .child(track.duration.clone()),
                                        )
                                })),
                        ),
                ),
        )
    }
}

fn matches(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(query)
}

fn surface() -> Div {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
}

fn image_field(release: &Release, compact: bool) -> Div {
    let ink = Hsla::from(rgb(BASE));
    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .child(
            img(release.background.clone())
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
        .child(div().absolute().inset_0().bg(ink.alpha(0.53)))
        .child(div().absolute().inset_0().bg(linear_gradient(
            180.,
            linear_color_stop(Hsla::from(rgb(0x74716c)).alpha(0.32), 0.),
            linear_color_stop(ink, if compact { 0.80 } else { 0.85 }),
        )))
}

fn sleeve(image: Arc<RenderImage>, size: f32) -> Div {
    let radius = px(if size > 220. { 13. } else { 5. });
    div()
        .size(px(size))
        .flex_none()
        .rounded(radius)
        .overflow_hidden()
        .child(
            img(image)
                .size_full()
                .rounded(radius)
                .object_fit(ObjectFit::Cover),
        )
}

fn title(id: &'static str, text: &str, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Heading)
        .aria_label(text.to_owned())
        .text_size(px(size))
        .line_height(px(size * 1.09))
        .font_weight(FontWeight(670.))
        .child(text.to_owned())
}

fn button(id: impl Into<String>, label: impl Into<String>, order: isize) -> Stateful<Div> {
    button_base(id, label, order).hover(|style| style.bg(rgb(CONTROL)))
}

fn button_base(id: impl Into<String>, label: impl Into<String>, order: isize) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .accessibility_id(id)
        .role(Role::Button)
        .aria_label(label.into())
        .tab_index(order)
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_visible(|style| style.border_color(rgb(ACCENT)))
        .cursor_pointer()
        .rounded(px(5.))
        .flex()
        .items_center()
}

fn glyph(icon: IconName, color: u32) -> Icon {
    Icon::new(icon)
        .pixel_size(px(16.))
        .color(Hsla::from(rgb(color)))
}
