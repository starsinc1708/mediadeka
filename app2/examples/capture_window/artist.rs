use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::{CaptureLayout, MainView};

use super::{Scene, draw, fail, native_dpi_change};

const SCENE: Scene = Scene {
    name: "artist-artwork",
    width: 1500.,
    height: 850.,
    dpi: 96,
    reduced: true,
};
const LIGHT: &str = "https://i.scdn.co/image/artist-capture-light";
const DARK: &str = "https://i.scdn.co/image/artist-capture-dark";
const PANORAMA: &str = "https://i.scdn.co/image/artist-capture-panorama";

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let usage = || {
        io::Error::other("--artist-artwork <каталог> <light.cover> <dark.cover> <panorama.cover>")
    };
    let output = PathBuf::from(arguments.next().ok_or_else(usage)?);
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    for url in [LIGHT, DARK, PANORAMA] {
        let fixture = PathBuf::from(arguments.next().ok_or_else(usage)?);
        MainView::seed_artist_artwork_for_capture(profile.path(), url, &fixture)?;
    }
    if arguments.next().is_some() {
        return Err(usage().into());
    }
    // The capture process owns its isolated profile before GPUI starts workers.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let window = mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: SCENE.width,
                height: SCENE.height,
                reduced_motion: SCENE.reduced,
            },
        )
        .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            capture(window, &output, cx)
                .await
                .unwrap_or_else(|error| fail(error));
            let _ = window.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn capture(
    window: WindowHandle<MainView>,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, _| native_dpi_change(window, SCENE))??
        .apply();
    window.update(cx, |view, window, cx| {
        view.start_home_for_capture(false, window, cx)
    })?;
    let deadline = Instant::now() + Duration::from_secs(32);
    while window
        .update(cx, |view, _, _| view.home_status_for_capture())?
        .is_none()
    {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Не открылась гостевая оболочка.").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
    for (name, portrait, banner) in [
        ("light", Some(LIGHT), None),
        ("dark", Some(DARK), None),
        ("missing", None, None),
        ("panorama", Some(DARK), Some(PANORAMA)),
    ] {
        window.update(cx, |view, _, cx| {
            view.show_artist_artwork_for_capture(portrait, banner, cx)
        })?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            draw(raw, SCENE, None, cx)?;
            if window.update(cx, |view, _, cx| {
                portrait
                    .into_iter()
                    .chain(banner)
                    .all(|url| view.artist_artwork_decoded_for_capture(url, cx))
            })? {
                break;
            }
            if Instant::now() >= deadline {
                return Err(
                    io::Error::other(format!("Не декодированы обложки артиста: {name}")).into(),
                );
            }
            cx.background_executor()
                .timer(Duration::from_millis(40))
                .await;
        }
        draw(
            raw,
            SCENE,
            Some(&output.join(format!("artist-{name}.png"))),
            cx,
        )?;
    }
    Ok(())
}
