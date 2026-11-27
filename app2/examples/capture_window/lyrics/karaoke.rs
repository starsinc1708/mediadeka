use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use image::GenericImageView as _;
use mediadeka::MainView;
use serde_json::Value;

use super::{
    SCENES, Scene, await_artwork, frame, layout, press, record, require_node, start_guest,
    start_playlist, wait,
};

#[path = "karaoke/manual_follow.rs"]
mod manual_follow;

pub(super) const SCENES_WITH_KARAOKE: [Scene; 4] = [
    Scene {
        reduced: false,
        ..SCENES[0]
    },
    Scene {
        reduced: false,
        ..SCENES[1]
    },
    Scene {
        name: "800x400-reduced-motion",
        ..SCENES[1]
    },
    SCENES[2],
];

pub(super) const LRC: &str = concat!(
    "[00:00.10]<00:00.10>Тихо <00:00.48>гаснут <00:00.82>фонари\n",
    "[00:01.20]<00:01.20>Снова <00:02.00>музыка <00:02.38>звучит\n",
    "[00:02.70]Ночь проводит нас домой\n",
    "[00:03.12]Мимо тёмного окна\n",
    "[00:03.50]В отражении вода\n",
    "[00:03.88]Слышен поезд за рекой\n",
    "[00:04.26]Свет плывёт над мостовой\n",
    "[00:04.64]Тихий город под дождём\n",
    "[00:05.42]Мы по мокрым улицам идём\n",
    "[00:05.80]Ветер тронет провода\n",
    "[00:06.18]Песню подхватит река\n",
    "[00:06.56]Эта ночь ещё светла\n",
    "[00:06.94]За домами тишина\n",
    "[00:07.32]На ступенях тёплый свет\n",
    "[00:07.70]На стекле остался след\n",
    "[00:08.08]На рассвете у окна\n",
    "[00:08.46]Наконец стихает дождь\n",
    "[00:08.84]Солнце медленно встаёт\n",
    "[00:09.22]Мокрый город оживёт\n",
    "[00:09.60]Лёгкий шум за дверью\n",
    "[00:09.98]Кто-то выключил маяк\n",
    "[00:10.36]Новый день стучится в дом\n",
    "[00:10.74]Всё, что снилось, позади\n",
);

pub(super) async fn capture(
    scene: Scene,
    playlist: i64,
    artwork: Option<&str>,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let output = output.join(scene.name);
    fs::create_dir_all(&output)?;
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(scene)))?;
    let raw = AnyWindowHandle::from(view);
    start_guest(view, cx).await?;
    start_playlist(scene, view, playlist, 2, cx).await?;
    wait(view, cx, |value| {
        value["source"] == "Локальный файл"
            && value["lines"] == 23
            && value["seekable"] == true
            && value["playing"] == true
    })
    .await?;
    press(raw, "player-play", cx)?;
    wait(view, cx, |value| value["playing"] == false).await?;
    press(raw, "player-listening", cx)?;
    press(raw, "listening-text", cx)?;

    let mut sweep_samples = [0; 2];
    let positions = [("word-25", 1.4, 1400), ("word-75", 1.8, 1800)];
    for ((name, seconds, milliseconds), pixels) in positions.into_iter().zip(&mut sweep_samples) {
        view.update(cx, |view, _, cx| {
            view.seek_lyrics_playback_for_capture(seconds, cx)
        })?;
        let state = wait(view, cx, |value| {
            value["line"] == 1
                && value["word"] == 0
                && value["playing"] == false
                && value["clock_ms"]
                    .as_u64()
                    .is_some_and(|clock| clock.abs_diff(milliseconds) <= 40)
                && value["player_position"]
                    .as_f64()
                    .is_some_and(|position| (position - seconds).abs() <= 0.04)
        })
        .await?;
        paint_artwork(view, artwork, cx).await?;
        let tree = frame(raw, scene, &output, name, cx)?;
        require_node(&tree, "lyrics-line-1")?;
        let (target, accent) = view.update(cx, |view, _, cx| {
            (
                view.lyrics_line_target_for_capture(1, cx),
                view.lyrics_accent_for_capture(cx),
            )
        })?;
        let row_center =
            target.ok_or_else(|| io::Error::other("Активная строка караоке вне окна."))?;
        *pixels = accent_ink(
            &output.join(format!("{name}.png")),
            f32::from(row_center.y),
            scene,
            accent,
        )?;
        record(&output, name, &state)?;
    }
    let [early, late] = sweep_samples;
    let swept = if scene.reduced {
        early == late
    } else {
        // A 22-DIP compact line has about 40% of the glyph area of a 35-DIP line.
        let minimum = if scene.height >= 500. { 20 } else { 8 };
        late > early + minimum
    };
    if !swept || early == 0 {
        return Err(io::Error::other(format!(
            "{}: неверная заливка слова, 25% = {early} пикс., 75% = {late} пикс., reduced_motion = {}.",
            scene.name, scene.reduced
        ))
        .into());
    }

    verify_emphasis(view, scene, &output, cx).await?;
    let initial = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    press(raw, "player-play", cx)?;
    let (following, steps) = follow_karaoke(view, scene, &initial, cx).await?;
    paint_artwork(view, artwork, cx).await?;
    frame(raw, scene, &output, "follow", cx)?;
    record(&output, "follow", &following)?;
    record(
        &output,
        "karaoke",
        &serde_json::json!({
            "reduced_motion": scene.reduced,
            "accent_pixels": [early, late],
            "before_scroll_y": initial["scroll_y"],
            "after_scroll_y": following["scroll_y"],
            "max_scroll_steps_same_line": steps,
        }),
    )?;
    println!(
        "{}: accent={early}→{late} pixels, scroll={}→{}, same-line steps={steps}",
        scene.name, initial["scroll_y"], following["scroll_y"],
    );
    manual_follow::verify(view, scene, &output, cx).await?;
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn verify_emphasis(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    let sample = |ms, cx: &mut AsyncApp| {
        view.update(cx, |view, _, cx| {
            view.sample_lyrics_emphasis_for_capture(ms, cx)
        })
    };
    sample(Some(280), cx)?;
    let before = view.update(cx, |view, _, cx| {
        [1, 2].map(|line| view.lyrics_emphasis_for_capture(line, cx))
    })?;
    view.update(cx, |view, _, cx| {
        view.seek_lyrics_playback_for_capture(2.8, cx)
    })?;
    wait(view, cx, |value| {
        value["line"] == 2 && value["playing"] == false
    })
    .await?;
    let mut frames = Vec::new();
    for elapsed in [0, 70, 140, 210, 280] {
        sample(Some(elapsed), cx)?;
        frame(raw, scene, output, &format!("emphasis-{elapsed}"), cx)?;
        frames.push(view.update(cx, |view, _, cx| {
            [1, 2].map(|line| view.lyrics_emphasis_for_capture(line, cx))
        })?);
    }
    sample(None, cx)?;
    let stable_heights = frames.iter().all(|sample| {
        [0, 1].into_iter().all(|index| {
            before[index]
                .zip(sample[index])
                .is_some_and(|(before, after)| (before.1 - after.1).abs() < 0.1)
        })
    });
    let progress: Vec<_> = frames
        .iter()
        .filter_map(|sample| Some([sample[0]?.0, sample[1]?.0]))
        .collect();
    let motion_ok = if scene.reduced {
        progress == [[0., 1.]; 5]
    } else {
        progress.first() == Some(&[1., 0.])
            && progress.last() == Some(&[0., 1.])
            && progress
                .windows(2)
                .all(|pair| pair[0][0] > pair[1][0] && pair[0][1] < pair[1][1])
    };
    if !motion_ok || !stable_heights {
        return Err(io::Error::other(format!(
            "Выделение текста сдвинуло строки или потеряло промежуточные состояния: {frames:?}."
        ))
        .into());
    }
    record(
        output,
        "emphasis",
        &serde_json::json!({
            "reduced_motion": scene.reduced,
            "phases_ms": [0., 70., 140., 210., 280.],
            "weights_previous_next": progress,
            "line_geometry": frames,
        }),
    )
}

async fn paint_artwork(
    view: WindowHandle<MainView>,
    artwork: Option<&str>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    if let Some(artwork) = artwork {
        // A real transport command refreshes artwork from the local queue entry, which has no URL.
        view.update(cx, |view, _, cx| {
            view.set_listening_artwork_for_capture(artwork, cx)
        })?;
        await_artwork(view, cx).await?;
    }
    Ok(())
}

fn accent_ink(path: &Path, center_y: f32, scene: Scene, accent: [u8; 3]) -> gpui::Result<usize> {
    let pixels = image::open(path)?.to_rgb8();
    let scale = scene.dpi as f32 / 96.;
    let center_y = center_y * scale;
    let radius = if scene.height >= 500. { 32. } else { 23. } * scale;
    let top = ((center_y - radius).max(0.) as u32).min(pixels.height());
    let bottom = ((center_y + radius).ceil() as u32).min(pixels.height());
    let ink = image::imageops::crop_imm(&pixels, 0, top, pixels.width(), bottom - top)
        .pixels()
        .filter(|(_, _, pixel)| {
            pixel
                .0
                .iter()
                .zip(accent)
                .all(|(found, expected)| found.abs_diff(expected) <= 5)
        })
        .count();
    Ok(ink)
}

async fn follow_karaoke(
    view: WindowHandle<MainView>,
    scene: Scene,
    initial: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<(Value, usize)> {
    let raw = AnyWindowHandle::from(view);
    let timer = cx.background_executor().clone();
    let deadline = Instant::now() + Duration::from_secs(9);
    let initial_y = scroll_y(initial)?;
    let mut previous_line = initial["line"].as_u64();
    let mut previous_y = initial_y;
    let mut steps = 0;
    let mut max_steps = 0;
    loop {
        raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        let state = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
        let line = state["line"].as_u64();
        let y = scroll_y(&state)?;
        let natural = state["playing"] == true && line.is_some_and(|line| line >= 2);
        if line != previous_line {
            steps = 0;
        } else if natural && (y - previous_y).abs() > 1. {
            steps += 1;
            max_steps = max_steps.max(steps);
        }
        if scene.reduced && max_steps > 1 {
            return Err(
                io::Error::other("Уменьшение движения сохранило плавную прокрутку.").into(),
            );
        }
        if natural
            && line.is_some_and(|line| line >= 7)
            && initial_y - y > 35.
            && (scene.reduced || max_steps > 1)
        {
            return Ok((state, max_steps));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "{}: автопрокрутка караоке не подтверждена: смещение {initial_y}→{y}, шагов на строку {max_steps}, состояние {state}.",
                scene.name
            ))
            .into());
        }
        previous_line = line;
        previous_y = y;
        timer.timer(Duration::from_millis(17)).await;
    }
}

fn scroll_y(state: &Value) -> gpui::Result<f64> {
    state["scroll_y"]
        .as_f64()
        .ok_or_else(|| io::Error::other("Положение прокрутки караоке недоступно.").into())
}
