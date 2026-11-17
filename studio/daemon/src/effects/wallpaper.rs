//! Read the Windows wallpaper image or sample Wallpaper Engine's desktop window.
//! A shared reader exists only while an effect needs wallpaper colours.

use super::frame::Colour;

pub trait WallpaperReader: Send {
    fn sample(&mut self) -> Option<Colour>;
}

pub struct Unavailable;
impl WallpaperReader for Unavailable {
    fn sample(&mut self) -> Option<Colour> {
        None
    }
}

/// Average visible pixels on an evenly spaced grid. Ignore row padding and
/// reject malformed or entirely transparent frames.
pub fn sample_rgba(bytes: &[u8], width: usize, height: usize, pitch: usize) -> Option<Colour> {
    if width == 0 || height == 0 || pitch < width.checked_mul(4)? {
        return None;
    }
    if bytes.len() < pitch.checked_mul(height)? {
        return None;
    }
    let (columns, rows) = (width.min(48), height.min(27));
    let mut total = [0u64; 3];
    let mut weight = 0;
    for row in 0..rows {
        let y = (2 * row + 1) * height / (2 * rows);
        for column in 0..columns {
            let x = (2 * column + 1) * width / (2 * columns);
            let index = y * pitch + x * 4;
            let alpha = u64::from(bytes[index + 3]);
            weight += alpha;
            for channel in 0..3 {
                total[channel] += alpha * u64::from(bytes[index + channel]);
            }
        }
    }
    (weight > 0).then(|| total.map(|value| ((value + weight / 2) / weight) as u8))
}

#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod native;

pub fn open() -> Box<dyn WallpaperReader> {
    #[cfg(windows)]
    {
        native::open()
    }
    #[cfg(not(windows))]
    {
        Box::new(Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_rgba_without_including_row_padding() {
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        assert_eq!(sample_rgba(&pixels, 2, 1, 12), Some([128, 0, 128]));
        assert_eq!(sample_rgba(&[0, 0, 0, 255], 1, 1, 4), Some([0, 0, 0]));
        assert_eq!(sample_rgba(&pixels[..7], 2, 1, 8), None);
        assert_eq!(sample_rgba(&pixels, 2, 1, 7), None);
    }

    #[test]
    fn transparent_capture_is_distinct_from_a_black_wallpaper() {
        let pixels = [255, 255, 255, 0, 0, 0, 0, 255];
        assert_eq!(sample_rgba(&pixels, 2, 1, 8), Some([0, 0, 0]));
        assert_eq!(sample_rgba(&pixels[..4], 1, 1, 4), None);
    }
}
