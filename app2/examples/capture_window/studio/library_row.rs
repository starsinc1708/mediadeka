use std::{fs, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, Bounds, Pixels, Point, WindowHandle, point, px};
use image::RgbaImage;
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    Scene,
    control_feedback::{events, mouse_down, mouse_move, mouse_up},
    frame, key, named, node, node_label, press, require, snapshot, sound, wait,
};

const ROW: &str = "library-row-0";
const MENU: &str = "library-track-menu-0";
const OPEN_TRACK: &str = "library-open-track-0";
const COLLECTION_REFLOW: Duration = Duration::from_millis(280);

pub(super) async fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(view);
    let outside = point(px(3.), px(3.));
    events(raw, [mouse_move(outside, false)], cx)?;
    let title = wait(
        cx,
        "первая доступная строка медиатеки",
        |cx| {
            let tree = snapshot(raw, cx)?;
            let title = node_label(&tree, ROW)
                .and_then(|name| name.strip_prefix("Слушать · "))
                .and_then(|name| name.split(" · ").next());
            Ok(title.map(str::to_owned))
        },
    )
    .await?;
    if !scene.reduced {
        cx.background_executor().timer(COLLECTION_REFLOW).await;
    }
    let idle = render_image(raw, scene, cx)?;
    let layout = view
        .update(cx, |view, window, _| {
            view.library_track_geometry_for_capture(0, window)
        })?
        .ok_or_else(|| std::io::Error::other("У строки медиатеки отсутствует геометрия."))?;
    let (row, cover) = layout;
    let title_point = point(cover.right() + px(17.), row.center().y - px(5.));
    require(
        row.contains(&title_point),
        "Точка клика по названию должна находиться внутри строки.",
    )?;

    idle.save(output.join("library-row-idle.png"))?;
    events(raw, [mouse_move(title_point, false)], cx)?;
    let hover = wait(
        cx,
        "появился светлый глиф Play поверх обложки",
        |cx| {
            let image = render_image(raw, scene, cx)?;
            Ok((added_play_pixels(&idle, &image, cover, scene) >= 8).then_some(image))
        },
    )
    .await?;
    hover.save(output.join("library-row-hover.png"))?;
    let hover_layout = view.update(cx, |view, window, _| {
        view.library_track_geometry_for_capture(0, window)
    })?;
    let columns = columns_overlap(&idle, &hover, row, cover, scene);
    let same_layout = hover_layout == Some(layout);
    require(
        same_layout && columns >= 0.88,
        &format!(
            "Наведение должно сохранить геометрию строки ({same_layout}) и положение текста ({columns:.3})."
        ),
    )?;
    events(raw, [mouse_move(outside, false)], cx)?;
    let away = wait(
        cx,
        "глиф Play скрылся после ухода мыши",
        |cx| {
            let image = render_image(raw, scene, cx)?;
            Ok((added_play_pixels(&idle, &image, cover, scene) <= 2).then_some(image))
        },
    )
    .await?;
    away.save(output.join("library-row-pointer-away.png"))?;

    click(raw, title_point, cx)?;
    let title_sound = played_in_library(view, &title, cx).await?;
    frame(raw, scene, output, "library-row-title-plays", cx)?;
    pause(view, raw, cx).await?;
    click(raw, cover.center(), cx)?;
    let cover_sound = played_in_library(view, &title, cx).await?;
    frame(raw, scene, output, "library-row-cover-plays", cx)?;
    pause(view, raw, cx).await?;
    press(raw, &named(ROW), cx)?;
    let keyboard_sound = played_in_library(view, &title, cx).await?;

    press(raw, &named(MENU), cx)?;
    let actions = frame(raw, scene, output, "library-row-actions", cx)?;
    require(
        node(&actions, OPEN_TRACK).is_some_and(|node| {
            node["aria"]["role"] == "MenuItem" && node["aria"]["label"] == "Открыть страницу трека"
        }),
        "Контекстная команда навигации должна быть отдельным пунктом.",
    )?;
    key(raw, "enter", cx)?;
    let selected = wait(
        cx,
        "открылась отдельная страница записи",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(node_label(&tree, "music-object-title").map(str::to_owned))
        },
    )
    .await?;
    require(selected == title, "Открыта выбранная композиция.")?;
    let navigation_sound = sound(view, &title, cx).await?;
    let report = json!({
        "track": title,
        "row_and_cover_footprint_unchanged": same_layout,
        "column_text_overlap": columns,
        "hover_play_pixels": added_play_pixels(&idle, &hover, cover, scene),
        "pointer_away_play_pixels": added_play_pixels(&idle, &away, cover, scene),
        "title_click_dip": [f32::from(title_point.x), f32::from(title_point.y)],
        "cover_click_dip": [f32::from(cover.center().x), f32::from(cover.center().y)],
        "title_playback": title_sound, "cover_playback": cover_sound,
        "keyboard_playback": keyboard_sound, "playback_after_navigation": navigation_sound,
    });
    fs::write(
        output.join("library-row-playback.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}

async fn played_in_library(
    view: WindowHandle<MainView>,
    title: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let playback = sound(view, title, cx).await?;
    let route = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
    require(
        route["page"] == "tracks",
        "Запуск строки сохраняет раздел «Треки».",
    )?;
    Ok(playback)
}

pub(super) async fn pause(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    press(raw, &named("player-play"), cx)?;
    wait(
        cx,
        "пауза между независимыми запусками",
        |cx| {
            let state = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
            let tree = snapshot(raw, cx)?;
            Ok((state["playing"] == false
                && node_label(&tree, "player-play") == Some("Воспроизвести"))
            .then_some(()))
        },
    )
    .await
}

pub(super) fn click(
    raw: AnyWindowHandle,
    target: Point<Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    events(
        raw,
        [
            mouse_move(target, false),
            mouse_down(target),
            mouse_up(target),
        ],
        cx,
    )
}

pub(super) fn added_play_pixels(
    idle: &RgbaImage,
    other: &RgbaImage,
    cover: Bounds<Pixels>,
    scene: Scene,
) -> usize {
    let inner = cover.size.width.min(cover.size.height) * 0.29;
    let centre = cover.center();
    let (left, top, right, bottom) = pixels(
        [
            centre.x - inner,
            centre.y - inner,
            centre.x + inner,
            centre.y + inner,
        ],
        scene,
        other,
    );
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let before = idle.get_pixel(x, y);
            let after = other.get_pixel(x, y);
            let darkest = after.0[..3].iter().copied().min().unwrap_or_default();
            darkest >= 184 && luma(after.0) - luma(before.0) > 42.
        })
        .count()
}

fn columns_overlap(
    idle: &RgbaImage,
    hover: &RgbaImage,
    row: Bounds<Pixels>,
    cover: Bounds<Pixels>,
    scene: Scene,
) -> f64 {
    let (left, top, right, bottom) = pixels(
        [
            cover.right() + px(6.),
            row.top() + px(3.),
            row.right() - px(4.),
            row.bottom() - px(3.),
        ],
        scene,
        idle,
    );
    let mut shared = 0;
    let mut union = 0;
    for (x, y) in (top..bottom).flat_map(|y| (left..right).map(move |x| (x, y))) {
        let before = luma(idle.get_pixel(x, y).0) >= 157.;
        let after = luma(hover.get_pixel(x, y).0) >= 157.;
        shared += usize::from(before && after);
        union += usize::from(before || after);
    }
    if union == 0 {
        0.
    } else {
        shared as f64 / union as f64
    }
}

fn pixels(
    [left, top, right, bottom]: [Pixels; 4],
    scene: Scene,
    image: &RgbaImage,
) -> (u32, u32, u32, u32) {
    let scale = scene.dpi as f32 / 96.;
    let coordinate = |value: Pixels, max| ((f32::from(value) * scale).round() as u32).min(max);
    (
        coordinate(left, image.width()),
        coordinate(top, image.height()),
        coordinate(right, image.width()),
        coordinate(bottom, image.height()),
    )
}

fn luma([red, green, blue, _]: [u8; 4]) -> f64 {
    0.2126 * f64::from(red) + 0.7152 * f64::from(green) + 0.0722 * f64::from(blue)
}
