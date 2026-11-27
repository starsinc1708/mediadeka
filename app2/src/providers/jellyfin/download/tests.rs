use std::{env, fs};

use ureq::http::{StatusCode, header};

use crate::{
    providers::{ProviderId, downloads},
    storage::Store,
};

use super::*;

#[test]
#[ignore = "uses an isolated Jellyfin server with an indexed original audio file"]
fn live_jellyfin_download_preserves_original_bytes_and_supports_ranges() {
    let fixture = |name: &str| env::var(name).unwrap_or_else(|_| panic!("missing {name}"));
    let server = fixture("MEDIADEKA_TEST_JELLYFIN_URL");
    let user = fixture("MEDIADEKA_TEST_JELLYFIN_USER");
    let password = fixture("MEDIADEKA_TEST_JELLYFIN_PASSWORD");
    let item = fixture("MEDIADEKA_TEST_JELLYFIN_ITEM");
    let expected = fs::read(fixture("MEDIADEKA_TEST_JELLYFIN_ORIGINAL_FILE")).unwrap();
    assert!(expected.len() > 128);
    let session = super::super::sign_in(&server, &user, &password).expect("fixture sign-in");
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.connect_account(&session, || Some(()), || {}).unwrap();
    let candidate = downloads::original(
        &store,
        ProviderId::Jellyfin,
        &session.profile.id,
        &item,
        &AtomicBool::new(false),
    );
    store
        .disconnect_account(ProviderId::Jellyfin, &session.profile.id, || Some(()))
        .unwrap();
    let candidate = candidate.unwrap();
    assert!(candidate.url.query().is_none());
    assert!(candidate.headers[AUTHORIZATION].is_sensitive());
    assert_eq!(candidate.format.as_deref(), Some("mp3"));
    assert!(candidate.bitrate_bps.is_some() && candidate.sample_rate_hz.is_some());
    assert!(
        candidate
            .bytes
            .is_none_or(|hint| hint == expected.len() as u64)
    );

    let agent = agent();
    let denied = agent.get(candidate.url.as_str()).call().unwrap().status();
    assert!(denied.is_client_error());
    let mut response = agent
        .get(candidate.url.as_str())
        .header(AUTHORIZATION, &candidate.headers[AUTHORIZATION])
        .call()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body_mut().read_to_vec().unwrap(), expected);

    let mut range = agent
        .get(candidate.url.as_str())
        .header(AUTHORIZATION, &candidate.headers[AUTHORIZATION])
        .header(header::RANGE, "bytes=9-23")
        .call()
        .unwrap();
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        range.headers()[header::CONTENT_RANGE],
        format!("bytes 9-23/{}", expected.len())
    );
    assert_eq!(range.body_mut().read_to_vec().unwrap(), expected[9..24]);
    println!(
        "Jellyfin authenticated original matched {} bytes; range returned 15 matching bytes; ETag present: {}; metadata size: {:?}",
        expected.len(),
        range.headers().contains_key(header::ETAG),
        candidate.bytes,
    );
}
