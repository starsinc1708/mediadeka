use std::ops::Range;

use super::*;
use crate::{
    playback::{CollectionPlan, CollectionProgressUpdate, SoundSource},
    playlist::{Media, PlaylistKey, PlaylistTrack},
};

pub(super) const HISTORY: Range<usize> = 37_000..37_044;
pub(super) const CONTINUE: usize = 38_001;
pub(super) const CONTINUE_ID: &str = "74369:38001";
pub(super) const CONTINUE_PATH: &str = "/users/74369/playlists/38001";
pub(super) const CONTINUE_TITLE: &str = "Незаконченный плейлист";
const RECENT: usize = 38_110;
const RECENT_TITLE: &str = "Прослушанный альбом";

fn track_name(id: usize) -> String {
    format!("Запись {id}")
}

pub(super) fn track_data(id: usize) -> Value {
    json!({"id": id, "title": track_name(id), "durationMs": 180_000})
}

fn remote_key(id: String) -> PlaylistKey {
    PlaylistKey::Source {
        provider: PROVIDER,
        profile: Some(PROFILE.into()),
        id,
    }
}

fn track(id: usize) -> PlaylistTrack {
    PlaylistTrack {
        id: id.to_string(),
        title: track_name(id),
        artist: String::new(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(180),
        media: Some(Media::Provider {
            provider: PROVIDER,
            id: id.to_string(),
        }),
    }
}

pub(super) fn seed(store: &Store) {
    let playlist = remote_key(CONTINUE_ID.into());
    for id in HISTORY {
        store
            .record_history(&playlist, &track(id), SoundSource::Stream(PROVIDER))
            .unwrap();
    }
    for (key, title, completed) in [
        (remote_key(format!("album:{RECENT}")), RECENT_TITLE, vec![0]),
        (playlist, CONTINUE_TITLE, Vec::new()),
    ] {
        let plan = CollectionPlan::new(key, title, None, [&track(HISTORY.start)]);
        let progress = serde_json::from_value(json!({
            "index": 0,
            "position_ms": 20_000,
            "duration_ms": 180_000,
            "total": 1,
            "completed": completed,
        }))
        .unwrap();
        store
            .save_collection_listening(
                &CollectionProgressUpdate {
                    plan: Arc::new(plan),
                    progress,
                },
                true,
            )
            .unwrap();
    }
}

pub(super) async fn check(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    requests: &Mutex<Vec<usize>>,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let start = requests.lock().unwrap().len();
    window.update(cx, |view, window, cx| {
        view.show_music_page(Page::History, window, cx)
    })?;
    let recent = wait(window, cx, requests, |sample| {
        sample
            .visible
            .iter()
            .all(|id| (HISTORY.end - 9..HISTORY.end).contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "listening-history-initial")?;
    scroll(raw, -340., cx)?;
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let intermediate = window.update(cx, |view, _, _| {
        view.music
            .prefetch
            .viewport
            .frame
            .borrow()
            .previous
            .iter()
            .map(|key| key.object.id.parse::<usize>().unwrap())
            .collect::<Vec<_>>()
    })?;
    assert!(!intermediate.is_empty());
    scroll(raw, -395., cx)?;
    dispatch(
        raw,
        MouseMoveEvent {
            position: point(px(328.), px(22.)),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )?;
    let older = wait(window, cx, requests, |sample| {
        sample
            .visible
            .iter()
            .all(|id| HISTORY.contains(id) && !recent.visible.contains(id))
            && sample.nearby.len() == 2
    })
    .await?;
    assert!(
        older
            .nearby
            .iter()
            .all(|id| *id < *older.visible.iter().min().unwrap())
    );
    assert!(
        intermediate
            .iter()
            .all(|id| !requests.lock().unwrap().contains(id)),
        "an intermediate viewport started a request"
    );
    capture(raw, cx, evidence, "listening-history-scrolled")?;
    let visited = older.visible[0];
    let before = requests.lock().unwrap().clone();
    artist::activate(
        window,
        &format!("Открыть · Трек · {}", track_name(visited)),
        cx,
    )?;
    artist::wait_for_page(window, cx, visited).await?;
    assert_eq!(*requests.lock().unwrap(), before);
    assert!(window.update(cx, |view, _, _| view.player_snapshot.queue_len == 0)?);
    capture(raw, cx, evidence, "listening-history-opened")?;

    window.update(cx, |view, window, cx| {
        view.show_music_page(Page::Home, window, cx)
    })?;
    let featured = wait(window, cx, requests, |sample| {
        sample.visible.contains(&CONTINUE)
    })
    .await?;
    capture(raw, cx, evidence, "listening-home-continue")?;
    artist::activate(window, &format!("Открыть · {CONTINUE_TITLE}"), cx)?;
    let deadline = Instant::now() + Duration::from_secs(6);
    while window.update(cx, |view, _, _| view.playlist_status_for_capture())? != Some(Ok(1)) {
        if Instant::now() >= deadline {
            return Err(io::Error::other("prepared continuation playlist did not open").into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
    assert!(window.update(cx, |view, _, _| view.player_snapshot.queue_len == 0)?);
    capture(raw, cx, evidence, "listening-continue-opened")?;

    window.update(cx, |view, window, cx| {
        view.playlist_history_for_capture(false, window, cx)
    })?;
    let recent_offset = seek_recent(window, cx)?;
    let home_recent = wait(window, cx, requests, |sample| {
        sample.visible.contains(&RECENT)
    })
    .await?;
    assert!(home_recent.visible.iter().any(|id| HISTORY.contains(id)));
    capture(raw, cx, evidence, "listening-home-recent")?;
    let name = format!("Открыть · {RECENT_TITLE}");
    artist::activate(window, &name, cx)?;
    artist::wait_for_page(window, cx, RECENT).await?;
    assert!(window.update(cx, |view, _, _| view.player_snapshot.queue_len == 0)?);

    let expected = [&recent, &older, &featured, &home_recent]
        .into_iter()
        .flat_map(|sample| sample.visible.iter().chain(&sample.nearby).copied())
        .collect::<BTreeSet<_>>();
    let actual = requests.lock().unwrap()[start..].to_vec();
    assert_eq!(actual.iter().copied().collect::<BTreeSet<_>>(), expected);
    assert_eq!(
        actual.len(),
        expected.len(),
        "a warmed recording or collection was requested again"
    );
    let report = format!(
        "History current {recent:?}\nHistory skipped intermediate {intermediate:?}\nHistory scrolled {older:?}\nHome continue {featured:?}\nHome recent at {recent_offset} DIP {home_recent:?}\nHTTP object ids {actual:?}\nKeyboard opened {visited}, {CONTINUE_ID}, {RECENT} without more HTTP or playback\n"
    );
    print!("{report}");
    fs::write(evidence.join("listening-viewport.txt"), report)?;
    Ok(())
}

fn seek_recent(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<usize> {
    let raw = AnyWindowHandle::from(window);
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let end = window.update(cx, |view, _, _| {
        view.home_scroll_extent_for_capture().ceil() as usize
    })?;
    for offset in (0..=end).step_by(20).chain(std::iter::once(end)) {
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(offset as f32, cx)
        })?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        if window.update(cx, |view, _, _| {
            let sample = view.music.prefetch.viewport.frame.borrow();
            sample
                .previous
                .iter()
                .any(|key| key.object.id == RECENT.to_string())
                && sample
                    .previous
                    .iter()
                    .any(|key| key.object.kind == ObjectKind::Track)
        })? {
            return Ok(offset);
        }
    }
    Err(io::Error::other("home viewport did not contain a recent track and release").into())
}
