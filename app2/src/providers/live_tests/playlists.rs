use super::*;

#[test]
#[ignore = "reads the current public YouTube Music playlist from the guest home feed"]
fn live_playlist_youtube_guest() {
    let cancelled = AtomicBool::new(false);
    let home = guest_home(&cancelled).expect("YouTube Music guest home");
    let provider = ProviderId::YouTubeMusic;
    assert_live_playlists(provider, None, &home, &cancelled, 1);
}

#[test]
#[ignore = "reads a paginated public YouTube Music playlist without an account"]
fn live_playlist_youtube_guest_progress() {
    use std::time::Instant;

    let id = std::env::var("MEDIADEKA_TEST_PUBLIC_YOUTUBE_PLAYLIST")
        .unwrap_or_else(|_| "PLFYkUdNE47eQ".into());
    let start = Instant::now();
    let before = crate::telemetry::received().api_requests;
    let mut received = Vec::new();
    let mut pages = 0;
    let playlist = read_playlist_with_progress(
        ProviderId::YouTubeMusic,
        None,
        &id,
        &AtomicBool::new(false),
        &mut |page| {
            pages += 1;
            let requests = crate::telemetry::received().api_requests - before;
            assert_eq!(page.offset, received.len());
            assert!(!page.tracks.is_empty());
            assert!(page.tracks.iter().all(|track| track.media.is_none()));
            assert_eq!(requests, pages, "More than one request per delivered page");
            println!(
                "Partial page {pages}: offset={}, rows={}, requests={requests}, elapsed={:.2} ms",
                page.offset,
                page.tracks.len(),
                start.elapsed().as_secs_f64() * 1_000.,
            );
            received.extend_from_slice(page.tracks);
            Ok(())
        },
    )
    .unwrap_or_else(|error| panic!("Public playlist {id}: {error:?}"));
    let requests = crate::telemetry::received().api_requests - before;
    println!(
        "Completed public playlist {id} ({:?}): {} tracks, {} delivered early, {requests} requests, {:.2} ms",
        playlist.name,
        playlist.tracks.len(),
        received.len(),
        start.elapsed().as_secs_f64() * 1_000.,
    );
    assert!(pages > 0, "The public playlist has no continuation");
    assert!(playlist.tracks.len() > received.len());
    assert!(playlist.tracks.starts_with(&received));
    assert!(playlist.tracks.iter().all(|track| track.media.is_none()));
    assert_eq!(requests, pages + 1, "More than one request per page");
}

#[test]
#[ignore = "checks a real YouTube Music playlist and its artist profile without an account"]
fn live_playlist_youtube_verified_author() {
    let provider = ProviderId::YouTubeMusic;
    let cancelled = AtomicBool::new(false);
    let playlist_id = "PLBwooDLbL25c";
    let playlist = read_playlist(provider, None, playlist_id, &cancelled)
        .expect("read the public The Beatles playlist");
    let author = playlist.owner.expect("YouTube Music playlist author");
    let target = author
        .reference()
        .expect("typed artist profile from playlist");
    assert_eq!(target.id, "UCc4K7bAqpdBP8jh1j9XZAww");
    assert_eq!(target.kind, crate::music::ObjectKind::Artist);
    let page = crate::providers::catalog::read(target, None, &cancelled)
        .expect("read the exact artist page");
    assert_eq!(page.reference, *target);
    assert_eq!(page.title, "The Beatles");

    let user_playlist = read_playlist(provider, None, "PLFYkUdNE47eQ", &cancelled)
        .expect("read the public playlist from a user channel");
    let uploader = user_playlist.owner.expect("user channel display name");
    assert!(!uploader.name().is_empty());
    assert!(uploader.reference().is_none());
    println!(
        "Public playlist {playlist_id} ({}) opens exact native author {} ({}) without sign-in; its user-channel counterpart supplies a plain name",
        playlist.name, page.title, target.id,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "reads playlists and a stream descriptor from the existing Windows Yandex test account"]
fn live_playlist_yandex() {
    let cancelled = AtomicBool::new(false);
    let session = existing_yandex_session();
    let home = home(&session, &cancelled).expect("Yandex Music feed");
    let mut collections = home
        .items
        .into_iter()
        .filter_map(|item| Some((playlist_id(session.provider, &item)?, item.title)))
        .collect::<Vec<_>>();
    collections.sort_by_key(|(id, _)| !id.contains(':'));
    let mut successes = Vec::new();
    for editorial in [false, true] {
        let (id, _) = collections
            .iter()
            .find(|(id, _)| editorial != id.contains(':'))
            .expect("playlist card in each type");
        let playlist = read_playlist(session.provider, Some(&session.credentials), id, &cancelled)
            .unwrap_or_else(|error| {
                panic!("Yandex playlist {id} (editorial={editorial}): {error}")
            });
        println!(
            "Yandex playlist (editorial={editorial}, {id}): {} rows, {} stream candidates",
            playlist.tracks.len(),
            playlist
                .tracks
                .iter()
                .filter(|track| track.media.is_some())
                .count()
        );
        successes.push(playlist);
    }
    verify_stream(
        session.provider,
        &session.credentials,
        &successes,
        &cancelled,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires a test SoundCloud profile path in MEDIADEKA_SOUNDCLOUD_TEST_PROFILE"]
fn live_playlist_soundcloud() {
    let cancelled = AtomicBool::new(false);
    let session = test_profile(
        ProviderId::SoundCloud,
        "MEDIADEKA_SOUNDCLOUD_TEST_PROFILE",
        &cancelled,
    );
    let home = home(&session, &cancelled).expect("SoundCloud home");
    let provider = session.provider;
    let lists = [false, true].map(|system| {
        let id = home
            .items
            .iter()
            .filter_map(|item| playlist_id(provider, item))
            .find(|id| soundcloud::system_playlist_id(id) == system)
            .expect("ordinary or system playlist card");
        let list = read_playlist(provider, Some(&session.credentials), &id, &cancelled)
            .expect("read SoundCloud playlist from home");
        println!(
            "SoundCloud playlist (system={system}, {id}): {} rows, {} stream candidates",
            list.tracks.len(),
            list.tracks.iter().filter(|row| row.media.is_some()).count()
        );
        assert!(!list.tracks.is_empty());
        list
    });
    verify_stream(provider, &session.credentials, &lists, &cancelled);
}

#[cfg(windows)]
#[test]
#[ignore = "requires a test Spotify profile path in MEDIADEKA_SPOTIFY_TEST_PROFILE"]
fn live_playlist_spotify() {
    let cancelled = AtomicBool::new(false);
    let session = test_profile(
        ProviderId::Spotify,
        "MEDIADEKA_SPOTIFY_TEST_PROFILE",
        &cancelled,
    );
    let provider = session.provider;
    if let Ok(id) = std::env::var("MEDIADEKA_SPOTIFY_TEST_PLAYLIST") {
        let playlist = read_playlist(provider, Some(&session.credentials), &id, &cancelled)
            .expect("Spotify playlist");
        assert!(!playlist.tracks.is_empty());
        println!(
            "Spotify playlist: {} rows, {} native playback candidates",
            playlist.tracks.len(),
            playlist
                .tracks
                .iter()
                .filter(|track| track.media.is_some())
                .count()
        );
    } else {
        let home = home(&session, &cancelled).expect("Spotify home");
        assert_live_playlists(provider, Some(&session.credentials), &home, &cancelled, 1);
        let shelf = home
            .sections
            .iter()
            .find(|shelf| shelf.title == "Плейлисты Spotify")
            .expect("Spotify rootlist shelf");
        let cards = &home.items[shelf.start..shelf.start + shelf.len];
        println!("Spotify rootlist shelf: {} cards", cards.len());
        if let Some(reference) = cards.iter().find(|item| item.title == "SET::0.1") {
            let id = playlist_id(provider, reference).unwrap();
            let list = read_playlist(provider, Some(&session.credentials), &id, &cancelled)
                .expect("playlist from the reference");
            println!(
                "Spotify reference playlist {id}: {} rows, {} stream candidates",
                list.tracks.len(),
                list.tracks.iter().filter(|row| row.media.is_some()).count()
            );
            assert!(!list.tracks.is_empty());
        }
    }
}

#[cfg(windows)]
#[test]
#[ignore = "reads a paginated public Spotify playlist through a copied test profile"]
fn live_playlist_spotify_saved_progress() {
    use std::time::Instant;

    let provider = ProviderId::Spotify;
    let isolated = profile_snapshot("MEDIADEKA_SPOTIFY_TEST_PROFILE");
    let store = crate::storage::Store::open(isolated.path()).expect("open isolated test profile");
    let account = store
        .preferred_account(provider)
        .expect("read preferred account")
        .expect("Spotify account in the supplied profile");
    let credentials = store
        .credentials(&account)
        .expect("read credential storage")
        .expect("saved Spotify credentials");
    let id = std::env::var("MEDIADEKA_SPOTIFY_TEST_PLAYLIST")
        .unwrap_or_else(|_| "0DAbLtu6YSb1rXIRtfTOH7".into());
    let start = Instant::now();
    let mut early = Vec::new();
    let mut reported_total = None;
    let playlist = read_playlist_with_progress(
        provider,
        Some(&credentials),
        &id,
        &AtomicBool::new(false),
        &mut |page| {
            let total = page.total.expect("Spotify playlist length");
            assert_eq!(page.offset, early.len());
            assert!(!page.tracks.is_empty());
            assert!(page.offset + page.tracks.len() < total);
            assert_eq!(*reported_total.get_or_insert(total), total);
            println!(
                "Partial: offset={}, rows={}, total={total}, elapsed={:.2} ms",
                page.offset,
                page.tracks.len(),
                start.elapsed().as_secs_f64() * 1_000.,
            );
            early.extend_from_slice(page.tracks);
            Ok(())
        },
    )
    .unwrap_or_else(|error| panic!("Public Spotify playlist {id}: {error}"));
    println!(
        "Completed public Spotify playlist {id} ({:?}): {} tracks, {} delivered early, {:.2} ms",
        playlist.name,
        playlist.tracks.len(),
        early.len(),
        start.elapsed().as_secs_f64() * 1_000.,
    );
    assert!(
        playlist.tracks.len() > 100,
        "The playlist must span two pages"
    );
    assert_eq!(reported_total, Some(playlist.tracks.len()));
    assert!(playlist.tracks.starts_with(&early));
}

#[test]
#[ignore = "requires a dedicated Jellyfin test server with a music playlist and duplicate entries"]
fn live_playlist_jellyfin() {
    let server = std::env::var("MEDIADEKA_TEST_JELLYFIN_URL").expect("fixture URL");
    let username = std::env::var("MEDIADEKA_TEST_JELLYFIN_USER").expect("fixture username");
    let password = std::env::var("MEDIADEKA_TEST_JELLYFIN_PASSWORD").expect("fixture password");
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::Jellyfin;
    let session = sign_in(
        provider,
        LoginInput::JellyfinPassword {
            server,
            username,
            password,
        },
        &cancelled,
        |_| {},
    )
    .expect("sign in to Jellyfin fixture");
    let collection = home(&session, &cancelled).expect("Jellyfin home");
    let lists = assert_live_playlists(
        provider,
        Some(&session.credentials),
        &collection,
        &cancelled,
        1,
    );
    assert_eq!(lists[0].tracks.len(), 2, "repeated playlist additions");
    assert_eq!(
        lists[0].tracks[0].media, lists[0].tracks[1].media,
        "same media in both additions"
    );
    verify_stream(provider, &session.credentials, &lists, &cancelled);
}

#[cfg(windows)]
pub(super) fn test_profile(
    provider: ProviderId,
    variable: &str,
    cancelled: &AtomicBool,
) -> Session {
    let isolated = profile_snapshot(variable);
    let store = crate::storage::Store::open(isolated.path()).expect("open isolated profile");
    let account = store
        .preferred_account(provider)
        .expect("lookup fixture account")
        .expect("saved fixture account");
    let credentials = store
        .credentials(&account)
        .expect("credential storage")
        .expect("fixture credentials");
    restore(provider, &account.profile, credentials, cancelled)
        .expect("restore saved fixture account")
}

#[cfg(windows)]
pub(in crate::providers) fn profile_snapshot(variable: &str) -> tempfile::TempDir {
    let path = std::env::var_os(variable).expect("test profile directory");
    // Snapshot the database under a read transaction so the live profile can remain open.
    let source = std::path::PathBuf::from(path).join("mediadeka.sqlite3");
    let isolated = tempfile::tempdir().expect("isolated test profile");
    let destination = isolated.path().join("mediadeka.sqlite3");
    let original =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read saved fixture");
    original
        .execute(
            "VACUUM INTO ?1",
            [destination.to_str().expect("fixture path")],
        )
        .expect("snapshot saved fixture");
    isolated
}

fn assert_live_playlists(
    provider: ProviderId,
    credentials: Option<&Credentials>,
    home: &Home,
    cancelled: &AtomicBool,
    minimum: usize,
) -> Vec<crate::playlist::Playlist> {
    let mut examined = std::collections::HashSet::new();
    let lists = home
        .items
        .iter()
        .filter_map(|item| playlist_id(provider, item))
        .filter(|id| examined.insert(id.clone()))
        .take(5)
        .filter_map(
            |id| match read_playlist(provider, credentials, &id, cancelled) {
                Ok(playlist) => {
                    println!(
                        "{} playlist ({id}): {} rows, {} stream candidates",
                        provider.display_name(),
                        playlist.tracks.len(),
                        playlist
                            .tracks
                            .iter()
                            .filter(|row| row.media.is_some())
                            .count()
                    );
                    Some(playlist)
                }
                Err(error) => {
                    println!("{} playlist ({id}): {error}", provider.display_name());
                    None
                }
            },
        )
        .filter(|list| !list.tracks.is_empty())
        .take(minimum)
        .collect::<Vec<_>>();
    assert_eq!(
        lists.len(),
        minimum,
        "nonempty playlists from {}",
        provider.display_name()
    );
    lists
}

fn verify_stream(
    provider: ProviderId,
    credentials: &Credentials,
    lists: &[crate::playlist::Playlist],
    cancelled: &AtomicBool,
) {
    use crate::playlist::Media;
    let agent = transport::agent();
    let candidates = lists
        .iter()
        .flat_map(|list| list.tracks.iter())
        .filter_map(|row| match row.media.as_ref()? {
            Media::Provider { id, .. } => Some(id),
            _ => None,
        });
    for id in candidates.take(5) {
        match direct_stream(provider, Some(credentials), id, cancelled) {
            Ok(StreamOffer::Full(stream)) => {
                let mut response = agent
                    .get(&stream.url)
                    .header("Range", "bytes=0-1023")
                    .call()
                    .expect("audio CDN response");
                let status = response.status();
                let content_type = response
                    .headers()
                    .get("Content-Type")
                    .and_then(|header| header.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                use std::io::Read;
                let mut buffer = [0; 64];
                let count = response
                    .body_mut()
                    .as_reader()
                    .read(&mut buffer)
                    .expect("read prefix without recording the private URL");
                let sample = &buffer[..count];
                let format = if sample.starts_with(b"fLaC") {
                    "FLAC"
                } else if sample.starts_with(b"#EXTM3U") {
                    "HLS"
                } else if sample.get(4..8) == Some(b"ftyp") {
                    "MP4"
                } else if sample.starts_with(b"OggS") {
                    "Ogg"
                } else if sample.starts_with(b"ID3") {
                    "ID3"
                } else if sample.starts_with(b"\xff\xfb") {
                    "MP3"
                } else {
                    "unspecified"
                };
                println!(
                    "{} audio CDN: HTTP {status}, {content_type}, {format}, {count} bytes sampled",
                    provider.display_name()
                );
                if status.is_success()
                    && count > 0
                    && (format != "unspecified" || content_type.starts_with("audio/"))
                {
                    return;
                }
            }
            Ok(StreamOffer::Preview) => {
                println!("{} audio descriptor: preview", provider.display_name());
            }
            Err(error) => println!("{} audio descriptor: {error}", provider.display_name()),
        }
    }
    panic!(
        "{} supplied no reachable audio descriptor",
        provider.display_name()
    );
}
