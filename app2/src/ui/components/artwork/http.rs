use std::{io::Read as _, sync::LazyLock, time::Duration};

use gpui::{Image, ImageFormat};
use serde::{Deserialize, Serialize};
use ureq::http::{HeaderMap, StatusCode, header};

use crate::{providers::is_public_artwork_url, telemetry};

use super::RESPONSE_LIMIT;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(8);
const HEADER_LIMIT: usize = 256;

static CLIENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .max_redirects(0)
        .build()
        .new_agent()
});

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct Validators {
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Validators {
    fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            etag: headers
                .get(header::ETAG)
                .and_then(|header| header.to_str().ok())
                .filter(|value| valid_etag(value))
                .map(str::to_owned),
            last_modified: headers
                .get(header::LAST_MODIFIED)
                .and_then(|header| header.to_str().ok())
                .filter(|value| valid_date(value))
                .map(str::to_owned),
        }
    }

    pub(super) fn update_from(&self, response: Self) -> Self {
        Self {
            etag: response.etag.or_else(|| self.etag.clone()),
            last_modified: response
                .last_modified
                .or_else(|| self.last_modified.clone()),
        }
    }

    fn conditional_header(&self) -> Option<(header::HeaderName, &str)> {
        self.etag
            .as_deref()
            .filter(|value| valid_etag(value))
            .map(|value| (header::IF_NONE_MATCH, value))
            .or_else(|| {
                self.last_modified
                    .as_deref()
                    .filter(|value| valid_date(value))
                    .map(|value| (header::IF_MODIFIED_SINCE, value))
            })
    }
}

fn valid_etag(value: &str) -> bool {
    if value.len() > HEADER_LIMIT {
        return false;
    }
    let tag = value.strip_prefix("W/").unwrap_or(value);
    tag.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .is_some_and(|value| value.bytes().all(|byte| matches!(byte, 0x21 | 0x23..=0x7e)))
}

fn valid_date(value: &str) -> bool {
    value.len() <= HEADER_LIMIT && httpdate::parse_http_date(value).is_ok()
}

pub(super) enum Response {
    Image {
        image: Image,
        validators: Validators,
        no_store: bool,
    },
    NotModified {
        validators: Validators,
        no_store: bool,
    },
}

pub(super) fn download(url: &str, validators: Option<&Validators>) -> Option<Response> {
    if !is_public_artwork_url(url) {
        return None;
    }
    #[cfg(all(test, feature = "capture"))]
    if let Some((origin, endpoint)) = TEST_ORIGIN.get()
        && origin == url
    {
        return request(&CLIENT, endpoint, validators);
    }
    request(&CLIENT, url, validators)
}

#[cfg(all(test, feature = "capture"))]
static TEST_ORIGIN: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

#[cfg(all(test, feature = "capture"))]
pub(super) fn loopback_for_capture(origin: &str, endpoint: std::net::SocketAddr) {
    assert!(endpoint.ip().is_loopback());
    TEST_ORIGIN
        .set((origin.to_owned(), format!("http://{endpoint}/cover.png")))
        .unwrap();
}

pub(super) fn request(
    agent: &ureq::Agent,
    url: &str,
    validators: Option<&Validators>,
) -> Option<Response> {
    let mut request = agent.get(url);
    let conditional = validators.and_then(Validators::conditional_header);
    if let Some((name, value)) = &conditional {
        request = request.header(name.clone(), *value);
    }
    let mut response = request.call().ok()?;
    let headers = response.headers();
    let validators = Validators::from_headers(headers);
    let no_store = headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"));
    match response.status() {
        StatusCode::NOT_MODIFIED if conditional.is_some() => Some(Response::NotModified {
            validators,
            no_store,
        }),
        StatusCode::OK => {
            let format = ImageFormat::from_mime_type(response.body().mime_type()?)?;
            if !matches!(
                format,
                ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Webp
            ) {
                return None;
            }
            let reader = response
                .body_mut()
                .with_config()
                .limit(RESPONSE_LIMIT)
                .reader();
            let mut bytes = Vec::new();
            telemetry::artwork_body(reader)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(Response::Image {
                image: Image::from_bytes(format, bytes),
                validators,
                no_store,
            })
        }
        _ => None,
    }
}
