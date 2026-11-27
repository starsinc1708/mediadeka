use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, MouseButton, WindowHandle,
    point, px,
};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{Scene, delay, draw, require, wait_playback};

const SEARCH: &str = r#"Name("playlist-search")"#;
const CLEAR_SEARCH: &str = r#"Name("playlist-search-clear")"#;
const TRACK: &str = r#"NamedInteger("playlist-row", 0)"#;
const COVER_PLAY: &str = r#"Name("playlist-cover-play-0")"#;
const COLUMNS: &str = r#"Name("playlist-columns")"#;
const MENU_TRIGGER: &str = r#"NamedInteger("playlist-row-menu", 2)"#;
const MENU: &str = r#"Name("playlist-menu")"#;
const SORT: &str = r#"Name("playlist-sort")"#;
const SORT_MENU: &str = r#"Name("playlist-sort-options")"#;
const SORT_TITLE: &str = r#"Name("playlist-sort-title")"#;

fn fluid_glass() -> bool {
    std::env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3")
}

pub(super) async fn check_filter(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    key(raw, "escape", cx)?;
    tab_to(raw, r#"Name("playlist-scrollbar")"#, cx)?;
    let before = tree(raw, cx)?;
    let persistent_cover = scene.width == 1920. && !fluid_glass();
    require(
        node(&before, SEARCH).is_some() == (persistent_cover || fluid_glass())
            && node(&before, r#"Name("playlist-collection")"#).is_some() == persistent_cover,
        "Положение фильтра и обложки не соответствует композиции при прокрутке.".into(),
    )?;
    key(raw, "ctrl-f", cx)?;
    let focused = tree(raw, cx)?;
    require(
        focus(&focused, "gpui_focus")["element_id"] == SEARCH && node(&focused, SEARCH).is_some(),
        format!(
            "Ctrl+F не показал фильтр: {}",
            focus(&focused, "gpui_focus")
        ),
    )?;
    if persistent_cover {
        let last_track = r#"NamedInteger("playlist-row", 9999)"#;
        require(
            node(&before, last_track).is_some() && node(&focused, last_track).is_some(),
            "Ctrl+F в широком окне сдвинул список.".into(),
        )?;
    }
    save(output, "filter-focus", &focused)?;
    if !scene.reduced {
        delay(cx, 180).await;
    }
    draw(raw, scene, Some(&output.join("filter-focus.png")), cx)?;
    let filter_top = window.update(cx, |view, window, cx| {
        view.playlist_filter_top_for_capture(window, cx)
    })?;
    for letter in ["z", "z", "q"] {
        key(raw, letter, cx)?;
    }
    let filtered = tree(raw, cx)?;
    save(output, "filter-empty", &filtered)?;
    require(
        node(&filtered, SEARCH).is_some_and(|search| search["aria"]["value"] == "zzq")
            && node(&filtered, r#"Name("playlist-empty")"#).is_some(),
        "Фильтр не принял запрос с клавиатуры или не показал пустой результат.".into(),
    )?;
    if fluid_glass() {
        require(
            node(&filtered, CLEAR_SEARCH)
                .is_some_and(|control| control["aria"]["label"] == "Очистить поиск"),
            "Поле поиска должно показать именованную кнопку очистки.".into(),
        )?;
        if !scene.reduced {
            delay(cx, 180).await;
        }
        draw(raw, scene, Some(&output.join("filter-typed.png")), cx)?;
        let typed_top = window.update(cx, |view, window, cx| {
            view.playlist_filter_top_for_capture(window, cx)
        })?;
        require(
            filter_top
                .zip(typed_top)
                .is_some_and(|(before, after)| (before - after).abs() < 1.),
            format!("Начало ввода изменило положение фильтра: {filter_top:?} → {typed_top:?}"),
        )?;
        tab_to(raw, CLEAR_SEARCH, cx)?;
        key(raw, "enter", cx)?;
        let cleared = tree(raw, cx)?;
        require(
            focus(&cleared, "gpui_focus")["element_id"] == SEARCH
                && node(&cleared, CLEAR_SEARCH).is_none(),
            "Очистка должна вернуть фокус полю и убрать кнопку.".into(),
        )?;
    } else {
        key(raw, "ctrl-a", cx)?;
        key(raw, "backspace", cx)?;
    }
    require(
        node(&tree(raw, cx)?, TRACK).is_some(),
        "Очистка поиска не вернула строки.".into(),
    )
}

pub(super) async fn check_commands(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    if fluid_glass() {
        let detail = tree(raw, cx)?;
        let title = node(&detail, r#"Name("playlist-title")"#);
        let cover = node(&detail, r#"Name("playlist-hero-artwork")"#);
        require(
            title.is_some_and(|node| {
                node["aria"]["role"] == "Heading"
                    && node["aria"]["label"]
                        .as_str()
                        .is_some_and(|label| label.len() > 80)
            }) && cover.is_some_and(|node| node["aria"]["role"] == "Image")
                && node(&detail, r#"Name("playlist-play")"#).is_some(),
            "Fluid Glass должен сохранить полное имя, обложку и кнопку запуска плейлиста.".into(),
        )?;
        let cover_play = node(&detail, COVER_PLAY);
        require(
            cover_play.is_some_and(|control| {
                control["aria"]["role"] == "Button"
                    && control["aria"]["label"]
                        .as_str()
                        .is_some_and(|name| name.starts_with("Слушать · "))
            }),
            "У обложки должна быть собственная доступная кнопка воспроизведения.".into(),
        )?;
        save(output, "fluid-detail", &detail)?;
        draw(raw, scene, Some(&output.join("fluid-detail.png")), cx)?;
        check_display_density(window, scene, output, cx)?;
    }
    tab_to(raw, if fluid_glass() { COVER_PLAY } else { TRACK }, cx)?;
    key(raw, "enter", cx)?;
    let playing = wait_playback(window, cx).await?;
    require(
        playing.0 == Some(0) && playing.1 == Some(0),
        format!("Enter не запустил первую строку: {playing:?}"),
    )?;
    window.update(cx, |view, window, cx| {
        view.close_right_panel_for_capture(window, cx)
    })?;
    tab_to(raw, r#"Name("player-play")"#, cx)?;
    key(raw, "enter", cx)?;
    let started = Instant::now();
    let paused = loop {
        let paused = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
        if paused.2 == "paused" {
            break paused;
        }
        require(
            started.elapsed().as_secs() < 6,
            format!("Клавиша не приостановила звук: {paused:?}"),
        )?;
        delay(cx, 40).await;
    };
    let paused_tree = tree(raw, cx)?;
    let paused_row = &node(&paused_tree, TRACK)
        .ok_or_else(|| io::Error::other("Первая строка вне области просмотра."))?["aria"];
    require(
        paused_row["selected"] == true
            && paused_row["description"] == "Текущий трек · пауза"
            && paused_row["label"]
                .as_str()
                .is_some_and(|label| label.contains(" · ")),
        format!("В дереве не различаются выбор, пауза или подпись строки: {paused_row}"),
    )?;
    save(output, "paused", &paused_tree)?;

    if fluid_glass() && scene.height == 400. {
        tab_to(raw, r#"Name("playlist-scrollbar")"#, cx)?;
        key(raw, "end", cx)?;
    }
    tab_to(raw, MENU_TRIGGER, cx)?;
    key(raw, "enter", cx)?;
    let opened = tree(raw, cx)?;
    require(
        focus(&opened, "gpui_focus")["element_id"] == MENU
            && focus(&opened, "active_descendant_focus")["aria"]["label"] == "Слушать",
        "Меню не получило фокус и активную команду.".into(),
    )?;
    key(raw, "down", cx)?;
    let menu = tree(raw, cx)?;
    require(
        focus(&menu, "active_descendant_focus")["aria"]["label"] == "Играть следующим",
        "Стрелка не сообщила новую команду в дереве доступности.".into(),
    )?;
    save(output, "menu-keyboard", &menu)?;
    draw(raw, scene, Some(&output.join("menu-keyboard.png")), cx)?;
    key(raw, "escape", cx)?;
    let closed = tree(raw, cx)?;
    require(
        node(&closed, MENU).is_none() && focus(&closed, "gpui_focus")["element_id"] == MENU_TRIGGER,
        "Esc не закрыл меню с возвратом на кнопку строки.".into(),
    )?;
    save(output, "menu-closed", &closed)?;

    for stroke in ["enter", "down", "enter"] {
        key(raw, stroke, cx)?;
    }
    tab_to(raw, r#"Name("player-next")"#, cx)?;
    key(raw, "enter", cx)?;
    let next = wait_playback(window, cx).await?;
    require(
        next.1 == Some(2),
        format!("Очередь не поставила выбранную запись следующей: {next:?}"),
    )?;
    let transport = tree(raw, cx)?;
    for id in [
        "player-shuffle",
        "player-prev",
        "player-play",
        "player-next",
        "player-repeat",
        "player-progress",
        "player-mute",
        "player-volume",
        "player-devices",
        "player-queue",
        "player-lyrics",
    ] {
        require(
            node(&transport, &format!(r#"Name("{id}")"#)).is_some(),
            format!("Плеер не отобразил {id} при ширине {}.", scene.width),
        )?;
    }
    tab_to(raw, r#"Name("player-queue")"#, cx)?;
    key(raw, "enter", cx)?;
    let queue = tree(raw, cx)?;
    save(output, "playing-queue", &queue)?;
    let current = queue["nodes"].as_object().is_some_and(|nodes| {
        nodes.values().any(|row| {
            row["aria"]["role"] == "ListBoxOption"
                && row["aria"]["selected"] == true
                && row["aria"]["position_in_set"] == 1
                && row["aria"]["description"]
                    .as_str()
                    .is_some_and(|label| label.starts_with("Текущий трек"))
        })
    });
    require(
        node(&queue, r#"Name("player-queue-list")"#).is_some()
            && node(&queue, r#"Name("right-panel-close")"#).is_some()
            && current,
        "Клавиатура не открыла доступную панель очереди.".into(),
    )?;
    draw(raw, scene, Some(&output.join("playing-queue.png")), cx)?;
    tab_to(raw, r#"Name("right-panel-close")"#, cx)?;
    key(raw, "enter", cx)?;
    require(
        node(&tree(raw, cx)?, r#"Name("player-queue-list")"#).is_none(),
        "Клавиатура не закрыла панель очереди.".into(),
    )?;
    for mode in [r#"Name("player-shuffle")"#, r#"Name("player-repeat")"#] {
        tab_to(raw, mode, cx)?;
        key(raw, "enter", cx)?;
        require(
            node(&tree(raw, cx)?, mode).is_some_and(|button| button["aria"]["toggled"] == "True"),
            format!("{mode} не сообщил о включении."),
        )?;
    }
    let toggled = tree(raw, cx)?;
    save(output, "transport-modes", &toggled)?;
    draw(raw, scene, Some(&output.join("transport-modes.png")), cx)?;
    save(
        output,
        "keyboard-report",
        &json!({"playing": playing, "paused": paused, "next": next,
            "queue_by_keyboard": true, "transport_modes_by_keyboard": true}),
    )
}

fn check_display_density(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let before = window.update(cx, |view, _, _| view.playlist_density_for_capture())??;
    let menu = choose_density(raw, "compact", cx)?;
    let compact = window.update(cx, |view, _, _| view.playlist_density_for_capture())??;
    require(
        compact.0 == "compact"
            && before
                .1
                .zip(compact.1)
                .is_some_and(|(before, after)| before > after),
        format!(
            "Настройки отображения не применили плотность к профилю и строкам: {before:?} → {compact:?}."
        ),
    )?;
    save(output, "display-density-menu", &menu)?;
    draw(
        raw,
        scene,
        Some(&output.join("display-density-compact.png")),
        cx,
    )?;
    choose_density(raw, before.0, cx)?;
    let restored = window.update(cx, |view, _, _| view.playlist_density_for_capture())??;
    require(
        restored == before,
        format!("Не восстановилась плотность строк: {restored:?}, ожидалось {before:?}."),
    )
}

fn choose_density(raw: AnyWindowHandle, density: &str, cx: &mut AsyncApp) -> gpui::Result<Value> {
    tab_to(raw, COLUMNS, cx)?;
    key(raw, "enter", cx)?;
    let id = format!(r#"Name("playlist-density-{density}")"#);
    for _ in 0..5 {
        let menu = tree(raw, cx)?;
        if focus(&menu, "active_descendant_focus")["element_id"] == id {
            key(raw, "enter", cx)?;
            require(
                focus(&tree(raw, cx)?, "gpui_focus")["element_id"] == COLUMNS,
                "Выбор плотности должен возвращать фокус на кнопку отображения.".into(),
            )?;
            return Ok(menu);
        }
        key(raw, "down", cx)?;
    }
    Err(io::Error::other(format!("Нет режима плотности {density}.")).into())
}

pub(super) async fn check_sort(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let mut before = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
    if before.2 != "playing" {
        window.update(cx, |view, _, cx| {
            view.play_playlist_track_for_capture(0, cx)
        })?;
        wait_playback(window, cx).await?;
        before = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
    }
    tab_to(raw, SORT, cx)?;
    let closed = tree(raw, cx)?;
    let trigger = node(&closed, SORT)
        .ok_or_else(|| io::Error::other("Нет триггера сортировки плейлиста."))?;
    require(
        trigger["aria"]["expanded"] == false
            && trigger["aria"]["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("Сортировка · ")),
        format!("Триггер сортировки не сообщает назначение и состояние: {trigger}"),
    )?;
    let initial = window.update(cx, |view, _, _| view.playlist_sort_for_capture())?;
    key(raw, "enter", cx)?;
    let opened = tree(raw, cx)?;
    require(
        node(&opened, SORT_MENU).is_some()
            && node(&opened, SORT).is_some_and(|trigger| trigger["aria"]["expanded"] == true)
            && focus(&opened, "gpui_focus")["element_id"] == SORT_MENU
            && active_row(&opened) == Some(initial)
            && node(&opened, r#"Name("playlist-sort-playlist")"#)
                .is_some_and(|row| row["aria"]["selected"] == true),
        "Enter не открыл меню сортировки с текущим значением под курсором.".to_owned(),
    )?;
    key(raw, "down", cx)?;
    let cursor = tree(raw, cx)?;
    require(
        active_row(&cursor) == Some("По названию")
            && node(&cursor, SORT).is_some_and(|trigger| {
                trigger["aria"]["label"]
                    .as_str()
                    .is_some_and(|label| label.ends_with(initial))
            }),
        "Стрелка должна двигать курсор, не применяя сортировку.".to_owned(),
    )?;
    if scene.reduced {
        draw(raw, scene, Some(&output.join("sort-menu.png")), cx)?;
    } else {
        for (millis, name) in [
            (0, "sort-menu-start.png"),
            (55, "sort-menu-55ms.png"),
            (160, "sort-menu.png"),
        ] {
            window.update(cx, |view, _, cx| {
                view.sample_playlist_sort_menu_for_capture(Some(Duration::from_millis(millis)), cx)
            })?;
            draw(raw, scene, Some(&output.join(name)), cx)?;
        }
        window.update(cx, |view, _, cx| {
            view.sample_playlist_sort_menu_for_capture(None, cx)
        })?;
    }
    save(output, "sort-menu", &cursor)?;
    let (trigger_bounds, menu_bounds) =
        window.update(cx, |view, _, _| view.playlist_sort_bounds_for_capture())?;
    let menu_bounds =
        menu_bounds.ok_or_else(|| io::Error::other("Меню сортировки не измерило себя."))?;
    let width = opened["frame"]["viewport_size"]["width"]
        .as_f64()
        .unwrap_or_default() as f32;
    let height = opened["frame"]["viewport_size"]["height"]
        .as_f64()
        .unwrap_or_default() as f32;
    require(
        menu_bounds.left() >= px(0.)
            && menu_bounds.top() >= px(0.)
            && menu_bounds.right() <= px(width + 1.)
            && menu_bounds.bottom() <= px(height + 1.)
            && (near(menu_bounds.top(), trigger_bounds.bottom() + px(4.))
                || near(menu_bounds.bottom(), trigger_bounds.top() - px(4.)))
            && (near(menu_bounds.right(), trigger_bounds.right())
                || near(menu_bounds.left(), trigger_bounds.left())),
        format!(
            "Меню сортировки не привязано к триггеру или выходит за окно: menu {menu_bounds:?}, trigger {trigger_bounds:?}."
        ),
    )?;
    key(raw, "enter", cx)?;
    let applied = tree(raw, cx)?;
    require(
        node(&applied, SORT_MENU).is_none()
            && focus(&applied, "gpui_focus")["element_id"] == SORT
            && node(&applied, SORT).is_some_and(|trigger| {
                trigger["aria"]["expanded"] == false
                    && trigger["aria"]["label"]
                        .as_str()
                        .is_some_and(|label| label.ends_with("По названию"))
            }),
        "Enter должен применить сортировку, закрыть меню и вернуть фокус.".to_owned(),
    )?;
    delay(cx, 250).await;
    let after = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
    require(
        after.2 == "playing" && after.1 == before.1 && after.3 > before.3,
        format!("Сортировка прервала воспроизведение: {before:?} → {after:?}"),
    )?;
    key(raw, "enter", cx)?;
    let reopened = tree(raw, cx)?;
    require(
        active_row(&reopened) == Some("По названию")
            && node(&reopened, SORT_TITLE).is_some_and(|row| row["aria"]["selected"] == true),
        "Повторное открытие не отметило применённое значение.".to_owned(),
    )?;
    key(raw, "escape", cx)?;
    let escaped = tree(raw, cx)?;
    require(
        node(&escaped, SORT_MENU).is_none() && focus(&escaped, "gpui_focus")["element_id"] == SORT,
        "Escape не закрыл меню сортировки с возвратом фокуса.".to_owned(),
    )?;
    key(raw, "enter", cx)?;
    tree(raw, cx)?;
    key(raw, "tab", cx)?;
    let tabbed = tree(raw, cx)?;
    require(
        node(&tabbed, SORT_MENU).is_none(),
        "Tab не закрыл меню сортировки перед передачей фокуса.".to_owned(),
    )?;
    tab_to(raw, SORT, cx)?;
    key(raw, "enter", cx)?;
    tree(raw, cx)?;
    super::super::home::click(raw, MouseButton::Left, point(px(8.), px(8.)), cx)?;
    let outside = tree(raw, cx)?;
    require(
        node(&outside, SORT_MENU).is_none() && focus(&outside, "gpui_focus")["element_id"] == SORT,
        "Клик снаружи не закрыл меню сортировки с возвратом фокуса.".to_owned(),
    )?;
    let (trigger_bounds, _) =
        window.update(cx, |view, _, _| view.playlist_sort_bounds_for_capture())?;
    super::super::home::click(raw, MouseButton::Left, trigger_bounds.center(), cx)?;
    let pointed = tree(raw, cx)?;
    require(
        node(&pointed, SORT_MENU).is_some(),
        "Клик по триггеру не открыл меню сортировки.".to_owned(),
    )?;
    key(raw, "escape", cx)?;
    let pointer_closed = tree(raw, cx)?;
    require(
        node(&pointer_closed, SORT_MENU).is_none()
            && focus(&pointer_closed, "gpui_focus")["element_id"] == SORT,
        "Escape после открытия указателем не вернул фокус триггеру.".to_owned(),
    )?;
    let resize = if scene.name == "1920x1080" {
        key(raw, "enter", cx)?;
        tree(raw, cx)?;
        let (trigger_before, menu_before) =
            window.update(cx, |view, _, _| view.playlist_sort_bounds_for_capture())?;
        let compact = Scene {
            name: "800x400-resized",
            width: 800.,
            height: 400.,
            dpi: 96,
            reduced: scene.reduced,
        };
        window
            .update(cx, |_, window, _| {
                super::super::native_dpi_change(window, compact)
            })??
            .apply();
        delay(cx, 150).await;
        tree(raw, cx)?;
        draw(
            raw,
            compact,
            Some(&output.join("sort-menu-resized.png")),
            cx,
        )?;
        let resized = tree(raw, cx)?;
        let (trigger_after, menu_after) =
            window.update(cx, |view, _, _| view.playlist_sort_bounds_for_capture())?;
        let menu_after = menu_after.ok_or_else(|| io::Error::other("Меню не пережило ресайз."))?;
        let width = resized["frame"]["viewport_size"]["width"]
            .as_f64()
            .unwrap_or_default() as f32;
        let height = resized["frame"]["viewport_size"]["height"]
            .as_f64()
            .unwrap_or_default() as f32;
        let rows = (0..4)
            .map(|index| {
                window.update(cx, |view, _, _| {
                    view.playlist_sort_row_bounds_for_capture(index)
                })
            })
            .collect::<gpui::Result<Vec<_>>>()?;
        require(
            menu_after.left() >= px(0.)
                && menu_after.top() >= px(0.)
                && menu_after.right() <= px(width + 1.)
                && menu_after.bottom() <= px(height + 1.)
                && (near(menu_after.top(), trigger_after.bottom() + px(4.))
                    || near(menu_after.bottom(), trigger_after.top() - px(4.)))
                && (near(menu_after.right(), trigger_after.right())
                    || near(menu_after.left(), trigger_after.left()))
                && rows.iter().all(Option::is_some)
                && rows.iter().flatten().all(|row| {
                    row.left() >= px(0.)
                        && row.top() >= px(0.)
                        && row.right() <= px(width + 1.)
                        && row.bottom() <= px(height + 1.)
                }),
            format!(
                "Меню после ресайза не привязано к триггеру или вышло за окно: menu {menu_after:?}, trigger {trigger_after:?}, rows {rows:?}."
            ),
        )?;
        save(
            output,
            "sort-resize-report",
            &json!({
                "before": {"trigger": bounds_json(trigger_before), "menu": menu_before.map(bounds_json)},
                "after": {"trigger": bounds_json(trigger_after), "menu": bounds_json(menu_after)},
                "viewport": [width, height],
                "rows": rows.iter().flatten().map(|row| bounds_json(*row)).collect::<Vec<_>>(),
            }),
        )?;
        key(raw, "escape", cx)?;
        let restored = tree(raw, cx)?;
        require(
            node(&restored, SORT_MENU).is_none()
                && focus(&restored, "gpui_focus")["element_id"] == SORT,
            "Escape после ресайза не вернул фокус.".to_owned(),
        )?;
        window
            .update(cx, |_, window, _| {
                super::super::native_dpi_change(window, scene)
            })??
            .apply();
        delay(cx, 150).await;
        draw(raw, scene, None, cx)?;
        draw(raw, scene, None, cx)?;
        json!({
            "resized_to": [800., 400.],
            "restored_to": [scene.width, scene.height],
            "menu_stayed_anchored": true,
        })
    } else {
        Value::Null
    };
    save(
        output,
        "sort-report",
        &json!({
            "scene": scene.name,
            "reduced_motion": scene.reduced,
            "initial": initial,
            "applied": "По названию",
            "playback_before": before,
            "playback_after": after,
            "trigger": bounds_json(trigger_bounds),
            "menu": bounds_json(menu_bounds),
            "cursor_skips_disabled": true,
            "keyboard_escape_focus": true,
            "tab_closes_menu": true,
            "outside_click_focus": true,
            "pointer_open_and_escape": true,
            "resize": resize,
        }),
    )?;
    Ok(())
}

fn near(value: gpui::Pixels, expected: gpui::Pixels) -> bool {
    (f32::from(value) - f32::from(expected)).abs() <= 1.
}

fn bounds_json(bounds: gpui::Bounds<gpui::Pixels>) -> Value {
    json!([
        f32::from(bounds.left()),
        f32::from(bounds.top()),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    ])
}

fn active_row(tree: &Value) -> Option<&str> {
    let id = tree["active_descendant_focus"].as_str()?;
    tree["nodes"][id]["aria"]["label"].as_str()
}

fn key(raw: AnyWindowHandle, input: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let keystroke = Keystroke::parse(input)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}

fn tab_to(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let frame = tree(raw, cx)?;
    let stops = frame["frame"]["tab_stop_count"].as_u64().unwrap_or(64);
    for _ in 0..=stops {
        if focus(&tree(raw, cx)?, "gpui_focus")["element_id"] == id {
            return Ok(());
        }
        key(raw, "tab", cx)?;
    }
    Err(io::Error::other(format!("Клавиатурный фокус не дошёл до {id}.")).into())
}

fn tree(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let dump = raw
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.debug_a11y_tree_json()
        })?
        .ok_or_else(|| io::Error::other("Нет дерева доступности GPUI."))?;
    Ok(serde_json::from_str(&dump)?)
}

fn node<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    tree["nodes"]
        .as_object()?
        .values()
        .find(|node| node["element_id"] == id)
}

fn focus<'a>(tree: &'a Value, key: &str) -> &'a Value {
    &tree["nodes"][tree[key].as_str().unwrap_or_default()]
}

fn save(output: &Path, name: &str, value: &Value) -> gpui::Result<()> {
    Ok(fs::write(
        output.join(format!("{name}.json")),
        serde_json::to_vec_pretty(value)?,
    )?)
}
