use std::{collections::BTreeSet, fs, io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, Bounds, MouseButton, Pixels, WindowHandle};
use mediadeka::{MainView, providers::ProviderId};
use serde_json::json;

use super::{Scene, activate_control, click, draw, mix::accessibility_tree};
use crate::sound::activate_accessibility;

const SHELF: usize = 0;
const PROVIDER: ProviderId = ProviderId::Spotify;
const PREVIOUS: &str = "shelf-previous-0";
const EPSILON: f32 = 0.002;

pub(super) fn capture(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.show_paged_shelf_for_capture(window, cx);
        view.set_fluid_high_contrast_for_capture(high_contrast, cx);
        view.sample_presentation_for_capture(Duration::ZERO, cx);
    })?;
    draw(raw, scene, None, cx)?;
    view.update(cx, |view, _, cx| {
        let offset = view.home_shelf_origin_for_capture() - 20.;
        view.set_home_scroll_offset_for_capture(offset, cx);
    })?;
    save(raw, scene, &output, "00-rest", cx)?;
    let initial = sample(view, cx)?;
    let initial_cards = shelf_cards(raw, cx)?;
    let arrows = [arrow(view, false, cx)?, arrow(view, true, cx)?];
    require(
        initial == (0, 0.) && initial_cards.contains(&0),
        "Первая страница должна начать с первой карточки.",
    )?;
    click(raw, MouseButton::Left, arrows[1].center(), cx)?;
    save(raw, scene, &output, "01-next-start", cx)?;
    let next = sample(view, cx)?;
    let animated = !scene.reduced && !high_contrast;
    require(
        next == (1, if animated { 0. } else { 1. }),
        "Щелчок должен выбрать страницу 1 с корректной начальной позицией.",
    )?;

    tick(view, 88, cx)?;
    save(raw, scene, &output, "02-next-middle", cx)?;
    let middle = sample(view, cx)?;
    require(
        if animated {
            (0. ..1.).contains(&middle.1) && middle.1 > 0.
        } else {
            middle == (1, 1.)
        },
        "Сдвиг полки должен зависеть от режима движения.",
    )?;
    let incoming_cards = shelf_cards(raw, cx)?;
    require(
        !incoming_cards.is_empty() && incoming_cards.is_disjoint(&initial_cards),
        "Уходящие карточки не должны попадать в доступные действия.",
    )?;
    let mid_arrow = arrow(view, false, cx)?;
    require(
        [mid_arrow, arrow(view, true, cx)?] == arrows,
        "Стрелки не должны сдвигаться вместе с карточками.",
    )?;
    click(raw, MouseButton::Left, mid_arrow.center(), cx)?;
    save(raw, scene, &output, "03-reverse-start", cx)?;
    let reverse = sample(view, cx)?;
    require(
        reverse.0 == 0 && (reverse.1 - if animated { middle.1 } else { 0. }).abs() < EPSILON,
        "Быстрый возврат начинается с видимой позиции.",
    )?;
    tick(view, 176, cx)?;
    save(raw, scene, &output, "04-reverse-middle", cx)?;
    let reverse_middle = sample(view, cx)?;
    require(
        if animated {
            (0. ..reverse.1).contains(&reverse_middle.1) && reverse_middle.1 > 0.
        } else {
            reverse_middle == (0, 0.)
        },
        "Кадр возврата должен показать движение в обратную сторону.",
    )?;
    tick(view, 400, cx)?;
    save(raw, scene, &output, "05-reverse-end", cx)?;
    let reverse_end = sample(view, cx)?;
    require(
        reverse_end == (0, 0.) && shelf_cards(raw, cx)? == initial_cards,
        "Возврат должен восстановить первую страницу.",
    )?;

    click(raw, MouseButton::Left, arrows[1].center(), cx)?;
    tick(view, 460, cx)?;
    draw(raw, scene, None, cx)?;
    let before_repeat = sample(view, cx)?;
    click(raw, MouseButton::Left, arrow(view, true, cx)?.center(), cx)?;
    save(raw, scene, &output, "06-repeat-start", cx)?;
    let repeat = sample(view, cx)?;
    require(
        repeat.0 == 2 && (repeat.1 - if animated { before_repeat.1 } else { 2. }).abs() < EPSILON,
        "Повторный щелчок должен продолжать путь к странице 2.",
    )?;
    tick(view, 700, cx)?;
    save(raw, scene, &output, "07-repeat-end", cx)?;
    let repeat_end = sample(view, cx)?;
    let repeated_cards = shelf_cards(raw, cx)?;
    require(
        repeat_end == (2, 2.)
            && !repeated_cards.is_empty()
            && repeated_cards.is_disjoint(&incoming_cards),
        "Финальная страница должна содержать только свои карточки.",
    )?;

    activate_control(raw, PREVIOUS, cx)?;
    save(raw, scene, &output, "08-keyboard", cx)?;
    let keyboard = sample(view, cx)?;
    let tree = accessibility_tree(raw, cx)?;
    let next_cards = shelf_cards(raw, cx)?;
    let focus = tree["gpui_focus"]
        .as_str()
        .and_then(|key| tree["nodes"][key]["element_id"].as_str());
    require(
        keyboard == (1, 1.)
            && focus == Some("Name(\"shelf-previous-0\")")
            && incoming_cards.is_subset(&next_cards)
            && next_cards.is_disjoint(&initial_cards)
            && next_cards.is_disjoint(&repeated_cards),
        "Enter должен сразу показать соседнюю страницу и сохранить фокус стрелки.",
    )?;
    activate_control(raw, PREVIOUS, cx)?;
    draw(raw, scene, None, cx)?;
    let boundary_tree = accessibility_tree(raw, cx)?;
    let boundary_arrow =
        &boundary_tree["nodes"][boundary_tree["gpui_focus"].as_str().unwrap_or("")];
    let boundary_focus = boundary_arrow["element_id"].as_str();
    require(
        sample(view, cx)? == (0, 0.)
            && boundary_focus == Some("Name(\"shelf-previous-0\")")
            && boundary_arrow["aria"]["description"] == "Недоступно"
            && boundary_arrow["aria"]["on_action"]
                .as_array()
                .is_none_or(|actions| !actions.iter().any(|action| action == "Click")),
        "Стрелка сохраняет фокус и сообщает недоступность без действия Click на первой странице.",
    )?;
    fs::write(
        output.join("shelf-motion.json"),
        serde_json::to_vec_pretty(&json!({
            "scene": scene.name,
            "dpi": scene.dpi,
            "reduced_motion": scene.reduced,
            "high_contrast": high_contrast,
            "samples": {
                "rest": initial,
                "next": next,
                "next_middle": middle,
                "reverse": reverse,
                "reverse_middle": reverse_middle,
                "reverse_end": reverse_end,
                "before_repeat": before_repeat,
                "repeat": repeat,
                "repeat_end": repeat_end,
                "keyboard": keyboard,
            },
            "cards": {
                "first": initial_cards,
                "incoming": incoming_cards,
                "next": next_cards,
                "repeated": repeated_cards,
            },
            "keyboard_focus": focus,
            "boundary_focus": boundary_focus,
        }))?,
    )?;
    if animated && scene.name == "desktop" {
        click(raw, MouseButton::Left, arrows[1].center(), cx)?;
        for ms in (0..=220).step_by(20) {
            tick(view, 700 + ms, cx)?;
            save(raw, scene, &output, &format!("slide-{ms:03}ms"), cx)?;
        }
    }
    Ok(())
}

fn tick(view: WindowHandle<MainView>, ms: u64, cx: &mut AsyncApp) -> gpui::Result<()> {
    view.update(cx, |view, _, cx| {
        view.sample_presentation_for_capture(Duration::from_millis(ms), cx);
    })
}

fn arrow(
    view: WindowHandle<MainView>,
    next: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<Bounds<Pixels>> {
    view.update(cx, |view, window, _| {
        view.home_shelf_arrow_bounds_for_capture(window, SHELF, next)
    })?
    .ok_or_else(|| io::Error::other(format!("В кадре нет стрелки полки (next={next}).")).into())
}

fn sample(view: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<(usize, f32)> {
    view.update(cx, |view, _, _| {
        view.home_shelf_turn_for_capture(PROVIDER, SHELF)
    })
}

fn shelf_cards(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<BTreeSet<usize>> {
    let tree = accessibility_tree(raw, cx)?;
    let nodes = tree["nodes"]
        .as_object()
        .ok_or_else(|| io::Error::other("Пустое дерево GPUI."))?;
    Ok(nodes
        .values()
        .filter(|node| node["aria"]["role"] == "Button")
        .filter_map(|node| node["element_id"].as_str())
        .filter_map(|id| id.strip_prefix("Name(\"home-item-")?.strip_suffix("\")"))
        .filter_map(|index| index.parse().ok())
        .collect())
}

fn save(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)
}

fn require(condition: bool, message: &str) -> gpui::Result<()> {
    condition
        .then_some(())
        .ok_or_else(|| io::Error::other(message).into())
}
