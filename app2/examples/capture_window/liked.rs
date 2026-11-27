#[path = "liked/filters.rs"]
mod filters;

use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use mediadeka::{CaptureLayout, MainView};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use super::{Scene, draw, fail, native_dpi_change, sound::activate_accessibility};

const SCENES: [Scene; 2] = [
    Scene {
        name: "minimum-100",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "minimum-150",
        width: 800.,
        height: 400.,
        dpi: 144,
        reduced: true,
    },
];

pub(super) fn run_stage_four(mut arguments: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let usage = || io::Error::other("--library-stage4 <каталог кадров> [профиль Spotify] [wide]");
    let output = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
    let profile_or_size = arguments.next();
    let (spotify_profile, wide) = if profile_or_size
        .as_deref()
        .is_some_and(|value| value == "wide")
    {
        (None, true)
    } else {
        let wide = match arguments.next().as_deref() {
            Some(value) if value == "wide" => true,
            Some(_) => return Err(usage().into()),
            None => false,
        };
        (profile_or_size.map(PathBuf::from), wide)
    };
    if spotify_profile
        .as_ref()
        .is_some_and(|path| !path.join("mediadeka.sqlite3").is_file())
    {
        return Err(io::Error::other("Профиль Spotify для кадров не найден.").into());
    }
    if arguments.next().is_some() {
        return Err(io::Error::other("Лишний аргумент проверки медиатеки.").into());
    }
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    let sound = profile.path().join("local.wav");
    super::shell::write_silence(&sound)?;
    // This capture process owns the temporary profile.
    unsafe {
        env::set_var("MEDIADEKA_PROFILE_DIR", profile.path());
    }
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            let scenes = [
                Scene {
                    name: if wide { "1920x1080" } else { "1500x850" },
                    width: if wide { 1920. } else { 1500. },
                    height: if wide { 1080. } else { 850. },
                    ..SCENES[0]
                },
                Scene {
                    name: "800x400",
                    ..SCENES[0]
                },
            ];
            for scene in scenes {
                if let Err(error) =
                    capture_library(scene, &output, &sound, spotify_profile.as_deref(), cx).await
                {
                    fail(error);
                }
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn capture_library(
    scene: Scene,
    output: &Path,
    sound: &Path,
    spotify_profile: Option<&Path>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let directory = output.join(scene.name);
    fs::create_dir_all(&directory)?;
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.start_home_skeleton_for_capture(window, cx);
        view.sample_library_for_capture(sound, window, cx);
    })?;
    let all = frame(view, scene, &directory, "tracks", cx)?;
    if all["tracks"] != 13
        || all["downloaded"] != 2
        || all["counts"]["spotify"] != 4
        || all["counts"]["youtube-music"] != 4
        || all["counts"]["jellyfin"] != 4
        || all["counts"]["local"] != 2
    {
        return Err(io::Error::other(format!("Счётчики не согласованы: {all}")).into());
    }
    let evidence = filters::check(view, scene, &directory, all, cx)?;
    fs::write(
        directory.join("semantics.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!(
        "{}: unified tracks, 4 source counts, keyboard filters, Escape, history and reset",
        scene.name
    );
    if let Some(profile) = spotify_profile {
        capture_spotify_albums(view, scene, &directory, profile, cx).await?;
    }
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn capture_spotify_albums(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    view.update(cx, |view, window, cx| {
        view.show_cached_spotify_albums_for_capture(profile, window, cx)
    })?
    .map_err(io::Error::other)?;
    draw(view.into(), scene, None, cx)?;
    cx.background_executor().timer(Duration::from_secs(2)).await;
    let tiles = frame(view, scene, output, "spotify-albums-tiles", cx)?;
    if tiles["album_view"] != "tiles"
        || tiles["columns"].as_u64().unwrap_or_default() < 2
        || tiles["counts"]["spotify"].as_u64().unwrap_or_default() == 0
    {
        return Err(io::Error::other("Сетка альбомов Spotify не заполнена.").into());
    }
    let compact = scene.width < 1000.;
    if compact {
        view.update(cx, |view, _, cx| {
            view.reveal_album_rows_for_capture(true, cx)
        })?;
        let scrolled = frame(view, scene, output, "spotify-albums-tiles-scrolled", cx)?;
        if scrolled["first_item"] != 1 {
            return Err(io::Error::other("Не удалось прокрутить к плиткам альбомов.").into());
        }
    }
    view.update(cx, |view, _, cx| view.toggle_album_view_for_capture(cx))?;
    if compact {
        let scrolled = frame(view, scene, output, "spotify-albums-list-scrolled", cx)?;
        if scrolled["first_item"] != 1 {
            return Err(io::Error::other("Таблица не сохранила позицию альбома.").into());
        }
        view.update(cx, |view, _, cx| {
            view.reveal_album_rows_for_capture(false, cx)
        })?;
    }
    let list = frame(view, scene, output, "spotify-albums-list", cx)?;
    if list["album_view"] != "list" || list["columns"] != 1 || list["counts"] != tiles["counts"] {
        return Err(io::Error::other("При переключении вида изменился набор альбомов.").into());
    }
    fs::write(
        output.join("spotify-albums-semantics.json"),
        serde_json::to_vec_pretty(&json!({ "tiles": tiles, "list": list }))?,
    )?;
    Ok(())
}

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
    let source = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(usage)?
        .join("mediadeka.sqlite3");
    if arguments.next().is_some() || !source.is_file() {
        return Err(usage().into());
    }
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    let database = profile.path().join("mediadeka.sqlite3");
    let snapshot = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    snapshot.execute("VACUUM INTO ?1", [database.to_string_lossy().as_ref()])?;
    drop(snapshot);
    // This process owns the isolated profile and routes real requests to a closed local proxy.
    unsafe {
        env::set_var("MEDIADEKA_PROFILE_DIR", profile.path());
        env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");
        env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
    }
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                if let Err(error) = capture(scene, &output, cx).await {
                    fail(error);
                }
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

fn usage() -> io::Error {
    io::Error::other("--liked-audit <каталог кадров> <профиль с сохранёнными снимками коллекций>")
}

fn layout(scene: Scene) -> CaptureLayout {
    CaptureLayout {
        width: scene.width,
        height: scene.height,
        reduced_motion: scene.reduced,
    }
}

async fn capture(scene: Scene, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    let directory = output.join(scene.name);
    fs::create_dir_all(&directory)?;
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    view.update(cx, |view, window, cx| {
        view.start_home_for_capture(true, window, cx)
    })?;

    let deadline = Instant::now() + Duration::from_secs(90);
    let timer = cx.background_executor().clone();
    while view
        .update(cx, |view, _, _| view.home_status_for_capture())?
        .is_none()
    {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Не открылся сохранённый профиль.").into());
        }
        timer.timer(Duration::from_millis(80)).await;
    }
    view.update(cx, |view, window, cx| {
        view.show_music_collection_for_capture(false, window, cx)
    })?;
    let loading = frame(view, scene, &directory, "loading", cx)?;
    if loading["pending"] != true {
        return Err(
            io::Error::other(format!("Загрузка не отражена в состоянии: {loading}")).into(),
        );
    }
    while view.update(cx, |view, _, _| view.liked_audit_for_capture())?["pending"] == true {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Коллекции не завершили загрузку.").into());
        }
        timer.timer(Duration::from_millis(80)).await;
    }
    let errors = frame(view, scene, &directory, "errors-top", cx)?;
    if errors["tracks"].as_u64().unwrap_or_default() < 2
        || errors["sources_with_errors"].as_array().map_or(0, Vec::len) < 2
    {
        return Err(io::Error::other(format!(
            "Нужны реальные снимки и минимум две ошибки источников: {errors}"
        ))
        .into());
    }

    view.update(cx, |view, _, cx| view.retry_liked_for_capture(cx))?;
    let retry = frame(view, scene, &directory, "errors-retrying", cx)?;
    if retry["pending"] != true || retry["sources_with_errors"] != errors["sources_with_errors"] {
        return Err(io::Error::other(format!(
            "Не получено состояние ошибок во время повтора: {retry}"
        ))
        .into());
    }
    let position = view.update(cx, |view, _, _| view.liked_viewport_for_capture().center())?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(-430.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    let scrolled = frame(view, scene, &directory, "errors-scrolled-tracks", cx)?;
    if scrolled["first_item"].as_u64().unwrap_or_default() == 0 {
        return Err(
            io::Error::other(format!("Единая область не дошла до треков: {scrolled}")).into(),
        );
    }
    let evidence =
        json!({ "loading": loading, "errors": errors, "retry": retry, "scrolled": scrolled });
    fs::write(
        directory.join("semantics.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!("{} {evidence}", scene.name);
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn frame(
    view: WindowHandle<MainView>,
    scene: Scene,
    directory: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    draw(
        AnyWindowHandle::from(view),
        scene,
        Some(&directory.join(format!("{name}.png"))),
        cx,
    )?;
    view.update(cx, |view, _, _| view.liked_audit_for_capture())
}
