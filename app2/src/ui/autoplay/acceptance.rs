use std::{
    thread,
    time::{Duration, Instant},
};

use gpui::{TestAppContext, WindowHandle};

use crate::{
    appearance::Appearance,
    playback::{QueueEntry, SoundSource, Status},
    storage::Store,
};

use super::MainView;

pub(crate) fn check_automatic_queue(entries: Vec<QueueEntry>, peak: impl FnOnce() -> f32) {
    let unlinked = entries[0].clone();
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| {
        crate::ui::theme::install(cx, false);
        MainView::new(window, cx, false, Some(true), false, Appearance::classic())
    });
    window
        .update(&mut cx, |view, _, cx| {
            view.toggle_autoplay(cx);
            view.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| {
                    playback.set_volume(0.14);
                    playback.play(entries, 0);
                });
            });
        })
        .unwrap();
    for row in 0..=1 {
        wait(&mut cx, window, |view, cx| {
            let player = view.player.read(cx);
            let state = player.snapshot();
            if state.status != Status::Playing
                || state.queue_position != Some(row)
                || player.position() < 0.3
            {
                return None;
            }
            assert_eq!(player.queue_len(), 2);
            assert!(state.continuation.is_none());
            Some(())
        });
        window
            .update(&mut cx, |view, _, cx| {
                view.player.update(cx, |bar, cx| {
                    bar.command(cx, |playback| {
                        playback.seek(playback.duration().unwrap() - 0.4)
                    });
                })
            })
            .unwrap();
    }
    let (id, title, source, position) = wait(&mut cx, window, |view, cx| {
        let player = view.player.read(cx);
        let state = player.snapshot();
        if state.status != Status::Playing
            || player.position() < 0.5
            || state.queue_position != Some(2)
        {
            return None;
        }
        let first = player.entries().nth(2).unwrap();
        assert!(player.current() == Some(first));
        Some((
            first.track.id.clone(),
            first.recommendations[0].playlist.clone(),
            state.sound_source?,
            player.position(),
        ))
    });
    assert!(matches!(
        source,
        SoundSource::Stream(_) | SoundSource::Cached
    ));
    assert!(
        Store::open_default()
            .unwrap()
            .panel_state()
            .unwrap()
            .autoplay
    );
    let peak = peak();
    println!(
        "MainView autoplay enabled: row 3, first candidate {id} from {title:?}, {}, position {position:.3}s, peak {peak:.6}.",
        source.label()
    );

    window
        .update(&mut cx, |view, _, cx| {
            view.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| playback.play(vec![unlinked], 0));
            })
        })
        .unwrap();
    wait(&mut cx, window, |view, cx| {
        let player = view.player.read(cx);
        (player.snapshot().status == Status::Playing && player.position() >= 0.3).then_some(())
    });
    window
        .update(&mut cx, |view, _, cx| {
            view.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| {
                    playback.seek(playback.duration().unwrap() - 0.4)
                });
            })
        })
        .unwrap();
    wait(&mut cx, window, |view, cx| {
        let message = view.autoplay.notice.as_deref()?;
        let player = view.player.read(cx);
        (player.snapshot().status == Status::Idle
            && message == "Очередь закончилась. Рекомендаций пока нет.")
            .then(|| assert_eq!(player.queue_len(), 1))
    });
    println!("MainView autoplay empty state: queue preserved, no recommendations notice shown.");
}

fn wait<T>(
    cx: &mut TestAppContext,
    window: WindowHandle<MainView>,
    mut ready: impl FnMut(&MainView, &gpui::App) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        cx.executor().advance_clock(Duration::from_millis(175));
        cx.run_until_parked();
        let result = window
            .update(cx, |view, _, cx| {
                let status = view.player.read(cx).snapshot().status;
                assert!(!matches!(status, Status::Failed(_)), "{status:?}");
                ready(view, cx)
            })
            .unwrap();
        if let Some(result) = result {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "headless player did not reach the expected queue position"
        );
        thread::sleep(Duration::from_millis(30));
    }
}
