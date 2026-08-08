//! Windows integration.

use std::{
    ffi::c_void,
    mem, ptr,
    sync::atomic::{AtomicI64, Ordering},
};

use egui::IconData;
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, FreeLibrary, GetLastError, HANDLE, HWND,
        INVALID_HANDLE_VALUE,
    },
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
    },
    System::{
        LibraryLoader::{
            GetModuleHandleA, GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32,
            LoadLibraryExA,
        },
        Memory::{
            CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS,
            MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
        },
        Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, EXECUTION_STATE,
            SetThreadExecutionState,
        },
        Threading::CreateMutexW,
    },
    UI::{
        Shell::SetCurrentProcessExplicitAppUserModelID,
        WindowsAndMessaging::{
            DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_ICON, IsIconic, IsWindow,
            LR_DEFAULTCOLOR, LoadImageW, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow,
            ShowWindow,
        },
    },
};

pub const UI_FONTS: [&str; 3] = ["Yu Gothic UI", "Meiryo UI", "Segoe UI"];

pub const FILE_MANAGER: Option<&str> = Some("explorer.exe");

pub const MANUAL_FALLBACK: Option<&str> = None;

pub const APP_DIRECTORY: &str = "RSSSTV";

/// Identifies the application to the shell.
///
/// Taskbar buttons, pinned shortcuts, and notifications are grouped by this
/// string. A process that sets none is grouped by its executable path
/// instead, so a development build and an installed copy would be treated as
/// unrelated applications.
const APP_USER_MODEL_ID: &str = "kb10uy.RSSSTV";

/// Names the objects that coordinate the single-instance claim.
///
/// The `Local\` prefix keeps them in the signed-in session's namespace, so two
/// operators signed in to the same machine each get their own copy.
const INSTANCE_MUTEX: &str = concat!(r"Local\", env!("CARGO_PKG_NAME"), "-instance");
const INSTANCE_WINDOW: &str = concat!(r"Local\", env!("CARGO_PKG_NAME"), "-instance-window");

pub fn prepare_process() {
    let _ = set_app_user_model_id();
    allow_dark_mode_for_app();
}

/// Returns the `HRESULT` the shell answered with.
fn set_app_user_model_id() -> i32 {
    let id = wide(APP_USER_MODEL_ID);
    unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) }
}

/// Encodes `value` as the null-terminated UTF-16 the Win32 API expects.
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Default)]
pub struct Host;

impl super::Platform for Host {
    /// Holds sleep off for as long as the activity lasts.
    ///
    /// `ES_CONTINUOUS` makes the request stand until it is replaced, rather
    /// than nudging the idle timer once, so the flags are restated on every
    /// transition and cleared by the idle case. The display is only kept on
    /// while transmitting, which is short and attended; a reception can finish
    /// with the screen off.
    ///
    /// The request belongs to the calling thread, which is the interface
    /// thread every transition arrives on.
    fn set_activity(&mut self, activity: super::Activity) {
        let state: EXECUTION_STATE = match activity {
            super::Activity::Idle => ES_CONTINUOUS,
            super::Activity::Receiving => ES_CONTINUOUS | ES_SYSTEM_REQUIRED,
            super::Activity::Transmitting => {
                ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
            }
        };
        unsafe { SetThreadExecutionState(state) };
    }
}

/// The claim held by the copy that got there first.
///
/// The mutex is what other copies test. The shared section carries this
/// copy's window handle so a later launch can raise it; it is mapped once and
/// kept mapped, because the window is published after the claim is taken.
pub struct Claim {
    mutex: HANDLE,
    mapping: HANDLE,
    published: *mut c_void,
}

pub fn claim_single_instance() -> Option<Claim> {
    claim_named(INSTANCE_MUTEX, INSTANCE_WINDOW)
}

/// Takes the claim held under `mutex_name`, publishing to `window_name`.
///
/// The names are arguments so tests can claim under their own, rather than
/// contending with a copy the operator has open.
fn claim_named(mutex_name: &str, window_name: &str) -> Option<Claim> {
    unsafe {
        let name = wide(mutex_name);
        let mutex = CreateMutexW(ptr::null(), 1, name.as_ptr());
        if mutex.is_null() {
            // Without the mutex there is no way to tell, and refusing to start
            // would be worse than allowing a second copy.
            return Some(Claim::unheld());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(mutex);
            raise_running_instance(window_name);
            return None;
        }

        let name = wide(window_name);
        let mapping = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            mem::size_of::<i64>() as u32,
            name.as_ptr(),
        );
        let published = if mapping.is_null() {
            ptr::null_mut()
        } else {
            MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, mem::size_of::<i64>()).Value
        };
        Some(Claim {
            mutex,
            mapping,
            published,
        })
    }
}

/// Brings the copy that already holds the claim to the front.
///
/// The window handle is read from the section the running copy published. A
/// launch that lands in the moment between the claim and the window being
/// created finds nothing and simply exits; the running copy is left as it is.
unsafe fn raise_running_instance(window_name: &str) {
    unsafe {
        let name = wide(window_name);
        let mapping = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
        if mapping.is_null() {
            return;
        }
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, mem::size_of::<i64>());
        if !view.Value.is_null() {
            // The mapping is shared with the running copy, which stores the
            // handle concurrently, so the read is atomic rather than a plain
            // load the compiler may assume unshared. The view is page-aligned,
            // which is more than the atomic asks for.
            let hwnd = (*view.Value.cast::<AtomicI64>()).load(Ordering::Acquire) as HWND;
            if !hwnd.is_null() && IsWindow(hwnd) != 0 {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                } else {
                    ShowWindow(hwnd, SW_SHOW);
                }
                // Windows only honors this when the calling process holds the
                // foreground privilege, which a launch the operator just made
                // normally does. When it refuses, it flashes the taskbar
                // button instead, which is the platform's own answer.
                SetForegroundWindow(hwnd);
            }
            UnmapViewOfFile(view);
        }
        CloseHandle(mapping);
    }
}

impl Claim {
    /// A claim that holds nothing, for when the platform would not say.
    const fn unheld() -> Self {
        Self {
            mutex: ptr::null_mut(),
            mapping: ptr::null_mut(),
            published: ptr::null_mut(),
        }
    }

    pub fn publish_window(&self, cc: &eframe::CreationContext<'_>) {
        let (Some(hwnd), false) = (main_window(cc), self.published.is_null()) else {
            return;
        };
        unsafe { (*self.published.cast::<AtomicI64>()).store(hwnd as i64, Ordering::Release) };
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        unsafe {
            if !self.published.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.published,
                });
            }
            for handle in [self.mapping, self.mutex] {
                if !handle.is_null() {
                    CloseHandle(handle);
                }
            }
        }
    }
}

pub fn prepare_window(cc: &eframe::CreationContext<'_>) {
    if let Some(hwnd) = main_window(cc) {
        allow_dark_mode_for_window(hwnd);
    }
}

/// Takes a window off the screen without destroying it.
///
/// The menu bar asks for this on the way out, because a window that is still
/// being torn down stays on screen for as long as the audio devices take to
/// close, which reads as an application that has stopped responding.
pub fn hide_window(hwnd: isize) {
    unsafe { ShowWindow(hwnd as HWND, SW_HIDE) };
}

/// Returns the handle of the window eframe created, if the platform gave one.
fn main_window(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(window) => Some(window.hwnd.get()),
        _ => None,
    }
}

/// The identifier the generated resource script gives the icon.
const ICON_RESOURCE_ID: u16 = 1;

/// The size asked of the resource, in pixels.
///
/// An icon group holds several sizes and `LoadImageW` returns the closest one
/// scaled to what was asked for, so the largest size the format allows is
/// requested and left to winit to scale down for each place the icon appears.
const WANTED_ICON_SIZE: i32 = 256;

pub fn window_icon() -> Option<IconData> {
    resource_icon().or_else(super::embedded_icon)
}

fn resource_icon() -> Option<IconData> {
    unsafe {
        let module = GetModuleHandleW(ptr::null());
        if module.is_null() {
            return None;
        }
        let handle = LoadImageW(
            module,
            // MAKEINTRESOURCEW: an identifier is passed in place of a name
            // pointer, which is how the resource was declared.
            ptr::without_provenance(ICON_RESOURCE_ID as usize),
            IMAGE_ICON,
            WANTED_ICON_SIZE,
            WANTED_ICON_SIZE,
            LR_DEFAULTCOLOR,
        );
        if handle.is_null() {
            return None;
        }
        let icon = handle as HICON;
        let data = icon_data(icon);
        DestroyIcon(icon);
        data
    }
}

/// Reads an icon's color bitmap back as the RGBA egui expects.
unsafe fn icon_data(icon: HICON) -> Option<IconData> {
    unsafe {
        let mut info: ICONINFO = mem::zeroed();
        if GetIconInfo(icon, &mut info) == 0 {
            return None;
        }
        let data = color_bitmap(info.hbmColor);
        for bitmap in [info.hbmColor, info.hbmMask] {
            if !bitmap.is_null() {
                DeleteObject(bitmap.cast());
            }
        }
        data
    }
}

unsafe fn color_bitmap(bitmap: HBITMAP) -> Option<IconData> {
    unsafe {
        if bitmap.is_null() {
            return None;
        }
        let mut described: BITMAP = mem::zeroed();
        let size = mem::size_of::<BITMAP>() as i32;
        if GetObjectW(bitmap.cast(), size, (&raw mut described).cast()) != size {
            return None;
        }
        let width = described.bmWidth;
        let height = described.bmHeight;
        if width <= 0 || height <= 0 {
            return None;
        }

        let mut header: BITMAPINFO = mem::zeroed();
        header.bmiHeader = BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A negative height asks for top-down rows, matching the order
            // egui reads the buffer in.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..mem::zeroed()
        };

        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        let context = GetDC(ptr::null_mut());
        let rows = GetDIBits(
            context,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &raw mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(ptr::null_mut(), context);
        if rows != height {
            return None;
        }

        Some(IconData {
            rgba: to_rgba(pixels),
            width: width as u32,
            height: height as u32,
        })
    }
}

/// Turns the BGRA pixels GDI hands back into RGBA.
///
/// An icon whose frames predate 32-bit alpha comes back fully transparent,
/// which would leave the window with a blank icon rather than a wrong one.
/// Such a frame is taken as opaque; its mask is not consulted, since every
/// frame this application embeds carries real alpha.
fn to_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    let transparent = pixels.chunks_exact(4).all(|pixel| pixel[3] == 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if transparent {
            pixel[3] = u8::MAX;
        }
    }
    pixels
}

fn allow_dark_mode_for_window(hwnd: isize) {
    type AllowDarkModeForWindow = unsafe extern "system" fn(HWND, bool) -> bool;
    const ALLOW_DARK_MODE_FOR_WINDOW_ORDINAL: usize = 133;

    unsafe {
        let module = LoadLibraryExA(
            c"uxtheme.dll".as_ptr().cast(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if module.is_null() {
            return;
        }
        if let Some(function) =
            GetProcAddress(module, ALLOW_DARK_MODE_FOR_WINDOW_ORDINAL as *const u8)
        {
            let allow: AllowDarkModeForWindow = mem::transmute(function);
            allow(hwnd as HWND, true);
        }
        FreeLibrary(module);
    }
}

fn allow_dark_mode_for_app() {
    type AllowDarkModeForApp = unsafe extern "system" fn(bool) -> bool;
    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
    type RefreshImmersiveColorPolicyState = unsafe extern "system" fn();
    const DARK_MODE_FOR_APP_ORDINAL: usize = 135;
    const REFRESH_COLOR_POLICY_ORDINAL: usize = 104;
    const WINDOWS_10_1903_BUILD: u32 = 18_362;
    const PREFERRED_APP_MODE_ALLOW_DARK: i32 = 1;

    let Some(build) = windows_build_number() else {
        return;
    };

    unsafe {
        let module = LoadLibraryExA(
            c"uxtheme.dll".as_ptr().cast(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if module.is_null() {
            return;
        }
        if let Some(function) = GetProcAddress(module, DARK_MODE_FOR_APP_ORDINAL as *const u8) {
            if build < WINDOWS_10_1903_BUILD {
                let allow: AllowDarkModeForApp = mem::transmute(function);
                allow(true);
            } else {
                let set_preference: SetPreferredAppMode = mem::transmute(function);
                set_preference(PREFERRED_APP_MODE_ALLOW_DARK);
            }
        }
        if let Some(function) = GetProcAddress(module, REFRESH_COLOR_POLICY_ORDINAL as *const u8) {
            let refresh: RefreshImmersiveColorPolicyState = mem::transmute(function);
            refresh();
        }
        FreeLibrary(module);
    }
}

fn windows_build_number() -> Option<u32> {
    type RtlGetNtVersionNumbers = unsafe extern "system" fn(*mut u32, *mut u32, *mut u32);

    unsafe {
        let module = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        if module.is_null() {
            return None;
        }
        let function = GetProcAddress(module, c"RtlGetNtVersionNumbers".as_ptr().cast())?;
        let get_version: RtlGetNtVersionNumbers = mem::transmute(function);
        let mut major = 0;
        let mut minor = 0;
        let mut build = 0;
        get_version(&mut major, &mut minor, &mut build);
        build &= !0xf000_0000;
        (major == 10 && minor == 0 && build >= 17_763).then_some(build)
    }
}

#[cfg(test)]
mod tests {
    use super::{claim_named, set_app_user_model_id, to_rgba};

    /// Names nothing else contends for, so the test does not disturb a copy
    /// the operator has open.
    fn test_names() -> (String, String) {
        let process = std::process::id();
        (
            format!(r"Local\rssstv-test-instance-{process}"),
            format!(r"Local\rssstv-test-window-{process}"),
        )
    }

    #[test]
    fn a_second_claim_is_refused_until_the_first_is_released() {
        let (mutex, window) = test_names();
        let first = claim_named(&mutex, &window).expect("the first claim should be granted");
        assert!(
            claim_named(&mutex, &window).is_none(),
            "a second claim should be refused while the first is held"
        );

        drop(first);
        assert!(
            claim_named(&mutex, &window).is_some(),
            "releasing the claim should let the next one through"
        );
    }

    /// S_OK. The shell refuses a late call, so this also pins that the
    /// identifier is set before anything else asks the shell for one.
    #[test]
    fn the_shell_accepts_the_app_user_model_id() {
        assert_eq!(set_app_user_model_id(), 0);
    }

    #[test]
    fn bgra_pixels_are_reordered() {
        let pixels = to_rgba(vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(pixels, vec![3, 2, 1, 4, 7, 6, 5, 8]);
    }

    /// A frame without alpha has to stay visible.
    #[test]
    fn a_fully_transparent_bitmap_is_taken_as_opaque() {
        let pixels = to_rgba(vec![1, 2, 3, 0, 5, 6, 7, 0]);
        assert_eq!(pixels, vec![3, 2, 1, 255, 7, 6, 5, 255]);
    }
}
