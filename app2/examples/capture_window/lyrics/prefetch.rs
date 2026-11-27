use std::{fs, io, path::Path, time::Instant};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;

use super::{
    SCENES, Scene, fail, frame, layout, load_playlist, press, record, require_node, start_guest,
    wait,
};

const TRACKS: usize = 4;

pub(super) fn run(output: &Path, audio: &Path) -> gpui::Result<()> {
    let profile = tempfile::tempdir()?;
    let mut files = Vec::new();
    for index in 0..TRACKS {
        let file = profile.path().join(format!("Предзагрузка {index}.mp3"));
        fs::copy(audio, &file)?;
        fs::write(
            file.with_extension("lrc"),
            format!("[00:00.00]Трек {index}\n[00:03.00]Вторая строка\n"),
        )?;
        files.push(file);
    }
    let playlist = mediadeka::seed_playlist_capture(profile.path(), &files)?;
    // This capture process owns its isolated profile.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    let output = output.to_owned();
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                let view = cx
                    .update(|cx| mediadeka::open_capture_window(cx, layout(scene)))
                    .unwrap_or_else(|error| fail(error));
                let result = capture(view, scene, playlist, &output.join(scene.name), cx).await;
                result.unwrap_or_else(|error| fail(error));
                let _ = view.update(cx, |_, window, _| window.remove_window());
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn capture(
    view: WindowHandle<MainView>,
    scene: Scene,
    playlist: i64,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let raw = AnyWindowHandle::from(view);
    start_guest(view, cx).await?;
    load_playlist(scene, view, playlist, TRACKS, cx).await?;
    view.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(0, cx)
    })?;
    let hidden = wait(view, cx, |state| {
        state["visible"] == false
            && state["source"] == "Локальный файл"
            && state["prefetch"]["upcoming"] == 2
            && state["prefetch"]["completed"] == 2
            && state["prefetch"]["prepared"] == 2
            && state["playing"] == true
            && state["player_position"]
                .as_f64()
                .is_some_and(|position| position > 0.1)
    })
    .await?;
    record(output, "hidden", &hidden)?;
    frame(raw, scene, output, "hidden", cx)?;

    let started = Instant::now();
    let mut opened = view.update(cx, |view, window, cx| {
        view.show_panel_for_capture("lyrics", window, cx);
        view.lyrics_state_for_capture(cx)
    })?;
    if opened["source"] != "Локальный файл" || opened["lines"] != 2 {
        return Err(io::Error::other("Подготовленный текст пропал при открытии панели.").into());
    }
    opened["panel_state_ms"] = serde_json::json!(started.elapsed().as_secs_f64() * 1000.);
    let tree = frame(raw, scene, output, "opened", cx)?;
    require_node(&tree, "right-panel-lyrics")?;
    opened["panel_capture_ms"] = serde_json::json!(started.elapsed().as_secs_f64() * 1000.);
    record(output, "opened", &opened)?;

    view.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(1, cx)
    })?;
    let advanced = wait(view, cx, |state| {
        state["track"] == 1
            && state["source"] == "Локальный файл"
            && state["prefetch"]["prepared"] == 2
            && state["playing"] == true
    })
    .await?;
    if advanced["current_track"] == hidden["current_track"] {
        return Err(io::Error::other("Плеер не перешёл на следующую запись.").into());
    }
    record(output, "advanced", &advanced)?;
    frame(raw, scene, output, "advanced", cx)?;

    press(raw, "player-lyrics", cx)?;
    view.update(cx, |view, _, cx| view.playlist_scroll_for_capture(0, cx))?;
    press(raw, "playlist-track-title-0", cx)?;
    press(raw, "music-object-lyrics", cx)?;
    let inspected = wait(view, cx, |state| {
        state["lyrics_track"] == hidden["lyrics_track"]
            && state["current_track"] == advanced["current_track"]
            && state["seekable"] == false
            && state["source"] == "Локальный файл"
    })
    .await?;
    record(output, "inspected", &inspected)?;
    frame(raw, scene, output, "inspected", cx)?;
    press(raw, "player-listening", cx)?;
    press(raw, "listening-text", cx)?;
    let immersive = wait(view, cx, |state| {
        state["listening_mode"] == "lyrics"
            && state["lyrics_track"] == advanced["lyrics_track"]
            && state["current_track"] == advanced["current_track"]
            && state["seekable"] == true
            && state["source"] == "Локальный файл"
    })
    .await?;
    record(output, "immersive-current", &immersive)?;
    let tree = frame(raw, scene, output, "immersive-current", cx)?;
    require_node(&tree, "lyrics-fullscreen")?;
    Ok(())
}
