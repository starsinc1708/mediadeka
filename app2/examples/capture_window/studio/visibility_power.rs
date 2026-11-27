//! Measure an actual visible window without generating frame or input events.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, FrameEvent, FrameTimingCollector, WindowHandle};
use mediadeka::{
    GlassPalette, MainView, PatternId, PatternSelection, QualityChoice, VisualSettings,
};
use serde::Serialize;
use serde_json::{Value, json};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{IsIconic, IsWindowVisible, SW_HIDE, SW_MINIMIZE, SW_RESTORE},
};

use super::{COVER, fixture};
use crate::live_motion::{change_state, hwnd};

const MEASURE: Duration = Duration::from_millis(1800);
const QUIESCE: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(25);
const TIMEOUT: Duration = Duration::from_secs(12);

pub(crate) fn run(output: PathBuf) -> gpui::Result<()> {
    fs::create_dir_all(&output)?;
    let profile = tempfile::tempdir()?;
    let wave = profile.path().join("visibility-tone.wav");
    fixture::write_wave(&wave, fixture::WAVEFORM_TONE_HZ, 60)?;
    let playlist = mediadeka::seed_playlist_capture(profile.path(), &[wave])?;
    // This capture process changes only its own environment, before GPUI starts.
    unsafe {
        env::set_var("MEDIADEKA_GUI", "3");
        env::set_var("MEDIADEKA_PROFILE_DIR", profile.path());
        env::set_var("MEDIADEKA_GLASS_AUDIO_TRACE", "1");
        env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");
        env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
        env::remove_var("MEDIADEKA_GLASS_EFFECTS");
    }
    mediadeka::application().run(move |cx| {
        let window = mediadeka::open_main_window(cx).unwrap_or_else(|error| crate::fail(error));
        cx.activate(true);
        cx.spawn(async move |cx| {
            let result = verify(window, &output, playlist, cx).await;
            let raw = AnyWindowHandle::from(window);
            let _ = raw.update(cx, |_, window, _| window.remove_window());
            drop(profile);
            if let Err(error) = result {
                crate::fail(error);
            }
        })
        .detach();
    });
    Ok(())
}

async fn verify(
    window: WindowHandle<MainView>,
    output: &Path,
    playlist: i64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let handle = raw.update(cx, |_, window, _| hwnd(window))??;
    let window_id = raw.window_id();
    await_state(window, handle, cx, |s| s.visible_active() && s.workspace).await?;
    window.update(cx, |view, window, cx| -> gpui::Result<()> {
        let settings = VisualSettings {
            quality: QualityChoice::Balanced,
            pattern: PatternSelection::Fixed(PatternId::ThermalFlow),
            ..VisualSettings::standard()
        };
        view.set_visual_for_capture(GlassPalette::Abyss, settings, window, cx);
        view.play_local_with_artwork_for_capture(playlist, COVER, cx)
    })??;

    let mut collector = FrameTimingCollector::new();
    gpui::set_trace_enabled(true);
    let mut phases = Vec::new();
    let timer = cx.background_executor().clone();
    for (name, command, expected) in [
        ("active", None, Visibility::Active),
        ("hidden", Some(SW_HIDE), Visibility::Hidden),
        ("restored", Some(SW_RESTORE), Visibility::Active),
        ("minimized", Some(SW_MINIMIZE), Visibility::Minimized),
        ("restored-final", Some(SW_RESTORE), Visibility::Active),
    ] {
        let changed = Instant::now();
        if let Some(command) = command {
            change_state(handle, command)?;
        }
        await_state(window, handle, cx, |sample| expected.ready(sample)).await?;
        let ready_after_ms = changed.elapsed().as_secs_f64() * 1000.;
        if matches!(expected, Visibility::Active) {
            save_frame(raw, &output.join(format!("{name}-begin.png")), cx)?;
        }
        timer.timer(QUIESCE).await;

        let sample_before = sample(window, handle, cx)?;
        let _ = collector.collect_unseen();
        println!("visibility {name} measure-start");
        let began = Instant::now();
        timer.timer(MEASURE).await;
        let ended = Instant::now();
        let sample_after = sample(window, handle, cx)?;
        println!("visibility {name} measure-end");
        let mut draws = 0usize;
        let mut presents = 0usize;
        let mut draw_cpu_ms = 0.;
        for event in collector.collect_unseen() {
            match event {
                FrameEvent::Draw(frame)
                    if frame.window_id == window_id
                        && (began..=ended).contains(&frame.draw_end) =>
                {
                    draws += 1;
                    draw_cpu_ms += frame.draw_duration().as_secs_f64() * 1000.;
                }
                FrameEvent::Present(frame)
                    if frame.window_id == window_id
                        && (began..=ended).contains(&frame.present_end) =>
                {
                    presents += 1;
                }
                _ => {}
            }
        }
        let result = Phase {
            name,
            ready_after_ms,
            measurement_ms: ended.duration_since(began).as_secs_f64() * 1000.,
            draws,
            presents,
            draw_cpu_ms,
            before: sample_before,
            after: sample_after,
        };
        println!("{}", serde_json::to_string(&result)?);
        phases.push(result);
        fs::write(
            output.join("visibility-power.json"),
            serde_json::to_vec_pretty(&json!({ "phases": phases }))?,
        )?;
        save_frame(raw, &output.join(format!("{name}.png")), cx)?;
        let phase = phases.last().expect("the sampled phase was appended");
        require(
            expected.ready(&phase.before) && expected.ready(&phase.after),
            format!("Несогласованная видимость или WASAPI в состоянии {name}."),
        )?;
        require(
            phase.after.position_seconds - phase.before.position_seconds > 0.8,
            format!("Позиция WAV не движется в состоянии {name}."),
        )?;
        match expected {
            Visibility::Active => require(
                phase.draws > 2 && phase.presents > 2,
                format!("В активном окне нет кадров Fluid Glass в состоянии {name}."),
            )?,
            Visibility::Hidden | Visibility::Minimized => require(
                phase.draws == 0 && phase.presents == 0,
                format!("Fluid Glass продолжил рисовать кадры в состоянии {name}."),
            )?,
        }
    }
    gpui::set_trace_enabled(false);
    Ok(())
}

#[derive(Clone, Copy)]
enum Visibility {
    Active,
    Hidden,
    Minimized,
}

impl Visibility {
    fn ready(self, sample: &Sample) -> bool {
        if !sample.playing || sample.sample_rate_hz != Some(44_100) {
            return false;
        }
        match self {
            Self::Active => {
                sample.visible_active()
                    && sample.capture == "streaming"
                    && sample.bands.iter().any(|level| *level > 0.001)
            }
            Self::Hidden | Self::Minimized => {
                sample.iconic == matches!(self, Self::Minimized)
                    && (sample.iconic || !sample.native_visible)
                    && !sample.gpui_visible
                    && sample.capture == "stopped"
                    && sample.bands == [0.; 3]
            }
        }
    }
}

#[derive(Serialize)]
struct Sample {
    native_visible: bool,
    gpui_visible: bool,
    active: bool,
    iconic: bool,
    workspace: bool,
    capture: &'static str,
    playing: bool,
    position_seconds: f64,
    sample_rate_hz: Option<u64>,
    bands: [f32; 3],
}

impl Sample {
    fn visible_active(&self) -> bool {
        self.native_visible && self.gpui_visible && self.active && !self.iconic
    }
}

#[derive(Serialize)]
struct Phase {
    name: &'static str,
    ready_after_ms: f64,
    measurement_ms: f64,
    draws: usize,
    presents: usize,
    draw_cpu_ms: f64,
    before: Sample,
    after: Sample,
}

async fn await_state(
    window: WindowHandle<MainView>,
    handle: HWND,
    cx: &mut AsyncApp,
    ready: impl Fn(&Sample) -> bool,
) -> gpui::Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let state = sample(window, handle, cx)?;
        if ready(&state) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Состояние окна или WASAPI не установилось: {}",
                serde_json::to_string(&state)?
            ))
            .into());
        }
        cx.background_executor().timer(POLL).await;
    }
}

fn sample(window: WindowHandle<MainView>, handle: HWND, cx: &mut AsyncApp) -> gpui::Result<Sample> {
    window.update(cx, |view, window, cx| {
        let (capture, playing) = view
            .fluid_glass_audio_status_for_capture(cx)
            .unwrap_or(("unavailable", false));
        let playback = view.sound_state_for_capture(cx).unwrap_or(Value::Null);
        Sample {
            // The HWND is owned by this example for the whole measurement.
            native_visible: unsafe { IsWindowVisible(handle) != 0 },
            gpui_visible: window.is_visible(),
            active: window.is_window_active(),
            iconic: unsafe { IsIconic(handle) != 0 },
            workspace: view.home_status_for_capture().is_some(),
            capture,
            playing,
            position_seconds: playback["position"].as_f64().unwrap_or(0.),
            sample_rate_hz: playback["sample_rate_hz"].as_u64(),
            bands: view.fluid_glass_audio_for_capture(cx),
        }
    })
}

fn save_frame(window: AnyWindowHandle, path: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    // Read back the last real scene outside the measurement interval.
    window.update(cx, |_, window, _| -> gpui::Result<()> {
        window.render_to_image()?.save(path)?;
        Ok(())
    })?
}

fn require(condition: bool, detail: String) -> gpui::Result<()> {
    if !condition {
        return Err(io::Error::other(detail).into());
    }
    Ok(())
}
