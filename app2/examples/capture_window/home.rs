use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, Point, ScrollDelta, ScrollWheelEvent,
    WindowHandle, point, px,
};
use mediadeka::CaptureLayout;

use super::{Scene, draw, fail, native_dpi_change};

#[path = "home/listening.rs"]
mod listening;
#[path = "home/mix.rs"]
mod mix;
pub(super) use mix::activate_control;
#[path = "home/radio.rs"]
mod radio;
#[path = "home/shelf_motion.rs"]
mod shelf_motion;
#[path = "home/soundcloud.rs"]
mod soundcloud;
#[path = "home/yandex.rs"]
mod yandex;

#[derive(Clone, Copy)]
enum CaptureMode {
    Standard(&'static str),
    Workspace,
    Scroll,
    SoundCloud,
    SoundCloudPartial,
    Yandex,
    YouTubeMixes,
    Combined,
    CombinedWide,
    Listening(i64),
    Mix,
    Radio,
    ShelfMotion { high_contrast: bool },
}

const SCENES: &[Scene] = &[
    Scene {
        name: "desktop",
        width: 1500.,
        height: 850.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "minimum",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: true,
    },
];

const WORKSPACE_SCENES: &[Scene] = &[
    Scene {
        name: "desktop",
        width: 1920.,
        height: 1080.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "minimum",
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
    Scene {
        name: "1039-dip",
        width: 1039.,
        height: 680.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "1040-dip",
        width: 1040.,
        height: 680.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "1041-dip",
        width: 1041.,
        height: 680.,
        dpi: 96,
        reduced: true,
    },
];

const COMBINED_WIDE_SCENES: &[Scene] = &[
    Scene {
        reduced: true,
        ..WORKSPACE_SCENES[0]
    },
    SCENES[1],
    WORKSPACE_SCENES[2],
];

const SCALED_SCENES: &[Scene] = &[SCENES[0], SCENES[1], WORKSPACE_SCENES[2]];
const SHELF_MOTION_SCENES: &[Scene] = &[
    WORKSPACE_SCENES[0],
    Scene {
        reduced: false,
        ..SCENES[1]
    },
    Scene {
        name: "minimum-reduced",
        ..SCENES[1]
    },
    Scene {
        reduced: false,
        ..WORKSPACE_SCENES[2]
    },
];

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let usage = "--home <каталог кадров> <skeleton|fresh|workspace|cached|empty|scroll|cadence|soundcloud|soundcloud-partial|yandex|youtube-mixes|combined|combined-1920|listening|mix|radio> [каталог тестового профиля или аудиофайл для listening]\n--home <каталог кадров> shelf-motion [--high-contrast]";
    let argument_error = || io::Error::new(io::ErrorKind::InvalidInput, usage);
    let output = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(argument_error)?;
    let mut mode = match arguments.next().as_deref().and_then(|value| value.to_str()) {
        Some("skeleton") => CaptureMode::Standard("skeleton"),
        Some("fresh") => CaptureMode::Standard("fresh"),
        Some("workspace") => CaptureMode::Workspace,
        Some("cached") => CaptureMode::Standard("cached"),
        Some("empty") => CaptureMode::Standard("empty"),
        Some("scroll") => CaptureMode::Scroll,
        Some("soundcloud") => CaptureMode::SoundCloud,
        Some("soundcloud-partial") => CaptureMode::SoundCloudPartial,
        Some("yandex") => CaptureMode::Yandex,
        Some("youtube-mixes") => CaptureMode::YouTubeMixes,
        Some("combined") => CaptureMode::Combined,
        Some("combined-1920") => CaptureMode::CombinedWide,
        Some("listening") => CaptureMode::Listening(0),
        Some("mix") => CaptureMode::Mix,
        Some("radio") => CaptureMode::Radio,
        Some("shelf-motion") => CaptureMode::ShelfMotion {
            high_contrast: match arguments.next().as_deref() {
                None => false,
                Some(flag) if flag == "--high-contrast" => true,
                _ => return Err(argument_error().into()),
            },
        },
        Some("cadence") if arguments.next().is_none() => return run_present_cadence(output),
        _ => return Err(argument_error().into()),
    };
    let mut profile_path = arguments.next().map(PathBuf::from);
    if matches!(mode, CaptureMode::ShelfMotion { .. }) && profile_path.is_some() {
        return Err(argument_error().into());
    }
    let use_profile_copy = matches!(mode, CaptureMode::Mix | CaptureMode::Radio);
    if use_profile_copy && profile_path.is_none() {
        profile_path = std::env::var_os("MEDIADEKA_PROFILE_DIR").map(PathBuf::from);
    }
    if arguments.next().is_some()
        || (matches!(
            mode,
            CaptureMode::Standard("cached")
                | CaptureMode::SoundCloud
                | CaptureMode::Yandex
                | CaptureMode::YouTubeMixes
                | CaptureMode::Mix
                | CaptureMode::Radio
        ) && profile_path.is_none())
    {
        return Err(argument_error().into());
    }
    let temporary_profile = tempfile::tempdir()?;
    if matches!(mode, CaptureMode::Listening(_)) {
        let fixture = fs::canonicalize(profile_path.take().ok_or_else(argument_error)?)?;
        let id = mediadeka::seed_playlist_capture(
            temporary_profile.path(),
            &[fixture.clone(), fixture],
        )?;
        mode = CaptureMode::Listening(id);
    }
    let check_overlay = matches!(mode, CaptureMode::Scroll) && profile_path.is_none();
    let profile = profile_path.as_deref().unwrap_or(temporary_profile.path());
    let restore = profile.join("mediadeka.sqlite3").is_file();
    if matches!(
        mode,
        CaptureMode::SoundCloud
            | CaptureMode::Yandex
            | CaptureMode::YouTubeMixes
            | CaptureMode::Mix
            | CaptureMode::Radio
    ) && !restore
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Для захвата нужен тестовый профиль с подключённым аккаунтом.",
        )
        .into());
    }
    let profile = if use_profile_copy {
        mix::copy_profile(profile, temporary_profile.path())?;
        temporary_profile.path()
    } else {
        profile
    };
    fs::create_dir_all(&output)?;
    // The CLI owns the process. Configure the isolated store before starting GPUI.
    unsafe {
        std::env::set_var("MEDIADEKA_PROFILE_DIR", profile);
        if matches!(mode, CaptureMode::ShelfMotion { .. }) {
            std::env::set_var("MEDIADEKA_GUI", "3");
        }
    }

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
            let _profile = temporary_profile;
            let scenes = match mode {
                CaptureMode::Workspace => WORKSPACE_SCENES,
                CaptureMode::CombinedWide => COMBINED_WIDE_SCENES,
                CaptureMode::Scroll | CaptureMode::Listening(_) => SCALED_SCENES,
                CaptureMode::ShelfMotion { .. } => SHELF_MOTION_SCENES,
                _ => SCENES,
            };
            for (index, scene) in scenes.iter().copied().enumerate() {
                if let Err(error) = capture(
                    scene,
                    &output,
                    mode,
                    restore || index > 0,
                    check_overlay,
                    cx,
                )
                .await
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

fn run_present_cadence(output: PathBuf) -> gpui::Result<()> {
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    let stats = output.join(format!("gpui-frame-stats-{}.log", std::process::id()));
    println!("GPUI_FRAME_STATS {}", stats.display());
    // Configure the isolated process before starting GPUI or any workers.
    unsafe {
        std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path());
        std::env::set_var(gpui::frame_stats::ENV_VAR, stats);
    }
    mediadeka::application().run(move |cx| {
        let window = mediadeka::open_main_window(cx).unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            sample_present_cadence(window, cx)
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

async fn sample_present_cadence(
    window: WindowHandle<mediadeka::MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(32);
    while !window.update(cx, |view, _, _| view.is_welcome_for_capture())? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: чистый профиль не открыл приветствие.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    window.update(cx, |view, window, cx| {
        view.start_home_for_capture(false, window, cx)
    })?;
    loop {
        if window.update(cx, |view, _, _| view.home_status_for_capture())? == Some("fresh") {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: нет свежей гостевой подборки для замера.").into());
        }
        timer.timer(Duration::from_millis(40)).await;
    }
    let raw = AnyWindowHandle::from(window);
    timer.timer(Duration::from_secs(2)).await;
    // Dispatch events on this GPUI window only. Its real platform frame loop
    // performs all draws; neither test frame ticks nor readbacks are used.
    click(raw, MouseButton::Left, point(px(32.), px(140.)), cx)?;
    timer.timer(Duration::from_millis(360)).await;
    let (active, viewport) = window.update(cx, |view, window, _| {
        (
            window.is_window_active(),
            view.home_scroll_bounds_for_capture(),
        )
    })?;
    if !active {
        return Err(io::Error::other("GPUI: окно замера не получило фокус.").into());
    }
    println!("GPUI: область прокрутки {:?}.", viewport.size);
    let anchor = viewport.center();
    let below = point(anchor.x, anchor.y + px(18.));
    click(raw, MouseButton::Middle, anchor, cx)?;
    move_pointer(raw, below, cx)?;
    timer.timer(Duration::from_millis(350)).await;
    let first = scroll_offset(window, cx)?;
    let start = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    timer.timer(Duration::from_secs(3)).await;
    let end = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let last = scroll_offset(window, cx)?;
    if first >= -5. || last >= first - 100. {
        return Err(io::Error::other("GPUI: в видимом окне автопрокрутка не продвинулась.").into());
    }
    println!("GPUI_WINDOW_UNIX_MS {start} {end}; scroll {first:.1} -> {last:.1}");
    // The GPUI recorder flushes its file after each one-second statistics block.
    timer.timer(Duration::from_millis(1150)).await;
    Ok(())
}

async fn capture(
    scene: Scene,
    output: &Path,
    mode: CaptureMode,
    restore: bool,
    check_overlay: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let expected = match mode {
        CaptureMode::Standard(expected) => expected,
        CaptureMode::SoundCloudPartial => "partial",
        CaptureMode::Workspace
        | CaptureMode::Combined
        | CaptureMode::CombinedWide
        | CaptureMode::Listening(_)
        | CaptureMode::Scroll
        | CaptureMode::SoundCloud
        | CaptureMode::Yandex
        | CaptureMode::Mix
        | CaptureMode::Radio
        | CaptureMode::ShelfMotion { .. }
        | CaptureMode::YouTubeMixes => "fresh",
    };
    let window = cx.update(|cx| {
        mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: scene.width,
                height: scene.height,
                reduced_motion: scene.reduced
                    && (expected != "skeleton" || scene.name != "desktop"),
            },
        )
    })?;
    let raw = AnyWindowHandle::from(window);
    if scene.dpi != 96 {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    if let CaptureMode::Listening(id) = mode {
        listening::capture(id, window, raw, scene, output, cx).await?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    if let CaptureMode::ShelfMotion { high_contrast } = mode {
        shelf_motion::capture(window, raw, scene, output, high_contrast, cx)?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    if matches!(mode, CaptureMode::Mix) {
        mix::capture(window, raw, scene, output, cx).await?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    if matches!(mode, CaptureMode::Radio) {
        radio::capture(window, raw, scene, output, cx).await?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    if matches!(mode, CaptureMode::Combined | CaptureMode::CombinedWide) {
        capture_combined(window, raw, scene, output, cx)?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }
    if expected == "skeleton" {
        window.update(cx, |view, window, cx| {
            view.start_home_skeleton_for_capture(window, cx)
        })?;
        let path = output.join(format!("home-{}-skeleton.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;
        if scene.name == "desktop" {
            let first = rendered_pixel(raw, 300, 330, cx)?;
            window.update(cx, |view, _, cx| {
                view.sample_welcome(Duration::from_millis(700), None, cx)
            })?;
            draw(
                raw,
                scene,
                Some(&output.join("home-desktop-skeleton-700ms.png")),
                cx,
            )?;
            let peak = rendered_pixel(raw, 300, 330, cx)?;
            if first == peak {
                return Err(io::Error::other(
                    "GPUI: пульсация skeleton не изменила пиксель кадра.",
                )
                .into());
            }
            println!("GPUI: пульсация skeleton, RGB в (300, 330) {first:?} → {peak:?} за 700 мс.");
        }
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }

    window.update(cx, |view, window, cx| match mode {
        CaptureMode::SoundCloudPartial => view.show_soundcloud_partial_for_capture(window, cx),
        _ => view.start_home_for_capture(restore, window, cx),
    })?;
    let deadline = Instant::now() + Duration::from_secs(32);
    let actual = loop {
        if let Some(status) = window.update(cx, |view, _, _| view.home_status_for_capture())? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Ожидание музыкальной главной истекло.").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    };
    if actual != expected {
        return Err(io::Error::other(format!(
            "{}: ожидалась главная {expected}, получена {actual}.",
            scene.name,
        ))
        .into());
    }
    if matches!(
        mode,
        CaptureMode::SoundCloud | CaptureMode::Yandex | CaptureMode::YouTubeMixes
    ) {
        window.update(cx, |view, _, cx| view.filter_active_home_for_capture(cx))?;
    }
    if matches!(mode, CaptureMode::Scroll | CaptureMode::Workspace) {
        super::sound::activate_accessibility(raw, cx)?;
        draw(raw, scene, None, cx)?;
        activate_control(raw, "home-page", cx)?;
    }
    if matches!(mode, CaptureMode::SoundCloud) {
        soundcloud::capture_pages(window, raw, scene, output, cx).await?;
        raw.update(cx, |_, window, _| window.remove_window())?;
        return Ok(());
    }

    draw(raw, scene, None, cx)?;
    if actual == "fresh" {
        // Square recommendation covers arrive from the live source after the feed itself.
        cx.background_executor().timer(Duration::from_secs(2)).await;
    }
    let path = output.join(format!("home-{}-{actual}.png", scene.name));
    draw(raw, scene, Some(&path), cx)?;
    if matches!(mode, CaptureMode::Workspace) {
        let (rail, content, viewport) = window.update(cx, |view, window, _| {
            view.workspace_layout_for_capture(window)
        })?;
        println!(
            "LAYOUT {}: rail {rail:.2}, content {content:.2}, viewport {viewport:.2} DIP",
            scene.name
        );
        if scene.name == "desktop" {
            check_navigation_motion(window, raw, scene, output, cx).await?;
        }
    }
    if matches!(mode, CaptureMode::SoundCloudPartial) {
        let (_, activity, _) = window.update(cx, |view, _, _| {
            view.soundcloud_home_tops_for_capture(gpui::size(px(scene.width), px(scene.height)))
        })?;
        let Some(top) = activity else {
            return Err(io::Error::other("GPUI: нет ленты подписок на главной SoundCloud.").into());
        };
        let origin = window.update(cx, |view, _, _| view.home_shelf_origin_for_capture())?;
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(origin + top - 12., cx)
        })?;
        draw(raw, scene, None, cx)?;
        let path = output.join(format!("soundcloud-{}-activity-partial.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;
    }
    if scene.name == "desktop" && actual == "fresh" {
        let shortcut = Keystroke::parse("ctrl-k")?;
        let before = window.update(cx, |view, window, cx| {
            view.home_search_focused_for_capture(window, cx)
        })?;
        raw.update(cx, |_, window, cx| window.dispatch_keystroke(shortcut, cx))?;
        let after = window.update(cx, |view, window, cx| {
            view.home_search_focused_for_capture(window, cx)
        })?;
        if before || !after {
            return Err(io::Error::other("Ctrl+K не переместил фокус в главном окне.").into());
        }
        println!("GPUI: Ctrl+K установил фокус в поле поиска.");
        let path = output.join("home-search-focus.png");
        draw(raw, scene, Some(&path), cx)?;
    }
    if matches!(mode, CaptureMode::Yandex) {
        yandex::capture_shelves(window, raw, scene, output, cx).await?;
    }
    if matches!(mode, CaptureMode::YouTubeMixes) {
        capture_youtube_mixes(window, raw, scene, output, cx).await?;
    }
    if matches!(mode, CaptureMode::Scroll) {
        capture_scroll_positions(window, raw, scene, output, cx).await?;
        if scene.name == "desktop" {
            measure_draw_times("рекомендации 1500 × 850", raw, None, cx)?;
        } else {
            check_autoscroll(window, raw, scene, cx).await?;
            if check_overlay {
                window.update(cx, |view, _, cx| {
                    view.set_home_scroll_offset_for_capture(0., cx)
                })?;
                draw(raw, scene, None, cx)?;
                draw(
                    raw,
                    scene,
                    Some(&output.join(format!("home-{}-sources.png", scene.name))),
                    cx,
                )?;
            }
        }
    }
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn capture_scroll_positions(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    draw(raw, scene, None, cx)?;
    let extent = window.update(cx, |view, _, _| view.home_scroll_extent_for_capture())?;
    if extent <= 1. {
        return Ok(());
    }
    for (position, fraction) in [("start", 0.), ("middle", 0.5), ("end", 1.)] {
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(extent * fraction, cx)
        })?;
        draw(raw, scene, None, cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(180))
            .await;
        let path = output.join(format!("home-{}-scroll-{position}.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;
    }
    window.update(cx, |view, _, cx| {
        view.set_home_scroll_offset_for_capture(0., cx)
    })?;
    Ok(())
}

fn capture_combined(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    use super::sound::{accessibility, activate_accessibility, node};

    const TOGGLE: &str = "home-source-toggle";
    const JELLYFIN: &str = "home-source-account-jellyfin";
    activate_accessibility(raw, cx)?;
    window.update(cx, |view, window, cx| {
        view.start_home_skeleton_for_capture(window, cx);
        view.show_combined_home_for_capture(cx);
        if std::env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3") {
            view.return_home_from_listening_fixture(window, cx);
        }
    })?;
    let all = window.update(cx, |view, _, _| view.combined_home_semantics_for_capture())?;
    if all["visible"].as_array().is_none_or(|sources| {
        sources.len() != 5
            || !sources.iter().any(|source| {
                source["provider"] == "jellyfin"
                    && source["shelves"] == 2
                    && source["status"] == "Нет сети"
            })
    }) {
        return Err(io::Error::other(format!("Общая главная: {all}")).into());
    }
    draw(
        raw,
        scene,
        Some(&output.join(format!("home-{}-all.png", scene.name))),
        cx,
    )?;
    let collapsed = mix::accessibility_tree(raw, cx)?;
    if node(&collapsed, TOGGLE).is_none_or(|button| {
        button["aria"]["expanded"] != false || button["aria"]["label"] != "Площадки · Все"
    }) || node(&collapsed, "home-source-all").is_some()
        || node(&collapsed, "refresh").is_some()
    {
        return Err(io::Error::other("Площадки не свёрнуты при входе на главную.").into());
    }
    activate_control(raw, TOGGLE, cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("home-{}-sources.png", scene.name))),
        cx,
    )?;
    let open = accessibility(
        raw,
        &output.join(format!("home-{}-sources.json", scene.name)),
        cx,
    )?;
    if node(&open, TOGGLE).is_none_or(|button| button["aria"]["expanded"] != true)
        || node(&open, "home-source-filters").is_none_or(|group| group["aria"]["role"] != "TabList")
        || node(&open, "refresh").is_none()
        || node(&open, "home-add-account").is_none()
        || node(&open, JELLYFIN).is_none_or(|tab| {
            tab["aria"]["label"]
                .as_str()
                .is_none_or(|label| !label.contains("Нет сети"))
        })
    {
        return Err(io::Error::other("Раскрытие не показало фильтры, статусы и действия.").into());
    }
    activate_control(raw, JELLYFIN, cx)?;
    let filtered = window.update(cx, |view, _, _| view.combined_home_semantics_for_capture())?;
    if filtered["active"] != "spotify"
        || filtered["visible"]
            .as_array()
            .is_none_or(|sources| sources.len() != 1 || sources[0]["provider"] != "jellyfin")
    {
        return Err(io::Error::other(format!("Фильтр Jellyfin: {filtered}")).into());
    }
    draw(raw, scene, None, cx)?;
    activate_control(raw, TOGGLE, cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("home-{}-jellyfin.png", scene.name))),
        cx,
    )?;
    let scoped = mix::accessibility_tree(raw, cx)?;
    let toggle_focused = scoped["gpui_focus"]
        .as_str()
        .is_some_and(|id| scoped["nodes"][id]["element_id"] == "Name(\"home-source-toggle\")");
    if !toggle_focused
        || node(&scoped, TOGGLE).is_none_or(|button| {
            button["aria"]["expanded"] != false || button["aria"]["label"] != "Площадки · Jellyfin"
        })
        || node(&scoped, JELLYFIN).is_some()
    {
        return Err(
            io::Error::other("Свёрнутый фильтр не сохранил выбранную площадку и фокус.").into(),
        );
    }
    activate_control(raw, TOGGLE, cx)?;
    draw(raw, scene, None, cx)?;
    activate_control(raw, JELLYFIN, cx)?;
    let reset = window.update(cx, |view, _, _| view.combined_home_semantics_for_capture())?;
    if reset != all {
        return Err(
            io::Error::other(format!("Повторный выбор не вернул все полки: {reset}")).into(),
        );
    }
    window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("home-{}-connections.png", scene.name))),
        cx,
    )?;
    fs::write(
        output.join(format!("home-{}-semantics.json", scene.name)),
        serde_json::to_vec_pretty(
            &serde_json::json!({"all": all, "filtered": filtered, "reset": reset, "sources_by_keyboard": true}),
        )?,
    )?;
    Ok(())
}

async fn check_navigation_motion(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let [(tracks, tracks_top), (playlists, _)] = window.update(cx, |view, window, _| {
        view.sidebar_motion_targets_for_capture(window)
    })?;
    click(raw, MouseButton::Left, tracks, cx)?;
    cx.background_executor()
        .timer(Duration::from_millis(35))
        .await;
    draw(
        raw,
        scene,
        Some(&output.join("sidebar-home-to-tracks.png")),
        cx,
    )?;
    let (first, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    click(raw, MouseButton::Left, playlists, cx)?;
    let (_, resumed_from) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    if first != resumed_from
        || first.is_none_or(|position| !(0.01..tracks_top - 0.01).contains(&position))
    {
        return Err(io::Error::other(format!(
            "Повторный переход: видимая позиция {first:?}, новое начало {resumed_from:?}."
        ))
        .into());
    }
    draw(
        raw,
        scene,
        Some(&output.join("sidebar-tracks-to-playlists.png")),
        cx,
    )?;
    activate_control(raw, "workspace-back", cx)?;
    cx.background_executor()
        .timer(Duration::from_millis(250))
        .await;
    draw(raw, scene, Some(&output.join("sidebar-back.png")), cx)?;
    let (back, _) = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    if back != Some(tracks_top) {
        return Err(io::Error::other(format!("Назад должно показать Треки: {back:?}.")).into());
    }
    println!(
        "SIDEBAR interrupted at {first:?}; resumed from {resumed_from:?}; Back ended at {back:?}"
    );
    activate_control(raw, "home-page", cx)?;
    draw(
        raw,
        scene,
        Some(&output.join("sidebar-keyboard-first-frame.png")),
        cx,
    )?;
    let keyboard = window.update(cx, |view, _, _| view.sidebar_motion_for_capture())?;
    if keyboard != (Some(0.), None) {
        return Err(io::Error::other(format!(
            "Клавиатурный переход не показал Главную сразу: {keyboard:?}."
        ))
        .into());
    }
    println!("SIDEBAR keyboard first frame = {keyboard:?}");
    Ok(())
}

async fn capture_youtube_mixes(
    view: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let shelves = view.update(cx, |view, _, _| {
        view.home_shelf_geometry_for_capture(gpui::size(px(scene.width), px(scene.height)))
    })?;
    let origin = view.update(cx, |view, _, _| view.home_shelf_origin_for_capture())?;
    for (title, name) in [
        ("Миксы для вас", "mixes"),
        ("Новинки и любимые треки", "favorites"),
        ("Плейлисты Recap", "recap"),
    ] {
        let Some((_, _, top, _)) = shelves.iter().find(|shelf| shelf.1 == title) else {
            if name == "recap" {
                continue;
            }
            return Err(io::Error::other(format!("GPUI: не найдена полка «{title}».")).into());
        };
        view.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(origin + top - 12., cx);
        })?;
        draw(raw, scene, None, cx)?;
        cx.background_executor().timer(Duration::from_secs(2)).await;
        let frame = output.join(format!("youtube-{}-{name}.png", scene.name));
        draw(raw, scene, Some(&frame), cx)?;
    }
    Ok(())
}

fn rendered_pixel(
    raw: AnyWindowHandle,
    x: u32,
    y: u32,
    cx: &mut AsyncApp,
) -> gpui::Result<[u8; 3]> {
    raw.update(cx, |_, window, _| {
        let frame = window.render_to_image()?;
        let [red, green, blue, _] = frame.get_pixel(x, y).0;
        Ok([red, green, blue])
    })?
}

async fn check_autoscroll(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    // The home collection itself is the vertically scrollable area.
    draw(raw, scene, None, cx)?;

    let viewport = window.update(cx, |view, _, _| view.home_scroll_bounds_for_capture())?;
    let (track, thumb) = window
        .update(cx, |view, window, _| {
            view.home_scrollbar_for_capture(window)
        })?
        .ok_or_else(|| io::Error::other("GPUI: список главной не имеет полосы прокрутки."))?;
    let top_of_thumb = point(track.center().x, thumb);
    let down_track = point(top_of_thumb.x, track.top() + track.size.height * 0.56);
    let initial = scroll_offset(window, cx)?;
    drag_thumb(raw, top_of_thumb, down_track, cx)?;
    draw(raw, scene, None, cx)?;
    let dragged = scroll_offset(window, cx)?;
    if dragged >= initial - 5. {
        return Err(
            io::Error::other("GPUI: перетаскивание правой полосы не прокрутило список.").into(),
        );
    }
    drag_thumb(raw, down_track, top_of_thumb, cx)?;
    draw(raw, scene, None, cx)?;
    if (scroll_offset(window, cx)? - initial).abs() > 2. {
        return Err(io::Error::other("GPUI: правый ползунок не вернулся к началу списка.").into());
    }
    println!(
        "GPUI: правый ползунок прокрутил содержимое ({initial:.1} → {dragged:.1}) и вернулся к началу."
    );

    let anchor = viewport.center();
    let below = point(anchor.x, anchor.y + viewport.size.height * 0.35);
    let above = point(anchor.x, anchor.y - viewport.size.height * 0.35);
    let start = scroll_offset(window, cx)?;

    click(raw, MouseButton::Middle, anchor, cx)?;
    draw(raw, scene, None, cx)?;
    if !window.update(cx, |view, _, _| view.home_autoscroll_active_for_capture())? {
        return Err(io::Error::other("GPUI: средняя кнопка не включила автопрокрутку.").into());
    }
    move_pointer(raw, below, cx)?;
    let down = wait_for_scroll(window, raw, scene, cx, |offset| offset < start - 5.).await?;

    move_pointer(raw, above, cx)?;
    let up = wait_for_scroll(window, raw, scene, cx, |offset| offset > down + 5.).await?;

    let escape = Keystroke::parse("escape")?;
    raw.update(cx, |_, window, cx| window.dispatch_keystroke(escape, cx))?;
    draw(raw, scene, None, cx)?;
    let stopped = scroll_offset(window, cx)?;
    move_pointer(raw, below, cx)?;
    for _ in 0..4 {
        cx.background_executor()
            .timer(Duration::from_millis(35))
            .await;
        draw(raw, scene, None, cx)?;
    }
    if window.update(cx, |view, _, _| view.home_autoscroll_active_for_capture())?
        || (scroll_offset(window, cx)? - stopped).abs() > 1.
    {
        return Err(io::Error::other("GPUI: Esc не остановил автопрокрутку.").into());
    }
    click(raw, MouseButton::Middle, anchor, cx)?;
    draw(raw, scene, None, cx)?;
    click(raw, MouseButton::Middle, anchor, cx)?;
    draw(raw, scene, None, cx)?;
    if window.update(cx, |view, _, _| view.home_autoscroll_active_for_capture())? {
        return Err(io::Error::other("GPUI: второй щелчок не остановил автопрокрутку.").into());
    }
    println!(
        "GPUI: средняя кнопка прокрутила вниз ({start:.1} → {down:.1}), затем вверх ({up:.1}), Esc остановил движение."
    );
    let before_wheel = scroll_offset(window, cx)?;
    dispatch(raw, wheel_event(anchor, -14.), cx)?;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    if (scroll_offset(window, cx)? - before_wheel).abs() < 1. {
        return Err(io::Error::other("GPUI: колесо не прокрутило медиатеку для замера.").into());
    }
    measure_draw_times("прокрутка медиатеки 800 × 400", raw, Some(anchor), cx)?;
    Ok(())
}

fn measure_draw_times(
    scenario: &str,
    raw: AnyWindowHandle,
    wheel_position: Option<Point<gpui::Pixels>>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const WARMUP: usize = 12;
    const MEASURED: usize = 80;
    let mut samples = raw.update(cx, |_, window, cx| {
        let mut samples = Vec::with_capacity(MEASURED);
        for index in 0..WARMUP + MEASURED {
            if let Some(position) = wheel_position {
                let delta = if index % 8 < 4 { -14. } else { 14. };
                window.dispatch_event(wheel_event(position, delta), cx);
            } else {
                window.refresh();
            }
            let started = Instant::now();
            window.draw(cx).clear(cx);
            if index >= WARMUP {
                samples.push(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        samples
    })?;
    samples.sort_unstable_by(f64::total_cmp);
    let median = samples[MEASURED / 2];
    let p95 = samples[MEASURED * 95 / 100 - 1];
    let maximum = samples[MEASURED - 1];
    println!(
        "GPUI: CPU-кадр {scenario}, {MEASURED} измерений, p50 {median:.2} мс, p95 {p95:.2} мс, max {maximum:.2} мс. Без D3D11 present и VSync."
    );
    Ok(())
}

fn wheel_event(position: Point<gpui::Pixels>, delta: f32) -> PlatformInput {
    ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
        ..Default::default()
    }
    .to_platform_input()
}

fn scroll_offset(
    window: WindowHandle<mediadeka::MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<f32> {
    window.update(cx, |view, _, _| view.home_scroll_offset_for_capture())
}

async fn wait_for_scroll(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    cx: &mut AsyncApp,
    changed: impl Fn(f32) -> bool,
) -> gpui::Result<f32> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        cx.background_executor()
            .timer(Duration::from_millis(28))
            .await;
        // A hidden test window has no compositor pulse. Deliver the callbacks
        // via GPUI's test hook before painting the off-screen frame.
        raw.update(cx, |_, window, cx| window.simulate_next_frame(cx))?;
        draw(raw, scene, None, cx)?;
        let offset = scroll_offset(window, cx)?;
        if changed(offset) {
            return Ok(offset);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: средняя кнопка не изменила прокрутку.").into());
        }
    }
}

pub(super) fn click(
    raw: AnyWindowHandle,
    button: MouseButton,
    position: Point<gpui::Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    move_pointer(raw, position, cx)?;
    dispatch(
        raw,
        MouseDownEvent {
            button,
            position,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )?;
    dispatch(
        raw,
        MouseUpEvent {
            button,
            position,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )
}

fn drag_thumb(
    raw: AnyWindowHandle,
    from: Point<gpui::Pixels>,
    to: Point<gpui::Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    move_pointer(raw, from, cx)?;
    dispatch(
        raw,
        MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )?;
    dispatch(
        raw,
        MouseMoveEvent {
            position: to,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )?;
    dispatch(
        raw,
        MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            click_count: 1,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )
}

fn move_pointer(
    raw: AnyWindowHandle,
    position: Point<gpui::Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    dispatch(
        raw,
        MouseMoveEvent {
            position,
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )
}

fn dispatch(raw: AnyWindowHandle, event: PlatformInput, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(event, cx);
    })
}
