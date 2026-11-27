#[cfg(test)]
mod tests;

use std::{
    cell::Cell,
    ffi::c_void,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    rc::Rc,
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{ICoreWebView2_2, ICoreWebView2Environment2},
    WebResourceResponseReceivedEventHandler, take_pwstr,
};
use windows_core::{HRESULT, HSTRING, Interface, PWSTR, w};
use windows_sys::Win32::Foundation::E_OUTOFMEMORY;
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder, WebViewExtWindows};

use super::{
    BrowserExit, LoginWindow, PROFILE_LOCK_TIMEOUT, PUMP_INTERVAL, SIGN_IN_DIRECTORY,
    WEBVIEW_DIRECTORY, WINDOW_TITLE, auth_cookies, builder, clear_auth_cookies, lock_profile,
    on_webview_thread, pump_messages, session_cookie, unavailable, verify_data_folder,
};
use crate::providers::{ErrorKind, ProviderError, transport::check_cancelled};

// The first-party SoundCloud Webi client posts to this endpoint with its cookie jar.
const REFRESH_URL: &str = "https://soundcloud.com/n/api/refresh-token";
const REFRESH_TIMEOUT: Duration = Duration::from_secs(12);
const REFRESH_COOKIE_TIMEOUT: Duration = Duration::from_secs(2);

#[link(name = "shlwapi")]
unsafe extern "system" {
    fn SHCreateMemStream(data: *const u8, length: u32) -> *mut c_void;
}

pub(in crate::providers::soundcloud) fn renew(
    profile: &Path,
    expired: &str,
    cancelled: &AtomicBool,
) -> Result<Option<String>, ProviderError> {
    check_cancelled(cancelled)?;
    let Some(directory) = existing_directory(profile)? else {
        return Ok(None);
    };
    let _lock = lock_profile(&directory, cancelled, Instant::now() + PROFILE_LOCK_TIMEOUT)?;
    on_webview_thread(|| {
        with_browser(
            &directory.join(WEBVIEW_DIRECTORY),
            Some(REFRESH_URL),
            |view| {
                check_cancelled(cancelled)?;
                let cookies = auth_cookies(view)?;
                if let Some(current) = cookies.access.filter(|candidate| candidate != expired) {
                    return Ok(Some(current));
                }
                if !cookies.has_refresh {
                    return Ok(None);
                }
                let status = post_refresh(view, REFRESH_URL, cancelled)?;
                if !refresh_status(status)? {
                    return Ok(None);
                }
                let deadline = Instant::now() + REFRESH_COOKIE_TIMEOUT;
                loop {
                    pump_messages();
                    if let Some(token) = replacement_cookie(view, expired)? {
                        return Ok(Some(token));
                    }
                    if Instant::now() >= deadline {
                        return Err(unavailable(
                            "SoundCloud не выдал обновлённый сеанс. Повторите проверку.",
                        ));
                    }
                    thread::sleep(PUMP_INTERVAL);
                }
            },
        )
    })
}

pub(in crate::providers::soundcloud) fn clear_browser_session(
    profile: &Path,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    check_cancelled(cancelled)?;
    let Some(directory) = existing_directory(profile)? else {
        return Ok(());
    };
    let _lock = lock_profile(&directory, cancelled, Instant::now() + PROFILE_LOCK_TIMEOUT)?;
    on_webview_thread(|| clear_in_browser(&directory.join(WEBVIEW_DIRECTORY)))
}

pub(super) fn clear_in_browser(profile: &Path) -> Result<(), ProviderError> {
    with_browser(profile, None, clear_auth_cookies)
}

fn existing_directory(profile: &Path) -> Result<Option<PathBuf>, ProviderError> {
    let directory = std::path::absolute(profile.join(SIGN_IN_DIRECTORY))
        .map_err(|_| unavailable("Не удалось определить каталог браузера SoundCloud."))?;
    directory
        .join(WEBVIEW_DIRECTORY)
        .try_exists()
        .map(|exists| exists.then_some(directory))
        .map_err(|_| unavailable("Не удалось открыть сохранённый каталог браузера SoundCloud."))
}

fn with_browser<T>(
    profile: &Path,
    target: Option<&str>,
    operation: impl FnOnce(&WebView) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let mut context = WebContext::new(Some(profile.to_path_buf()));
    let target = target.map(str::to_owned);
    let window = LoginWindow::new(
        WINDOW_TITLE,
        null_mut(),
        builder(WebViewBuilder::new_with_web_context(&mut context))
            .with_visible(false)
            .with_javascript_disabled()
            .with_navigation_handler(move |url| target.as_ref() == Some(&url))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny),
    )?;
    let browser_exit = BrowserExit::watch(&window.view);
    let result = verify_data_folder(&window.view, profile).and_then(|()| operation(&window.view));
    drop(window);
    if let Some(browser_exit) = browser_exit {
        browser_exit.wait();
    }
    result
}

fn replacement_cookie(view: &WebView, expired: &str) -> Result<Option<String>, ProviderError> {
    Ok(session_cookie(view)?.filter(|candidate| candidate != expired))
}

fn post_refresh(view: &WebView, url: &str, cancelled: &AtomicBool) -> Result<i32, ProviderError> {
    check_cancelled(cancelled)?;
    let status = Rc::new(Cell::new(None));
    navigate_post(view, url, Rc::clone(&status))?;
    let deadline = Instant::now() + REFRESH_TIMEOUT;
    loop {
        // The server may already have rotated the grant. Let WebView2 persist its response.
        pump_messages();
        if let Some(status) = status.get() {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(ProviderError::new(
                ErrorKind::Network,
                "SoundCloud не ответил на обновление сеанса. Повторите проверку.",
            ));
        }
        thread::sleep(PUMP_INTERVAL);
    }
}

fn navigate_post(
    view: &WebView,
    url: &str,
    status: Rc<Cell<Option<i32>>>,
) -> Result<(), ProviderError> {
    let target = url.to_owned();
    let request_url = HSTRING::from(url);
    let register = || -> windows_core::Result<()> {
        let webview = view.webview().cast::<ICoreWebView2_2>()?;
        let environment = view.environment().cast::<ICoreWebView2Environment2>()?;
        // Both callbacks and cookie processing stay on the dedicated WebView2 STA thread.
        // WebView2 attaches cookies to the HTTPS request using its own cookie rules.
        unsafe {
            // SHCreateMemStream returns one owned COM reference to an empty POST body.
            let raw = SHCreateMemStream(null(), 0);
            if raw.is_null() {
                return Err(windows_core::Error::from_hresult(HRESULT(E_OUTOFMEMORY)));
            }
            let stream = Some(Interface::from_raw(raw));
            let request = environment.CreateWebResourceRequest(
                &request_url,
                w!("POST"),
                stream.as_ref(),
                w!("Accept: application/json\r\n"),
            )?;
            webview.add_WebResourceResponseReceived(
                &WebResourceResponseReceivedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut uri = PWSTR::null();
                        args.Request()?.Uri(&mut uri)?;
                        if take_pwstr(uri) == target {
                            let mut code = 0;
                            args.Response()?.StatusCode(&mut code)?;
                            status.set(Some(code));
                        }
                    }
                    Ok(())
                })),
                &mut 0,
            )?;
            webview.NavigateWithWebResourceRequest(&request)
        }
    };
    register().map_err(|error| {
        unavailable(format!(
            "Не удалось запросить обновление сеанса SoundCloud (WebView2 {}).",
            error.code()
        ))
    })
}

fn refresh_status(status: i32) -> Result<bool, ProviderError> {
    match status {
        200..=299 => Ok(true),
        401 => Ok(false),
        408 | 425 | 429 | 500..=599 => Err(ProviderError::new(
            ErrorKind::Network,
            format!("SoundCloud временно не обновил сеанс (HTTP {status}). Повторите проверку."),
        )),
        _ => Err(unavailable(format!(
            "SoundCloud не подтвердил обновление сеанса (HTTP {status}). Повторите проверку."
        ))),
    }
}
