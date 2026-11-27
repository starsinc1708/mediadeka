use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use mediadeka::MainView;
use serde_json::Value;

use super::{
    home::activate_control,
    sound::{accessibility, activate_accessibility},
};

const STATE_TIMEOUT: Duration = Duration::from_secs(6);
const FEED_TIMEOUT: Duration = Duration::from_secs(32);
const POLL: Duration = Duration::from_millis(40);

pub(super) fn run(output: PathBuf) -> gpui::Result<()> {
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    // The example owns the process and scopes all storage before GPUI starts.
    unsafe { env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let window = mediadeka::open_main_window(cx).unwrap_or_else(|error| super::fail(error));
        cx.activate(true);
        cx.spawn(async move |cx| {
            let _profile = profile;
            if let Err(error) = capture(window, &output, cx).await {
                super::fail(error);
            }
        })
        .detach();
    });
    Ok(())
}

async fn capture(
    main: WindowHandle<MainView>,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    let raw = AnyWindowHandle::from(main);
    activate_accessibility(raw, cx)?;
    let deadline = Instant::now() + STATE_TIMEOUT;
    loop {
        let welcome = main.update(cx, |view, _, _| view.is_welcome_for_capture())?;
        if welcome
            && accessibility_frame(raw, cx)?
                .is_some_and(|tree| accessible(&tree, "Button", "Диагностика"))
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: в приветствии нет кнопки диагностики.").into());
        }
        timer.timer(POLL).await;
    }
    capture_frame(raw, &output.join("welcome-diagnostics-button.png"), cx)?;
    activate_control(raw, "diagnostics", cx)?;
    let deadline = Instant::now() + STATE_TIMEOUT;
    let diagnostics = loop {
        if let Some(diagnostics) =
            main.update(cx, |view, _, _| view.diagnostics_window_for_capture())?
        {
            break diagnostics;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: кнопка не открыла окно диагностики.").into());
        }
        timer.timer(POLL).await;
    };

    main.update(cx, |view, window, cx| {
        view.start_home_for_capture(false, window, cx)
    })?;
    let deadline = Instant::now() + FEED_TIMEOUT;
    while main.update(cx, |view, _, _| view.home_status_for_capture())? != Some("fresh") {
        if Instant::now() >= deadline {
            return Err(io::Error::other("GPUI: гостевая подборка не загрузилась.").into());
        }
        timer.timer(POLL).await;
    }
    main.update(cx, |view, window, cx| {
        view.return_home_from_listening_fixture(window, cx);
    })?;
    timer.timer(Duration::from_millis(350)).await;
    capture_frame(raw, &output.join("home-diagnostics-button.png"), cx)?;
    let position = main.update(cx, |view, _, _| {
        view.home_scroll_bounds_for_capture().center()
    })?;
    let before = main.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
    // Exercise the real platform frame loop while collecting timings from the main window.
    for _ in 0..24 {
        raw.update(cx, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-12.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })?;
        timer.timer(POLL).await;
    }
    let after = main.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
    if (before - after).abs() < 10. {
        return Err(io::Error::other("GPUI: прокрутка для диагностики не продвинулась.").into());
    }
    activate_control(raw, "diagnostics", cx)?;
    timer.timer(POLL).await;
    if main.update(cx, |view, _, _| view.diagnostics_window_for_capture())? != Some(diagnostics) {
        return Err(
            io::Error::other("GPUI: повторный щелчок открыл второе окно диагностики.").into(),
        );
    }
    let activity = main.update(cx, |view, _, cx| view.diagnostics_activity_for_capture(cx))?;
    if activity != Some((true, true)) {
        return Err(io::Error::other(format!(
            "GPUI: в окне диагностики нет истории кадров и полученных данных: {activity:?}."
        ))
        .into());
    }
    println!(
        "GPUI: прокрутка {before:.1} → {after:.1}; окно диагностики сохранило свой идентификатор."
    );
    capture_frame(diagnostics, &output.join("diagnostics-live.png"), cx)?;
    activate_accessibility(diagnostics, cx)?;
    diagnostics.update(cx, |_, window, cx| {
        let viewport = window.viewport_size();
        window.dispatch_event(
            ScrollWheelEvent {
                position: point(viewport.width / 2., viewport.height / 2.),
                delta: ScrollDelta::Pixels(point(px(0.), px(-360.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    capture_frame(
        diagnostics,
        &output.join("diagnostics-catalog-cache.png"),
        cx,
    )?;
    let tree = accessibility(
        diagnostics,
        &output.join("diagnostics-catalog-cache-a11y.json"),
        cx,
    )?;
    if !accessible(&tree, "Group", "Кэш каталога") {
        return Err(io::Error::other("GPUI: в диагностике нет семантики кэша каталога.").into());
    }
    let gpu = tree["nodes"].as_object().and_then(|nodes| {
        nodes.values().find_map(|node| {
            (node["aria"]["role"] == "Label")
                .then(|| node["aria"]["label"].as_str())
                .flatten()
                .and_then(|label| label.strip_prefix("Выделенная GPU-память процесса · "))
        })
    });
    let gpu = gpu.ok_or_else(|| io::Error::other("GPUI: в диагностике нет счётчика GPU."))?;
    if env::var_os("MEDIADEKA_CAPTURE_REQUIRE_GPU_MEMORY").is_some() && gpu == "—" {
        return Err(io::Error::other("Windows не вернула выделенную GPU-память процесса.").into());
    }
    println!("Выделенная GPU-память процесса · {gpu}");
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn accessibility_frame(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<Option<Value>> {
    let tree = raw.update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.debug_a11y_tree_json()
    })?;
    Ok(tree.as_deref().map(serde_json::from_str).transpose()?)
}

fn accessible(tree: &Value, role: &str, label: &str) -> bool {
    tree["nodes"].as_object().is_some_and(|nodes| {
        nodes
            .values()
            .any(|node| node["aria"]["role"] == role && node["aria"]["label"] == label)
    })
}

fn capture_frame(raw: AnyWindowHandle, path: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        let frame = window.render_to_image()?;
        let scale = window.scale_factor();
        let size = window.viewport_size();
        if frame.width() != (f32::from(size.width) * scale).round() as u32
            || frame.height() != (f32::from(size.height) * scale).round() as u32
        {
            return Err(io::Error::other("GPUI: размер кадра не совпал с размером окна.").into());
        }
        frame.save(path)?;
        println!(
            "{} × {} @ {:.0}% {}",
            frame.width(),
            frame.height(),
            scale * 100.,
            path.display(),
        );
        Ok(())
    })?
}
