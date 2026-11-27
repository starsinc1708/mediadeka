use std::process::Command;

use gpui::{InputEvent, KeyUpEvent, Keystroke};

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef, region},
    playlist::{Playlist, PlaylistOwner},
    ui::workspace::Route,
};

use super::*;

const CHILD: &str = "MEDIADEKA_NATIVE_PLAYLIST_OWNER_CHILD";
const EVIDENCE: &str = "MEDIADEKA_NATIVE_PLAYLIST_OWNER_EVIDENCE";
const TEST: &str = "ui::playlist::loading::tests::owner::verified_playlist_owner_uses_native_route";
const PROVIDER: ProviderId = ProviderId::YouTubeMusic;
const ARTIST: &str = "UCPlaylistComposer";
const PLAYLIST: &str = "PLAuthorFixture";
const LEGACY: &str = "PLAuthorLegacyFixture";
const NAME: &str = "Verified playlist author";

#[test]
#[ignore = "captures a native GPUI author link and opens a previously cached artist page"]
fn verified_playlist_owner_uses_native_route() {
    if env::var_os(CHILD).is_none() {
        for mode in ["1920", "800", "800-150"] {
            let profile = tempfile::tempdir().unwrap();
            assert!(
                Command::new(env::current_exe().unwrap())
                    .args(["--exact", TEST, "--ignored", "--nocapture"])
                    .env(CHILD, mode)
                    .env("MEDIADEKA_PROFILE_DIR", profile.path())
                    .env("MEDIADEKA_GUI", "3")
                    .status()
                    .unwrap()
                    .success()
            );
        }
        return;
    }

    let store = Store::open_default().unwrap();
    let region = region();
    let artist = ObjectRef::native(PROVIDER, ObjectKind::Artist, ARTIST.into()).unwrap();
    let page = MusicPage::new(artist.clone(), NAME.into());
    assert!(
        store
            .save_music_snapshot(PROVIDER, None, &region, &format!("artist:{ARTIST}"), &page)
            .unwrap()
    );
    for (id, owner) in [
        (
            PLAYLIST,
            PlaylistOwner::Profile {
                name: NAME.into(),
                reference: artist.clone(),
            },
        ),
        (LEGACY, PlaylistOwner::Named(NAME.into())),
    ] {
        let playlist = Playlist {
            name: "Playlist with author".into(),
            owner: Some(owner),
            artwork: None,
            link: None,
            tracks: Vec::new(),
        };
        assert!(
            store
                .save_music_snapshot(
                    PROVIDER,
                    None,
                    &region,
                    &format!("playlist:{id}"),
                    &playlist
                )
                .unwrap()
        );
    }
    let mode = env::var(CHILD).unwrap();
    let outcome = Arc::new(Mutex::new(None));
    let result = Arc::clone(&outcome);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            *result.lock().unwrap() = Some(verify(&mode, &artist, cx).await);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    outcome.lock().unwrap().take().unwrap().unwrap();
}

async fn verify(mode: &str, artist: &ObjectRef, cx: &mut AsyncApp) -> gpui::Result<()> {
    let compact = mode.starts_with("800");
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: if compact { 800. } else { 1_920. },
                height: if compact { 400. } else { 1_080. },
                reduced_motion: true,
            },
        )
    })?;
    if mode == "800-150" {
        paging::apply_compact_dpi(window, cx)?;
    }
    paging::enable_accessibility(window, cx)?;
    open(window, PLAYLIST, cx).await?;
    let initial = capture(window, cx, mode, "verified")?;
    if mode == "800-150" {
        check(initial["frame"]["scale_factor"] == 1.5, "Expected 150% DPI")?;
    }
    let id = "Name(\"playlist-owner\")";
    let expected = format!("Открыть · Артист · {NAME}");
    check(
        initial["nodes"].as_object().is_some_and(|nodes| {
            nodes.values().any(|node| {
                node["element_id"] == id
                    && node["aria"]["role"] == "Link"
                    && node["aria"]["label"] == expected
            })
        }),
        "The verified author is missing from the accessibility tree",
    )?;
    let count = initial["frame"]["tab_stop_count"].as_u64().unwrap_or(64);
    for _ in 0..=count {
        let current = tree(window, cx)?;
        let focus = current["gpui_focus"].as_str().unwrap_or_default();
        if current["nodes"][focus]["element_id"] == id {
            break;
        }
        key(window, "tab", cx)?;
    }
    let focused = capture(window, cx, mode, "focused")?;
    let focus = focused["gpui_focus"].as_str().unwrap_or_default();
    check(
        focused["nodes"][focus]["element_id"] == id,
        "Keyboard focus skipped author",
    )?;
    key(window, "enter", cx)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if window.update(cx, |view, _, _|
            matches!(view.workspace.routes.current(), Route::Object(target) if &target.object == artist)
                && view.music_semantics_for_capture()["details"]["title"] == NAME
        )? {
            break;
        }
        check(
            Instant::now() < deadline,
            "Activating the author did not open its exact native page",
        )?;
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    capture(window, cx, mode, "artist")?;
    window.update(cx, |view, window, cx| {
        view.playlist_history_for_capture(false, window, cx)
    })?;
    prefetch::wait_for_playlist(window, cx, |open| open.complete_snapshot().is_some()).await?;
    check(window.update(cx, |view, _, _| matches!(view.workspace.routes.current(),
        Route::Playlist(target) if matches!(&target.key, PlaylistKey::Source { id, .. } if id == PLAYLIST)
    ))?, "History did not return to the originating playlist")?;
    open(window, LEGACY, cx).await?;
    let label = capture(window, cx, mode, "legacy")?;
    check(
        !label["nodes"].as_object().is_some_and(|nodes| {
            nodes
                .values()
                .any(|node| node["aria"]["role"] == "Link" && node["aria"]["label"] == expected)
        }),
        "A legacy name was turned into an author link",
    )?;
    println!(
        "{mode}: verified author Link activated with Enter, exact profile loaded, Back restored playlist, legacy author stayed plain"
    );
    Ok(())
}

async fn open(window: WindowHandle<MainView>, id: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    window.update(cx, |view, window, cx| {
        view.open_saved_playlist_for_capture(
            PROVIDER,
            id.into(),
            "Playlist with author".into(),
            window,
            cx,
        )
    })??;
    prefetch::wait_for_playlist(window, cx, |open| open.complete_snapshot().is_some()).await
}

fn key(window: WindowHandle<MainView>, input: &str, cx: &mut AsyncApp) -> gpui::Result<()> {
    let key = Keystroke::parse(input)?;
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.dispatch_keystroke(key.clone(), cx);
        window.dispatch_event(KeyUpEvent { keystroke: key }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
    })
}

fn tree(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let tree = AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        window
            .debug_a11y_tree_json()
            .ok_or_else(|| io::Error::other("Missing accessibility tree"))
    })??;
    Ok(serde_json::from_str(&tree)?)
}

fn capture(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
    mode: &str,
    stem: &str,
) -> gpui::Result<Value> {
    let tree = tree(window, cx)?;
    if let Some(path) = env::var_os(EVIDENCE) {
        let evidence = PathBuf::from(path).join(mode);
        std::fs::create_dir_all(&evidence)?;
        frame(window, cx, &evidence.join(format!("{stem}.png")))?;
        std::fs::write(
            evidence.join(format!("{stem}-a11y.json")),
            serde_json::to_vec_pretty(&tree)?,
        )?;
    }
    Ok(tree)
}
