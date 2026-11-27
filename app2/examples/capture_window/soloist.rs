use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp};
use mediadeka::CaptureLayout;

use super::{Scene, draw, fail, native_dpi_change};

const SCENES: [Scene; 2] = [
    Scene {
        name: "wide-100",
        width: 1500.,
        height: 850.,
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

pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Укажите каталог кадров Spotify Soloist."))?;
    let profile = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(io::Error::other("Лишний аргумент проверки Soloist.").into());
    }
    let clean = tempfile::tempdir()?;
    let directory = profile.as_deref().unwrap_or(clean.path());
    if profile.is_none() {
        mediadeka::seed_playlist_capture(directory, &[])?;
    }
    fs::create_dir_all(&output)?;
    // The CLI owns this isolated process and its profile.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", directory) };
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
            let _keep_profile = clean;
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

async fn capture(scene: Scene, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
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
    cx.background_executor()
        .timer(Duration::from_millis(160))
        .await;
    window.update(cx, |view, window, cx| {
        view.start_home_for_capture(true, window, cx)
    })?;
    let started = Instant::now();
    while !window.update(cx, |view, _, _| view.home_status_for_capture().is_some())? {
        if started.elapsed() >= Duration::from_secs(35) {
            return Err(io::Error::other("Медиатека не открылась.").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(80))
            .await;
    }
    window.update(cx, |view, window, cx| {
        view.open_soloist_for_capture(window, cx)
    })?;
    let start = Instant::now();
    let stage = loop {
        if let Some(stage) = window.update(cx, |view, _, _| view.soloist_stage_for_capture())? {
            break stage;
        }
        if start.elapsed() >= Duration::from_secs(25) {
            return Err(io::Error::other("Soloist не завершил проверку.").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(60))
            .await;
    };
    draw(raw, scene, None, cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("{}-{stage}.png", scene.name))),
        cx,
    )?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}
