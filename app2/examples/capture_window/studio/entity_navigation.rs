use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle, point, px};
use mediadeka::{
    MainView,
    playlist::{ArtistCredit, Media, ObjectKind, ObjectRef, PlaylistTrack},
    providers::ProviderId,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::{
    COVER, Scene,
    control_feedback::{events, mouse_move},
    focus, focused, frame, key,
    library_row::{added_play_pixels, click, pause},
    named, node, node_label, press, require, snapshot, sound, wait,
};

const ALBUM_ID: &str = "NavAlbumRelease0000001";
const ALBUM: &str = "Прибрежный маршрут";
const TRACK: &str = "Маяк в тумане";
const RELATED_ALBUM: &str = "NavSecondRelease000001";
const RELEASE_TRACKS: [&str; 6] = [
    "Вечерний берег",
    "Северный ветер",
    "Горизонт",
    "Бухта",
    "Последний рейс",
    "Белая ночь",
];
const ARTISTS: [(&str, &str); 2] = [
    ("NavPrimaryArtist000001", "Полярная линия"),
    ("NavGuestArtist00000001", "Маяки"),
];

struct KnownRelease {
    address: String,
    release: String,
    track: String,
    count: usize,
    platform: Option<String>,
}

pub(crate) fn run_known_release(
    output: PathBuf,
    profile: PathBuf,
    mut arguments: impl Iterator<Item = OsString>,
) -> gpui::Result<()> {
    let usage = || {
        io::Error::other(
            "--music-pages <кадры> <изолированный профиль> track-release <URL трека> <релиз> <трек> <число треков> [площадка]",
        )
    };
    let mut next = || arguments.next().and_then(|arg| arg.into_string().ok());
    let expected = KnownRelease {
        address: next().ok_or_else(usage)?,
        release: next().ok_or_else(usage)?,
        track: next().ok_or_else(usage)?,
        count: next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(usage)?,
        platform: next(),
    };
    require(
        arguments.next().is_none() && profile.join("mediadeka.sqlite3").is_file(),
        "Нужна отдельная копия профиля.",
    )?;
    fs::create_dir_all(&output)?;
    // The acceptance process exclusively owns the supplied isolated profile.
    unsafe {
        env::set_var("MEDIADEKA_GUI", "3");
        env::set_var("MEDIADEKA_PROFILE_DIR", profile);
    }
    let scene = super::HERO_SCENES[0];
    mediadeka::application().run(move |cx| {
        let view = mediadeka::open_capture_window(cx, super::layout(scene))
            .unwrap_or_else(|error| super::fail(error));
        cx.spawn(async move |cx| {
            verify_known_release(view, scene, &expected, &output, cx)
                .await
                .unwrap_or_else(|error| super::fail(error));
            view.update(cx, |_, window, _| window.remove_window())
                .unwrap_or_else(|error| super::fail(error));
        })
        .detach();
    });
    Ok(())
}

async fn verify_known_release(
    view: WindowHandle<MainView>,
    scene: Scene,
    expected: &KnownRelease,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    super::activate_accessibility(raw, cx)?;
    view.update(cx, |view, window, cx| {
        view.start_home_for_capture(true, window, cx)
    })?;
    wait(cx, "сохранённый профиль", |cx| {
        view.update(cx, |view, _, _| view.home_status_for_capture())
    })
    .await?;
    let before = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(&expected.address, window, cx)
    })?;
    wait_for_release_selection(
        view,
        raw,
        &expected.release,
        &expected.track,
        expected.count,
        cx,
    )
    .await?;
    wait(
        cx,
        "декодированная обложка выбранного релиза",
        |cx| {
            super::draw(raw, scene, None, cx)?;
            let decoded = view.update(cx, |view, window, cx| {
                view.selected_release_artwork_decoded_for_capture(window, cx)
            })?;
            Ok(decoded.then_some(()))
        },
    )
    .await?;
    frame(raw, scene, output, "known-release", cx)?;
    press(raw, &named("release-track-info"), cx)?;
    wait(
        cx,
        "сведения и подтверждённая площадка трека",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok((node(&tree, "release-track-metadata").is_some()
                && expected.platform.as_ref().is_none_or(|platform| {
                    tree["nodes"]
                        .as_object()
                        .into_iter()
                        .flat_map(|nodes| nodes.values())
                        .any(|node| {
                            node["aria"]["role"] == "Link"
                                && node["aria"]["label"]
                                    .as_str()
                                    .is_some_and(|label| label.ends_with(&format!(" · {platform}")))
                        })
                }))
            .then_some(()))
        },
    )
    .await?;
    frame(raw, scene, output, "known-release-track-info", cx)?;
    let after = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    require(
        before["current_track"] == after["current_track"] && before["playing"] == after["playing"],
        "Открытие композиции сохранило звук и текущую запись.",
    )?;
    let model = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
    press(raw, &named("workspace-back"), cx)?;
    let returned = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
    require(
        returned["release_selection"].is_null(),
        "Назад завершает контекст композиции за один шаг.",
    )?;
    let report = json!({
        "selection": model["release_selection"], "required_track_platform": expected.platform,
        "artwork_decoded": true, "playback_preserved": true, "returned_page": returned["page"],
    });
    fs::write(
        output.join("known-release.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}

fn spotify_id(kind: ObjectKind, id: &str) -> ObjectRef {
    ObjectRef {
        provider: ProviderId::Spotify,
        kind,
        id: id.to_owned(),
    }
}

fn release_track_id(index: usize) -> String {
    format!("NavRelatedTrack{index:07}")
}

pub(super) fn seed(profile: &Path, region: &str) -> gpui::Result<()> {
    fs::write(
        profile.join("track-0.lrc"),
        "[00:01.00]Далёкий свет маяка\n[00:02.00]Берег скрывает туман\n",
    )?;
    let db = Connection::open(profile.join("mediadeka.sqlite3"))?;
    let save = |slot: String, content: Value| {
        db.execute(
            "INSERT INTO music_snapshot (provider, profile_id, region, slot, payload, saved_at_ms, encoded_bytes)
             VALUES ('spotify', '', ?1, ?2, ?3, unixepoch() * 1000, length(CAST(?3 AS BLOB)))",
            params![region, slot, content.to_string()],
        )
    };
    let album = spotify_id(ObjectKind::Album, ALBUM_ID);
    let album_url = format!("https://open.spotify.com/album/{ALBUM_ID}");
    let credits: Vec<_> = ARTISTS
        .iter()
        .map(|(id, name)| ArtistCredit {
            name: (*name).to_owned(),
            reference: Some(spotify_id(ObjectKind::Artist, id)),
        })
        .collect();
    let track = PlaylistTrack {
        id: "navigation-local-fixture".into(),
        title: TRACK.into(),
        artist: ARTISTS.map(|(_, name)| name).join(", "),
        artists: ARTISTS.map(|(_, name)| name.to_owned()).to_vec(),
        artist_credits: credits.clone(),
        album_ref: Some(album.clone()),
        album: Some(ALBUM.into()),
        artwork: Some(COVER.into()),
        added: None,
        duration: Some(60),
        media: Some(Media::File(profile.join("track-0.wav"))),
    };
    save(
        format!("album:{ALBUM_ID}"),
        json!({
            "reference": album, "title": ALBUM, "artwork": COVER, "url": album_url,
            "artists": [ARTISTS[0].1, ARTISTS[1].1], "artist_credits": credits,
            "tracks": [track], "releases": [], "single": false
        }),
    )?;
    for (id, name) in ARTISTS {
        save(
            format!("artist:{id}"),
            json!({
                "reference": spotify_id(ObjectKind::Artist, id), "title": name,
                "artists": [], "tracks": [track], "single": false,
                "releases": [{"id": format!("spotify:album:{ALBUM_ID}"), "title": ALBUM,
                    "subtitle": name, "artwork": COVER, "url": album_url}]
            }),
        )?;
    }
    let related_album = spotify_id(ObjectKind::Album, RELATED_ALBUM);
    let related_tracks: Vec<_> = RELEASE_TRACKS
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let id = release_track_id(index);
            PlaylistTrack {
                id: id.clone(),
                title: (*title).to_owned(),
                album_ref: Some(related_album.clone()),
                album: Some("Линия прилива".to_owned()),
                media: Some(Media::Provider {
                    provider: ProviderId::Spotify,
                    id,
                }),
                ..track.clone()
            }
        })
        .collect();
    save(
        format!("album:{RELATED_ALBUM}"),
        json!({
            "reference": related_album, "title": "Линия прилива", "artists": [],
            "tracks": related_tracks, "releases": [], "single": false
        }),
    )?;
    for track in &related_tracks[4..] {
        let reference = spotify_id(ObjectKind::Track, &track.id);
        save(
            format!("track:{}", track.id),
            json!({
                "reference": reference, "title": track.title, "artists": [],
                "album_ref": related_album, "album": track.album, "isrc": "USAAA2600055",
                "url": format!("https://open.spotify.com/track/{}", track.id),
                "tracks": [track], "releases": [], "single": false
            }),
        )?;
    }
    Ok(())
}

pub(super) async fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, window, cx| {
        view.show_music_collection_for_capture(false, window, cx)
    })?;
    let library_playback = super::library_row::verify(view, scene, output, cx).await?;
    let local = library_playback["track"]
        .as_str()
        .ok_or_else(|| io::Error::other("Проверка строки не вернула название трека."))?
        .to_owned();
    let before = &library_playback["playback_after_navigation"];
    frame(raw, scene, output, "local-track", cx)?;
    press(raw, &named("workspace-back"), cx)?;
    press(raw, &named("player-track"), cx)?;
    wait_for_title(raw, &local, cx).await?;
    let history_playback = check_history_cover(view, scene, output, &local, cx).await?;

    let started = Instant::now();
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(
            &format!("https://open.spotify.com/album/{ALBUM_ID}"),
            window,
            cx,
        )
    })?;
    wait_for_title(raw, ALBUM, cx).await?;
    let cold_ms = started.elapsed().as_millis();
    let album_tree = frame(raw, scene, output, "release-links", cx)?;
    for (index, (_, name)) in ARTISTS.into_iter().enumerate() {
        let id = format!("music-object-artist-{index}");
        require(
            node(&album_tree, &id).is_some_and(|node| node["aria"]["role"] == "Link"),
            "Каждому исполнителю нужна собственная ссылка.",
        )?;
        press(raw, &named(&id), cx)?;
        wait_for_title(raw, name, cx).await?;
        wait(
            cx,
            "дискография артиста из снимка",
            |cx| {
                let ready = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
                Ok(
                    (ready["details"]["title"] == name && ready["details"]["releases"] == 1)
                        .then_some(()),
                )
            },
        )
        .await?;
        focus(raw, &named("music-track-cover-play-0"), cx)?;
        let artist = frame(raw, scene, output, &format!("artist-{index}"), cx)?;
        require(
            node_label(&artist, "music-track-cover-play-0")
                .is_some_and(|label| label == format!("Слушать · {TRACK}")),
            "В списке артиста у обложки есть команда воспроизведения нужного трека.",
        )?;
        press(raw, &named("workspace-back"), cx)?;
        wait_for_title(raw, ALBUM, cx).await?;
    }

    press(raw, &named("music-track-title-0"), cx)?;
    wait_for_release_selection(view, raw, ALBUM, TRACK, 1, cx).await?;
    press(raw, &named("music-object-lyrics"), cx)?;
    let inspection = wait(
        cx,
        "текст открытого трека без смены звука",
        |cx| {
            let state = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
            Ok((state["lyrics_track"] == "navigation-local-fixture"
                && state["lines"] == 2
                && state["current_track"] != state["lyrics_track"]
                && state["playing"] == true
                && state["follows_playback"] == false
                && state["seekable"] == false)
                .then_some(state))
        },
    )
    .await?;
    frame(raw, scene, output, "track-lyrics-inspection", cx)?;
    press(raw, &named("right-panel-close"), cx)?;
    require(
        focused(&snapshot(raw, cx)?) == Some(named("music-object-lyrics").as_str()),
        "Закрытие текста должно вернуть фокус кнопке страницы трека.",
    )?;
    let started = Instant::now();
    press(raw, &named("workspace-back"), cx)?;
    wait_for_album(view, raw, cx).await?;
    let warm_ms = started.elapsed().as_millis();
    let after = sound(view, &local, cx).await?;
    let inspector = check_queue_and_inspector(view, scene, output, cx).await?;
    let soak = soak_navigation(view, raw, cx).await?;
    let related_tracks = check_release_selection(view, scene, output, cx).await?;
    let search_playback = check_search_cover(view, scene, output, &local, cx).await?;
    fs::write(
        output.join("entity-navigation.json"),
        serde_json::to_vec_pretty(&json!({
            "album": ALBUM, "artists": ARTISTS.map(|(_, name)| name),
            "local_track": local, "playback_before": before, "playback_after": after,
            "library_row_playback": library_playback,
            "history_cover_playback": history_playback,
            "search_cover_playback": search_playback,
            "queue_and_inspector": inspector,
            "lyrics_inspection": inspection,
            "release_tracks": related_tracks,
            "soak": soak,
            "first_offline_open_ms_including_a11y_sampling": cold_ms,
            "warm_open_ms_including_a11y_sampling": warm_ms
        }))?,
    )?;
    Ok(())
}

async fn check_search_cover(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    title: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(title, window, cx)
    })?;
    let label = format!("Слушать · {title}");
    let play = wait(
        cx,
        "кнопка на обложке результата поиска",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(tree["nodes"].as_object().and_then(|nodes| {
                nodes.values().find_map(|node| {
                    let id = node["element_id"].as_str()?;
                    (id.starts_with("Name(\"music-search-saved-play-")
                        && node["aria"]["role"] == "Button"
                        && node["aria"]["label"] == label)
                        .then(|| id.to_owned())
                })
            }))
        },
    )
    .await?;
    focus(raw, &play, cx)?;
    frame(raw, scene, output, "search-cover-focus", cx)?;
    key(raw, "enter", cx)?;
    let playback = sound(view, title, cx).await?;
    require(
        node(&snapshot(raw, cx)?, "music-catalog-search").is_some(),
        "Воспроизведение с обложки сохраняет результаты поиска.",
    )?;
    Ok(playback)
}

async fn check_history_cover(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    title: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(view);
    view.update(cx, |view, window, cx| {
        view.open_history_for_capture(window, cx)
    })?;
    let outside = point(px(3.), px(3.));
    events(raw, [mouse_move(outside, false)], cx)?;
    let label = format!("Слушать снова · {title}");
    let cover_id = wait(
        cx,
        "кнопка воспроизведения на обложке в истории",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(tree["nodes"].as_object().and_then(|nodes| {
                nodes.values().find_map(|node| {
                    let id = node["element_id"].as_str()?;
                    let id = id
                        .strip_prefix("Name(\"history-cover-play-")?
                        .strip_suffix("\")")?
                        .parse::<i64>()
                        .ok()?;
                    (node["aria"]["role"] == "Button" && node["aria"]["label"] == label)
                        .then_some(id)
                })
            }))
        },
    )
    .await?;
    let cover_name = named(&format!("history-cover-play-{cover_id}"));
    let idle = super::super::render_image(raw, scene, cx)?;
    idle.save(output.join("history-cover-idle.png"))?;
    let bounds = view
        .update(cx, |view, window, _| {
            view.history_cover_bounds_for_capture(cover_id, window)
        })?
        .ok_or_else(|| io::Error::other("Отсутствуют границы обложки в истории."))?;
    let title_point = point(bounds.right() + px(16.), bounds.center().y);
    events(raw, [mouse_move(title_point, false)], cx)?;
    let hovered = wait(
        cx,
        "Play виден поверх обложки при наведении на строку",
        |cx| {
            let frame = super::super::render_image(raw, scene, cx)?;
            let pixels = added_play_pixels(&idle, &frame, bounds, scene);
            Ok((pixels >= 8).then_some((frame, pixels)))
        },
    )
    .await?;
    hovered.0.save(output.join("history-cover-hover.png"))?;
    require(
        view.update(cx, |view, window, _| {
            view.history_cover_bounds_for_capture(cover_id, window)
        })? == Some(bounds),
        "Наведение сохраняет положение и размер обложки в истории.",
    )?;
    events(raw, [mouse_move(outside, false)], cx)?;
    focus(raw, &cover_name, cx)?;
    let keyboard = super::super::render_image(raw, scene, cx)?;
    let keyboard_pixels = added_play_pixels(&idle, &keyboard, bounds, scene);
    require(
        keyboard_pixels >= 8,
        "Play виден на обложке в истории при клавиатурном фокусе.",
    )?;
    keyboard.save(output.join("history-cover-focus.png"))?;
    pause(view, raw, cx).await?;
    click(raw, bounds.center(), cx)?;
    let playback = sound(view, title, cx).await?;
    require(
        node(&snapshot(raw, cx)?, "history-list").is_some(),
        "Нажатие на обложку воспроизводит запись и сохраняет экран истории.",
    )?;

    let title_link = wait(
        cx,
        "ссылка в названии записи в истории",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(find_link(&tree, "history-track-", title))
        },
    )
    .await?;
    frame(raw, scene, output, "history-link", cx)?;
    press(raw, &title_link, cx)?;
    wait_for_title(raw, title, cx).await?;
    Ok(json!({
        "hover_play_pixels": hovered.1,
        "keyboard_play_pixels": keyboard_pixels,
        "cover_footprint_unchanged": true,
        "cover_playback": playback,
        "title_link_opens_track": true
    }))
}

async fn check_release_selection(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(view);
    let before = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    let address = format!("https://open.spotify.com/track/{}", release_track_id(5));
    view.update(cx, |view, window, cx| {
        view.submit_music_url_for_capture(&address, window, cx)
    })?;
    let album = "Линия прилива";
    wait_for_release_selection(
        view,
        raw,
        album,
        RELEASE_TRACKS[5],
        RELEASE_TRACKS.len(),
        cx,
    )
    .await?;
    let tree = frame(raw, scene, output, "release-selected-track", cx)?;
    require(
        node(&tree, "track-release-tracks").is_none(),
        "Отдельный список треков релиза не нужен поверх полного треклиста.",
    )?;
    if scene.height <= 400. {
        wait(
            cx,
            "автоматическая прокрутка к последнему треку",
            |cx| {
                let offset = view.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
                Ok((offset < -1.).then_some(()))
            },
        )
        .await?;
    }
    let selected_offset = view.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
    press(raw, &named("release-track-info"), cx)?;
    if scene.height <= 400. {
        wait(
            cx,
            "видимое начало сведений о выбранном треке",
            |cx| {
                snapshot(raw, cx)?;
                let offset = view.update(cx, |view, _, _| view.home_scroll_offset_for_capture())?;
                Ok((offset < selected_offset - 1.).then_some(()))
            },
        )
        .await?;
    }
    let metadata = frame(raw, scene, output, "release-track-metadata", cx)?;
    require(
        node(&metadata, "release-track-metadata").is_some()
            && focused(&metadata) == Some(named("release-track-info").as_str()),
        "Сведения открываются у выбранной строки, кнопка сохраняет клавиатурный фокус.",
    )?;
    press(raw, &named("release-track-info"), cx)?;
    press(raw, &named("music-track-title-4"), cx)?;
    wait_for_release_selection(
        view,
        raw,
        album,
        RELEASE_TRACKS[4],
        RELEASE_TRACKS.len(),
        cx,
    )
    .await?;
    let after = wait(cx, "страница соседнего трека", |cx| {
        let model = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
        let playback = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
        Ok((model["details"]["title"] == RELEASE_TRACKS[4]
            && playback["current_track"] == before["current_track"]
            && playback["playing"] == before["playing"])
            .then_some(playback))
    })
    .await?;
    frame(raw, scene, output, "release-opened-sibling", cx)?;
    press(raw, &named("workspace-back"), cx)?;
    wait_for_release_selection(
        view,
        raw,
        album,
        RELEASE_TRACKS[5],
        RELEASE_TRACKS.len(),
        cx,
    )
    .await?;
    frame(raw, scene, output, "release-back-to-selected-track", cx)?;
    Ok(
        json!({"selected": RELEASE_TRACKS[5], "full_tracklist": RELEASE_TRACKS.len(), "selected_scroll_offset_dip": selected_offset,
            "followed": RELEASE_TRACKS[4], "playback_kept": after["current_track"]}),
    )
}

async fn check_queue_and_inspector(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(view);
    focus(raw, &named("music-track-cover-play-0"), cx)?;
    frame(raw, scene, output, "album-cover-focus", cx)?;
    key(raw, "enter", cx)?;
    let before = sound(view, TRACK, cx).await?;
    focus(raw, &named("player-track"), cx)?;
    key(raw, "shift-f10", cx)?;
    let menu = frame(raw, scene, output, "player-context", cx)?;
    require(
        menu["nodes"]
            .as_object()
            .into_iter()
            .flat_map(|nodes| nodes.values())
            .any(|node| {
                node["aria"]["role"] == "MenuItem"
                    && node["aria"]["label"] == "Открыть страницу трека"
            }),
        "Контекстное меню плеера открывает страницу трека.",
    )?;
    key(raw, "escape", cx)?;
    press(raw, &named("player-track"), cx)?;
    wait_for_release_selection(view, raw, ALBUM, TRACK, 1, cx).await?;
    press(raw, &named("workspace-back"), cx)?;
    wait_for_album(view, raw, cx).await?;
    press(raw, &named("player-queue"), cx)?;
    let track = wait(cx, "ссылка трека в очереди", |cx| {
        let tree = snapshot(raw, cx)?;
        Ok(tree["nodes"]
            .as_object()
            .into_iter()
            .flat_map(|nodes| nodes.values())
            .find_map(|node| {
                let id = node["element_id"].as_str()?;
                (id.starts_with("Name(\"queue-track-") && node["aria"]["role"] == "Link")
                    .then(|| id.to_owned())
            }))
    })
    .await?;
    let play = track.replace("queue-track-", "queue-cover-play-");
    focus(raw, &play, cx)?;
    frame(raw, scene, output, "queue-cover-focus", cx)?;
    key(raw, "enter", cx)?;
    let queue_playback = sound(view, TRACK, cx).await?;
    frame(raw, scene, output, "queue-link", cx)?;
    press(raw, &track, cx)?;
    wait_for_release_selection(view, raw, ALBUM, TRACK, 1, cx).await?;
    if node(&snapshot(raw, cx)?, "right-panel-track").is_none() {
        press(raw, &named("player-queue"), cx)?;
    }
    press(raw, &named("right-panel-track"), cx)?;
    let tree = frame(raw, scene, output, "inspector-link", cx)?;
    require(
        node(&tree, "playlist-inspector-artwork-link")
            .is_some_and(|node| node["aria"]["role"] == "Link"),
        "Обложка инспектора должна открывать известный альбом.",
    )?;
    press(raw, &named("playlist-inspector-artwork-link"), cx)?;
    wait_for_title(raw, ALBUM, cx).await?;
    let panel = view.update(cx, |view, _, _| view.panel_state_for_capture())?;
    require(
        panel == (true, "track"),
        "Инспектор сохранил вкладку при переходе к альбому.",
    )?;
    let after = sound(view, TRACK, cx).await?;
    frame(raw, scene, output, "album-after-inspector", cx)?;
    Ok(
        json!({"preserved_panel": panel, "before": before, "queue_cover": queue_playback, "after": after}),
    )
}

async fn wait_for_release_selection(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    album: &str,
    track: &str,
    count: usize,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait(
        cx,
        "выбранная запись на странице её релиза",
        |cx| {
            let model = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
            let tree = snapshot(raw, cx)?;
            Ok((node_label(&tree, "music-object-title") == Some(album)
                && node_label(&tree, "release-selected-track")
                    == Some(format!("Выбранный трек · {track}").as_str())
                && model["release_selection"]["track"] == track
                && model["release_selection"]["tracks"] == count)
                .then_some(()))
        },
    )
    .await
}

async fn wait_for_title(raw: AnyWindowHandle, title: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    wait(cx, title, |cx| {
        let tree = snapshot(raw, cx)?;
        Ok((node_label(&tree, "music-object-title") == Some(title)).then_some(()))
    })
    .await
}

fn find_link(tree: &Value, prefix: &str, name: &str) -> Option<String> {
    tree["nodes"].as_object()?.values().find_map(|node| {
        let id = node["element_id"].as_str()?;
        (id.strip_prefix("Name(\"")?.starts_with(prefix)
            && node["aria"]["role"] == "Link"
            && node["aria"]["label"]
                .as_str()
                .is_some_and(|label| label.ends_with(name)))
        .then(|| id.to_owned())
    })
}

async fn wait_for_album(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait(cx, ALBUM, |cx| {
        let model = view.update(cx, |view, _, _| view.music_semantics_for_capture())?;
        let tree = snapshot(raw, cx)?;
        Ok((node_label(&tree, "music-object-title") == Some(ALBUM)
            && model["details"]["tracks"] == 1)
            .then_some(()))
    })
    .await
}

async fn soak_navigation(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> gpui::Result<Option<Value>> {
    let Some(seconds) = env::var("MEDIADEKA_ENTITY_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    else {
        return Ok(None);
    };
    let started = Instant::now();
    let mut cycles = 0;
    while started.elapsed() < Duration::from_secs(seconds) {
        press(raw, &named("music-object-artist-0"), cx)?;
        wait_for_title(raw, ARTISTS[0].1, cx).await?;
        press(raw, &named("workspace-back"), cx)?;
        wait_for_album(view, raw, cx).await?;
        cycles += 1;
    }
    Ok(Some(
        json!({"elapsed_ms": started.elapsed().as_millis(), "cycles": cycles}),
    ))
}
