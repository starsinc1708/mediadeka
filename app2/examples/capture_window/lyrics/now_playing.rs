use std::{
    ffi::OsString,
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{
    CAPTURE_ARTWORK, SCENES, Scene, draw, ensure_same_playback, fail, focus, focused_id, key,
    layout, load_playlist, node, record, require_node, start_guest, tree,
};

#[path = "now_playing/first_artwork.rs"]
mod first_artwork;

const LRC: &str = concat!(
    "[00:00.00]Первая строка проверочной записи\n",
    "[00:03.00]Продолжается воспроизведение\n",
    "[00:06.00]Третья строка проверочной записи\n",
    "[00:09.00]Четвёртая строка проверочной записи\n",
    "[00:12.00]Звук продолжается после перехода\n",
);
const TITLE: &str = "Ночная запись";
const ARTIST: &str = "Тестовый исполнитель";

pub(super) fn run(arguments: &[OsString]) -> gpui::Result<()> {
    let [output, mode, audio, cover, flags @ ..] = arguments else {
        return Err(io::Error::other("--lyrics-stage6 --now-playing <кадры> <1920x1080|800x400|800x400-150pct> <MP3 с тегами от 25 с> <файл .cover> [--first-artwork]").into());
    };
    let first_artwork = match flags {
        [] => false,
        [flag] if flag == "--first-artwork" => true,
        _ => return Err(io::Error::other("Неизвестный параметр проверки обложки.").into()),
    };
    let base = SCENES
        .into_iter()
        .find(|scene| mode == scene.name)
        .ok_or_else(|| io::Error::other("Неизвестная раскладка режима прослушивания."))?;
    let scene = Scene {
        reduced: false,
        ..base
    };
    let output = Path::new(output).to_owned();
    let profile = tempfile::tempdir()?;
    let current = profile.path().join("Проверка режима прослушивания.mp3");
    let next = profile.path().join("Следующая запись.mp3");
    fs::copy(audio, &current)?;
    fs::copy(audio, &next)?;
    fs::write(current.with_extension("lrc"), LRC)?;
    fs::write(
        next.with_extension("lrc"),
        "[00:00.00]Текст следующего трека",
    )?;
    let playlist = mediadeka::seed_playlist_capture(profile.path(), &[current, next])?;
    MainView::seed_artist_artwork_for_capture(profile.path(), CAPTURE_ARTWORK, Path::new(cover))?;
    // The native capture owns the clean guest profile before opening its only window.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let view =
            mediadeka::open_capture_window(cx, layout(scene)).unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            verify(view, scene, playlist, &output, first_artwork, cx)
                .await
                .unwrap_or_else(|error| fail(error));
            let _ = view.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    playlist: i64,
    output: &Path,
    first_artwork: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    start_guest(view, cx).await?;
    load_playlist(scene, view, playlist, 2, cx).await?;
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, _, cx| {
        view.play_fluid_glass_transition_fixture_for_capture(
            playlist,
            CAPTURE_ARTWORK,
            CAPTURE_ARTWORK,
            cx,
        )
    })??;
    let startup = Instant::now();
    let timer = cx.background_executor().clone();
    let (ready, initial_sound, labels) = loop {
        // In the hidden D3D11 window, explicitly run the application's real process loopback.
        view.update(cx, |view, _, cx| {
            view.sample_fluid_glass_audio_for_capture(startup.elapsed(), cx);
        })?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        let (state, sound, labels, palette, bands) = view.update(cx, |view, _, cx| {
            (
                view.lyrics_state_for_capture(cx),
                view.sound_state_for_capture(cx),
                view.fluid_track_labels_for_capture(cx),
                view.fluid_glass_palette_ready_for_capture(CAPTURE_ARTWORK, cx),
                view.fluid_glass_audio_for_capture(cx),
            )
        })?;
        if let Some(sound) = sound
            && prepared_text(&state)
            && state["loading"] == false
            && state["playing"] == true
            && state["visible"] == false
            && state["prefetch"]["prepared"] == 1
            && state["player_position"]
                .as_f64()
                .is_some_and(|position| position > 1.0)
            && palette
            && labels
                .iter()
                .any(|(title, artist, _)| title == TITLE && artist == ARTIST)
            && bands.iter().any(|&level| level > 0.02)
        {
            break (state, sound, labels);
        }
        if startup.elapsed() > Duration::from_secs(12) {
            return Err(io::Error::other(format!("Метаданные, палитра или звук не готовы до открытия панели: {state}; labels={labels:?}, bands={bands:?}, palette={palette}.")).into());
        }
        timer.timer(Duration::from_millis(40)).await;
    };
    record(
        output,
        "before",
        &json!({"lyrics": ready, "sound": initial_sound, "palette_ready": true, "labels": labels}),
    )?;
    let origin = Instant::now();
    eprintln!("[now-playing] begin {}", scene.name);
    let steps = [
        (
            "docked-text",
            "player-lyrics",
            "right-panel-lyrics",
            true,
            None,
        ),
        (
            "queue",
            "right-panel-queue",
            "player-queue-list",
            false,
            None,
        ),
        (
            "docked-return",
            "right-panel-lyrics",
            "right-panel-lyrics",
            true,
            None,
        ),
        (
            "immersive-cover",
            "player-listening",
            "fluid-listening-artwork",
            true,
            Some("cover"),
        ),
        (
            "immersive-text",
            "listening-text",
            "playback-lyrics",
            true,
            Some("lyrics"),
        ),
        ("exit-immersive", "escape", "right-panel-lyrics", true, None),
    ];
    let mut previous_position = ready["player_position"].as_f64().unwrap();
    let mut cover_end = None;
    for (name, action, control, visible, mode) in steps {
        if action != "escape" {
            focus(raw, action, cx)?;
        }
        let capture_artwork = first_artwork && mode.is_some();
        if capture_artwork {
            first_artwork::sample(view, 0, cx)?;
        }
        eprintln!(
            "[now-playing] before {name} elapsed_ms={}",
            origin.elapsed().as_millis()
        );
        let started = Instant::now();
        key(
            raw,
            if action == "escape" {
                "escape"
            } else {
                "enter"
            },
            cx,
        )?;
        // The action effect commits after the input update used by `key`.
        let first = tree(raw, cx)?;
        let first_frame_ms = started.elapsed().as_secs_f64() * 1000.;
        if capture_artwork {
            let origin = first_artwork::capture(view, output, name, 0, cx)?;
            if let Some(cover) = cover_end {
                first_artwork::check_origin(output, cover, origin)?;
            }
        }
        require_node(&first, control)?;
        if node(&first, "lyrics-refresh").is_some() {
            return Err(io::Error::other(format!(
                "Переход {name} повторно запустил загрузку текста."
            ))
            .into());
        }
        if node(&first, "playback-lyrics").is_some() != (visible && mode != Some("cover")) {
            return Err(io::Error::other(format!(
                "Переход {name} потерял состояние вкладки текста."
            ))
            .into());
        }
        let (state, sound, art_ready, palette_ready) = view.update(cx, |view, window, cx| {
            let state = view.lyrics_state_for_capture(cx);
            let sound = view.sound_state_for_capture(cx);
            let art_ready = mode.map(|_| view.listening_artwork_decoded_for_capture(window, cx));
            let palette = view.fluid_glass_palette_ready_for_capture(CAPTURE_ARTWORK, cx);
            (state, sound, art_ready, palette)
        })?;
        ensure_same_playback(&ready, &state)?;
        let sound =
            sound.ok_or_else(|| io::Error::other("Данные звука исчезли во время перехода."))?;
        let position = state["player_position"].as_f64().unwrap();
        if !prepared_text(&state)
            || state["visible"] != visible
            || state["listening_mode"].as_str() != mode
            || state["playing"] != true
            || position < previous_position
            || sound["source"] != initial_sound["source"]
            || sound["time_to_sound_ms"] != initial_sound["time_to_sound_ms"]
            || !palette_ready
        {
            return Err(io::Error::other(format!("Переход {name} потерял подготовленные данные или изменил звук: {state}; {sound}; palette_ready={palette_ready}." )).into());
        }
        let expected_focus = match name {
            "immersive-cover" => Some("Name(\"listening-fullscreen\")"),
            "immersive-text" => Some("Name(\"listening-text\")"),
            "exit-immersive" => Some("Name(\"player-listening\")"),
            _ => None,
        };
        if expected_focus.is_some() && focused_id(&first) != expected_focus {
            return Err(io::Error::other(format!("Переход {name} потерял фокус.")).into());
        }
        if capture_artwork {
            for phase in [100, 260] {
                first_artwork::sample(view, phase, cx)?;
                raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
                first_artwork::capture(view, output, name, phase, cx)?;
            }
        }
        let art_ready_ms = if art_ready == Some(false) {
            super::await_artwork(view, cx).await?;
            Some(started.elapsed().as_secs_f64() * 1000.)
        } else {
            art_ready.map(|_| first_frame_ms)
        };
        if capture_artwork {
            first_artwork::sample(view, 520, cx)?;
            raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
            let ready_bounds = first_artwork::capture(view, output, name, 520, cx)?;
            if mode == Some("cover") {
                cover_end = Some(ready_bounds);
            }
            view.update(cx, |view, _, cx| {
                view.sample_listening_artwork_for_capture(None, cx);
            })?;
        }
        previous_position = position;
        eprintln!(
            "[now-playing] after {name} elapsed_ms={} position={position:.3}",
            origin.elapsed().as_millis()
        );
        draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
        record(
            output,
            name,
            &json!({
                "main_content_ms": first_frame_ms, "art_ready_on_first_frame": art_ready,
                "art_ready_ms": art_ready_ms, "sound": sound, "lyrics": state, "focus": focused_id(&first),
            }),
        )?;
        // Let the independent playback poll publish position between synchronous captures.
        timer.timer(Duration::from_millis(180)).await;
    }
    eprintln!(
        "[now-playing] end elapsed_ms={}",
        origin.elapsed().as_millis()
    );
    cx.background_executor()
        .timer(Duration::from_millis(350))
        .await;
    println!(
        "{}: six transitions retained the decoded track, queue and prepared lyrics",
        scene.name
    );
    Ok(())
}

fn prepared_text(state: &Value) -> bool {
    state["source"] == "Локальный файл" && state["timed_lines"] == 5
}
