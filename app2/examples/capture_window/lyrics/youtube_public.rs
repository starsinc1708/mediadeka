use std::{
    ffi::OsString,
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;

use super::{
    SCENES, Scene, draw, ensure_same_playback, fail, focused_id, key, layout, load_playlist, node,
    press, record, start_guest, tree, wait,
};

const TRACK: &str = "https://music.youtube.com/watch?v=DyDfgMOUjCI";
const VIDEO: &str = "DyDfgMOUjCI";
const SOURCE: &str = "YouTube Music";

pub(super) fn run(arguments: &[OsString]) -> gpui::Result<()> {
    let [output, audio] = arguments else {
        return Err(io::Error::other(
            "--lyrics-stage6 --youtube-public <каталог кадров> <локальное аудио от 25 секунд>",
        )
        .into());
    };
    let output = Path::new(output).to_owned();
    let audio = Path::new(audio).canonicalize()?;
    let profile = tempfile::tempdir()?;
    let playlist = mediadeka::seed_playlist_capture(profile.path(), &[audio])?;
    // All windows use this clean guest profile before background work starts.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(SCENES[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for scene in SCENES {
                let view = cx
                    .update(|cx| mediadeka::open_capture_window(cx, layout(scene)))
                    .unwrap_or_else(|error| fail(error));
                verify(view, scene, playlist, &output.join(scene.name), cx)
                    .await
                    .unwrap_or_else(|error| fail(error));
                let _ = view.update(cx, |_, window, _| window.remove_window());
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
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
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    start_guest(view, cx).await?;
    load_playlist(scene, view, playlist, 1, cx).await?;
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(0, cx)
    })?;
    let local = wait(view, cx, |state| {
        state["playing"] == true
            && state["player_position"]
                .as_f64()
                .is_some_and(|position| position > 0.05)
    })
    .await?;
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(TRACK, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(15);
    while node(&tree(raw, cx)?, "music-object-lyrics").is_none() {
        if Instant::now() >= deadline {
            return Err(
                io::Error::other("Публичная страница трека не показала кнопку текста.").into(),
            );
        }
        timer.timer(Duration::from_millis(40)).await;
    }

    press(raw, "music-object-lyrics", cx)?;
    press(raw, "lyrics-source", cx)?;
    // Starting at Auto, wrap past Genius to YouTube Music.
    key(raw, "up", cx)?;
    key(raw, "up", cx)?;
    key(raw, "enter", cx)?;
    let panel = wait(view, cx, |state| {
        state["selection"] == SOURCE
            && state["source"] == SOURCE
            && state["lyrics_track"] == VIDEO
            && state["loading"] == false
            && state["lines"] == 60
    })
    .await?;
    ensure_same_playback(&local, &panel)?;
    if panel["timed_lines"] != 0
        || panel["line"].is_number()
        || panel["seekable"] != false
        || panel["follows_playback"] != false
        || panel["playing"] != true
        || !panel["player_position"]
            .as_f64()
            .zip(local["player_position"].as_f64())
            .is_some_and(|(after, before)| after > before)
    {
        return Err(io::Error::other(
            "Публичный обычный текст нарушил воспроизведение другого трека.",
        )
        .into());
    }
    let a11y = tree(raw, cx)?;
    let focus = focused_id(&a11y);
    let group = node(&a11y, "playback-lyrics").is_some_and(|node| node["aria"]["role"] == "Group");
    if focus != Some("Name(\"lyrics-source\")") || !group {
        return Err(io::Error::other(format!(
            "Панель текста потеряла фокус или доступную группу: {focus:?}, группа {group}."
        ))
        .into());
    }
    // Lyrics appear in the requested frame. Logs retain only source and playback metadata.
    draw(raw, scene, Some(&output.join("guest-lyrics.png")), cx)?;
    record(output, "guest-lyrics", &panel)?;
    record(
        output,
        "guest-lyrics-semantic",
        &serde_json::json!({
            "focus": focus,
            "lyrics_group": group,
            "player_position_before": local["player_position"],
            "player_position_after": panel["player_position"],
        }),
    )?;
    println!(
        "{}: 60 plain lines, source selector kept keyboard focus, local playback continued",
        scene.name
    );
    Ok(())
}
