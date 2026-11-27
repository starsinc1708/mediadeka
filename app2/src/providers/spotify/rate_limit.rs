use std::{
    sync::{Mutex, PoisonError, atomic::AtomicBool},
    time::{Duration, Instant, SystemTime},
};

use ureq::{
    Body,
    http::{HeaderMap, Response, StatusCode, header::RETRY_AFTER},
};

use crate::providers::{ErrorKind, LoginPrompt, ProviderError, transport};

use super::PROVIDER;

const DEFAULT_DELAY: Duration = Duration::from_secs(30);
const MIN_DELAY: Duration = Duration::from_secs(1);
const CODE_RETRY_WINDOW: Duration = Duration::from_secs(60);

pub(super) static ACCOUNTS: Cooldown = Cooldown::new();
pub(super) static FEED: Cooldown = Cooldown::new();
pub(super) static LIBRARY: Cooldown = Cooldown::new();

#[derive(Clone, Copy)]
pub(super) enum Step {
    ExchangeCode,
    RefreshToken,
    Feed,
    Library,
}

impl Step {
    fn label(self) -> &'static str {
        match self {
            Self::ExchangeCode => "обмена кода входа",
            Self::RefreshToken => "обновления сеанса",
            Self::Feed => "загрузки рекомендаций",
            Self::Library => "запроса медиатеки",
        }
    }

    fn retry_window(self) -> Duration {
        match self {
            Self::ExchangeCode => CODE_RETRY_WINDOW,
            _ => transport::LOGIN_TIMEOUT,
        }
    }

    fn limit_message(self) -> String {
        format!(
            "Spotify ограничил запросы во время {} (HTTP 429).",
            self.label()
        )
    }

    fn error(self, remaining: Duration) -> ProviderError {
        ProviderError::new(
            ErrorKind::Network,
            format!(
                "{} Повторите через {}.",
                self.limit_message(),
                display_delay(remaining)
            ),
        )
    }

    fn waiting(self, remaining: Duration) -> LoginPrompt {
        LoginPrompt::Waiting {
            message: format!(
                "{} Текущий вход продолжится после паузы в {}.",
                self.limit_message(),
                display_delay(remaining),
            ),
        }
    }
}

pub(super) struct LoginRetry<'a> {
    pub cancelled: &'a AtomicBool,
    pub prompt: &'a dyn Fn(LoginPrompt),
}

pub(super) struct Cooldown(Mutex<Option<Limit>>);

struct Limit {
    step: Step,
    observed: Instant,
    delay: Duration,
}

impl Limit {
    fn remaining(&self) -> Duration {
        self.delay.saturating_sub(self.observed.elapsed())
    }
}

enum Attempt {
    Response(Response<Body>),
    Limited(Step, Duration),
}

impl Cooldown {
    const fn new() -> Self {
        Self(Mutex::new(None))
    }

    fn remaining(&self) -> Option<(Step, Duration)> {
        let current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let limit = current.as_ref()?;
        let remaining = limit.remaining();
        (!remaining.is_zero()).then_some((limit.step, remaining))
    }

    fn record(&self, step: Step, delay: Duration) {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if current
            .as_ref()
            .is_none_or(|limit| delay >= limit.remaining())
        {
            *current = Some(Limit {
                step,
                observed: Instant::now(),
                delay,
            });
        }
    }

    pub(super) fn check(&self) -> Result<(), ProviderError> {
        if let Some((step, remaining)) = self.remaining() {
            return Err(step.error(remaining));
        }
        Ok(())
    }

    pub(super) fn limited(&self, step: Step, headers: &HeaderMap) -> ProviderError {
        let (step, remaining) = self.observe_limit(step, headers);
        step.error(remaining)
    }

    pub(super) fn call(
        &self,
        step: Step,
        retry: Option<&LoginRetry<'_>>,
        request: impl Fn() -> Result<Response<Body>, ureq::Error>,
    ) -> Result<Response<Body>, ProviderError> {
        let (limited_step, remaining) = match self.attempt(step, &request)? {
            Attempt::Response(response) => return Ok(response),
            Attempt::Limited(limited_step, remaining) => (limited_step, remaining),
        };
        let Some(retry) = retry.filter(|_| remaining <= step.retry_window()) else {
            return Err(limited_step.error(remaining));
        };

        (retry.prompt)(limited_step.waiting(remaining));
        transport::wait(
            retry.cancelled,
            remaining,
            Instant::now() + remaining + Duration::from_secs(2),
        )?;
        match self.attempt(step, &request)? {
            Attempt::Response(response) => Ok(response),
            Attempt::Limited(limited_step, remaining) => Err(limited_step.error(remaining)),
        }
    }

    fn attempt(
        &self,
        step: Step,
        request: &impl Fn() -> Result<Response<Body>, ureq::Error>,
    ) -> Result<Attempt, ProviderError> {
        if let Some((limited_step, remaining)) = self.remaining() {
            return Ok(Attempt::Limited(limited_step, remaining));
        }
        let response = request().map_err(|error| transport::network(PROVIDER, error))?;
        if response.status() != StatusCode::TOO_MANY_REQUESTS {
            return Ok(Attempt::Response(response));
        }
        let (limited_step, remaining) = self.observe_limit(step, response.headers());
        Ok(Attempt::Limited(limited_step, remaining))
    }

    fn observe_limit(&self, step: Step, headers: &HeaderMap) -> (Step, Duration) {
        let value = headers
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok());
        let delay = retry_after(value, SystemTime::now());
        self.record(step, delay);
        self.remaining().unwrap_or((step, delay))
    }
}

fn retry_after(value: Option<&str>, now: SystemTime) -> Duration {
    let delay = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| {
            if value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
            }
            httpdate::parse_http_date(value)
                .ok()
                .map(|date| date.duration_since(now).unwrap_or_default())
        });
    delay.unwrap_or(DEFAULT_DELAY).max(MIN_DELAY)
}

fn display_delay(duration: Duration) -> String {
    let seconds = duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0));
    match seconds {
        0..60 => format!("{seconds} с"),
        60..3600 => format!("{} мин.", seconds.div_ceil(60)),
        3600..86400 => format!("{} ч", seconds.div_ceil(3600)),
        _ => format!("{} дн.", seconds.div_ceil(86400)),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, sync::atomic::Ordering};

    use super::*;

    fn reply(status: StatusCode, delay: &str) -> Response<Body> {
        Response::builder()
            .status(status)
            .header(RETRY_AFTER, delay)
            .body(Body::builder().data(Vec::new()))
            .unwrap()
    }

    #[test]
    fn feed_records_retry_after_and_refuses_requests_during_cooldown() {
        let feed = Cooldown::new();
        let accounts = Cooldown::new();
        let response = reply(StatusCode::TOO_MANY_REQUESTS, "90");
        let error = feed.limited(Step::Feed, response.headers());
        assert_eq!(error.kind, ErrorKind::Network);
        assert!(error.message.contains("2 мин."));
        assert_eq!(feed.check().unwrap_err(), error);
        assert!(accounts.check().is_ok());
    }

    #[test]
    fn reads_retry_after_seconds_and_date() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_700);
        let in_45_seconds = httpdate::fmt_http_date(now + Duration::from_secs(45));
        assert_eq!(retry_after(Some(" 17 "), now), Duration::from_secs(17));
        assert_eq!(
            retry_after(Some(&in_45_seconds), now),
            Duration::from_secs(45)
        );
        assert_eq!(retry_after(Some("0"), now), MIN_DELAY);
        assert_eq!(retry_after(Some("invalid"), now), DEFAULT_DELAY);
        assert_eq!(retry_after(None, now), DEFAULT_DELAY);
    }

    #[test]
    fn exchange_retries_the_same_request_once_after_server_delay() {
        let cooldown = Cooldown::new();
        let requests = Cell::new(0);
        let waits = Cell::new(0);
        let first_request = Cell::new(None::<Instant>);
        let response = cooldown
            .call(
                Step::ExchangeCode,
                Some(&LoginRetry {
                    cancelled: &AtomicBool::new(false),
                    prompt: &|status| {
                        let LoginPrompt::Waiting { message } = status else {
                            panic!("expected wait")
                        };
                        assert!(message.contains("обмена кода входа"));
                        waits.set(waits.get() + 1);
                    },
                }),
                || {
                    requests.set(requests.get() + 1);
                    if let Some(first) = first_request.get() {
                        assert!(first.elapsed() >= Duration::from_secs(1));
                        Ok(reply(StatusCode::OK, "0"))
                    } else {
                        first_request.set(Some(Instant::now()));
                        Ok(reply(StatusCode::TOO_MANY_REQUESTS, "1"))
                    }
                },
            )
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(requests.get(), 2);
        assert_eq!(waits.get(), 1);
    }

    #[test]
    fn long_limit_reports_the_step_and_blocks_immediate_reentry() {
        let cooldown = Cooldown::new();
        let error = cooldown
            .call(
                Step::ExchangeCode,
                Some(&LoginRetry {
                    cancelled: &AtomicBool::new(false),
                    prompt: &|_| panic!("too long to retry the authorization code"),
                }),
                || Ok(reply(StatusCode::TOO_MANY_REQUESTS, "3600")),
            )
            .unwrap_err();
        assert!(error.message.contains("обмена кода входа"));
        assert!(error.message.contains("1 ч"));
        assert_eq!(cooldown.check().unwrap_err().kind, ErrorKind::Network);
        let blocked = cooldown
            .call(Step::RefreshToken, None, || {
                panic!("request sent during active cooldown");
            })
            .unwrap_err();
        assert!(blocked.message.contains("обмена кода входа"));
    }

    #[test]
    fn cancellation_during_pause_skips_the_retry() {
        let cooldown = Cooldown::new();
        let flag = AtomicBool::new(false);
        let requests = Cell::new(0);
        let error = cooldown
            .call(
                Step::ExchangeCode,
                Some(&LoginRetry {
                    cancelled: &flag,
                    prompt: &|_| flag.store(true, Ordering::Relaxed),
                }),
                || {
                    requests.set(requests.get() + 1);
                    Ok(reply(StatusCode::TOO_MANY_REQUESTS, "1"))
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
        assert_eq!(requests.get(), 1);
    }

    #[test]
    fn a_second_429_sets_a_new_cooldown_without_another_automatic_request() {
        let cooldown = Cooldown::new();
        let requests = Cell::new(0);
        let error = cooldown
            .call(
                Step::ExchangeCode,
                Some(&LoginRetry {
                    cancelled: &AtomicBool::new(false),
                    prompt: &|_| {},
                }),
                || {
                    let count = requests.get() + 1;
                    requests.set(count);
                    let delay = if count == 1 { "1" } else { "120" };
                    Ok(reply(StatusCode::TOO_MANY_REQUESTS, delay))
                },
            )
            .unwrap_err();
        assert!(error.message.contains("2 мин."));
        assert_eq!(requests.get(), 2);
        assert!(cooldown.check().is_err());
    }
}
