use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, Window, WindowHandle};
use mediadeka::MainView;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        IsIconic, SPI_GETCLIENTAREAANIMATION, SW_MINIMIZE, SW_RESTORE, ShowWindowAsync,
        SystemParametersInfoW,
    },
};

const STATE_TIMEOUT: Duration = Duration::from_secs(5);
const STATE_POLL: Duration = Duration::from_millis(15);
const SETTLE: Duration = Duration::from_millis(120);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(340);
const MINIMIZED_INTERVAL: Duration = Duration::from_millis(2100);

pub(super) fn run(output: PathBuf) -> gpui::Result<()> {
    let mut animations_enabled = 0i32;
    // The system setting is read only. The buffer is one Win32 BOOL.
    if unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut animations_enabled as *mut i32).cast(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if animations_enabled == 0 {
        return Err(io::Error::other("Windows отключила анимацию содержимого окон.").into());
    }

    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    // This runs before constructing the GPUI event loop or any worker threads.
    unsafe { env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let window = mediadeka::open_main_window(cx).unwrap_or_else(|error| super::fail(error));
        cx.activate(true);
        let handle = AnyWindowHandle::from(window);
        cx.spawn(async move |cx| {
            verify(window, output, cx)
                .await
                .unwrap_or_else(|error| super::fail(error));
            if let Err(error) = handle.update(cx, |_, window, _| window.remove_window()) {
                super::fail(error);
            }
        })
        .detach();
    });
    Ok(())
}

async fn verify(
    window: WindowHandle<MainView>,
    output: PathBuf,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let started = Instant::now();
    let handle = AnyWindowHandle::from(window);
    let hwnd = handle.update(cx, |_, window, _| hwnd(window))??;
    await_state(handle, hwnd, false, cx).await?;
    await_welcome(window, cx).await?;

    let timer = cx.background_executor().clone();
    timer.timer(SETTLE).await;
    let first = capture(handle, cx, &output, "visible-first", started)?;
    timer.timer(SAMPLE_INTERVAL).await;
    let second = capture(handle, cx, &output, "visible-second", started)?;
    timer.timer(SAMPLE_INTERVAL).await;
    let third = capture(handle, cx, &output, "visible-third", started)?;
    require(
        first.background == second.background && second.background == third.background,
        "Фон приветствия меняется без ввода.",
    )?;

    change_state(hwnd, SW_MINIMIZE)?;
    await_state(handle, hwnd, true, cx).await?;
    timer.timer(SETTLE).await;
    let paused = capture(handle, cx, &output, "minimized-first", started)?;
    timer.timer(MINIMIZED_INTERVAL).await;
    let still_paused = capture(handle, cx, &output, "minimized-last", started)?;
    require(
        paused.background == still_paused.background,
        "Фон изменился в свёрнутом окне.",
    )?;

    change_state(hwnd, SW_RESTORE)?;
    await_state(handle, hwnd, false, cx).await?;
    timer.timer(SETTLE).await;
    let restored = capture(handle, cx, &output, "restored-first", started)?;
    require(
        still_paused.size == restored.size && still_paused.background == restored.background,
        "После восстановления изменились размер окна или фон.",
    )?;
    timer.timer(SAMPLE_INTERVAL).await;
    let resumed = capture(handle, cx, &output, "restored-second", started)?;
    require(
        restored.background == resumed.background,
        "В восстановленном окне фон меняется без ввода.",
    )?;
    println!("Матовое приветствие стабильно в обычном, свёрнутом и восстановленном окне.");
    Ok(())
}

pub(super) fn hwnd(window: &Window) -> gpui::Result<HWND> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(io::Error::other("Ожидалось окно Win32.").into());
    };
    Ok(handle.hwnd.get() as HWND)
}

pub(super) fn change_state(hwnd: HWND, command: i32) -> gpui::Result<()> {
    // This HWND stays live while the example owns the GPUI window.
    if unsafe { ShowWindowAsync(hwnd, command) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

async fn await_welcome(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let deadline = Instant::now() + STATE_TIMEOUT;
    while !cx.update(|cx| window.read(cx).map(MainView::is_welcome_for_capture))? {
        require(
            Instant::now() < deadline,
            "Приветствие не открылось в чистом тестовом профиле.",
        )?;
        cx.background_executor().timer(STATE_POLL).await;
    }
    Ok(())
}

async fn await_state(
    handle: AnyWindowHandle,
    hwnd: HWND,
    minimized: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let deadline = Instant::now() + STATE_TIMEOUT;
    loop {
        let iconic = unsafe { IsIconic(hwnd) } != 0;
        let visible = handle.update(cx, |_, window, _| window.is_visible())?;
        if iconic == minimized && visible != minimized {
            return Ok(());
        }
        require(
            Instant::now() < deadline,
            "Win32 и GPUI не подтвердили ожидаемое состояние окна.",
        )?;
        cx.background_executor().timer(STATE_POLL).await;
    }
}

struct Snapshot {
    size: (u32, u32),
    background: [u8; 32],
}

fn capture(
    handle: AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
    started: Instant,
) -> gpui::Result<Snapshot> {
    handle.update(cx, |_, window, _| -> gpui::Result<Snapshot> {
        // Read the scene already built by GPUI's normal frame callbacks.
        // This example never calls draw, refresh, notify or request_animation_frame.
        let image = window.render_to_image()?;
        let (width, height) = image.dimensions();
        require(width >= 300 && height >= 240, "Кадр GPUI отсутствует.")?;

        let mut hash = Sha256::new();
        // The upper-left stage excludes the window chrome and the entry controls.
        for y in height / 16..height / 5 {
            for x in 0..width / 5 {
                hash.update(image.get_pixel(x, y).0);
            }
        }
        let background = hash.finalize();
        image.save(output.join(format!("{name}.png")))?;
        println!(
            "{name}: {:.0} ms, {width} x {height}, background SHA-256 {background:x}",
            started.elapsed().as_secs_f64() * 1000.0,
        );
        Ok(Snapshot {
            size: (width, height),
            background: background.into(),
        })
    })?
}

fn require(condition: bool, error: &'static str) -> gpui::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(error).into())
    }
}
