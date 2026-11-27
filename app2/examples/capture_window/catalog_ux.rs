use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke};
use mediadeka::CaptureLayout;
use serde_json::Value;

use super::{
    Scene, draw, fail,
    home::activate_control,
    native_dpi_change,
    sound::{accessibility, activate_accessibility, node, node_label},
};

const SCENES: [Scene; 3] = [
    Scene {
        name: "1920x1080",
        width: 1920.,
        height: 1080.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "800x400",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "800x400-150pct",
        width: 800.,
        height: 400.,
        dpi: 144,
        reduced: true,
    },
];

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = args
        .next()
        .map(PathBuf::from)
        .filter(|_| args.next().is_none())
        .ok_or_else(|| io::Error::other("--catalog-ux <каталог кадров>"))?;
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    // This process has no signed-in account and its unavailable network is deterministic.
    unsafe {
        env::set_var("MEDIADEKA_PROFILE_DIR", profile.path());
        env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");
        env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
    }
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[1]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                capture(scene, &output.join(scene.name), cx)
                    .await
                    .unwrap_or_else(|error| fail(error));
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

fn layout(scene: Scene) -> CaptureLayout {
    CaptureLayout {
        width: scene.width,
        height: scene.height,
        reduced_motion: scene.reduced,
    }
}

async fn capture(scene: Scene, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.start_home_for_capture(false, window, cx)
    })?;
    wait(cx, |cx| {
        view.update(cx, |view, _, _| view.home_status_for_capture().is_some())
    })
    .await?;
    wait_node(raw, "home-page", cx).await?;
    activate(raw, "home-page", cx)?;
    if let Err(error) = wait_node(raw, "home-source-toggle", cx).await {
        frame(raw, scene, output, "guest-start-failure", cx)?;
        return Err(error);
    }

    activate(raw, "home-source-toggle", cx)?;
    let sources = frame(raw, scene, output, "guest-sources", cx)?;
    require_label(&sources, "home-source-guest", "Гостевая витрина")?;
    if node(&sources, "home-liked").is_some() {
        require_label(&sources, "workspace-search-scope", "Медиатека и ссылки")?;
    }
    require_label(&sources, "home-search", "Поиск музыки")?;

    submit(raw, "beatles", cx)?;
    wait_node(raw, "music-search-empty", cx).await?;
    let search = frame(raw, scene, output, "guest-search", cx)?;
    require_label(
        &search,
        "music-search-empty",
        "В медиатеке ничего не найдено",
    )?;
    require_label(&search, "music-search-connect", "Подключить площадку")?;
    activate(raw, "music-search-connect", cx)?;
    require_node(&snapshot(raw, cx)?, "settings-page")?;
    activate(raw, "workspace-back", cx)?;
    wait_node(raw, "music-search-empty", cx).await?;
    let restored = frame(raw, scene, output, "guest-search-return", cx)?;
    if node(&restored, "home-search").and_then(|field| field["aria"]["value"].as_str())
        != Some("beatles")
    {
        return Err(io::Error::other("Возврат из подключений потерял поисковый запрос.").into());
    }

    let sidebar = snapshot(raw, cx)?;
    let library = if node(&sidebar, "studio-library").is_some() {
        verify_recent_queries(raw, scene, output, cx).await?;
        "studio-library"
    } else {
        "home-liked"
    };
    activate(raw, library, cx)?;
    wait_node(raw, "liked-empty", cx).await?;
    let tracks = frame(raw, scene, output, "library-tracks", cx)?;
    require_label(&tracks, "music-liked-title", "Медиатека")?;
    check_empty_library(&tracks)?;
    for tab in ["albums", "artists"] {
        let sidebar_tab = format!("studio-library-{tab}");
        let target = if node(&tracks, &sidebar_tab).is_some() {
            sidebar_tab
        } else {
            format!("library-tab-{tab}")
        };
        activate(raw, &target, cx)?;
        wait_node(raw, "library-empty", cx).await?;
        let tree = frame(raw, scene, output, &format!("library-{tab}"), cx)?;
        check_empty_library(&tree)?;
    }
    activate(raw, "library-empty-import", cx)?;
    require_node(
        &frame(raw, scene, output, "library-import", cx)?,
        "playlist-import",
    )?;

    view.update(cx, |view, window, cx| {
        view.show_station_for_capture(window, cx)
    })?;
    let station = frame(raw, scene, output, "station-connect", cx)?;
    require_label(&station, "station-connect", "Подключить Яндекс Музыку")?;
    activate(raw, "station-connect", cx)?;
    wait_node(raw, "settings-login-modal", cx).await?;
    let login = frame(raw, scene, output, "station-provider", cx)?;
    require_label(&login, "login-panel", "Яндекс Музыка")?;
    key(raw, "escape", cx)?;
    activate(raw, "workspace-back", cx)?;
    let returned = frame(raw, scene, output, "station-return", cx)?;
    require_label(&returned, "station-title", "Моя волна")?;
    require_node(&returned, "station-connect")?;

    submit(
        raw,
        "https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3",
        cx,
    )?;
    wait_node(raw, "music-object-connect", cx).await?;
    let object = frame(raw, scene, output, "object-connect", cx)?;
    require_label(&object, "music-object-connect", "Подключить Spotify")?;
    require_absent(&object, "music-object-retry")?;
    submit(
        raw,
        "https://open.spotify.com/playlist/7q3kJIttHRQMCXZ0BwTRXr",
        cx,
    )?;
    wait_node(raw, "playlist-connect", cx).await?;
    let playlist = frame(raw, scene, output, "playlist-connect", cx)?;
    require_label(&playlist, "playlist-connect", "Подключить Spotify")?;
    require_absent(&playlist, "playlist-retry")?;
    require_absent(&playlist, "playlist-play")?;

    println!(
        "{}: guest search scope, empty library, station recovery and public links passed",
        scene.name
    );
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn check_empty_library(tree: &Value) -> gpui::Result<()> {
    require_node(tree, "library-empty-connect")?;
    require_node(tree, "library-empty-import")?;
    require_absent(tree, "library-filters")?;
    require_absent(tree, "music-filter")
}

async fn verify_recent_queries(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    submit(raw, "beethoven", cx)?;
    wait_node(raw, "music-search-empty", cx).await?;
    activate(raw, "studio-library", cx)?;
    enter_query(raw, "be", cx)?;
    key(raw, "down", cx)?;
    key(raw, "down", cx)?;
    let suggestions = frame(raw, scene, output, "recent-searches", cx)?;
    require_node(&suggestions, "workspace-search-suggestions")?;
    let selected = suggestions["nodes"].as_object().is_some_and(|nodes| {
        nodes.values().any(|entry| {
            entry["aria"]["role"] == "ListBoxOption"
                && entry["aria"]["label"] == "beatles"
                && entry["aria"]["selected"] == true
        })
    });
    if !selected {
        return Err(io::Error::other("Стрелки не выбрали предыдущий запрос.").into());
    }
    key(raw, "enter", cx)?;
    wait_node(raw, "music-search-empty", cx).await?;
    let searched = snapshot(raw, cx)?;
    require_absent(&searched, "workspace-search-suggestions")?;
    if node(&searched, "home-search").and_then(|field| field["aria"]["value"].as_str())
        != Some("beatles")
    {
        return Err(io::Error::other("Enter не запустил выбранный запрос.").into());
    }
    Ok(())
}

fn frame(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
    accessibility(raw, &output.join(format!("{name}-a11y.json")), cx)
}

fn snapshot(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let data = raw
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.debug_a11y_tree_json()
        })?
        .ok_or_else(|| io::Error::other("GPUI не построил дерево доступности."))?;
    Ok(serde_json::from_str(&data)?)
}

async fn wait_node(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    wait(cx, |cx| Ok(node(&snapshot(raw, cx)?, id).is_some()))
        .await
        .map_err(|error| io::Error::other(format!("Ожидался {id}. {error}")).into())
}

async fn wait(
    cx: &mut AsyncApp,
    ready: impl Fn(&mut AsyncApp) -> gpui::Result<bool>,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready(cx)? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Ожидание состояния превысило 15 секунд.").into());
        }
        timer.timer(Duration::from_millis(50)).await;
    }
    Ok(())
}

fn submit(raw: AnyWindowHandle, value: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    enter_query(raw, value, cx)?;
    key(raw, "enter", cx)
}

fn enter_query(raw: AnyWindowHandle, value: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    key(raw, "ctrl-k", cx)?;
    key(raw, "ctrl-a", cx)?;
    for character in value.chars() {
        let character = character.to_string();
        dispatch(
            raw,
            Keystroke {
                key: character.clone(),
                key_char: Some(character),
                ..Default::default()
            },
            cx,
        )?;
    }
    Ok(())
}

fn activate(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    activate_control(raw, id, cx)?;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))
}

fn key(raw: AnyWindowHandle, key: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    dispatch(raw, Keystroke::parse(key)?, cx)
}

fn dispatch(raw: AnyWindowHandle, key: Keystroke, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}

fn require_node(tree: &Value, id: &str) -> gpui::Result<()> {
    node(tree, id)
        .map(|_| ())
        .ok_or_else(|| io::Error::other(format!("Отсутствует {id}.")).into())
}

fn require_absent(tree: &Value, id: &str) -> gpui::Result<()> {
    if node(tree, id).is_none() {
        Ok(())
    } else {
        Err(io::Error::other(format!("Лишнее действие {id}.")).into())
    }
}

fn require_label(tree: &Value, id: &str, text: &str) -> gpui::Result<()> {
    if node_label(tree, id).is_some_and(|label| label.contains(text)) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{id} не содержит подпись {text:?}. Получено {:?}.",
            node_label(tree, id)
        ))
        .into())
    }
}
