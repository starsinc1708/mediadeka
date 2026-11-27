use std::{
    env, io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use serde_json::Value;
use url::Url;

use crate::{
    providers::{self, LoginInput},
    storage::Store,
    ui::{CaptureLayout, application, open_capture_window},
};

use super::*;

mod owner;
mod paging;
mod prefetch;

#[test]
#[ignore = "requires an isolated Jellyfin fixture, a playlist with the order A–B–A and libmpv"]
fn jellyfin_refresh_reorders_and_offline_history_keeps_the_snapshot() {
    let server = fixture_var("URL");
    let address = Url::parse(&server).unwrap();
    assert_eq!(address.host_str(), Some("127.0.0.1"));
    assert!(address.port().is_some());
    let name = fixture_var("USER");
    assert_eq!(name, "mediadeka-fixture");
    let session = providers::sign_in(
        ProviderId::Jellyfin,
        LoginInput::JellyfinPassword {
            server: server.clone(),
            username: name,
            password: fixture_var("PASSWORD"),
        },
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let profile = Store::profile_directory().unwrap();
    assert!(env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    assert!(!profile.join("mediadeka.sqlite3").exists());
    let store = Store::open(profile).unwrap();
    store.use_account(&session, || Some(()), || {}).unwrap();
    let profile = session.profile.id;
    let _account = FixtureAccount(&store, &profile);
    let fixture = Fixture {
        server,
        playlist: fixture_var("PLAYLIST"),
        profile: profile.clone(),
        user_id: fixture_var("USER_ID"),
        authorization: fixture_var("AUTHORIZATION"),
        evidence: PathBuf::from(fixture_var("EVIDENCE")),
    };
    let result = Arc::new(Mutex::new(None));
    let completion = Arc::clone(&result);
    application().run(move |cx| {
        cx.spawn(async move |cx| {
            let outcome = fixture.check(cx).await;
            *completion.lock().unwrap() = Some(outcome);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    result.lock().unwrap().take().unwrap().unwrap();
}

fn fixture_var(name: &str) -> String {
    env::var(format!("MEDIADEKA_TEST_JELLYFIN_{name}")).expect("isolated Jellyfin fixture variable")
}

struct FixtureAccount<'a>(&'a Store, &'a str);

impl Drop for FixtureAccount<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .disconnect_account(ProviderId::Jellyfin, self.1, || Some(()));
    }
}

struct Fixture {
    server: String,
    playlist: String,
    profile: String,
    user_id: String,
    authorization: String,
    evidence: PathBuf,
}

impl Fixture {
    async fn check(&self, cx: &mut AsyncApp) -> gpui::Result<()> {
        let window = cx.update(|cx| {
            open_capture_window(
                cx,
                CaptureLayout {
                    width: 1500.,
                    height: 850.,
                    reduced_motion: true,
                },
            )
        })?;
        let key = PlaylistKey::Source {
            provider: ProviderId::Jellyfin,
            profile: Some(self.profile.clone()),
            id: self.playlist.clone(),
        };
        window.update(cx, |view, window, cx| {
            view.open_saved_playlist_for_capture(
                ProviderId::Jellyfin,
                self.playlist.clone(),
                "Jellyfin fixture".into(),
                window,
                cx,
            )
        })??;
        self.wait(window, cx, |view| {
            view.playlist_status_for_capture() == Some(Ok(3))
        })
        .await?;
        let before = inspect(window, cx)?;
        check(
            before.0[0] == before.0[2] && before.0[0] != before.0[1],
            "Expected A–B–A",
        )?;
        window.update(cx, |view, _, cx| {
            view.play_playlist_track_for_capture(0, cx)
        })?;
        self.wait(window, cx, |view| {
            view.player_snapshot.highlights(&key, before.1, 0)
                && !view.player_snapshot.highlights(&key, before.1, 2)
        })
        .await?;
        frame(
            window,
            cx,
            &self.evidence.join("jellyfin-playing-original.png"),
        )?;

        window.update(cx, |view, window, cx| {
            view.select_track(0, window, cx);
            view.refresh_playlist(cx);
        })?;
        self.wait(window, cx, |view| {
            view.playlist
                .open
                .as_ref()
                .is_some_and(|open| open.request.is_none())
        })
        .await?;
        check(
            inspect(window, cx)? == before,
            "Refreshing identical rows changed their snapshot revision",
        )?;
        check(
            window.update(cx, |view, _, _| {
                view.playlist
                    .open
                    .as_ref()
                    .is_some_and(|open| open.selected == Some(0))
                    && view.player_snapshot.highlights(&key, before.1, 0)
            })?,
            "Refreshing identical rows lost the selection or playback marker",
        )?;

        self.move_middle_entry()?;
        window.update(cx, |view, _, cx| view.refresh_playlist(cx))?;
        self.wait(window, cx, |view| {
            view.playlist.open.as_ref().is_some_and(|open| {
                open.request.is_none()
                    && open
                        .snapshot()
                        .is_some_and(|snapshot| snapshot.revision != before.1)
            })
        })
        .await?;
        let after = inspect(window, cx)?;
        check(
            after.0
                == [
                    before.0[1].clone(),
                    before.0[0].clone(),
                    before.0[0].clone(),
                ],
            "Expected B–A–A",
        )?;
        let correct = window.update(cx, |view, _, _| {
            view.player_snapshot.highlights(&key, before.1, 0)
                && (0..3).all(|row| !view.player_snapshot.highlights(&key, after.1, row))
        })?;
        check(correct, "Playback marker followed a stale row index")?;
        frame(window, cx, &self.evidence.join("jellyfin-reordered.png"))?;

        self.shutdown_fixture()?;
        let url = Url::parse(&self.server)?;
        let port = url.port().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            check(Instant::now() < deadline, "Fixture still listening")?;
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
        }
        window.update(cx, |view, window, cx| {
            view.show_music_page(crate::ui::workspace::Page::Home, window, cx);
            view.playlist_history_for_capture(false, window, cx);
        })?;
        prefetch::wait_for_playlist(window, cx, |open| open.request.is_none()).await?;
        check(
            inspect(window, cx)? == after,
            "History discarded the cached snapshot",
        )?;
        check(
            window.update(cx, |view, _, _| {
                view.playlist.open.as_ref().unwrap().request.is_none()
            })?,
            "History started a reload",
        )?;
        window.update(cx, |view, _, cx| view.refresh_playlist(cx))?;
        self.wait(window, cx, |view| {
            view.playlist
                .open
                .as_ref()
                .is_some_and(|open| open.refresh_error.is_some())
        })
        .await?;
        check(
            inspect(window, cx)? == after,
            "Offline refresh removed the cached snapshot",
        )?;
        frame(
            window,
            cx,
            &self.evidence.join("jellyfin-offline-refresh.png"),
        )?;
        self.capture_favorite_read_error(window, cx).await?;
        println!(
            "Jellyfin unchanged refresh kept the selection; A–B–A → B–A–A cleared the playback marker; after server shutdown history kept three rows and refresh reported an error"
        );
        Ok(())
    }

    async fn capture_favorite_read_error(
        &self,
        window: WindowHandle<MainView>,
        cx: &mut AsyncApp,
    ) -> gpui::Result<()> {
        let profile = Store::profile_directory()?;
        let store = Store::open(&profile)?;
        let id =
            store.create_local_playlist("Избранное", &[PathBuf::from(fixture_var("MEDIA"))])?;
        let sqlite = rusqlite::Connection::open(profile.join("mediadeka.sqlite3"))?;
        sqlite.execute_batch("ALTER TABLE favorite_track RENAME TO saved_favorite_track")?;
        let outcome = async {
            window.update(cx, |view, window, cx| {
                view.open_local_playlist_for_capture(id, window, cx);
            })?;
            self.wait(window, cx, |view| {
                view.playlist.favorites.read_failed(&PlaylistKey::Local(id))
            })
            .await?;
            frame(window, cx, &self.evidence.join("favorites-read-error.png"))
        }
        .await;
        sqlite.execute_batch("ALTER TABLE saved_favorite_track RENAME TO favorite_track")?;
        outcome
    }

    async fn wait(
        &self,
        window: WindowHandle<MainView>,
        cx: &mut AsyncApp,
        ready: impl Fn(&MainView) -> bool,
    ) -> gpui::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(18);
        loop {
            if window.update(cx, |view, _, _| ready(view))? {
                return Ok(());
            }
            let status = window.update(cx, |view, _, _| {
                (
                    view.playlist_status_for_capture(),
                    view.player_snapshot.status.clone(),
                )
            })?;
            check(Instant::now() < deadline, format!("Timed out: {status:?}"))?;
            cx.background_executor()
                .timer(Duration::from_millis(45))
                .await;
        }
    }

    fn move_middle_entry(&self) -> gpui::Result<()> {
        let mut response = self
            .agent()
            .get(format!("{}/Playlists/{}/Items", self.server, self.playlist))
            .header("Authorization", &self.authorization)
            .query("UserId", &self.user_id)
            .query("StartIndex", "0")
            .query("Limit", "200")
            .call()?;
        require_success(&mut response, "Reading test playlist entries")?;
        let items: Value = response.body_mut().read_json()?;
        let id = items["Items"][1]["PlaylistItemId"]
            .as_str()
            .ok_or_else(|| io::Error::other("No Jellyfin playlist-entry ID"))?;
        let mut response = self
            .agent()
            .post(format!(
                "{}/Playlists/{}/Items/{id}/Move/0",
                self.server, self.playlist
            ))
            .header("Authorization", &self.authorization)
            .send_empty()?;
        require_success(&mut response, "Moving the test entry")
    }

    fn shutdown_fixture(&self) -> gpui::Result<()> {
        let mut response = self
            .agent()
            .post(format!("{}/System/Shutdown", self.server))
            .header("Authorization", &self.authorization)
            .send_empty()?;
        require_success(&mut response, "Shutting down the Jellyfin fixture")
    }

    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(5)))
            .max_redirects(0)
            .build()
            .new_agent()
    }
}

fn require_success(
    response: &mut ureq::http::Response<ureq::Body>,
    action: &str,
) -> gpui::Result<()> {
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let detail = response
        .body_mut()
        .with_config()
        .limit(4096)
        .read_to_string()
        .unwrap_or_default();
    Err(io::Error::other(format!("{action}: HTTP {status} {detail}")).into())
}

fn inspect(
    window: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<(Vec<String>, Option<u64>)> {
    window.update(cx, |view, _, _| {
        let open = view
            .playlist
            .open
            .as_ref()
            .and_then(OpenPlaylist::snapshot)
            .ok_or_else(|| io::Error::other("Playlist snapshot missing"))?;
        Ok((
            open.playlist
                .tracks
                .iter()
                .map(|track| track.id.clone())
                .collect(),
            open.revision,
        ))
    })?
}

fn frame(window: WindowHandle<MainView>, cx: &mut AsyncApp, path: &Path) -> gpui::Result<()> {
    AnyWindowHandle::from(window).update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.render_to_image()?.save(path)?;
        Ok(())
    })?
}

fn check(condition: bool, message: impl Into<String>) -> gpui::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message.into()).into())
    }
}
