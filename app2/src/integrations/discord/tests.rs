use super::*;
#[cfg(windows)]
use serde_json::Value;
use serde_json::json;

const APPLICATION_ID: &str = "123456789012345678";
const PUBLIC_COVER: &str = "https://i.scdn.co/image/cover";

fn track(title: &str, cover: Option<&str>) -> PlaylistTrack {
    PlaylistTrack {
        id: title.into(),
        title: title.into(),
        artist: "Исполнитель".into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: Some("Альбом".into()),
        artwork: cover.map(str::to_owned),
        added: None,
        duration: Some(120),
        media: None,
    }
}

#[test]
fn fit_text_obeys_utf16_length() {
    assert_eq!(fit_text(" \t "), None);
    assert_eq!(fit_text(" ツ ").as_deref(), Some("ツ\u{a0}"));
    let wide = "🙂".repeat(64);
    assert_eq!(fit_text(&(wide.clone() + "🙂")), Some(wide));
}

#[test]
fn activity_only_uses_public_artwork_and_omits_hidden_fields() {
    let public = Activity::track(APPLICATION_ID, &track("Трек", Some(PUBLIC_COVER)), None);
    let value = serde_json::to_value(public).unwrap();
    assert_eq!(value["assets"]["large_image"], PUBLIC_COVER);
    for cover in [
        "file:///C:/artwork.jpg",
        "https://127.0.0.1/Items/1/Images/Primary",
        "https://images.example.test/Items/1/Images/Primary",
        "https://i.scdn.co/image/cover?api_key=secret",
        "https://example:secret@i.scdn.co/image/cover",
    ] {
        let activity = Activity::track(APPLICATION_ID, &track("Трек", Some(cover)), None);
        let value = serde_json::to_value(activity).unwrap();
        assert!(value.get("assets").is_none());
        assert!(value.get("timestamps").is_none());
        assert!(value.get("application_id").is_none());
    }

    let hidden = Activity::anonymous(APPLICATION_ID, Some(1_760_000_000));
    assert_eq!(
        serde_json::to_value(hidden).unwrap(),
        json!({"type": 2, "details": "Слушает музыку", "timestamps": {"start": 1_760_000_000}})
    );
}

#[cfg(windows)]
#[test]
fn windows_pipe_receives_track_seek_pause_privacy_and_clear() {
    use std::time::Duration;

    use tokio::{net::windows::named_pipe::ServerOptions, runtime::Builder, time::timeout};

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let path = format!(
            r"\\?\pipe\mediadeka-discord-test-{}-{}",
            std::process::id(),
            u64::from_ne_bytes(random)
        );
        let mut pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&path)
            .unwrap();
        let mut worker = Discord::for_pipe(path);
        let start = 1_760_000_000;
        let one = track("Первый трек", Some(PUBLIC_COVER));
        let two = track("Второй трек", None);
        worker.publish(Some(Activity::track(
            APPLICATION_ID,
            &one,
            Some((start, Some(start + 120))),
        )));

        timeout(Duration::from_secs(10), async {
            handshake(&mut pipe).await;
            // Socket control traffic can precede the acknowledgement to an activity.
            ipc::write_json(&mut pipe, 3, &json!({"ping": "fixture"}))
                .await
                .unwrap();
            let first = command(&mut pipe).await;
            let (opcode, pong) = ipc::read_frame(&mut pipe).await.unwrap();
            assert_eq!(opcode, 4);
            assert_eq!(
                serde_json::from_slice::<Value>(&pong).unwrap(),
                json!({"ping": "fixture"})
            );
            assert_eq!(first["details"], "Первый трек");
            assert_eq!(first["state"], "Исполнитель");
            assert_eq!(first["assets"]["large_image"], PUBLIC_COVER);
            assert_eq!(
                first["timestamps"],
                json!({"start":start,"end":start + 120})
            );

            worker.publish(Some(Activity::track(
                APPLICATION_ID,
                &two,
                Some((start, Some(start + 120))),
            )));
            assert_eq!(command(&mut pipe).await["details"], "Второй трек");

            worker.publish(Some(Activity::track(
                APPLICATION_ID,
                &two,
                Some((start - 53, Some(start + 67))),
            )));
            assert_eq!(
                command(&mut pipe).await["timestamps"],
                json!({"start":start - 53,"end":start + 67})
            );

            worker.publish(Some(Activity::track(APPLICATION_ID, &two, None)));
            let paused = command(&mut pipe).await;
            assert_eq!(paused["details"], "Второй трек");
            assert!(paused.get("timestamps").is_none());

            worker.publish(Some(Activity::anonymous(APPLICATION_ID, None)));
            assert_eq!(
                command(&mut pipe).await,
                json!({"type": 2, "details":"Слушает музыку"})
            );
            worker.publish(None);
            assert_eq!(command(&mut pipe).await, Value::Null);

            pipe.disconnect().unwrap();
            worker.publish(Some(Activity::track(APPLICATION_ID, &one, None)));
            handshake(&mut pipe).await;
            assert_eq!(command(&mut pipe).await["details"], "Первый трек");
            drop(worker);
            assert_eq!(command(&mut pipe).await, Value::Null);
        })
        .await
        .expect("Discord fixture reached each stage within 10 seconds");
    });
}

#[cfg(windows)]
async fn handshake(pipe: &mut tokio::net::windows::named_pipe::NamedPipeServer) {
    pipe.connect().await.unwrap();
    let (opcode, body) = ipc::read_frame(pipe).await.unwrap();
    assert_eq!(opcode, 0);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"v":1, "client_id":APPLICATION_ID})
    );
    ipc::write_json(pipe, 1, &json!({"cmd":"DISPATCH","evt":"READY"}))
        .await
        .unwrap();
}

#[cfg(windows)]
async fn command(pipe: &mut tokio::net::windows::named_pipe::NamedPipeServer) -> Value {
    let (opcode, body) = ipc::read_frame(pipe).await.unwrap();
    assert_eq!(opcode, 1);
    let request: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["cmd"], "SET_ACTIVITY");
    assert_eq!(request["args"]["pid"], std::process::id());
    assert!(request["args"].get("activity").is_some());
    ipc::write_json(
        pipe,
        1,
        &json!({"cmd":"SET_ACTIVITY","nonce": request["nonce"],"evt":null}),
    )
    .await
    .unwrap();
    request["args"]["activity"].clone()
}
