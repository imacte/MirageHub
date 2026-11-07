//! Shared RsRazer mark. The UI and icon generator use the same geometry.
pub const BACKGROUND: [u8; 3] = [18, 22, 28];
pub const GREEN: [u8; 3] = [68, 214, 44];
pub const WIDTH: f32 = 0.105;
pub const STEM: &[[f32; 2]] = &[
    [0.29, 0.78],
    [0.29, 0.22],
    [0.59, 0.22],
    [0.71, 0.33],
    [0.71, 0.44],
    [0.59, 0.55],
    [0.29, 0.55],
];
pub const LEG: &[[f32; 2]] = &[[0.51, 0.55], [0.74, 0.78]];

fn on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let v = [b[0] - a[0], b[1] - a[1]];
    let t = (((p[0] - a[0]) * v[0] + (p[1] - a[1]) * v[1]) / (v[0] * v[0] + v[1] * v[1]))
        .clamp(0.0, 1.0);
    (p[0] - a[0] - t * v[0]).powi(2) + (p[1] - a[1] - t * v[1]).powi(2) <= (WIDTH / 2.0).powi(2)
}

/// Supersampled RGBA for the window, tray and multi-size executable icon.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut colour = [0u32; 3];
            let mut coverage = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = [
                        (x as f32 + (sx as f32 + 0.5) / 4.0) / size as f32,
                        (y as f32 + (sy as f32 + 0.5) / 4.0) / size as f32,
                    ];
                    let q = [(p[0] - 0.5).abs() - 0.31, (p[1] - 0.5).abs() - 0.31];
                    if q[0].max(0.0).hypot(q[1].max(0.0)) > 0.15 {
                        continue;
                    }
                    coverage += 1;
                    let ink = [STEM, LEG]
                        .iter()
                        .any(|path| path.windows(2).any(|s| on_segment(p, s[0], s[1])));
                    let rgb = if ink { GREEN } else { BACKGROUND };
                    for c in 0..3 {
                        colour[c] += u32::from(rgb[c]);
                    }
                }
            }
            for c in colour {
                pixels.push((c / coverage.max(1)) as u8);
            }
            pixels.push((coverage * 255 / 16) as u8);
        }
    }
    pixels
}
