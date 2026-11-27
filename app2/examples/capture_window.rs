use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, Window, WindowHandle};
use mediadeka::{CaptureLayout, providers::ProviderId};

#[cfg(windows)]
#[path = "capture_window/artist.rs"]
mod artist;
#[cfg(windows)]
#[path = "capture_window/catalog_ux.rs"]
mod catalog_ux;
#[cfg(windows)]
#[path = "capture_window/diagnostics.rs"]
mod diagnostics;
#[cfg(windows)]
#[path = "capture_window/home.rs"]
mod home;
#[cfg(windows)]
#[path = "capture_window/liked.rs"]
mod liked;
#[cfg(windows)]
#[path = "capture_window/live_motion.rs"]
mod live_motion;
#[cfg(windows)]
#[path = "capture_window/login_ux.rs"]
mod login_ux;
#[cfg(windows)]
#[path = "capture_window/lyrics.rs"]
mod lyrics;
#[cfg(windows)]
#[path = "capture_window/music.rs"]
mod music;
#[cfg(windows)]
#[path = "capture_window/music_concept.rs"]
mod music_concept;
#[cfg(windows)]
#[path = "capture_window/player.rs"]
mod player;
#[cfg(windows)]
#[path = "capture_window/playlist.rs"]
mod playlist;
#[cfg(windows)]
#[path = "capture_window/scroll_viewport.rs"]
mod scroll_viewport;
#[cfg(windows)]
#[path = "capture_window/settings.rs"]
mod settings;
#[cfg(windows)]
#[path = "capture_window/shell.rs"]
mod shell;
#[cfg(windows)]
#[path = "capture_window/soloist.rs"]
mod soloist;
#[cfg(windows)]
#[path = "capture_window/sound.rs"]
mod sound;
#[cfg(windows)]
#[path = "capture_window/studio.rs"]
mod studio;

#[derive(Clone, Copy)]
struct Scene {
    name: &'static str,
    width: f32,
    height: f32,
    dpi: u32,
    reduced: bool,
}

const SCENES: &[Scene] = &[
    Scene {
        name: "desktop-100",
        width: 1920.,
        height: 1080.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "wide-100",
        width: 1100.,
        height: 700.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "minimum-100",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "narrow-tall-100",
        width: 800.,
        height: 700.,
        dpi: 96,
        reduced: false,
    },
    Scene {
        name: "minimum-150",
        width: 800.,
        height: 400.,
        dpi: 144,
        reduced: false,
    },
    Scene {
        name: "reduced-100",
        width: 1100.,
        height: 700.,
        dpi: 96,
        reduced: true,
    },
];
const SERVICES: [&str; 5] = ["spotify", "youtube", "yandex", "soundcloud", "jellyfin"];

fn main() -> gpui::Result<()> {
    let mut arguments = env::args_os().skip(1);
    let output = arguments.next().map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Укажите каталог для кадров Mediadeka.",
        )
    })?;
    #[cfg(windows)]
    if output.as_os_str() == "--home" {
        return home::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--login-ux" {
        return login_ux::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--playlist" {
        return playlist::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--player" {
        return player::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--sound-stage5" {
        return sound::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--lyrics-stage6" {
        return lyrics::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--lyrics-genius-stage6" {
        return lyrics::run_genius(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--music-pages" {
        return music::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--music-concept" {
        return music_concept::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--fluid-glass" || output.as_os_str() == "--studio" {
        return studio::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--appearance-reopen" {
        return studio::reopen(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--liked-audit" {
        return liked::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--library-stage4" {
        return liked::run_stage_four(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--catalog-ux" {
        return catalog_ux::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--artist-artwork" {
        return artist::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--soloist" {
        return soloist::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--seed-soloist-startup" {
        let directory = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Укажите изолированный профиль Soloist."))?;
        if arguments.next().is_some() {
            return Err(io::Error::other("Лишний аргумент проверки запуска Soloist.").into());
        }
        return mediadeka::seed_soloist_startup_capture(&directory);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--seed-playlist" {
        return playlist::seed(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--playlist-source" {
        return playlist::source(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--diagnostics" {
        let evidence = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Укажите каталог для кадров диагностики Mediadeka."))?;
        if arguments.next().is_some() {
            return Err(io::Error::other("Лишний аргумент после каталога диагностики.").into());
        }
        return diagnostics::run(evidence);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--live-motion" {
        let evidence = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Укажите каталог для проверки движения Mediadeka."))?;
        return live_motion::run(evidence);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--visibility-power" {
        let evidence = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Укажите каталог для проверки скрытого окна."))?;
        if arguments.next().is_some() {
            return Err(io::Error::other("Лишний аргумент после каталога проверки.").into());
        }
        return studio::visibility_power::run(evidence);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--shell" {
        return shell::run(arguments);
    }
    #[cfg(windows)]
    if output.as_os_str() == "--settings" {
        let evidence = arguments
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("Укажите каталог для кадров настроек Mediadeka."))?;
        return settings::run(evidence, capture_high_contrast(arguments)?);
    }
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    // This tool owns the process. Redirect the store before GPUI starts workers.
    unsafe { env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        // Keep the GPUI event loop alive while replacing hidden windows.
        let anchor = match mediadeka::open_capture_window(
            cx,
            CaptureLayout {
                width: 800.,
                height: 400.,
                reduced_motion: true,
            },
        ) {
            Ok(window) => window,
            Err(error) => fail(error),
        };
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                if let Err(error) = capture(*scene, &output, cx).await {
                    fail(error);
                }
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

#[cfg(windows)]
fn capture_high_contrast(mut args: impl Iterator<Item = std::ffi::OsString>) -> gpui::Result<bool> {
    match args.next() {
        None => Ok(false),
        Some(option) if option == "--high-contrast" && args.next().is_none() => {
            if env::var_os("MEDIADEKA_GUI").is_some_and(|gui| gui == "3") {
                Ok(true)
            } else {
                Err(io::Error::other("Для --high-contrast требуется MEDIADEKA_GUI=3.").into())
            }
        }
        _ => Err(io::Error::other("После каталога кадров допустим только --high-contrast.").into()),
    }
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
    let dpi_change = raw.update(cx, |_, window, _| native_dpi_change(window, scene))??;
    dpi_change.apply();
    cx.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    let directory = output.join(scene.name);
    fs::create_dir_all(&directory)?;
    sample(window, 0, None, cx)?;
    draw(raw, scene, None, cx)?;
    draw(raw, scene, Some(&directory.join("welcome.png")), cx)?;
    for ms in [750, 1500, 3000] {
        sample(window, ms, None, cx)?;
        draw(
            raw,
            scene,
            Some(&directory.join(format!("background-{ms}ms.png"))),
            cx,
        )?;
    }
    if scene.width > 800. {
        let mut ms = 3200;
        for (index, service) in SERVICES.iter().enumerate() {
            sample(window, ms, Some(index), cx)?;
            draw(raw, scene, None, cx)?;
            if index == 0 && !scene.reduced {
                for (elapsed, name) in [
                    (16, "spring-16ms"),
                    (64, "spring-64ms"),
                    (120, "spring-120ms"),
                ] {
                    sample(window, ms + elapsed, Some(index), cx)?;
                    draw(raw, scene, Some(&directory.join(format!("{name}.png"))), cx)?;
                }
            }
            sample(window, ms + 380, Some(index), cx)?;
            draw(
                raw,
                scene,
                Some(&directory.join(format!("hover-{service}.png"))),
                cx,
            )?;
            ms += 450;
        }
        sample(window, 6000, None, cx)?;
        draw(raw, scene, None, cx)?;
        sample(window, 6500, None, cx)?;
        draw(
            raw,
            scene,
            Some(&directory.join("background-6500ms.png")),
            cx,
        )?;
        if scene.name == "wide-100" {
            let mut times = Vec::new();
            for i in 0..30 {
                let start = Instant::now();
                sample(
                    window,
                    6500 + (i as u64 + 1) * 16,
                    Some(i % SERVICES.len()),
                    cx,
                )?;
                draw(raw, scene, None, cx)?;
                times.push(start.elapsed().as_secs_f64() * 1000.);
            }
            times.sort_by(f64::total_cmp);
            let message = format!(
                "30 consecutive switches, GPUI build + D3D11 readback, ms: median {:.2}, p95 {:.2}, max {:.2}\n",
                times[15], times[28], times[29],
            );
            print!("{message}");
            fs::write(directory.join("rapid-switch.txt"), message)?;
        }
    }
    capture_connection(window, raw, scene, &directory, cx).await?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn capture_connection(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    directory: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let (provider, settled_frame) = if scene.name == "desktop-100" {
        (ProviderId::SoundCloud, "soundcloud-connection.png")
    } else {
        (ProviderId::YouTubeMusic, "youtube-connection.png")
    };
    let click = 7500;
    sample(window, click - 400, None, cx)?;
    draw(raw, scene, None, cx)?;
    sample(window, click, None, cx)?;
    window.update(cx, |view, window, cx| {
        view.select_connection_for_capture(provider, window, cx);
    })?;
    if scene.name == "wide-100" {
        measure_connection(window, raw, scene, directory, click, cx)?;
    }
    if !scene.reduced {
        let phases = if scene.width >= 900. {
            [120, 260, 385, 535, 670]
        } else {
            [90, 190, 285, 400, 495]
        };
        for offset in phases {
            sample(window, click + offset, None, cx)?;
            draw(
                raw,
                scene,
                Some(&directory.join(format!("connection-{offset}ms.png"))),
                cx,
            )?;
        }
    }
    sample(window, click + 800, None, cx)?;
    draw(raw, scene, Some(&directory.join(settled_frame)), cx)?;
    if scene.width >= 900. {
        let switch = click + 950;
        sample(window, switch, None, cx)?;
        window.update(cx, |view, window, cx| {
            view.select_connection_for_capture(ProviderId::Jellyfin, window, cx);
        })?;
        if !scene.reduced {
            sample(window, switch + 320, None, cx)?;
            draw(
                raw,
                scene,
                Some(&directory.join("connection-switch-320ms.png")),
                cx,
            )?;
        }
        sample(window, switch + 600, None, cx)?;
        draw(
            raw,
            scene,
            Some(&directory.join("jellyfin-connection.png")),
            cx,
        )?;
        if !scene.reduced {
            window.update(cx, |view, window, cx| {
                view.submit_empty_jellyfin_for_capture(window, cx)
            })?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !window.update(cx, |view, _, _| view.failed_connection_for_capture())? {
                if Instant::now() >= deadline {
                    return Err(io::Error::other("Ожидание ошибки формы Jellyfin истекло.").into());
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
            }
            draw(raw, scene, Some(&directory.join("jellyfin-error.png")), cx)?;
        }
    }
    Ok(())
}

fn measure_connection(
    window: WindowHandle<mediadeka::MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    directory: &Path,
    click: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let mut timings = [0.; 30];
    for (index, timing) in timings.iter_mut().enumerate() {
        let start = Instant::now();
        sample(window, click + 120 + index as u64 * 20, None, cx)?;
        draw(raw, scene, None, cx)?;
        *timing = start.elapsed().as_secs_f64() * 1000.;
    }
    timings.sort_by(f64::total_cmp);
    let report = format!(
        "30 transfer frames, GPUI build + D3D11 readback, ms: median {:.2}, p95 {:.2}, max {:.2}\n",
        (timings[14] + timings[15]) * 0.5,
        timings[28],
        timings[29],
    );
    print!("{report}");
    fs::write(directory.join("transfer-timing.txt"), report)?;
    Ok(())
}

fn sample(
    window: WindowHandle<mediadeka::MainView>,
    milliseconds: u64,
    hover: Option<usize>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    window.update(cx, |view, _, cx| {
        view.sample_welcome(Duration::from_millis(milliseconds), hover, cx)
    })?;
    Ok(())
}

fn draw(
    raw: AnyWindowHandle,
    scene: Scene,
    path: Option<&Path>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let frame = render_image(raw, scene, cx)?;
    if let Some(path) = path {
        frame.save(path)?;
        println!(
            "{} × {} @ {:.0}% {}",
            frame.width(),
            frame.height(),
            scene.dpi as f32 / 96. * 100.,
            path.display()
        );
    }
    Ok(())
}

fn render_image(
    raw: AnyWindowHandle,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<image::RgbaImage> {
    raw.update(cx, |_, window, cx| -> gpui::Result<image::RgbaImage> {
        window.draw(cx).clear(cx);
        let frame = window.render_to_image()?;
        let scale = scene.dpi as f32 / 96.;
        if window.scale_factor() != scale
            || frame.width() != (scene.width * scale).round() as u32
            || frame.height() != (scene.height * scale).round() as u32
        {
            return Err(io::Error::other(format!(
                "{}: unexpected {} × {} @ {}%",
                scene.name,
                frame.width(),
                frame.height(),
                window.scale_factor() * 100.,
            ))
            .into());
        }
        Ok(frame)
    })?
}

#[cfg(windows)]
struct DpiChange {
    hwnd: windows_sys::Win32::Foundation::HWND,
    rect: windows_sys::Win32::Foundation::RECT,
    dpi: u32,
}

#[cfg(windows)]
impl DpiChange {
    fn apply(self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_DPICHANGED};
        // GPUI handles the native DPI message synchronously. Window and RECT
        // remain valid throughout dispatch, and no GPUI view is borrowed here.
        unsafe {
            SendMessageW(
                self.hwnd,
                WM_DPICHANGED,
                (self.dpi | self.dpi << 16) as usize,
                &self.rect as *const _ as isize,
            )
        };
    }
}

#[cfg(windows)]
fn native_dpi_change(window: &Window, scene: Scene) -> gpui::Result<DpiChange> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

    let raw = HasWindowHandle::window_handle(window)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let RawWindowHandle::Win32(handle) = raw.as_raw() else {
        return Err(io::Error::other("Ожидалось окно Win32.").into());
    };
    let hwnd = handle.hwnd.get() as _;
    let (mut rect, mut client) = (Default::default(), Default::default());
    // Windows writes into the two local rectangles. The borrowed HWND is live.
    if unsafe { GetWindowRect(hwnd, &mut rect) == 0 || GetClientRect(hwnd, &mut client) == 0 } {
        return Err(io::Error::last_os_error().into());
    }
    let scale = scene.dpi as f32 / 96.;
    rect.right += (scene.width * scale).round() as i32 - (client.right - client.left);
    rect.bottom += (scene.height * scale).round() as i32 - (client.bottom - client.top);
    Ok(DpiChange {
        hwnd,
        rect,
        dpi: scene.dpi,
    })
}

fn fail(error: impl std::fmt::Display) -> ! {
    eprintln!("Не удалось сохранить кадры Mediadeka: {error:#}");
    process::exit(1)
}
