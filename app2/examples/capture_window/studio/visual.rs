//! Native acceptance of the independent palette, pattern, optical and quality
//! layers, including the live preview and a pattern/palette matrix.

use std::{
    env, fs, io,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::{
    GlassPalette, MainView, PatternId, PatternSelection, QualityChoice, VisualFlags, VisualSettings,
};
use rusqlite::Connection;
use serde_json::json;

use super::super::sound::node_label;
use super::{
    super::{Scene, draw, render_image},
    details, focus, focused, frame, junction, key, named, node, press, require, snapshot, wait,
};

const ARTWORK: &str = "https://i.scdn.co/image/fluid-scroll-album";

fn visual(pattern: PatternId, quality: QualityChoice, effects: VisualFlags) -> VisualSettings {
    VisualSettings {
        pattern: PatternSelection::Fixed(pattern),
        intensity: 0.65,
        speed: 0.35,
        effects,
        quality,
        audio: Default::default(),
        surface: Default::default(),
    }
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    profile: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(details::ALBUM_URL, window, cx)
    })?;
    wait(cx, details::ALBUM_TITLE, |cx| {
        let tree = snapshot(raw, cx)?;
        let decoded = window.update(cx, |view, _, cx| {
            view.artist_artwork_decoded_for_capture(ARTWORK, cx)
        })?;
        Ok(
            (decoded && node_label(&tree, "music-object-title") == Some(details::ALBUM_TITLE))
                .then_some(()),
        )
    })
    .await?;
    window.update(cx, |view, _, cx| {
        view.set_fluid_glass_cover_for_capture(ARTWORK, cx)
    })?;
    junction::verify(window, scene, output, cx)?;

    if scene.name != "1920x1080" {
        return fraction(window, scene, output, cx).await;
    }

    let quick = env::var_os("MEDIADEKA_VISUAL_QUICK").is_some();
    if quick {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_for_capture(Duration::from_millis(8000), cx)
        })?;
        draw(raw, scene, Some(&output.join("album-auto.png")), cx)?;
        benchmark(window, scene, output, cx).await?;
        return settings(window, scene, output, profile, None, cx).await;
    }

    // Deterministic still frames: the same pattern, seed and time must paint
    // the same pixels. Static quality also settles the 380 ms retarget.
    window.update(cx, |view, window, cx| {
        view.set_visual_for_capture(
            GlassPalette::Solar,
            visual(PatternId::GemSmoke, QualityChoice::Static, VisualFlags::ALL),
            window,
            cx,
        )
    })?;
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::from_millis(8000), cx)
    })?;
    // The preceding geometry check closed the queue. Its control glyph uses
    // wall time independently of the static material phase.
    wait(
        cx,
        "завершение морфа иконки очереди",
        |cx| {
            snapshot(raw, cx)?;
            window.update(cx, |view, _, cx| {
                (!view.fluid_icon_motion_active_for_capture(cx)).then_some(())
            })
        },
    )
    .await?;
    draw(
        raw,
        scene,
        Some(&output.join("pattern-gem-smoke-a.png")),
        cx,
    )?;
    draw(
        raw,
        scene,
        Some(&output.join("pattern-gem-smoke-b.png")),
        cx,
    )?;
    let first = image::open(output.join("pattern-gem-smoke-a.png"))?.to_rgba8();
    let second = image::open(output.join("pattern-gem-smoke-b.png"))?.to_rgba8();
    let different = first
        .pixels()
        .zip(second.pixels())
        .filter(|(a, b)| a != b)
        .count();
    require(
        different == 0,
        "Повторный кадр того же паттерна, seed и времени отличается.",
    )?;
    fs::write(
        output.join("determinism.json"),
        serde_json::to_vec_pretty(&json!({
            "pattern": "gem-smoke",
            "seed": 0,
            "time_ms": 8000,
            "different_pixels": different,
        }))?,
    )?;

    let mut capture_ms = Vec::new();
    // Every palette with every pattern. The motif is palette-free; only the
    // colour mapping changes, so this also proves the two layers are
    // independent in the native renderer.
    let palette_only = env::var("MEDIADEKA_VISUAL_PALETTE").ok();
    let pattern_only = env::var("MEDIADEKA_VISUAL_PATTERN").ok();
    for palette in GlassPalette::ALL {
        if palette_only
            .as_deref()
            .is_some_and(|name| name != palette.as_str())
        {
            continue;
        }
        for pattern in PatternId::ALL {
            if pattern_only
                .as_deref()
                .is_some_and(|name| name != pattern.as_str())
            {
                continue;
            }
            let chosen = visual(pattern, QualityChoice::Static, VisualFlags::ALL);
            window.update(cx, |view, window, cx| {
                view.set_visual_for_capture(palette, chosen, window, cx)
            })?;
            window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_for_capture(Duration::from_millis(8000), cx)
            })?;
            let started = Instant::now();
            draw(
                raw,
                scene,
                Some(&output.join(format!(
                    "matrix-{}-{}.png",
                    palette.as_str(),
                    pattern.as_str()
                ))),
                cx,
            )?;
            capture_ms.push(started.elapsed().as_secs_f64() * 1000.);
            if palette == GlassPalette::Monochrome {
                let applied =
                    window.update(cx, |view, _, cx| view.fluid_glass_pattern_for_capture(cx))?;
                require(
                    applied.is_some_and(|(a, b, mix, _, _)| {
                        a == pattern.as_str() && b == a && mix == 0.
                    }),
                    &format!("Рендерер не переключился на паттерн {}.", pattern.as_str()),
                )?;
            }
        }
    }

    for (name, effects) in [
        ("effects-none", VisualFlags::NONE),
        (
            "effects-optics",
            VisualFlags::from_bits(
                VisualFlags::FLUTED | VisualFlags::REFRACTION | VisualFlags::DISPERSION,
            ),
        ),
        (
            "effects-post",
            VisualFlags::from_bits(
                VisualFlags::GRAIN
                    | VisualFlags::BLOOM
                    | VisualFlags::GLITCH
                    | VisualFlags::VIGNETTE,
            ),
        ),
    ] {
        window.update(cx, |view, window, cx| {
            view.set_visual_for_capture(
                GlassPalette::Rosewood,
                visual(PatternId::LightLeak, QualityChoice::Static, effects),
                window,
                cx,
            )
        })?;
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_for_capture(Duration::from_millis(8000), cx)
        })?;
        draw(raw, scene, Some(&output.join(format!("{name}.png"))), cx)?;
    }
    let none_frame = image::open(output.join("effects-none.png"))?.to_rgba8();
    let mut effect_differences = serde_json::Map::new();
    for name in ["effects-optics", "effects-post"] {
        let frame = image::open(output.join(format!("{name}.png")))?.to_rgba8();
        let different = none_frame
            .pixels()
            .zip(frame.pixels())
            .filter(|(a, b)| a != b)
            .count();
        require(
            different > 1000,
            &format!("Переключатель {name} не изменил кадр."),
        )?;
        effect_differences.insert(name.to_owned(), json!(different));
    }
    let average_capture = capture_ms.iter().sum::<f64>() / capture_ms.len() as f64;
    let maximum_capture = capture_ms.iter().copied().fold(0.0_f64, f64::max);
    fs::write(
        output.join("timing.json"),
        serde_json::to_vec_pretty(&json!({
            "frames": capture_ms.len(),
            "average_capture_ms": average_capture,
            "maximum_capture_ms": maximum_capture,
            "note": "native draw plus screenshot readback, not GPU timestamp time",
        }))?,
    )?;
    fs::write(
        output.join("effects.json"),
        serde_json::to_vec_pretty(&json!({
            "reference": "effects-none",
            "different_pixels": effect_differences,
        }))?,
    )?;

    benchmark(window, scene, output, cx).await?;
    settings(
        window,
        scene,
        output,
        profile,
        Some((capture_ms.len(), different)),
        cx,
    )
    .await
}

/// Times a full native draw with a readback, which forces the GPU to finish.
/// The absolute number includes reading 1920×1080 back to the CPU, so the
/// relative cost between patterns and profiles is the useful signal.
async fn benchmark(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let mut profile_ms = serde_json::Map::new();
    let mut pattern_ms = serde_json::Map::new();
    let mut measured = Vec::new();
    for pattern in PatternId::ALL {
        for quality in [
            QualityChoice::Balanced,
            QualityChoice::High,
            QualityChoice::Eco,
        ] {
            if quality != QualityChoice::High && pattern != PatternId::LiquidMetal {
                continue;
            }
            window.update(cx, |view, window, cx| {
                view.set_visual_for_capture(
                    GlassPalette::Auto,
                    visual(pattern, quality, VisualFlags::ALL),
                    window,
                    cx,
                )
            })?;
            for generation in 0..4 {
                window.update(cx, |view, _, cx| {
                    view.sample_fluid_glass_for_capture(
                        Duration::from_millis(10_000 + generation * 400),
                        cx,
                    )
                })?;
                let _ = render_image(raw, scene, cx)?;
            }
            let mut times = Vec::new();
            for generation in 0..3 {
                window.update(cx, |view, _, cx| {
                    view.sample_fluid_glass_for_capture(
                        Duration::from_millis(12_000 + generation * 400),
                        cx,
                    )
                })?;
                let started = Instant::now();
                let _ = render_image(raw, scene, cx)?;
                times.push(started.elapsed().as_secs_f64() * 1000.);
            }
            let average = times.iter().sum::<f64>() / times.len() as f64;
            measured.push(average);
            if quality == QualityChoice::High {
                pattern_ms.insert(pattern.as_str().to_owned(), json!(average));
            } else {
                profile_ms.insert(quality.as_str().to_owned(), json!(average));
            }
        }
    }
    fs::write(
        output.join("benchmark.json"),
        serde_json::to_vec_pretty(&json!({
            "draw_and_readback_ms": {
                "by_pattern_high": pattern_ms,
                "liquid_metal_profiles": profile_ms,
            },
            "note": "window.draw plus 1920×1080 GPU readback per frame; includes the capture readback cost, not only shading",
        }))?,
    )?;
    let _ = measured;
    Ok(())
}

async fn settings(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    profile: &Path,
    matrix: Option<(usize, usize)>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    // Start with both enabled and disabled hidden flags to verify that changing
    // a visible effect preserves the saved values of the hidden ones.
    let initial_effects = VisualFlags::from_bits(VisualFlags::ALL.bits() & !VisualFlags::GRAIN);
    let expected_effects = initial_effects.bits() & !VisualFlags::BLOOM;
    window.update(cx, |view, window, cx| {
        view.enable_settings_diagnostics_for_capture(cx);
        view.set_visual_for_capture(
            GlassPalette::Auto,
            VisualSettings {
                pattern: PatternSelection::Auto,
                intensity: 0.65,
                speed: 0.35,
                effects: initial_effects,
                quality: QualityChoice::Auto,
                audio: Default::default(),
                surface: Default::default(),
            },
            window,
            cx,
        )
    })?;
    window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    wait(cx, "настройки", |cx| {
        Ok(window
            .update(cx, |view, _, _| view.is_settings_for_capture())?
            .then_some(()))
    })
    .await?;
    press(raw, &named("settings-appearance-shortcut"), cx)?;
    // Let the contextual retarget settle so the capture compares the same
    // pattern family the live preview names.
    for time in [12_000, 13_000] {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_for_capture(Duration::from_millis(time), cx)
        })?;
        draw(raw, scene, None, cx)?;
    }
    draw(raw, scene, Some(&output.join("settings-visual.png")), cx)?;
    let tree = snapshot(raw, cx)?;
    for id in [
        "appearance-visual",
        "appearance-pattern-group",
        "appearance-quality-group",
        "appearance-intensity-group",
        "appearance-speed-group",
        "appearance-opacity-group",
        "appearance-blur-group",
        "appearance-neutral-background",
        "appearance-effects-group",
        "appearance-copy-preset",
        "fluid-glass-live-preview-card",
        "appearance-pattern-auto",
        "appearance-pattern-liquid-metal",
        "appearance-quality-select",
        "appearance-effect-16",
    ] {
        require(
            node(&tree, id).is_some(),
            &format!("В настройках нет {id}."),
        )?;
    }
    eprintln!("visual: opening gallery");
    press(raw, &named("appearance-gallery-toggle"), cx)?;
    draw(raw, scene, Some(&output.join("settings-gallery.png")), cx)?;
    eprintln!("visual: gallery captured");
    let gallery = snapshot(raw, cx)?;
    for pattern in PatternId::ALL {
        let id = format!("appearance-gallery-{}", pattern.as_str());
        require(
            node(&gallery, &id).is_some(),
            &format!("В Shader Gallery нет {id}."),
        )?;
    }
    require(
        node(&gallery, "appearance-gallery-grid").is_some(),
        "Сетка Shader Gallery не построена.",
    )?;
    press(raw, &named("shader-debug-toggle"), cx)?;
    draw(raw, scene, None, cx)?;
    require(
        window.update(cx, |view, window, cx| {
            view.reveal_shader_debug_for_capture(window, cx)
        })?,
        "Нативный кадр не содержит область сравнения Shader Debug Studio.",
    )?;
    let studio = frame(raw, scene, output, "shader-debug-studio", cx)?;
    require(
        node(&studio, "shader-debug-comparison").is_some(),
        "Shader Debug Studio не показывает сравнение.",
    )?;
    press(raw, &named("shader-debug-pass"), cx)?;
    choose_menu_item(raw, "shader-debug-pass-2", 2, cx)?;
    require(
        focused(&snapshot(raw, cx)?) == Some(named("shader-debug-pass").as_str()),
        "Селектор диаграмм должен восстанавливать фокус после выбора.",
    )?;
    frame(raw, scene, output, "shader-debug-normals", cx)?;
    press(raw, &named("shader-debug-toggle"), cx)?;
    press(raw, &named("appearance-gallery-toggle"), cx)?;
    press(raw, &named("settings-appearance-shortcut"), cx)?;
    eprintln!("visual: selecting pattern");
    focus(raw, &named("appearance-pattern-liquid-metal"), cx)?;
    key(raw, "enter", cx)?;
    eprintln!("visual: toggling effect");
    focus(raw, &named("appearance-effect-16"), cx)?;
    key(raw, "enter", cx)?;
    eprintln!("visual: selecting quality");
    press(raw, &named("appearance-quality-select"), cx)?;
    let quality_menu = frame(raw, scene, output, "settings-quality-menu", cx)?;
    require(
        node(&quality_menu, "appearance-quality-options").is_some()
            && node(&quality_menu, "appearance-quality-high").is_some(),
        "Селектор производительности не открыл меню.",
    )?;
    choose_menu_item(raw, "appearance-quality-high", 2, cx)?;
    require(
        focused(&snapshot(raw, cx)?) == Some(named("appearance-quality-select").as_str()),
        "Выбор профиля должен вернуть фокус селектору.",
    )?;
    focus(raw, &named("appearance-opacity-slider"), cx)?;
    key(raw, "end", cx)?;
    focus(raw, &named("appearance-blur-slider"), cx)?;
    key(raw, "home", cx)?;
    press(raw, &named("appearance-neutral-background"), cx)?;
    frame(raw, scene, output, "settings-neutral-background", cx)?;
    eprintln!("visual: waiting for saved preset");
    if let Err(error) = wait(
        cx,
        "сохранённый визуальный пресет",
        |_cx| {
            Ok(saved_visual(profile)
                .is_some_and(|(pattern, effects, quality)| {
                    pattern == "liquid-metal" && effects == expected_effects && quality == "high"
                })
                .then_some(()))
        },
    )
    .await
    {
        eprintln!(
            "visual diagnostics: saved={:?} db={} error={:?}",
            saved_visual(profile),
            profile.join("mediadeka.sqlite3").exists(),
            window.update(cx, |view, _, _| {
                view.appearance_error_for_capture().map(str::to_owned)
            })?,
        );
        return Err(error);
    }
    let chosen = window.update(cx, |view, _, _| view.visual_for_capture())?;
    require(
        chosen.pattern == PatternSelection::Fixed(PatternId::LiquidMetal)
            && chosen.effects.bits() == expected_effects
            && chosen.quality == QualityChoice::High
            && chosen.surface.opacity == 100
            && chosen.surface.blur == 0
            && chosen.surface.neutral,
        "Настройки не применились к активному состоянию.",
    )?;
    draw(
        raw,
        scene,
        Some(&output.join("settings-visual-chosen.png")),
        cx,
    )?;
    press(raw, &named("appearance-copy-preset"), cx)?;
    let preset_text = window
        .update(cx, |view, _, cx| view.copied_visual_preset_for_capture(cx))?
        .ok_or_else(|| io::Error::other("Команда копирования не сформировала пресет."))?;
    let preset: serde_json::Value = serde_json::from_str(&preset_text)?;
    let (saved_shell, saved_palette) =
        saved_appearance(profile).ok_or_else(|| io::Error::other("Нет сохранённой оболочки."))?;
    let reopened = restart(output, profile, &saved_shell, &saved_palette)?;
    require(
        reopened["visual"]["pattern"] == "liquid-metal"
            && reopened["visual"]["effects"]
                .as_u64()
                .is_some_and(|effects| effects == u64::from(expected_effects))
            && reopened["visual"]["quality"] == "high"
            && reopened["visual"]["surface"]["opacity"] == 100
            && reopened["visual"]["surface"]["blur"] == 0
            && reopened["visual"]["surface"]["neutral"] == true,
        "Перезапуск не восстановил независимые визуальные настройки.",
    )?;
    require(
        preset["pattern"] == "liquid-metal"
            && preset["effects"]["bloom"] == false
            && preset["effects"]["fluted"] == true
            && preset["effects"]["grain"] == false
            && preset["quality"] == "high"
            && preset["surface"] == reopened["visual"]["surface"],
        "Скопированный пресет не совпадает с выбранным оформлением.",
    )?;
    fs::write(
        output.join("preset.json"),
        serde_json::to_vec_pretty(&preset)?,
    )?;
    fs::write(
        output.join("visual.json"),
        serde_json::to_vec_pretty(&json!({
            "patterns": PatternId::ALL.map(|pattern| pattern.as_str()),
            "palettes": GlassPalette::ALL.map(|palette| palette.as_str()),
            "matrix_combinations": matrix.map(|(frames, _)| frames),
            "determinism": matrix.map(|(_, differences)| json!({"different_pixels": differences})),
            "keyboard": {
                "pattern": "liquid-metal",
                "bloom": false,
                "quality": "high",
            },
            "saved": saved_visual(profile).map(|(pattern, effects, quality)| json!({
                "pattern": pattern,
                "effects": effects,
                "quality": quality,
            })),
            "reopened": reopened["visual"].clone(),
            "live_preview": node(&tree, "fluid-glass-live-preview-card").is_some(),
            "benchmark": serde_json::from_slice::<serde_json::Value>(
                &fs::read(output.join("benchmark.json")).unwrap_or_default(),
            )
            .ok(),
        }))?,
    )?;
    if let Some((frames, different)) = matrix {
        println!(
            "{}: {frames} palette/pattern frames, determinism {different} differing pixels",
            scene.name
        );
    }
    println!(
        "{}: Shader Debug Studio, live preview and persisted preset checked",
        scene.name
    );
    Ok(())
}

fn choose_menu_item(
    raw: AnyWindowHandle,
    item: &str,
    steps: usize,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    for _ in 0..steps {
        key(raw, "down", cx)?;
    }
    let tree = snapshot(raw, cx)?;
    let active = tree["active_descendant_focus"]
        .as_str()
        .and_then(|id| tree["nodes"][id]["element_id"].as_str());
    require(
        active == Some(named(item).as_str()),
        &format!("Стрелки не выбрали ожидаемую строку меню {item}."),
    )?;
    key(raw, "enter", cx)
}

async fn fraction(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::from_millis(8000), cx)
    })?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("album-{}.png", scene.name))),
        cx,
    )?;
    let pattern = window.update(cx, |view, _, cx| view.fluid_glass_pattern_for_capture(cx))?;
    require(
        pattern.is_some(),
        "Fractional window did not render the contextual layer.",
    )?;
    window.update(cx, |view, window, cx| {
        view.show_settings_for_capture(window, cx)
    })?;
    wait(cx, "настройки", |cx| {
        Ok(window
            .update(cx, |view, _, _| view.is_settings_for_capture())?
            .then_some(()))
    })
    .await?;
    press(raw, &named("settings-appearance-shortcut"), cx)?;
    draw(
        raw,
        scene,
        Some(&output.join(format!("settings-{}.png", scene.name))),
        cx,
    )?;
    fs::write(
        output.join(format!("fraction-{}.json", scene.name)),
        serde_json::to_vec_pretty(&json!({
            "scene": scene.name,
            "dpi": scene.dpi,
            "pattern": pattern.map(|(a, b, mix, intensity, seed)| json!({
                "a": a, "b": b, "mix": mix, "intensity": intensity, "seed": seed,
            })),
        }))?,
    )?;
    println!(
        "{}: fractional window kept the contextual layer and settings",
        scene.name
    );
    Ok(())
}

fn saved_appearance(profile: &Path) -> Option<(String, String)> {
    let connection = Connection::open(profile.join("mediadeka.sqlite3")).ok()?;
    connection
        .query_row(
            "SELECT shell, glass_palette FROM appearance WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()
}

fn restart(
    output: &Path,
    profile: &Path,
    shell: &str,
    palette: &str,
) -> gpui::Result<serde_json::Value> {
    let directory = output.join("reopen-visual");
    fs::create_dir_all(&directory)?;
    let mut command = Command::new(env::current_exe()?);
    command
        .arg("--appearance-reopen")
        .arg(profile)
        .arg(&directory)
        .arg(shell)
        .arg(palette);
    for name in [
        "MEDIADEKA_GUI",
        "MEDIADEKA_GLASS_THEME",
        "MEDIADEKA_GLASS_EFFECTS",
        "MEDIADEKA_GLASS_QUALITY",
        "MEDIADEKA_GLASS_PATTERN",
        "MEDIADEKA_GLASS_INTENSITY",
        "MEDIADEKA_GLASS_SPEED",
        "MEDIADEKA_VISUAL_QUICK",
        "MEDIADEKA_STUDIO_SCENE",
        "MEDIADEKA_PROFILE_DIR",
    ] {
        command.env_remove(name);
    }
    let result = command.output()?;
    fs::write(directory.join("stdout.txt"), &result.stdout)?;
    fs::write(directory.join("stderr.txt"), &result.stderr)?;
    require(
        result.status.success(),
        &format!(
            "Перезапуск визуальных настроек завершился с {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ),
    )?;
    Ok(serde_json::from_slice(&fs::read(
        directory.join("reopen.json"),
    )?)?)
}

fn saved_visual(profile: &Path) -> Option<(String, u8, String)> {
    let connection = Connection::open(profile.join("mediadeka.sqlite3")).ok()?;
    connection
        .query_row(
            "SELECT pattern, effects, quality FROM appearance WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
}
