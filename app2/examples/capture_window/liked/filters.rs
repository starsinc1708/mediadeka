use std::{io, path::Path};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{
    super::{
        home::activate_control,
        sound::{accessibility, node},
    },
    Scene, frame,
};

const FILTERS: &str = "library-filters";
const RESET: &str = "library-reset-filters";
const FILTER_CONTROLS: &[&str] = &[
    "library-filter-options",
    "library-source-select",
    "library-downloaded",
    "library-sort-select",
];
const QUERY: &str = "вечер";

pub(super) fn check(
    view: WindowHandle<MainView>,
    scene: Scene,
    directory: &Path,
    all: Value,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(view);
    let tree = accessibility(raw, &directory.join("tracks-a11y.json"), cx)?;
    panel_state(&tree, false)?;

    activate_control(raw, FILTERS, cx)?;
    frame(view, scene, directory, "tracks-filters-open", cx)?;
    let tree = accessibility(raw, &directory.join("tracks-filters-open-a11y.json"), cx)?;
    panel_state(&tree, true)?;
    if node(&tree, RESET).is_some() {
        return Err(io::Error::other("Начальная панель уже предлагает сброс фильтров.").into());
    }

    apply_choices(raw, cx)?;
    let applied = frame(view, scene, directory, "tracks-filters-applied", cx)?;
    require_applied(&applied, "", 2)?;
    let tree = accessibility(raw, &directory.join("tracks-filters-applied-a11y.json"), cx)?;
    if node(&tree, RESET).is_none() {
        return Err(io::Error::other("Сброс применённых фильтров недоступен.").into());
    }

    key(raw, Keystroke::parse("escape")?, cx)?;
    frame(view, scene, directory, "tracks-filters-closed", cx)?;
    let tree = accessibility(raw, &directory.join("tracks-filters-closed-a11y.json"), cx)?;
    panel_state(&tree, false)?;
    if !focused(&tree, FILTERS) {
        return Err(io::Error::other("Escape не вернул фокус на кнопку фильтров.").into());
    }

    key(raw, Keystroke::parse("ctrl-f")?, cx)?;
    for character in QUERY.chars() {
        key(raw, Keystroke::parse(&character.to_string())?, cx)?;
    }
    let filtered = frame(view, scene, directory, "tracks-filtered", cx)?;
    let tree = accessibility(raw, &directory.join("tracks-filtered-a11y.json"), cx)?;
    let search_reveals_filters =
        view.update(cx, |view, _, _| view.uses_fluid_glass_for_capture())?;
    panel_state(&tree, search_reveals_filters)?;
    if !focused(&tree, "music-filter") {
        return Err(io::Error::other("Ctrl+F не сфокусировал поиск по медиатеке.").into());
    }
    require_applied(&filtered, QUERY, 1)?;

    if !search_reveals_filters {
        activate_control(raw, FILTERS, cx)?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    }
    activate_control(raw, RESET, cx)?;
    let reset = frame(view, scene, directory, "tracks-filters-reset", cx)?;
    if reset["query"] != QUERY
        || !reset["source"].is_null()
        || reset["only_downloaded"] != false
        || reset["sort"] != all["sort"]
        || reset["tracks"] != 1
    {
        return Err(io::Error::other(format!(
            "Сброс изменил запрос или сохранил фильтр: {reset}."
        ))
        .into());
    }
    let tree = accessibility(raw, &directory.join("tracks-filters-reset-a11y.json"), cx)?;
    panel_state(&tree, true)?;
    if node(&tree, RESET).is_some() {
        return Err(io::Error::other("После сброса команда сброса осталась видна.").into());
    }

    apply_choices(raw, cx)?;
    key(raw, Keystroke::parse("escape")?, cx)?;
    view.update(cx, |view, window, cx| {
        view.check_library_history_for_capture(window, cx)
    })?;
    let restored = view.update(cx, |view, _, _| view.liked_audit_for_capture())?;
    for field in ["query", "source", "only_downloaded", "sort"] {
        if restored[field] != filtered[field] {
            return Err(
                io::Error::other(format!("История не восстановила {field}: {restored}.")).into(),
            );
        }
    }

    Ok(json!({
        "all": all,
        "applied": applied,
        "filtered": filtered,
        "navigation_restored": restored,
        "reset": reset,
        "escape_restored_filter_focus": true,
        "ctrl_f_revealed_filters": search_reveals_filters,
    }))
}

fn apply_choices(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    for selector in ["library-source-select", "library-sort-select"] {
        activate_control(raw, selector, cx)?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        key(raw, Keystroke::parse("down")?, cx)?;
        key(raw, Keystroke::parse("enter")?, cx)?;
    }
    activate_control(raw, "library-downloaded", cx)
}

fn panel_state(tree: &Value, open: bool) -> gpui::Result<()> {
    let toggle = node(tree, FILTERS)
        .ok_or_else(|| io::Error::other("Кнопка фильтров отсутствует в дереве GPUI."))?;
    if toggle["aria"]["expanded"] != open {
        return Err(
            io::Error::other(format!("Кнопка фильтров не показывает состояние {open}.")).into(),
        );
    }
    for &id in FILTER_CONTROLS {
        if node(tree, id).is_some() != open {
            return Err(io::Error::other(format!(
                "Фильтр {id} не соответствует состоянию панели {open}."
            ))
            .into());
        }
    }
    Ok(())
}

fn require_applied(state: &Value, query: &str, tracks: u64) -> gpui::Result<()> {
    if state["source"] == "Mediadeka"
        && state["only_downloaded"] == true
        && state["sort"] == "По названию"
        && state["query"] == query
        && state["tracks"] == tracks
        && state["downloaded"] == tracks
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Не применились фильтры источника, скачивания или сортировки: {state}."
        ))
        .into())
    }
}

fn focused(tree: &Value, id: &str) -> bool {
    tree["gpui_focus"]
        .as_str()
        .is_some_and(|focused| tree["nodes"][focused]["element_id"] == format!("Name({id:?})"))
}

fn key(raw: AnyWindowHandle, keystroke: Keystroke, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}
