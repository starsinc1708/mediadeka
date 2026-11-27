use std::sync::atomic::{AtomicBool, Ordering};

use url::Url;

use super::{
    PublicLink,
    address::{AddressError, from_https_url, parse_https},
    http, public_link,
};

const MAX_REDIRECTS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShortLinkError {
    NotShort,
    ForeignHost,
    UnsafeAddress,
    TooManyRedirects,
    Network,
    NoObject,
    Cancelled,
}

pub(crate) fn resolve(value: &str, cancelled: &AtomicBool) -> Result<PublicLink, ShortLinkError> {
    let agent = http::client();
    resolve_with(value, cancelled, |url| {
        let response = agent
            .get(url.as_str())
            .call()
            .map_err(|_| ShortLinkError::Network)?;
        let status = response.status();
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(ShortLinkError::Network);
        }
        if !status.is_redirection() {
            return Ok(None);
        }
        Ok(response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned))
    })
}

fn resolve_with(
    value: &str,
    cancelled: &AtomicBool,
    mut request: impl FnMut(&Url) -> Result<Option<String>, ShortLinkError>,
) -> Result<PublicLink, ShortLinkError> {
    let mut current = parse_https(value, None)
        .filter(is_short_host)
        .ok_or(ShortLinkError::NotShort)?;
    for _ in 0..MAX_REDIRECTS {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ShortLinkError::Cancelled);
        }
        let location = request(&current);
        if cancelled.load(Ordering::Relaxed) {
            return Err(ShortLinkError::Cancelled);
        }
        let location = location?.ok_or(ShortLinkError::NoObject)?;
        current = next_hop(&current, &location)?;
        if !is_short_host(&current) {
            return public_link(current.as_str()).ok_or(ShortLinkError::NoObject);
        }
    }
    Err(ShortLinkError::TooManyRedirects)
}

fn next_hop(current: &Url, location: &str) -> Result<Url, ShortLinkError> {
    if location.trim().is_empty() {
        return Err(ShortLinkError::NoObject);
    }
    let next = parse_https(location, Some(current)).ok_or(ShortLinkError::UnsafeAddress)?;
    if !is_short_host(&next) {
        from_https_url(&next).map_err(|error| match error {
            AddressError::UnknownHost => ShortLinkError::ForeignHost,
            AddressError::NoObject => ShortLinkError::NoObject,
        })?;
    }
    Ok(next)
}

fn is_short_host(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some(
            "spotify.link"
                | "spoti.fi"
                | "on.soundcloud.com"
                | "deezer.page.link"
                | "link.deezer.com"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShortLinkError::*;

    const START: &str = "https://spoti.fi/start";
    const TRACK: &str = "https://open.spotify.com/track/1E96rC1JYgZoPPw8kvxd1I";
    const REJECTED_HOPS: &[(&str, ShortLinkError)] = &[
        ("https://outside.invalid/recording", ForeignHost),
        ("https://spoti.fi.example.com/hop", ForeignHost),
        ("http://spoti.fi/hop", UnsafeAddress),
        ("https://spoti.fi:444/hop", UnsafeAddress),
        ("//spoti.fi:443/hop", UnsafeAddress),
        ("https://user:password@spoti.fi/hop", UnsafeAddress),
        ("//user@spoti.fi/hop", UnsafeAddress),
        ("https://@spoti.fi/hop", UnsafeAddress),
        ("https://open.spotify.com/search/recording", NoObject),
        (
            "https://soundcloud.com/artist/track?secret_token=s-test",
            NoObject,
        ),
        ("", NoObject),
    ];

    #[test]
    fn next_hop_accepts_relative_and_public_destinations() -> Result<(), ShortLinkError> {
        let current = parse_https(START, None).ok_or(UnsafeAddress)?;
        for (location, expected) in [
            ("hop", "https://spoti.fi/hop"),
            ("//on.soundcloud.com/hop", "https://on.soundcloud.com/hop"),
            (TRACK, TRACK),
        ] {
            assert_eq!(next_hop(&current, location)?.as_str(), expected);
        }
        Ok(())
    }

    #[test]
    fn next_hop_rejects_unsafe_or_unrecognized_destinations() -> Result<(), ShortLinkError> {
        let current = parse_https(START, None).ok_or(UnsafeAddress)?;
        for &(location, error) in REJECTED_HOPS {
            assert_eq!(next_hop(&current, location), Err(error), "{location}");
        }
        Ok(())
    }

    #[test]
    fn resolves_a_public_object_on_the_third_redirect_without_requesting_it() {
        let mut requested = Vec::new();
        let result = resolve_with(START, &AtomicBool::new(false), |url| {
            requested.push(url.to_string());
            let location = match url.path() {
                "/start" => "hop",
                "/hop" => "https://on.soundcloud.com/last",
                _ => TRACK,
            };
            Ok(Some(location.into()))
        });
        assert_eq!(result.as_ref().map(|link| link.url.as_str()), Ok(TRACK));
        assert_eq!(
            requested,
            [
                START,
                "https://spoti.fi/hop",
                "https://on.soundcloud.com/last",
            ]
        );
    }

    #[test]
    fn invalid_redirects_stop_the_chain_before_the_next_request() {
        for &(location, error) in REJECTED_HOPS {
            let mut requested = Vec::new();
            let result = resolve_with(START, &AtomicBool::new(false), |url| {
                requested.push(url.to_string());
                Ok(Some(location.into()))
            });
            assert_eq!(result, Err(error), "{location}");
            assert_eq!(requested, [START], "{location}");
        }
    }

    #[test]
    fn non_short_inputs_do_not_use_the_transport() {
        for input in [
            TRACK,
            "https://outside.invalid/start",
            "http://spoti.fi/start",
            "https://spoti.fi:443/start",
            "https://spoti.fi@outside.invalid/start",
        ] {
            let mut calls = 0;
            let result = resolve_with(input, &AtomicBool::new(false), |_| {
                calls += 1;
                Ok(None)
            });
            assert_eq!(result, Err(NotShort), "{input}");
            assert_eq!(calls, 0, "{input}");
        }
    }

    #[test]
    fn stops_after_three_redirects_to_short_hosts() {
        let mut requested = Vec::new();
        let result = resolve_with(START, &AtomicBool::new(false), |url| {
            requested.push(url.to_string());
            Ok(Some(format!("/{}", requested.len())))
        });
        assert_eq!(result, Err(TooManyRedirects));
        assert_eq!(
            requested,
            [START, "https://spoti.fi/1", "https://spoti.fi/2"]
        );
    }

    #[test]
    fn cancelled_chain_does_not_start_or_make_a_second_request() {
        for (initially_cancelled, expected_calls) in [(true, 0), (false, 1)] {
            let cancelled = AtomicBool::new(initially_cancelled);
            let mut calls = 0;
            let result = resolve_with(START, &cancelled, |_| {
                calls += 1;
                cancelled.store(true, Ordering::Relaxed);
                Ok(Some("/hop".into()))
            });
            assert_eq!(result, Err(Cancelled));
            assert_eq!(calls, expected_calls);
        }
    }

    #[test]
    fn distinguishes_transport_failure_and_response_without_location() {
        let cancelled = AtomicBool::new(false);
        assert_eq!(
            resolve_with(START, &cancelled, |_| Err(Network)),
            Err(Network)
        );
        assert_eq!(resolve_with(START, &cancelled, |_| Ok(None)), Err(NoObject));
    }
}
