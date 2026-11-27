use std::{cell::Cell, fs, io, path::Path, time::Duration};

use gpui::{AsyncApp, Keystroke, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{
    QUEUE_BUTTON, QUEUE_LIST, Scene, check_dock_pixels, dispatch, draw,
    fixture::{SPECTRUM_TONES, WAVEFORM_TONE_HZ},
    focus, frame, key, named, node_label, press, require, settle_panel, snapshot, sound, wait,
};

const PHASE: Duration = Duration::from_millis(4600);
const PLAY_BUTTON: &str = "player-play";
const SEEK: &str = "player-progress";
const MINIMUM_SIGNAL: f32 = 0.04;
const QUIET_SIGNAL: f32 = 0.01;
const DOCK_TEST_BASS: f32 = 0.50;
// Desktop areas clear of controls: the right light field, the curved junction,
// and the top edge of the player.
const REGIONS: [[u32; 4]; 3] = [
    [1380, 70, 1690, 153],
    [252, 916, 382, 969],
    [650, 955, 1280, 968],
];
const WAVEFORM_SLOTS: [[u32; 4]; 2] = [[704, 986, 848, 1014], [1072, 986, 1216, 1014]];
// The pause button is opaque inside this rectangle. The seek thumb can move
// between captures, so the slider is checked through its control semantics.
const PAUSE_BUTTON_INTERIOR: [[u32; 4]; 1] = [[945, 990, 975, 1010]];

pub(super) async fn verify_settings(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let saved = |cx: &mut AsyncApp| window.update(cx, |view, _, _| view.visual_for_capture());
    let original = saved(cx)?;
    window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    wait(cx, "Настройки оформления", |cx| {
        Ok(node_label(&snapshot(raw, cx)?, "settings-appearance-shortcut").map(|_| ()))
    })
    .await?;
    press(raw, &named("settings-appearance-shortcut"), cx)?;
    press(raw, &named("appearance-audio-calm"), cx)?;
    focus(raw, &named("appearance-audio-sensitivity-input"), cx)?;
    key(raw, "ctrl-a", cx)?;
    for digit in ["4", "8"] {
        dispatch(
            raw,
            Keystroke {
                key: digit.into(),
                key_char: Some(digit.into()),
                ..Default::default()
            },
            cx,
        )?;
    }
    key(raw, "enter", cx)?;
    press(raw, &named("appearance-audio-channel-pulses"), cx)?;
    press(raw, &named("appearance-audio-enabled"), cx)?;
    require(
        node_label(&snapshot(raw, cx)?, "appearance-audio-status")
            == Some("Реакция на музыку выключена"),
        "Настройки должны отобразить состояние выключенной реакции.",
    )?;
    let visual = saved(cx)?;
    require(
        visual.effects == original.effects,
        "Настройки реакции не должны изменять оптические переключатели.",
    )?;
    let settings = rusqlite::Connection::open(profile.join("mediadeka.sqlite3"))?;
    let persisted: (bool, String, u8, u8) = settings.query_row(
        "SELECT audio_enabled, audio_profile, audio_sensitivity, audio_channels FROM appearance WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    require(
        persisted == (false, "calm".into(), 48, 47) && visual.audio.sensitivity() == 48,
        &format!("Профиль реакции, чувствительность и каналы не сохранены: {persisted:?}."),
    )?;
    fs::write(
        output.join("audio-settings.json"),
        serde_json::to_vec_pretty(&visual.audio)?,
    )?;
    require(
        window.update(cx, |view, window, cx| {
            view.reveal_audio_controls_for_capture(window, cx)
        })?,
        "Геометрия секции музыкальной реактивности должна быть доступна.",
    )?;
    frame(raw, scene, output, "audio-settings", cx)?;
    press(raw, &named("appearance-audio-enabled"), cx)?;
    press(raw, &named("appearance-audio-immersive"), cx)?;
    require(
        saved(cx)?.audio.profile.as_str() == "immersive",
        "Иммерсивный профиль не применился.",
    )?;
    press(raw, &named("appearance-audio-balanced"), cx)?;
    focus(raw, &named("appearance-audio-sensitivity"), cx)?;
    for _ in 48..original.audio.sensitivity() {
        key(raw, "right", cx)?;
    }
    press(raw, &named("appearance-audio-channel-pulses"), cx)?;
    require(
        saved(cx)?.audio == original.audio,
        "Сбалансированный режим с исходными параметрами не восстановлен.",
    )?;
    press(raw, &named("studio-library"), cx)?;
    wait(
        cx,
        "Строки локальной медиатеки",
        |cx| Ok(super::queue_candidate(&snapshot(raw, cx)?).map(|_| ())),
    )
    .await?;
    Ok(())
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_audio_for_capture(PHASE, cx)
    })?;
    let mut samples = Vec::with_capacity(SPECTRUM_TONES.len());
    for (index, (title, frequency)) in SPECTRUM_TONES.into_iter().enumerate() {
        let tree = snapshot(raw, cx)?;
        let prefix = format!("Слушать · {title} · ");
        let row = tree["nodes"].as_object().and_then(|nodes| {
            nodes.values().find_map(|node| {
                let id = node["element_id"].as_str()?;
                let label = node["aria"]["label"].as_str()?;
                (id.starts_with("Name(\"library-row-") && label.starts_with(&prefix)).then_some(id)
            })
        });
        let row =
            row.ok_or_else(|| std::io::Error::other(format!("Нет строки трека «{title}».")))?;
        press(raw, row, cx)?;
        let playback = sound(window, title, cx).await?;
        let peak = Cell::new([0.0_f32; 3]);
        let levels = wait(
            cx,
            &format!("полоса сигнала {frequency} Гц"),
            |cx| {
                let (levels, _) = sample(window, cx)?;
                let previous = peak.get();
                peak.set(std::array::from_fn(|band| previous[band].max(levels[band])));
                let next = levels
                    .iter()
                    .enumerate()
                    .filter(|(band, _)| *band != index)
                    .map(|(_, level)| *level)
                    .fold(0.0_f32, f32::max);
                Ok(
                    (levels[index] > MINIMUM_SIGNAL && levels[index] > next * 1.5)
                        .then_some(levels),
                )
            },
        )
        .await
        .map_err(|error| {
            let status = window
                .update(cx, |view, _, cx| {
                    view.fluid_glass_audio_status_for_capture(cx)
                })
                .ok()
                .flatten();
            io::Error::other(format!(
                "{error} Пики полос: {:?}. Состояние WASAPI и признак воспроизведения: {status:?}.",
                peak.get()
            ))
        })?;
        let playing = format!("audio-{frequency}-hz-playing.png");
        draw(raw, scene, Some(&output.join(&playing)), cx)?;
        let docked_bands = if index == 0 {
            wait(
                cx,
                "басовый сигнал для проверки контраста",
                |cx| {
                    let (bands, _) = sample(window, cx)?;
                    Ok((bands[0] >= DOCK_TEST_BASS).then_some(()))
                },
            )
            .await?;
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_dock_for_capture(Some(Duration::ZERO), cx);
            })?;
            press(raw, &named(QUEUE_BUTTON), cx)?;
            let _ = frame(raw, scene, output, "audio-80-hz-queue-opening", cx)?;
            // At the start of the morph, the dock has zero width. The
            // fixed-width contrast sampler applies once the dock is open.
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_dock_for_capture(Some(Duration::from_millis(420)), cx);
            })?;
            let (docked_bands, _) = sample(window, cx)?;
            require(
                docked_bands[0] >= DOCK_TEST_BASS,
                "Контраст очереди должен проверяться при сильном басе.",
            )?;
            let queue = frame(raw, scene, output, "audio-80-hz-queue", cx)?;
            require(
                node_label(&queue, QUEUE_LIST) == Some("Треки в очереди"),
                "Проверка контраста должна захватить открытую очередь.",
            )?;
            let contrast = check_dock_pixels(
                output,
                "audio-80-hz-queue",
                "audio-80-hz-queue-contrast.json",
                super::queue::background_top(window, scene, cx)?,
            );
            key(raw, "escape", cx)?;
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_dock_for_capture(None, cx);
            })?;
            settle_panel(raw, scene, cx).await?;
            contrast?;
            Some(docked_bands)
        } else {
            None
        };
        let waveform = if frequency == WAVEFORM_TONE_HZ {
            Some(verify_waveform_playing(window, scene, output, cx).await?)
        } else {
            None
        };

        press(raw, &named(PLAY_BUTTON), cx)?;
        if let Some(waveform) = waveform {
            verify_waveform_paused(window, scene, output, waveform, cx).await?;
        }
        let quiet = wait(
            cx,
            "пауза и затухание подсветки",
            |cx| {
                let (levels, tree) = sample(window, cx)?;
                Ok((node_label(&tree, PLAY_BUTTON) == Some("Воспроизвести")
                    && levels
                        .iter()
                        .all(|&level| (0.0..QUIET_SIGNAL).contains(&level)))
                .then_some(levels))
            },
        )
        .await?;
        let paused = format!("audio-{frequency}-hz-paused.png");
        draw(raw, scene, Some(&output.join(&paused)), cx)?;
        let changes = pixel_changes(&output.join(&playing), &output.join(&paused))?;
        samples.push(json!({
            "title": title,
            "frequency_hz": frequency,
            "playing": {"bands": levels, "frame": playing, "audio": playback},
            "paused": {"bands": quiet, "frame": paused},
            "docked_bands": docked_bands,
            "changed_pixels": changes,
        }));
        fs::write(
            output.join("atmosphere-audio.json"),
            serde_json::to_vec_pretty(&json!({
                "shader_phase_ms": PHASE.as_millis(),
                "band_order": ["bass", "mid", "treble"],
                "regions_exclusive_right_bottom": REGIONS,
                "samples": samples,
            }))?,
        )?;
        let responding_pixels = if index == 2 {
            changes[1] + changes[2]
        } else {
            changes[0]
        };
        require(
            responding_pixels > 0,
            &format!(
                "Световой рисунок не отреагировал на {frequency} Гц при одинаковой фазе шейдера."
            ),
        )?;
        println!(
            "Fluid Glass {frequency} Hz bands {levels:.3?} → {quiet:.3?}; changed pixels {changes:?}"
        );
    }
    Ok(())
}

fn sample(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<([f32; 3], Value)> {
    // The hidden window has no scheduled backdrop updates. Invalidate the
    // cached layer at the same shader phase so it reads the newest live bands.
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_audio_for_capture(PHASE, cx)
    })?;
    let tree = snapshot(window.into(), cx)?;
    let levels = window.update(cx, |view, _, cx| view.fluid_glass_audio_for_capture(cx))?;
    Ok((levels, tree))
}

async fn verify_waveform_playing(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let (gain, envelope, span) = wait(
        cx,
        "форма волны модулированного WAV",
        |cx| {
            let (gain, envelope) = waveform_sample(window, cx)?;
            let min = envelope.iter().copied().fold(1.0_f32, f32::min);
            let max = envelope.iter().copied().fold(0.0_f32, f32::max);
            let span = max - min;
            Ok((gain > 0.02
                && envelope.iter().all(|&level| level > 0.0 && level <= 1.0)
                && span > 0.02)
                .then_some((gain, envelope, span)))
        },
    )
    .await?;

    let raw = window.into();
    let playing_tree = snapshot(raw, cx)?;
    // Capture both states in one turn without sleeping or writing PNGs between frames.
    let playing = render_waveform(window, scene, true, cx)?;
    let hidden =
        render_waveform(window, scene, false, cx).and_then(|frame| Ok((frame, snapshot(raw, cx)?)));
    window.update(cx, |view, _, cx| {
        view.show_fluid_glass_waveform_for_capture(true, cx);
    })?;
    let (hidden, hidden_tree) = hidden?;
    playing.save(output.join("waveform-playing.png"))?;
    hidden.save(output.join("waveform-hidden.png"))?;
    let slots = compare_pixels(&playing, &hidden, WAVEFORM_SLOTS);
    let [opaque_control_changes] = compare_pixels(&playing, &hidden, PAUSE_BUTTON_INTERIOR);
    let transport_preserved = [&playing_tree, &hidden_tree].iter().all(|tree| {
        node_label(tree, PLAY_BUTTON) == Some("Пауза")
            && node_label(tree, SEEK) == Some("Позиция воспроизведения")
    });
    println!(
        "Waveform gain {gain:.3}, envelope span {span:.3}, slot pixels {slots:?}, changed pause-button pixels {opaque_control_changes}"
    );
    require(
        slots.iter().all(|&count| count > 100)
            && opaque_control_changes == 0
            && transport_preserved,
        "Форма волны должна рисоваться в обеих областях и не изменять пиксели транспорта.",
    )?;
    Ok(json!({
        "gain": gain,
        "rms": envelope,
        "rms_span": span,
        "changed_slot_pixels": slots,
        "changed_pause_button_pixels": opaque_control_changes,
        "transport_labels_preserved": transport_preserved,
    }))
}

async fn verify_waveform_paused(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    playing: Value,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let (gain, envelope) = wait(
        cx,
        "полное затухание формы волны после паузы",
        |cx| {
            let (gain, envelope) = waveform_sample(window, cx)?;
            Ok((gain == 0.0 && envelope == [0.0; 32]).then_some((gain, envelope)))
        },
    )
    .await?;
    render_waveform(window, scene, true, cx)?.save(output.join("waveform-paused.png"))?;
    fs::write(
        output.join("waveform.json"),
        serde_json::to_vec_pretty(&json!({
            "shader_phase_ms": PHASE.as_millis(),
            "carrier_hz": WAVEFORM_TONE_HZ,
            "slot_regions_exclusive_right_bottom": WAVEFORM_SLOTS,
            "opaque_pause_button_region_exclusive_right_bottom": PAUSE_BUTTON_INTERIOR[0],
            "playing": playing,
            "paused": {"gain": gain, "rms": envelope},
        }))?,
    )?;
    println!("Waveform gain decayed to {gain:.3} after keyboard pause");
    Ok(())
}

fn waveform_sample(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<(f32, [f32; 32])> {
    sample(window, cx)?;
    window.update(cx, |view, _, cx| view.fluid_glass_waveform_for_capture(cx))
}

fn render_waveform(
    window: WindowHandle<MainView>,
    scene: Scene,
    visible: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<image::RgbaImage> {
    window.update(cx, |view, _, cx| {
        view.show_fluid_glass_waveform_for_capture(visible, cx);
    })?;
    super::super::render_image(window.into(), scene, cx)
}

fn pixel_changes(playing: &Path, paused: &Path) -> gpui::Result<[usize; REGIONS.len()]> {
    let playing = image::open(playing)?.into_rgba8();
    let paused = image::open(paused)?.into_rgba8();
    Ok(compare_pixels(&playing, &paused, REGIONS))
}

fn compare_pixels<const N: usize>(
    before: &image::RgbaImage,
    after: &image::RgbaImage,
    regions: [[u32; 4]; N],
) -> [usize; N] {
    regions.map(|[left, top, right, bottom]| {
        (top..bottom)
            .flat_map(|y| (left..right).map(move |x| (x, y)))
            .filter(|&(x, y)| before.get_pixel(x, y) != after.get_pixel(x, y))
            .count()
    })
}
