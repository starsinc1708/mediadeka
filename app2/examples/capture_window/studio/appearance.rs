//! Native acceptance of the in-app shell and palette choice.
//!
//! The palette is selected through the keyboard in the settings page. The
//! shell and palette are then reopened in a separate process that reads the
//! same profile without `MEDIADEKA_GUI`, proving the saved choice survives a
//! restart.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, MouseButton, MouseMoveEvent, WindowHandle, point,
    px,
};
use mediadeka::{CaptureLayout, GlassPalette, MainView};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::super::{home::click, sound::accessibility};
use super::{
    super::{Scene, draw, fail, render_image},
    focus, focused, frame, key, named, node, node_label, press, require, snapshot, wait,
};

const SHELL_FLUID: &str = "fluid-glass";
const SHELL_CLASSIC: &str = "classic";
const ARTWORK: &str = "https://i.scdn.co/image/fluid-scroll-album";

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    window.update(cx, |view, _, cx| {
        view.enable_settings_diagnostics_for_capture(cx)
    })?;
    if env::var_os("MEDIADEKA_APPEARANCE_MENUS_ONLY").is_some() {
        return verify_menus_only(window, scene, output, cx).await;
    }
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(super::details::ALBUM_URL, window, cx)
    })?;
    wait(cx, super::details::ALBUM_TITLE, |cx| {
        let tree = snapshot(raw, cx)?;
        let decoded = window.update(cx, |view, _, cx| {
            view.artist_artwork_decoded_for_capture(
                "https://i.scdn.co/image/fluid-scroll-album",
                cx,
            )
        })?;
        Ok((decoded
            && node_label(&tree, "music-object-title") == Some(super::details::ALBUM_TITLE))
        .then_some(()))
    })
    .await?;
    // Put the decoded cover on the player so Auto has live artwork to read.
    window.update(cx, |view, _, cx| {
        view.set_fluid_glass_cover_for_capture(ARTWORK, cx)
    })?;
    // Let the artwork light and the page pattern settle before comparing palettes.
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::from_secs(10), cx)
    })?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("album-{}.png", scene.name))),
        cx,
    )?;
    let artwork_palette = window.update(cx, |view, _, cx| {
        view.fluid_glass_target_colors_for_capture(cx)
    })?;

    window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    wait(cx, "настройки", |cx| {
        Ok(window
            .update(cx, |view, _, _| view.is_settings_for_capture())?
            .then_some(()))
    })
    .await?;
    super::press(raw, &named("settings-appearance-shortcut"), cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("settings-appearance-{}.png", scene.name))),
        cx,
    )?;
    let tree = accessibility(
        raw,
        &output.join(format!("settings-appearance-{}.json", scene.name)),
        cx,
    )?;
    require_node(&tree, "settings-appearance")?;
    for id in ["appearance-shell-fluid", "appearance-shell-classic"] {
        require_node(&tree, id)?;
    }
    for palette in GlassPalette::ALL {
        require_node(&tree, &format!("appearance-palette-{}", palette.as_str()))?;
    }
    require(
        node(&tree, "appearance-restart-notice").is_none(),
        "Без смены оболочки предложение перезапуска не показывается.",
    )?;
    verify_track_display(raw, scene, output, profile, cx)?;

    // Keyboard selection must retarget the live material.
    let before = theme(window, cx)?;
    require(
        before == Some("auto"),
        "Начальная палитра должна быть авто.",
    )?;
    for palette in GlassPalette::ALL
        .into_iter()
        .filter(|p| *p != GlassPalette::Auto)
    {
        let name = palette.as_str();
        focus(raw, &named(&format!("appearance-palette-{name}")), cx)?;
        key(raw, "enter", cx)?;
        draw(
            raw,
            scene,
            Some(&output.join(format!("{name}-{}.png", scene.name))),
            cx,
        )?;
        require(
            theme(window, cx)? == Some(name),
            &format!("Клавиатура не применила палитру {name} сразу."),
        )?;
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(100_000., cx)
        })?;
        draw(
            raw,
            scene,
            Some(&output.join(format!("gallery-{name}.png"))),
            cx,
        )?;
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(0., cx)
        })?;
        press(raw, &named("settings-appearance-shortcut"), cx)?;
    }
    focus(raw, &named("appearance-palette-auto"), cx)?;
    key(raw, "enter", cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("auto-{}.png", scene.name))),
        cx,
    )?;
    require(theme(window, cx)? == Some("auto"), "Авто не вернулось.")?;
    require(
        window.update(cx, |view, _, cx| {
            view.fluid_glass_target_colors_for_capture(cx)
        })? == artwork_palette,
        "Авто не восстановило палитру текущей обложки без перехода.",
    )?;

    // Saving a different shell exposes the restart action.
    focus(raw, &named("appearance-shell-classic"), cx)?;
    key(raw, "enter", cx)?;
    wait(cx, "предложение перезапуска", |cx| {
        let tree = snapshot(raw, cx)?;
        Ok((node(&tree, "appearance-restart-notice").is_some()
            && node(&tree, "appearance-restart").is_some())
        .then_some(()))
    })
    .await?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("restart-{}.png", scene.name))),
        cx,
    )?;
    require(
        window
            .update(cx, |view, _, _| view.saved_appearance_for_capture())?
            .0
            == SHELL_CLASSIC,
        "Выбранная классическая оболочка не попала в состояние профиля.",
    )?;
    wait_for_saved(profile, SHELL_CLASSIC, "auto", cx).await?;
    let classic = reopen_saved(output, profile, SHELL_CLASSIC, "auto", cx).await?;
    require(
        classic["active_fluid_glass"] == false,
        "Перезапуск с сохранённой классической оболочкой открыл Fluid Glass.",
    )?;

    // Choose Fluid Glass and a fixed palette, then reopen again.
    focus(raw, &named("appearance-shell-fluid"), cx)?;
    key(raw, "enter", cx)?;
    focus(raw, &named("appearance-palette-ultraviolet"), cx)?;
    key(raw, "enter", cx)?;
    require(
        theme(window, cx)? == Some("ultraviolet"),
        "Палитра Ultraviolet не применилась после возврата Fluid Glass.",
    )?;
    wait_for_saved(profile, SHELL_FLUID, "ultraviolet", cx).await?;
    let fluid = reopen_saved(output, profile, SHELL_FLUID, "ultraviolet", cx).await?;
    require(
        fluid["active_fluid_glass"] == true && fluid["theme"] == "ultraviolet",
        "Перезапуск не восстановил Fluid Glass с сохранённой палитрой.",
    )?;

    focus(raw, &named("appearance-palette-auto"), cx)?;
    key(raw, "enter", cx)?;

    // Animated Component Gallery: all glyphs, explicit states and a real
    // Play->Pause morph captured at three points of its spring.
    window.update(cx, |view, _, cx| {
        view.set_home_scroll_offset_for_capture(100_000., cx)
    })?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("icon-gallery-{}.png", scene.name))),
        cx,
    )?;
    let gallery_tree = accessibility(
        raw,
        &output.join(format!("icon-gallery-{}.json", scene.name)),
        cx,
    )?;
    for id in [
        "appearance-icon-gallery",
        "appearance-icon-play",
        "appearance-icon-shuffle",
        "appearance-icon-like",
        "appearance-icon-copy",
        "appearance-icon-disabled-like",
        "appearance-icon-loading-copy",
    ] {
        require_node(&gallery_tree, id)?;
    }
    press(raw, &named("appearance-icon-play"), cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("icon-play-0ms-{}.png", scene.name))),
        cx,
    )?;
    cx.background_executor()
        .timer(Duration::from_millis(70))
        .await;
    draw(
        raw,
        scene,
        Some(&output.join(format!("icon-play-70ms-{}.png", scene.name))),
        cx,
    )?;
    cx.background_executor()
        .timer(Duration::from_millis(220))
        .await;
    draw(
        raw,
        scene,
        Some(&output.join(format!("icon-play-290ms-{}.png", scene.name))),
        cx,
    )?;
    verify_menus(window, scene, output, cx).await?;
    fs::write(
        output.join(format!("appearance-{}.json", scene.name)),
        serde_json::to_vec_pretty(&json!({
            "scene": scene.name,
            "keyboard_palettes": GlassPalette::ALL.map(GlassPalette::as_str),
            "editorial_menus": true,
            "restart_notice": true,
            "saved_shell": SHELL_FLUID,
            "saved_palette": "auto",
            "reopened_classic": classic,
            "reopened_fluid": fluid,
        }))?,
    )?;
    println!(
        "{}: палитры с клавиатуры, перезапуск классической и Fluid Glass оболочек проверены",
        scene.name
    );
    Ok(())
}

fn verify_track_display(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const DENSITY: &str = "appearance-density-comfortable";
    const COLUMN: &str = "appearance-column-album";
    press(raw, &named(DENSITY), cx)?;
    press(raw, &named(COLUMN), cx)?;
    let tree = frame(raw, scene, output, "track-display", cx)?;
    let connection = Connection::open(profile.join("mediadeka.sqlite3"))?;
    let saved: (String, u8) = connection.query_row(
        "SELECT density, hidden_columns FROM track_density WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    require(
        node(&tree, DENSITY).is_some_and(|choice| choice["aria"]["selected"] == true)
            && node(&tree, COLUMN).is_some_and(|choice| choice["aria"]["toggled"] == "False")
            && saved.0 == "comfortable"
            && saved.1 != 0,
        "Клавиатура должна применять и сохранять плотность и видимость колонок.",
    )?;
    fs::write(
        output.join("track-display.json"),
        serde_json::to_vec_pretty(&saved)?,
    )?;
    press(raw, &named("appearance-density-standard"), cx)?;
    press(raw, &named(COLUMN), cx)?;
    Ok(())
}

async fn verify_menus_only(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    wait(cx, "настройки", |cx| {
        Ok(window
            .update(cx, |view, _, _| view.is_settings_for_capture())?
            .then_some(()))
    })
    .await?;
    super::press(raw, &named("settings-appearance-shortcut"), cx)?;
    window.update(cx, |view, _, cx| {
        view.set_home_scroll_offset_for_capture(100_000., cx)
    })?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("icon-gallery-{}.png", scene.name))),
        cx,
    )?;
    verify_menus(window, scene, output, cx).await?;
    fs::write(
        output.join(format!("appearance-menus-{}.json", scene.name)),
        serde_json::to_vec_pretty(&json!({
            "scene": scene.name,
            "menus_only": true,
            "editorial_menus": true,
        }))?,
    )?;
    println!("{}: меню галереи проверены", scene.name);
    Ok(())
}

async fn verify_menus(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const TRIGGER: &str = "editorial-menu-trigger";
    const MENU: &str = "editorial-menu";
    const TOOLTIP: &str = "guise-tooltip";
    const SELECT: &str = "editorial-select";
    const OPTIONS: &str = "editorial-select-options";
    let raw = AnyWindowHandle::from(window);
    let anchor = window.update(cx, |view, _, _| view.editorial_menu_anchor_for_capture())?;
    require(
        anchor.size.width > px(0.) && anchor.size.height > px(0.),
        "Галерея не измерила кнопку действий.",
    )?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                position: anchor.center(),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
    })?;
    wait(
        cx,
        "подсказка действий галереи",
        |cx| {
            Ok(node_label(&snapshot(raw, cx)?, TOOLTIP)
                .is_some_and(|label| label == "Действия")
                .then_some(()))
        },
    )
    .await?;
    frame(raw, scene, output, "editorial-tooltip", cx)?;

    press(raw, &named(TRIGGER), cx)?;
    let opened = frame(raw, scene, output, "editorial-menu-open", cx)?;
    require(
        node(&opened, MENU).is_some() && node(&opened, TOOLTIP).is_none(),
        "Подсказка должна закрыться при открытии меню.",
    )?;
    if !scene.reduced {
        // The trigger moves when the settings page scrolls into place. Read
        // the live bounds after the opening frame, not the stale pre-scroll
        // copy used to aim the pointer.
        let anchor = window.update(cx, |view, _, _| view.editorial_menu_anchor_for_capture())?;
        let scale = scene.dpi as f32 / 96.;
        let x = ((f32::from(anchor.left()) + 20.) * scale).round() as u32;
        let y = ((f32::from(anchor.bottom()) + 13.) * scale).round() as u32;
        let mut samples = Vec::new();
        for (millis, name) in [
            (0, "editorial-menu-start"),
            (55, "editorial-menu-55ms"),
            (160, "editorial-menu"),
        ] {
            window.update(cx, |view, _, cx| {
                view.sample_editorial_menu_for_capture(Some(Duration::from_millis(millis)), cx)
            })?;
            let image = render_image(raw, scene, cx)?;
            image.save(output.join(format!("{name}.png")))?;
            samples.push(image::imageops::crop_imm(&image, x, y, 70, 21).to_image());
        }
        window.update(cx, |view, _, cx| {
            view.sample_editorial_menu_for_capture(None, cx)
        })?;
        require(
            samples.windows(2).all(|pair| pair[0] != pair[1]),
            "Кадр раскрытия на 55 мс должен отличаться от начального и конечного.",
        )?;
    } else {
        frame(raw, scene, output, "editorial-menu", cx)?;
    }
    key(raw, "escape", cx)?;
    let closed = snapshot(raw, cx)?;
    require(
        node(&closed, MENU).is_none() && focused(&closed) == Some(named(TRIGGER).as_str()),
        "Escape должен закрыть меню галереи и вернуть фокус.",
    )?;

    press(raw, &named(TRIGGER), cx)?;
    snapshot(raw, cx)?;
    click(raw, MouseButton::Left, point(px(8.), px(8.)), cx)?;
    let outside = snapshot(raw, cx)?;
    require(
        node(&outside, MENU).is_none() && focused(&outside) == Some(named(TRIGGER).as_str()),
        "Щелчок вне меню должен закрыть его и вернуть фокус.",
    )?;

    press(raw, &named(SELECT), cx)?;
    let selected = frame(raw, scene, output, "editorial-select", cx)?;
    require(
        node(&selected, OPTIONS).is_some()
            && node(&selected, "editorial-select-auto")
                .is_some_and(|row| row["aria"]["selected"] == true)
            && active_row(&selected) == Some("Автоматически"),
        "В списке галереи должен быть отмечен текущий вариант.",
    )?;
    let unavailable = node(&selected, "editorial-select-unavailable")
        .ok_or_else(|| io::Error::other("В галерее нет недоступной строки."))?;
    require(
        unavailable["aria"]["role"] == "MenuItem"
            && unavailable["aria"]["label"] == "Копировать ссылку · Ссылка недоступна"
            && unavailable["aria"]["description"] == "Ссылка недоступна"
            && unavailable["aria"]["on_action"]
                .as_array()
                .is_none_or(|actions| !actions.iter().any(|action| action == "Click")),
        &format!("Недоступная строка должна сообщать причину и не иметь Click: {unavailable}"),
    )?;
    for (key_name, expected) in [
        ("down", "Локальный файл"),
        ("down", "Каталог"),
        ("down", "Автоматически"),
        ("up", "Каталог"),
    ] {
        key(raw, key_name, cx)?;
        require(
            active_row(&snapshot(raw, cx)?) == Some(expected),
            &format!("Курсор не пропустил недоступную строку: ожидался {expected}."),
        )?;
    }
    let row = window
        .update(cx, |view, _, _| {
            view.editorial_select_row_bounds_for_capture(3)
        })?
        .ok_or_else(|| io::Error::other("Галерея не измерила недоступную строку."))?;
    click(raw, MouseButton::Left, row.center(), cx)?;
    let pointed = snapshot(raw, cx)?;
    require(
        node(&pointed, OPTIONS).is_some()
            && active_row(&pointed) == Some("Каталог")
            && node_label(&pointed, SELECT) == Some("Источник текста · Автоматически"),
        &format!(
            "Щелчок по недоступной строке не должен менять значение или закрывать меню: menu={}, active={:?}, trigger={:?}, row={:?}.",
            node(&pointed, OPTIONS).is_some(),
            active_row(&pointed),
            node_label(&pointed, SELECT),
            row,
        ),
    )?;
    key(raw, "up", cx)?;
    require(
        active_row(&snapshot(raw, cx)?) == Some("Локальный файл"),
        "Курсор не вернулся к предыдущей доступной строке.",
    )?;
    key(raw, "enter", cx)?;
    let changed = snapshot(raw, cx)?;
    require(
        node(&changed, OPTIONS).is_none()
            && node_label(&changed, SELECT) == Some("Источник текста · Локальный файл")
            && focused(&changed) == Some(named(SELECT).as_str()),
        "Выбор с клавиатуры должен изменить значение и восстановить фокус.",
    )
}

fn active_row(tree: &Value) -> Option<&str> {
    let id = tree["active_descendant_focus"].as_str()?;
    tree["nodes"][id]["aria"]["label"].as_str()
}

fn require_node(tree: &Value, id: &str) -> gpui::Result<()> {
    require(
        node(tree, id).is_some(),
        &format!("В дереве настроек нет {id}."),
    )
}

fn theme(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Option<&'static str>> {
    window.update(cx, |view, _, cx| view.fluid_glass_theme_for_capture(cx))
}

fn saved(profile: &Path) -> Option<(String, String)> {
    let connection = Connection::open(profile.join("mediadeka.sqlite3")).ok()?;
    connection
        .query_row(
            "SELECT shell, glass_palette FROM appearance WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
}

async fn wait_for_saved(
    profile: &Path,
    shell: &str,
    palette: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if saved(profile).is_some_and(|value| value == (shell.to_owned(), palette.to_owned())) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Профиль не сохранил оболочку {shell} и палитру {palette}."
            ))
            .into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
}

async fn reopen_saved(
    output: &Path,
    profile: &Path,
    shell: &str,
    palette: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let executable = env::current_exe()?;
    let directory = output.join(format!("reopen-{shell}"));
    fs::create_dir_all(&directory)?;
    let mut command = Command::new(executable);
    command
        .arg("--appearance-reopen")
        .arg(profile)
        .arg(&directory)
        .arg(shell)
        .arg(palette);
    for name in [
        "MEDIADEKA_GUI",
        "MEDIADEKA_GLASS_THEME",
        "MEDIADEKA_GLASS_EFFECTS",
        "MEDIADEKA_GLASS_QUALITY",
        "MEDIADEKA_PROFILE_DIR",
    ] {
        command.env_remove(name);
    }
    let result = command.output()?;
    fs::write(directory.join("stdout.txt"), &result.stdout)?;
    fs::write(directory.join("stderr.txt"), &result.stderr)?;
    require(
        result.status.success(),
        &format!(
            "Перезапуск {shell} завершился с {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ),
    )?;
    let report: Value = serde_json::from_slice(&fs::read(directory.join("reopen.json"))?)?;
    require(
        report["saved_shell"] == shell
            && report["saved_palette"] == palette
            && report["active_fluid_glass"] == (shell == SHELL_FLUID)
            && (shell != SHELL_FLUID || report["theme"] == palette),
        "Перезапущенный процесс не восстановил сохранённое оформление.",
    )?;
    cx.background_executor()
        .timer(Duration::from_millis(10))
        .await;
    Ok(report)
}

/// A normal launch of the capture executable without `MEDIADEKA_GUI` on the
/// same profile. The window is hidden; the report is the restart evidence.
pub(crate) fn reopen(args: impl Iterator<Item = std::ffi::OsString>) -> gpui::Result<()> {
    let mut args = args;
    let usage = || {
        io::Error::other("--appearance-reopen <профиль> <каталог> <fluid-glass|classic> <палитра>")
    };
    let profile = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let output = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let shell = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    let palette = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage().into());
    }
    fs::create_dir_all(&output)?;
    unsafe { env::set_var("MEDIADEKA_PROFILE_DIR", &profile) };
    mediadeka::application().run(move |cx| {
        let window = mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: 1100.,
                height: 700.,
                reduced_motion: true,
            },
        )
        .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let result = (|| -> gpui::Result<Value> {
                let (saved_shell, saved_palette) =
                    window.update(cx, |view, _, _| view.saved_appearance_for_capture())?;
                let active_fluid_glass =
                    window.update(cx, |view, _, _| view.uses_fluid_glass_for_capture())?;
                let theme =
                    window.update(cx, |view, _, cx| view.fluid_glass_theme_for_capture(cx))?;
                let visual = window.update(cx, |view, _, _| view.visual_for_capture())?;
                let scene = Scene {
                    name: "reopened",
                    width: 1100.,
                    height: 700.,
                    dpi: 96,
                    reduced: true,
                };
                draw(
                    AnyWindowHandle::from(window),
                    scene,
                    Some(&output.join("reopened.png")),
                    cx,
                )?;
                let report = json!({
                    "saved_shell": saved_shell,
                    "saved_palette": saved_palette,
                    "active_fluid_glass": active_fluid_glass,
                    "theme": theme,
                    "visual": {
                        "pattern": visual.pattern.as_str(),
                        "intensity": visual.intensity,
                        "speed": visual.speed,
                        "effects": visual.effects.bits(),
                        "quality": visual.quality.as_str(),
                        "surface": { "opacity": visual.surface.opacity, "blur": visual.surface.blur, "neutral": visual.surface.neutral },
                    },
                    "expected_shell": shell,
                    "expected_palette": palette,
                });
                fs::write(
                    output.join("reopen.json"),
                    serde_json::to_vec_pretty(&report)?,
                )?;
                require(
                    saved_shell == shell
                        && saved_palette == palette
                        && active_fluid_glass == (shell == SHELL_FLUID)
                        && (shell != SHELL_FLUID || theme == Some(palette.as_str())),
                    &format!("Новый процесс не совпал с сохранённым оформлением: {report}"),
                )?;
                Ok(report)
            })();
            let _ = window.update(cx, |_, window, _| window.remove_window());
            match result {
                Ok(report) => {
                    println!("Перезапуск восстановил оформление: {report}");
                    cx.update(|cx| cx.quit());
                }
                Err(error) => fail(format!("{error:#}")),
            }
        })
        .detach();
    });
    Ok(())
}
