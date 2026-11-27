use std::{
    collections::BTreeMap,
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle, point, px,
};
use mediadeka::MainView;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use super::{Scene, dispatch, draw, move_pointer, wheel_event};
use crate::sound::{accessibility, activate_accessibility, node};

pub(super) fn copy_profile(source: &Path, destination: &Path) -> gpui::Result<()> {
    let source = source.join("mediadeka.sqlite3");
    let destination = destination.join("mediadeka.sqlite3");
    let connection = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])?;
    Ok(())
}

pub(super) async fn capture(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.show_saved_home_for_mix_capture(window, cx)
    })??;
    reveal_for_you(view, raw, scene, cx)?;

    activate_control(raw, "home-mix-discoveries", cx)?;
    let tree = frame(raw, scene, &output, "discoveries", cx)?;
    check_modes(&tree, true)?;
    check_button(&tree, "home-general-wave", "Общая волна")?;
    activate_control(raw, "home-mix-familiar", cx)?;
    let tree = frame(raw, scene, &output, "for-you", cx)?;
    check_modes(&tree, false)?;
    check_button(&tree, "home-build-mix", "Собрать микс")?;
    activate_control(raw, "home-build-mix", cx)?;

    let model = wait_for_queue(view, raw, false, cx).await?;
    let labels = check_queue(raw, scene, &output, "queue", &model, cx)?;
    let tree = accessibility_tree(raw, cx)?;
    check_button(&tree, "queue-recommendation-more", "Больше такого")?;
    check_button(&tree, "queue-recommendation-hide", "Скрыть")?;
    for (id, label, file) in [
        ("queue-recommendation-more", "Больше такого", "more-saved"),
        ("queue-recommendation-hide", "Скрыть", "hide-saved"),
    ] {
        activate_control(raw, id, cx)?;
        let expected = format!("Сохранено · {label}");
        wait_until(raw, cx, &expected, |tree, _| {
            node(tree, "queue-recommendation-status")
                .is_some_and(|status| {
                    status["aria"]["role"] == "Status" && status["aria"]["label"] == expected
                })
                .then_some(())
        })
        .await?;
        frame(raw, scene, &output, file, cx)?;
    }
    activate_control(raw, "right-panel-close", cx)?;
    reveal_for_you(view, raw, scene, cx)?;
    activate_control(raw, "home-mix-discoveries", cx)?;
    activate_control(raw, "home-general-wave", cx)?;

    let wave = wait_for_queue(view, raw, true, cx).await?;
    let wave_labels = check_queue(raw, scene, &output, "wave-queue", &wave, cx)?;
    let hidden = &model["queue"][0]["recording"];
    if hidden.is_null()
        || wave["queue"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["recording"] == *hidden))
    {
        return Err(io::Error::other("Скрытая запись сохранилась в рекомендациях волны.").into());
    }
    let tree = accessibility_tree(raw, cx)?;
    check_button(&tree, "queue-wave-stop", "Остановить волну")?;
    if node(&tree, "queue-recommendation-status").is_some() {
        return Err(io::Error::other("Статус реакции остался у другой записи.").into());
    }
    if scene.name == "desktop" {
        check_button(&tree, "home-general-wave", "Остановить волну")?;
    }
    activate_control(raw, "queue-wave-stop", cx)?;
    wait_until(
        raw,
        cx,
        "Остановка общей волны",
        |tree, _| node(tree, "queue-wave-stop").is_none().then_some(()),
    )
    .await?;
    if view.update(cx, |view, _, cx| view.home_mix_queue_for_capture(cx))? != Some(wave) {
        return Err(io::Error::other("Остановка волны изменила штатную очередь.").into());
    }
    let tree = frame(raw, scene, &output, "wave-stopped", cx)?;
    if scene.name == "desktop" {
        check_button(&tree, "home-general-wave", "Общая волна")?;
    }
    println!(
        "GPUI {}: оба режима, обе реакции, старт и остановка волны, {} подписей микса и {} подписей волны.",
        scene.name,
        labels.len(),
        wave_labels.len()
    );
    Ok(())
}

fn reveal_for_you(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    draw(raw, scene, None, cx)?;
    view.update(cx, |view, _, cx| {
        let origin = view.home_for_you_origin_for_capture();
        view.set_home_scroll_offset_for_capture((origin - 6.).max(0.), cx);
    })?;
    draw(raw, scene, None, cx)
}

pub(super) fn frame(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
    accessibility(raw, &output.join(format!("{name}-a11y.json")), cx)
}

pub(super) fn check_button(tree: &Value, id: &str, label: &str) -> gpui::Result<()> {
    let aria = node(tree, id).map(|node| &node["aria"]);
    if !aria.is_some_and(|aria| aria["role"] == "Button" && aria["label"] == label) {
        return Err(io::Error::other(format!("Нет доступной кнопки «{label}» ({id}).")).into());
    }
    Ok(())
}

fn check_modes(tree: &Value, discoveries: bool) -> gpui::Result<()> {
    for (id, label, selected) in [
        ("home-mix-familiar", "Знакомое", !discoveries),
        ("home-mix-discoveries", "Открытия", discoveries),
    ] {
        if !node(tree, id).is_some_and(|tab| {
            let aria = &tab["aria"];
            aria["role"] == "Tab" && aria["label"] == label && aria["selected"] == selected
        }) {
            return Err(io::Error::other(format!(
                "В дереве GPUI у режима «{label}» ожидалось состояние selected={selected}."
            ))
            .into());
        }
    }
    Ok(())
}

pub(super) async fn wait_until<T>(
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
    message: &str,
    mut check: impl FnMut(&Value, &mut AsyncApp) -> Option<T>,
) -> gpui::Result<T> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        let tree = accessibility_tree(raw, cx)?;
        if let Some(result) = check(&tree, cx) {
            return Ok(result);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("Истекло время ожидания: {message}.")).into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
}

async fn wait_for_queue(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    wave: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    wait_until(
        raw,
        cx,
        "Открытие штатной очереди",
        |tree, cx| {
            if wave && node(tree, "queue-wave-stop").is_none() {
                return None;
            }
            view.update(cx, |view, _, cx| view.home_mix_queue_for_capture(cx))
                .transpose()
        },
    )
    .await?
}

fn check_queue(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    name: &str,
    model: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<BTreeMap<usize, String>> {
    let rows = model["queue"]
        .as_array()
        .ok_or_else(|| io::Error::other("Нет модели очереди."))?;
    if model["current_index"] != 0 || rows.len() != 20 {
        return Err(io::Error::other(format!(
            "Ожидался старт с первой из 20 записей. Получено {} записей, текущая позиция {}.",
            rows.len(),
            model["current_index"]
        ))
        .into());
    }
    if rows
        .iter()
        .any(|row| row["origins"].as_array().is_none_or(Vec::is_empty))
    {
        return Err(io::Error::other("В очереди микса есть запись без происхождения.").into());
    }
    scroll_queue_to_top(raw, scene, cx)?;
    let tree = frame(raw, scene, output, name, cx)?;
    if node(&tree, "player-queue-list").is_none() {
        return Err(io::Error::other("Штатный список очереди отсутствует в дереве GPUI.").into());
    }
    let labels = check_queue_rows(raw, scene, rows, tree, cx)?;
    fs::write(
        output.join(format!("{name}-provenance.json")),
        serde_json::to_vec_pretty(&json!({"model": model, "accessible_labels": labels}))?,
    )?;
    scroll_queue_to_top(raw, scene, cx)?;
    Ok(labels)
}

pub(super) fn scroll_queue_to_top(
    raw: AnyWindowHandle,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let anchor = queue_scroll_anchor(scene);
    move_pointer(raw, anchor, cx)?;
    dispatch(raw, wheel_event(anchor, scene.height * 8.), cx)?;
    move_pointer(raw, point(px(12.), px(10.)), cx)?;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))
}

pub(crate) fn activate_control(
    raw: AnyWindowHandle,
    id: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let element_id = format!("Name({id:?})");
    let tab = Keystroke::parse("tab")?;
    let frame = accessibility_tree(raw, cx)?;
    if node(&frame, id).is_none() {
        return Err(io::Error::other(format!("В дереве GPUI отсутствует {id}.")).into());
    }
    for _ in 0..=frame["frame"]["tab_stop_count"].as_u64().unwrap_or(64) {
        let tree = accessibility_tree(raw, cx)?;
        if tree["gpui_focus"]
            .as_str()
            .is_some_and(|focused| tree["nodes"][focused]["element_id"] == element_id)
        {
            return press_enter(raw, cx);
        }
        raw.update(cx, |_, window, cx| {
            window.dispatch_keystroke(tab.clone(), cx);
            window.draw(cx).clear(cx);
        })?;
    }
    Err(io::Error::other(format!("Фокус GPUI не дошёл до {id}.")).into())
}

pub(super) fn press_enter(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    let enter = Keystroke::parse("enter")?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(enter.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: enter }.to_platform_input(), cx);
    })
}

fn queue_scroll_anchor(scene: Scene) -> gpui::Point<gpui::Pixels> {
    point(px(scene.width - 160.), px(scene.height * 0.64))
}

pub(super) fn check_queue_rows(
    raw: AnyWindowHandle,
    scene: Scene,
    rows: &[Value],
    mut tree: Value,
    cx: &mut AsyncApp,
) -> gpui::Result<BTreeMap<usize, String>> {
    let anchor = queue_scroll_anchor(scene);
    let mut labels = BTreeMap::new();
    for _ in 0..rows.len() * 5 {
        let nodes = tree["nodes"]
            .as_object()
            .ok_or_else(|| io::Error::other("Пустое дерево GPUI."))?;
        for row in nodes.values() {
            let Some(index) = row["element_id"].as_str().and_then(|id| {
                id.strip_prefix("NamedInteger(\"player-queue-entry\", ")?
                    .strip_suffix(')')?
                    .parse::<usize>()
                    .ok()
            }) else {
                continue;
            };
            if labels.contains_key(&(index + 1)) {
                continue;
            }
            let expected = rows
                .get(index)
                .ok_or_else(|| io::Error::other("Строка вне очереди."))?;
            let label = check_queue_row(row, expected, index, rows.len())?;
            labels.insert(index + 1, label.to_owned());
        }
        if labels.len() == rows.len() {
            return Ok(labels);
        }
        move_pointer(raw, anchor, cx)?;
        dispatch(raw, wheel_event(anchor, -scene.height * 0.28), cx)?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        tree = accessibility_tree(raw, cx)?;
    }
    Err(io::Error::other(format!(
        "В дереве доступны {} из {} строк очереди.",
        labels.len(),
        rows.len()
    ))
    .into())
}

fn check_queue_row<'a>(
    node: &'a Value,
    entry: &Value,
    index: usize,
    count: usize,
) -> gpui::Result<&'a str> {
    let aria = &node["aria"];
    let fail = || io::Error::other(format!("Нет полного происхождения у строки {}.", index + 1));
    let label = aria["label"].as_str().ok_or_else(fail)?;
    let title = entry["title"].as_str().ok_or_else(fail)?;
    let origins = entry["origins"].as_array().ok_or_else(fail)?;
    let expected_origins = origins
        .iter()
        .map(|origin| {
            let provider = origin["provider"]
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or_else(fail)?;
            let playlist = origin["playlist"]
                .as_str()
                .filter(|title| !title.is_empty())
                .ok_or_else(fail)?;
            Ok(format!("{provider} · {playlist}"))
        })
        .collect::<Result<Vec<_>, io::Error>>()?
        .join(" / ");
    let mut expected_label = format!("{}. {title}", index + 1);
    if !expected_origins.is_empty() {
        expected_label.push_str(" · Из подборок · ");
        expected_label.push_str(&expected_origins);
    }
    if aria["role"] != "ListBoxOption"
        || aria["size_of_set"] != count
        || aria["selected"] != (index == 0)
        || label != expected_label
    {
        return Err(fail().into());
    }
    Ok(label)
}

pub(super) fn accessibility_tree(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let tree = raw
        .update(cx, |_, window, _| window.debug_a11y_tree_json())?
        .ok_or_else(|| io::Error::other("GPUI не построил дерево доступности."))?;
    Ok(serde_json::from_str(&tree)?)
}
