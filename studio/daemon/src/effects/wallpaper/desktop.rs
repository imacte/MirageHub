use super::{sample_rgba, Colour};
use std::{
    ffi::OsString,
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr,
    time::SystemTime,
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
            WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
            WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_MULTITHREADED,
        },
        UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DSS_ENABLED},
    },
};

#[derive(Default)]
pub(super) struct Desktop {
    image: Option<(PathBuf, SystemTime, u64, Colour)>,
}

impl Desktop {
    pub(super) fn sample(&mut self) -> Option<Colour> {
        // Keep COM interfaces local to the calling effect worker.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
            let colour = self.read();
            CoUninitialize();
            colour
        }
    }

    unsafe fn read(&mut self) -> Option<Colour> {
        let desktop: IDesktopWallpaper = CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).ok()?;
        if desktop.GetStatus().ok()?.contains(DSS_ENABLED) {
            for index in 0..desktop.GetMonitorDevicePathCount().ok()? {
                let Some(monitor) = desktop.GetMonitorDevicePathAt(index).ok().and_then(take_string)
                else {
                    continue;
                };
                let monitor = PCWSTR(monitor.as_ptr());
                let Ok(rect) = desktop.GetMonitorRECT(monitor) else {
                    continue;
                };
                // The primary monitor starts at (0, 0) in virtual-screen coordinates.
                if rect.left == 0 && rect.top == 0 && rect.right > 0 && rect.bottom > 0 {
                    let path = take_string(desktop.GetWallpaper(monitor).ok()?);
                    if let Some(path) = path.filter(|path| path.len() > 1) {
                        return self.image_colour(PathBuf::from(OsString::from_wide(
                            &path[..path.len() - 1],
                        )));
                    }
                    break;
                }
            }
        }
        let [r, g, b, _] = desktop.GetBackgroundColor().ok()?.0.to_le_bytes();
        Some([r, g, b])
    }

    unsafe fn image_colour(&mut self, path: PathBuf) -> Option<Colour> {
        let metadata = fs::metadata(&path).ok()?;
        let modified = metadata.modified().ok()?;
        let size = metadata.len();
        if let Some((cached_path, cached_time, cached_size, colour)) = &self.image {
            if *cached_path == path && *cached_time == modified && *cached_size == size {
                return Some(*colour);
            }
        }
        let colour = decode(&path)?;
        self.image = Some((path, modified, size, colour));
        Some(colour)
    }
}

fn take_string(wide: PWSTR) -> Option<Vec<u16>> {
    if wide.is_null() {
        return None;
    }
    unsafe {
        let string = wide.as_wide().iter().copied().chain([0]).collect();
        CoTaskMemFree(Some(wide.0.cast()));
        Some(string)
    }
}

unsafe fn decode(path: &Path) -> Option<Colour> {
    let name: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_ALL).ok()?;
    let decoder = wic
        .CreateDecoderFromFilename(
            PCWSTR(name.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
        .ok()?;
    let frame = decoder.GetFrame(0).ok()?;
    let (mut width, mut height) = (0, 0);
    frame.GetSize(&mut width, &mut height).ok()?;
    let (width, height) = (width.min(48), height.min(27));
    let scaler = wic.CreateBitmapScaler().ok()?;
    scaler
        .Initialize(&frame, width, height, WICBitmapInterpolationModeFant)
        .ok()?;
    // WIC can change the pixel format during scaling. Convert the final pixels.
    let converter = wic.CreateFormatConverter().ok()?;
    converter
        .Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .ok()?;
    let pitch = width * 4;
    let mut pixels = vec![0; (pitch * height) as usize];
    converter.CopyPixels(ptr::null(), pitch, &mut pixels).ok()?;
    sample_rgba(&pixels, width as usize, height as usize, pitch as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwritten_wallpaper_is_decoded_again() {
        // A one-pixel, 24-bit BMP with a padded BGR row.
        let mut bmp = [0u8; 58];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2] = 58;
        bmp[10] = 54;
        bmp[14] = 40;
        bmp[18] = 1;
        bmp[22] = 1;
        bmp[26] = 1;
        bmp[28] = 24;
        bmp[54..57].copy_from_slice(&[20, 80, 200]);
        let path = std::env::temp_dir().join(format!("mirage-wallpaper-{}.bmp", std::process::id()));
        fs::write(&path, bmp).unwrap();

        let mut desktop = Desktop::default();
        let (before, after) = unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            let before = desktop.image_colour(path.clone());
            bmp[54..57].copy_from_slice(&[160, 40, 10]);
            // Change the size as well so the test works on filesystems with
            // coarse modification timestamps.
            fs::write(&path, [bmp.as_slice(), &[0]].concat()).unwrap();
            let after = desktop.image_colour(path.clone());
            CoUninitialize();
            (before, after)
        };
        fs::remove_file(path).unwrap();
        assert_eq!(before, Some([200, 80, 20]));
        assert_eq!(after, Some([10, 40, 160]));
    }
}
