use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle};
use mediadeka::{CaptureLayout, MainView};
use serde_json::Value;

use super::{Scene, capture_high_contrast, draw, fail, native_dpi_change};

#[path = "music/mixed.rs"]
mod mixed;

const ALBUM: (&str, &str) = ("album", "https://music.yandex.ru/album/43177750");
const ARTIST: (&str, &str) = ("artist", "https://music.yandex.ru/artist/7245399");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    Playlist,
    Artwork,
    MixedCatalog,
}

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
    let profile = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
    let mode = arguments.next();
    if mode.as_deref().is_some_and(|arg| arg == "track-release") {
        return super::studio::run_known_release(output, profile, arguments);
    }
    let (width, height, offline, scope) = match mode.as_deref().and_then(|arg| arg.to_str()) {
        None => (1672., 941., false, Scope::All),
        Some("offline") => (1672., 941., true, Scope::All),
        Some("desktop") => (1920., 1080., true, Scope::All),
        Some("compact") => (800., 400., true, Scope::All),
        Some("playlist-1920") => (1920., 1080., true, Scope::Playlist),
        Some("playlist-800") => (800., 400., true, Scope::Playlist),
        Some("artwork-1920") => (1920., 1080., true, Scope::Artwork),
        Some("artwork-800") => (800., 400., true, Scope::Artwork),
        Some("mixed-catalog") => (1920., 1080., false, Scope::MixedCatalog),
        _ => return Err(usage().into()),
    };
    let high_contrast = capture_high_contrast(arguments)?;
    if !profile.join("mediadeka.sqlite3").is_file() {
        return Err(usage().into());
    }
    fs::create_dir_all(&output)?;
    // The capture process uses the specified isolated profile before GPUI starts workers.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", &profile) };
    mediadeka::application().run(move |cx| {
        let scene = Scene {
            name: "music-pages",
            width,
            height,
            dpi: 96,
            reduced: true,
        };
        let window = mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: scene.width,
                height: scene.height,
                reduced_motion: scene.reduced,
            },
        )
        .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            capture(window, scene, &output, offline, scope, high_contrast, cx)
                .await
                .unwrap_or_else(|error| fail(error));
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap_or_else(|error| fail(error));
        })
        .detach();
    });
    Ok(())
}

fn usage() -> io::Error {
    io::Error::other(
        "--music-pages <каталог кадров> <изолированный профиль> [offline|desktop|compact|playlist-1920|playlist-800|artwork-1920|artwork-800|mixed-catalog] [--high-contrast]",
    )
}

async fn capture(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    offline: bool,
    scope: Scope,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    if scope != Scope::All {
        super::sound::activate_accessibility(raw, cx)?;
    }
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    window.update(cx, |view, window, cx| {
        if high_contrast {
            view.set_fluid_high_contrast_for_capture(true, cx);
        }
        view.start_home_for_capture(scope != Scope::MixedCatalog, window, cx)
    })?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !window.update(cx, |view, _, _| view.home_status_for_capture().is_some())? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Не открылся сохранённый профиль.").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    match scope {
        Scope::MixedCatalog => return mixed::verify(window, scene, output, cx).await,
        Scope::Artwork => {
            return capture_objects(window, scene, output, offline, &[ARTIST, ALBUM], true, cx)
                .await;
        }
        Scope::All => capture_library(window, scene, output, offline, cx).await?,
        Scope::Playlist => {}
    }
    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(
            "https://open.spotify.com/playlist/7q3kJIttHRQMCXZ0BwTRXr",
            window,
            cx,
        )
    })?;
    capture_playlist(window, scene, output, scope == Scope::Playlist, cx).await
}

async fn capture_library(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    offline: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    capture_objects(
        window,
        scene,
        output,
        offline,
        &[
            (
                "track",
                "https://music.yandex.ru/album/43177750/track/153919042",
            ),
            ALBUM,
            ARTIST,
            ("single", "https://music.yandex.ru/album/33992620"),
        ],
        false,
        cx,
    )
    .await?;
    for (name, playlists) in [("liked", false), ("playlists", true)] {
        window.update(cx, |view, window, cx| {
            view.show_music_collection_for_capture(playlists, window, cx)
        })?;
        record(window, scene, output, name, offline, false, cx).await?;
    }
    Ok(())
}

async fn capture_objects(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    offline: bool,
    objects: &[(&str, &str)],
    check_accessibility: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    for &(name, url) in objects {
        window.update(cx, |view, window, cx| {
            view.submit_music_url_for_capture(url, window, cx)
        })?;
        let snapshot = record(
            window,
            scene,
            output,
            name,
            offline,
            check_accessibility,
            cx,
        )
        .await?;
        if check_accessibility {
            check_object_accessibility(window.into(), output, name, &snapshot, cx)?;
        }
    }
    Ok(())
}

async fn capture_playlist(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    review_menu: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let deadline = Instant::now() + Duration::from_secs(65);
    let count = loop {
        match window.update(cx, |view, _, _| view.playlist_status_for_capture())? {
            Some(Ok(count)) if count > 0 => break count,
            Some(other) => return Err(io::Error::other(format!("Плейлист: {other:?}")).into()),
            None if Instant::now() < deadline => {
                draw(raw, scene, None, cx)?;
                cx.background_executor()
                    .timer(Duration::from_millis(110))
                    .await;
            }
            None => return Err(io::Error::other("Плейлист не загрузился вовремя.").into()),
        }
    };
    draw(raw, scene, None, cx)?;
    cx.background_executor()
        .timer(Duration::from_millis(600))
        .await;
    draw(raw, scene, Some(&output.join("playlist.png")), cx)?;
    if review_menu && std::env::var_os("MEDIADEKA_GUI").is_some_and(|gui| gui == "3") {
        let tree = super::sound::accessibility(raw, &output.join("playlist-hero-a11y.json"), cx)?;
        visit_controls(
            raw,
            output,
            "playlist",
            "playlist-play",
            &["playlist-play"],
            &tree,
            cx,
        )?;
    }
    let index = 2.min(count - 1);
    window.update(cx, |view, window, cx| {
        view.select_playlist_track_for_capture(index, window, cx)
    })?;
    let title = window.update(cx, |view, _, _| {
        view.playlist_selection_for_capture().map(str::to_owned)
    })?;
    draw(raw, scene, None, cx)?;
    cx.background_executor()
        .timer(Duration::from_millis(600))
        .await;
    draw(raw, scene, Some(&output.join("playlist-selected.png")), cx)?;
    if review_menu || scene.width == 800. {
        capture_playlist_menu(window, scene, output, index, review_menu, cx).await?;
    }
    let snapshot = serde_json::json!({ "page": "playlist", "tracks": count, "selected": title });
    fs::write(
        output.join("playlist.json"),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    println!("playlist {snapshot}");
    Ok(())
}

async fn capture_playlist_menu(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    index: usize,
    verify: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    if scene.width == 800. {
        window.update(cx, |view, _, cx| {
            view.playlist_scroll_for_capture(index, cx)
        })?;
        draw(raw, scene, None, cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(600))
            .await;
    }
    window.update(cx, |view, window, cx| {
        view.show_playlist_menu_for_capture(index, window, cx)
    })?;
    draw(
        raw,
        scene,
        Some(&output.join("playlist-track-menu.png")),
        cx,
    )?;
    if verify {
        let snapshot =
            super::sound::accessibility(raw, &output.join("playlist-track-menu.json"), cx)?;
        let actions = snapshot["nodes"]
            .as_object()
            .into_iter()
            .flat_map(|nodes| nodes.values())
            .filter(|node| node["aria"]["role"] == "MenuItem")
            .filter_map(|node| node["aria"]["label"].as_str())
            .collect::<Vec<_>>();
        for label in [
            "Слушать",
            "Играть следующим",
            "В конец очереди",
            "Сведения",
            "Открыть страницу трека",
            "Найти артиста",
            "Найти альбом",
        ] {
            if !actions.contains(&label) {
                return Err(
                    io::Error::other(format!("В меню трека нет команды «{label}».")).into(),
                );
            }
        }
        println!("playlist menu: {} accessible actions", actions.len());
    }
    Ok(())
}

async fn record(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    name: &str,
    offline: bool,
    check_palette: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let raw = AnyWindowHandle::from(window);
    let snapshot = loop {
        let snapshot = window.update(cx, |view, _, _| view.music_semantics_for_capture())?;
        let ready = snapshot["details"]["title"].is_string()
            || snapshot["collections"]
                .as_object()
                .is_some_and(|sources| !sources.is_empty());
        if ready && (offline || snapshot["pending"] == Value::Bool(false)) {
            break snapshot;
        }
        if snapshot["pending"] == Value::Bool(false) || Instant::now() >= deadline {
            return Err(io::Error::other(format!("{name}: {snapshot}")).into());
        }
        draw(raw, scene, None, cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(110))
            .await;
    };
    if !offline || matches!(name, "album" | "artist") {
        draw(raw, scene, None, cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(650))
            .await;
    }
    if check_palette
        && std::env::var_os("MEDIADEKA_GLASS_THEME").is_none()
        && window.update(cx, |view, window, cx| {
            view.fluid_glass_quality_for_capture(window, cx).is_some()
        })?
    {
        let palette = wait_for_page_palette(window, cx).await?;
        fs::write(
            output.join(format!("{name}-page-palette.json")),
            serde_json::to_vec_pretty(&palette)?,
        )?;
        println!("{name}: палитра изображения {palette:?}");
    }
    draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
    fs::write(
        output.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    println!("{name} {snapshot}");
    Ok(snapshot)
}

async fn wait_for_page_palette(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<[[f32; 3]; 4]> {
    let deadline = Instant::now() + Duration::from_secs(7);
    let raw = AnyWindowHandle::from(window);
    loop {
        raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        if let Some(palette) = window.update(cx, |view, _, cx| {
            view.fluid_glass_page_palette_for_capture(cx)
        })? {
            return Ok(palette);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Не готова палитра обложки страницы Fluid Glass.").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
}

fn check_object_accessibility(
    raw: AnyWindowHandle,
    output: &Path,
    kind: &str,
    snapshot: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let title = snapshot["details"]["title"]
        .as_str()
        .filter(|_| snapshot["details"]["kind"] == kind)
        .ok_or_else(|| io::Error::other(format!("Не загрузилась страница {kind}: {snapshot}")))?;
    use super::sound::{accessibility, node};
    let tree = accessibility(raw, &output.join(format!("{kind}-a11y.json")), cx)?;
    let heading = node(&tree, "music-object-title");
    if !heading
        .is_some_and(|node| node["aria"]["role"] == "Heading" && node["aria"]["label"] == title)
    {
        return Err(io::Error::other(format!("В дереве GPUI нет заголовка «{title}».")).into());
    }
    let actions = [
        ("music-object-play", "Слушать"),
        ("music-object-shuffle", "Перемешать и слушать"),
        ("music-object-enqueue", "Добавить в очередь"),
        ("pin-toggle", "Закрепить"),
    ];
    for (id, label) in actions {
        if !node(&tree, id).is_some_and(|node| {
            let aria = &node["aria"];
            aria["role"] == "Button"
                && aria["disabled"] != true
                && (aria["label"] == label || (id == "pin-toggle" && aria["label"] == "Открепить"))
        }) {
            return Err(io::Error::other(format!(
                "На странице {kind} недоступно действие «{label}»."
            ))
            .into());
        }
    }
    let ids = actions.map(|(id, _)| id);
    let visited = visit_controls(raw, output, kind, "music-object-play", &ids, &tree, cx)?;
    fs::write(
        output.join(format!("{kind}-keyboard.json")),
        serde_json::to_vec_pretty(&serde_json::json!({ "title": title, "tab_reached": visited }))?,
    )?;
    println!("{kind}: доступны {} действий через Tab", visited.len());
    Ok(())
}

fn visit_controls<'a>(
    raw: AnyWindowHandle,
    output: &Path,
    kind: &str,
    play_id: &str,
    ids: &[&'a str],
    tree: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<Vec<&'a str>> {
    let elements: Vec<_> = ids
        .iter()
        .map(|&id| (id, format!("Name({id:?})")))
        .collect();
    let stops = tree["frame"]["tab_stop_count"].as_u64().unwrap_or(64);
    let tab = Keystroke::parse("tab")?;
    let fluid = std::env::var_os("MEDIADEKA_GUI").is_some_and(|gui| gui == "3");
    let visited = raw.update(cx, |_, window, cx| -> gpui::Result<Vec<&str>> {
        let original_focus = window.focused(cx);
        let rest = fluid.then(|| window.render_to_image()).transpose()?;
        let mut visited = Vec::new();
        for _ in 0..=stops {
            let keystroke = tab.clone();
            window.dispatch_keystroke(keystroke.clone(), cx);
            window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
            window.draw(cx).clear(cx);
            let frame: Value =
                serde_json::from_str(&window.debug_a11y_tree_json().ok_or_else(|| {
                    io::Error::other(format!("Пропало дерево доступности страницы {kind}."))
                })?)?;
            let focus = frame["gpui_focus"].as_str().unwrap_or_default();
            if let Some((id, _)) = elements
                .iter()
                .find(|(_, element)| frame["nodes"][focus]["element_id"] == element.as_str())
                && !visited.contains(id)
            {
                if *id == play_id {
                    let focused = window.render_to_image()?;
                    focused.save(output.join(format!("{kind}-play-focus.png")))?;
                    if let Some(rest) = &rest {
                        let span = focus_outline_span(rest, &focused);
                        if span < 48 {
                            return Err(io::Error::other(format!(
                                "Фокус «Слушать» на странице {kind} виден только на {span} соседних пикселях."
                            )).into());
                        }
                        println!("{kind}: keyboard outline measured {span} consecutive pixels");
                    }
                }
                visited.push(*id);
                if visited.len() == ids.len() {
                    break;
                }
            }
        }
        if let Some(focus) = original_focus {
            window.focus(&focus, cx);
        } else {
            window.blur(cx);
        }
        Ok(visited)
    })??;
    if visited.len() != ids.len() {
        return Err(
            io::Error::other(format!("Tab на странице {kind} достиг только {visited:?}.")).into(),
        );
    }
    Ok(visited)
}

fn focus_outline_span(rest: &image::RgbaImage, focused: &image::RgbaImage) -> usize {
    let mut longest = 0;
    let width = rest.width() as usize * 4;
    for (idle_row, focused_row) in rest.chunks_exact(width).zip(focused.chunks_exact(width)) {
        let mut span = 0;
        for (idle, active) in idle_row
            .as_chunks::<4>()
            .0
            .iter()
            .zip(focused_row.as_chunks::<4>().0)
        {
            let outline = matches!(active, [242, 206, 227, 255] | [255, 255, 255, 255]);
            if outline && idle != active {
                span += 1;
                longest = longest.max(span);
            } else {
                span = 0;
            }
        }
    }
    longest
}
