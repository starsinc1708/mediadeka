use std::time::{SystemTime, UNIX_EPOCH};

use ureq::http::HeaderMap;

pub(crate) const MAX_AGE: i64 = 7 * 24 * 60 * 60;

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_secs().try_into().unwrap_or(i64::MAX)
        })
}

fn epoch(value: &ureq::http::HeaderValue) -> Option<i64> {
    httpdate::parse_http_date(value.to_str().ok()?)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs()
        .try_into()
        .ok()
}

pub(crate) fn response_max_age(headers: &HeaderMap) -> Option<i64> {
    let now = now();
    let date = headers.get("date").and_then(epoch).unwrap_or(now);
    let age = headers
        .get("age")
        .and_then(|value| value.to_str().ok())
        .and_then(|age| age.parse::<i64>().ok())
        .unwrap_or_default()
        .max(now.saturating_sub(date))
        .max(0);
    let mut max_age = None;
    for value in headers.get_all("cache-control") {
        for directive in value.to_str().ok()?.split(',').map(str::trim) {
            let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
            if ["no-store", "no-cache", "must-revalidate"]
                .iter()
                .any(|unavailable| name.eq_ignore_ascii_case(unavailable))
            {
                return None;
            }
            if name.eq_ignore_ascii_case("max-age") {
                let limit = value.trim_matches('"').parse::<i64>().ok()?;
                max_age = Some(max_age.map_or(limit, |old: i64| old.min(limit)));
            }
        }
    }
    let limit = max_age
        .or_else(|| {
            headers
                .get("expires")
                .map(|value| epoch(value).unwrap_or(0).saturating_sub(date))
        })
        .unwrap_or(MAX_AGE)
        .saturating_sub(age)
        .min(MAX_AGE);
    (limit > 0).then_some(limit)
}
