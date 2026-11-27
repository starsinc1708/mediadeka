use std::{
    cell::Cell,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

use webview2_com::{
    GetCookiesCompletedHandler,
    Microsoft::Web::WebView2::Win32::{ICoreWebView2_2, ICoreWebView2CookieList},
    take_pwstr,
};
use windows_core::{HSTRING, Interface, PCWSTR, PWSTR};
use wry::{WebView, WebViewExtWindows};

use super::{
    PUMP_INTERVAL, REFRESH_COOKIE, SESSION_COOKIE, SESSION_DOMAIN, SESSION_ORIGIN, SESSION_PATH,
    pump_messages, unavailable,
};
use crate::providers::{ProviderError, transport::valid_token};

const COOKIE_OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct AuthCookies {
    pub(super) access: Option<String>,
    pub(super) has_refresh: bool,
    pub(super) present: bool,
}

pub(super) fn session_cookie(view: &WebView) -> Result<Option<String>, ProviderError> {
    Ok(auth_cookies(view)?.access)
}

pub(super) fn auth_cookies(view: &WebView) -> Result<AuthCookies, ProviderError> {
    let completed = Rc::new(Cell::new(None));
    let signal = Rc::clone(&completed);
    let origin = HSTRING::from(SESSION_ORIGIN);
    let read_error = || unavailable("Не удалось прочитать сеанс из окна входа SoundCloud.");
    // GetCookies delivers the cookie interfaces to this STA. Only the access cookie's value
    // is copied into app memory; WebView2 retains and submits the HttpOnly refresh credential.
    unsafe {
        view.webview()
            .cast::<ICoreWebView2_2>()
            .and_then(|webview| webview.CookieManager())
            .and_then(|manager| {
                manager.GetCookies(
                    &origin,
                    &GetCookiesCompletedHandler::create(Box::new(move |result, cookies| {
                        signal.set(Some(result.and_then(|()| read_cookies(cookies))));
                        Ok(())
                    })),
                )
            })
    }
    .map_err(|_| read_error())?;
    let deadline = Instant::now() + COOKIE_OPERATION_TIMEOUT;
    loop {
        pump_messages();
        if let Some(result) = completed.take() {
            return result.map_err(|_| read_error());
        }
        if Instant::now() >= deadline {
            return Err(read_error());
        }
        thread::sleep(PUMP_INTERVAL);
    }
}

fn read_cookies(cookies: Option<ICoreWebView2CookieList>) -> windows_core::Result<AuthCookies> {
    let mut state = AuthCookies::default();
    let Some(cookies) = cookies else {
        return Ok(state);
    };
    let mut count = 0;
    // The callback owns the cookie list and its elements on the WebView2 STA.
    unsafe {
        cookies.Count(&mut count)?;
        for index in 0..count {
            let cookie = cookies.GetValueAtIndex(index)?;
            let name = cookie_string(|out| cookie.Name(out))?;
            if !matches!(name.as_str(), SESSION_COOKIE | REFRESH_COOKIE) {
                continue;
            }
            let domain = cookie_string(|out| cookie.Domain(out))?;
            if domain.trim_start_matches('.') != SESSION_DOMAIN
                || cookie_string(|out| cookie.Path(out))? != SESSION_PATH
            {
                continue;
            }
            state.present = true;
            let mut secure = false.into();
            cookie.IsSecure(&mut secure)?;
            if !secure.as_bool() {
                continue;
            }
            if name == REFRESH_COOKIE {
                let mut http_only = false.into();
                cookie.IsHttpOnly(&mut http_only)?;
                state.has_refresh |= http_only.as_bool();
            } else if state.access.is_none() {
                let token = cookie_string(|out| cookie.Value(out))?;
                if valid_token(&token, super::super::TOKEN_LIMIT) {
                    state.access = Some(token);
                }
            }
        }
    }
    Ok(state)
}

fn cookie_string(
    get: impl FnOnce(&mut PWSTR) -> windows_core::Result<()>,
) -> windows_core::Result<String> {
    let mut value = PWSTR::null();
    let result = get(&mut value);
    let value = take_pwstr(value);
    result.map(|()| value)
}

pub(super) fn clear_auth_cookies(view: &WebView) -> Result<(), ProviderError> {
    let path = HSTRING::from(SESSION_PATH);
    let domains = [
        HSTRING::from(SESSION_DOMAIN),
        HSTRING::from(format!(".{SESSION_DOMAIN}")),
    ];
    let error = || unavailable("Не удалось очистить предыдущий сеанс SoundCloud.");
    // Match both the host-only and domain forms without touching the sign-in provider.
    view.webview()
        .cast::<ICoreWebView2_2>()
        .and_then(|webview| unsafe {
            let manager = webview.CookieManager()?;
            for name in [SESSION_COOKIE, REFRESH_COOKIE].map(HSTRING::from) {
                for domain in &domains {
                    manager.DeleteCookiesWithDomainAndPath(
                        PCWSTR(name.as_ptr()),
                        PCWSTR(domain.as_ptr()),
                        PCWSTR(path.as_ptr()),
                    )?;
                }
            }
            Ok(())
        })
        .map_err(|_| error())?;
    let deadline = Instant::now() + COOKIE_OPERATION_TIMEOUT;
    while auth_cookies(view)?.present {
        if Instant::now() >= deadline {
            return Err(error());
        }
        pump_messages();
        thread::sleep(PUMP_INTERVAL);
    }
    Ok(())
}
