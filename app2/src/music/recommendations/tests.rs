use crate::{
    music::{ObjectKind, ObjectRef, likes::LikeIdentity, matching::MatchLinks},
    playback::SoundSource,
    playlist::{Media, PlaylistTrack},
};

use super::*;

fn build_entries(playlists: &[(usize, LoadedPlaylist)], links: MatchLinks) -> Vec<QueueEntry> {
    Ranking::without_signals(links).entries(&build_candidates(playlists), &[])
}

fn confirm_yandex_alias(store: &Store, provider: ProviderId, id: &str) {
    let region = region();
    let reference = ObjectRef {
        provider,
        kind: ObjectKind::Track,
        id: id.into(),
    };
    assert!(
        store
            .claim_music_match(&region, &reference, 100, 150)
            .unwrap()
    );
    store
        .save_music_match(
            &region,
            &[reference],
            150,
            999,
            &serde_json::json!({
                "links": [{
                    "platform": "yandex-music",
                    "id": "100:201",
                    "url": "https://music.yandex.ru/album/201/track/100",
                    "evidence": "ISRC",
                    "checked_at": 100
                }]
            }),
        )
        .unwrap();
}

fn list(provider: ProviderId, id: &str, title: &str, ids: &[(&str, &str)]) -> LoadedPlaylist {
    LoadedPlaylist {
        key: PlaylistKey::Source {
            provider,
            profile: Some("profile".into()),
            id: id.into(),
        },
        origin: RecommendationOrigin {
            provider,
            playlist: title.into(),
            playlist_id: id.into(),
        },
        data: Playlist {
            name: title.into(),
            owner: None,
            artwork: None,
            link: None,
            tracks: ids
                .iter()
                .map(|&(id, title)| PlaylistTrack {
                    id: id.into(),
                    title: title.into(),
                    artist: "Artist".into(),
                    artists: Vec::new(),
                    artist_credits: Vec::new(),
                    album_ref: None,
                    album: None,
                    artwork: None,
                    added: None,
                    duration: Some(180),
                    media: Some(Media::Provider {
                        provider,
                        id: id.into(),
                    }),
                })
                .collect(),
        },
    }
}

#[test]
fn confirmed_duplicates_keep_origins_and_different_recordings_stay_separate() {
    use ProviderId::{SoundCloud, Spotify, YandexMusic};

    let spotify = "12AbCdEfgHiJkL0mNOpQRs";
    let different_version = "34AbCdEfgHiJkL0mNOpQRs";
    let lists = [
        (
            0,
            list(
                Spotify,
                "daily",
                "Daily Mix",
                &[(spotify, "Song"), (different_version, "Song (Live)")],
            ),
        ),
        (
            0,
            list(
                YandexMusic,
                "weekly",
                "Дежавю",
                &[("100:200", "Song"), ("101:200", "Song")],
            ),
        ),
        (
            0,
            list(SoundCloud, "discovery", "Weekly", &[("123", "Song")]),
        ),
        (
            1,
            list(YandexMusic, "daily", "Плейлист дня", &[("100:201", "Song")]),
        ),
    ];
    let reference = |provider, id: &str| ObjectRef {
        provider,
        kind: ObjectKind::Track,
        id: id.into(),
    };
    let links = MatchLinks {
        native: vec![(
            reference(Spotify, spotify),
            reference(YandexMusic, "100:999"),
        )],
        ..Default::default()
    };
    let entries = build_entries(&lists, links);
    assert_eq!(
        entries
            .iter()
            .map(|entry| &*entry.track.id)
            .collect::<Vec<_>>(),
        [spotify, "123", different_version, "101:200"]
    );
    assert_eq!(
        entries[0].recommendations,
        [&lists[0], &lists[1], &lists[3]].map(|(_, list)| list.origin.clone())
    );
    assert!(
        entries[1..]
            .iter()
            .all(|entry| entry.recommendations.len() == 1)
    );
}

#[test]
fn first_twenty_distinct_recordings_keep_origins_seen_after_the_cutoff() {
    use ProviderId::SoundCloud;
    let titles = (1..=21).map(|index| index.to_string()).collect::<Vec<_>>();
    let tracks = titles
        .iter()
        .map(|id| (id.as_str(), id.as_str()))
        .collect::<Vec<_>>();
    let mut rotated = tracks.clone();
    rotated.rotate_left(1);
    let other = list(SoundCloud, "weekly", "Weekly", &rotated);
    let first = list(SoundCloud, "daily", "Mix 1", &tracks);
    let entries = build_entries(&[(0, first), (1, other)], MatchLinks::default());
    assert_eq!(entries.len(), 20);
    assert_eq!(entries[0].track.id, "1");
    assert_eq!(entries[0].recommendations.len(), 2);
    assert_eq!(entries.last().unwrap().track.id, "20");
}

#[test]
fn modes_use_history_and_likes_and_reactions_survive_reopening() {
    use ProviderId::{SoundCloud, Spotify, YandexMusic};

    let first = "12AbCdEfgHiJkL0mNOpQRs";
    let familiar = "34AbCdEfgHiJkL0mNOpQRs";
    let promoted = "56AbCdEfgHiJkL0mNOpQRs";
    let names = &[(first, "First"), (familiar, "Heard"), (promoted, "More")];
    let candidates = build_candidates(&[
        (0, list(Spotify, "daily", "Daily Mix", names)),
        (
            0,
            list(YandexMusic, "daily", "Дежавю", &[("100:200", "Heard")]),
        ),
        (0, list(SoundCloud, "weekly", "Weekly", &[("123", "Liked")])),
    ]);
    let played_alias = candidates
        .iter()
        .find(|entry| entry.track.id == "100:200")
        .unwrap();
    let liked = candidates
        .iter()
        .find(|entry| entry.track.id == "123")
        .unwrap();
    let promoted_entry = candidates
        .iter()
        .find(|entry| entry.track.id == promoted)
        .unwrap();
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    confirm_yandex_alias(&store, Spotify, familiar);
    store
        .record_history(
            &played_alias.playlist,
            &played_alias.track,
            SoundSource::Stream(YandexMusic),
        )
        .unwrap();
    let liked_identity = LikeIdentity::from_track(&liked.playlist, &liked.track).unwrap();
    store
        .set_local_like(&liked_identity, &liked.track, true, &AtomicBool::new(false))
        .unwrap();
    let ranked = |store: &Store, mode| {
        Ranking::load(store, mode)
            .unwrap()
            .entries(&candidates, &[])
    };
    let ids = |entries: Vec<QueueEntry>| {
        entries
            .into_iter()
            .map(|entry| entry.track.id)
            .collect::<Vec<_>>()
    };
    let familiar_entries = ranked(&store, Mode::Familiar);
    assert_eq!(familiar_entries[0].recommendations.len(), 2);
    assert_eq!(ids(familiar_entries), ["100:200", "123", first, promoted]);
    assert_eq!(
        ids(ranked(&store, Mode::Discoveries)),
        [first, "123", promoted, "100:200"]
    );
    set_reaction(&store, promoted_entry, Reaction::MoreLikeThis).unwrap();
    set_reaction(&store, played_alias, Reaction::Hide).unwrap();
    drop(store);

    let reopened = Store::open(profile.path()).unwrap();
    assert_eq!(
        ids(ranked(&reopened, Mode::Familiar)),
        ["123", promoted, first]
    );
    assert_eq!(
        ids(ranked(&reopened, Mode::Discoveries)),
        [promoted, first, "123"]
    );
    let spotify_alias = candidates
        .iter()
        .find(|entry| entry.track.id == familiar)
        .unwrap();
    set_reaction(&reopened, spotify_alias, Reaction::MoreLikeThis).unwrap();
    assert_eq!(
        ids(ranked(&reopened, Mode::Familiar)),
        ["100:200", "123", promoted, first]
    );
}

#[test]
fn more_promotes_the_full_artist_credit_until_the_recording_is_hidden() {
    use ProviderId::{SoundCloud, YandexMusic};

    let mut playlist = list(
        SoundCloud,
        "daily",
        "Weekly",
        &[
            ("1", "Queued"),
            ("2", "Other"),
            ("3", "Same artist"),
            ("4", "Collaboration"),
            ("5", "Exact reaction"),
        ],
    );
    for (track, artist) in playlist.data.tracks.iter_mut().zip([
        "АККОРД!",
        "Artist",
        " АкКорд ",
        "Аккорд, Duo",
        "Solo",
    ]) {
        track.artist = artist.into();
    }
    let candidates = build_candidates(&[(0, playlist)]);
    let queued_alias = build_candidates(&[(
        0,
        list(YandexMusic, "daily", "Микс", &[("100:200", "Queued")]),
    )]);
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    confirm_yandex_alias(&store, SoundCloud, "1");
    set_reaction(&store, &candidates[0], Reaction::MoreLikeThis).unwrap();
    set_reaction(&store, &candidates[4], Reaction::MoreLikeThis).unwrap();
    drop(store);

    let rank = |store: &Store, excluded: &[QueueEntry]| {
        Ranking::load(store, Mode::Discoveries)
            .unwrap()
            .entries(&candidates, excluded)
            .into_iter()
            .map(|entry| entry.track.id)
            .collect::<Vec<_>>()
    };
    let reopened = Store::open(profile.path()).unwrap();
    assert_eq!(rank(&reopened, &queued_alias), ["5", "3", "2", "4"]);
    set_reaction(&reopened, &queued_alias[0], Reaction::Hide).unwrap();
    drop(reopened);

    let reopened = Store::open(profile.path()).unwrap();
    assert_eq!(rank(&reopened, &[]), ["5", "2", "3", "4"]);
}

#[test]
fn confirmed_exclusions_cross_an_alias_absent_from_the_queue_and_batch() {
    use ProviderId::{SoundCloud, Spotify, YandexMusic};

    let spotify_id = "12AbCdEfgHiJkL0mNOpQRs";
    let batch = build_candidates(&[(
        0,
        list(Spotify, "daily", "Daily Mix", &[(spotify_id, "Same")]),
    )]);
    let current =
        build_candidates(&[(0, list(SoundCloud, "weekly", "Weekly", &[("123", "Same")]))]);
    let identity =
        |entry: &QueueEntry| LikeIdentity::from_track(&entry.playlist, &entry.track).unwrap();
    let absent = LikeIdentity {
        provider: Some(YandexMusic),
        profile: Some("profile".into()),
        id: "100:200".into(),
    };
    let links = MatchLinks {
        choices: vec![
            (identity(&batch[0]), absent.clone()),
            (absent, identity(&current[0])),
        ],
        ..Default::default()
    };
    let ranking = Ranking::without_signals(links);
    assert_eq!(ranking.entries(&batch, &[]).len(), 1);
    assert!(ranking.entries(&batch, &current).is_empty());
}
