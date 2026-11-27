use std::path::Path;

use gpui::{AnyWindowHandle, AsyncApp};
use serde_json::Value;

use super::{Scene, focused, frame, key, named, node, node_label, press, require, snapshot};

const TRIGGER: &str = "library-filters";
const OPTIONS: &str = "library-filter-options";
const SOURCE_SELECT: &str = "library-source-select";
const SORT_SELECT: &str = "library-sort-select";
const ALL: &str = "liked-source-all";
const LOCAL: &str = "liked-source-local";
const RECENT: &str = "library-sort-recent";
const TITLE: &str = "library-sort-title";
const DOWNLOADED: &str = "library-downloaded";
const RESET: &str = "library-reset-filters";

pub(super) fn verify(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    press(raw, &named(TRIGGER), cx)?;
    let opened = frame(raw, scene, output, "filters-open", cx)?;
    check_defaults(&opened)?;

    let sources = open(raw, scene, output, "filters-source", SOURCE_SELECT, cx)?;
    require(
        checked(&sources, ALL, true) && checked(&sources, LOCAL, false),
        "Меню площадок должно отметить текущее значение.",
    )?;
    key(raw, "down", cx)?;
    key(raw, "enter", cx)?;
    require(
        focused(&snapshot(raw, cx)?) == Some(named(SOURCE_SELECT).as_str()),
        "Выбор площадки должен вернуть фокус селектору.",
    )?;
    let sorts = open(raw, scene, output, "filters-sort", SORT_SELECT, cx)?;
    require(
        checked(&sorts, RECENT, true) && checked(&sorts, TITLE, false),
        "Меню сортировки должно отметить текущее значение.",
    )?;
    key(raw, "down", cx)?;
    key(raw, "enter", cx)?;
    press(raw, &named(DOWNLOADED), cx)?;
    let selected = frame(raw, scene, output, "filters-selected", cx)?;
    require(
        node_label(&selected, TRIGGER) == Some("Фильтры · 3")
            && node(&selected, RESET).is_some()
            && node_label(&selected, SOURCE_SELECT) == Some("Площадки медиатеки · На компьютере")
            && node_label(&selected, SORT_SELECT) == Some("Сортировка · По названию")
            && downloaded(&selected, true),
        "Клавиатура должна выбрать площадку, сортировку и только скачанные треки.",
    )?;

    press(raw, &named(RESET), cx)?;
    check_defaults(&snapshot(raw, cx)?)?;
    key(raw, "escape", cx)?;
    let returned = snapshot(raw, cx)?;
    require(
        node(&returned, OPTIONS).is_none()
            && node(&returned, TRIGGER).is_some_and(|button| button["aria"]["expanded"] == false)
            && focused(&returned) == Some(named(TRIGGER).as_str()),
        "Escape должен закрыть параметры и вернуть фокус кнопке фильтров.",
    )?;
    println!(
        "{}: keyboard library filters, selectors, reset and focus passed",
        scene.name
    );
    Ok(())
}

fn check_defaults(tree: &Value) -> gpui::Result<()> {
    require(
        node(tree, TRIGGER).is_some_and(|button| button["aria"]["expanded"] == true)
            && node_label(tree, OPTIONS) == Some("Фильтры медиатеки")
            && node(tree, RESET).is_none()
            && node_label(tree, SOURCE_SELECT) == Some("Площадки медиатеки · Все")
            && node_label(tree, SORT_SELECT) == Some("Сортировка · По дате добавления")
            && node(tree, ALL).is_none()
            && node(tree, RECENT).is_none()
            && downloaded(tree, false),
        "Панель фильтров должна показать доступные параметры с исходным выбором.",
    )
}

fn open(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    name: &str,
    id: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    press(raw, &named(id), cx)?;
    frame(raw, scene, output, name, cx)
}

fn checked(tree: &Value, id: &str, selected: bool) -> bool {
    node(tree, id).is_some_and(|item| {
        item["aria"]["role"] == "MenuItem" && item["aria"]["selected"].as_bool() == Some(selected)
    })
}

fn downloaded(tree: &Value, checked: bool) -> bool {
    node(tree, DOWNLOADED).is_some_and(|checkbox| {
        checkbox["aria"]["role"] == "CheckBox"
            && checkbox["aria"]["toggled"] == if checked { "True" } else { "False" }
    })
}
