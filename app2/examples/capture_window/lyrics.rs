#[path = "lyrics/contrast.rs"]
mod contrast;
#[path = "lyrics/karaoke.rs"]
mod karaoke;
#[path = "lyrics/now_playing.rs"]
mod now_playing;
#[path = "lyrics/panel_resize.rs"]
mod panel_resize;
#[path = "lyrics/prefetch.rs"]
mod prefetch;
#[path = "lyrics/versions.rs"]
mod versions;
#[path = "lyrics/yandex_versions.rs"]
mod yandex_versions;
#[path = "lyrics/youtube_public.rs"]
mod youtube_public;

use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, KeyUpEvent, Keystroke, MouseButton, WindowHandle,
};
use mediadeka::{CaptureLayout, MainView};
use serde_json::Value;

use super::{
    Scene, draw, fail,
    home::click,
    native_dpi_change,
    sound::{accessibility, activate_accessibility, node, node_label},
};

const SCENES: [Scene; 3] = [
    Scene {
        name: "1920x1080",
        width: 1920.,
        height: 1080.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "800x400",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: true,
    },
    Scene {
        name: "800x400-150pct",
        width: 800.,
        height: 400.,
        dpi: 144,
        reduced: true,
    },
];
const CAPTURE_ARTWORK: &str = "https://i.scdn.co/image/ab67616d00001e02bc24b6dc33b1c0fd6372e3a9";
const LRC: &str = concat!(
    "[00:00.10]<00:00.10>Тихо <00:00.55>гаснут <00:01.10>фонари\n",
    "[00:02.00]<00:02.00>Снова <00:02.45>музыка <00:03.10>звучит\n",
    "[00:04.00]<00:04.00>Ночь <00:04.45>проводит <00:05.10>нас домой\n",
    "[00:06.00]<00:06.00>Утро <00:06.45>встретим <00:07.10>мы с тобой\n",
);

pub(super) fn run(args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Check {
        Listening,
        Karaoke,
        HighContrast,
        PanelResize,
        Prefetch,
        Versions,
        LocalVersions,
        YandexVersions,
        YouTubePublic,
        NowPlaying,
    }

    let arguments = args.collect::<Vec<_>>();
    let (check, arguments) = match arguments.as_slice() {
        [mode, rest @ ..] if mode == "--karaoke" => (Check::Karaoke, rest),
        [mode, rest @ ..] if mode == "--high-contrast" => (Check::HighContrast, rest),
        [mode, rest @ ..] if mode == "--panel-resize" => (Check::PanelResize, rest),
        [mode, rest @ ..] if mode == "--prefetch" => (Check::Prefetch, rest),
        [mode, rest @ ..] if mode == "--versions" => (Check::Versions, rest),
        [mode, rest @ ..] if mode == "--local-versions" => (Check::LocalVersions, rest),
        [mode, rest @ ..] if mode == "--yandex-versions" => (Check::YandexVersions, rest),
        [mode, rest @ ..] if mode == "--youtube-public" => (Check::YouTubePublic, rest),
        [mode, rest @ ..] if mode == "--now-playing" => (Check::NowPlaying, rest),
        arguments => (Check::Listening, arguments),
    };
    if check == Check::YandexVersions {
        return yandex_versions::run(arguments);
    }
    if check == Check::YouTubePublic {
        return youtube_public::run(arguments);
    }
    if check == Check::NowPlaying {
        return now_playing::run(arguments);
    }
    let karaoke = check == Check::Karaoke;
    let high_contrast = check == Check::HighContrast;
    let (output, audio, artwork) = match arguments {
        [output, audio] => (output, audio, None),
        [output, audio, artwork] => (output, audio, Some(Path::new(artwork))),
        _ => {
            return Err(io::Error::other(
                "--lyrics-stage6 [--karaoke | --high-contrast | --panel-resize | --prefetch | --versions | --local-versions] <каталог кадров> <MP3 от 8 сек. (караоке от 12)> [<файл обложки .cover>]",
            )
            .into());
        }
    };
    if high_contrast && env::var_os("MEDIADEKA_GUI").is_none_or(|value| value != "3") {
        return Err(io::Error::other("Для --high-contrast требуется MEDIADEKA_GUI=3.").into());
    }
    if check == Check::PanelResize && env::var_os("MEDIADEKA_GUI").is_none_or(|value| value != "3")
    {
        return Err(io::Error::other("Для --panel-resize требуется MEDIADEKA_GUI=3.").into());
    }
    let output = PathBuf::from(output);
    fs::create_dir_all(&output)?;
    if check == Check::Prefetch {
        return prefetch::run(&output, Path::new(audio));
    }
    if matches!(check, Check::Versions | Check::LocalVersions) {
        return versions::run(&output, Path::new(audio), check == Check::LocalVersions);
    }
    let profile = tempfile::tempdir()?;
    let with_text = profile.path().join("Ночная музыка.mp3");
    let without_text = profile.path().join("Утро без слов.mp3");
    fs::copy(audio, &with_text)?;
    fs::copy(audio, &without_text)?;
    fs::write(
        with_text.with_extension("lrc"),
        if karaoke { karaoke::LRC } else { LRC },
    )?;
    if let Some(artwork) = artwork {
        MainView::seed_artist_artwork_for_capture(profile.path(), CAPTURE_ARTWORK, artwork)?;
    }
    let artwork = artwork.map(|_| CAPTURE_ARTWORK);
    let playlist = mediadeka::seed_playlist_capture(profile.path(), &[with_text, without_text])?;
    // GPUI and the playback worker have not started yet. This process owns the profile.
    unsafe { std::env::set_var("MEDIADEKA_PROFILE_DIR", profile.path()) };
    let scenes = if karaoke {
        &karaoke::SCENES_WITH_KARAOKE[..]
    } else if check == Check::PanelResize {
        &SCENES[..1]
    } else {
        &SCENES
    };
    mediadeka::application().run(move |cx| {
        let anchor = mediadeka::open_capture_window(cx, layout(scenes[0]))
            .unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            let _profile = profile;
            for &scene in scenes {
                let result = match check {
                    Check::Karaoke => karaoke::capture(scene, playlist, artwork, &output, cx).await,
                    Check::PanelResize => panel_resize::capture(playlist, &output, cx).await,
                    _ => capture(scene, playlist, artwork, high_contrast, &output, cx).await,
                };
                result.unwrap_or_else(|error| fail(error));
            }
            let _ = anchor.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

pub(super) fn run_genius(args: impl Iterator<Item = OsString>) -> gpui::Result<()> {
    let arguments = args.collect::<Vec<_>>();
    let [output, audio] = arguments.as_slice() else {
        return Err(io::Error::other(
            "--lyrics-genius-stage6 <каталог кадров> <MP3 с тегами Creep и Radiohead>",
        )
        .into());
    };
    let profile = PathBuf::from(
        env::var_os("MEDIADEKA_PROFILE_DIR")
            .ok_or_else(|| io::Error::other("Укажите изолированный профиль с Genius."))?,
    );
    let fragment = env::var("MEDIADEKA_GENIUS_CAPTURE_FRAGMENT")?;
    let fragment = fragment.split_whitespace().collect::<Vec<_>>().join(" ");
    if fragment.is_empty() || fragment.len() > 480 {
        return Err(
            io::Error::other("Нужен короткий фрагмент существующего пояснения Genius.").into(),
        );
    }
    let output = PathBuf::from(output);
    fs::create_dir_all(&output)?;
    let with_text = profile.join("genius-capture-fixture.mp3");
    fs::copy(audio, &with_text)?;
    fs::write(
        with_text.with_extension("lrc"),
        format!("[00:00.00]{fragment}\n"),
    )?;
    let playlist = mediadeka::seed_playlist_capture(&profile, &[with_text])?;
    let offline = env::var_os("MEDIADEKA_GENIUS_CAPTURE_OFFLINE").is_some();
    mediadeka::application().run(move |cx| {
        let scene = SCENES[0];
        let view =
            mediadeka::open_capture_window(cx, layout(scene)).unwrap_or_else(|error| fail(error));
        cx.spawn(async move |cx| {
            if let Err(error) = capture_genius(scene, view, playlist, &output, offline, cx).await {
                fail(error);
            }
            let _ = view.update(cx, |_, window, _| window.remove_window());
        })
        .detach();
    });
    Ok(())
}

async fn capture_genius(
    scene: Scene,
    view: WindowHandle<MainView>,
    playlist: i64,
    output: &Path,
    offline: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    start_playlist(scene, view, playlist, 1, cx).await?;
    if offline {
        let state = wait(view, cx, |state| {
            state["genius_failed"] == true
                && state["source"] == "Локальный файл"
                && state["lines"] == 1
                && state["seekable"] == true
        })
        .await?;
        let tree = frame(raw, scene, output, "genius-unavailable", cx)?;
        require_node(&tree, "lyrics-line-0")?;
        record(output, "genius-unavailable", &state)?;
        println!("Genius unavailable: local lyric and playback retained");
        return Ok(());
    }
    let state = wait(view, cx, |state| {
        state["genius"]["marked_lines"] == 1
            && state["genius"]["writers"]
                .as_u64()
                .is_some_and(|count| count > 0)
            && state["seekable"] == true
    })
    .await?;
    frame(raw, scene, output, "genius", cx)?;
    let target = view
        .update(cx, |view, _, cx| view.lyrics_note_target_for_capture(0, cx))?
        .ok_or_else(|| io::Error::other("У первой строки нет видимого пояснения."))?;
    click(raw, MouseButton::Left, target, cx)?;
    let open = wait(view, cx, |state| state["genius"]["open_note"] == 0).await?;
    let tree = frame(raw, scene, output, "genius-note", cx)?;
    require_node(&tree, "lyrics-note-body-0")?;
    record(output, "genius", &state)?;
    record(output, "genius-note", &open)?;
    println!("Genius: {}", open["genius"]);
    Ok(())
}

fn layout(scene: Scene) -> CaptureLayout {
    CaptureLayout {
        width: scene.width,
        height: scene.height,
        reduced_motion: scene.reduced,
    }
}

async fn capture(
    scene: Scene,
    playlist: i64,
    artwork: Option<&str>,
    high_contrast: bool,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(view);
    if high_contrast {
        view.update(cx, |view, _, cx| {
            view.set_fluid_high_contrast_for_capture(false, cx);
        })?;
    }
    start_guest(view, cx).await?;
    start_playlist(scene, view, playlist, 2, cx).await?;
    let panel = wait(view, cx, |value| {
        value["source"] == "Локальный файл"
            && value["lines"] == 4
            && value["seekable"] == true
            && value["line"] == 0
    })
    .await?;
    let panel_tree = frame(raw, scene, &output, "panel", cx)?;
    require_node(&panel_tree, "right-panel-lyrics")?;
    require_node(&panel_tree, "lyrics-line-0")?;
    record(&output, "panel", &panel)?;

    let seeked = seek_line(view, 1, 2., cx).await?;
    if node(&tree(raw, cx)?, "playlist-menu").is_some() {
        return Err(
            io::Error::other("Щелчок по строке текста открыл меню под плавающей панелью.").into(),
        );
    }
    record(&output, "seek", &seeked)?;

    press(raw, "player-play", cx)?;
    let paused = wait(view, cx, |value| value["playing"] == false).await?;
    if let Some(artwork) = artwork {
        view.update(cx, |view, _, cx| {
            view.set_listening_artwork_for_capture(artwork, cx)
        })?;
    }
    press(raw, "player-listening", cx)?;
    let cover = wait(view, cx, |value| value["listening_mode"] == "cover").await?;
    ensure_same_pause(&paused, &cover)?;
    let cover_focus = tree(raw, cx)?;
    for shortcut in ["ctrl-k", "ctrl-f"] {
        key(raw, shortcut, cx)?;
        let state = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
        if state["listening_mode"] != "cover"
            || focused_id(&tree(raw, cx)?) != focused_id(&cover_focus)
        {
            return Err(
                io::Error::other(format!("{shortcut} открыл поиск поверх обложки.")).into(),
            );
        }
    }
    if artwork.is_some() {
        await_artwork(view, cx).await?;
    }
    let cover_tree = frame(raw, scene, &output, "listening-cover", cx)?;
    for id in [
        "listening-fullscreen",
        "listening-back",
        "listening-cover",
        "listening-text",
        "player-play",
    ] {
        require_node(&cover_tree, id)?;
    }
    if env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3") {
        verify_fluid_listening_artwork(&cover_tree)?;
    }
    if node(&cover_tree, "right-panel-lyrics").is_some() {
        return Err(io::Error::other("Панель текста видна поверх режима прослушивания.").into());
    }
    record(&output, "listening-cover", &cover)?;

    press(raw, "listening-text", cx)?;
    let text = wait(view, cx, |value| value["listening_mode"] == "lyrics").await?;
    ensure_same_pause(&paused, &text)?;
    if artwork.is_some() && env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3") {
        await_artwork(view, cx).await?;
    }
    let lyrics_tree = frame(raw, scene, &output, "fullscreen", cx)?;
    require_node(&lyrics_tree, "lyrics-fullscreen")?;
    require_node(&lyrics_tree, "lyrics-line-1")?;
    if env::var_os("MEDIADEKA_GUI").is_some_and(|value| value == "3") {
        verify_fluid_listening_artwork(&lyrics_tree)?;
    }
    record(&output, "fullscreen", &text)?;
    if high_contrast {
        contrast::verify(view, scene, &output, cx)?;
    }
    let immersive_seek = seek_line(view, 2, 4., cx).await?;
    ensure_same_playback(&paused, &immersive_seek)?;
    record(&output, "listening-seek", &immersive_seek)?;

    press(raw, "lyrics-source", cx)?;
    require_node(
        &accessibility(raw, &output.join("listening-sources-a11y.json"), cx)?,
        "lyrics-sources",
    )?;
    // The open picker is a menu of rows with one checked source, not a row of
    // framed buttons; keep a frame of it.
    draw(raw, scene, Some(&output.join("sources-menu.png")), cx)?;
    key(raw, "escape", cx)?;
    if node(&tree(raw, cx)?, "lyrics-sources").is_some() {
        return Err(io::Error::other("Escape не закрыл выбор источника текста.").into());
    }
    key(raw, "escape", cx)?;
    let returned = wait(view, cx, |value| value["listening_mode"].is_null()).await?;
    ensure_same_pause(&immersive_seek, &returned)?;
    let returned_tree = accessibility(raw, &output.join("listening-return-a11y.json"), cx)?;
    require_node(&returned_tree, "right-panel-lyrics")?;
    if focused_id(&returned_tree) != Some("Name(\"player-listening\")") {
        return Err(io::Error::other("Фокус не вернулся на кнопку режима прослушивания.").into());
    }
    record(&output, "listening-return", &returned)?;

    press(raw, "player-play", cx)?;
    let resumed = wait(view, cx, |value| {
        value["playing"] == true
            && value["player_position"]
                .as_f64()
                .is_some_and(|position| position > 4.18)
    })
    .await?;
    ensure_same_playback(&returned, &resumed)?;
    record(&output, "listening-resume", &resumed)?;
    view.update(cx, |view, _, cx| {
        view.play_playlist_track_for_capture(1, cx);
    })?;
    let missing = wait(view, cx, |value| {
        value["track"] == 1
            && value["selection"] == "Авто"
            && value["lines"] == 0
            && value["source"].is_null()
            && value["loading"] == false
            && value["seekable"] == true
    })
    .await?;
    let missing_tree = frame(raw, scene, &output, "missing", cx)?;
    if node_label(&missing_tree, "lyrics-status") != Some("Текст пока недоступен")
    {
        return Err(
            io::Error::other("После смены записи панель не показывает отсутствие текста.").into(),
        );
    }
    record(&output, "missing", &missing)?;
    require_node(&missing_tree, "lyrics-retry")?;
    require_node(&missing_tree, "lyrics-change-source")?;
    press(raw, "lyrics-change-source", cx)?;
    let picker = frame(raw, scene, &output, "missing-source-menu", cx)?;
    require_node(&picker, "lyrics-sources")?;
    key(raw, "escape", cx)?;
    if focused_id(&tree(raw, cx)?) != Some("Name(\"lyrics-change-source\")") {
        return Err(
            io::Error::other("Escape не вернул фокус к выбору другого источника текста.").into(),
        );
    }
    press(raw, "lyrics-retry", cx)?;
    wait(view, cx, |value| {
        value["track"] == 1 && value["loading"] == false
    })
    .await?;
    if focused_id(&tree(raw, cx)?) != Some("Name(\"lyrics-source\")") {
        return Err(io::Error::other("Повтор поиска потерял фокус в панели текста.").into());
    }

    key(raw, "ctrl-l", cx)?;
    wait(view, cx, |value| value["listening_mode"] == "cover").await?;
    press(raw, "listening-text", cx)?;
    let no_lyrics = wait(view, cx, |value| value["listening_mode"] == "lyrics").await?;
    ensure_same_playback(&missing, &no_lyrics)?;
    let missing_tree = frame(raw, scene, &output, "listening-missing", cx)?;
    if node_label(&missing_tree, "lyrics-status") != Some("Текст пока недоступен")
    {
        return Err(
            io::Error::other("В режиме прослушивания нет статуса отсутствия текста.").into(),
        );
    }
    record(&output, "listening-missing", &no_lyrics)?;
    press(raw, "listening-back", cx)?;
    wait(view, cx, |value| value["listening_mode"].is_null()).await?;
    println!(
        "{}: seek={immersive_seek}; returned={returned}; track-change={missing}",
        scene.name
    );
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

fn verify_fluid_listening_artwork(tree: &Value) -> gpui::Result<()> {
    let cover = node(tree, "fluid-listening-artwork");
    if !cover.is_some_and(|node| {
        node["aria"]["role"] == "Image"
            && node["aria"]["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("Обложка · "))
    }) {
        return Err(io::Error::other("Полноэкранная обложка Fluid Glass недоступна.").into());
    }
    Ok(())
}

pub(super) async fn start_guest(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    view.update(cx, |view, window, cx| {
        view.start_home_for_capture(true, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(30);
    while view
        .update(cx, |view, _, _| view.home_status_for_capture())?
        .is_none()
    {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Тестовый гостевой профиль не загрузился.").into());
        }
        timer.timer(Duration::from_millis(50)).await;
    }
    Ok(())
}

async fn start_playlist(
    scene: Scene,
    view: WindowHandle<MainView>,
    playlist: i64,
    count: usize,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    load_playlist(scene, view, playlist, count, cx).await?;
    view.update(cx, |view, window, cx| {
        view.play_playlist_track_for_capture(0, cx);
        view.show_panel_for_capture("lyrics", window, cx);
        view.choose_local_lyrics_for_capture(window, cx);
    })?;
    Ok(())
}

async fn load_playlist(
    scene: Scene,
    view: WindowHandle<MainView>,
    playlist: i64,
    count: usize,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.open_local_playlist_for_capture(playlist, window, cx)
    })?;
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(12);
    while view.update(cx, |view, _, _| view.playlist_status_for_capture())? != Some(Ok(count)) {
        if Instant::now() >= deadline {
            return Err(io::Error::other("Тестовый плейлист не открылся.").into());
        }
        timer.timer(Duration::from_millis(30)).await;
    }
    Ok(())
}

async fn seek_line(
    view: WindowHandle<MainView>,
    index: usize,
    seconds: f64,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let target = view
        .update(cx, |view, _, cx| {
            view.lyrics_line_target_for_capture(index, cx)
        })?
        .ok_or_else(|| io::Error::other("Строка текста вне области прокрутки."))?;
    click(view.into(), MouseButton::Left, target, cx)?;
    wait(view, cx, |value| {
        value["line"] == index
            && value["player_position"]
                .as_f64()
                .is_some_and(|position| (seconds - 0.05..seconds + 0.4).contains(&position))
    })
    .await
}

async fn await_artwork(view: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        let decoded = view.update(cx, |view, window, cx| {
            view.listening_artwork_decoded_for_capture(window, cx)
        })?;
        if decoded {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Обложка режима прослушивания не декодировалась.").into());
        }
        timer.timer(Duration::from_millis(30)).await;
    }
}

fn ensure_same_playback(before: &Value, after: &Value) -> gpui::Result<()> {
    if !before["current_track"].is_null()
        && before["current_track"] == after["current_track"]
        && before["queue"]["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
        && before["queue"] == after["queue"]
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Переход изменил трек или очередь. До перехода {before}. После перехода {after}."
        ))
        .into())
    }
}

fn ensure_same_pause(before: &Value, after: &Value) -> gpui::Result<()> {
    ensure_same_playback(before, after)?;
    let before_position = before["player_position"].as_f64();
    let after_position = after["player_position"].as_f64();
    if after["playing"] == false
        && before["selection"] == after["selection"]
        && before_position
            .zip(after_position)
            .is_some_and(|(before, after)| (before - after).abs() < 0.15)
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Переход изменил паузу, позицию или источник текста. До перехода {before}. После перехода {after}."
        ))
        .into())
    }
}

fn key(raw: AnyWindowHandle, input: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let keystroke = Keystroke::parse(input)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_keystroke(keystroke.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}

fn press(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    focus(raw, id, cx)?;
    key(raw, "enter", cx)
}

fn focus(raw: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let mut current = tree(raw, cx)?;
    require_node(&current, id)?;
    let stops = current["frame"]["tab_stop_count"].as_u64().unwrap_or(64);
    let expected = format!("Name({id:?})");
    let mut visited = Vec::new();
    for _ in 0..=stops {
        let focused = focused_id(&current).unwrap_or("(без фокуса)");
        if focused == expected {
            return Ok(());
        }
        if visited.last().is_none_or(|previous| previous != focused) {
            visited.push(focused.to_owned());
        }
        key(raw, "tab", cx)?;
        current = tree(raw, cx)?;
    }
    Err(io::Error::other(format!("Фокус не дошёл до {id}. Порядок Tab {visited:?}.")).into())
}

fn tree(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let tree = raw
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.debug_a11y_tree_json()
        })?
        .ok_or_else(|| io::Error::other("GPUI не построил дерево доступности."))?;
    Ok(serde_json::from_str(&tree)?)
}

fn focused_id(tree: &Value) -> Option<&str> {
    tree["nodes"][tree["gpui_focus"].as_str()?]["element_id"].as_str()
}

async fn wait(
    view: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    ready: impl Fn(&Value) -> bool,
) -> gpui::Result<Value> {
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let value = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
        if ready(&value) {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(
                io::Error::other(format!("Ожидалось состояние текста. Получено {value}")).into(),
            );
        }
        timer.timer(Duration::from_millis(15)).await;
    }
}

fn frame(
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
    accessibility(raw, &output.join(format!("{name}-a11y.json")), cx)
}

fn record(output: &Path, name: &str, value: &Value) -> gpui::Result<()> {
    fs::write(
        output.join(format!("{name}-state.json")),
        serde_json::to_vec_pretty(value)?,
    )?;
    Ok(())
}

fn require_node(tree: &Value, id: &str) -> gpui::Result<()> {
    if node(tree, id).is_some() {
        Ok(())
    } else {
        Err(io::Error::other(format!("Узел {id} отсутствует в дереве доступности.")).into())
    }
}
