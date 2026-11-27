use std::{
    io::{ErrorKind, Write},
    net::{Ipv4Addr, TcpListener},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    playback::{ListeningPhase, ListeningSample, Playback, QueueEntry, Status},
    playlist::{Media, PlaylistKey, PlaylistTrack},
};

use super::super::{
    Preferences, Service,
    playback::Listening,
    scrobble::{LastFmAccount, ListenBrainzAccount, tests::read_request},
    submission::{Account, Delivery, Row},
};

#[test]
#[ignore = "requires real libmpv/WASAPI output and MEDIADEKA_INTEGRATION_AUDIO (30–34 second audio fixture)"]
fn windows_shared_queue_submits_only_audible_listens_to_both_services() {
    let fixture = PathBuf::from(std::env::var_os("MEDIADEKA_INTEGRATION_AUDIO").unwrap());
    assert!(fixture.is_absolute() && fixture.is_file());
    let lastfm = Stand::start(Service::LastFm);
    let listenbrainz = Stand::start(Service::ListenBrainz);
    let mut rows = Service::ALL.map(Row::new);
    rows[0].set_account(Some(Account::LastFm(LastFmAccount::test_at(
        lastfm.endpoint.clone(),
    ))));
    rows[1].set_account(Some(Account::ListenBrainz(ListenBrainzAccount::test_at(
        listenbrainz.endpoint.clone(),
    ))));
    rows.iter_mut().for_each(|row| row.set_enabled(true));
    let (deliveries, delivered) = async_channel::unbounded();
    let mut scenario = Scenario {
        playback: Playback::new(),
        listening: Listening::default(),
        rows,
        deliveries,
    };
    let first = entry("First test track", fixture.clone());
    let mut second = first.clone();
    second.index = 1;
    let third = entry("Different test track", fixture);
    scenario.playback.set_volume(0.08);
    scenario.playback.play(vec![first, second, third], 0);
    let initial =
        scenario.until(|sample| sample.phase == ListeningPhase::Playing && sample.position > 0.4);
    let duration = initial.duration.unwrap();
    assert!((30. ..=34.).contains(&duration));
    assert_eq!(
        scenario.playback.sound_source(),
        Some(&crate::playback::SoundSource::LocalFile)
    );
    assert_eq!(lastfm.next().kind, "track.updateNowPlaying");
    assert_eq!(listenbrainz.next().kind, "playing_now");

    scenario.playback.toggle_pause();
    let paused = scenario.until(|sample| sample.phase == ListeningPhase::Paused);
    let target = duration / 2. + 1.;
    scenario.playback.seek(target);
    let sought = scenario.until(|sample| {
        sample.phase == ListeningPhase::Paused && sample.seek_revision > paused.seek_revision
    });
    assert!((sought.position - target).abs() < 0.3);
    assert_eq!(sought.listened, paused.listened);
    scenario.drive_for(Duration::from_millis(350));
    assert_eq!(
        scenario.playback.listening_sample().unwrap().listened,
        paused.listened
    );
    assert!(lastfm.requests.try_recv().is_err() && listenbrainz.requests.try_recv().is_err());

    scenario.playback.toggle_pause();
    let resumed = scenario
        .until(|sample| sample.phase == ListeningPhase::Playing && sample.position > target + 0.4);
    assert!(resumed.listened < Duration::from_secs(3));
    assert!(lastfm.requests.try_recv().is_err() && listenbrainz.requests.try_recv().is_err());
    scenario.playback.seek(0.5);
    let rewound = scenario.until(|sample| {
        sample.phase == ListeningPhase::Playing
            && sample.seek_revision > resumed.seek_revision
            && sample.position < 1.
    });
    assert!((rewound.listened.as_secs_f64() - resumed.listened.as_secs_f64()).abs() < 0.25);
    let earned = scenario.until(|sample| sample.listened.as_secs_f64() >= duration / 2.);
    let last_one = lastfm.next();
    let brainz_one = listenbrainz.next();
    assert_eq!(last_one.kind, "track.scrobble");
    assert_eq!(brainz_one.kind, "single");
    assert_eq!(last_one.title, "First test track");
    assert_eq!(last_one.at, brainz_one.at);
    for _ in 0..4 {
        let result = delivered.recv_blocking().unwrap();
        scenario.rows[result.service as usize].delivered(result);
    }
    assert!(matches!(
        scenario.rows[0].status,
        super::super::AccountStatus::Failed(_)
    ));
    assert!(matches!(
        scenario.rows[1].status,
        super::super::AccountStatus::Connected(_)
    ));
    scenario.drive_for(Duration::from_millis(350));
    assert!(lastfm.requests.try_recv().is_err() && listenbrainz.requests.try_recv().is_err());

    scenario.playback.seek(duration - 0.6);
    let repeated = scenario.until(|sample| {
        sample.generation != earned.generation && sample.phase == ListeningPhase::Playing
    });
    assert!(repeated.listened < Duration::from_millis(250));
    assert_eq!(lastfm.next().title, "First test track");
    assert_eq!(listenbrainz.next().title, "First test track");
    let second_earned = scenario.until(|sample| sample.listened.as_secs_f64() >= duration / 2.);
    let last_two = lastfm.next();
    let brainz_two = listenbrainz.next();
    assert_eq!(last_two.kind, "track.scrobble");
    assert_eq!(brainz_two.kind, "single");
    assert_eq!(last_two.at, brainz_two.at);
    assert!(last_two.at > last_one.at);
    scenario.drive_for(Duration::from_millis(350));
    assert!(lastfm.requests.try_recv().is_err() && listenbrainz.requests.try_recv().is_err());

    scenario.playback.next();
    scenario.until(|sample| {
        sample.generation != second_earned.generation && sample.phase == ListeningPhase::Playing
    });
    assert_eq!(lastfm.next().title, "Different test track");
    assert_eq!(listenbrainz.next().title, "Different test track");
    scenario.drive_for(Duration::from_millis(350));
    assert!(lastfm.requests.try_recv().is_err() && listenbrainz.requests.try_recv().is_err());
    assert!(matches!(scenario.playback.status(), Status::Playing));
    println!(
        "libmpv queue: paused at {:.3}s; seek landed at {:.3}s with {:.3}s listened; earned at {:.3}s and {:.3}s of listening; HTTP 503 from Last.fm left ListenBrainz working",
        paused.position,
        sought.position,
        sought.listened.as_secs_f64(),
        earned.listened.as_secs_f64(),
        second_earned.listened.as_secs_f64()
    );
}

fn entry(title: &str, path: PathBuf) -> QueueEntry {
    QueueEntry {
        playlist: PlaylistKey::Local(1),
        playlist_revision: None,
        index: usize::from(title.starts_with("Different")) * 2,
        recommendations: Vec::new(),
        track: PlaylistTrack {
            id: title.into(),
            title: title.into(),
            artist: "Fixture artist".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: Some("Fixture album".into()),
            artwork: None,
            added: None,
            duration: None,
            media: Some(Media::File(path)),
        },
    }
}

struct Scenario {
    playback: Playback,
    listening: Listening,
    rows: [Row; 2],
    deliveries: async_channel::Sender<Delivery>,
}

impl Scenario {
    fn sample(&mut self) -> Option<ListeningSample> {
        self.playback.poll();
        if let Some(started) = self.playback.take_sound_started() {
            self.listening.started(&started);
        }
        let preferences = Preferences::default();
        if let Some(ended) = self.playback.take_completed_listening() {
            self.listening.sample(Some(ended));
            self.listening
                .reconcile(&preferences, &mut self.rows, &self.deliveries);
        }
        let sample = self.playback.listening_sample();
        self.listening.sample(sample);
        self.listening
            .reconcile(&preferences, &mut self.rows, &self.deliveries);
        sample
    }

    fn until(&mut self, accepts: impl Fn(ListeningSample) -> bool) -> ListeningSample {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if let Some(sample) = self.sample().filter(|sample| accepts(*sample)) {
                return sample;
            }
            assert!(
                Instant::now() < deadline,
                "player did not reach the expected state: {:?} at {:.2}s",
                self.playback.status(),
                self.playback.position()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn drive_for(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.sample();
            thread::sleep(Duration::from_millis(25));
        }
    }
}

struct ExpectedRequest {
    kind: String,
    title: String,
    at: Option<i64>,
}

struct Stand {
    endpoint: String,
    requests: mpsc::Receiver<ExpectedRequest>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Stand {
    fn start(service: Service) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let closing = Arc::clone(&stop);
        let (send, requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut rejected = false;
            while !closing.load(Ordering::Relaxed) {
                let mut connection = match listener.accept() {
                    Ok((connection, _)) => connection,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("loopback failed: {error}"),
                };
                connection.set_nonblocking(false).unwrap();
                connection
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let request = read_request(&mut connection);
                let (event, status, response) = match service {
                    Service::LastFm => {
                        let form = request.form();
                        let kind = form["method"].clone();
                        let now = kind == "track.updateNowPlaying";
                        let title = form[if now { "track" } else { "track[0]" }].clone();
                        let at = form.get("timestamp[0]").map(|at| at.parse().unwrap());
                        let (status, response) = if now {
                            (200, r#"{"nowplaying":{"ignoredMessage":{"code":"0"}}}"#)
                        } else if !rejected {
                            rejected = true;
                            (503, r#"{}"#)
                        } else {
                            (200, r#"{"scrobbles":{"@attr":{"accepted":1,"ignored":0}}}"#)
                        };
                        (ExpectedRequest { kind, title, at }, status, response)
                    }
                    Service::ListenBrainz => {
                        let json = request.json();
                        let payload = &json["payload"][0];
                        let kind = json["listen_type"].as_str().unwrap().to_owned();
                        let title = payload["track_metadata"]["track_name"]
                            .as_str()
                            .unwrap()
                            .to_owned();
                        let at = payload["listened_at"].as_i64();
                        (
                            ExpectedRequest { kind, title, at },
                            200,
                            r#"{"status":"ok"}"#,
                        )
                    }
                };
                write!(connection, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                send.send(event).unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn next(&self) -> ExpectedRequest {
        self.requests.recv_timeout(Duration::from_secs(4)).unwrap()
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
