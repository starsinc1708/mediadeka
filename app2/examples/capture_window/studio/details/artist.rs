use std::{fs, io, path::Path, time::Duration};

use gpui::{AsyncApp, WindowHandle};
use image::Rgb;
use mediadeka::{
    MainView,
    playlist::{Media, PlaylistTrack},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::super::{
    Scene, by_element, focus, focused, frame, key, named, node, node_label, press, queue_candidate,
    require, shader_with_artwork_palette, snapshot, sound, wait,
};

pub(in super::super) const ARTIST_REGION: &str = "US";
const TITLE: &str = "Северный ансамбль";
const TRACK: &str = "Поворот реки";
const SECOND_TRACK: &str = "Течение ночью";
const HEADING: &str = "music-object-title";
const PLAY: &str = "music-object-play";
const PORTRAIT: &str = "music-object-artwork";
const SMALL_PORTRAIT_PIXELS: u32 = 64;
const PALETTE_SETTLE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    id: &'static str,
    source_pixels: Option<u32>,
    panorama: bool,
}

const CASES: [Case; 4] = [
    Case {
        name: "artist-hero-large",
        id: "ArtistPortraitLarge001",
        source_pixels: Some(960),
        panorama: false,
    },
    Case {
        name: "artist-hero-panorama",
        id: "ArtistPanoramaWide0011",
        source_pixels: Some(1400),
        panorama: true,
    },
    Case {
        name: "artist-hero-small",
        id: "ArtistPortraitSmall001",
        source_pixels: Some(SMALL_PORTRAIT_PIXELS),
        panorama: false,
    },
    Case {
        name: "artist-hero-missing",
        id: "ArtistWithoutPortrait1",
        source_pixels: None,
        panorama: false,
    },
];

impl Case {
    fn url(self) -> String {
        format!("https://open.spotify.com/artist/{}", self.id)
    }

    fn artwork(self) -> Option<String> {
        self.source_pixels
            .map(|_| format!("https://i.scdn.co/image/{}", self.name))
    }
}

pub(in super::super) fn seed_artists(profile: &Path) -> gpui::Result<()> {
    let mut database = Connection::open(profile.join("mediadeka.sqlite3"))?;
    let transaction = database.transaction()?;
    let tracks = [
        (TRACK, 18, "track-0.wav"),
        (SECOND_TRACK, 22, "track-1.wav"),
    ]
    .map(|(title, duration, file)| PlaylistTrack {
        id: format!("native-artist-hero-wave-{duration}"),
        title: title.into(),
        artist: TITLE.into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(duration),
        media: Some(Media::File(profile.join(file))),
    });
    for case in CASES {
        let artwork = case.artwork();
        if let Some((url, pixels)) = artwork.as_deref().zip(case.source_pixels) {
            let path = profile.join(format!("{}.cover", case.name));
            let mut encoded = io::Cursor::new(vec![1]); // JPEG cache format tag.
            encoded.set_position(1);
            let height = if case.panorama { pixels / 2 } else { pixels };
            let image = image::RgbImage::from_fn(pixels, height, |x, y| {
                let color = fixture_pixel(
                    x as f32 / pixels as f32,
                    y as f32 / height as f32,
                    pixels == SMALL_PORTRAIT_PIXELS,
                );
                if case.panorama {
                    let [r, g, b] = color.0;
                    Rgb([g / 2, b.saturating_add(28), r])
                } else {
                    color
                }
            });
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90)
                .encode_image(&image)?;
            fs::write(&path, encoded.into_inner())?;
            MainView::seed_artist_artwork_for_capture(profile, url, &path)?;
        }
        let data = json!({
            "reference": {"provider": "spotify", "kind": "Artist", "id": case.id},
            "title": TITLE,
            "artists": [],
            "artwork": artwork,
            "url": case.url(),
            "tracks": tracks,
            "releases": if case.name == CASES[0].name {
                super::album::SHORT_RELEASES.iter().map(|release| release.item()).collect::<Vec<_>>()
            } else {
                Vec::new()
            },
            "single": false,
            "artist_details": {
                "banner": case.panorama.then(|| artwork.clone()).flatten(),
                "monthly_listeners": 134_635,
                "biography": (case.name == CASES[0].name).then_some("Камерный ансамбль из Хельсинки."),
                "related": if case.name == CASES[0].name {
                    vec![json!({
                        "id": format!("spotify:artist:{}", CASES[1].id),
                        "title": "Северное трио",
                        "subtitle": "Исполнитель",
                        "url": CASES[1].url(),
                        "artwork": CASES[1].artwork(),
                    })]
                } else {
                    Vec::new()
                },
                "release_is_single": {},
            },
        });
        transaction.execute(
            "INSERT INTO music_snapshot (provider, profile_id, region, slot, payload)
             VALUES ('spotify', '', ?1, ?2, ?3)",
            params![
                ARTIST_REGION,
                format!("artist:{}", case.id),
                data.to_string()
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

// A repeatable portrait-like color fixture, with a saturated calibration color in the small image.
fn fixture_pixel(x: f32, y: f32, small: bool) -> Rgb<u8> {
    let ellipse = |center_x: f32, center_y: f32, radius_x: f32, radius_y: f32| {
        ((x - center_x) / radius_x).powi(2) + ((y - center_y) / radius_y).powi(2)
    };
    let torso = ellipse(0.51, 1.04, 0.4, 0.37);
    let face = ellipse(0.56, 0.43, 0.18, 0.27);
    let hair = ellipse(0.52, 0.29, 0.215, 0.165);
    let [r, g, b] = if face < 1. && hair > 0.85 + 0.9 * (y - x + 0.24) {
        let light = (1.1 - 1.35 * x + 0.21 * y).clamp(0.19, 0.86);
        [165. + 89. * light, 91. + 96. * light, 87. + 85. * light]
    } else if hair < 1. || torso < 1. {
        let light = (0.6 - 0.56 * x + 0.54 * y) * (0.5 + 0.5 * torso.min(1.));
        [27. + 57. * light, 20. + 35. * light, 41. + 79. * light]
    } else if small {
        [27., 238., 141.]
    } else {
        let light = 1. / (1. + 4. * ellipse(0.32, 0.28, 0.52, 0.59));
        [45. + 146. * light, 46. + 52. * light, 78. + 28. * light]
    };
    let grain = ((x * 5167. + y * 8831.).sin() * 15.7).fract() * 8.;
    Rgb([r, g, b].map(|channel| (channel + grain) as u8))
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let focus_id = named(PLAY);
    let automatic_palette = shader_with_artwork_palette()
        && window.update(cx, |view, window, cx| {
            view.fluid_glass_quality_for_capture(window, cx).is_some()
        })?;
    let mut previous_palette = None;
    let mut render_time = Duration::ZERO;
    let mut observations = Vec::with_capacity(CASES.len());
    for case in CASES {
        window.update(cx, |view, window, cx| {
            view.submit_music_url_for_capture(&case.url(), window, cx);
        })?;
        let artwork = case.artwork();
        let palette = wait(cx, case.name, |cx| {
            let tree = snapshot(raw, cx)?;
            let (details, palette) = window.update(cx, |view, _, cx| {
                (
                    view.music_semantics_for_capture(),
                    view.fluid_glass_page_palette_for_capture(cx),
                )
            })?;
            Ok((!details["pending"].as_bool().unwrap_or(true)
                && details["details"]["kind"] == "artist"
                && node_label(&tree, HEADING) == Some(TITLE)
                && node_label(&tree, PLAY) == Some("Слушать")
                && node_label(&tree, PORTRAIT)
                    == artwork.as_ref().map(|_| {
                        if case.panorama {
                            "Панорамная фотография исполнителя"
                        } else {
                            "Портрет исполнителя"
                        }
                    })
                && (!automatic_palette || palette.is_some() == artwork.is_some()))
            .then_some(palette))
        })
        .await?;
        if automatic_palette && palette.is_some() {
            require(
                palette != previous_palette,
                "Разные фотографии должны давать разные палитры страницы.",
            )?;
            previous_palette = palette;
        }
        render_time += PALETTE_SETTLE;
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_palette_for_capture(render_time, Duration::ZERO, cx);
        })?;
        let tree = frame(raw, scene, output, case.name, cx)?;
        require(
            node(&tree, HEADING).is_some_and(|element| element["aria"]["role"] == "Heading")
                && node(&tree, PLAY).is_some_and(|element| element["aria"]["role"] == "Button")
                && node(&tree, PORTRAIT).is_some_and(|element| element["aria"]["role"] == "Image")
                    == artwork.is_some(),
            &format!("Роли заголовка, действия и фото в состоянии {}.", case.name),
        )?;
        focus(raw, &focus_id, cx)?;
        let tree = frame(raw, scene, output, &format!("{}-focus", case.name), cx)?;
        require(
            focused(&tree) == Some(focus_id.as_str()),
            "Tab должен фокусировать команду «Слушать» на странице исполнителя.",
        )?;
        let playback = if case.source_pixels == CASES[0].source_pixels {
            key(raw, "enter", cx)?;
            let first = sound(window, TRACK, cx).await?;
            press(raw, &named("player-play"), cx)?;
            wait(
                cx,
                "пауза перед проверкой переходов",
                |cx| {
                    Ok(
                        (node_label(&snapshot(raw, cx)?, "player-play") == Some("Воспроизвести"))
                            .then_some(()),
                    )
                },
            )
            .await?;
            verify_sections(window, scene, output, cx)?;
            let controls = verify_track_and_source(window, scene, output, cx).await?;
            Some(json!({"hero_play": first, "track_and_platforms": controls}))
        } else {
            require(
                node_label(&tree, "artist-section-tracks") == Some("Треки")
                    && node(&tree, "artist-section-releases").is_none()
                    && node(&tree, "artist-section-related").is_none()
                    && node(&tree, "artist-section-biography").is_none(),
                "Навигация показывает только разделы, для которых есть данные.",
            )?;
            None
        };
        let marker = case
            .source_pixels
            .filter(|&pixels| pixels == SMALL_PORTRAIT_PIXELS)
            .map(|pixels| check_small_image(output, case, pixels))
            .transpose()?;
        observations.push(json!({
            "variant": case.name,
            "source_dimensions_px": case.source_pixels.map(|width| {
                [width, if case.panorama { width / 2 } else { width }]
            }),
            "portrait_palette": palette,
            "visible_marker_bounds_px_exclusive": marker,
            "visible_marker_extent_px": marker.map(|[left, top, right, bottom]| [right - left, bottom - top]),
            "tab_reached": PLAY,
            "playback": playback,
        }));
    }
    fs::write(
        output.join("artist-hero-report.json"),
        serde_json::to_vec_pretty(&json!({
            "logical_window_dip": [scene.width, scene.height],
            "dpi": scene.dpi,
            "portraits": observations,
        }))?,
    )?;
    press(raw, &named("studio-library"), cx)?;
    wait(
        cx,
        "возврат в медиатеку после проверки портретов",
        |cx| Ok(queue_candidate(&snapshot(raw, cx)?).is_some().then_some(())),
    )
    .await?;
    println!(
        "{}: large, small and missing artist portraits, keyboard focus and WAV playback verified",
        scene.name
    );
    Ok(())
}

fn verify_sections(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let overview = snapshot(raw, cx)?;
    require(
        node(&overview, "artist-section-navigation")
            .is_some_and(|nav| nav["aria"]["role"] == "Navigation"),
        "Список разделов артиста должен иметь роль навигации.",
    )?;
    let mut visits = Vec::new();
    for section in ["tracks", "related", "releases", "biography"] {
        press(raw, &named(&format!("artist-section-{section}")), cx)?;
        let target = named(&format!("artist-section-{section}-content"));
        let tree = frame(raw, scene, output, &format!("artist-section-{section}"), cx)?;
        let offset = window.update(cx, |view, _, _| -view.home_scroll_offset_for_capture())?;
        require(
            focused(&tree) == Some(&target)
                && node_label(&tree, "player-track") == Some(TRACK)
                && (scene.height > 400. || offset > 0.),
            &format!(
                "Переход к разделу {section} должен перенести фокус и сохранить трек. Смещение {offset} DIP, фокус {:?}.",
                focused(&tree)
            ),
        )?;
        visits.push(json!({"section": section, "focus": target, "scroll_offset_dip": offset}));
        press(raw, &named("music-object-title-top"), cx)?;
    }
    press(raw, &named("artist-section-overview"), cx)?;
    let offset = window.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
    require(
        offset.abs() < 1.,
        "Обзор должен открываться в начале страницы.",
    )?;
    fs::write(
        output.join("artist-sections-report.json"),
        serde_json::to_vec_pretty(
            &json!({"visits": visits, "overview_offset_dip": offset, "track": TRACK}),
        )?,
    )?;
    Ok(())
}

async fn verify_track_and_source(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = window.into();
    let first = r#"NamedInteger("music-track", 0)"#;
    let second = r#"NamedInteger("music-track", 1)"#;
    let current = |tree: &Value, state: &str| {
        by_element(tree, second).is_some_and(|row| row["aria"]["description"] == state)
            && by_element(tree, first).is_some_and(|row| row["aria"]["description"].is_null())
    };
    let scroll_offset =
        |cx: &mut AsyncApp| window.update(cx, |view, _, _| -view.home_scroll_offset_for_capture());
    let before_focus = scroll_offset(cx)?;
    focus(raw, second, cx)?;
    let focused_row = scroll_offset(cx)?;
    key(raw, "enter", cx)?;
    let playback = sound(window, SECOND_TRACK, cx).await?;
    const PLAYING: &str = "Текущий трек · воспроизводится";
    const PAUSED: &str = "Текущий трек · пауза";
    let playing = frame(raw, scene, output, "artist-track-playing", cx)?;
    require(
        current(&playing, PLAYING),
        "Только вторая строка должна сообщать о воспроизведении.",
    )?;
    press(raw, &named("player-play"), cx)?;
    wait(
        cx,
        "пауза во второй строке артиста",
        |cx| Ok(current(&snapshot(raw, cx)?, PAUSED).then_some(())),
    )
    .await?;
    frame(raw, scene, output, "artist-track-paused", cx)?;

    let trigger = named("music-object-hero-menu");
    let copy_count = |tree: &Value| {
        tree["nodes"].as_object().map_or(0, |nodes| {
            nodes
                .values()
                .filter(|row| {
                    row["aria"]["label"]
                        .as_str()
                        .is_some_and(|label| label.starts_with("Копировать ссылку"))
                })
                .count()
        })
    };
    let closed = snapshot(raw, cx)?;
    require(
        copy_count(&closed) == 0,
        "Копирование ссылки должно оставаться в меню страницы исполнителя.",
    )?;
    press(raw, &trigger, cx)?;
    let open = frame(raw, scene, output, "artist-platform-menu", cx)?;
    require(
        node(&open, "playlist-menu").is_some() && copy_count(&open) == 1,
        "Меню страницы исполнителя должно показать один пункт копирования ссылки Spotify.",
    )?;
    key(raw, "escape", cx)?;
    let closed = snapshot(raw, cx)?;
    require(
        node(&closed, "playlist-menu").is_none() && focused(&closed) == Some(trigger.as_str()),
        "Escape должен закрыть меню страницы и вернуть фокус к его кнопке.",
    )?;
    Ok(json!({
        "second_track_playback": playback,
        "scroll_offset_dip_before_and_after_tab": [before_focus, focused_row],
        "playing_row_description": PLAYING,
        "paused_row_description": PAUSED,
        "copy_actions_in_menu": copy_count(&open),
        "escape_returns_focus": true,
    }))
}

fn check_small_image(output: &Path, case: Case, pixels: u32) -> gpui::Result<[u32; 4]> {
    let frame = image::open(output.join(format!("{}.png", case.name)))?.into_rgb8();
    let mut regions: Vec<([u32; 4], u32)> = Vec::new();
    for (x, y, &Rgb([r, g, b])) in frame.enumerate_pixels() {
        // The blue component excludes Spotify's green service glyph.
        if g > 200 && g.saturating_sub(r) > 85 && b > 113 {
            // Bright reflections can share the portrait hue. Isolate the densest row band.
            if regions.last().is_none_or(|(bounds, _)| y > bounds[3]) {
                regions.push(([x, y, x + 1, y + 1], 1));
            } else if let Some(([left, _, right, bottom], count)) = regions.last_mut() {
                *left = (*left).min(x);
                *right = (*right).max(x + 1);
                *bottom = y + 1;
                *count += 1;
            }
        }
    }
    let ([left, top, right, bottom], _) = regions
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .ok_or_else(|| io::Error::other("В кадре нет цветовой метки тестового портрета."))?;
    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    require(
        (pixels / 4..=pixels + 1).contains(&width) && (pixels / 4..=pixels + 1).contains(&height),
        &format!(
            "Цветовая метка исходного фото {pixels} × {pixels} px заняла {width} × {height} px."
        ),
    )?;
    Ok([left, top, right, bottom])
}
