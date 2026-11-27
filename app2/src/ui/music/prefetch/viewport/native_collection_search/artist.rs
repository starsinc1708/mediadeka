use std::ops::Range;

use gpui::{KeyUpEvent, Keystroke};

use super::*;
use crate::{music::ObjectRef, ui::workspace::MusicTarget};

const ORIGIN: usize = 34_000;
pub(super) const ARTISTS: Range<usize> = ORIGIN..ORIGIN + 7;
const RELEASES: Range<usize> = 35_000..35_018;

pub(super) fn brief(id: usize) -> Value {
    let artist = |id| json!({"id": id, "name": format!("Артист {id}")});
    let mut result = json!({"artist": artist(id)});
    if id == ORIGIN {
        result["similarArtists"] = json!((ORIGIN + 1..ARTISTS.end).map(artist).collect::<Vec<_>>());
        result["albums"] = json!(RELEASES.map(release).collect::<Vec<_>>());
        result["popularTracks"] = json!(
            (36_000..36_006)
                .map(|id| json!({"id":id,"title":format!("Трек {id}"),"durationMs":180_000}))
                .collect::<Vec<_>>()
        );
    }
    json!({"result": result})
}

pub(super) fn release(id: usize) -> Value {
    let mut album = album(id);
    if RELEASES.contains(&id) && !id.is_multiple_of(2) {
        album["type"] = json!("single");
    }
    album
}

pub(super) async fn check(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    evidence: &Path,
    requests: &Mutex<Vec<usize>>,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let start = requests.lock().unwrap().len();
    dispatch(
        raw,
        MouseMoveEvent {
            position: point(px(330.), px(22.)),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    )?;
    window.update(cx, |view, window, cx| {
        view.open_music_target(
            MusicTarget {
                object: ObjectRef::native(PROVIDER, ObjectKind::Artist, ORIGIN.to_string())
                    .unwrap(),
                profile: Some(PROFILE.into()),
                title: format!("Артист {ORIGIN}"),
                artwork: None,
            },
            window,
            cx,
        );
        assert!(Sample::read(view).visible.is_empty());
    })?;
    wait_for_page(window, cx, ORIGIN).await?;
    capture(raw, cx, evidence, "artist-initial")?;
    cx.background_executor()
        .timer(Duration::from_millis(220))
        .await;
    assert_eq!(requests.lock().unwrap()[start..], [ORIGIN]);

    let in_related = |id: &usize| *id != ORIGIN && ARTISTS.contains(id);
    let related_offset = seek(window, cx, 0, in_related)?;
    let related = wait(window, cx, requests, |sample| {
        sample.visible.iter().all(in_related)
    })
    .await?;
    capture(raw, cx, evidence, "artist-related")?;

    let release_offset = seek(window, cx, related_offset, |id| RELEASES.contains(id))?;
    let releases = wait(window, cx, requests, |sample| {
        sample.visible.iter().all(|id| RELEASES.contains(id))
    })
    .await?;
    capture(raw, cx, evidence, "artist-releases")?;

    activate(window, "Синглы", cx)?;
    let cold_single = |id: &usize| {
        RELEASES.contains(id) && !id.is_multiple_of(2) && !releases.visible.contains(id)
    };
    // Reach a row of singles whose pages were outside the previous viewport.
    seek(window, cx, release_offset, cold_single)?;
    let singles = wait(window, cx, requests, |sample| {
        sample.visible.iter().all(cold_single)
    })
    .await?;
    capture(raw, cx, evidence, "artist-singles")?;
    let requested = requests.lock().unwrap()[start..].to_vec();
    let expected = std::iter::once(ORIGIN)
        .chain(
            [&related, &releases, &singles]
                .into_iter()
                .flat_map(|sample| sample.visible.iter().copied()),
        )
        .collect::<BTreeSet<_>>();
    assert_eq!(requested.iter().copied().collect::<BTreeSet<_>>(), expected);
    assert_eq!(requested.len(), expected.len());
    assert!(window.update(cx, |view, _, _| Sample::read(view).nearby.is_empty())?);

    let target = singles.visible[0];
    activate(window, &format!("Альбом {target}"), cx)?;
    wait_for_page(window, cx, target).await?;
    capture(raw, cx, evidence, "artist-single-opened")?;
    assert_eq!(requests.lock().unwrap()[start..], requested);
    let report = format!(
        "Related at {related_offset} DIP: {related:?}\nReleases at {release_offset} DIP: {releases:?}\nSingles: {singles:?}\nHTTP object ids: {requested:?}\nOpened {target} with no extra HTTP request\n",
    );
    print!("{report}");
    fs::write(evidence.join("artist-viewport.txt"), report)?;
    Ok(())
}

fn seek(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    start: usize,
    matches: impl Fn(&usize) -> bool,
) -> gpui::Result<usize> {
    let raw = AnyWindowHandle::from(window);
    let end = window.update(cx, |view, _, _| {
        view.home_scroll_extent_for_capture().ceil() as usize
    })?;
    for offset in (start..=end).step_by(22).chain(std::iter::once(end)) {
        window.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(offset as f32, cx);
        })?;
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        let matched = window.update(cx, |view, _, _| {
            let sample = view.music.prefetch.viewport.frame.borrow();
            sample.previous.len() >= 2
                && sample
                    .previous
                    .iter()
                    .all(|key| key.object.id.parse().ok().is_some_and(|id| matches(&id)))
        })?;
        if matched {
            return Ok(offset);
        }
    }
    Err(io::Error::other(format!(
        "no matching artist viewport between {start} and {end} DIP"
    ))
    .into())
}

pub(super) async fn wait_for_page(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    id: usize,
) -> gpui::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(6);
    let id = id.to_string();
    let raw = AnyWindowHandle::from(window);
    loop {
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        if window.update(cx, |view, _, _| {
            view.music
                .object
                .as_ref()
                .and_then(|detail| detail.data.as_ref())
                .is_some_and(|data| {
                    data.page.reference.id == id && data.profile.as_deref() == Some(PROFILE)
                })
        })? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("catalog fixture did not load {id}")).into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
    }
}

pub(super) fn activate(
    window: WindowHandle<MainView>,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        let press =
            |key: &str, window: &mut gpui::Window, cx: &mut gpui::App| -> gpui::Result<()> {
                let keystroke = Keystroke::parse(key)?;
                window.dispatch_keystroke(keystroke.clone(), cx);
                window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
                Ok(())
            };
        for _ in 0..100 {
            window.draw(cx).clear(cx);
            let tree = window
                .debug_a11y_tree_json()
                .ok_or_else(|| io::Error::other("missing accessibility tree"))?;
            let tree: Value = serde_json::from_str(&tree)?;
            let focus = tree["gpui_focus"].as_str().unwrap_or_default();
            if tree["nodes"][focus]["aria"]["label"] == name {
                return press("enter", window, cx);
            }
            press("tab", window, cx)?;
        }
        Err(io::Error::other(format!("keyboard could not activate {name}")).into())
    })?
}
