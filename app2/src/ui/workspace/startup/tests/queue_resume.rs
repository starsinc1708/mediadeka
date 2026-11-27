use std::process::Command;

use crate::{
    playback::{Playback, QueueEntry, SoundSource, Status},
    playlist::{Media, PlaylistKey, PlaylistTrack},
};

use super::*;

const TEST_NAME: &str = "ui::workspace::startup::tests::queue_resume::native_restores_queue_without_playing_until_requested";
const PHASE: &str = "MEDIADEKA_QUEUE_RESUME_PHASE";
const SEEK_TO: f64 = 7.5;

#[test]
#[ignore = "requires Windows D3D11 and an actual 32-second WAV in MEDIADEKA_QUEUE_RESUME_AUDIO"]
fn native_restores_queue_without_playing_until_requested() {
    let evidence = PathBuf::from(env::var_os("MEDIADEKA_QUEUE_RESUME_EVIDENCE").unwrap());
    let audio = PathBuf::from(env::var_os("MEDIADEKA_QUEUE_RESUME_AUDIO").unwrap());
    assert!(audio.is_file());
    fs::create_dir_all(&evidence).unwrap();
    if let Ok(phase) = env::var(PHASE) {
        let save = phase == "save";
        let result = Arc::new(Mutex::new(None));
        let completed = Arc::clone(&result);
        application().run(move |cx| {
            cx.spawn(async move |cx| {
                let capture = check(cx, &evidence, &audio, save).await;
                *completed.lock().unwrap() = Some(capture);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
        if let Some(result) = result.lock().unwrap().take() {
            result.unwrap();
        } else {
            // Closing the last window stops the native event loop. The parent
            // checks the committed checkpoint before launching the new process.
            assert!(save);
        }
        return;
    }
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    store.use_guest(|| Some(())).unwrap();
    for phase in ["save", "restore"] {
        let output = Command::new(env::current_exe().unwrap())
            .args([TEST_NAME, "--exact", "--ignored", "--nocapture"])
            .env(PHASE, phase)
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .output()
            .unwrap();
        let log = [output.stdout, output.stderr].concat();
        fs::write(evidence.join(format!("{phase}.log")), &log).unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&log));
        println!("{}", String::from_utf8_lossy(&log));
        if phase == "save" {
            let saved = store.queue_resume().unwrap().unwrap();
            assert_eq!(saved.progress.current, Some(1));
            assert!((7_500..9_000).contains(&saved.progress.position_ms));
            let report = format!("SQLite close checkpoint: {} ms", saved.progress.position_ms);
            fs::write(evidence.join("checkpoint.txt"), &report).unwrap();
            println!("{report}");
        }
    }
}

async fn check(cx: &mut AsyncApp, evidence: &Path, audio: &Path, save: bool) -> gpui::Result<()> {
    let store = Store::open_default()?;
    let persisted = store.queue_resume()?;
    let window = cx.update(|cx| {
        open_capture_window(
            cx,
            CaptureLayout {
                width: 1200.,
                height: 720.,
                reduced_motion: true,
            },
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(15);
    window.update(cx, |view, window, cx| {
        view.start_desktop(Instant::now(), window, cx)
    })?;
    loop {
        let ready = window.update(cx, |view, _, _| !view.workspace.startup.loading)?;
        if ready {
            break;
        }
        wait(cx, deadline, "workspace opening").await;
    }
    if save {
        assert!(persisted.is_none());
        let queue = ["Первый", "Выбранный", "Следующий"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| QueueEntry {
                playlist: PlaylistKey::Local(42),
                playlist_revision: None,
                index,
                track: PlaylistTrack {
                    id: format!("audio-{index}"),
                    title: title.into(),
                    artist: "Приёмка".into(),
                    artists: Vec::new(),
                    artist_credits: Vec::new(),
                    album: None,
                    album_ref: None,
                    artwork: None,
                    added: None,
                    duration: Some(32),
                    media: Some(Media::File(audio.to_owned())),
                },
                recommendations: Vec::new(),
            })
            .collect();
        window.update(cx, |view, _, cx| {
            view.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| playback.play(queue, 1))
            });
        })?;
        loop {
            if window.update(cx, |view, _, cx| view.player.read(cx).lyrics_seekable())? {
                break;
            }
            wait(cx, deadline, "initial local playback").await;
        }
        window.update(cx, |view, _, cx| {
            view.player.update(cx, |bar, cx| bar.seek(SEEK_TO, cx))
        })?;
        loop {
            if window.update(cx, |view, _, cx| view.player.read(cx).position())? >= SEEK_TO {
                break;
            }
            wait(cx, deadline, "actual libmpv seek").await;
        }
        window.update(cx, |view, window, cx| {
            if view.close_after_saved_route(window, cx) {
                window.remove_window();
            }
        })?;
        cx.background_executor().timer(Duration::from_secs(5)).await;
        panic!("the native window did not close after the journal drained");
    }

    let position = persisted.unwrap().progress.position_ms as f64 / 1000.;
    assert!((SEEK_TO..9.).contains(&position));
    for _ in 0..4 {
        window.update(cx, |view, _, cx| {
            let bar = view.player.read(cx);
            assert!(bar.dormant());
            assert_eq!(bar.snapshot().status, Status::Idle);
            assert_eq!(bar.current_sound_source(), None);
            assert_eq!(bar.current_index(), Some(1));
            assert_eq!(bar.position(), position);
            assert_eq!(
                bar.entries()
                    .map(|entry| entry.track.title.as_str())
                    .collect::<Vec<_>>(),
                ["Первый", "Выбранный", "Следующий"]
            );
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
    }
    let tree: serde_json::Value =
        serde_json::from_str(&frame(window, cx, evidence, "startup-queue-idle")?)?;
    let play = tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| node["element_id"] == r#"Name("player-play")"#)
        .unwrap();
    assert_eq!(play["aria"]["label"], "Воспроизвести");
    assert!(
        play["aria"]["on_action"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "Click")
    );
    window.update(cx, |view, _, cx| {
        view.player
            .update(cx, |bar, cx| bar.command(cx, Playback::toggle_pause))
    })?;
    let resumed = loop {
        let (source, elapsed) = window.update(cx, |view, _, cx| {
            let bar = view.player.read(cx);
            (bar.current_sound_source(), bar.position())
        })?;
        if source == Some(SoundSource::LocalFile) && elapsed >= position {
            break elapsed;
        }
        wait(cx, deadline, "resume actual local audio").await;
    };
    assert!(resumed < position + 2.0);
    loop {
        let playing = window.update(cx, |view, _, cx| view.player.read(cx).position())?;
        if playing > resumed + 0.15 {
            break;
        }
        wait(cx, deadline, "resumed audio position advancing").await;
    }
    frame(window, cx, evidence, "startup-queue-playing")?;
    println!(
        "separate process held queue silent at {position:.3} s, then produced LocalFile audio at {resumed:.3} s"
    );
    Ok(())
}

async fn wait(cx: &AsyncApp, deadline: Instant, stage: &str) {
    assert!(Instant::now() < deadline, "timed out during {stage}");
    cx.background_executor()
        .timer(Duration::from_millis(15))
        .await;
}
