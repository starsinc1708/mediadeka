mod address;
mod http;
pub(super) mod jellyfin;
mod metadata;
mod musicbrainz;
pub(super) mod short;

use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::Value;
use url::Url;

use super::{
    ConfirmedLink, MusicPage,
    ObjectKind::{self, Album, Artist, Track},
    now, same_object,
};
use http::LookupError;

pub(crate) use address::{PublicLink, public_link};
pub(crate) use metadata::{PublicMetadata, public_metadata};

#[derive(Default)]
pub(crate) struct Resolution {
    pub(crate) links: Vec<ConfirmedLink>,
    pub(crate) retry_after: Option<Duration>,
}

impl Resolution {
    fn defer(&mut self, error: LookupError) {
        if let LookupError::Temporary(delay) = error {
            self.retry_after = Some(
                self.retry_after
                    .map_or(delay, |previous| previous.max(delay)),
            );
        }
    }
}

/// The caller persists the outcome within its record and region scope.
pub(crate) fn resolve(
    page: &MusicPage,
    known: &[ConfirmedLink],
    region: &str,
    cancelled: &AtomicBool,
) -> Resolution {
    let kind = page.reference.kind;
    if !matches!(kind, Track | Album | Artist) || page.title.trim().is_empty() {
        return Resolution::default();
    }
    let direct = |url: &str| public_link(url).filter(|link| link.kind == kind);
    let mut inputs: Vec<_> = page
        .url
        .as_deref()
        .and_then(direct)
        .or_else(|| page.reference.url().as_deref().and_then(direct))
        .into_iter()
        .collect();
    for link in known
        .iter()
        .filter_map(|link| direct(&link.url).filter(|address| address.platform == link.platform))
    {
        if !inputs.iter().any(|input| input.same_address(&link)) {
            inputs.push(link);
        }
    }
    if inputs.is_empty() {
        return Resolution::default();
    }
    let mut covered: HashSet<_> = inputs.iter().map(|input| input.platform.clone()).collect();
    let mut checked = HashSet::new();
    let mut outcome = Resolution::default();
    let client = http::client();
    for resolver in [Resolver::Lab86, Resolver::Musiccloud] {
        if cancelled.load(Ordering::Relaxed) || !resolver.covers_missing(kind, &covered) {
            continue;
        }
        let response = resolver.request(&client, &inputs, cancelled);
        let data = match response {
            Ok(data) => data,
            Err(error) => {
                outcome.defer(error);
                continue;
            }
        };
        let candidates = candidates(resolver, &data, kind);
        for candidate in candidates {
            if covered.contains(&candidate.link.platform)
                || !metadata::covers(&candidate.link.platform, kind)
                || !checked.insert((candidate.link.platform.clone(), candidate.link.id.clone()))
            {
                continue;
            }
            let link = metadata::in_region(candidate.link, region);
            match metadata::read(&client, &link, region, cancelled) {
                Ok(metadata) => {
                    let mut found = metadata.into_page(page.reference.clone());
                    attach_target_isrc(
                        &mut found,
                        candidate.isrc,
                        candidate.reported_title,
                        candidate.reported_artists,
                        candidate.reported_duration,
                    );
                    if let Some(evidence) = same_object(page, &found) {
                        outcome.links.push(ConfirmedLink {
                            platform: link.platform.clone(),
                            id: link.id.clone(),
                            url: link.url.clone(),
                            evidence: format!(
                                "{} + {} · {evidence}",
                                resolver.name(),
                                metadata::evidence_source(&link.platform)
                            ),
                            checked_at: now(),
                        });
                        covered.insert(link.platform.clone());
                        inputs.push(link);
                    }
                }
                Err(error) => outcome.defer(error),
            }
        }
    }
    outcome
}

/// Lab86 reports target metadata obtained using that target's catalogue. For Apple tracks
/// the public page exposes the title, performers and duration, while Lab86 can read ISRC.
fn attach_target_isrc(
    found: &mut MusicPage,
    isrc: Option<String>,
    title: Option<String>,
    artists: Vec<String>,
    duration: Option<u32>,
) {
    if found.isrc.is_some() || isrc.is_none() {
        return;
    }
    let Some(title) = title else { return };
    let mut reported = MusicPage::new(found.reference.clone(), title);
    reported.artists = artists;
    reported.duration = duration;
    if same_object(found, &reported).is_some() {
        found.isrc = isrc;
    }
}

struct Candidate {
    link: PublicLink,
    isrc: Option<String>,
    reported_title: Option<String>,
    reported_artists: Vec<String>,
    reported_duration: Option<u32>,
}

fn candidates(resolver: Resolver, data: &Value, kind: ObjectKind) -> Vec<Candidate> {
    if data["type"].as_str() != Some(kind.path()) {
        return Vec::new();
    }
    data["links"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            if (resolver == Resolver::Lab86 && row["kind"] != "direct")
                || row["matchMethod"] == "search-fallback"
            {
                return None;
            }
            let link = public_link(row["url"].as_str()?)?;
            if link.kind != kind || !service_matches(&link, row["service"].as_str()?) {
                return None;
            }
            let trusted_metadata = resolver == Resolver::Lab86
                && row["metadata"]["type"].as_str() == Some(kind.path())
                && row["metadata"]["url"]
                    .as_str()
                    .and_then(public_link)
                    .is_some_and(|reported| link.same_address(&reported));
            let (isrc, reported_title, reported_artists, reported_duration) = if trusted_metadata {
                let data = &row["metadata"];
                (
                    metadata::text(&data["isrc"]),
                    metadata::text(&data["title"]),
                    metadata::text(&data["artist"]).into_iter().collect(),
                    data["duration"]
                        .as_u64()
                        .and_then(|ms| u32::try_from(ms).ok()),
                )
            } else {
                (None, None, Vec::new(), None)
            };
            Some(Candidate {
                link,
                isrc,
                reported_title,
                reported_artists,
                reported_duration,
            })
        })
        .take(MAX_CANDIDATES)
        .collect()
}

const MAX_CANDIDATES: usize = 20;

fn service_matches(link: &PublicLink, service: &str) -> bool {
    let service = match service {
        "apple" => "apple-music",
        "youtube" => "youtube-music",
        "amazon" => "amazon-music",
        other => other,
    };
    link.platform == service
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Resolver {
    Lab86,
    Musiccloud,
}

impl Resolver {
    fn name(self) -> &'static str {
        match self {
            Self::Lab86 => "Lab86",
            Self::Musiccloud => "musiccloud",
        }
    }

    fn supports_source(self, platform: &str) -> bool {
        match self {
            Self::Lab86 => matches!(
                platform,
                "spotify" | "apple-music" | "deezer" | "tidal" | "youtube-music" | "amazon-music"
            ),
            Self::Musiccloud => matches!(
                platform,
                "spotify"
                    | "apple-music"
                    | "deezer"
                    | "tidal"
                    | "youtube-music"
                    | "qobuz"
                    | "soundcloud"
                    | "amazon-music"
            ),
        }
    }

    fn covers_missing(self, kind: ObjectKind, covered: &HashSet<String>) -> bool {
        let verifiable: &[_] = match (self, kind) {
            (_, Track) => &["apple-music", "deezer", "tidal"],
            (Self::Musiccloud, Album) => &["apple-music", "deezer", "qobuz"],
            (_, Album | Artist) => &["apple-music", "deezer"],
            _ => &[],
        };
        verifiable
            .iter()
            .any(|platform| !covered.contains(*platform))
    }

    fn request(
        self,
        client: &ureq::Agent,
        inputs: &[PublicLink],
        cancelled: &AtomicBool,
    ) -> http::Result<Value> {
        let mut inputs = inputs
            .iter()
            .filter(|input| self.supports_source(&input.platform));
        let input = inputs.next().ok_or(LookupError::Unsupported)?;
        let result = self.query(client, input, cancelled);
        if result == Err(LookupError::Unsupported)
            && let Some(alternative) =
                inputs.find(|alternative| alternative.platform != input.platform)
        {
            return self.query(client, alternative, cancelled);
        }
        result
    }

    fn query(
        self,
        client: &ureq::Agent,
        input: &PublicLink,
        cancelled: &AtomicBool,
    ) -> http::Result<Value> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(LookupError::Cancelled);
        }
        let (base, parameter, budget) = match self {
            Self::Lab86 => (
                "https://music.lab86.io/api/convert/link",
                "url",
                &LAB86_BUDGET,
            ),
            Self::Musiccloud => (
                "https://api.musiccloud.io/api/v1/resolve",
                "query",
                &MUSICCLOUD_BUDGET,
            ),
        };
        let request = Url::parse_with_params(base, [(parameter, &input.url)])
            .map_err(|_| LookupError::Unsupported)?;
        {
            let mut budget = budget
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            budget.reserve(Instant::now(), self)?;
        }
        let response = http::json(client, request.as_str(), cancelled);
        if let Err(LookupError::Temporary(delay)) = response {
            let mut budget = budget
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let until = Instant::now() + delay;
            budget.until = Some(budget.until.map_or(until, |previous| previous.max(until)));
        }
        response
    }
}

// Aggregate quotas belong to the service, independently of the page or account using them.
static LAB86_BUDGET: Mutex<Budget> = Mutex::new(Budget {
    until: None,
    calls: VecDeque::new(),
});
static MUSICCLOUD_BUDGET: Mutex<Budget> = Mutex::new(Budget {
    until: None,
    calls: VecDeque::new(),
});

#[derive(Default)]
struct Budget {
    until: Option<Instant>,
    calls: VecDeque<Instant>,
}

impl Budget {
    fn reserve(&mut self, now: Instant, resolver: Resolver) -> http::Result<()> {
        if let Some(delay) = self
            .until
            .and_then(|until| until.checked_duration_since(now))
            && !delay.is_zero()
        {
            return Err(LookupError::Temporary(delay));
        }
        if resolver == Resolver::Lab86 {
            return Ok(());
        }
        const MINUTE: Duration = Duration::from_secs(60);
        const DAY: Duration = Duration::from_secs(86_400);
        const MINUTE_QUOTA: usize = 10;
        const DAY_QUOTA: usize = 500;
        while self
            .calls
            .front()
            .is_some_and(|call| now.duration_since(*call) >= DAY)
        {
            self.calls.pop_front();
        }
        if self.calls.len() >= DAY_QUOTA
            && let Some(call) = self.calls.front()
        {
            return Err(LookupError::Temporary(
                DAY.saturating_sub(now.duration_since(*call)),
            ));
        }
        if let Some(index) = self.calls.len().checked_sub(MINUTE_QUOTA)
            && let Some(call) = self.calls.get(index)
            && now.duration_since(*call) < MINUTE
        {
            return Err(LookupError::Temporary(MINUTE - now.duration_since(*call)));
        }
        self.calls.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{music::ObjectRef, providers::ProviderId};
    use serde_json::json;

    #[test]
    fn aggregator_hints_cannot_turn_searches_or_user_playlists_into_albums() {
        let data = json!({"type":"album", "links":[
            {"kind":"direct","service":"youtube","confidence":100,
             "url":"https://music.youtube.com/playlist?list=PLZ60rAdTLVNuCAWD8W3dZTLBXbSPda-DX"},
            {"kind":"search","service":"deezer","url":"https://www.deezer.com/search/random"},
            {"kind":"direct","service":"deezer","url":"https://www.deezer.com/album/6575789"},
            {"kind":"direct","service":"apple","url":"https://www.deezer.com/album/6575789"}
        ]});
        let candidates = candidates(Resolver::Lab86, &data, Album);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].link.id, "6575789");
    }

    #[test]
    fn external_quota_is_shared_across_objects_and_reopens_after_one_minute() {
        let mut budget = Budget::default();
        let now = Instant::now();
        for _ in 0..10 {
            assert_eq!(budget.reserve(now, Resolver::Musiccloud), Ok(()));
        }
        assert_eq!(
            budget.reserve(now, Resolver::Musiccloud),
            Err(LookupError::Temporary(Duration::from_secs(60)))
        );
        assert_eq!(
            budget.reserve(now + Duration::from_secs(60), Resolver::Musiccloud),
            Ok(())
        );
    }

    #[test]
    #[ignore = "public network and regional catalog availability"]
    fn lab86_candidates_are_checked_against_live_deezer_and_tidal() {
        let reference = ObjectRef {
            provider: ProviderId::Spotify,
            kind: Track,
            id: "1E96rC1JYgZoPPw8kvxd1I".into(),
        };
        let mut page = MusicPage::new(reference, "ПЕРВЫЙ РАЗ".into());
        page.artists = vec!["GATASKI".into()];
        page.isrc = Some("FRX452603197".into());
        page.duration = Some(175_500);
        let result = resolve(&page, &[], "US", &AtomicBool::new(false));
        for link in &result.links {
            eprintln!("{} {} {}", link.platform, link.id, link.evidence);
        }
        assert!(
            result
                .links
                .iter()
                .any(|link| link.platform == "deezer" && link.id == "4199566742")
        );
        assert!(
            result
                .links
                .iter()
                .any(|link| link.platform == "tidal" && link.id == "548933173")
        );
    }

    #[test]
    #[ignore = "public network"]
    fn musiccloud_qobuz_candidate_is_linked_to_a_live_musicbrainz_edition() {
        let source = public_link("https://open.spotify.com/album/4m2880jivSbbyEGAKfITCa").unwrap();
        let cancelled = AtomicBool::new(false);
        let client = http::client();
        let response = Resolver::Musiccloud
            .request(&client, &[source], &cancelled)
            .unwrap();
        let target = candidates(Resolver::Musiccloud, &response, Album)
            .into_iter()
            .find(|candidate| candidate.link.platform == "qobuz")
            .unwrap()
            .link;
        let metadata = metadata::read(&client, &target, "US", &cancelled).unwrap();
        assert_eq!(target.id, "0886443927087");
        assert_eq!(metadata.barcode.as_deref(), Some("886443927087"));
        assert_eq!(metadata.title, "Random Access Memories");
        assert_eq!(metadata.artists, ["Daft Punk"]);
        assert_eq!(metadata.tracks.len(), 13);
        eprintln!(
            "musiccloud → Qobuz {} → MusicBrainz UPC {} · {} ordered tracks",
            target.id,
            metadata.barcode.unwrap(),
            metadata.tracks.len()
        );
    }
}
