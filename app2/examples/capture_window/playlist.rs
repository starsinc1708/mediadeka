use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use mediadeka::{CaptureLayout, MainView, providers::ProviderId};

use super::{Scene, draw, fail, native_dpi_change};

#[path = "playlist_review.rs"]
mod review;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Native,
    Review,
    Viewport,
}

const SCENES: [Scene; 4] = [
    Scene {
        name: "wide-100",
        width: 1500.,
        height: 850.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "wide-150",
        width: 1500.,
        height: 700.,
        dpi: 144,
        reduced: false,
    },
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
        reduced: false,
    },
];

const REVIEW_SCENES: [Scene; 4] = [
    Scene {
        name: "1920x1080",
        width: 1920.,
        height: 1080.,
        ..SCENES[0]
    },
    Scene {
        name: "800x400",
        ..SCENES[2]
    },
    Scene {
        name: "800x400-motion",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "800x400-150",
        ..SCENES[3]
    },
];

pub(super) fn seed(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let profile = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Укажите изолированный профиль."))?;
    let file = args
        .next()
        .ok_or_else(|| io::Error::other("Укажите WAV или площадку для проверки."))?;
    if args.next().is_some() {
        return Err(io::Error::other("Лишний аргумент профиля.").into());
    }
    if let Some(provider) = ProviderId::ALL
        .into_iter()
        .find(|provider| file == provider.as_str())
    {
        return mediadeka::seed_playlist_source_capture(&profile, provider);
    }
    let file = PathBuf::from(file).canonicalize()?;
    println!(
        "{}",
        mediadeka::seed_playlist_capture(&profile, &vec![file; 1_000])?
    );
    Ok(())
}

pub(super) fn source(args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let arguments = args.collect::<Vec<_>>();
    let [output, database, provider, id] = arguments.as_slice() else {
        return Err(io::Error::other(
            "--playlist-source <кадр> <копируемый SQLite> <площадка> <ID>",
        )
        .into());
    };
    let output = PathBuf::from(output);
    let provider = ProviderId::ALL
        .into_iter()
        .find(|item| provider == item.as_str())
        .ok_or_else(|| io::Error::other("Неизвестная площадка."))?;
    let id = id.to_string_lossy().into_owned();
    let profile = tempfile::tempdir()?;
    fs::copy(database, profile.path().join("mediadeka.sqlite3"))?;
    // The capture process uses a snapshot of the original SQLite, with no writes to the owner profile.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let scene = SCENES[0];
        let window = mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: scene.width,
                height: scene.height,
                reduced_motion: true,
            },
        )
        .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            let run: gpui::Result<()> = async {
                let raw = AnyWindowHandle::from(window);
                raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
                    .apply();
                delay(cx, 150).await;
                window.update(cx, |view, window, cx| {
                    view.open_saved_playlist_for_capture(
                        provider,
                        id,
                        provider.display_name().to_owned(),
                        window,
                        cx,
                    )
                })??;
                let start = Instant::now();
                loop {
                    if let Some(result) =
                        window.update(cx, |view, _, _| view.playlist_status_for_capture())?
                    {
                        let count = result.map_err(io::Error::other)?;
                        println!("{}: {count} tracks", provider.display_name());
                        break;
                    }
                    require(
                        start.elapsed() < Duration::from_secs(40),
                        "Таймаут чтения источника.".to_owned(),
                    )?;
                    delay(cx, 80).await;
                }
                draw(raw, scene, None, cx)?;
                delay(cx, 1500).await;
                draw(raw, scene, Some(&output), cx)?;
                raw.update(cx, |_, window, _| window.remove_window())?;
                Ok(())
            }
            .await;
            if let Err(error) = run {
                fail(error);
            }
        })
        .detach();
    });
    Ok(())
}

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Укажите каталог проверки плейлиста."))?;
    let file = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Укажите WAV для проверки плейлиста."))?
        .canonicalize()?;
    let mode = match args.next().as_deref() {
        None => Mode::Native,
        Some(mode) if mode == "review" => Mode::Review,
        Some(mode) if mode == "viewport" => Mode::Viewport,
        Some(_) => return Err(io::Error::other("Неизвестный режим проверки плейлиста.").into()),
    };
    let high_contrast = match args.next() {
        None => false,
        Some(flag) if flag == "--high-contrast" => {
            let fluid = std::env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3");
            if !fluid {
                return Err(
                    io::Error::other("Для --high-contrast требуется MEDIADEKA_GUI=3.").into(),
                );
            }
            true
        }
        Some(_) => {
            return Err(io::Error::other(
                "После режима проверки плейлиста допустим только --high-contrast.",
            )
            .into());
        }
    };
    if args.next().is_some() {
        return Err(io::Error::other("Лишний аргумент проверки плейлиста.").into());
    }
    let scenes: &[_] = if mode == Mode::Native {
        &SCENES
    } else {
        &REVIEW_SCENES
    };
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    let files = vec![file.clone(); 10_000];
    let id = mediadeka::seed_playlist_capture(profile.path(), &files)?;
    let menu_id =
        mediadeka::seed_playlist_capture(profile.path(), &[file.clone(), file.clone(), file])?;
    // The isolated capture process owns this environment variable.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let keep_alive = mediadeka::open_capture_window(
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
            let only = std::env::var("MEDIADEKA_STUDIO_SCENE").ok();
            let result: gpui::Result<()> = async {
                for &scene in scenes
                    .iter()
                    .filter(|scene| only.as_deref().is_none_or(|name| name == scene.name))
                {
                    capture(scene, &output, id, menu_id, mode, high_contrast, cx).await?;
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                fail(error);
            }
            let _ = keep_alive.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn capture(
    scene: Scene,
    output: &Path,
    id: i64,
    menu_id: i64,
    mode: Mode,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let review = mode == Mode::Review;
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
    if review {
        super::sound::activate_accessibility(raw, cx)?;
    }
    // The initial GPUI resize is asynchronous on Win32 and uses the monitor DPI.
    delay(cx, 100).await;
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    delay(cx, 150).await;
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    window.update(cx, |view, window, cx| {
        if high_contrast {
            view.set_fluid_high_contrast_for_capture(true, cx);
        }
        view.open_local_playlist_for_capture(id, window, cx);
        if review {
            view.reset_player_probe_focus_for_capture(window, cx);
        }
    })?;
    wait_count(window, 10_000, cx).await?;
    draw(raw, scene, None, cx)?;
    draw(raw, scene, Some(&output.join("playlist.png")), cx)?;
    let before = window.update(cx, |view, _, _| view.playlist_geometry_for_capture(0))?;
    require(
        before.3 == 0 && (before.4 || scene.width <= 800.),
        format!("Первый трек не в области просмотра: {before:?}"),
    )?;
    if mode == Mode::Viewport {
        require(
            window.update(cx, |view, _, _| view.uses_fluid_glass_for_capture())?,
            "Проверка маски плейлиста требует Fluid Glass.".to_owned(),
        )?;
        super::scroll_viewport::verify(
            window,
            scene,
            &output,
            super::scroll_viewport::Surface::Playlist,
            cx,
        )?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }

    window.update(cx, |view, window, cx| {
        view.play_playlist_track_for_capture(0, cx);
        view.select_playlist_track_for_capture(1, window, cx);
    })?;
    if scene.width <= 800. {
        window.update(cx, |view, _, cx| view.playlist_scroll_for_capture(0, cx))?;
    }
    // Drag on the same draw that lays out the inspector, with no following frame.
    draw(
        raw,
        scene,
        Some(&output.join("selected-first-frame.png")),
        cx,
    )?;
    let opened = check_drag(window, scene, &output.join("scroll-dragging.png"), cx)?;
    window.update(cx, |view, window, cx| {
        view.close_right_panel_for_capture(window, cx)
    })?;
    draw(raw, scene, Some(&output.join("closed-first-frame.png")), cx)?;
    let closed = check_drag(
        window,
        scene,
        &output.join("closed-scroll-dragging.png"),
        cx,
    )?;
    window.update(cx, |view, window, cx| {
        view.select_playlist_track_for_capture(1, window, cx);
        view.playlist_scroll_for_capture(0, cx);
    })?;
    let playing = wait_playback(window, cx).await?;
    require(
        playing.0 == Some(1) && playing.1 == Some(0),
        format!("Выбор и звук: {playing:?}"),
    )?;
    draw(raw, scene, Some(&output.join("playing-selected.png")), cx)?;

    let scrollbar = check_scrollbar(window, scene, &output, cx)?;
    let after = window.update(cx, |view, _, _| view.playlist_geometry_for_capture(9_999))?;
    require(
        after.4
            && after.3 >= 9_975
            && before.0 == after.0
            && before.1 == after.1
            && (before.2 - after.2).abs() < 12.,
        format!("Геометрия прокрутки: {before:?} → {after:?}"),
    )?;
    draw(raw, scene, Some(&output.join("scroll-10000.png")), cx)?;
    window.update(cx, |view, window, cx| {
        view.show_playlist_menu_for_capture(9_999, window, cx)
    })?;
    draw(raw, scene, None, cx)?;
    delay(cx, 155).await;
    draw(raw, scene, Some(&output.join("menu-at-edge.png")), cx)?;
    if review {
        review::check_filter(window, scene, &output, cx).await?;
    }
    window.update(cx, |view, window, cx| {
        view.playlist_history_for_capture(false, window, cx)
    })?;
    delay(cx, 400).await;
    let switched = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
    require(
        switched.2 == "playing" && switched.3 > playing.3 && switched.1 == Some(0),
        format!("Трек сменился при навигации: {playing:?} → {switched:?}"),
    )?;
    draw(
        raw,
        scene,
        Some(&output.join("navigation-keeps-player.png")),
        cx,
    )?;
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(menu_id, window, cx)
    })?;
    wait_count(window, 3, cx).await?;
    if review {
        review::check_commands(window, scene, &output, cx).await?;
        review::check_sort(window, scene, &output, cx).await?;
    }

    window.update(cx, |view, window, cx| {
        view.open_source_playlist_for_capture(
            ProviderId::Spotify,
            "37i9dQZF1DXcBWIGoYBM5M".to_owned(),
            "SET::0.1".to_owned(),
            window,
            cx,
        )
    })?;
    let start = Instant::now();
    loop {
        if let Some(result) = window.update(cx, |view, _, _| view.playlist_status_for_capture())? {
            require(
                result.is_err(),
                "Spotify unexpectedly accepted the guest profile".to_owned(),
            )?;
            break;
        }
        require(
            start.elapsed() < Duration::from_secs(8),
            "Проверка недоступного источника не завершилась.".to_owned(),
        )?;
        delay(cx, 50).await;
    }
    draw(
        raw,
        scene,
        Some(&output.join("spotify-unavailable.png")),
        cx,
    )?;
    fs::write(
        output.join("report.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "logical_size": [scene.width, scene.height], "dpi": scene.dpi, "reduced_motion": scene.reduced,
            "playlist_rows": 10000, "viewport_before": before, "viewport_after": after,
            "while_playing": playing, "after_navigation": switched, "spotify_guest_access_denied": true,
            "scrollbar": scrollbar,
            "first_frame_with_selection": opened, "first_frame_after_close": closed,
        }))?,
    )?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn check_scrollbar(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<serde_json::Value> {
    let raw = AnyWindowHandle::from(window);
    let drag = check_drag(
        window,
        scene,
        &output.join("scroll-dragging-playing.png"),
        cx,
    )?;
    let (track, _, _) = window
        .update(cx, |view, window, _| {
            view.playlist_scrollbar_for_capture(window)
        })?
        .ok_or_else(|| io::Error::other("Полоса прокрутки плейлиста не отрисована."))?;
    let x = track.center().x;
    let top = point(x, track.top() + px(1.));
    pointer(raw, [mouse_down(top), mouse_up(top)], cx)?;
    draw(raw, scene, None, cx)?;
    let after_track_click =
        window.update(cx, |view, _, _| view.playlist_geometry_for_capture(0).3)?;
    require(
        after_track_click == 0,
        format!("Щелчок по дорожке не вернул к началу: {after_track_click}"),
    )?;
    pointer(
        raw,
        [ScrollWheelEvent {
            position: point(x - px(30.), track.center().y),
            delta: ScrollDelta::Pixels(point(Pixels::ZERO, -px(600.))),
            ..Default::default()
        }
        .to_platform_input()],
        cx,
    )?;
    draw(raw, scene, None, cx)?;
    let after_wheel = window.update(cx, |view, _, _| view.playlist_geometry_for_capture(0).3)?;
    require(
        after_wheel > 0,
        format!("Колесо не прокрутило плейлист: {after_wheel}"),
    )?;
    let end = point(x, track.bottom() - px(1.));
    pointer(raw, [mouse_down(end), mouse_up(end)], cx)?;
    draw(raw, scene, None, cx)?;
    Ok(serde_json::json!({
        "drag": drag,
        "first_item_after_track_click": after_track_click,
        "first_item_after_wheel": after_wheel,
    }))
}

fn check_drag(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<serde_json::Value> {
    let raw = AnyWindowHandle::from(window);
    let (track, thumb_y, _) = window
        .update(cx, |view, window, _| {
            view.playlist_scrollbar_for_capture(window)
        })?
        .ok_or_else(|| io::Error::other("Полоса прокрутки плейлиста не отрисована."))?;
    let (extent, thumb_height) = window
        .update(cx, |view, window, _| {
            view.playlist_scrollbar_size_for_capture(window)
        })?
        .ok_or_else(|| io::Error::other("Нет масштаба полосы прокрутки."))?;
    let (_, top, bottom, _, _) =
        window.update(cx, |view, _, _| view.playlist_geometry_for_capture(0))?;
    let content_height = extent + (bottom - top);
    require(
        (420_000. ..=420_600.).contains(&content_height) && thumb_height <= 27.,
        format!("Масштаб для 10 000 строк по 42 DIP: {content_height}, бегунок {thumb_height} DIP"),
    )?;
    let x = track.center().x;
    let start = point(x, thumb_y);
    let midpoint = point(x - px(30.), track.center().y);
    let held = Some(MouseButton::Left);
    pointer(raw, [mouse_down(start), mouse_move(midpoint, held)], cx)?;
    draw(raw, scene, Some(output), cx)?;
    let middle = window.update(cx, |view, _, _| view.playlist_geometry_for_capture(5_000).3)?;
    let dragging = window.update(cx, |view, window, _| {
        view.playlist_scrollbar_for_capture(window)
            .is_some_and(|(_, _, active)| active)
    })?;
    require(
        dragging && (4_500..=5_500).contains(&middle),
        format!("Бегунок не дошёл до середины вне дорожки: {middle}"),
    )?;

    let bottom = point(midpoint.x, track.bottom() + px(13.));
    pointer(raw, [mouse_move(bottom, held)], cx)?;
    draw(raw, scene, None, cx)?;
    let at_bottom = window.update(cx, |view, _, _| view.playlist_geometry_for_capture(9_999))?;
    require(
        at_bottom.4,
        format!("Перетаскивание не дошло до последнего трека: {at_bottom:?}"),
    )?;
    pointer(raw, [mouse_up(bottom), mouse_move(midpoint, None)], cx)?;
    draw(raw, scene, None, cx)?;
    let after_release =
        window.update(cx, |view, _, _| view.playlist_geometry_for_capture(9_999))?;
    let still_dragging = window.update(cx, |view, window, _| {
        view.playlist_scrollbar_for_capture(window)
            .is_some_and(|(_, _, active)| active)
    })?;
    require(
        after_release.4 && !still_dragging,
        format!("Отпускание не завершило перетаскивание: {after_release:?}"),
    )?;

    Ok(serde_json::json!({
        "extent_dip": extent, "thumb_height_dip": thumb_height,
        "content_height_dip": content_height,
        "first_item_mid_drag_outside_rail": middle,
        "last_track_visible_after_release": after_release.4,
    }))
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

fn mouse_down(position: Point<Pixels>) -> PlatformInput {
    MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }
    .to_platform_input()
}

fn mouse_move(position: Point<Pixels>, pressed_button: Option<MouseButton>) -> PlatformInput {
    MouseMoveEvent {
        position,
        pressed_button,
        ..Default::default()
    }
    .to_platform_input()
}

fn mouse_up(position: Point<Pixels>) -> PlatformInput {
    MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }
    .to_platform_input()
}

async fn wait_count(
    window: WindowHandle<MainView>,
    expected: usize,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let start = Instant::now();
    loop {
        if let Some(result) = window.update(cx, |view, _, _| view.playlist_status_for_capture())? {
            return require(
                result == Ok(expected),
                format!("Ожидалось {expected} строк: {result:?}"),
            );
        }
        require(
            start.elapsed() < Duration::from_secs(12),
            "Таймаут загрузки плейлиста.".to_owned(),
        )?;
        delay(cx, 35).await;
    }
}

async fn wait_playback(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<(Option<usize>, Option<usize>, String, f64)> {
    let start = Instant::now();
    loop {
        let state = window.update(cx, |view, _, cx| view.playlist_state_for_capture(cx))?;
        if state.2 == "playing" && state.3 > 0.05 {
            return Ok(state);
        }
        require(
            start.elapsed() < Duration::from_secs(12) && !state.2.starts_with("failed"),
            format!("Нет подтверждения звука: {state:?}"),
        )?;
        delay(cx, 50).await;
    }
}

fn require(passed: bool, error: String) -> gpui::Result<()> {
    if passed {
        Ok(())
    } else {
        Err(io::Error::other(error).into())
    }
}

async fn delay(cx: &AsyncApp, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}
