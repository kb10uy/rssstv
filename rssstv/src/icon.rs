//! The application icon, taken from wherever the platform keeps it.
//!
//! Windows reads it back out of the executable's own resources, which the
//! build script embeds from `assets/icon.ico`: the shell already shows that
//! icon on the file, so the window and task switcher show the same artwork
//! rather than a second copy that could drift from it. Every other platform
//! has no such resource section, so `assets/icon.png` is compiled into the
//! binary and decoded instead.

pub use platform::window_icon;

#[cfg(target_os = "windows")]
mod platform {
    use egui::IconData;
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
            GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
        },
    };

    /// The identifier `assets/rssstv.rc` gives the icon.
    const RESOURCE_ID: u16 = 1;

    /// The size asked of the resource, in pixels.
    ///
    /// An icon group holds several sizes and `LoadImageW` returns the closest
    /// one scaled to what was asked for, so the largest size the format allows
    /// is requested and left to winit to scale down for each place the window
    /// icon appears.
    const WANTED_SIZE: i32 = 256;

    pub fn window_icon() -> Option<IconData> {
        unsafe {
            let module = GetModuleHandleW(ptr::null());
            if module.is_null() {
                return None;
            }
            let handle = LoadImageW(
                module,
                // MAKEINTRESOURCEW: an identifier is passed in place of a name
                // pointer, which is how the resource was declared.
                ptr::without_provenance(RESOURCE_ID as usize),
                IMAGE_ICON,
                WANTED_SIZE,
                WANTED_SIZE,
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

    #[cfg(test)]
    mod tests {
        use super::to_rgba;

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
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use egui::IconData;
    use image::ImageFormat;

    const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

    pub fn window_icon() -> Option<IconData> {
        let image = image::load_from_memory_with_format(ICON_PNG, ImageFormat::Png)
            .ok()?
            .into_rgba8();
        let (width, height) = image.dimensions();
        Some(IconData {
            rgba: image.into_raw(),
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::window_icon;

    /// Whichever platform this runs on has to produce a usable icon.
    #[test]
    fn the_platform_icon_loads() {
        let icon = window_icon().expect("the application icon should be available");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(
            icon.rgba.len(),
            icon.width as usize * icon.height as usize * 4
        );
        assert!(
            icon.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0),
            "the icon should not be fully transparent"
        );
    }
}
