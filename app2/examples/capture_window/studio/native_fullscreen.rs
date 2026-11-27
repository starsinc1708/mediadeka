use std::{ffi::c_void, fs, io, path::Path};

use gpui::{AnyWindowHandle, AsyncApp, Window, WindowHandle};
use mediadeka::MainView;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
        SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
        SetWindowPos,
    },
};

use super::{key, named, node, node_label, press, require, snapshot, wait};

const DWMWA_CLOAK: u32 = 13;
const DWMWA_CLOAKED: u32 = 14;
const DWM_CLOAKED_APP: u32 = 1;
const TRACK: &str = "Свет в переходе";

#[link(name = "dwmapi")]
unsafe extern "system" {
    #[link_name = "DwmSetWindowAttribute"]
    fn dwm_set(hwnd: HWND, attribute: u32, out: *const c_void, bytes: u32) -> i32;
    #[link_name = "DwmGetWindowAttribute"]
    fn dwm_get(hwnd: HWND, attribute: u32, out: *mut c_void, bytes: u32) -> i32;
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw: AnyWindowHandle = window.into();
    let hwnd = raw.update(cx, |_, window, _| hwnd(window))??;
    let result = verify_hidden_fullscreen(window, hwnd, output, cx).await;
    if let Err(error) = &result {
        let fullscreen = raw.update(cx, |_, window, _| window.is_fullscreen()).ok();
        let window_rect = rect(hwnd).ok();
        fs::write(
            output.join("native-fullscreen-blocker.json"),
            serde_json::to_vec_pretty(&json!({
                "error": format!("{error:#}"),
                "fullscreen_after_cleanup": fullscreen,
                "window_rect_after_cleanup": window_rect,
            }))?,
        )?;
    }
    result
}

async fn verify_hidden_fullscreen(
    window: WindowHandle<MainView>,
    hwnd: HWND,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw: AnyWindowHandle = window.into();
    let guard = CloakedWindow::show(hwnd)?;
    let initial = rect(hwnd)?;
    let monitor = monitor_rect(hwnd)?;
    require(
        initial != monitor,
        "Начальное окно уже совпадает с границами монитора.",
    )?;
    window.update(cx, |view, window, _| -> gpui::Result<()> {
        require(!window.is_fullscreen(), "Начальное окно уже полноэкранное.")?;
        view.allow_native_listening_fullscreen_for_capture();
        Ok(())
    })??;

    let tree = snapshot(raw, cx)?;
    let prefix = format!("Слушать · {TRACK} · ");
    let track = tree["nodes"]
        .as_object()
        .and_then(|nodes| {
            nodes.values().find_map(|node| {
                let id = node["element_id"].as_str()?;
                let title = node["aria"]["label"].as_str()?;
                (id.starts_with("Name(\"library-row-") && title.starts_with(&prefix)).then_some(id)
            })
        })
        .ok_or_else(|| io::Error::other(format!("Нет локальной композиции «{TRACK}».")))?;
    press(raw, track, cx)?;
    super::sound(window, TRACK, cx).await?;
    key(raw, "ctrl-k", cx)?;
    let original_focus = raw.update(cx, |_, window, cx| window.focused(cx))?;
    require(original_focus.is_some(), "Перед входом нет фокуса GPUI.")?;
    save_frame(raw, output, "windowed", cx)?;
    let before = playback(window, cx)?;
    let before_position = position(&before)?;

    key(raw, "ctrl-l", cx)?;
    wait(
        cx,
        "Windows Fullscreen для скрытого DWM окна",
        |cx| {
            guard.verify_cloak()?;
            require(
                position(&playback(window, cx)?)? >= before_position,
                "При входе в Windows Fullscreen позиция звука сбросилась.",
            )?;
            let fullscreen = raw.update(cx, |_, window, _| window.is_fullscreen())?;
            Ok((fullscreen && rect(hwnd)? == monitor).then_some(()))
        },
    )
    .await?;
    let entered = snapshot(raw, cx)?;
    require(
        node(&entered, "listening-fullscreen").is_some(),
        "Нет иммерсивного представления.",
    )?;
    let entered_focus = raw.update(cx, |_, window, cx| window.focused(cx))?;
    require(
        entered_focus.is_some() && entered_focus != original_focus,
        "Вход не перенёс фокус GPUI.",
    )?;
    let full = advance(window, before_position, cx).await?;
    save_frame(raw, output, "native-fullscreen", cx)?;
    press(raw, &named("listening-text"), cx)?;
    save_frame(raw, output, "native-fullscreen-lyrics", cx)?;
    let before_exit = playback(window, cx)?;
    let leaving_position = position(&before_exit)?;

    key(raw, "escape", cx)?;
    wait(
        cx,
        "восстановление границ окна после Escape",
        |cx| {
            guard.verify_cloak()?;
            require(
                position(&playback(window, cx)?)? >= leaving_position,
                "При выходе из Windows Fullscreen позиция звука сбросилась.",
            )?;
            let fullscreen = raw.update(cx, |_, window, _| window.is_fullscreen())?;
            Ok((!fullscreen && rect(hwnd)? == initial).then_some(()))
        },
    )
    .await?;
    let restored = snapshot(raw, cx)?;
    let restored_focus = raw.update(cx, |_, window, cx| window.focused(cx))?;
    require(
        restored_focus == original_focus && node(&restored, "listening-fullscreen").is_none(),
        "Escape не восстановил фокус или оставил полноэкранное представление.",
    )?;
    let after = advance(window, leaving_position, cx).await?;
    save_frame(raw, output, "restored", cx)?;
    guard.verify_cloak()?;
    fs::write(
        output.join("native-fullscreen.json"),
        serde_json::to_vec_pretty(&json!({
            "cloaked_by_application": true,
            "foreground": false,
            "initial_rect": initial,
            "fullscreen_monitor_rect": monitor,
            "restored_rect": rect(hwnd)?,
            "focus_moved_and_restored": true,
            "playback": { "before": before, "fullscreen": full, "before_exit": before_exit, "restored": after }
        }))?,
    )?;
    println!(
        "Нативный Windows Fullscreen вошёл и вышел на скрытом DWM окне. Фокус и воспроизведение сохранены."
    );
    Ok(())
}

fn playback(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Value> {
    window
        .update(cx, |view, _, cx| view.sound_state_for_capture(cx))?
        .filter(|sound| sound["source"] == "Локальный файл")
        .ok_or_else(|| io::Error::other("Потерян локальный звук во время переключения.").into())
}

fn position(value: &Value) -> gpui::Result<f64> {
    value["position"]
        .as_f64()
        .ok_or_else(|| io::Error::other("Нет позиции libmpv.").into())
}

async fn advance(
    window: WindowHandle<MainView>,
    previous: f64,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    wait(
        cx,
        "звук продолжает воспроизводиться",
        |cx| {
            let tree = snapshot(window.into(), cx)?;
            require(
                node_label(&tree, "player-track") == Some(TRACK),
                "Композиция сменилась во время перехода.",
            )?;
            let sound = playback(window, cx)?;
            let position = position(&sound)?;
            require(
                position >= previous,
                "При смене режима позиция воспроизведения сбросилась.",
            )?;
            Ok((position >= previous + 0.2).then_some(sound))
        },
    )
    .await
}

fn save_frame(
    raw: AnyWindowHandle,
    output: &Path,
    name: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.render_to_image()
    })??
    .save(output.join(format!("{name}.png")))?;
    Ok(())
}

struct CloakedWindow(HWND);

impl CloakedWindow {
    fn show(hwnd: HWND) -> gpui::Result<Self> {
        let mut pid = 0;
        // The capture window is live and hidden. The PID confirms ownership.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        require(
            pid == std::process::id() && unsafe { IsWindowVisible(hwnd) == 0 },
            "Полноэкранная проверка требует собственное скрытое окно.",
        )?;
        let enabled = 1_i32;
        let result = unsafe {
            dwm_set(
                hwnd,
                DWMWA_CLOAK,
                (&enabled as *const i32).cast(),
                size_of::<i32>() as u32,
            )
        };
        hresult(result, "DwmSetWindowAttribute(DWMWA_CLOAK)")?;
        let guard = Self(hwnd);
        guard
            .verify_cloak()
            .map_err(|error| io::Error::other(format!("До показа: {error:#}")))?;
        // Unlike the first ShowWindow call, this uses explicit no-activate
        // flags without applying the launcher's STARTUPINFO show state.
        if !set_visibility(hwnd, SWP_SHOWWINDOW) {
            return Err(io::Error::last_os_error().into());
        }
        guard
            .verify_cloak()
            .map_err(|error| io::Error::other(format!("После SWP_SHOWWINDOW: {error:#}")))?;
        require(
            unsafe { IsWindowVisible(hwnd) != 0 },
            "Win32 не установила WS_VISIBLE на cloaked HWND.",
        )?;
        Ok(guard)
    }

    fn verify_cloak(&self) -> gpui::Result<()> {
        let mut flags = 0_u32;
        let result = unsafe {
            dwm_get(
                self.0,
                DWMWA_CLOAKED,
                (&mut flags as *mut u32).cast(),
                size_of::<u32>() as u32,
            )
        };
        hresult(result, "DwmGetWindowAttribute(DWMWA_CLOAKED)")?;
        require(
            flags & DWM_CLOAKED_APP != 0,
            &format!("DWM не подтвердил app cloak, flags={flags:#x}."),
        )?;
        require(
            unsafe { GetForegroundWindow() != self.0 },
            "Cloaked HWND получило foreground.",
        )
    }
}

impl Drop for CloakedWindow {
    fn drop(&mut self) {
        // The GPUI host is closed by the caller directly after the scenario.
        // App cloak remains set until that window is destroyed.
        set_visibility(self.0, SWP_HIDEWINDOW);
    }
}

fn set_visibility(hwnd: HWND, action: u32) -> bool {
    // The handle belongs to the still-live GPUI window owned by this scenario.
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            action | SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE,
        ) != 0
    }
}

fn hresult(code: i32, operation: &str) -> gpui::Result<()> {
    require(
        code >= 0,
        &format!("{operation} вернул HRESULT 0x{:08X}.", code as u32),
    )
}

fn hwnd(window: &Window) -> gpui::Result<HWND> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| io::Error::other(error.to_string()))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
        _ => Err(io::Error::other("Ожидалось окно Win32.").into()),
    }
}

fn as_array(rect: RECT) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

fn rect(hwnd: HWND) -> gpui::Result<[i32; 4]> {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(as_array(rect))
}

fn monitor_rect(hwnd: HWND) -> gpui::Result<[i32; 4]> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(as_array(info.rcMonitor))
}
