use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES,
            GetLastError, HANDLE, HWND, SetLastError, WAIT_OBJECT_0, WIN32_ERROR,
        },
        System::{Diagnostics::ToolHelp::*, Registry::*, Threading::*},
        UI::{Controls::Dialogs::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, PWSTR, w},
};

pub struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            tracing::error!(%error, "CloseHandle failed");
        }
    }
}
pub fn process_path(pid: u32) -> Result<PathBuf> {
    let process =
        OwnedHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }?);
    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }?;
    Ok(OsString::from_wide(&buf[..len as usize]).into())
}
pub fn processes() -> Result<Vec<PathBuf>> {
    let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    unsafe { Process32FirstW(snapshot.0, &mut entry) }?;
    let mut paths = Vec::new();
    loop {
        if entry.th32ProcessID != std::process::id() {
            match process_path(entry.th32ProcessID) {
                Ok(path)
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")) =>
                {
                    paths.push(path)
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(pid = entry.th32ProcessID, %error, "process not accessible")
                }
            }
        }
        if let Err(err) = unsafe { Process32NextW(snapshot.0, &mut entry) } {
            if err.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break;
            }
            return Err(err.into());
        }
    }
    paths.sort_by_cached_key(|path| {
        (
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
            path.to_string_lossy().to_lowercase(),
        )
    });
    paths.dedup();
    Ok(paths)
}

pub struct Instance(OwnedHandle);
impl Instance {
    pub fn acquire() -> Result<Option<Self>> {
        unsafe {
            SetLastError(WIN32_ERROR(0));
        }
        let event = unsafe { CreateEventW(None, false, false, w!("Local\\PadPointer-Show-0913")) }?;
        let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = OwnedHandle(event);
        if existing {
            unsafe { SetEvent(handle.0) }?;
            Ok(None)
        } else {
            Ok(Some(Self(handle)))
        }
    }
    pub fn show_requested(&self) -> bool {
        unsafe { WaitForSingleObject(self.0.0, 0) == WAIT_OBJECT_0 }
    }
}

pub struct HotkeyRegistration(Option<(i32, crate::config::Hotkey)>);
impl HotkeyRegistration {
    pub fn new() -> Self {
        // Create the thread's message queue before registering the hotkey.
        unsafe {
            let _ = PeekMessageW(&mut MSG::default(), None, 0, 0, PM_NOREMOVE);
        }
        Self(None)
    }
    pub fn set(&mut self, h: crate::config::Hotkey) -> Result<()> {
        if self.0.is_some_and(|(_, active)| active == h) {
            return Ok(());
        }
        let next = if self.0.is_some_and(|(id, _)| id == 1) {
            2
        } else {
            1
        };
        unsafe {
            RegisterHotKey(
                None,
                next,
                HOT_KEY_MODIFIERS(h.modifiers) | MOD_NOREPEAT,
                u32::from(h.key),
            )
        }
        .context("无法注册热键，可能已被其他程序或 Windows 占用")?;
        if let Some((old, _)) = self.0.replace((next, h))
            && let Err(error) = unsafe { UnregisterHotKey(None, old) }
        {
            tracing::error!(%error, "cannot unregister previous hotkey");
        }
        Ok(())
    }
    pub fn toggled(&self) -> bool {
        let mut msg = MSG::default();
        let mut toggle = false;
        while unsafe { PeekMessageW(&mut msg, None, WM_HOTKEY, WM_HOTKEY, PM_REMOVE).as_bool() } {
            if self.0.is_some_and(|(id, _)| id == msg.wParam.0 as i32) {
                toggle = !toggle;
            }
        }
        toggle
    }
}
impl Drop for HotkeyRegistration {
    fn drop(&mut self) {
        if let Some((id, _)) = self.0
            && let Err(error) = unsafe { UnregisterHotKey(None, id) }
        {
            tracing::error!(%error, "UnregisterHotKey failed");
        }
    }
}

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const ENTRY: PCWSTR = w!("PadPointer");
pub fn startup_enabled() -> Result<bool> {
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            ENTRY,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    result.ok()?;
    Ok(true)
}
pub fn set_startup(enabled: bool, exe: &Path) -> Result<()> {
    if enabled {
        let mut value = OsString::from("\"");
        value.push(exe);
        value.push("\" --startup");
        let wide = crate::config::wide(&value);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN,
                ENTRY,
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            )
        }
        .ok()?;
    } else {
        let result = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, ENTRY) };
        if result != ERROR_FILE_NOT_FOUND {
            result.ok()?;
        }
    }
    Ok(())
}
pub fn system_dark() -> Result<bool> {
    let mut value: u32 = 1;
    let mut len: u32 = 4;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut len),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    result.ok()?;
    Ok(value == 0)
}

/// Match the window frame to the Slint theme: the title bar follows the light or
/// dark theme, and on Windows 11 the caption and border are tinted to the app
/// background so the frame blends with the page. Returns `false` while the
/// native window does not exist yet, so the caller can retry after showing it.
pub fn apply_window_theme(window: &slint::Window, dark: bool) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::{WinitWindowAccessor, winit};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DwmSetWindowAttribute,
    };

    let applied = std::cell::Cell::new(false);
    // COLORREF is 0x00BBGGRR; both colors are gray, so the byte order is moot.
    let caption = if dark { 0x1C1C1C } else { 0xF3F3F3 };
    window.with_winit_window(|winit_window| {
        winit_window.set_theme(Some(if dark {
            winit::window::Theme::Dark
        } else {
            winit::window::Theme::Light
        }));
        applied.set(true);
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = HWND(win32.hwnd.get() as *mut std::ffi::c_void);
        for attribute in [DWMWA_CAPTION_COLOR, DWMWA_BORDER_COLOR] {
            let result = unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    attribute,
                    (&raw const caption).cast(),
                    size_of::<i32>() as u32,
                )
            };
            if let Err(error) = result {
                tracing::debug!(%error, "window frame color not supported");
            }
        }
    });
    applied.get()
}
pub fn message(text: &str) {
    let message: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("PadPointer"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub enum Dialog {
    Game,
    Import,
    Export,
}
pub fn file_dialog(action: Dialog, folder: &Path, owner: HWND) -> Result<Option<PathBuf>> {
    let mut file = vec![0u16; 32768];
    let initial = crate::config::wide(folder.as_os_str());
    let export = matches!(action, Dialog::Export);
    if export {
        for (out, c) in file.iter_mut().zip("PadPointer.json".encode_utf16()) {
            *out = c;
        }
    }
    let mut options = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: if matches!(action, Dialog::Game) {
            w!("可执行文件 (*.exe)\0*.exe\0\0")
        } else {
            w!("PadPointer 配置 (*.json)\0*.json\0\0")
        },
        lpstrDefExt: if matches!(action, Dialog::Game) {
            w!("exe")
        } else {
            w!("json")
        },
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrInitialDir: PCWSTR(initial.as_ptr()),
        Flags: OFN_EXPLORER
            | OFN_NOCHANGEDIR
            | OFN_PATHMUSTEXIST
            | if export {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            },
        ..Default::default()
    };
    let chosen = unsafe {
        if export {
            GetSaveFileNameW(&mut options)
        } else {
            GetOpenFileNameW(&mut options)
        }
    };
    if chosen.as_bool() {
        let len = file
            .iter()
            .position(|&c| c == 0)
            .context("文件对话框返回的路径过长")?;
        return Ok(Some(OsString::from_wide(&file[..len]).into()));
    }
    let code = unsafe { CommDlgExtendedError() };
    if code.0 != 0 {
        bail!("文件对话框失败，代码 {:#x}", code.0);
    }
    Ok(None)
}
