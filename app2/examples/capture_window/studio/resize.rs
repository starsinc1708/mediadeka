use std::{env, fs, io, ops::Range, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    QUEUE_LIST, Scene, activate_accessibility, by_element, focus, focused, luminance, named,
    native_dpi_change, node, node_label, press, queue_candidate, require, snapshot, wait,
};

const ALBUMS_RAIL: &str = "studio-library-albums";
const ALBUMS_TAB: &str = "library-tab-albums";
const ALBUMS_COUNT: &str = "8 альбомов";
const WALL: Duration = Duration::from_secs(60);
const PHASE: Duration = Duration::from_millis(4600);
const EDGE_COMPARE_REFLOW_MS: u64 = 140;
const WIDE_RAIL: f32 = 184.;
const FULL_NAV_HEIGHT: f32 = 720.;
const PINNED_FULL_NAV_HEIGHT: f32 = 760.;
const WIDTHS: [f32; 6] = [900., 1040., 1140., 1180., 1280., 1360.];
const HEIGHTS: [f32; 7] = [400., 450., 500., 560., 580., 620., FULL_NAV_HEIGHT];
const WIDTH_SWEEP_HEIGHT: f32 = 720.;
const HEIGHT_SWEEP_WIDTH: f32 = 1360.;

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let initial = snapshot(raw, cx)?;
    require(
        node(&initial, QUEUE_LIST).is_none()
            && node_label(&initial, "player-play") == Some("Воспроизвести")
            && queue_candidate(&initial).is_some(),
        "Изменение размера должно начаться в медиатеке на паузе и с закрытой очередью.",
    )?;

    let database = profile_database()?;
    let tested = exercise(window, scene, output, &database, cx).await;
    let restored = restore(window, scene, &database, cx).await;
    tested.and(restored)
}

pub(super) async fn verify_pins(
    window: WindowHandle<MainView>,
    compact: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    // This hidden anchor is torn down after the check together with its temporary profile.
    let raw = window.into();
    let wide = Scene {
        width: HEIGHT_SWEEP_WIDTH,
        ..compact
    };
    resize(raw, wide, cx)?;
    activate_accessibility(raw, cx)?;
    window.update(cx, |view, window, cx| {
        view.start_fluid_glass_for_capture(window, cx);
        view.sample_fluid_glass_track_change_for_capture(WALL, PHASE, cx);
    })?;
    wait_for_tracks(raw, cx).await?;
    let (id, title): (i64, String) =
        profile_database()?.query_row("SELECT id, name FROM local_playlist", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    window.update(cx, |view, _, cx| view.seed_pins_for_capture(id, cx))?;
    let pin = "NamedInteger(\"home-pinned-item\", 0)";
    let label = format!("{title} · Плейлист");
    wait(
        cx,
        "локальный плейлист в закреплениях",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(
                (by_element(&tree, pin).and_then(|item| item["aria"]["label"].as_str())
                    == Some(label.as_str()))
                .then_some(()),
            )
        },
    )
    .await?;

    let mut minimum_frames = Vec::new();
    for scene in [wide, compact] {
        resize(raw, scene, cx)?;
        minimum_frames.push(inspect_minimum_pin(window, scene, pin, output, cx)?);
    }
    press(raw, pin, cx)?;
    let loaded = wait(
        cx,
        "фактическое открытие закреплённого плейлиста с клавиатуры",
        |cx| {
            let status = window.update(cx, |view, _, _| view.playlist_status_for_capture())?;
            match status {
                Some(Ok(count)) => Ok(Some(count)),
                Some(Err(error)) => Err(io::Error::other(error).into()),
                None => Ok(None),
            }
        },
    )
    .await?;
    require(
        loaded == 8,
        "Закреплённый плейлист должен открыть восемь локальных треков.",
    )?;

    let short = Scene {
        height: PINNED_FULL_NAV_HEIGHT - 1.,
        ..wide
    };
    let full = Scene {
        height: PINNED_FULL_NAV_HEIGHT,
        ..wide
    };
    resize(raw, short, cx)?;
    press(raw, &named("studio-library"), cx)?;
    wait_for_tracks(raw, cx).await?;
    focus(raw, &named(ALBUMS_TAB), cx)?;
    let short_tree = snapshot(raw, cx)?;
    resize(raw, full, cx)?;
    let full_tree = snapshot(raw, cx)?;
    let clean_handoff = node(&short_tree, ALBUMS_RAIL).is_none()
        && node(&short_tree, "library-tabs").is_some()
        && focused(&short_tree) == Some(&named(ALBUMS_TAB))
        && node(&full_tree, "library-tabs").is_none()
        && node_label(&full_tree, ALBUMS_RAIL) == Some("Альбомы")
        && focused(&full_tree) == Some(&named(ALBUMS_RAIL));
    let threshold_frame = "resize-1360x760-pinned-full-nav.png";
    render_image(raw, full, cx)?.save(output.join(threshold_frame))?;
    fs::write(
        output.join("responsive-resize-pins.json"),
        serde_json::to_vec_pretty(&json!({
            "playlist_title": title,
            "opened_track_count": loaded,
            "minimum_height_frames": minimum_frames,
            "full_navigation_with_pins": {
                "short_height_dip": short.height,
                "full_height_dip": full.height,
                "short_focus": focused(&short_tree),
                "full_focus": focused(&full_tree),
                "frame": threshold_frame,
            },
        }))?,
    )?;
    require(
        clean_handoff,
        "С закреплениями полное меню включается с 760 DIP и сохраняет фокус.",
    )?;
    println!(
        "Fluid Glass pins: visible and keyboard-reachable at 1360×400 and 800×400, full navigation at 760 DIP"
    );
    Ok(())
}

fn profile_database() -> gpui::Result<Connection> {
    let profile = env::var_os("MEDIADEKA_PROFILE_DIR")
        .ok_or_else(|| io::Error::other("Тест изменения размера требует собственного профиля."))?;
    Ok(Connection::open_with_flags(
        Path::new(&profile).join("mediadeka.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

async fn wait_for_tracks(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<()> {
    wait(
        cx,
        "строки локальной медиатеки",
        |cx| Ok(queue_candidate(&snapshot(raw, cx)?).is_some().then_some(())),
    )
    .await
}

async fn exercise(
    window: WindowHandle<MainView>,
    original: Scene,
    output: &Path,
    database: &Connection,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    press(raw, &named(ALBUMS_RAIL), cx)?;
    wait(
        cx,
        "альбомы перед изменением размера",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok((node_label(&tree, "library-counts") == Some(ALBUMS_COUNT)).then_some(()))
        },
    )
    .await?;
    await_saved_section(database, "Albums", cx).await?;
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_track_change_for_capture(WALL, PHASE, cx);
    })?;

    let initial = inspect(window, original, output, false, cx)?;
    let mut widths = Vec::with_capacity(WIDTHS.len());
    let mut heights = Vec::with_capacity(HEIGHTS.len());
    for width in WIDTHS {
        let scene = Scene {
            width,
            height: WIDTH_SWEEP_HEIGHT,
            ..original
        };
        resize(raw, scene, cx)?;
        widths.push(inspect(window, scene, output, true, cx)?);
    }
    for height in HEIGHTS {
        let scene = Scene {
            width: HEIGHT_SWEEP_WIDTH,
            height,
            ..original
        };
        resize(raw, scene, cx)?;
        heights.push(inspect(window, scene, output, true, cx)?);
    }
    let mut footer = Vec::new();
    for height in [HEIGHTS[0], HEIGHTS[5], FULL_NAV_HEIGHT] {
        let scene = Scene {
            width: HEIGHT_SWEEP_WIDTH,
            height,
            ..original
        };
        resize(raw, scene, cx)?;
        footer.extend(inspect_sidebar_footer(window, scene, output, cx)?);
    }
    let fractional = Scene {
        width: WIDTHS[1],
        height: WIDTH_SWEEP_HEIGHT,
        ..original
    };
    resize(raw, fractional, cx)?;
    let (different_rgba, max_channel_delta) = compare_edges(window, fractional, output, cx)?;
    resize(raw, original, cx)?;
    let returned = inspect(window, original, output, false, cx)?;

    let capture = json!({
        "forced_shader_phase_ms": PHASE.as_millis(),
        "same_hidden_window": true,
        "initial": initial.summary(),
        "width_sweep": widths.iter().map(Observation::summary).collect::<Vec<_>>(),
        "height_sweep": heights.iter().map(Observation::summary).collect::<Vec<_>>(),
        "sidebar_footer_focus": footer,
        "fractional_edge_ab": {
            "client_px": [fractional.width as u32, fractional.height as u32],
            "collection_motion_ms": EDGE_COMPARE_REFLOW_MS,
            "different_rgba_pixels": different_rgba,
            "max_channel_delta": max_channel_delta,
            "strip_frame": frame_name(fractional),
            "full_frame": "resize-1040x720-full-edge.png",
        },
        "restored": returned.summary(),
    });
    fs::write(
        output.join("responsive-resize.json"),
        serde_json::to_vec_pretty(&capture)?,
    )?;
    verify_geometry(&widths, &heights)?;
    for sample in [&initial, &returned]
        .into_iter()
        .chain(&widths)
        .chain(&heights)
    {
        sample.verify()?;
    }
    require(
        footer.iter().all(|item| {
            item["focus_changes_above_player"]
                .as_u64()
                .is_some_and(|count| count > 0)
                && item["focus_changes_inside_player"] == 0
        }),
        &format!("Фокус нижних пунктов меню должен оставаться выше плеера: {footer:?}"),
    )?;
    require(
        // At the fractional width, a few blended pixels on the translucent
        // controls can round to adjacent 8-bit values. Keep the raw difference
        // count and require equivalence within one output level per channel.
        initial.geometry == returned.geometry && max_channel_delta <= 1,
        &format!(
            "Геометрия должна восстановиться, а промежуточный кадр с полосами оптики совпасть с полной отрисовкой с точностью до одного уровня канала. Разных пикселей: {different_rgba}, максимальная разница: {max_channel_delta}."
        ),
    )?;
    println!(
        "{} native resize: {} widths, {} heights, Albums focus retained; fractional optical edge changed {different_rgba} RGBA pixels by at most {max_channel_delta} levels",
        original.name,
        widths.len(),
        heights.len()
    );
    Ok(())
}

pub(super) fn resize(raw: AnyWindowHandle, scene: Scene, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    Ok(())
}

async fn restore(
    window: WindowHandle<MainView>,
    original: Scene,
    database: &Connection,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    resize(raw, original, cx)?;
    press(raw, &named("studio-library"), cx)?;
    // The other native scenes open the same profile after this view is dropped.
    await_saved_section(database, "Tracks", cx).await
}

async fn await_saved_section(
    database: &Connection,
    expected: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait(
        cx,
        &format!("сохранённый раздел медиатеки {expected}"),
        |_| {
            let payload: Option<String> = database
                .query_row(
                    "SELECT payload FROM workspace_route WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let route: Value = payload
                .map(|json| serde_json::from_str(&json))
                .transpose()?
                .unwrap_or(Value::Null);
            Ok((route["Library"].as_str() == Some(expected)).then_some(()))
        },
    )
    .await
}

fn inspect(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    save: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<Observation> {
    let raw = window.into();
    let tree = snapshot(raw, cx)?;
    let image = render_image(raw, scene, cx)?;
    if save {
        image.save(output.join(frame_name(scene)))?;
    }
    let geometry = Geometry::from(window.update(cx, |view, window, _| {
        view.fluid_glass_geometry_for_capture(window)
    })?);
    let rail_x = geometry.inset + geometry.rail;
    let player_y = scene.height - geometry.inset - geometry.player;
    let rail = ridge(rail_x, |x| {
        mean_luma((134..232).map(|y| image.get_pixel(x, y).0))
    });
    let player = ridge(player_y, |y| {
        // Skip Play/Pause. In a short window the top of its glyph and fill
        // already reach the comparison row six pixels below the optical edge.
        let span = (scene.width * 0.55) as u32..(scene.width * 0.67) as u32;
        mean_luma(span.map(|x| image.get_pixel(x, y).0))
    });
    Ok(Observation {
        scene,
        geometry,
        rail,
        player,
        focus: focused(&tree).map(str::to_owned),
        heading: node_label(&tree, "library-title").map(str::to_owned),
        albums_active: node(&tree, ALBUMS_RAIL).is_some_and(|control| {
            control["aria"]["label"] == "Альбомы" && control["aria"]["toggled"] == "True"
        }),
        albums_tab_selected: node(&tree, ALBUMS_TAB).is_some_and(|control| {
            control["aria"]["label"] == "Альбомы"
                && control["aria"]["role"] == "Tab"
                && control["aria"]["selected"] == true
        }),
        workspace_tabs: node(&tree, "library-tabs").is_some(),
        shared_controls: node_label(&tree, "home-navigation") == Some("Навигация")
            && node_label(&tree, "home-search") == Some("Поиск музыки")
            && node_label(&tree, "player-progress") == Some("Позиция воспроизведения")
            && node_label(&tree, "library-counts") == Some(ALBUMS_COUNT),
    })
}

fn compare_edges(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<(usize, u8)> {
    let raw = window.into();
    window.update(cx, |view, _, cx| {
        view.sample_collection_motion_for_capture(Some(EDGE_COMPARE_REFLOW_MS), cx)
    })?;
    let compared = (|| -> gpui::Result<_> {
        // Both passes must sample the same resized layout and cover positions.
        snapshot(raw, cx)?;
        let optimized = render_image(raw, scene, cx)?;
        window.update(cx, |view, _, cx| {
            view.set_fluid_glass_full_edge_for_capture(true, cx)
        })?;
        let full = render_image(raw, scene, cx)?;
        Ok((optimized, full))
    })();
    window.update(cx, |view, _, cx| {
        view.set_fluid_glass_full_edge_for_capture(false, cx);
        view.sample_collection_motion_for_capture(None, cx);
    })?;
    let (optimized, full) = compared?;
    optimized.save(output.join(frame_name(scene)))?;
    full.save(output.join("resize-1040x720-full-edge.png"))?;
    let mut changed = 0;
    let mut max_delta = 0;
    for (a, b) in optimized.pixels().zip(full.pixels()) {
        let delta =
            a.0.into_iter()
                .zip(b.0)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap_or_default();
        changed += usize::from(delta != 0);
        max_delta = max_delta.max(delta);
    }
    Ok((changed, max_delta))
}

fn frame_name(scene: Scene) -> String {
    format!("resize-{}x{}.png", scene.width, scene.height)
}

fn inspect_minimum_pin(
    window: WindowHandle<MainView>,
    scene: Scene,
    pin: &str,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = window.into();
    let base = render_image(raw, scene, cx)?;
    focus(raw, pin, cx)?;
    let pinned = render_image(raw, scene, cx)?;
    let (pin_bounds, geometry) = window.update(cx, |view, window, _| {
        (
            view.pinned_bounds_for_capture(0),
            Geometry::from(view.fluid_glass_geometry_for_capture(window)),
        )
    })?;
    let bounds = pin_bounds
        .ok_or_else(|| io::Error::other("У закрепления отсутствуют нативные границы."))?;
    let pin_rect = [
        f32::from(bounds.left()).floor() as u32,
        f32::from(bounds.top()).floor() as u32,
        f32::from(bounds.right()).ceil() as u32,
        f32::from(bounds.bottom()).ceil() as u32,
    ];
    let [left, top, right, bottom] = pin_rect;
    let sidebar_right = (geometry.inset + geometry.rail).floor() as u32;
    let player_top = (scene.height - geometry.inset - geometry.player).ceil() as u32;
    let prefix = format!("resize-{}x{}-pinned", scene.width, scene.height);
    let pin_frame = format!("{prefix}-focus.png");
    pinned.save(output.join(&pin_frame))?;
    require(
        left < right && top < bottom && right <= sidebar_right && bottom < player_top,
        &format!(
            "Ряд закрепления должен помещаться в боковой панели выше плеера: {pin_rect:?}, y плеера {player_top}."
        ),
    )?;

    focus(raw, &named("home-settings"), cx)?;
    let settings = render_image(raw, scene, cx)?;
    let settings_frame = format!("{prefix}-settings-focus.png");
    settings.save(output.join(&settings_frame))?;
    let sidebar = geometry.inset.ceil() as u32..sidebar_right;
    let pin_changes = changed_pixels(&base, &pinned, left..right, top..bottom);
    let settings_changes = changed_pixels(&pinned, &settings, sidebar.clone(), bottom..player_top);
    let player_changes = changed_pixels(&pinned, &settings, sidebar, player_top..settings.height());
    let tree = snapshot(raw, cx)?;
    let evidence = json!({
        "client_px": [scene.width as u32, scene.height as u32],
        "pin_bounds_px_exclusive": pin_rect,
        "player_top_px": player_top,
        "pin_focus_changed_pixels": pin_changes,
        "settings_focus_changed_pixels_between_pin_and_player": settings_changes,
        "settings_focus_changed_pixels_inside_player": player_changes,
        "pin_frame": pin_frame,
        "settings_frame": settings_frame,
    });
    require(
        pin_changes > 0
            && settings_changes > 0
            && player_changes == 0
            && focused(&tree) == Some(&named("home-settings"))
            && node_label(&tree, "home-settings") == Some("Настройки")
            && node_label(&tree, "home-pinned") == Some("Закреплённое"),
        &format!("Закрепление и настройки требуют видимого фокуса над плеером: {evidence}"),
    )?;
    Ok(evidence)
}

fn inspect_sidebar_footer(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Vec<Value>> {
    let raw = window.into();
    let geometry = Geometry::from(window.update(cx, |view, window, _| {
        view.fluid_glass_geometry_for_capture(window)
    })?);
    let sidebar_right = (geometry.inset + geometry.rail).floor() as u32;
    let player_top = (scene.height - geometry.inset - geometry.player).ceil() as u32;
    let base = render_image(raw, scene, cx)?;
    let mut observations = Vec::new();
    for (id, label) in [
        ("home-accounts", "Гостевой вход"),
        ("home-settings", "Настройки"),
    ] {
        focus(raw, &named(id), cx)?;
        let tree = snapshot(raw, cx)?;
        require(
            node_label(&tree, id).is_some_and(|name| name.contains(label)),
            &format!("Отсутствует доступное имя нижнего пункта меню {id}."),
        )?;
        let frame = render_image(raw, scene, cx)?;
        let changed = |rows| {
            changed_pixels(
                &base,
                &frame,
                geometry.inset.ceil() as u32..sidebar_right,
                rows,
            )
        };
        let name = format!("resize-{}x{}-focused-{id}.png", scene.width, scene.height);
        frame.save(output.join(&name))?;
        observations.push(json!({
            "client_px": [scene.width as u32, scene.height as u32],
            "element": id,
            "frame": name,
            "focus_changes_above_player": changed(0..player_top),
            "focus_changes_inside_player": changed(player_top..frame.height()),
        }));
    }
    let albums = if scene.height >= FULL_NAV_HEIGHT {
        ALBUMS_RAIL
    } else {
        ALBUMS_TAB
    };
    focus(raw, &named(albums), cx)?;
    Ok(observations)
}

fn changed_pixels(
    before: &image::RgbaImage,
    after: &image::RgbaImage,
    columns: Range<u32>,
    rows: Range<u32>,
) -> usize {
    rows.flat_map(|y| columns.clone().map(move |x| (x, y)))
        .filter(|&(x, y)| before.get_pixel(x, y) != after.get_pixel(x, y))
        .count()
}

#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    inset: f32,
    rail: f32,
    player: f32,
    inner_radius: f32,
    margin: f32,
    body_bottom: f32,
    content_width: f32,
}

impl From<[f32; 7]> for Geometry {
    fn from(
        [
            inset,
            rail,
            player,
            inner_radius,
            margin,
            body_bottom,
            content_width,
        ]: [f32; 7],
    ) -> Self {
        Self {
            inset,
            rail,
            player,
            inner_radius,
            margin,
            body_bottom,
            content_width,
        }
    }
}

impl Geometry {
    fn available_height(self, height: f32) -> f32 {
        height - 2. * self.inset - self.player - self.body_bottom
    }
}

struct Observation {
    scene: Scene,
    geometry: Geometry,
    rail: Ridge,
    player: Ridge,
    focus: Option<String>,
    heading: Option<String>,
    albums_active: bool,
    albums_tab_selected: bool,
    workspace_tabs: bool,
    shared_controls: bool,
}

impl Observation {
    fn full_navigation(&self) -> bool {
        self.geometry.rail >= WIDE_RAIL && self.scene.height >= FULL_NAV_HEIGHT
    }

    fn summary(&self) -> Value {
        let geometry = self.geometry;
        json!({
            "client_px": [self.scene.width as u32, self.scene.height as u32],
            "geometry_dip": {
                "inset": geometry.inset,
                "rail": geometry.rail,
                "player": geometry.player,
                "inner_radius": geometry.inner_radius,
                "workspace_margin": geometry.margin,
                "workspace_bottom_margin": geometry.body_bottom,
                "workspace_available_width": geometry.content_width,
                "workspace_available_height": geometry.available_height(self.scene.height),
            },
            "rail_optical_ridge": self.rail.summary(),
            "player_optical_ridge": self.player.summary(),
            "full_navigation": self.full_navigation(),
            "workspace_tabs": self.workspace_tabs,
            "workspace_heading": self.heading,
            "albums_rail_active": self.albums_active,
            "albums_tab_selected": self.albums_tab_selected,
            "accessible_search_player_navigation_and_album_count": self.shared_controls,
            "keyboard_focus": self.focus,
        })
    }

    fn verify(&self) -> gpui::Result<()> {
        let full = self.full_navigation();
        let expected_focus = named(if full { ALBUMS_RAIL } else { ALBUMS_TAB });
        let title = if full {
            "Альбомы"
        } else {
            "Медиатека"
        };
        let expected_width = self.scene.width
            - 2. * self.geometry.inset
            - self.geometry.rail
            - 2. * self.geometry.margin;
        require(
            self.focus.as_deref() == Some(expected_focus.as_str())
                && self.heading.as_deref() == Some(title)
                && self.albums_active == full
                && self.albums_tab_selected != full
                && self.workspace_tabs != full
                && self.shared_controls
                && (self.geometry.content_width - expected_width).abs() < 0.05
                && self.rail.aligned()
                && self.player.aligned(),
            &format!(
                "Несогласованные геометрия или фокус при размере {}×{}: {}",
                self.scene.width,
                self.scene.height,
                self.summary()
            ),
        )
    }
}

fn verify_geometry(widths: &[Observation], heights: &[Observation]) -> gpui::Result<()> {
    let width_grows = widths.windows(2).all(|pair| {
        let left = pair[0].geometry;
        let right = pair[1].geometry;
        left.rail < right.rail
            && left.margin < right.margin
            && left.body_bottom < right.body_bottom
            && left.content_width < right.content_width
            && left.inset == right.inset
            && left.player == right.player
            && left.inner_radius == right.inner_radius
    });
    let height_grows = heights.windows(2).all(|pair| {
        let low = pair[0].geometry;
        let high = pair[1].geometry;
        let stretches = pair[0].scene.height < 600. && pair[1].scene.height > 460.;
        let advances = |previous, next| {
            if stretches {
                previous < next
            } else {
                previous == next
            }
        };
        advances(low.inset, high.inset)
            && advances(low.player, high.player)
            && advances(low.inner_radius, high.inner_radius)
            && advances(high.content_width, low.content_width)
            && low.available_height(pair[0].scene.height)
                < high.available_height(pair[1].scene.height)
            && low.rail == high.rail
            && low.margin == high.margin
            && low.body_bottom == high.body_bottom
    });
    require(
        width_grows
            && height_grows
            && widths[0].geometry == Geometry::from([20., 72., 100., 56., 16., 4., 756.])
            && widths[5].geometry == Geometry::from([20., 248., 100., 56., 40., 12., 992.])
            && heights[0].geometry == Geometry::from([8., 248., 62., 24., 40., 12., 1016.])
            && heights[6].geometry == widths[5].geometry
            && !widths[2].full_navigation()
            && widths[3].full_navigation()
            && !heights[5].full_navigation()
            && heights[6].full_navigation(),
        "Промежуточные размеры должны последовательно менять оболочку и рабочую область с ожидаемыми крайними значениями и порогами навигации.",
    )
}

struct Ridge {
    expected: f32,
    peak: u32,
    prominence: f64,
}

impl Ridge {
    fn aligned(&self) -> bool {
        (self.peak as f32 + 0.5 - self.expected).abs() < 3. && self.prominence > 0.01
    }

    fn summary(&self) -> Value {
        json!({
            "expected_px": self.expected,
            "brightest_pixel": self.peak,
            "relative_luminance_above_adjacent_max": self.prominence,
        })
    }
}

fn ridge(expected: f32, sample: impl Fn(u32) -> f64) -> Ridge {
    let center = expected.round() as u32;
    let (peak, brightness) = (center - 3..=center + 3)
        .map(|pixel| (pixel, sample(pixel)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("Seven pixels around the optical contour");
    Ridge {
        expected,
        peak,
        prominence: brightness - sample(center - 6).max(sample(center + 6)),
    }
}

fn mean_luma(pixels: impl Iterator<Item = [u8; 4]>) -> f64 {
    let (sum, count) = pixels.fold((0., 0.), |(sum, count), [r, g, b, _]| {
        (sum + luminance([r, g, b]), count + 1.)
    });
    sum / count
}
