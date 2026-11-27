use std::path::PathBuf;
use std::thread;

use crate::playback::QueueEntry;
use crate::playback::tests::windows::Loopback;
use crate::playlist::{Media, PlaylistKey, PlaylistTrack};

use super::*;

#[test]
#[ignore = "requires libmpv, WASAPI output and MEDIADEKA_AUDIO_FIXTURE"]
fn windows_reopens_after_libmpv_shutdown() {
    let fixture = PathBuf::from(std::env::var_os("MEDIADEKA_AUDIO_FIXTURE").unwrap());
    assert!(fixture.is_absolute() && fixture.is_file());
    let commands = mpsc::channel().0;
    let meter = Loopback::start();
    let mut driver = Driver::default();
    let volume = 0.35;
    driver.handle(Command::Volume(volume), &commands);

    driver.mpv().unwrap().command(&["quit"]).unwrap();
    poll_until(&mut driver, |driver| driver.mpv.is_none());
    assert_eq!(driver.frame.status, Status::Idle);

    let load = || Command::Load {
        requested_at: Instant::now(),
        entry: Box::new(QueueEntry {
            playlist_revision: None,
            recommendations: Vec::new(),
            playlist: PlaylistKey::Local(1),
            index: 0,
            track: PlaylistTrack {
                id: "local-audio".into(),
                title: "Local audio".into(),
                artist: String::new(),
                artwork: None,
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album_ref: None,
                album: None,
                added: None,
                duration: None,
                media: Some(Media::File(fixture.clone())),
            },
        }),
        position: 0.,
    };
    let playing =
        |driver: &Driver| driver.frame.status == Status::Playing && driver.frame.position > 0.20;

    driver.handle(load(), &commands);
    poll_until(&mut driver, playing);
    let initial_peak = meter.wait(|peak| peak > 0.001);
    driver.mpv().unwrap().command(&["quit"]).unwrap();
    poll_until(&mut driver, |driver| {
        matches!(driver.frame.status, Status::Failed(_))
    });
    assert!(driver.mpv.is_none());
    assert!(driver.file.is_none());
    assert!(driver.frame.output.is_none());
    assert!(!driver.frame.seekable);
    let stopped_peak = meter.wait(|peak| peak < 0.0001);

    driver.handle(load(), &commands);
    poll_until(&mut driver, playing);
    assert_eq!(driver.frame.output.as_deref(), Some("wasapi"));
    assert!((driver.frame.volume - volume).abs() < 0.001);
    let recovered_peak = meter.wait(|peak| peak > 0.001);
    println!(
        "WASAPI initial_peak={initial_peak:.6} stopped_peak={stopped_peak:.6} recovered_peak={recovered_peak:.6} position={:.3}s volume={:.2}",
        driver.frame.position, driver.frame.volume,
    );
    driver.stop().unwrap();
}

#[test]
fn setup_failure_has_distinct_signal_and_preserves_position() {
    let mut driver = Driver::default();
    driver.frame.position = 47.25;
    driver.fail(Failure::SoloistSetup("Нужно сопряжение.".into()));
    assert!(driver.frame.soloist_setup_required);
    assert_eq!(driver.frame.position, 47.25);
    assert!(matches!(driver.frame.status, Status::Failed(_)));

    driver.fail("Запись недоступна.".into());
    assert!(!driver.frame.soloist_setup_required);
    assert!(matches!(driver.frame.status, Status::Failed(_)));
}

fn poll_until(driver: &mut Driver, ready: impl Fn(&Driver) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready(driver) {
        assert!(
            Instant::now() < deadline,
            "status={:?}, position={}",
            driver.frame.status,
            driver.frame.position,
        );
        driver.poll();
        thread::sleep(TICK);
    }
}
