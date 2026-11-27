use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, FrameEvent, FrameTimingCollector, InputEvent as _, KeyUpEvent,
    Keystroke, WindowHandle,
};
use mediadeka::{CaptureLayout, MainView};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_SETCLIENTAREAANIMATION, SPIF_SENDCHANGE, SystemParametersInfoW,
};

use super::{Scene, draw, fail, native_dpi_change};

const ARTWORK: &str = "https://i.scdn.co/image/ab67616d00001e02815e5c079d7641d60b51825d";
const CACHED_ARTWORK: &str =
    "spotify/b1c8dbfc94496c95638b5608d9c327c578fcf1eed6f94d3f1145cdfee71e3d9d.cover";
const QUEUE_COUNT: usize = 24;
type PlayerState = (bool, bool, Option<usize>, bool, Option<&'static str>, usize);
const SCENES: [Scene; 3] = [
    Scene {
        name: "wide-100",
        width: 1500.,
        height: 850.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "minimum-100",
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

pub(super) fn run(args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let arguments = args.collect::<Vec<_>>();
    let [output, wav, artwork_dir] = arguments.as_slice() else {
        return Err(io::Error::other("--player <кадры> <WAV> <каталог кэша artwork>").into());
    };
    let output = PathBuf::from(output);
    fs::create_dir_all(&output)?;
    let source = PathBuf::from(wav).canonicalize()?;
    let profile = tempfile::tempdir()?;
    let id = mediadeka::seed_playlist_capture(profile.path(), &vec![source; QUEUE_COUNT])?;
    let cache = profile.path().join("artwork").join(CACHED_ARTWORK);
    fs::create_dir_all(profile.path().join("artwork/spotify"))?;
    fs::copy(Path::new(artwork_dir).join(CACHED_ARTWORK), cache)?;
    // Set the isolated profile before constructing GPUI and its worker threads.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let anchor = new_window(SCENES[0], cx).unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            let result: gpui::Result<()> = async {
                for scene in SCENES {
                    verify_scene(scene, id, &output, cx).await?;
                }
                verify_live_motion(id, &output.join("live-motion"), cx).await?;
                anchor.update(cx, |_, window, _| window.remove_window())?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                fail(error);
            }
        })
        .detach();
    });
    Ok(())
}

fn new_window(scene: Scene, cx: &mut gpui::App) -> gpui::Result<WindowHandle<MainView>> {
    mediadeka::open_capture_window(
        cx,
        CaptureLayout {
            width: scene.width,
            height: scene.height,
            reduced_motion: scene.reduced,
        },
    )
}

async fn verify_scene(scene: Scene, id: i64, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    let window = cx.update(|cx| new_window(scene, cx))?;
    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(id, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(8);
    while window
        .update(cx, |view, _, _| view.playlist_status_for_capture())?
        .is_none()
    {
        require(
            Instant::now() < deadline,
            "Локальный плейлист не загрузился.",
        )?;
        timer.timer(Duration::from_millis(30)).await;
    }
    draw(raw, scene, Some(&output.join("empty.png")), cx)?;
    window.update(cx, |view, _, cx| {
        view.play_local_with_artwork_for_capture(id, ARTWORK, cx)
    })??;
    await_seekable(window, 0, cx).await?;
    if scene.name == SCENES[0].name {
        measure_position_ticks(window, scene, &output, cx).await?;
    }
    let focus = |window: WindowHandle<MainView>, cx: &mut AsyncApp| -> gpui::Result<()> {
        window.update(cx, |view, window, cx| {
            view.reset_player_probe_focus_for_capture(window, cx)
        })?;
        Ok(())
    };

    focus(window, cx)?;
    for _ in 0..4 {
        key(raw, "shift-tab", cx)?;
        draw(raw, scene, None, cx)?;
    }
    draw(raw, scene, Some(&output.join("volume-focus.png")), cx)?;
    key(raw, "shift-tab", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "shift-tab", cx)?;
    capture_progress(raw, scene, &output.join("progress-focus.png"), cx)?;

    focus(window, cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "shift-tab", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "shift-tab", cx)?;
    draw(raw, scene, Some(&output.join("queue-button-focus.png")), cx)?;
    key(raw, "enter", cx)?;
    require(
        state(window, cx)?.4 == Some("queue"),
        "Клавиатура не открыла очередь.",
    )?;
    draw(raw, scene, Some(&output.join("queue.png")), cx)?;
    key(raw, "tab", cx)?;
    draw(raw, scene, Some(&output.join("queue-row-focus.png")), cx)?;
    key(raw, "escape", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "shift-tab", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "enter", cx)?;
    require(
        state(window, cx)?.4 == Some("devices"),
        "Клавиатура не открыла устройства.",
    )?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while state(window, cx)?.5 == 0 {
        require(
            Instant::now() < deadline,
            "Список устройств не получен от движка.",
        )?;
        timer.timer(Duration::from_millis(50)).await;
    }
    draw(raw, scene, Some(&output.join("devices.png")), cx)?;
    key(raw, "tab", cx)?;
    draw(raw, scene, Some(&output.join("device-row-focus.png")), cx)?;
    key(raw, "escape", cx)?;
    draw(raw, scene, None, cx)?;
    for _ in 0..3 {
        key(raw, "shift-tab", cx)?;
        draw(raw, scene, None, cx)?;
    }
    key(raw, "shift-tab", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "shift-tab", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "enter", cx)?;
    await_seekable(window, 1, cx).await?;
    await_bright_artwork(raw, scene, cx).await?;
    key(raw, "tab", cx)?;
    draw(raw, scene, None, cx)?;
    key(raw, "tab", cx)?;
    capture_progress(raw, scene, &output.join("bright-progress-focus.png"), cx)?;
    println!(
        "{}: очередь с {QUEUE_COUNT} повторениями, устройств {}, навигация с клавиатуры.",
        scene.name,
        state(window, cx)?.5
    );
    raw.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn measure_position_ticks(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    timer.timer(Duration::from_millis(450)).await;
    let notifications =
        window.update(cx, |view, _, cx| view.observe_player_ticks_for_capture(cx))?;
    let raw = AnyWindowHandle::from(window);
    let trace_enabled_here = gpui::set_trace_enabled(true);
    let mut frames = FrameTimingCollector::new();
    let sampled: gpui::Result<()> = async {
        for _ in 0..6 {
            timer.timer(Duration::from_millis(160)).await;
            draw(raw, scene, None, cx)?;
        }
        Ok(())
    }
    .await;
    let events = frames.collect_unseen();
    if trace_enabled_here {
        gpui::set_trace_enabled(false);
    }
    sampled?;
    let (ticks, transport) = notifications();
    require(ticks >= 4, "Плеер не сообщил о ходе позиции.")?;
    require(
        transport == 0,
        "Тики позиции уведомили подписчиков транспорта.",
    )?;
    let mut times: Vec<_> = events
        .into_iter()
        .filter_map(|event| match event {
            FrameEvent::Draw(frame) if frame.window_id == raw.window_id() => {
                Some(frame.draw_duration().as_secs_f64() * 1_000.)
            }
            _ => None,
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let p95 = times.get((times.len() * 95).div_ceil(100).saturating_sub(1));
    let report = format!(
        "GPUI position events {ticks}, transport observer calls {transport}, draws {}, CPU draw P95 {p95:?} ms\n",
        times.len()
    );
    print!("{report}");
    fs::write(output.join("position-ticks.txt"), report)?;
    Ok(())
}

async fn await_bright_artwork(
    raw: AnyWindowHandle,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        draw(raw, scene, None, cx)?;
        let source_tint_visible = raw.update(cx, |_, window, _| -> gpui::Result<bool> {
            let frame = window.render_to_image()?;
            let x = (frame.width() as f32 * 0.28) as u32;
            let y = frame.height() - (20. * window.scale_factor()).round() as u32;
            // The warmed Spotify cover gives a green tint in this unobstructed part of the bar.
            let [r, g, b, _] = frame.get_pixel(x, y).0;
            Ok(g.saturating_sub(r) > 3 && g.saturating_sub(b) > 3)
        })??;
        if source_tint_visible {
            return Ok(());
        }
        require(Instant::now() < deadline, "Обложка не окрасила фон плеера.")?;
        timer.timer(Duration::from_millis(60)).await;
    }
}

fn capture_progress(
    raw: AnyWindowHandle,
    scene: Scene,
    path: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    draw(raw, scene, Some(path), cx)?;
    let ring_pixels = raw.update(cx, |_, window, _| -> gpui::Result<usize> {
        let frame = window.render_to_image()?;
        let scale = scene.dpi as f32 / 96.;
        let first_row = frame.height().saturating_sub((90. * scale) as u32);
        let middle = frame.width() / 2;
        let count = (first_row..frame.height())
            .filter(|&y| {
                (middle - 10..middle + 10).all(|x| {
                    let [r, g, b, _] = frame.get_pixel(x, y).0;
                    r.abs_diff(0xdc) <= 3 && g.abs_diff(0xc1) <= 3 && b.abs_diff(0xff) <= 3
                })
            })
            .count();
        Ok(count)
    })??;
    require(
        ring_pixels >= 2,
        "Не виден непрерывный контур фокуса перемотки.",
    )
}

async fn verify_live_motion(id: i64, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let original = MotionSetting::read()?;
    MotionSetting::write(true)?;
    let window = cx.update(mediadeka::open_main_window)?;
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(id, window, cx)
    })?;
    window.update(cx, |view, _, cx| {
        view.play_local_with_artwork_for_capture(id, ARTWORK, cx)
    })??;
    await_seekable(window, 0, cx).await?;
    require(
        !state(window, cx)?.0,
        "Первое состояние движения не совпало с Windows.",
    )?;
    raw.update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
    })?;
    // Shift+Tab starts at the last transport control; the next-track control is seventh.
    window.update(cx, |view, window, cx| {
        view.reset_player_probe_focus_for_capture(window, cx)
    })?;
    for _ in 0..8 {
        key(raw, "shift-tab", cx)?;
        raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
    }
    // Observe a full preference cycle to align the next switch with Mediadeka's one-second
    // observer. Otherwise a natural fade could finish before Windows' change reaches the view.
    MotionSetting::write(false)?;
    await_motion(window, true, cx).await?;
    MotionSetting::write(true)?;
    await_motion(window, false, cx).await?;
    let timer = cx.background_executor().clone();
    timer.timer(Duration::from_millis(850)).await;
    let transition_start = Instant::now();
    key(raw, "enter", cx)?;
    require(
        state(window, cx)?.1 && state(window, cx)?.2 == Some(1),
        "Обычный переход обложки не запущен.",
    )?;
    save_frame(raw, &output.join("animated-first.png"), cx)?;
    MotionSetting::write(false)?;
    let start = Instant::now();
    await_motion(window, true, cx).await?;
    require(
        transition_start.elapsed() < Duration::from_millis(240),
        "Windows сменила настройку после естественного окончания анимации обложки.",
    )?;
    require(
        !state(window, cx)?.1,
        "После изменения настройки переход остался активен.",
    )?;
    println!(
        "Windows: изменение обнаружено уже открытым плеером за {} мс. Переход завершён через {} мс от начала.",
        start.elapsed().as_millis(),
        transition_start.elapsed().as_millis(),
    );
    save_frame(raw, &output.join("settled.png"), cx)?;
    key(raw, "enter", cx)?;
    await_seekable(window, 2, cx).await?;
    require(
        !state(window, cx)?.1,
        "Следующий трек запустил переход при уменьшенном движении.",
    )?;
    save_frame(raw, &output.join("next-first.png"), cx)?;
    timer.timer(Duration::from_millis(350)).await;
    save_frame(raw, &output.join("next-last.png"), cx)?;
    raw.update(cx, |_, window, _| window.remove_window())?;
    MotionSetting::write(original.0)?;
    Ok(())
}

async fn await_motion(
    window: WindowHandle<MainView>,
    reduced: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(3);
    while state(window, cx)?.0 != reduced {
        require(
            Instant::now() < deadline,
            "Открытый плеер не получил системное уменьшение движения.",
        )?;
        timer.timer(Duration::from_millis(10)).await;
    }
    Ok(())
}

fn save_frame(raw: AnyWindowHandle, path: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| -> gpui::Result<()> {
        window.draw(cx).clear(cx);
        window.render_to_image()?.save(path)?;
        Ok(())
    })?
}

async fn await_seekable(
    window: WindowHandle<MainView>,
    index: usize,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, _, track, seekable, _, _) = state(window, cx)?;
        if track == Some(index) && seekable {
            return Ok(());
        }
        require(
            Instant::now() < deadline,
            "libmpv не подтвердил доступ к перемотке записи.",
        )?;
        timer.timer(Duration::from_millis(50)).await;
    }
}

fn state(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<PlayerState> {
    window.update(cx, |view, _, cx| view.player_state_for_capture(cx))
}

fn key(raw: AnyWindowHandle, key: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let key = Keystroke::parse(key)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
    })?;
    Ok(())
}

fn require(condition: bool, error: &str) -> gpui::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(error).into())
    }
}

struct MotionSetting(bool);

impl MotionSetting {
    fn read() -> io::Result<Self> {
        let mut enabled = 0i32;
        // Windows writes one BOOL into the live stack slot.
        if unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(enabled != 0))
    }

    fn write(enabled: bool) -> io::Result<()> {
        // SPI_SETCLIENTAREAANIMATION interprets pvParam as a BOOL value.
        // Send the change to the current session without updating the user profile.
        if unsafe {
            SystemParametersInfoW(
                SPI_SETCLIENTAREAANIMATION,
                0,
                usize::from(enabled) as *mut _,
                SPIF_SENDCHANGE,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for MotionSetting {
    fn drop(&mut self) {
        let _ = Self::write(self.0);
    }
}
