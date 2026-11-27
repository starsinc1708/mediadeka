use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, Point, WindowHandle, point, px,
};
use mediadeka::{CaptureLayout, MainView};

use super::{Scene, draw, fail, native_dpi_change};

#[derive(Clone, Copy)]
enum Kind {
    PanelQueue,
    PanelTrack,
    PanelOverlayWide,
    PanelOverlayMinimum,
    PanelQueue150,
    Search,
    HistoryDesktop,
    HistoryMinimum,
    PinnedWide,
    PinnedCompact,
    PinnedDrag,
    StationDesktop,
    StationMinimum,
}

#[derive(Clone, Copy)]
struct ShellScene {
    kind: Kind,
    scene: Scene,
}

const SCENES: &[ShellScene] = &[
    ShellScene {
        kind: Kind::PanelQueue,
        scene: Scene {
            name: "panel-queue-desktop",
            width: 1500.,
            height: 850.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PanelTrack,
        scene: Scene {
            name: "panel-track-desktop",
            width: 1500.,
            height: 850.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PanelOverlayWide,
        scene: Scene {
            name: "panel-overlay-1040",
            width: 1040.,
            height: 700.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PanelOverlayMinimum,
        scene: Scene {
            name: "panel-overlay-minimum",
            width: 800.,
            height: 400.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PanelQueue150,
        scene: Scene {
            name: "panel-queue-150",
            width: 800.,
            height: 400.,
            dpi: 144,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::Search,
        scene: Scene {
            name: "search-desktop",
            width: 1920.,
            height: 1080.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::Search,
        scene: Scene {
            name: "search-minimum",
            width: 800.,
            height: 400.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::Search,
        scene: Scene {
            name: "search-150",
            width: 800.,
            height: 400.,
            dpi: 144,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::HistoryDesktop,
        scene: Scene {
            name: "history-desktop",
            width: 1920.,
            height: 1080.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::HistoryMinimum,
        scene: Scene {
            name: "history-minimum",
            width: 800.,
            height: 400.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PinnedWide,
        scene: Scene {
            name: "pinned-wide",
            width: 1500.,
            height: 850.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PinnedCompact,
        scene: Scene {
            name: "pinned-compact",
            width: 800.,
            height: 400.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::PinnedDrag,
        scene: Scene {
            name: "pinned-drag",
            width: 1500.,
            height: 850.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::StationDesktop,
        scene: Scene {
            name: "station-desktop",
            width: 1920.,
            height: 1080.,
            dpi: 96,
            reduced: true,
        },
    },
    ShellScene {
        kind: Kind::StationMinimum,
        scene: Scene {
            name: "station-minimum",
            width: 800.,
            height: 400.,
            dpi: 96,
            reduced: true,
        },
    },
];

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Укажите каталог кадров оболочки."))?;
    let (history_only, search_only) = match args.next() {
        None => (false, false),
        Some(flag) if flag == "--history-only" && args.next().is_none() => (true, false),
        Some(flag) if flag == "--search-only" && args.next().is_none() => (false, true),
        Some(_) => return Err(io::Error::other("Лишний аргумент кадров оболочки.").into()),
    };
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    let wav = profile.path().join("Awake.wav");
    write_silence(&wav)?;
    let id = mediadeka::seed_playlist_capture(profile.path(), &vec![wav; 240])?;
    // The capture process owns the isolated profile before GPUI starts its workers.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: 800.,
                height: 400.,
                reduced_motion: true,
            },
        )
        .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            let result: gpui::Result<()> = async {
                if history_only {
                    anchor.update(cx, |view, _, _| view.seed_history_for_capture(id))??;
                }
                for entry in SCENES {
                    if history_only
                        && !matches!(entry.kind, Kind::HistoryDesktop | Kind::HistoryMinimum)
                        || search_only && !matches!(entry.kind, Kind::Search)
                    {
                        continue;
                    }
                    capture(*entry, id, &output, history_only, cx).await?;
                }
                anchor.update(cx, |_, window, _| window.remove_window())?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                fail(error);
            }
        })
        .detach();
    });
    Ok(())
}

async fn capture(
    entry: ShellScene,
    id: i64,
    output: &Path,
    history_only: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let fluid_history =
        history_only && std::env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3");
    let mut scene = entry.scene;
    if fluid_history && matches!(entry.kind, Kind::HistoryDesktop) {
        scene.reduced = false;
    }
    let directory = output.join(scene.name);
    fs::create_dir_all(&directory)?;
    let window = cx.update(|cx| {
        mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: scene.width,
                height: scene.height,
                reduced_motion: scene.reduced,
            },
        )
    })?;
    let raw = AnyWindowHandle::from(window);
    delay(cx, 100).await;
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    delay(cx, 150).await;
    if history_only {
        window.update(cx, |view, window, cx| {
            view.start_home_skeleton_for_capture(window, cx)
        })?;
        capture_history(window, scene, &directory, fluid_history, cx).await?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    if matches!(entry.kind, Kind::Search) {
        capture_search(window, raw, scene, &directory, cx).await?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(id, window, cx);
        view.load_saved_state_for_capture(cx);
    })?;
    wait_playlist(window, cx).await?;
    // The first scene seeds the profile; later windows read the same rows.
    if matches!(entry.kind, Kind::PanelQueue) {
        window.update(cx, |view, _, cx| -> gpui::Result<()> {
            view.seed_history_for_capture(id)?;
            view.seed_pins_for_capture(id, cx);
            Ok(())
        })??;
    }
    wait_panel(window, cx).await?;
    delay(cx, 250).await;
    window.update(cx, |view, window, cx| {
        view.close_right_panel_for_capture(window, cx);
    })?;
    match entry.kind {
        Kind::PanelQueue => {
            window.update(cx, |view, _, cx| {
                view.play_playlist_track_for_capture(0, cx)
            })?;
            wait_playing(window, cx).await?;
            window.update(cx, |view, window, cx| {
                view.show_panel_for_capture("queue", window, cx)
            })?;
            draw(
                raw,
                scene,
                Some(&directory.join("panel-queue-desktop.png")),
                cx,
            )?;
        }
        Kind::PanelTrack => {
            window.update(cx, |view, window, cx| {
                view.select_playlist_track_for_capture(1, window, cx);
            })?;
            draw(
                raw,
                scene,
                Some(&directory.join("panel-track-desktop.png")),
                cx,
            )?;
        }
        Kind::PanelOverlayWide | Kind::PanelOverlayMinimum | Kind::PanelQueue150 => {
            window.update(cx, |view, _, cx| {
                view.play_playlist_track_for_capture(0, cx)
            })?;
            wait_playing(window, cx).await?;
            window.update(cx, |view, window, cx| {
                view.show_panel_for_capture("queue", window, cx)
            })?;
            draw(
                raw,
                scene,
                Some(&directory.join(format!("{}.png", scene.name))),
                cx,
            )?;
        }
        Kind::Search => unreachable!(),
        Kind::HistoryDesktop | Kind::HistoryMinimum => {
            capture_history(window, scene, &directory, false, cx).await?;
        }
        Kind::PinnedWide | Kind::PinnedCompact => {
            wait_pins(window, cx).await?;
            let order = window.update(cx, |view, _, _| view.pinned_order_for_capture())?;
            if order.len() != 2 {
                return Err(io::Error::other(format!(
                    "Закреплённое сцены {}: {order:?}",
                    scene.name
                ))
                .into());
            }
            draw(
                raw,
                scene,
                Some(&directory.join(format!("{}.png", scene.name))),
                cx,
            )?;
        }
        Kind::PinnedDrag => {
            drag_pins(window, raw, scene, &directory, cx).await?;
        }
        Kind::StationDesktop | Kind::StationMinimum => {
            capture_station(window, raw, scene, &directory, cx).await?;
        }
    }
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn capture_history(
    window: WindowHandle<MainView>,
    scene: Scene,
    directory: &Path,
    check_controls: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    if check_controls {
        super::sound::activate_accessibility(raw, cx)?;
    }
    window.update(cx, |view, window, cx| {
        view.open_history_for_capture(window, cx)
    })?;
    wait_history(window, cx).await?;
    draw(
        raw,
        scene,
        Some(&directory.join(format!("{}.png", scene.name))),
        cx,
    )?;
    let rows = window.update(cx, |view, _, _| view.history_rows_for_capture())?;
    if rows.len() < 4 || !rows.iter().any(|row| row.3 == "Поток Spotify") {
        return Err(io::Error::other(format!(
            "История сцены {} не показала записи: {rows:?}",
            scene.name
        ))
        .into());
    }
    if !check_controls {
        return Ok(());
    }
    let tree = super::sound::accessibility(raw, &directory.join("history-a11y.json"), cx)?;
    let nodes = tree["nodes"]
        .as_object()
        .ok_or_else(|| io::Error::other("GPUI не построил дерево истории."))?;
    if !nodes.values().any(|node| {
        node["aria"]["role"] == "Heading"
            && node["element_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("NamedInteger(\"history-day\", "))
    }) {
        return Err(io::Error::other("В истории отсутствует календарная группа.").into());
    }
    let replay = nodes
        .values()
        .find_map(|node| {
            if node["aria"]["role"] != "Button"
                || !node["aria"]["label"]
                    .as_str()?
                    .starts_with("Слушать снова · Третий")
            {
                return None;
            }
            node["element_id"]
                .as_str()?
                .strip_prefix("Name(\"")?
                .strip_suffix("\")")
        })
        .ok_or_else(|| io::Error::other("GPUI не показал команду повтора записи."))?;
    super::home::activate_control(raw, replay, cx)?;
    wait_playing(window, cx).await?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let position = loop {
        let state = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
        if state.1 == Some(0) && state.2 == "playing" && state.3 > 0.10 {
            break state.3;
        }
        if Instant::now() >= deadline {
            return Err(
                io::Error::other(format!("Позиция повтора не продвинулась: {state:?}")).into(),
            );
        }
        delay(cx, 35).await;
    };
    draw(raw, scene, Some(&directory.join("history-playing.png")), cx)?;
    fs::write(
        directory.join("history-replay.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "action": replay,
            "position_seconds": position,
        }))?,
    )?;
    Ok(())
}

async fn capture_search(
    window: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    directory: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    super::lyrics::start_guest(window, cx).await?;
    super::sound::activate_accessibility(raw, cx)?;
    draw(raw, scene, None, cx)?;
    super::home::activate_control(raw, "home-search-page", cx)?;
    search_key(raw, "ctrl-k", cx)?;
    if !window.update(cx, |view, window, cx| {
        view.home_search_focused_for_capture(window, cx)
    })? {
        return Err(io::Error::other("Ctrl+K не сфокусировал поле каталога.").into());
    }
    draw(raw, scene, Some(&directory.join("search.png")), cx)?;
    use super::sound::{accessibility, node, node_label};
    let tree = accessibility(raw, &directory.join("search-a11y.json"), cx)?;
    if node(&tree, "music-search-title").is_none() {
        return Err(io::Error::other("После перехода не найден заголовок поиска.").into());
    }
    if node_label(&tree, "home-catalog-search") != Some("Начать поиск музыки")
        || search_tabs(&tree).count() != 4
    {
        return Err(io::Error::other("В дереве поиска отсутствуют кнопка или категории.").into());
    }
    for key in ["a", "w", "a", "k", "e"] {
        search_key(raw, key, cx)?;
    }
    let draft = accessibility(raw, &directory.join("search-draft-a11y.json"), cx)?;
    if node(&draft, "music-search-prompt").is_none()
        || node(&draft, "home-search").is_none_or(|field| field["aria"]["value"] != "awake")
    {
        return Err(io::Error::other("Ввод черновика изменил подтверждённый поиск.").into());
    }
    search_key(raw, "enter", cx)?;
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let found = raw
            .update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.debug_a11y_tree_json()
            })?
            .ok_or_else(|| io::Error::other("Не построено дерево локального поиска."))?;
        if has_awake(&serde_json::from_str(&found)?) {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Запрос Awake не вернул локальный трек.").into());
        }
        delay(cx, 25).await;
    }
    draw(raw, scene, Some(&directory.join("search-local.png")), cx)?;
    accessibility(raw, &directory.join("search-local-a11y.json"), cx)?;
    for key in ["ctrl-k", "ctrl-a", "q", "z", "z", "q", "enter"] {
        search_key(raw, key, cx)?;
    }
    // TextInput emits Submit at the end of the GPUI update that handled Enter.
    draw(raw, scene, None, cx)?;
    let entered = accessibility(raw, &directory.join("search-enter-a11y.json"), cx)?;
    if node(&entered, "music-search-prompt").is_some()
        || node(&entered, "home-search").is_none_or(|field| field["aria"]["value"] != "qzzq")
        || has_awake(&entered)
    {
        return Err(io::Error::other("Enter не отправил запрос из поля.").into());
    }

    let album_id = r#"NamedInteger("catalog-search-kind", 1)"#;
    let stops = entered["frame"]["tab_stop_count"].as_u64().unwrap_or(64);
    let mut reached_album = false;
    for _ in 0..=stops {
        search_key(raw, "tab", cx)?;
        let frame: serde_json::Value = serde_json::from_str(
            &raw.update(cx, |_, window, _| window.debug_a11y_tree_json())?
                .ok_or_else(|| io::Error::other("Не построено дерево поиска после Tab."))?,
        )?;
        let focused = frame["gpui_focus"].as_str().unwrap_or_default();
        if frame["nodes"][focused]["element_id"] == album_id {
            reached_album = true;
            break;
        }
    }
    if !reached_album {
        return Err(io::Error::other("Tab не дошёл до категории альбомов.").into());
    }
    search_key(raw, "enter", cx)?;
    draw(raw, scene, None, cx)?;
    let switched = accessibility(raw, &directory.join("search-category-a11y.json"), cx)?;
    if !album_selected(&switched)
        || node(&switched, "home-search").is_none_or(|field| field["aria"]["value"] != "qzzq")
    {
        return Err(
            io::Error::other("Категория альбомов не выбралась с сохранением запроса.").into(),
        );
    }

    for key in ["ctrl-k", "ctrl-a", "q", "z", "z", "f"] {
        search_key(raw, key, cx)?;
    }
    super::home::activate_control(raw, "home-catalog-search", cx)?;
    draw(raw, scene, None, cx)?;
    super::home::activate_control(raw, "workspace-back", cx)?;
    draw(raw, scene, None, cx)?;
    let returned = accessibility(raw, &directory.join("search-restored-a11y.json"), cx)?;
    // If the magnifier failed to submit, Back would restore the default
    // Track category instead of the album search that preceded the new query.
    if !album_selected(&returned)
        || node(&returned, "home-search").is_none_or(|field| field["aria"]["value"] != "qzzq")
    {
        return Err(io::Error::other(
            "После отправки лупой кнопка «Назад» не вернула поиск альбомов.",
        )
        .into());
    }
    println!(
        "{}: локальный трек, Enter, лупа, Tab, категория и возврат в дереве поиска.",
        scene.name
    );
    Ok(())
}

fn has_awake(tree: &serde_json::Value) -> bool {
    tree["nodes"].as_object().is_some_and(|nodes| {
        nodes.values().any(|node| {
            node["element_id"] == r#"NamedInteger("music-search-saved", 0)"#
                && node["aria"]["label"]
                    .as_str()
                    .is_some_and(|label| label.contains("Awake"))
        })
    })
}

fn search_key(raw: AnyWindowHandle, key: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let keystroke = Keystroke::parse(key)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}

fn search_tabs(tree: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    tree["nodes"]
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.values())
        .filter(|node| {
            node["aria"]["role"] == "Tab"
                && node["element_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with(r#"NamedInteger("catalog-search-kind", "#))
        })
}

fn album_selected(tree: &serde_json::Value) -> bool {
    search_tabs(tree)
        .any(|tab| tab["aria"]["selected"] == true && tab["aria"]["label"] == "Альбомы и синглы")
}

async fn capture_station(
    window: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    directory: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    window.update(cx, |view, window, cx| {
        view.show_station_for_capture(window, cx)
    })?;
    let ready = window.update(cx, |view, _, _| view.station_semantics_for_capture())?;
    if ready["title"] != "Моя волна" || ready["pin_available"] != true {
        return Err(io::Error::other(format!("Станция не открылась: {ready}")).into());
    }
    draw(raw, scene, Some(&directory.join("station-ready.png")), cx)?;
    window.update(cx, |view, window, cx| {
        view.play_station_for_capture(window, cx)
    })?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let failed = loop {
        let state = window.update(cx, |view, _, _| view.station_semantics_for_capture())?;
        if state["error"]
            .as_str()
            .is_some_and(|error| error.contains("Подключите Яндекс Музыку"))
        {
            break state;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Станция не сообщила об отсутствии профиля: {state}"
            ))
            .into());
        }
        delay(cx, 40).await;
    };
    draw(
        raw,
        scene,
        Some(&directory.join("station-no-account.png")),
        cx,
    )?;
    fs::write(
        directory.join("semantics.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "ready": ready,
            "no_account": failed,
        }))?,
    )?;
    Ok(())
}

async fn drag_pins(
    window: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    directory: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait_pins(window, cx).await?;
    draw(raw, scene, None, cx)?;
    let first = window
        .update(cx, |view, _, _| view.pinned_bounds_for_capture(0))?
        .ok_or_else(|| io::Error::other("Нет первого пункта закреплённого."))?;
    let second = window
        .update(cx, |view, _, _| view.pinned_bounds_for_capture(1))?
        .ok_or_else(|| io::Error::other("Нет второго пункта закреплённого."))?;
    let order = window.update(cx, |view, _, _| view.pinned_order_for_capture())?;
    let held = Some(MouseButton::Left);
    let start = first.center();
    let moved = point(start.x, start.y + px(14.));
    pointer(raw, [mouse_down(start), mouse_move(moved, held)], cx)?;
    draw(raw, scene, Some(&directory.join("pinned-drag.png")), cx)?;
    let target = second.center();
    pointer(raw, [mouse_move(target, held), mouse_up(target)], cx)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let saved =
            window.update(cx, |view, _, _| view.pinned_order_from_store_for_capture())??;
        if saved.len() == 2 && saved != order {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Перетаскивание не изменило порядок в базе: {order:?}"
            ))
            .into());
        }
        delay(cx, 40).await;
    }
    draw(raw, scene, Some(&directory.join("pinned-dropped.png")), cx)?;
    Ok(())
}

async fn wait_playing(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
        if state.2 == "playing" {
            return Ok(());
        }
        if Instant::now() >= deadline || state.2.starts_with("failed") {
            return Err(io::Error::other(format!("Нет подтверждения звука: {state:?}")).into());
        }
        delay(cx, 40).await;
    }
}

async fn wait_panel(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if !window.update(cx, |view, _, _| view.panel_load_pending_for_capture())? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Состояние панели не прочиталось.").into());
        }
        delay(cx, 35).await;
    }
}

async fn wait_pins(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let order = window.update(cx, |view, _, _| view.pinned_order_for_capture())?;
        if order.len() == 2 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("Закреплённое не прочиталось: {order:?}")).into());
        }
        delay(cx, 35).await;
    }
}

async fn wait_playlist(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(result) = window.update(cx, |view, _, _| view.playlist_status_for_capture())? {
            return result
                .map(|_| ())
                .map_err(|error| io::Error::other(error).into());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Локальный плейлист не загрузился.").into());
        }
        delay(cx, 35).await;
    }
}

async fn wait_history(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if window.update(cx, |view, _, _| view.history_ready_for_capture())? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("История не прочиталась.").into());
        }
        delay(cx, 35).await;
    }
}

fn pointer(
    raw: AnyWindowHandle,
    inputs: impl IntoIterator<Item = PlatformInput>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        for input in inputs {
            window.dispatch_event(input, cx);
        }
    })
}

fn mouse_down(position: Point<gpui::Pixels>) -> PlatformInput {
    MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }
    .to_platform_input()
}

fn mouse_move(position: Point<gpui::Pixels>, pressed_button: Option<MouseButton>) -> PlatformInput {
    MouseMoveEvent {
        position,
        pressed_button,
        ..Default::default()
    }
    .to_platform_input()
}

fn mouse_up(position: Point<gpui::Pixels>) -> PlatformInput {
    MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }
    .to_platform_input()
}

async fn delay(cx: &AsyncApp, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}

/// A short mono PCM WAV keeps the seeded playlist readable without another fixture.
pub(super) fn write_silence(path: &Path) -> io::Result<()> {
    let samples = 80_000u32;
    let data_len = samples * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8_000u32.to_le_bytes());
    bytes.extend_from_slice(&16_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(44 + data_len as usize, 0);
    fs::write(path, bytes)
}
