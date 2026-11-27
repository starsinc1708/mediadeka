use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::{AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, WindowHandle};
use mediadeka::{
    CaptureLayout, MainView, PatternId, PatternSelection, VisualFlags, providers::ProviderId,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    Scene, draw, fail,
    home::activate_control,
    login_ux::{check_invalid_youtube, submit_invalid_youtube},
    native_dpi_change,
    sound::{accessibility, activate_accessibility, node},
};

const SETTINGS_SCENES: [Scene; 3] = [
    Scene {
        name: "wide",
        width: 1920.,
        height: 1080.,
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
    Scene {
        name: "minimum-150",
        width: 800.,
        height: 400.,
        dpi: 144,
        reduced: true,
    },
];
const HOME_TIMEOUT: Duration = Duration::from_secs(32);
const POLL: Duration = Duration::from_millis(40);
const DISCORD: &str = "settings-discord-enabled";
const LASTFM: &str = "settings-lastfm-enabled";
const LISTENBRAINZ: &str = "settings-listenbrainz-enabled";

pub(super) fn run(output: PathBuf, high_contrast: bool) -> gpui::Result<()> {
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    // The example owns the process. Isolate the profile before starting GPUI workers.
    unsafe { env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        // Keep the event loop alive between windows while testing both client sizes.
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
            for (index, scene) in SETTINGS_SCENES.into_iter().enumerate() {
                if let Err(error) = capture(scene, &output, index > 0, high_contrast, cx).await {
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
    restore: bool,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
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
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    window.update(cx, |view, window, cx| {
        if high_contrast {
            view.set_fluid_high_contrast_for_capture(true, cx);
        }
        view.start_home_for_capture(restore, window, cx);
    })?;
    let deadline = Instant::now() + HOME_TIMEOUT;
    while window
        .update(cx, |view, _, _| view.home_status_for_capture())?
        .is_none()
    {
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: не открылась гостевая оболочка.").into());
        }
        cx.background_executor().timer(POLL).await;
    }

    let settings_open = window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx);
        view.is_settings_for_capture()
    })?;
    if !settings_open {
        return Err(io::Error::other("GPUI: не открылись настройки подключений.").into());
    }
    activate_accessibility(raw, cx)?;
    wait_for_page_settings(raw, cx).await?;
    let settings_path = output.join(format!("settings-{}.png", scene.name));
    draw(raw, scene, Some(&settings_path), cx)?;
    let tree = accessibility(
        raw,
        &output.join(format!("settings-{}.json", scene.name)),
        cx,
    )?;
    require_node(&tree, "settings-connections")?;
    for section in [
        "settings-connections",
        "settings-library",
        "settings-integrations",
        "settings-appearance",
    ] {
        require_node(&tree, &format!("{section}-shortcut"))?;
    }
    capture_appearance(window, scene, output, cx)?;
    capture_artwork_clear(window, raw, scene, output, cx).await?;

    let profile = window.update(cx, |view, _, _| view.active_profile_for_capture())?;
    for (provider, keys, method) in [
        (ProviderId::Jellyfin, &["shift-tab", "enter"][..], "кнопка"),
        (ProviderId::SoundCloud, &["escape"][..], "Esc"),
        (ProviderId::YouTubeMusic, &["escape"][..], "Esc"),
    ] {
        if provider == ProviderId::YouTubeMusic
            && env::var_os("MEDIADEKA_GUI").is_none_or(|gui| gui != "3")
        {
            continue;
        }
        let modal_open = window.update(cx, |view, window, cx| {
            view.open_settings_login_for_capture(provider, window, cx);
            view.is_settings_for_capture() && view.is_settings_login_for_capture()
        })?;
        if !modal_open {
            return Err(io::Error::other(format!(
                "GPUI: вход {} открылся вне настроек.",
                provider.display_name()
            ))
            .into());
        }
        let modal_path = output.join(format!("settings-{}-{}.png", scene.name, provider.as_str()));
        draw(raw, scene, Some(&modal_path), cx)?;
        if provider == ProviderId::YouTubeMusic {
            submit_invalid_youtube(window, cx).await?;
            let stem = format!("settings-{}-youtube-invalid", scene.name);
            draw(raw, scene, Some(&output.join(format!("{stem}.png"))), cx)?;
            let error = accessibility(raw, &output.join(format!("{stem}-a11y.json")), cx)?;
            require_node(&error, "settings-login-modal")?;
            check_invalid_youtube(&error, true)?;
        }

        for key in keys {
            let key = Keystroke::parse(key)?;
            raw.update(cx, |_, window, cx| {
                window.dispatch_keystroke(key.clone(), cx);
                window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
            })?;
            draw(raw, scene, None, cx)?;
        }
        let returned = window.update(cx, |view, window, _| {
            view.is_settings_for_capture()
                && !view.is_settings_login_for_capture()
                && view.has_settings_provider_focus_for_capture(provider, window)
                && view.active_profile_for_capture() == profile
        })?;
        if !returned {
            return Err(io::Error::other(format!(
                "GPUI: {method} не восстановил настройки, источник и фокус {}.",
                provider.display_name()
            ))
            .into());
        }
        println!(
            "GPUI: {method} закрыл вход {}, сохранил настройки и источник, вернул фокус ({})",
            provider.display_name(),
            scene.name
        );
    }
    capture_section_jump(window, raw, scene, output, cx).await?;
    capture_listening_settings(window, scene, output, restore, cx).await?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn capture_appearance(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const GALLERY: &str = "appearance-gallery-toggle";
    const LIQUID_METAL: &str = "appearance-gallery-liquid-metal";
    let raw = AnyWindowHandle::from(window);
    activate_control(raw, "settings-appearance-shortcut", cx)?;
    let stem = format!("settings-appearance-{}", scene.name);
    draw(raw, scene, Some(&output.join(format!("{stem}.png"))), cx)?;
    let tree = accessibility(raw, &output.join(format!("{stem}-a11y.json")), cx)?;
    require_node(&tree, "settings-appearance")?;
    require_node(&tree, "appearance-shell-fluid")?;
    require_ordinary_settings(&tree)?;

    if window.update(cx, |view, _, _| view.uses_fluid_glass_for_capture())? {
        for id in [
            "appearance-palette-auto",
            "appearance-density-standard",
            "appearance-column-album",
            "appearance-pattern-auto",
            "appearance-pattern-liquid-metal",
            "appearance-intensity-slider",
            "appearance-speed-slider",
            "appearance-effects-group",
            "appearance-effect-16",
            "appearance-quality-select",
            "fluid-glass-live-preview-card",
            "appearance-copy-preset",
        ] {
            require_node(&tree, id)?;
        }
        capture_effect_dependency(window, scene, output, cx)?;
        activate_control(raw, GALLERY, cx)?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        activate_control(raw, LIQUID_METAL, cx)?;
        if window.update(cx, |view, _, _| view.visual_for_capture().pattern)?
            != PatternSelection::Fixed(PatternId::LiquidMetal)
        {
            return Err(
                io::Error::other("Галерея шейдеров не применила выбранный паттерн.").into(),
            );
        }
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(100_000., cx)
        })?;
        let stem = format!("settings-shader-gallery-{}", scene.name);
        draw(raw, scene, Some(&output.join(format!("{stem}.png"))), cx)?;
        let gallery = accessibility(raw, &output.join(format!("{stem}-a11y.json")), cx)?;
        require_node(&gallery, "appearance-gallery-grid")?;
        require_node(&gallery, LIQUID_METAL)?;
        require_ordinary_settings(&gallery)?;
        activate_control(raw, GALLERY, cx)?;
        activate_control(raw, "appearance-pattern-auto", cx)?;
    }
    println!(
        "GPUI: обычные настройки скрывают демонстрации и сохраняют рабочие разделы ({})",
        scene.name
    );
    Ok(())
}

fn require_ordinary_settings(tree: &Value) -> gpui::Result<()> {
    for id in [
        "appearance-icon-gallery",
        "editorial-menu-trigger",
        "shader-debug-toggle",
        "appearance-effect-1",
        "appearance-effect-8",
        "appearance-effect-32",
        "appearance-effect-64",
    ] {
        if node(tree, id).is_some() {
            return Err(
                io::Error::other(format!("В обычных настройках присутствует {id}.")).into(),
            );
        }
    }
    Ok(())
}

fn capture_effect_dependency(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const REFRACTION: &str = "appearance-effect-2";
    const DISPERSION: &str = "appearance-effect-4";
    const DEPENDENCY: &str = "Доступна при включённом преломлении";
    let raw = AnyWindowHandle::from(window);
    let original = window.update(cx, |view, _, _| view.visual_for_capture().effects)?;
    let refraction_enabled = original.enabled(VisualFlags::REFRACTION);
    for enabled in [!refraction_enabled, refraction_enabled] {
        activate_control(raw, REFRACTION, cx)?;
        if scene.width == 800.
            && !window.update(cx, |view, window, cx| {
                // Effects are directly above the audio section. Use its measured
                // position so the captured row is visible at both compact scales.
                view.reveal_audio_controls_for_capture(window, cx)
            })?
        {
            return Err(io::Error::other("Не измерена позиция строки эффектов.").into());
        }
        let state = if enabled { "on" } else { "off" };
        let stem = format!("settings-effects-{}-refraction-{state}", scene.name);
        draw(raw, scene, Some(&output.join(format!("{stem}.png"))), cx)?;
        let tree = accessibility(raw, &output.join(format!("{stem}-a11y.json")), cx)?;
        require_switch(&tree, REFRACTION, enabled)?;
        require_switch(
            &tree,
            DISPERSION,
            enabled && original.enabled(VisualFlags::DISPERSION),
        )?;
        require_switch(
            &tree,
            "appearance-effect-16",
            original.enabled(VisualFlags::BLOOM),
        )?;
        let dispersion = &require_node(&tree, DISPERSION)?["aria"];
        let clickable = dispersion["on_action"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action == "Click"));
        if clickable != enabled
            || dispersion["description"].as_str() != (!enabled).then_some(DEPENDENCY)
        {
            return Err(io::Error::other(
                "Дисперсия должна быть доступна только при включённом преломлении.",
            )
            .into());
        }
        let mut expected = original;
        expected.set(VisualFlags::REFRACTION, enabled);
        if window.update(cx, |view, _, _| view.visual_for_capture().effects)? != expected {
            return Err(io::Error::other("Преломление изменило настройки других эффектов.").into());
        }
    }
    println!(
        "GPUI: зависимость дисперсии и настройки эффектов сохранены ({})",
        scene.name
    );
    Ok(())
}

async fn capture_artwork_clear(
    window: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    const ACTION: &str = "settings-clear-artwork";
    const STATUS: &str = "settings-clear-artwork-status";
    const URL: &str = "https://i.scdn.co/image/artwork-clear-acceptance";
    let profile = PathBuf::from(env::var_os("MEDIADEKA_PROFILE_DIR").unwrap());
    let fixture = tempfile::NamedTempFile::new()?;
    let mut cover = vec![2];
    STANDARD.decode_vec(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAEElEQVR4AQEFAPr/AP8AAP8FAAH/+lyI0QAAAABJRU5ErkJggg==",
        &mut cover,
    )?;
    fs::write(fixture.path(), cover)?;
    MainView::seed_artist_artwork_for_capture(&profile, URL, fixture.path())?;
    let entry = profile
        .join("artwork/spotify")
        .join(format!("{:x}.cover", Sha256::digest(URL)));
    if !entry.is_file() {
        return Err(io::Error::other("Для проверки не сохранена обложка.").into());
    }
    let before = window.update(cx, |view, _, cx| {
        (
            view.active_profile_for_capture(),
            view.queue_order_for_capture(cx),
        )
    })?;
    activate_control(raw, "settings-library-shortcut", cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("artwork-cache-{}-before.png", scene.name))),
        cx,
    )?;
    activate_control(raw, ACTION, cx)?;
    let tree_path = output.join(format!("artwork-cache-{}-a11y.json", scene.name));
    let deadline = Instant::now() + Duration::from_secs(5);
    let tree = loop {
        draw(raw, scene, None, cx)?;
        let tree = accessibility(raw, &tree_path, cx)?;
        if node(&tree, STATUS).is_some_and(|node| node["aria"]["label"] == "Кэш обложек очищен.")
        {
            break tree;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Очистка обложек не подтвердилась в настройках.").into());
        }
        cx.background_executor().timer(POLL).await;
    };
    let focused = tree["gpui_focus"]
        .as_str()
        .is_some_and(|focus| tree["nodes"][focus]["element_id"] == format!("Name({ACTION:?})"));
    let after = window.update(cx, |view, _, cx| {
        (
            view.active_profile_for_capture(),
            view.queue_order_for_capture(cx),
        )
    })?;
    if entry.exists() || !focused || before != after {
        return Err(io::Error::other(
            "Очистка обложек изменила фокус, очередь или профиль либо оставила файл.",
        )
        .into());
    }
    draw(
        raw,
        scene,
        Some(&output.join(format!("artwork-cache-{}-after.png", scene.name))),
        cx,
    )?;
    println!(
        "GPUI: очистка обложек сохранила фокус, очередь и профиль ({})",
        scene.name
    );
    Ok(())
}

async fn capture_section_jump(
    window: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    window.update(cx, |view, _, cx| {
        view.set_home_scroll_offset_for_capture(0., cx)
    })?;
    draw(raw, scene, None, cx)?;
    activate_control(raw, "settings-integrations-shortcut", cx)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while window.update(cx, |view, _, _| view.home_scroll_offset_for_capture())? >= -1. {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Ярлык интеграций не прокрутил страницу.").into());
        }
        cx.background_executor().timer(POLL).await;
    }
    draw(
        raw,
        scene,
        Some(&output.join(format!("settings-{}-jump.png", scene.name))),
        cx,
    )?;
    let tree = accessibility(
        raw,
        &output.join(format!("settings-{}-jump.json", scene.name)),
        cx,
    )?;
    if tree["gpui_focus"]
        .as_str()
        .is_none_or(|focus| tree["nodes"][focus]["element_id"] != "Name(\"settings-integrations\")")
    {
        return Err(io::Error::other("Ярлык интеграций не передал фокус заголовку.").into());
    }
    Ok(())
}

async fn wait_for_page_settings(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + HOME_TIMEOUT;
    loop {
        let tree = raw
            .update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.debug_a11y_tree_json()
            })?
            .ok_or_else(|| io::Error::other("GPUI не построил дерево настроек."))?;
        let tree: Value = serde_json::from_str(&tree)?;
        if node(&tree, "settings-sound-download-loading").is_none()
            && node(&tree, "settings-genius-loading").is_none()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI не завершил чтение настроек страницы.").into());
        }
        cx.background_executor().timer(POLL).await;
    }
}

async fn capture_listening_settings(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    restore: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    activate_accessibility(raw, cx)?;
    window.update(cx, |view, _, cx| {
        view.load_listening_integrations_for_capture(cx)
    })?;
    wait_for_preferences(window, cx).await?;
    draw(raw, scene, None, cx)?;

    if !restore {
        let tree = integration_frame(window, scene, output, "default", 0., cx)?;
        require_node(&tree, "settings-discord-missing-id")?;
        for id in [DISCORD, LASTFM, LISTENBRAINZ] {
            require_switch(&tree, id, false)?;
        }

        // The hidden capture window has no playback and the profile exists only in this run.
        window.update(cx, |view, _, cx| {
            view.enter_discord_id_for_capture("42", cx)
        })?;
        draw(raw, scene, None, cx)?;
        activate_control(raw, "settings-discord-save", cx)?;
        wait_for_preferences(window, cx).await?;
        draw(raw, scene, None, cx)?;

        for id in [DISCORD, LASTFM] {
            activate_control(raw, id, cx)?;
            wait_for_preferences(window, cx).await?;
            draw(raw, scene, None, cx)?;
        }
        let tree = integration_frame(window, scene, output, "lastfm-enabled", 0., cx)?;
        require_switch(&tree, DISCORD, true)?;
        require_switch(&tree, LASTFM, true)?;
        require_switch(&tree, LISTENBRAINZ, false)?;

        for id in [
            LISTENBRAINZ,
            LASTFM,
            "settings-discord-paused",
            "settings-discord-private",
        ] {
            activate_control(raw, id, cx)?;
            wait_for_preferences(window, cx).await?;
            draw(raw, scene, None, cx)?;
        }
    }

    let tree = integration_frame(window, scene, output, "configured", 0., cx)?;
    require_node(&tree, "settings-discord-saved-id")?;
    require_switch(&tree, LASTFM, false)?;
    for id in [
        DISCORD,
        LISTENBRAINZ,
        "settings-discord-paused",
        "settings-discord-private",
    ] {
        require_switch(&tree, id, true)?;
    }
    if !require_node(&tree, "settings-listenbrainz-status")?["aria"]["label"]
        .as_str()
        .is_some_and(|label| label.contains("Отправка истории включится после подключения"))
    {
        return Err(io::Error::other("ListenBrainz не объясняет ожидание подключения.").into());
    }
    if scene.height < 500. {
        integration_frame(window, scene, output, "discord", 225., cx)?;
    }

    const MASKED_DRAFT: &str = "draft-mask-check";
    for (service, fields) in [
        ("lastfm", &["key", "secret"][..]),
        ("listenbrainz", &["token"][..]),
    ] {
        activate_control(raw, &format!("settings-{service}-connect"), cx)?;
        window.update(cx, |view, _, cx| {
            view.enter_scrobble_drafts_for_capture(MASKED_DRAFT, cx)
        })?;
        draw(raw, scene, None, cx)?;
        let from_bottom = if scene.height < 500. && service == "lastfm" {
            125.
        } else {
            0.
        };
        let tree = integration_frame(
            window,
            scene,
            output,
            &format!("{service}-form"),
            from_bottom,
            cx,
        )?;
        for field in fields {
            let id = format!("settings-{service}-{field}");
            let aria = &require_node(&tree, &id)?["aria"];
            if aria["role"] != "PasswordInput"
                || aria["value"].as_str().is_some_and(|v| !v.is_empty())
            {
                return Err(io::Error::other(format!("Не замаскировано поле {id}.")).into());
            }
        }
        activate_control(raw, &format!("settings-{service}-cancel"), cx)?;
        draw(raw, scene, None, cx)?;
        let tree = raw.update(cx, |_, window, _| window.debug_a11y_tree_json())?;
        let tree: Value = serde_json::from_str(
            tree.as_deref()
                .ok_or_else(|| io::Error::other("Дерево GPUI отсутствует."))?,
        )?;
        let focus = tree["gpui_focus"].as_str();
        let expected = format!("Name(\"settings-{service}-connect\")");
        if !focus.is_some_and(|id| tree["nodes"][id]["element_id"] == expected) {
            return Err(
                io::Error::other(format!("GPUI: отмена {service} не вернула фокус.")).into(),
            );
        }
    }
    println!(
        "GPUI: настройки Discord и два независимых скробблера, скрытые поля доступа ({})",
        scene.name
    );
    Ok(())
}

async fn wait_for_preferences(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let deadline = Instant::now() + HOME_TIMEOUT;
    while window.update(cx, |view, _, cx| {
        view.listening_integrations_busy_for_capture(cx)
    })? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: параметры интеграций не применились.").into());
        }
        cx.background_executor().timer(POLL).await;
    }
    Ok(())
}

fn integration_frame(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    state: &str,
    from_bottom: f32,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    window.update(cx, |view, _, cx| {
        view.reveal_listening_integrations_for_capture(from_bottom, cx)
    })?;
    let raw = AnyWindowHandle::from(window);
    let name = format!("integrations-{}-{state}", scene.name);
    draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
    accessibility(raw, &output.join(format!("{name}-a11y.json")), cx)
}

fn require_node<'a>(tree: &'a Value, id: &str) -> gpui::Result<&'a Value> {
    node(tree, id)
        .ok_or_else(|| io::Error::other(format!("В дереве GPUI отсутствует {id}.")).into())
}

fn require_switch(tree: &Value, id: &str, enabled: bool) -> gpui::Result<()> {
    let aria = &require_node(tree, id)?["aria"];
    let expected = if enabled { "True" } else { "False" };
    if aria["role"] != "Switch" || aria["toggled"] != expected {
        return Err(io::Error::other(format!("GPUI: состояние {id} не равно {expected}.")).into());
    }
    Ok(())
}
