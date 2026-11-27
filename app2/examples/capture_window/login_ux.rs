use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle};
use mediadeka::{CaptureLayout, MainView, providers::ProviderId};
use serde_json::{Value, json};

use super::{
    Scene, capture_high_contrast, draw, fail,
    home::activate_control,
    native_dpi_change,
    sound::{accessibility, activate_accessibility, node, node_label},
};

const SCENES: [Scene; 2] = [
    Scene {
        name: "1920x1080",
        reduced: true,
        ..super::SCENES[0]
    },
    Scene {
        name: "800x400",
        reduced: true,
        ..super::SCENES[2]
    },
];

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("--login-ux <каталог кадров> [--high-contrast]"))?;
    let high_contrast = capture_high_contrast(args)?;
    let profile = tempfile::tempdir()?;
    // The hidden runner owns the process and isolates the store before creating workers.
    unsafe { env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
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
            for scene in SCENES {
                if let Err(error) =
                    capture(scene, &output.join(scene.name), high_contrast, cx).await
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

async fn capture(
    scene: Scene,
    output: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let view = cx.update(|cx| {
        mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: scene.width,
                height: scene.height,
                reduced_motion: true,
            },
        )
    })?;
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    if high_contrast {
        view.update(cx, |view, _, cx| {
            view.set_fluid_high_contrast_for_capture(true, cx)
        })?;
    }
    activate_accessibility(raw, cx)?;
    draw(raw, scene, Some(&output.join("welcome.png")), cx)?;
    let welcome = accessibility(raw, &output.join("welcome.json"), cx)?;
    ensure(
        node(&welcome, "guest")
            .is_some_and(|node| node["aria"]["description"] == "Публичные подборки без аккаунта"),
        "Гостевой вход не объясняет доступную подборку.",
    )?;

    view.update(cx, |view, window, cx| {
        view.select_connection_for_capture(ProviderId::Jellyfin, window, cx)
    })?;
    draw(raw, scene, None, cx)?;
    for _ in 0..2 {
        key(raw, "tab", cx)?;
    }
    draw(raw, scene, Some(&output.join("jellyfin-password.png")), cx)?;
    let jellyfin = accessibility(raw, &output.join("jellyfin-password.json"), cx)?;
    let setup_visible = check_jellyfin(&jellyfin, view, cx)?;
    activate_control(raw, "jellyfin-submit", cx)?;
    wait_failure(view, cx).await?;
    draw(raw, scene, None, cx)?;
    focus(raw, "jellyfin-password", cx)?;
    draw(raw, scene, Some(&output.join("jellyfin-error.png")), cx)?;
    let jellyfin_error = accessibility(raw, &output.join("jellyfin-error.json"), cx)?;
    let error_visible = check_jellyfin(&jellyfin_error, view, cx)?;
    ensure(
        node(&jellyfin_error, "login-error").is_some(),
        "Не показана ошибка адреса Jellyfin.",
    )?;

    key(raw, "escape", cx)?;
    view.update(cx, |view, window, cx| {
        view.select_connection_for_capture(ProviderId::YouTubeMusic, window, cx)
    })?;
    draw(raw, scene, Some(&output.join("youtube.png")), cx)?;
    let youtube = accessibility(raw, &output.join("youtube.json"), cx)?;
    ensure(
        node(&youtube, "youtube-help-1").is_none(),
        "Техническая подсказка раскрыта при входе.",
    )?;
    activate_control(raw, "choose-youtube", cx)?;
    draw(raw, scene, Some(&output.join("youtube-empty.png")), cx)?;
    let empty = accessibility(raw, &output.join("youtube-empty.json"), cx)?;
    ensure(
        node_label(&empty, "youtube-session-error")
            == Some("Вставьте значение Cookie из YouTube Music.")
            && has_focus(&empty, "youtube-session"),
        "Пустой Cookie не получил пояснение у поля и фокус.",
    )?;
    submit_invalid_youtube(view, cx).await?;
    draw(raw, scene, Some(&output.join("youtube-invalid.png")), cx)?;
    let invalid = accessibility(raw, &output.join("youtube-invalid.json"), cx)?;
    let fluid = env::var_os("MEDIADEKA_GUI").is_some_and(|gui| gui == "3");
    check_invalid_youtube(&invalid, fluid)?;
    activate_control(raw, "youtube-help-toggle", cx)?;
    draw(raw, scene, None, cx)?;
    draw(raw, scene, Some(&output.join("youtube-help.png")), cx)?;
    let help = accessibility(raw, &output.join("youtube-help.json"), cx)?;
    ensure(
        (1..=3).all(|step| node(&help, &format!("youtube-help-{step}")).is_some()),
        "Подсказка YouTube Music раскрылась не полностью.",
    )?;
    let help_overflow = view
        .update(cx, |view, _, _| view.login_scroll_for_capture())?
        .map_or(0., |(extent, _)| extent);
    if help_overflow > 1. {
        key(raw, "shift-tab", cx)?;
        draw(
            raw,
            scene,
            Some(&output.join("youtube-help-input-focus.png")),
            cx,
        )?;
        let restored = accessibility(raw, &output.join("youtube-help-input-focus.json"), cx)?;
        ensure(
            has_focus(&restored, "youtube-session")
                && view
                    .update(cx, |view, _, _| view.login_scroll_for_capture())?
                    .is_some_and(|(_, visible)| visible),
            "Прокрутка не раскрыла поле ввода после перехода с подсказки.",
        )?;
    }
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "jellyfin_password_setup_visible": setup_visible,
            "jellyfin_password_error_visible": error_visible,
            "youtube_empty_and_invalid": true,
            "youtube_invalid_input_focused": has_focus(&invalid, "youtube-session"),
            "youtube_help_overflow_dip": help_overflow,
            "youtube_focus_revealed_after_scroll": help_overflow > 1.,
        }))?,
    )?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

pub(super) async fn submit_invalid_youtube(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = view.into();
    for stroke in ["w", "r", "o", "n", "g"] {
        key(raw, stroke, cx)?;
    }
    activate_control(raw, "choose-youtube", cx)?;
    wait_failure(view, cx).await
}

pub(super) fn check_invalid_youtube(tree: &Value, fluid: bool) -> gpui::Result<()> {
    ensure(
        node_label(tree, "youtube-session-error")
            .is_some_and(|label| label.contains("значение заголовка Cookie"))
            && node(tree, "login-error").is_none()
            && (!fluid || has_focus(tree, "youtube-session")),
        "Невалидное значение Cookie не объяснено рядом с полем или Fluid Glass не вернул фокус.",
    )
}

fn check_jellyfin(
    tree: &Value,
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<bool> {
    let visible = view
        .update(cx, |view, _, _| view.login_scroll_for_capture())?
        .is_some_and(|(_, visible)| visible);
    ensure(
        has_focus(tree, "jellyfin-password")
            && visible
            && node_label(tree, "jellyfin-username") == Some("Имя пользователя")
            && node_label(tree, "jellyfin-password") == Some("Пароль"),
        "Поле пароля Jellyfin обрезано, не в фокусе или отсутствует доступное имя.",
    )?;
    Ok(visible)
}

async fn wait_failure(view: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !view.update(cx, |view, _, _| view.failed_connection_for_capture())? {
        ensure(
            Instant::now() < deadline,
            "Форма не вернула ожидаемую ошибку.",
        )?;
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    Ok(())
}

fn key(raw: AnyWindowHandle, stroke: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let key = Keystroke::parse(stroke)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}

fn has_focus(tree: &Value, id: &str) -> bool {
    tree["gpui_focus"]
        .as_str()
        .is_some_and(|focus| tree["nodes"][focus]["element_id"] == format!("Name({id:?})"))
}

fn focus(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    for _ in 0..32 {
        let snapshot = raw
            .update(cx, |_, window, _| window.debug_a11y_tree_json())?
            .ok_or_else(|| io::Error::other("GPUI не построила дерево входа."))?;
        if has_focus(&serde_json::from_str(&snapshot)?, id) {
            return Ok(());
        }
        key(raw, "tab", cx)?;
    }
    Err(io::Error::other(format!("Фокус не дошёл до {id}.")).into())
}

fn ensure(condition: bool, reason: &'static str) -> gpui::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(reason).into())
    }
}
