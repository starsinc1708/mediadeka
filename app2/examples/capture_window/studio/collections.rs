use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use gpui::{AnyWindowHandle, AsyncApp, Bounds, Pixels, WindowHandle};
use image::RgbaImage;
use mediadeka::MainView;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Value, json};

use super::{
    Scene, focus, focused, frame, key, named, node, press, require, shader_with_artwork_palette,
    snapshot, wait,
};
use crate::render_image;

const SIZE: &str = "collection-card-size";
const PIN: &str = "collection-card-pin-0";
const PIN_PENDING: &str = "Сохраняем изменение закрепления…";

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let mut report = Vec::new();
    for (collection, wide, compact) in [
        ("albums", "studio-library-albums", "library-tab-albums"),
        ("playlists", "home-playlists", "library-tab-playlists"),
    ] {
        let rail_visible = node(&snapshot(raw, cx)?, wide).is_some();
        if !rail_visible && node(&snapshot(raw, cx)?, compact).is_none() {
            press(raw, &named("studio-library"), cx)?;
        }
        press(raw, &named(if rail_visible { wide } else { compact }), cx)?;
        wait_for_grid(window, scene, cx).await?;
        let grid = frame(raw, scene, output, &format!("{collection}-grid"), cx)?;
        require(
            selected(&grid, "grid"),
            "Начальный вид коллекции должен быть сеткой.",
        )?;
        let initial = cover_edge(window, cx)?;
        require(
            initial.0 == initial.1,
            "Обложка карточки должна быть квадратной.",
        )?;
        let size = node(&grid, SIZE)
            .and_then(|control| control["aria"]["numeric_value"].as_f64())
            .ok_or_else(|| io::Error::other("Шкала размера карточек не показывает значение."))?;
        if scene.width == 1920. {
            let columns =
                window.update(cx, |view, _, _| view.liked_audit_for_capture())?["columns"].as_u64();
            press(raw, &named("player-queue"), cx)?;
            frame(raw, scene, output, &format!("{collection}-grid-docked"), cx)?;
            let docked =
                window.update(cx, |view, _, _| view.liked_audit_for_capture())?["columns"].as_u64();
            require(
                docked.is_some() && docked < columns,
                "Правая панель должна уменьшить число колонок сетки.",
            )?;
            key(raw, "escape", cx)?;
            snapshot(raw, cx)?;
        }

        focus(raw, &named(SIZE), cx)?;
        let baseline = cover_edge(window, cx)?;
        let sample = |ms, cx: &mut AsyncApp| {
            window.update(cx, |view, _, cx| {
                view.sample_collection_motion_for_capture(ms, cx)
            })
        };
        sample(Some(0), cx)?;
        key(raw, "end", cx)?;
        let mut zoom_frames = Vec::new();
        for ms in [0, 70, 140, 210, 280] {
            sample(Some(ms), cx)?;
            frame(raw, scene, output, &format!("{collection}-zoom-{ms}"), cx)?;
            zoom_frames.push(cover_edge(window, cx)?.0);
        }
        sample(None, cx)?;
        wait_for_grid(window, scene, cx).await?;
        let large = frame(raw, scene, output, &format!("{collection}-grid-large"), cx)?;
        let enlarged = cover_edge(window, cx)?;
        require(
            enlarged.0 == enlarged.1
                && enlarged.0 > initial.0
                && node(&large, SIZE)
                    .is_some_and(|control| control["aria"]["numeric_value"] == 256.),
            "Увеличение должно менять размеры квадратных обложек.",
        )?;
        require(
            if scene.reduced {
                zoom_frames
                    .iter()
                    .all(|&edge| (edge - enlarged.0).abs() < 0.5)
            } else {
                (zoom_frames[0] - baseline.0).abs() < 0.5
                    && (zoom_frames[4] - enlarged.0).abs() < 0.5
                    && zoom_frames.windows(2).all(|pair| pair[0] < pair[1])
            },
            &format!("Масштаб видимой обложки неоправданно скачет между кадрами: {zoom_frames:?}."),
        )?;
        if collection == "playlists" && scene.width == 1920. {
            let profile = PathBuf::from(
                env::var_os("MEDIADEKA_PROFILE_DIR")
                    .ok_or_else(|| io::Error::other("Нет профиля проверки."))?,
            );
            let mut database = Connection::open(profile.join("mediadeka.sqlite3"))?;
            database.execute_batch(
                "CREATE TRIGGER capture_reject_pin BEFORE INSERT ON pinned_object
                BEGIN SELECT RAISE(ABORT, 'capture pin rejection'); END;",
            )?;
            let before = toggle_pin_during_lock(&mut database, raw, "True", cx)?;
            wait_for_pin(raw, "False", cx).await?;
            require(
                pin_count(&database)? == before,
                "После отказа SQLite закрепления должны откатиться.",
            )?;
            let failed = frame(raw, scene, output, "playlist-pin-rollback", cx)?;
            require(
                focused(&failed) == Some(named(PIN).as_str()),
                "Откат должен сохранить фокус команды.",
            )?;
            require(
                node(&failed, "player-message").is_some_and(|message| {
                    message["aria"]["label"] == "Не удалось изменить закреплённое."
                }),
                "Отказ SQLite должен быть виден пользователю.",
            )?;
            database.execute_batch("DROP TRIGGER capture_reject_pin;")?;
            println!(
                "{} playlist pin rolled back after a real SQLite rejection",
                scene.name
            );
            for expected in ["True", "False"] {
                let withdrawing = expected == "False";
                if withdrawing {
                    focus(raw, &named(PIN), cx)?;
                    window.update(cx, |view, _, cx| {
                        view.sample_pin_departure_for_capture(Some(0), cx);
                    })?;
                }
                let before = toggle_pin_during_lock(&mut database, raw, expected, cx)?;
                wait_for_pin(raw, expected, cx).await?;
                let count = pin_count(&database)?;
                require(
                    count == before + if withdrawing { -1 } else { 1 },
                    "SQLite должна сохранить один переход закрепления.",
                )?;
                if withdrawing {
                    let mut alpha = Vec::new();
                    for ms in [0, 55, 110, 165, 220] {
                        window.update(cx, |view, _, cx| {
                            view.sample_pin_departure_for_capture(Some(ms), cx);
                        })?;
                        let tree =
                            frame(raw, scene, output, &format!("playlist-unpin-{ms:03}ms"), cx)?;
                        require(
                            focused(&tree) == Some(named(PIN).as_str()),
                            "Открепление должно сохранить фокус команды.",
                        )?;
                        alpha.push(
                            window
                                .update(cx, |view, _, _| view.pin_departure_alpha_for_capture())?,
                        );
                    }
                    window.update(cx, |view, _, cx| {
                        view.sample_pin_departure_for_capture(None, cx)
                    })?;
                    require(
                        if scene.reduced {
                            alpha.iter().all(|&value| value == 0.)
                        } else {
                            alpha[0] == 1.
                                && alpha[4] == 0.
                                && alpha.windows(2).all(|pair| pair[0] > pair[1])
                        },
                        &format!("Откреплённый плейлист должен постепенно исчезнуть: {alpha:?}."),
                    )?;
                    println!("{} playlist departure alpha: {alpha:?}", scene.name);
                }
            }
            println!(
                "{} playlist pin/unpin was visible during the SQLite lock and ignored duplicate keys",
                scene.name
            );
        }

        for mode in ["list", "compact"] {
            let id = format!("collection-view-{mode}");
            let before = cover_edge(window, cx)?;
            sample(Some(0), cx)?;
            press(raw, &named(&id), cx)?;
            let mut frames = Vec::new();
            for ms in [0, 70, 140, 210, 280] {
                sample(Some(ms), cx)?;
                frame(raw, scene, output, &format!("{collection}-{mode}-{ms}"), cx)?;
                frames.push(cover_edge(window, cx)?.0);
            }
            sample(None, cx)?;
            require(
                if scene.reduced {
                    frames.iter().all(|&edge| (edge - frames[4]).abs() < 0.5)
                } else {
                    (before.0 - frames[0]).abs() < 0.5
                        && frames.windows(2).all(|pair| pair[0] > pair[1])
                },
                &format!("Переход в {mode} должен вести ту же обложку по кадрам: {frames:?}."),
            )?;
            let view = frame(raw, scene, output, &format!("{collection}-{mode}"), cx)?;
            require(
                selected(&view, mode) && node(&view, SIZE).is_none(),
                "Список и компактный вид должны иметь выбранный переключатель без шкалы карточек.",
            )?;
            require(
                window.update(cx, |view, _, _| view.saved_collection_view_for_capture())?
                    == Some((mode, 256)),
                "Вид и масштаб нужно сохранить в профиле.",
            )?;
        }

        if collection == "playlists" {
            press(raw, &named("playlist-import"), cx)?;
            let menu = frame(raw, scene, output, "playlists-import-menu", cx)?;
            require(
                [
                    "playlist-import-files",
                    "playlist-import-folder",
                    "playlist-import-m3u",
                ]
                .into_iter()
                .all(|item| {
                    node(&menu, item).is_some_and(|element| element["aria"]["role"] == "MenuItem")
                }),
                "Меню импорта должно содержать аудиофайлы, папку и M3U.",
            )?;
            key(raw, "escape", cx)?;
            let closed = snapshot(raw, cx)?;
            require(
                node(&closed, "playlist-menu").is_none()
                    && focused(&closed) == Some(named("playlist-import").as_str()),
                "Escape должен вернуть фокус кнопке импорта.",
            )?;
        }

        press(raw, &named("collection-view-grid"), cx)?;
        focus(raw, &named(SIZE), cx)?;
        key(raw, "home", cx)?;
        snapshot(raw, cx)?;
        for _ in 0..((size - 112.) / 8.) as usize {
            key(raw, "right", cx)?;
            snapshot(raw, cx)?;
        }
        require(
            window.update(cx, |view, _, _| view.saved_collection_view_for_capture())?
                == Some(("grid", size as u16)),
            "После проверки профиль должен вернуть исходные параметры сетки.",
        )?;
        report.push(
            json!({ "collection": collection, "card_before_dip": initial.0,
            "card_after_dip": enlarged.0, "zoom_frames_dip": zoom_frames,
            "persisted_modes": ["grid", "list", "compact"] }),
        );
    }
    fs::write(
        output.join("collection-views.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

fn pin_count(database: &Connection) -> gpui::Result<i64> {
    Ok(database.query_row("SELECT COUNT(*) FROM pinned_object", [], |row| row.get(0))?)
}

fn toggle_pin_during_lock(
    database: &mut Connection,
    raw: AnyWindowHandle,
    expected: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<i64> {
    let gate = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count = pin_count(&gate)?;
    for _ in 0..2 {
        press(raw, &named(PIN), cx)?;
        let tree = snapshot(raw, cx)?;
        require(
            node(&tree, PIN).is_some_and(|control| {
                control["aria"]["toggled"] == expected
                    && control["aria"]["description"] == PIN_PENDING
            }),
            "Переключатель должен показать новое состояние до снятия блокировки SQLite.",
        )?;
        require(
            pin_count(&gate)? == count,
            "Нажатие меняет интерфейс до записи SQLite.",
        )?;
    }
    gate.commit()?;
    Ok(count)
}

async fn wait_for_pin(raw: AnyWindowHandle, expected: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    wait(
        cx,
        "завершение записи закрепления",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(node(&tree, PIN)
                .is_some_and(|control| {
                    control["aria"]["toggled"] == expected
                        && control["aria"]["description"] != PIN_PENDING
                })
                .then_some(()))
        },
    )
    .await
}

fn selected(tree: &Value, mode: &str) -> bool {
    node(tree, &format!("collection-view-{mode}"))
        .is_some_and(|control| control["aria"]["toggled"] == "True")
}

async fn wait_for_grid(
    window: WindowHandle<MainView>,
    scene: Scene,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    wait(
        cx,
        "сетка коллекции с отрисованной обложкой",
        |cx| {
            let current = snapshot(raw, cx)?;
            if !selected(&current, "grid") || node(&current, SIZE).is_none() {
                return Ok(None);
            }
            let cover = window.update(cx, |view, window, _| {
                view.first_album_tile_cover_for_capture(window)
            })?;
            let Some(bounds) = cover else {
                return Ok(None);
            };
            if shader_with_artwork_palette()
                && !has_artwork(
                    &render_image(raw, scene, cx)?,
                    bounds,
                    scene.dpi as f32 / 96.,
                )
            {
                return Ok(None);
            }
            Ok(Some(()))
        },
    )
    .await
}

fn has_artwork(image: &RgbaImage, bounds: Bounds<Pixels>, scale: f32) -> bool {
    let physical = |position| (f32::from(position) * scale).round() as u32;
    let x = physical(bounds.left() + bounds.size.width * 0.30);
    let y = physical(bounds.top() + bounds.size.height * 0.10);
    let right = physical(bounds.right() - bounds.size.width * 0.30).min(image.width());
    let bottom = physical(bounds.top() + bounds.size.height * 0.28).min(image.height());
    let Some(first) = image.get_pixel_checked(x, y) else {
        return false;
    };
    (y..bottom).step_by(3).any(|row| {
        (x..right).step_by(3).any(|column| {
            image.get_pixel(column, row).0[..3]
                .iter()
                .zip(first.0)
                .any(|(&value, first)| value.abs_diff(first) > 9)
        })
    })
}

fn cover_edge(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<(f32, f32)> {
    window
        .update(cx, |view, window, _| {
            view.first_album_tile_cover_for_capture(window)
        })?
        .map(|bounds| (f32::from(bounds.size.width), f32::from(bounds.size.height)))
        .ok_or_else(|| io::Error::other("Не найдена обложка карточки.").into())
}
