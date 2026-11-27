mod cookies;
mod renewal;

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    fs::{self, File, OpenOptions, TryLockError},
    num::NonZeroIsize,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    rc::Rc,
    sync::{OnceLock, atomic::AtomicBool},
    thread,
    time::{Duration, Instant},
};

use raw_window_handle::{HandleError, HasWindowHandle, Win32WindowHandle, WindowHandle};
use url::Url;
use webview2_com::{
    BrowserProcessExitedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
        ICoreWebView2Environment5, ICoreWebView2Environment7,
    },
    NavigationCompletedEventHandler, WindowCloseRequestedEventHandler, take_pwstr,
};
use windows_core::{Interface, PWSTR};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GWLP_USERDATA, GetWindowLongPtrW, IDC_ARROW, IsWindow, LoadCursorW, MSG, PM_REMOVE,
        PeekMessageW, PostMessageW, RegisterClassW, SW_HIDE, SW_SHOW, SetWindowLongPtrW,
        ShowWindow, TranslateMessage, WM_CLOSE, WNDCLASSW, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW,
    },
};
use wry::{
    NewWindowResponse, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows,
    WebViewExtWindows,
};

use crate::providers::{
    ErrorKind, ProviderError,
    transport::{LOGIN_TIMEOUT, check_cancelled, wait},
};

use self::cookies::{auth_cookies, clear_auth_cookies, session_cookie};

const SIGN_IN: &str = "https://soundcloud.com/signin";
const SESSION_ORIGIN: &str = "https://soundcloud.com/";
const SESSION_COOKIE: &str = "oauth_token";
const REFRESH_COOKIE: &str = "oauth_refresh_token";
const SESSION_DOMAIN: &str = "soundcloud.com";
const SESSION_PATH: &str = "/";
const SIGN_IN_DIRECTORY: &str = "SoundCloudSignIn";
const WEBVIEW_DIRECTORY: &str = "WebView2";
const WINDOW_TITLE: &str = "Вход в SoundCloud — Mediadeka";
const PUMP_INTERVAL: Duration = Duration::from_millis(35);
const COOKIE_INTERVAL: Duration = Duration::from_millis(500);
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(8);
const PROFILE_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) use renewal::{clear_browser_session, renew};

pub(super) fn sign_in<T>(
    profile: &Path,
    owner: isize,
    cancelled: &AtomicBool,
    confirm: impl FnOnce(String) -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    check_cancelled(cancelled)?;
    let directory = login_directory(profile)?;
    let _lock = lock_profile(&directory, cancelled, Instant::now() + PROFILE_LOCK_TIMEOUT)?;
    let webview = directory.join(WEBVIEW_DIRECTORY);
    let token = on_webview_thread(|| run_window(&webview, owner, cancelled))?;
    let result = check_cancelled(cancelled).and_then(|()| confirm(token));
    if result.is_err() && on_webview_thread(|| renewal::clear_in_browser(&webview)).is_err() {
        eprintln!("Не удалось очистить неподтверждённый сеанс WebView2 SoundCloud.");
    }
    result
}

fn on_webview_thread<T: Send>(
    operation: impl FnOnce() -> Result<T, ProviderError> + Send,
) -> Result<T, ProviderError> {
    thread::scope(|scope| {
        thread::Builder::new()
            .name("soundcloud-webview".into())
            .spawn_scoped(scope, operation)
            .map_err(|_| unavailable("Не удалось запустить окно входа SoundCloud."))?
            .join()
            .map_err(|_| unavailable("Окно входа SoundCloud завершилось с ошибкой."))?
    })
}

fn login_directory(profile: &Path) -> Result<PathBuf, ProviderError> {
    let directory = std::path::absolute(profile.join(SIGN_IN_DIRECTORY))
        .map_err(|_| unavailable("Не удалось определить каталог браузера SoundCloud."))?;
    fs::create_dir_all(&directory)
        .map_err(|_| unavailable("Не удалось создать отдельный профиль входа SoundCloud."))?;
    Ok(directory)
}

fn lock_profile(
    directory: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<File, ProviderError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("login.lock"))
        .map_err(|_| unavailable("Не удалось открыть профиль входа SoundCloud."))?;
    // The lock belongs to this attempt through controller teardown and the browser's flush.
    loop {
        check_cancelled(cancelled)?;
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(PUMP_INTERVAL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(unavailable(
                    "Окно входа SoundCloud уже использует этот профиль. Закройте его и повторите.",
                ));
            }
            Err(TryLockError::Error(_)) => {
                return Err(unavailable(
                    "Не удалось заблокировать профиль входа SoundCloud.",
                ));
            }
        }
    }
}

fn run_window(
    profile: &Path,
    owner: isize,
    cancelled: &AtomicBool,
) -> Result<String, ProviderError> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    // This dedicated STA thread owns the HWNDs, COM objects, and message pump.
    let mut context = WebContext::new(Some(profile.to_path_buf()));
    let popups = Rc::new(RefCell::new(Vec::<LoginWindow>::new()));
    let pending = Rc::downgrade(&popups);
    let window = LoginWindow::new(
        WINDOW_TITLE,
        owner as HWND,
        builder(WebViewBuilder::new_with_web_context(&mut context)).with_new_window_req_handler(
            move |uri, features| {
                if uri != "about:blank" && !login_url(&uri) {
                    return NewWindowResponse::Deny;
                }
                let Some(popups) = pending.upgrade() else {
                    return NewWindowResponse::Deny;
                };
                let Ok(popup) = LoginWindow::new(
                    "Подтверждение входа в SoundCloud",
                    owner as HWND,
                    builder(WebViewBuilder::new()).with_environment(features.opener.environment),
                ) else {
                    return NewWindowResponse::Deny;
                };
                let webview = popup.view.webview();
                popup.show();
                popups.borrow_mut().push(popup);
                NewWindowResponse::Create { webview }
            },
        ),
    )?;
    let browser_exit = BrowserExit::watch(&window.view);
    let result = (|| {
        check_cancelled(cancelled)?;
        verify_data_folder(&window.view, profile)?;
        clear_auth_cookies(&window.view)?;
        let token = wait_for_token(&window, &popups, cancelled, deadline);
        if token.is_err() && clear_auth_cookies(&window.view).is_err() {
            eprintln!("Не удалось очистить прерванный сеанс WebView2 SoundCloud.");
        }
        token
    })();
    drop(window);
    drop(popups);
    if let Some(browser_exit) = browser_exit {
        browser_exit.wait();
    }
    result
}

fn wait_for_token(
    window: &LoginWindow,
    popups: &RefCell<Vec<LoginWindow>>,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<String, ProviderError> {
    let navigation = Rc::new(Cell::new(None));
    let status = Rc::clone(&navigation);
    // The handler and its state are owned by the controller on this thread.
    unsafe {
        window.view.webview().add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut success = false.into();
                    args.IsSuccess(&mut success)?;
                    if success.as_bool() {
                        status.set(Some(Ok(())));
                    } else {
                        let mut error = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                        args.WebErrorStatus(&mut error)?;
                        if error != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                            status.set(Some(Err(error.0)));
                        }
                    }
                }
                Ok(())
            })),
            &mut 0,
        )
    }
    .map_err(|_| unavailable("Не удалось проверить загрузку страницы SoundCloud."))?;
    check_cancelled(cancelled)?;
    window
        .view
        .load_url(SIGN_IN)
        .map_err(|_| unavailable("Не удалось открыть страницу входа SoundCloud."))?;
    window.show();

    let mut next_cookie = Instant::now();
    loop {
        check_cancelled(cancelled)?;
        pump_messages();
        if window.host.closed() {
            return Err(ProviderError::new(
                ErrorKind::Cancelled,
                "Окно входа SoundCloud закрыто.",
            ));
        }
        let closed: Vec<_> = popups
            .borrow_mut()
            .extract_if(.., |popup| popup.host.closed())
            .collect();
        drop(closed);
        if let Some(Err(code)) = navigation.get() {
            return Err(ProviderError::new(
                ErrorKind::Network,
                format!(
                    "Страница входа SoundCloud не загрузилась (WebView2 {code}). Повторите попытку."
                ),
            ));
        }
        if Instant::now() >= next_cookie {
            let token = session_cookie(&window.view)?;
            check_cancelled(cancelled)?;
            if !window.host.closed()
                && let Some(token) = token
            {
                return Ok(token);
            }
            next_cookie = Instant::now() + COOKIE_INTERVAL;
        }
        wait(cancelled, PUMP_INTERVAL, deadline)?;
    }
}

struct BrowserExit {
    environment: ICoreWebView2Environment5,
    released: Rc<Cell<bool>>,
    handler: i64,
}

impl BrowserExit {
    fn watch(view: &WebView) -> Option<Self> {
        let environment = view
            .environment()
            .cast::<ICoreWebView2Environment5>()
            .ok()?;
        let released = Rc::new(Cell::new(false));
        let signal = Rc::clone(&released);
        let mut handler = 0;
        // The event is delivered on the STA after all WebViews for this profile close.
        unsafe {
            environment.add_BrowserProcessExited(
                &BrowserProcessExitedEventHandler::create(Box::new(move |_, _| {
                    signal.set(true);
                    Ok(())
                })),
                &mut handler,
            )
        }
        .ok()?;
        Some(Self {
            environment,
            released,
            handler,
        })
    }

    fn wait(self) {
        let deadline = Instant::now() + PROCESS_EXIT_TIMEOUT;
        while !self.released.get() && Instant::now() < deadline {
            pump_messages();
            thread::sleep(PUMP_INTERVAL);
        }
        // The environment remains valid until its subscription is removed.
        unsafe {
            let _ = self.environment.remove_BrowserProcessExited(self.handler);
        };
    }
}

fn builder<'a>(builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
    builder
        .with_devtools(false)
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(false)
        .with_additional_browser_args("")
        .with_navigation_handler(|uri| login_url(&uri))
        .with_download_started_handler(|_, _| false)
}

fn verify_data_folder(view: &WebView, expected: &Path) -> Result<(), ProviderError> {
    let mut folder = PWSTR::null();
    // WebView2 can override the requested data folder through environment or policy.
    let result = view
        .environment()
        .cast::<ICoreWebView2Environment7>()
        .and_then(|environment| unsafe { environment.UserDataFolder(&mut folder) });
    let actual = take_pwstr(folder);
    result.map_err(|_| unavailable("Не удалось проверить каталог браузера SoundCloud."))?;
    let actual = fs::canonicalize(actual);
    let expected = fs::canonicalize(expected);
    if !matches!((actual, expected), (Ok(actual), Ok(expected)) if actual == expected) {
        return Err(unavailable(
            "WebView2 использует другой каталог входа SoundCloud.",
        ));
    }
    Ok(())
}

fn login_url(uri: &str) -> bool {
    Url::parse(uri).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && matches!(
                url.host_str(),
                Some(
                    "soundcloud.com"
                        | "www.soundcloud.com"
                        | "secure.soundcloud.com"
                        | "accounts.google.com"
                        | "www.facebook.com"
                        | "m.facebook.com"
                        | "appleid.apple.com"
                )
            )
    })
}

fn unavailable(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ErrorKind::Unavailable, message)
}

struct LoginWindow {
    view: WebView,
    host: HostWindow,
}

impl LoginWindow {
    fn new(title: &str, owner: HWND, builder: WebViewBuilder<'_>) -> Result<Self, ProviderError> {
        let host = HostWindow::new(title, owner)?;
        let view = builder.build(&host).map_err(|_| {
            unavailable("Не удалось открыть окно входа SoundCloud. Проверьте Microsoft Edge WebView2 Runtime.")
        })?;
        let hwnd = host.0;
        // WebView2 reports script-driven closure separately from the native close button.
        unsafe {
            view.webview().add_WindowCloseRequested(
                &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    Ok(())
                })),
                &mut 0,
            )
        }
        .map_err(|_| unavailable("Не удалось настроить окно входа SoundCloud."))?;
        Ok(Self { view, host })
    }

    fn show(&self) {
        // The host exists for the lifetime of its WebView2 controller.
        unsafe { ShowWindow(self.host.0, SW_SHOW) };
        let _ = self.view.focus();
    }
}

struct HostWindow(HWND);

impl HostWindow {
    fn new(title: &str, owner: HWND) -> Result<Self, ProviderError> {
        static REGISTERED: OnceLock<bool> = OnceLock::new();
        let class = windows_sys::w!("MediadekaSoundCloudLogin");
        // Both the class and the windows belong to this executable. Each window
        // handles messages on its WebView2 thread.
        let instance = unsafe { GetModuleHandleW(null()) };
        let registered = REGISTERED.get_or_init(|| unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(host_message),
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: class,
                ..Default::default()
            }) != 0
        });
        if !registered {
            return Err(unavailable(
                "Не удалось зарегистрировать окно входа SoundCloud.",
            ));
        }
        let title: Vec<_> = title.encode_utf16().chain([0]).collect();
        // The native owner is the Mediadeka window. Wry owns the WebView2 child.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class,
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                590,
                760,
                owner,
                null_mut(),
                instance,
                null::<c_void>(),
            )
        };
        (!hwnd.is_null())
            .then_some(Self(hwnd))
            .ok_or_else(|| unavailable("Не удалось создать окно входа SoundCloud."))
    }

    fn exists(&self) -> bool {
        // The owner may already have closed.
        unsafe { IsWindow(self.0) != 0 }
    }

    fn closed(&self) -> bool {
        // Close the controller while the host still exists.
        !self.exists() || unsafe { GetWindowLongPtrW(self.0, GWLP_USERDATA) != 0 }
    }
}

unsafe extern "system" fn host_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // The callback receives this HWND on its owning thread.
    unsafe {
        if message == WM_CLOSE {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1);
            ShowWindow(hwnd, SW_HIDE);
            return 0;
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

impl HasWindowHandle for HostWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        let handle = Win32WindowHandle::new(hwnd);
        // Wry borrows this HWND during construction on its owning thread.
        Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        if self.exists() {
            // The WebView2 controller is dropped first. DestroyWindow runs on the HWND's thread.
            unsafe { DestroyWindow(self.0) };
        }
    }
}

fn pump_messages() {
    let mut message = MSG::default();
    // Process only this worker's window queue and yield regularly to cancellation and polling.
    for _ in 0..128 {
        unsafe {
            if PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) == 0 {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_2;
    use windows_core::HSTRING;
    use wry::cookie::time;

    const GOOGLE_ORIGIN: &str = "https://accounts.google.com/";
    const GOOGLE_COOKIE: &str = "mediadeka-fixture";

    #[test]
    fn login_profile_is_exclusive_until_the_attempt_releases_it() {
        let profile = tempfile::tempdir().unwrap();
        let directory = login_directory(profile.path()).unwrap();
        let active = AtomicBool::new(false);
        let first = lock_profile(&directory, &active, Instant::now()).unwrap();
        let conflict = lock_profile(&directory, &active, Instant::now()).unwrap_err();
        assert_eq!(conflict.kind, ErrorKind::Unavailable);
        drop(first);
        assert!(lock_profile(&directory, &active, Instant::now()).is_ok());
    }

    #[test]
    #[ignore = "requires Windows WebView2 Runtime; uses temporary profiles and no network or real cookies"]
    fn live_soundcloud_webview_cookie_isolation() {
        let reusable = tempfile::tempdir().unwrap();
        let independent = tempfile::tempdir().unwrap();
        with_view(reusable.path(), |view| {
            for (name, domain) in [
                (SESSION_COOKIE, SESSION_DOMAIN),
                (SESSION_COOKIE, ".soundcloud.com"),
                (REFRESH_COOKIE, SESSION_DOMAIN),
                (GOOGLE_COOKIE, "accounts.google.com"),
            ] {
                seed_fixture_cookie(view, name, domain).expect("seed a synthetic cookie");
            }
            assert_eq!(soundcloud_fixture_count(view), 2);
            assert!(has_google_fixture(view));
        });
        with_view(reusable.path(), |view| {
            assert_eq!(soundcloud_fixture_count(view), 2);
            assert!(has_google_fixture(view));
            assert!(auth_cookies(view).unwrap().has_refresh);
            clear_auth_cookies(view).unwrap();
            assert_eq!(soundcloud_fixture_count(view), 0);
            assert!(!auth_cookies(view).unwrap().present);
            assert!(has_google_fixture(view));
        });
        with_view(reusable.path(), |view| {
            assert_eq!(session_cookie(view).unwrap(), None);
            assert!(has_google_fixture(view));
        });
        with_view(independent.path(), |view| {
            assert_eq!(session_cookie(view).unwrap(), None);
            assert!(!has_google_fixture(view));
        });
        reusable.close().unwrap();
        independent.close().unwrap();
    }

    fn with_view(profile: &Path, exercise: impl FnOnce(&WebView)) {
        let folder = login_directory(profile).unwrap().join("WebView2");
        let mut context = WebContext::new(Some(folder.clone()));
        let window = LoginWindow::new(
            WINDOW_TITLE,
            null_mut(),
            builder(WebViewBuilder::new_with_web_context(&mut context)),
        )
        .expect("initialize WebView2 test window");
        let browser_exit = BrowserExit::watch(&window.view).expect("watch WebView2 shutdown");
        verify_data_folder(&window.view, &folder).unwrap();
        exercise(&window.view);
        drop(window);
        browser_exit.wait();
    }

    fn seed_fixture_cookie(view: &WebView, name: &str, domain: &str) -> windows_core::Result<()> {
        let name = HSTRING::from(name);
        let domain = HSTRING::from(domain);
        let expiry = (time::OffsetDateTime::now_utc() + time::Duration::days(1)).unix_timestamp();
        // Seed both domain forms without Wry's normalization. No page is loaded.
        unsafe {
            let manager = view.webview().cast::<ICoreWebView2_2>()?.CookieManager()?;
            let cookie = manager.CreateCookie(
                &name,
                windows_core::w!("fixture-value"),
                &domain,
                windows_core::w!("/"),
            )?;
            cookie.SetExpires(expiry as f64)?;
            cookie.SetIsHttpOnly(true)?;
            cookie.SetIsSecure(true)?;
            manager.AddOrUpdateCookie(&cookie)
        }
    }

    fn soundcloud_fixture_count(view: &WebView) -> usize {
        view.cookies_for_url(SESSION_ORIGIN)
            .unwrap()
            .iter()
            .filter(|cookie| cookie.name() == SESSION_COOKIE && cookie.value() == "fixture-value")
            .count()
    }

    fn has_google_fixture(view: &WebView) -> bool {
        view.cookies_for_url(GOOGLE_ORIGIN)
            .unwrap()
            .iter()
            .any(|cookie| cookie.name() == GOOGLE_COOKIE && cookie.value() == "fixture-value")
    }
}
