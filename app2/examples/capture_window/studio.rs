//! Native acceptance of Fluid Glass with a real local profile.

#[path = "studio/appearance.rs"]
mod appearance;
pub(super) use appearance::reopen;
#[path = "studio/audio.rs"]
mod audio;
#[path = "studio/collapsing_hero.rs"]
mod collapsing_hero;
#[path = "studio/collections.rs"]
mod collections;
#[path = "studio/contrast.rs"]
mod contrast;
#[path = "studio/control_feedback.rs"]
mod control_feedback;
#[path = "studio/details.rs"]
mod details;
#[path = "studio/dock.rs"]
mod dock;
#[path = "studio/edge.rs"]
mod edge;
#[path = "studio/entity_navigation.rs"]
mod entity_navigation;
pub(super) use entity_navigation::run_known_release;
#[path = "studio/filters.rs"]
mod filters;
#[path = "studio/fixture.rs"]
mod fixture;
#[path = "studio/junction.rs"]
mod junction;
#[path = "studio/launch.rs"]
mod launch;
#[path = "studio/library_row.rs"]
mod library_row;
#[path = "studio/listening_tabs.rs"]
mod listening_tabs;
#[path = "studio/motion.rs"]
mod motion;
#[path = "studio/native_fullscreen.rs"]
mod native_fullscreen;
#[path = "studio/nav.rs"]
mod nav;
#[path = "studio/page_motion.rs"]
mod page_motion;
#[path = "studio/palette.rs"]
mod palette;
#[path = "studio/player_layout.rs"]
mod player_layout;
#[path = "studio/quality.rs"]
mod quality;
#[path = "studio/queue.rs"]
mod queue;
#[path = "studio/readability.rs"]
mod readability;
#[path = "studio/resize.rs"]
mod resize;
#[path = "studio/transition.rs"]
mod transition;
#[path = "studio/visibility_power.rs"]
pub(super) mod visibility_power;
#[path = "studio/visual.rs"]
mod visual;

use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle};
use mediadeka::{CaptureLayout, MainView};
use serde_json::{Value, json};

use super::{
    Scene, draw, fail, native_dpi_change,
    sound::{accessibility, activate_accessibility, node, node_label},
};

const VISUAL_SCENES: [Scene; 5] = [
    Scene {
        name: "1920x1080",
        width: 1920.,
        height: 1080.,
        dpi: 96,
        reduced: false,
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
    Scene {
        name: "801x401",
        width: 801.,
        height: 401.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "1279x719",
        width: 1279.,
        height: 719.,
        dpi: 96,
        reduced: false,
    },
];

const SCENES: [Scene; 3] = [
    Scene {
        name: "1920x1080",
        width: 1920.,
        height: 1080.,
        dpi: 96,
        reduced: false,
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
const HERO_SCENES: [Scene; 3] = [
    SCENES[0],
    Scene {
        reduced: false,
        ..SCENES[1]
    },
    SCENES[2],
];
const SEARCH_QUERY: &str = "тихая гавань";
const TITLE: &str = "Тихая гавань";
const COVER: &str = "https://i.scdn.co/image/mediadeka-fluid-glass-fixture";
const SAVED_RESULT: &str = "NamedInteger(\"music-search-saved\", 0)";
const QUEUE_BUTTON: &str = "player-queue";
const QUEUE_LIST: &str = "player-queue-list";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Checks {
    All,
    Controls,
    HighContrast,
    Details { high_contrast: bool },
    CollapsingHero { high_contrast: bool },
    Collections,
    EntityNavigation,
    SpatialMotion,
    NativeFullscreen,
    ListeningTabs { high_contrast: bool },
    PlayerLayout,
    QueueMotion,
    TrackMotion,
    Audio,
    Appearance,
    Visual,
}

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let usage = || {
        io::Error::other(
            "--fluid-glass <каталог кадров> [--controls | --high-contrast | --details [--high-contrast] | --collapsing-hero [--high-contrast] | --collections | --entity-navigation | --spatial-motion | --native-fullscreen | --listening-tabs [--high-contrast] | --player-layout | --queue-motion | --track-motion | --audio | --appearance | --visual]",
        )
    };
    let output = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let checks = match args.next() {
        None => Checks::All,
        Some(flag) if flag == "--controls" && args.next().is_none() => Checks::Controls,
        Some(flag) if flag == "--high-contrast" && args.next().is_none() => Checks::HighContrast,
        Some(flag) if flag == "--appearance" && args.next().is_none() => Checks::Appearance,
        Some(flag) if flag == "--collections" && args.next().is_none() => Checks::Collections,
        Some(flag) if flag == "--entity-navigation" && args.next().is_none() => {
            Checks::EntityNavigation
        }
        Some(flag) if flag == "--spatial-motion" && args.next().is_none() => Checks::SpatialMotion,
        Some(flag) if flag == "--native-fullscreen" && args.next().is_none() => {
            Checks::NativeFullscreen
        }
        Some(flag) if flag == "--player-layout" && args.next().is_none() => Checks::PlayerLayout,
        Some(flag) if flag == "--queue-motion" && args.next().is_none() => Checks::QueueMotion,
        Some(flag) if flag == "--track-motion" && args.next().is_none() => Checks::TrackMotion,
        Some(flag) if flag == "--audio" && args.next().is_none() => Checks::Audio,
        Some(flag) if flag == "--visual" && args.next().is_none() => Checks::Visual,
        Some(flag)
            if flag == "--details" || flag == "--collapsing-hero" || flag == "--listening-tabs" =>
        {
            let high_contrast = match args.next() {
                None => false,
                Some(flag) if flag == "--high-contrast" && args.next().is_none() => true,
                Some(_) => return Err(usage().into()),
            };
            match flag.to_str() {
                Some("--details") => Checks::Details { high_contrast },
                Some("--collapsing-hero") => Checks::CollapsingHero { high_contrast },
                _ => Checks::ListeningTabs { high_contrast },
            }
        }
        Some(_) => return Err(usage().into()),
    };
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    fixture::seed(
        profile.path(),
        matches!(checks, Checks::QueueMotion | Checks::EntityNavigation),
    )?;
    if matches!(
        checks,
        Checks::Details { .. }
            | Checks::CollapsingHero { .. }
            | Checks::Appearance
            | Checks::Visual
    ) {
        details::seed(profile.path())?;
    }
    if matches!(checks, Checks::CollapsingHero { .. }) {
        collapsing_hero::seed(profile.path())?;
    }
    if checks == Checks::EntityNavigation {
        entity_navigation::seed(profile.path(), details::ARTIST_REGION)?;
    }
    // This process owns the profile and proxy configuration. Set them before it starts GPUI.
    unsafe {
        env::set_var("MEDIADEKA_GUI", "3");
        env::set_var("MEDIADEKA_PROFILE_DIR", profile.path());
        if matches!(
            checks,
            Checks::Details { .. }
                | Checks::CollapsingHero { .. }
                | Checks::Appearance
                | Checks::Visual
                | Checks::EntityNavigation
        ) {
            env::set_var("MEDIADEKA_MUSIC_REGION", details::ARTIST_REGION);
        }
        env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");
        env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
    }
    let profile_path = profile.path().to_path_buf();
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[1]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let mut failure = None;
            let only = env::var("MEDIADEKA_STUDIO_SCENE").ok();
            let pool: &[Scene] = match checks {
                Checks::Visual => &VISUAL_SCENES,
                Checks::Collections => &VISUAL_SCENES[..4],
                Checks::EntityNavigation => &HERO_SCENES,
                Checks::CollapsingHero { .. } => &HERO_SCENES,
                Checks::Audio => &SCENES[..1],
                Checks::NativeFullscreen => &VISUAL_SCENES[3..4],
                _ => &SCENES,
            };
            let scenes: Vec<Scene> = pool
                .iter()
                .take(if checks == Checks::HighContrast {
                    2
                } else {
                    pool.len()
                })
                .copied()
                .filter(|scene| only.as_deref().is_none_or(|name| name == scene.name))
                .collect();
            for &scene in &scenes {
                if let Err(error) =
                    capture(scene, &output.join(scene.name), checks, &profile_path, cx).await
                {
                    failure = Some(format!("{}: {error:#}", scene.name));
                    break;
                }
            }
            if failure.is_none()
                && checks == Checks::All
                && automatic_shader(quality::requested().as_deref())
            {
                let directory = output.join(SCENES[0].name);
                if let Err(error) = resize::verify_pins(anchor, SCENES[1], &directory, cx).await {
                    failure = Some(format!("Адаптивное меню с закреплениями: {error:#}"));
                }
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
            drop(profile);
            if let Some(error) = failure {
                fail(error);
            }
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

fn automatic_shader(quality: Option<&str>) -> bool {
    shader_with_artwork_palette() && quality.is_none_or(|name| name == "balanced")
}

fn shader_with_artwork_palette() -> bool {
    env::var_os("MEDIADEKA_GLASS_EFFECTS").is_none()
        && env::var_os("MEDIADEKA_GLASS_THEME").is_none()
}

async fn capture(
    scene: Scene,
    output: &Path,
    checks: Checks,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let window = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    let result = flow(window, scene, output, checks, profile, cx).await;
    if result.is_err() {
        let _ = frame(raw, scene, output, "failure", cx);
    }
    window.update(cx, |_, window, _| window.remove_window())?;
    result
}

async fn flow(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    checks: Checks,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    if matches!(checks, Checks::CollapsingHero { .. }) {
        println!("{}: открытие профиля для проверки Hero", scene.name);
    }
    let high_contrast = matches!(
        checks,
        Checks::HighContrast
            | Checks::ListeningTabs {
                high_contrast: true
            }
            | Checks::Details {
                high_contrast: true
            }
            | Checks::CollapsingHero {
                high_contrast: true
            }
    );
    window.update(cx, |view, window, cx| {
        if high_contrast {
            view.set_fluid_high_contrast_for_capture(true, cx);
        }
        if matches!(checks, Checks::Appearance | Checks::Visual) {
            // A previous scene may have stored the settings route. Start from
            // the guest home so the album fixture is reachable in every size.
            view.start_home_for_capture(false, window, cx);
        } else {
            view.start_fluid_glass_for_capture(window, cx);
        }
        view.sample_fluid_glass_for_capture(Duration::ZERO, cx);
    })?;
    wait(
        cx,
        "профиль и первые строки медиатеки",
        |cx| {
            let ready = window.update(cx, |view, _, _| view.home_status_for_capture().is_some())?;
            let tree = snapshot(raw, cx)?;
            // These checks can begin on a restored collection route, where
            // playable track rows are outside the current view.
            let playable = matches!(
                checks,
                Checks::Appearance
                    | Checks::Visual
                    | Checks::Collections
                    | Checks::EntityNavigation
                    | Checks::CollapsingHero { .. }
            ) || queue_candidate(&tree).is_some();
            Ok((ready && playable).then_some(()))
        },
    )
    .await?;
    if checks == Checks::Appearance {
        return appearance::verify(window, scene, output, profile, cx).await;
    }
    if checks == Checks::Visual {
        return visual::verify(window, scene, output, profile, cx).await;
    }
    if matches!(checks, Checks::Details { .. }) {
        return details::verify(window, scene, output, high_contrast, cx).await;
    }
    if matches!(checks, Checks::CollapsingHero { .. }) {
        println!("{}: профиль готов, проверка Hero", scene.name);
        return collapsing_hero::verify(window, scene, output, profile, high_contrast, cx).await;
    }
    if checks == Checks::Collections {
        return collections::verify(window, scene, output, cx).await;
    }
    if checks == Checks::EntityNavigation {
        return entity_navigation::verify(window, scene, output, cx).await;
    }
    if checks == Checks::QueueMotion {
        return queue::verify_drag(window, scene, output, cx).await;
    }
    if checks == Checks::TrackMotion {
        return transition::verify(window, scene, output, cx).await;
    }
    if checks == Checks::NativeFullscreen {
        return native_fullscreen::verify(window, output, cx).await;
    }
    if matches!(checks, Checks::ListeningTabs { .. }) {
        return listening_tabs::verify(window, scene, output, high_contrast, cx).await;
    }
    if checks == Checks::PlayerLayout {
        return player_layout::verify_controls(window, scene, output, cx).await;
    }
    if checks == Checks::Audio {
        audio::verify_settings(window, scene, output, profile, cx).await?;
        return audio::verify(window, scene, output, cx).await;
    }
    let library = frame(raw, scene, output, "library", cx)?;
    let quality = quality::requested();
    if let Some(requested) = quality.as_deref().filter(|_| !high_contrast) {
        quality::verify(window, scene, requested, output, cx)?;
    }
    nav::verify(window, scene, output, &library, high_contrast, cx).await?;
    filters::verify(raw, scene, output, cx)?;

    key(raw, "ctrl-k", cx)?;
    require(
        focused(&snapshot(raw, cx)?) == Some(&named("home-search")),
        "Ctrl+K должен фокусировать поле поиска.",
    )?;
    for character in SEARCH_QUERY.chars() {
        let key = character.to_string();
        dispatch(
            raw,
            Keystroke {
                key: key.clone(),
                key_char: Some(key),
                ..Default::default()
            },
            cx,
        )?;
    }
    key(raw, "enter", cx)?;
    wait(
        cx,
        "результат SQLite из заголовка WAV",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(by_element(&tree, SAVED_RESULT)
                .is_some_and(|row| labeled_button(row, TITLE))
                .then_some(()))
        },
    )
    .await?;
    focus(raw, SAVED_RESULT, cx)?;
    frame(raw, scene, output, "search", cx)?;
    key(raw, "enter", cx)?;
    let first = sound(window, TITLE, cx).await?;
    frame(raw, scene, output, "playing", cx)?;
    if matches!(checks, Checks::Controls | Checks::HighContrast) {
        launch::verify(window, scene, output, cx)?;
    }
    if !scene.reduced && !high_contrast {
        for milliseconds in [600, 1500] {
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_for_capture(Duration::from_millis(milliseconds), cx);
            })?;
            draw(
                raw,
                scene,
                Some(&output.join(format!("playing-{milliseconds}ms.png"))),
                cx,
            )?;
        }
    }

    press(raw, &named("studio-library"), cx)?;
    let (append_control, next_title) =
        wait(cx, "другая запись в медиатеке", |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(queue_candidate(&tree)
                .map(|(control, title)| (control.to_owned(), title.to_owned())))
        })
        .await?;
    press(raw, &append_control, cx)?;
    let undocked = window.update(cx, |view, window, _| {
        view.adaptive_content_for_capture(window)
    })?;
    press(raw, &named(QUEUE_BUTTON), cx)?;
    settle_panel(raw, scene, cx).await?;
    let queue = frame(raw, scene, output, "queue", cx)?;
    require(
        node_label(&queue, QUEUE_LIST) == Some("Треки в очереди")
            && queue_row(&queue, 0, TITLE, true)
            && queue_row(&queue, 1, &next_title, false),
        "Добавление должно оставить текущую запись и показать следующую в очереди.",
    )?;
    let automatic_shader = automatic_shader(quality.as_deref());
    let default_atmosphere = !scene.reduced && !high_contrast && automatic_shader;
    let queue_background_top = queue::background_top(window, scene, cx)?;
    if default_atmosphere && checks != Checks::SpatialMotion {
        check_dock_pixels(output, "queue", "dock-contrast.json", queue_background_top)?;
    }
    if checks == Checks::Controls {
        let docked = window.update(cx, |view, window, _| {
            view.adaptive_content_for_capture(window)
        })?;
        if scene.width >= 1440. {
            require(
                undocked.0 > docked.0 + 240. && undocked.1 != docked.1,
                "Правая панель должна уменьшить измеренную ширину и сменить режим компоновки.",
            )?;
        } else {
            require(
                undocked == docked,
                "Очередь в компактном режиме должна занимать рабочую область целиком.",
            )?;
        }
        fs::write(
            output.join("adaptive-content.json"),
            serde_json::to_vec_pretty(&json!({ "closed": undocked, "open": docked }))?,
        )?;
        queue::verify(window, scene, output, cx)?;
    }
    key(raw, "escape", cx)?;
    let restored = frame(raw, scene, output, "library-return", cx)?;
    require(
        node(&restored, QUEUE_LIST).is_none()
            && queue_candidate(&restored).is_some()
            && focused(&restored) == Some(&named(QUEUE_BUTTON)),
        "Escape должен закрыть очередь, вернуть медиатеку и восстановить фокус кнопки.",
    )?;
    press(raw, &named("player-next"), cx)?;
    let second = sound(window, &next_title, cx).await?;
    require(
        focused(&snapshot(raw, cx)?).is_some(),
        "Переход к последнему треку должен сохранить клавиатурный фокус после отключения «Далее».",
    )?;
    press(raw, &named(QUEUE_BUTTON), cx)?;
    settle_panel(raw, scene, cx).await?;
    let advanced = frame(raw, scene, output, "queue-next", cx)?;
    require(
        queue_row(&advanced, 0, TITLE, false) && queue_row(&advanced, 1, &next_title, true),
        "После перехода отметка текущей записи должна смениться в дереве очереди.",
    )?;
    fs::write(
        output.join("playback.json"),
        serde_json::to_vec_pretty(&json!({
            "search_query": SEARCH_QUERY,
            "reduced_motion": scene.reduced,
            "first": {"title": TITLE, "sound": first},
            "next": {"title": next_title, "sound": second},
        }))?,
    )?;
    if high_contrast {
        return contrast::verify(window, scene, output, queue_background_top, cx);
    }
    if checks == Checks::Controls {
        key(raw, "escape", cx)?;
        press(raw, &named("player-play"), cx)?;
        wait(
            cx,
            "Пауза перед проверкой элементов плеера",
            |cx| {
                Ok(
                    (node_label(&snapshot(raw, cx)?, "player-play") == Some("Воспроизвести"))
                        .then_some(()),
                )
            },
        )
        .await?;
        if scene.dpi == 96 {
            control_feedback::verify(window, scene, output, cx).await?;
        }
        if automatic_shader {
            edge::verify(window, scene, output, cx).await?;
            if scene.name == SCENES[0].name {
                resize::verify(window, scene, output, cx).await?;
            }
        }
        println!(
            "{}: native filters, playback and controls checked",
            scene.name
        );
        return Ok(());
    }
    if checks == Checks::SpatialMotion {
        key(raw, "escape", cx)?;
        dock::verify(window, scene, output, cx)?;
        player_layout::verify_artwork_motion(window, scene, output, cx)?;
        if scene.dpi == 96 {
            page_motion::verify(window, scene, output, cx).await?;
        }
        return Ok(());
    }
    if default_atmosphere {
        key(raw, "escape", cx)?;
        if scene.name == SCENES[0].name {
            require(
                node(&snapshot(raw, cx)?, QUEUE_LIST).is_none(),
                "Проверка фазы Fluid Glass ожидает медиатеку без открытой очереди.",
            )?;
            motion::verify(window, output, cx)?;
        }
        palette::verify_cyan(window, scene, output, cx).await?;
        audio::verify(window, scene, output, cx).await?;
        palette::verify_multicolor(window, scene, output, cx).await?;
        transition::verify(window, scene, output, cx).await?;
        readability::verify(output)?;
        dock::verify(window, scene, output, cx)?;
    } else if automatic_shader && scene.name == SCENES[1].name {
        key(raw, "escape", cx)?;
        palette::verify_reduced(window, scene, output, cx).await?;
        transition::verify(window, scene, output, cx).await?;
        dock::verify(window, scene, output, cx)?;
    } else if !scene.reduced && quality.as_deref() == Some("eco") && shader_with_artwork_palette() {
        key(raw, "escape", cx)?;
        transition::verify(window, scene, output, cx).await?;
    }
    if automatic_shader {
        if scene.dpi == 96 {
            player_layout::verify(window, scene, output, cx).await?;
        }
        edge::verify(window, scene, output, cx).await?;
        if scene.dpi == 96 {
            page_motion::verify(window, scene, output, cx).await?;
            control_feedback::verify(window, scene, output, cx).await?;
        }
        if scene.name == SCENES[0].name {
            resize::verify(window, scene, output, cx).await?;
        }
    }
    println!(
        "{}: SQLite search, keyboard focus, libmpv position, queue append and next passed",
        scene.name
    );
    Ok(())
}

fn check_dock_pixels(
    output: &Path,
    stem: &str,
    report_name: &str,
    background_top: u32,
) -> gpui::Result<()> {
    let frame = image::open(output.join(format!("{stem}.png")))?.to_rgb8();
    // In the fixed desktop scene the 320-DIP panel starts at x = 1580.
    // Sample the dock junction at several heights, not one row: the seam is
    // vertical but its contrast can vary with the pattern and the player.
    let seam_rows = [240, 360, 520, 700, 860];
    let seam = seam_rows.map(|y| [1579, 1580, 1581].map(|x| frame.get_pixel(x, y).0));
    let seam_jump = seam
        .iter()
        .flat_map(|row| row.windows(2))
        .flat_map(|pair| {
            std::array::from_fn::<_, 3, _>(|index| pair[0][index].abs_diff(pair[1][index]))
        })
        .max()
        .unwrap_or_default();
    let mut peak = 0_f64;
    // Start below the seeded track rows and finish before the player's border.
    require(
        background_top < 940,
        "Под треками не осталось области для замера фона.",
    )?;
    for y in (background_top..940).step_by(5) {
        for x in (1590..1895).step_by(5) {
            peak = peak.max(luminance(frame.get_pixel(x, y).0));
        }
    }
    let contrast = (luminance([176, 170, 185]) + 0.05) / (peak + 0.05);
    let report = json!({
        "seam_rgb": seam,
        "sampled_y": seam_rows,
        "largest_seam_jump": seam_jump,
        "specular_rim_px": 1580,
        "background_region": [1590, background_top, 1895, 940],
        "contrast_secondary": contrast,
    });
    fs::write(
        output.join(report_name),
        serde_json::to_vec_pretty(&report)?,
    )?;
    // The dock has a one-pixel specular rim at the junction; it is local edge
    // lighting, not a horizontal seam. The broad field stays within the same
    // 16-step budget at the centre rows, while the rim itself can reach 20.
    require(
        seam_jump <= 24 && contrast >= 4.5,
        "Стык очереди или контраст подписей не прошёл проверку.",
    )?;
    println!(
        "{stem}: dock seam max RGB step {seam_jump}, sampled secondary contrast {contrast:.2}:1"
    );
    Ok(())
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let [r, g, b] = rgb.map(|value| {
        let channel = f64::from(value) / 255.;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

async fn sound(
    window: WindowHandle<MainView>,
    title: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let current = |cx: &mut AsyncApp, after: f64| {
        let tree = snapshot(window.into(), cx)?;
        let sound = window.update(cx, |view, _, cx| view.sound_state_for_capture(cx))?;
        Ok(sound.filter(|data| {
            node_label(&tree, "player-track") == Some(title)
                && data["source"] == "Локальный файл"
                && data["position"].as_f64().is_some_and(|time| time > after)
        }))
    };
    let first = wait(
        cx,
        &format!("libmpv воспроизводит «{title}»"),
        |cx| current(cx, 0.20),
    )
    .await?;
    let initial = first["position"]
        .as_f64()
        .ok_or_else(|| io::Error::other("Нет позиции libmpv."))?;
    let later = wait(
        cx,
        &format!("растёт позиция «{title}»"),
        |cx| current(cx, initial + 0.20),
    )
    .await?;
    Ok(json!({ "initial_position": initial, "later": later }))
}

async fn settle_panel(raw: AnyWindowHandle, scene: Scene, cx: &mut AsyncApp) -> gpui::Result<()> {
    if !scene.reduced {
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        cx.background_executor()
            .timer(Duration::from_millis(180))
            .await;
    }
    Ok(())
}

fn queue_candidate(tree: &Value) -> Option<(&str, &str)> {
    tree["nodes"].as_object()?.values().find_map(|row| {
        let id = row["element_id"].as_str()?;
        let title = row["aria"]["label"]
            .as_str()?
            .strip_prefix("Добавить в очередь · ")?;
        (id.starts_with("Name(\"studio-library-queue-") && title != TITLE).then_some((id, title))
    })
}

fn queue_row(tree: &Value, index: usize, title: &str, selected: bool) -> bool {
    let expected = format!("{}. {title}", index + 1);
    queue_entry(tree, index).is_some_and(|item| {
        let aria = &item["aria"];
        aria["selected"] == selected
            && aria["label"]
                .as_str()
                .and_then(|label| label.strip_prefix(&expected))
                .is_some_and(|tail| tail.is_empty() || tail.starts_with(" · "))
    })
}

fn queue_entry(tree: &Value, index: usize) -> Option<&Value> {
    tree["nodes"].as_object()?.values().find(|item| {
        item["aria"]["role"] == "ListBoxOption"
            && item["aria"]["position_in_set"].as_u64() == Some(index as u64)
            && item["element_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("NamedInteger(\"player-queue-entry\", "))
    })
}

fn labeled_button(node: &Value, name: &str) -> bool {
    let aria = &node["aria"];
    aria["role"] == "Button"
        && aria["label"]
            .as_str()
            .is_some_and(|label| label.contains(name))
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
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
            window.debug_a11y_tree_json()
        })?
        .ok_or_else(|| io::Error::other("GPUI не построил дерево доступности."))?;
    Ok(serde_json::from_str(&data)?)
}

async fn wait<T>(
    cx: &mut AsyncApp,
    description: &str,
    ready: impl Fn(&mut AsyncApp) -> gpui::Result<Option<T>>,
) -> gpui::Result<T> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(value) = ready(cx)? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("Ожидалось состояние: {description}.")).into());
        }
        timer.timer(Duration::from_millis(35)).await;
    }
}

fn named(id: &str) -> String {
    format!("Name({id:?})")
}

fn by_element<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    tree["nodes"]
        .as_object()?
        .values()
        .find(|node| node["element_id"] == id)
}

fn focused(tree: &Value) -> Option<&str> {
    let id = tree["gpui_focus"].as_str()?;
    tree["nodes"][id]["element_id"].as_str()
}

fn press(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    focus(raw, id, cx)?;
    key(raw, "enter", cx)
}

fn focus(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let tree = snapshot(raw, cx)?;
    require(
        by_element(&tree, id).is_some(),
        &format!("В дереве GPUI отсутствует {id}."),
    )?;
    for _ in 0..=tree["frame"]["tab_stop_count"].as_u64().unwrap_or(64) {
        if focused(&snapshot(raw, cx)?) == Some(id) {
            return Ok(());
        }
        key(raw, "tab", cx)?;
    }
    Err(io::Error::other(format!("Фокус GPUI не дошёл до {id}.")).into())
}

fn key(raw: AnyWindowHandle, value: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    dispatch(raw, Keystroke::parse(value)?, cx)
}

fn dispatch(raw: AnyWindowHandle, key: Keystroke, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
    })
}

fn require(condition: bool, message: &str) -> gpui::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}
