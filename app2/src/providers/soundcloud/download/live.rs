use std::{env, fs, io, path::Path};

use crate::{
    providers::{
        Credentials, StreamOffer, direct_stream, downloads, live_tests::audio::stage5_profile,
    },
    storage::Store,
};

use super::*;

const MAX_REFERENCES: usize = 20;
const MAX_AUDIO_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DOWNLOADS: usize = 3;

#[test]
#[ignore = "requires an isolated snapshot of a connected SoundCloud profile and public track references"]
fn live_stage5_soundcloud_original_candidates() {
    let profile = stage5_profile();
    let store = Store::open(profile.path()).expect("isolated SoundCloud profile");
    let account = store
        .preferred_account(PROVIDER)
        .expect("SoundCloud account lookup")
        .expect("connected SoundCloud account");
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("SoundCloud credentials");
    let Credentials::SoundCloud { token, client_id } = &credentials else {
        panic!("expected SoundCloud credentials");
    };
    let references = env::var_os("MEDIADEKA_SOUNDCLOUD_ORIGINAL_REFERENCES")
        .expect("path to a file of SoundCloud public track URLs or numeric IDs, one per line");
    let references =
        fs::read_to_string(references).expect("read public SoundCloud track references");
    let destination =
        env::var_os("MEDIADEKA_SOUNDCLOUD_ORIGINAL_EVIDENCE").map(std::path::PathBuf::from);
    let cancelled = AtomicBool::new(false);
    let agent = agent();
    let mut checked = 0;
    let mut available = 0;
    let mut downloaded = 0;
    for reference in references
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(MAX_REFERENCES)
    {
        let id = reference
            .strip_prefix("https://soundcloud.com/")
            .unwrap_or(reference);
        let snapshot = match track_snapshot(&agent, id, token, client_id) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                println!("{reference}: {error}");
                continue;
            }
        };
        checked += 1;
        let title = snapshot["title"].as_str().unwrap_or("?");
        let artist = snapshot["user"]["username"].as_str().unwrap_or("?");
        let license = snapshot["license"].as_str().unwrap_or("unknown");
        let public = snapshot["permalink_url"].as_str().unwrap_or(reference);
        println!(
            "{title} · {artist} · {public} · ID {} · duration_ms={} · license={license}, sharing={}, downloadable={}, downloads_left={}, policy={}",
            snapshot["id"],
            snapshot["full_duration"]
                .as_u64()
                .or_else(|| snapshot["duration"].as_u64())
                .unwrap_or_default(),
            snapshot["sharing"],
            snapshot["downloadable"],
            snapshot["has_downloads_left"],
            snapshot["policy"],
        );
        let Some(id) = downloadable_id(&snapshot) else {
            continue;
        };
        available += 1;
        if !(license.starts_with("cc-") || license == "cc0")
            || snapshot["original_content_size"]
                .as_u64()
                .is_some_and(|bytes| bytes > MAX_AUDIO_BYTES)
        {
            continue;
        }
        let Some(destination) = destination.as_ref().filter(|_| downloaded < MAX_DOWNLOADS) else {
            continue;
        };
        let id = id.to_string();
        let stream = match direct_stream(PROVIDER, Some(&credentials), &id, &cancelled) {
            Ok(StreamOffer::Full(stream)) => {
                format!("full {:?} {:?}", stream.codec, stream.bitrate_kbps)
            }
            Ok(StreamOffer::Preview) => "preview".to_owned(),
            Err(error) => error.to_string(),
        };
        println!("  Direct stream: {stream}");
        match downloads::original(&store, PROVIDER, &account.profile.id, &id, &cancelled)
            .and_then(|candidate| save(&agent, &candidate, &id, destination))
        {
            Ok(()) => downloaded += 1,
            Err(error) => println!("  Original: {error}"),
        }
    }
    println!(
        "SoundCloud checked={checked}, public originals authorized={available}, saved={downloaded}"
    );
    assert!(checked > 0, "no SoundCloud track metadata could be read");
    if destination.is_some() {
        assert!(downloaded > 0, "no permitted original transfer completed");
    }
}

fn save(
    agent: &ureq::Agent,
    candidate: &DownloadCandidate,
    id: &str,
    destination: &Path,
) -> Result<(), String> {
    assert!(
        candidate.headers.is_empty(),
        "SoundCloud CDN must not receive account headers"
    );
    let mut response = agent
        .get(candidate.url.as_str())
        .header(ACCEPT_ENCODING, "identity")
        .call()
        .map_err(|_| "file server did not respond")?;
    if response.status() != ureq::http::StatusCode::OK {
        return Err(format!("file server returned HTTP {}", response.status()));
    }
    let format = candidate.format.as_deref().unwrap_or("audio");
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut temp =
        tempfile::NamedTempFile::new_in(destination).map_err(|error| error.to_string())?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_AUDIO_BYTES)
        .reader();
    let bytes = io::copy(&mut reader, &mut temp).map_err(|error| error.to_string())?;
    if bytes == 0 || candidate.bytes.is_some_and(|expected| bytes != expected) {
        return Err(format!(
            "original size differs: received {bytes}, expected {:?}",
            candidate.bytes
        ));
    }
    let output = destination.join(format!("{id}.original.{format}"));
    temp.persist(&output)
        .map_err(|error| error.error.to_string())?;
    println!(
        "  Original: {bytes} bytes, {format}, offer bitrate={:?}, {}",
        candidate.bitrate_bps,
        output.display()
    );
    Ok(())
}
