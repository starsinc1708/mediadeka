use std::{
    collections::{BTreeMap, HashSet},
    env,
    process::Command,
};

use crate::{
    music::{
        likes::LikeIdentity,
        recommendations::{
            Mode, PLAYLISTS_PER_PROVIDER, Selection,
            acceptance::{clear_snapshots, copy_profile, seed_saved_snapshots},
            personal_playlists,
            wave::{self, Access, Batch},
        },
        region,
        station::recommendations::{self as stations, StationSource},
    },
    storage::CacheScope,
};

use super::*;

const LIVE_CHILD: &str = "MEDIADEKA_GENERAL_WAVE_LIVE_CHILD";

#[test]
#[ignore = "reads real stored playlist snapshots and checks the production wave and queue without audio"]
fn general_wave_saved_batches_and_stop() {
    let (_profile, store) = copy_profile();
    let input = seed_saved_snapshots(&store);
    let (_, station_input) = selections(&store);
    let cancelled = AtomicBool::new(false);
    let first = wave::start(
        &store,
        input.clone(),
        station_input.clone(),
        Mode::Discoveries,
        Access::Saved,
        &cancelled,
    )
    .unwrap();
    report("saved start", &first);
    let entries = first.entries(&[]);
    assert_eq!(entries.len(), 20);
    let mut playback = Playback::new();
    playback.queue.replace(entries, 0);
    let queue_generation = playback.queue_replacement_revision();
    assert!(!wave::should_refill(
        playback.current_index(),
        playback.entries().len()
    ));
    assert!(playback.queue.select(15));
    assert!(wave::should_refill(
        playback.current_index(),
        playback.entries().len()
    ));
    let second = first.feed.refill(&store, &cancelled).unwrap();
    report("saved refill", &second);

    let draft = second.entries(&playback.entries().cloned().collect::<Vec<_>>());
    assert_eq!(draft.len(), 20);
    let mut manual = draft[0].clone();
    manual.recommendations.clear();
    playback.append(vec![manual.clone()]);
    let entries = second
        .continuation(
            queue_generation,
            playback.queue_replacement_revision(),
            playback.entries().cloned(),
            &cancelled,
        )
        .unwrap();
    assert_eq!(entries.len(), 20);
    assert!(
        entries
            .iter()
            .all(|entry| entry.track.id != manual.track.id)
    );
    playback.append_continuation(entries);
    assert!(playback.entries().nth(20).unwrap() == &manual);
    assert_eq!(playback.entries().len(), 41);
    assert_eq!(identities(playback.entries()).len(), 41);
    assert_eq!(playback.current_index(), Some(15));

    let pending = second.feed.refill(&store, &cancelled).unwrap();
    let snapshot = playback.entries().cloned().collect::<Vec<_>>();
    cancelled.store(true, Ordering::Relaxed);
    assert!(
        pending
            .continuation(
                queue_generation,
                playback.queue_replacement_revision(),
                playback.entries().cloned(),
                &cancelled,
            )
            .is_none()
    );
    assert!(playback.entries().eq(&snapshot));
    cancelled.store(false, Ordering::Relaxed);
    playback.queue.replace(vec![manual], 0);
    assert!(
        pending
            .continuation(
                queue_generation,
                playback.queue_replacement_revision(),
                playback.entries().cloned(),
                &cancelled,
            )
            .is_none()
    );
    println!(
        "start=20, second=20, manual at row 21 ahead of refill, 41 distinct, cancellation and queue replacement rejected the pending batch"
    );

    let failed = ProviderId::Spotify;
    assert!(
        clear_snapshots(
            &store,
            &region(),
            input.iter().filter(|list| list.provider == failed)
        ) > 0
    );
    let partial = wave::start(
        &store,
        input,
        station_input,
        Mode::Familiar,
        Access::Saved,
        &cancelled,
    )
    .unwrap();
    report("saved Spotify failure", &partial);
    assert!(partial.unavailable.contains(&failed));
    let first = partial.entries(&[]);
    assert_eq!(first.len(), 20);
    assert!(
        first
            .iter()
            .all(|entry| entry.playlist.provider() != Some(failed))
    );
    let next = partial
        .feed
        .refill(&store, &cancelled)
        .unwrap()
        .entries(&first);
    assert!(!next.is_empty());
    assert_eq!(
        identities(first.iter().chain(&next)).len(),
        first.len() + next.len()
    );
    println!(
        "partial startup={}, partial refill={}",
        first.len(),
        next.len()
    );
}

#[test]
#[ignore = "queries real recommendation sources on an isolated profile and measures playback in each batch"]
fn general_wave_live_sources_and_sound() {
    if env::var_os(LIVE_CHILD).is_none() {
        let (profile, _) = copy_profile();
        let result = Command::new(env::current_exe().unwrap())
            .args([
                "general_wave_live_sources_and_sound",
                "--ignored",
                "--nocapture",
            ])
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .env(LIVE_CHILD, "1")
            .status()
            .unwrap();
        assert!(result.success());
        return;
    }

    let store = Store::open_default().unwrap();
    let cancelled = AtomicBool::new(false);
    let (playlists, station_input) = selections(&store);
    for provider in ProviderId::ALL {
        let connected = store.preferred_account(provider).unwrap().is_some();
        let list_count = playlists
            .iter()
            .filter(|list| list.provider == provider)
            .count();
        let station_count = station_input
            .iter()
            .filter(|station| station.key.provider() == Some(provider))
            .count();
        println!(
            "{} connected={connected} personal={list_count} stations={station_count}",
            provider.display_name()
        );
    }
    clear_snapshots(&store, &region(), playlists.iter());
    let first = wave::start(
        &store,
        playlists,
        station_input,
        Mode::Discoveries,
        Access::Live,
        &cancelled,
    )
    .unwrap();
    report("live start", &first);
    let entries = first.entries(&[]);
    assert_eq!(entries.len(), 20);
    let first_audio =
        audible_position(&entries).expect("a Yandex or SoundCloud track in the live first batch");
    let meter = Loopback::start();
    let mut playback = Playback::new();
    playback.set_volume(0.12);
    wait(&mut playback, |p| (p.volume() - 0.12).abs() < 0.001);
    playback.play(entries, first_audio);
    confirm_audio(&mut playback, &meter);

    playback.play_queued(15);
    assert!(wave::should_refill(
        playback.current_index(),
        playback.entries().len()
    ));
    let second = first.feed.refill(&store, &cancelled).unwrap();
    report("live refill", &second);
    let queue = playback.queue_replacement_revision();
    let entries = second
        .continuation(queue, queue, playback.entries().cloned(), &cancelled)
        .unwrap();
    assert!(!entries.is_empty());
    let second_audio =
        audible_position(&entries).expect("a Yandex or SoundCloud track in the live second batch");
    let first_len = playback.entries().len();
    let second_len = entries.len();
    playback.append_continuation(entries);
    assert_eq!(identities(playback.entries()).len(), first_len + second_len);
    playback.play_queued(first_len + second_audio);
    confirm_audio(&mut playback, &meter);
    println!(
        "live queue: first={first_len} second={second_len} wave rows have origins and unique recordings"
    );
}

fn selections(store: &Store) -> (Vec<Selection>, Vec<StationSource>) {
    let mut playlists = Vec::new();
    let mut station_input = Vec::new();
    for provider in ProviderId::ALL {
        let Some(account) = store.preferred_account(provider).unwrap() else {
            continue;
        };
        let profile = &account.profile.id;
        let snapshot = store.cache(CacheScope::Account(provider, profile)).unwrap();
        let home = snapshot.as_ref().map(|snapshot| &snapshot.home);
        if let Some(home) = home {
            playlists
                .extend(personal_playlists(home, provider, profile).take(PLAYLISTS_PER_PROVIDER));
        }
        station_input.extend(stations::selections(home, provider, profile));
    }
    (playlists, station_input)
}

fn report(label: &str, batch: &Batch) {
    let mut origins = BTreeMap::new();
    for entry in batch.candidates() {
        assert!(entry.is_playable());
        assert!(!entry.recommendations.is_empty());
        for origin in &entry.recommendations {
            *origins
                .entry((origin.provider.display_name(), &origin.playlist))
                .or_insert(0) += 1;
        }
    }
    println!(
        "{label}: candidates={} by source={origins:?}, unavailable={:?}",
        batch.candidates().len(),
        batch.unavailable
    );
}

fn identities<'a>(entries: impl Iterator<Item = &'a QueueEntry>) -> HashSet<LikeIdentity> {
    entries
        .map(|entry| LikeIdentity::from_track(&entry.playlist, &entry.track).unwrap())
        .collect()
}

fn audible_position(entries: &[QueueEntry]) -> Option<usize> {
    [ProviderId::YandexMusic, ProviderId::SoundCloud]
        .into_iter()
        .find_map(|provider| {
            entries
                .iter()
                .position(|entry| entry.playlist.provider() == Some(provider))
        })
}

fn confirm_audio(playback: &mut Playback, meter: &Loopback) {
    wait(playback, |p| {
        p.status() == &Status::Playing && p.position() >= 0.4
    });
    let peak = meter.wait(|peak| peak > 0.0001);
    let started = playback.take_sound_started().unwrap();
    assert!(playback.frame.output.as_deref() == Some("wasapi"));
    println!(
        "WASAPI queue position={:?} source={} seconds={:.3} peak={peak:.6}",
        playback.current_index(),
        started.source.label(),
        playback.position()
    );
    playback.toggle_pause();
    wait(playback, |p| p.status() == &Status::Paused);
    meter.wait(|value| value <= 0.0001);
}
