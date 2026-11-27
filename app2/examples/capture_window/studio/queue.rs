use std::{fs, io, path::Path};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, WindowHandle, point, px,
};
use mediadeka::MainView;
use serde_json::json;

use super::{
    COVER, Scene, TITLE, by_element, fixture::WARM_COVER, focus, focused, frame, key, luminance,
    named, node, node_label, press, queue_entry, require, settle_panel, snapshot, sound,
    transition, wait,
};

pub(super) async fn verify_drag(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let playlist = transition::playlist_id(raw, cx).await?;
    window.update(cx, |view, _, cx| {
        view.play_fluid_glass_transition_fixture_for_capture(playlist, COVER, WARM_COVER, cx)
    })??;
    let initial_sound = sound(window, TITLE, cx).await?;
    press(raw, &named("player-queue"), cx)?;
    settle_panel(raw, scene, cx).await?;
    let phase = |ms, name: &str, cx: &mut AsyncApp| {
        window.update(cx, |view, _, cx| {
            view.sample_queue_motion_for_capture(ms, cx)
        })?;
        frame(raw, scene, output, name, cx)
    };
    phase(0, "queue-drag-before", cx)?;
    let before = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    let row = |index, cx: &mut AsyncApp| -> gpui::Result<_> {
        Ok(window
            .update(cx, |view, _, _| view.queue_item_bounds_for_capture(index))?
            .ok_or_else(|| io::Error::other("Строка очереди вне видимой области."))?)
    };
    let source: gpui::Bounds<gpui::Pixels> = row(1, cx)?;
    let target: gpui::Bounds<gpui::Pixels> = row(2, cx)?;
    let start = point(source.left() + px(28.), source.bottom() - px(19.));
    let destination = point(target.left() + px(36.), target.center().y + px(2.));
    let move_pointer = |position, window: &mut gpui::Window, cx: &mut gpui::App| {
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    };
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        move_pointer(point(start.x, start.y + px(8.)), window, cx);
    })?;
    phase(0, "queue-drag-lifted", cx)?;
    raw.update(cx, |_, window, cx| move_pointer(destination, window, cx))?;
    for (ms, name) in [
        (0, "queue-drag-0"),
        (55, "queue-drag-25"),
        (110, "queue-drag-50"),
        (165, "queue-drag-75"),
        (220, "queue-drag-100"),
    ] {
        phase(ms, name, cx)?;
    }
    let pending = window.update(cx, |view, _, cx| {
        (
            view.queue_order_for_capture(cx),
            view.queue_drag_for_capture(),
        )
    })?;
    require(
        pending == (before.clone(), Some((before[1].0, Some(2)))),
        "Перетаскивание показывает позицию вставки и сохраняет очередь до отпускания.",
    )?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: destination,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    phase(440, "queue-drag-settled", cx)?;
    let after = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    let mut expected = before.clone();
    expected.swap(1, 2);
    require(
        after == expected,
        "GPUI drop должен переставить треки в настоящей очереди плеера.",
    )?;
    let moved_id = format!("NamedInteger(\"player-queue-entry\", {})", before[1].0);
    focus(raw, &moved_id, cx)?;
    key(raw, "alt-up", cx)?;
    let keyboard = phase(660, "queue-drag-return-keyboard", cx)?;
    let restored = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    require(
        restored == before && focused(&keyboard) == Some(moved_id.as_str()),
        "Alt+↑ должен вернуть трек и сохранить фокус на той же записи.",
    )?;
    window.update(cx, |view, _, cx| {
        view.append_playing_track_to_queue_for_capture(cx)
    })?;
    phase(660, "queue-insert-0", cx)?;
    let inserted = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    require(
        inserted.len() == restored.len() + 1
            && inserted[0] == restored[0]
            && inserted[1].0 != restored[0].0
            && inserted[1].1 == restored[0].1
            && inserted[2..] == restored[1..],
        "Команда «Далее» должна вставить отдельную запись после текущего трека.",
    )?;
    let mut reveal = Vec::new();
    for elapsed in [0, 55, 110, 165, 220] {
        if elapsed > 0 {
            phase(660 + elapsed, &format!("queue-insert-{elapsed}"), cx)?;
        }
        reveal.push(window.update(cx, |view, _, _| {
            view.queue_appearance_for_capture(inserted[1].0)
        })?);
    }
    require(
        if scene.reduced {
            reveal == [1.; 5]
        } else {
            reveal[0] == 0. && reveal[4] == 1. && reveal.windows(2).all(|pair| pair[0] < pair[1])
        },
        &format!(
            "Добавленная запись должна проявиться, а при reduced motion сразу достичь конечного состояния: {reveal:?}."
        ),
    )?;
    let removal = verify_removal(window, scene, output, &inserted, cx).await?;
    let current_sound = removal["sound_after"].clone();
    let clear = verify_clear(window, scene, output, cx).await?;
    fs::write(
        output.join("queue-drag.json"),
        serde_json::to_vec_pretty(&json!({
            "before": before,
            "after": after,
            "restored": restored,
            "inserted": inserted,
            "insertion_progress": reveal,
            "removal": removal,
            "clear_upcoming": clear,
            "reduced_motion": scene.reduced,
            "focus": moved_id,
            "sound_before": initial_sound,
            "sound_after": current_sound,
        }))?,
    )?;
    Ok(())
}

async fn verify_removal(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    inserted: &[(usize, String)],
    cx: &mut AsyncApp,
) -> gpui::Result<serde_json::Value> {
    let raw = AnyWindowHandle::from(window);
    let position_before = window
        .update(cx, |view, _, cx| view.sound_state_for_capture(cx))?
        .and_then(|value| value["position"].as_f64())
        .ok_or_else(|| io::Error::other("Перед удалением нет позиции текущего трека."))?;
    let id = |index: usize| {
        format!(
            "NamedInteger(\"player-queue-entry\", {})",
            inserted[index].0
        )
    };
    focus(raw, &id(0), cx)?;
    key(raw, "delete", cx)?;
    require(
        window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))? == inserted,
        "Delete должен сохранять текущую запись.",
    )?;
    let removed = id(1);
    let successor = id(2);
    focus(raw, &removed, cx)?;
    let read = |id, cx: &mut AsyncApp| {
        window.update(cx, |view, window, _| {
            view.queue_removal_for_capture(id, window)
        })
    };
    let before = read(inserted[2].0, cx)?.1;
    key(raw, "delete", cx)?;
    let mut faded = Vec::new();
    let mut shift = Vec::new();
    for elapsed in [0, 55, 110, 165, 220] {
        window.update(cx, |view, _, cx| {
            view.sample_queue_motion_for_capture(880 + elapsed, cx)
        })?;
        let tree = frame(raw, scene, output, &format!("queue-remove-{elapsed}"), cx)?;
        require(
            by_element(&tree, &removed).is_none()
                && focused(&tree) == Some(successor.as_str())
                && node_label(&tree, "player-track") == Some(TITLE),
            "Удалённый трек должен исчезнуть из дерева доступности, а фокус перейти следующей строке.",
        )?;
        faded.push(read(inserted[1].0, cx)?);
        shift.push(read(inserted[2].0, cx)?.1);
    }
    let mut expected = inserted.to_vec();
    expected.remove(1);
    let remaining = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    require(
        remaining == expected,
        "Удаление должно изменить одну предстоящую запись по её ID.",
    )?;
    require(
        if scene.reduced {
            faded == [(0., None); 5]
        } else {
            faded[0].0 == 1.
                && faded[4] == (0., None)
                && faded[..4].iter().all(|(_, top)| top.is_some())
                && faded.windows(2).all(|pair| pair[0].0 > pair[1].0)
                && before
                    .zip(shift[0])
                    .is_some_and(|(before, start)| (before - start).abs() < 0.1)
                && shift
                    .windows(2)
                    .all(|pair| pair[0].zip(pair[1]).is_some_and(|(from, to)| from > to))
        },
        &format!("Кадры удаления не подтвердили затухание и схождение: {faded:?}, {shift:?}."),
    )?;
    let playing = sound(window, TITLE, cx).await?;
    require(
        playing["later"]["position"]
            .as_f64()
            .is_some_and(|position| position > position_before),
        "Удаление предстоящего трека должно сохранить ход текущей позиции.",
    )?;
    Ok(json!({
        "removed_id": inserted[1].0,
        "focus": successor,
        "fade_alpha_and_top_dip": faded,
        "neighbor_top_dip": shift,
        "position_before": position_before,
        "sound_after": playing,
    }))
}

async fn verify_clear(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<serde_json::Value> {
    let raw = AnyWindowHandle::from(window);
    press(raw, &named("player-play"), cx)?;
    let playback =
        |cx: &mut AsyncApp| window.update(cx, |view, _, cx| view.sound_state_for_capture(cx));
    let paused = wait(
        cx,
        "пауза перед очисткой очереди",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(playback(cx)?.filter(|sound| {
                node_label(&tree, "player-play") == Some("Воспроизвести")
                    && sound["position"].as_f64().is_some_and(|time| time > 0.2)
            }))
        },
    )
    .await?;
    let before = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    let ready = snapshot(raw, cx)?;
    require(
        before.len() > 1
            && node_label(&ready, "queue-clear-upcoming") == Some("Очистить предстоящие"),
        "Команда очистки должна быть доступна, пока очередь содержит предстоящие треки.",
    )?;
    press(raw, &named("queue-clear-upcoming"), cx)?;
    let cleared = frame(raw, scene, output, "queue-upcoming-cleared", cx)?;
    let after = window.update(cx, |view, _, cx| view.queue_order_for_capture(cx))?;
    let sound =
        playback(cx)?.ok_or_else(|| io::Error::other("Очистка прервала воспроизведение."))?;
    let same_position = sound["position"]
        .as_f64()
        .zip(paused["position"].as_f64())
        .is_some_and(|(after, before)| (after - before).abs() <= 0.05);
    require(
        after == before[..1]
            && before[1..].iter().all(|(id, _)| {
                by_element(
                    &cleared,
                    &format!("NamedInteger(\"player-queue-entry\", {id})"),
                )
                .is_none()
            })
            && node(&cleared, "queue-clear-upcoming").is_none()
            && node_label(&cleared, "player-track") == Some(TITLE)
            && node_label(&cleared, "player-play") == Some("Воспроизвести")
            && sound["source"] == paused["source"]
            && same_position,
        "Очистка должна убрать предстоящие строки и сохранить трек, источник, паузу и позицию.",
    )?;
    Ok(
        json!({"before": before, "after": after, "position_before": paused["position"], "position_after": sound["position"]}),
    )
}

pub(super) fn background_top(
    window: WindowHandle<MainView>,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<u32> {
    let bottom = window
        .update(cx, |view, _, _| view.queue_last_item_bottom_for_capture())?
        .ok_or_else(|| io::Error::other("Не измерена последняя строка очереди."))?;
    // Keep the probe clear of the final track row and its antialiased edge.
    Ok((f32::from(bottom + px(6.)) * (scene.dpi as f32 / 96.)).ceil() as u32)
}

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let rest = frame(raw, scene, output, "queue-row-rest", cx)?;
    require(
        node_label(&rest, "queue-now-playing") == Some("Сейчас играет")
            && node_label(&rest, "queue-up-next") == Some("Далее"),
        "У очереди должны быть заголовки текущего трека и продолжения.",
    )?;
    let bounds = window
        .update(cx, |view, _, _| view.queue_item_bounds_for_capture(1))?
        .ok_or_else(|| io::Error::other("Строка следующего трека вне видимой области."))?;
    let hover = point(bounds.right() - px(10.), bounds.bottom() - px(16.));
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                position: hover,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    frame(raw, scene, output, "queue-row-hover", cx)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                position: point(px(1.), px(1.)),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    let id = queue_entry(&rest, 1)
        .and_then(|row| row["element_id"].as_str())
        .ok_or_else(|| io::Error::other("В дереве GPUI нет строки следующего трека."))?;
    focus(raw, id, cx)?;
    let keyboard = frame(raw, scene, output, "queue-row-focus", cx)?;
    require(
        focused(&keyboard) == Some(id),
        "Клавиатурный фокус очереди должен оставаться на следующем треке.",
    )?;
    let scale = scene.dpi as f32 / 96.;
    let pixel = |position: gpui::Pixels| (f32::from(position) * scale).floor() as u32;
    let read =
        |stem: &str| image::open(output.join(format!("{stem}.png"))).map(|image| image.to_rgb8());
    let rest = read("queue-row-rest")?;
    let hovered = read("queue-row-hover")?;
    let focused = read("queue-row-focus")?;
    let [x, y] = [pixel(hover.x), pixel(hover.y)];
    let baseline = luminance(rest.get_pixel(x, y).0);
    let highlight = luminance(hovered.get_pixel(x, y).0);
    let [left, top, bottom] = [
        pixel(bounds.left() + px(12.)),
        pixel(bounds.bottom() - px(3.)),
        pixel(bounds.bottom() + px(1.)).min(rest.height()),
    ];
    let peak = |image: &image::RgbImage| {
        (top..bottom)
            .map(|y| luminance(image.get_pixel(left, y).0))
            .fold(0., f64::max)
    };
    let rest_edge = peak(&rest);
    let focus_edge = peak(&focused);
    fs::write(
        output.join("queue-row-states.json"),
        serde_json::to_vec_pretty(&json!({
            "hover_sample": [x, y],
            "rest_luminance": baseline,
            "hover_luminance": highlight,
            "focus_sample": [left, top, bottom],
            "rest_edge_luminance": rest_edge,
            "focus_edge_luminance": focus_edge,
            "focused_element": id,
        }))?,
    )?;
    require(
        highlight > baseline && focus_edge > rest_edge + 0.1,
        "Строка очереди должна отдельно показывать наведение и клавиатурный фокус.",
    )?;
    if scene.width >= 1440. {
        verify_splitter(window, scene, output, cx)?;
    } else {
        require(
            node(&keyboard, "right-panel-resize").is_none(),
            "Компактная очередь не должна показывать разделитель пристыкованной панели.",
        )?;
    }
    Ok(())
}

fn verify_splitter(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let geometry = |cx: &mut AsyncApp| {
        window.update(cx, |view, window, _| view.dock_splitter_for_capture(window))
    };
    let original = geometry(cx)?;
    let id = named("right-panel-resize");
    focus(raw, &id, cx)?;
    key(raw, "left", cx)?;
    let a11y = frame(raw, scene, output, "queue-resized-keyboard", cx)?;
    let keyboard = geometry(cx)?;
    let handle = node(&a11y, "right-panel-resize");
    require(
        focused(&a11y) == Some(id.as_str())
            && handle.is_some_and(|node| node["aria"]["role"] == "Splitter")
            && (keyboard.0 - original.0 - 16.).abs() < 0.1
            && (f32::from(original.1 - keyboard.1) - 16.).abs() < 0.1,
        "Клавиатура должна изменить ширину и оптическую границу, сохранив фокус разделителя.",
    )?;

    let start = point(keyboard.1 + px(1.), px(scene.height * 0.45));
    let left = point(start.x - px(48.), start.y);
    let restore = point(start.x + px(16.), start.y);
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseMoveEvent {
                pressed_button: Some(MouseButton::Left),
                position: left,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    frame(raw, scene, output, "queue-resized-drag", cx)?;
    let dragged = geometry(cx)?;
    require(
        (dragged.0 - keyboard.0 - 48.).abs() < 0.1
            && (f32::from(keyboard.1 - dragged.1) - 48.).abs() < 0.1,
        "Граница панели должна следовать мыши в том же кадре.",
    )?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                pressed_button: Some(MouseButton::Left),
                position: restore,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: restore,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })?;
    let restored = geometry(cx)?;
    require(
        restored == original,
        "Панель должна вернуться к исходной ширине после обратного движения.",
    )?;
    let seams = [original.1, keyboard.1, dragged.1, restored.1].map(f32::from);
    fs::write(
        output.join("queue-resize.json"),
        serde_json::to_vec_pretty(&json!({
            "widths_dip": [original.0, keyboard.0, dragged.0, restored.0],
            "seams_dip": seams,
            "keyboard_focus": id,
        }))?,
    )?;
    Ok(())
}
