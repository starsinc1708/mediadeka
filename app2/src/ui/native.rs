use gpui::{App, Window};

#[derive(Clone, Copy)]
pub(super) enum Command {
    Minimize,
    ToggleMaximize,
    Close,
}

#[cfg(windows)]
mod win32 {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
            WindowsAndMessaging::{
                IsIconic, PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE,
                SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SystemParametersInfoW,
                WM_SYSCOMMAND,
            },
        },
    };

    use super::{Command, Window};

    pub(super) fn hwnd(window: &Window) -> Option<HWND> {
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        Some(handle.hwnd.get() as HWND)
    }

    pub(super) fn dispatch(command: Command, window: &Window) {
        let Some(hwnd) = hwnd(window) else { return };
        let operation = match command {
            Command::Minimize => SC_MINIMIZE,
            Command::ToggleMaximize if window.is_maximized() => SC_RESTORE,
            Command::ToggleMaximize => SC_MAXIMIZE,
            Command::Close => SC_CLOSE,
        };
        // The HWND belongs to the live GPUI Window. Posting keeps Win32 window
        // callbacks out of the current GPUI update.
        unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, operation as usize, 0) };
    }

    pub(super) fn minimized(window: &Window) -> bool {
        hwnd(window).is_some_and(|handle| {
            // The handle is borrowed from the window for the duration of the query.
            unsafe { IsIconic(handle) != 0 }
        })
    }

    pub(super) fn reduced_motion() -> bool {
        let mut enabled = 1i32;
        // SPI_GETCLIENTAREAANIMATION writes a single BOOL to this local.
        let success = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            )
        };
        success != 0 && enabled == 0
    }

    pub(super) fn high_contrast() -> bool {
        let mut contrast = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            dwFlags: 0,
            lpszDefaultScheme: std::ptr::null_mut(),
        };
        // SPI_GETHIGHCONTRAST fills the initialized structure. Windows keeps
        // ownership of the returned scheme name; only the flags are needed.
        let success = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                (&mut contrast as *mut HIGHCONTRASTW).cast(),
                0,
            )
        };
        success != 0 && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

#[cfg(windows)]
pub(super) fn login_owner(window: &Window) -> isize {
    win32::hwnd(window).map_or(0, |handle| handle as isize)
}

#[cfg(not(windows))]
pub(super) fn login_owner(_: &Window) -> isize {
    0
}

pub(super) fn run(command: Command, window: &mut Window, _: &mut App) {
    #[cfg(windows)]
    win32::dispatch(command, window);

    #[cfg(not(windows))]
    match command {
        Command::Minimize => window.minimize_window(),
        Command::ToggleMaximize => window.zoom_window(),
        Command::Close => window.remove_window(),
    }
}

pub(super) fn minimized(window: &Window) -> bool {
    #[cfg(windows)]
    {
        win32::minimized(window)
    }
    #[cfg(not(windows))]
    {
        !window.is_visible()
    }
}

pub(super) fn reduced_motion() -> bool {
    #[cfg(windows)]
    {
        win32::reduced_motion()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(super) fn high_contrast() -> bool {
    #[cfg(windows)]
    {
        win32::high_contrast()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(all(test, windows))]
#[test]
fn repeated_high_contrast_queries_keep_the_windows_scheme_alive() {
    for _ in 0..32 {
        std::hint::black_box(high_contrast());
    }
}
