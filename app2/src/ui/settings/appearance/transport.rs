use gpui::{Context, Div, div, prelude::*, px};
use guise::{ActionIcon, IconName};

use crate::ui::{
    MainView,
    components::{Activate, section_tab},
    design_system::icons::{self, Glyph},
    player::ControlKind,
};

pub(super) fn gallery(view: &MainView, compact: bool, cx: &Context<MainView>) -> Div {
    let transport = [
        (
            "gallery-shuffle",
            "Перемешать очередь",
            IconName::Shuffle,
            Glyph::Shuffle,
            ControlKind::Tool,
        ),
        (
            "gallery-previous",
            "Предыдущий трек",
            IconName::SkipBack,
            Glyph::Previous,
            ControlKind::Transport,
        ),
        (
            "gallery-play",
            if view.icon_gallery[0] {
                "Пауза"
            } else {
                "Воспроизвести"
            },
            IconName::Play,
            Glyph::PlayPause,
            ControlKind::Primary,
        ),
        (
            "gallery-next",
            "Следующий трек",
            IconName::SkipForward,
            Glyph::Next,
            ControlKind::Transport,
        ),
        (
            "gallery-repeat",
            "Повтор очереди",
            IconName::Repeat,
            Glyph::Repeat,
            ControlKind::Tool,
        ),
    ];
    let tools = [
        (
            "gallery-volume",
            "Громкость",
            IconName::Volume2,
            Glyph::Volume,
            ControlKind::Tool,
        ),
        (
            "gallery-queue",
            "Очередь",
            IconName::ListMusic,
            Glyph::Queue,
            ControlKind::Tool,
        ),
        (
            "gallery-lyrics",
            "Текст песни",
            IconName::Captions,
            Glyph::Lyrics,
            ControlKind::Tool,
        ),
        (
            "gallery-fullscreen",
            "Полный экран",
            IconName::Maximize2,
            Glyph::Fullscreen,
            ControlKind::Tool,
        ),
    ];
    let group =
        |buttons: &[(_, _, _, _, _)], start| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .children(buttons.iter().enumerate().map(
                    |(order, &(id, label, icon, glyph, kind))| {
                        let index = Glyph::ALL
                            .iter()
                            .position(|&candidate| candidate == glyph)
                            .expect("gallery glyph");
                        let target = view.icon_gallery[index];
                        let kind = if kind == ControlKind::Tool && target {
                            ControlKind::Selected
                        } else {
                            kind
                        };
                        let button = ActionIcon::new(id, icon)
                            .label(label)
                            .tab_order(start + order as isize)
                            .glyph_content(icons::animated_glyph(
                                id,
                                glyph,
                                target,
                                kind.dimensions(compact).1,
                                None,
                                cx,
                            ))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                if matches!(glyph, Glyph::Next | Glyph::Previous) {
                                    icons::pulse(window, cx, id);
                                } else {
                                    view.icon_gallery[index] = !view.icon_gallery[index];
                                }
                                cx.notify();
                            }));
                        kind.editorial_style(button, compact, true, cx)
                    },
                ))
        };
    let material = view.glass_controls(cx);
    let tab = |id, label, index, order| {
        section_tab(
            id,
            label,
            view.editorial_gallery_tab == index,
            order,
            material,
        )
        .on_activate(cx.listener(move |view, _, _, cx| {
            view.editorial_gallery_tab = index;
            cx.notify();
        }))
    };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(14.))
        .child(group(&transport, 902))
        .child(group(&tools, 907))
        .child(
            div()
                .id("appearance-gallery-tabs")
                .flex()
                .gap(px(2.))
                .id("gallery-tabs")
                .role(gpui::Role::TabList)
                .child(tab("gallery-tab-queue", "Очередь", 0, 911))
                .child(tab("gallery-tab-lyrics", "Текст", 1, 912)),
        )
}
