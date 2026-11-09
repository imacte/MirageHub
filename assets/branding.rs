//! Shared Mirage Hub linework for the UI, Windows icons and website favicon.
use std::sync::OnceLock;

pub const BACKGROUND: [u8; 3] = [13, 17, 23];
pub const GREEN: [u8; 3] = [68, 214, 44];
pub const VIEW_BOX: f32 = 128.0;
pub const CORNER_RADIUS: f32 = 27.0 / VIEW_BOX;
pub const WIDTH: f32 = 5.5 / VIEW_BOX;

/// Each path starts with one point, followed by cubic control/control/end triples.
/// The three gaps let the strands pass under one another, without animal heads.
/// `mirage-hub-tools icons` exports these same curves to the website SVG.
pub const CURVES: [[[f32; 2]; 28]; 3] = [
    [
        [57.51, 99.68],
        [51.86, 101.99],
        [45.0, 102.56],
        [38.61, 100.68],
        [32.22, 98.81],
        [26.4, 94.47],
        [22.83, 88.67],
        [19.26, 82.88],
        [18.0, 75.73],
        [19.21, 69.15],
        [20.43, 62.58],
        [24.07, 56.69],
        [28.76, 52.68],
        [33.44, 48.67],
        [39.04, 46.55],
        [44.01, 45.88],
        [48.97, 45.22],
        [53.23, 45.91],
        [56.48, 46.64],
        [59.72, 47.37],
        [61.99, 48.04],
        [64.26, 48.0],
        [66.53, 47.95],
        [68.91, 47.19],
        [72.29, 46.47],
        [75.67, 45.76],
        [80.1, 45.18],
        [85.15, 46.06],
    ],
    [
        [98.15, 51.78],
        [102.97, 55.52],
        [106.89, 61.17],
        [108.46, 67.65],
        [110.03, 74.12],
        [109.19, 81.32],
        [105.95, 87.31],
        [102.72, 93.3],
        [97.16, 97.98],
        [90.85, 100.21],
        [84.55, 102.44],
        [77.63, 102.24],
        [71.82, 100.18],
        [66.0, 98.13],
        [61.36, 94.34],
        [58.3, 90.37],
        [55.25, 86.41],
        [53.72, 82.37],
        [52.73, 79.2],
        [51.74, 76.02],
        [51.19, 73.72],
        [50.01, 71.77],
        [48.83, 69.83],
        [46.99, 68.15],
        [44.68, 65.58],
        [42.37, 63.02],
        [39.65, 59.47],
        [37.89, 54.66],
    ],
    [
        [36.35, 40.54],
        [37.17, 34.49],
        [40.1, 28.27],
        [44.93, 23.67],
        [49.75, 19.07],
        [56.41, 16.21],
        [63.22, 16.01],
        [70.02, 15.82],
        [76.85, 18.3],
        [81.93, 22.64],
        [87.02, 26.98],
        [90.3, 33.08],
        [91.43, 39.14],
        [92.56, 45.2],
        [91.59, 51.12],
        [89.69, 55.75],
        [87.78, 60.38],
        [85.05, 63.72],
        [82.79, 66.16],
        [80.54, 68.6],
        [78.82, 70.24],
        [77.73, 72.23],
        [76.63, 74.22],
        [76.1, 76.65],
        [75.03, 79.94],
        [73.96, 83.23],
        [72.25, 87.35],
        [68.96, 91.28],
    ],
];

/// Cache the flattened curves once; UI painting does not rebuild them each frame.
pub fn paths() -> &'static [Vec<[f32; 2]>; 3] {
    static PATHS: OnceLock<[Vec<[f32; 2]>; 3]> = OnceLock::new();
    PATHS.get_or_init(|| {
        CURVES.map(|curve| {
            let mut points = vec![curve[0].map(|v| v / VIEW_BOX)];
            for i in (1..curve.len()).step_by(3) {
                let [a, b, c, d] = [curve[i - 1], curve[i], curve[i + 1], curve[i + 2]];
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    points.push(std::array::from_fn(|axis| {
                        (u * u * u * a[axis]
                            + 3.0 * u * u * t * b[axis]
                            + 3.0 * u * t * t * c[axis]
                            + t * t * t * d[axis])
                            / VIEW_BOX
                    }));
                }
            }
            points
        })
    })
}

fn on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let v = [b[0] - a[0], b[1] - a[1]];
    let t = (((p[0] - a[0]) * v[0] + (p[1] - a[1]) * v[1]) / (v[0] * v[0] + v[1] * v[1]))
        .clamp(0.0, 1.0);
    (p[0] - a[0] - t * v[0]).powi(2) + (p[1] - a[1] - t * v[1]).powi(2) <= (WIDTH / 2.0).powi(2)
}

/// Supersampled RGBA for the window, tray and multi-size executable icon.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let samples = size * 4;
    let scale = samples as f32;
    let radius = WIDTH / 2.0;
    let mut ink = vec![false; (samples * samples) as usize];
    // Rasterize each segment only inside its bounds. Scanning every segment for
    // every sample would make the curved mark expensive during application startup.
    for path in paths() {
        for segment in path.windows(2) {
            let [a, b] = [segment[0], segment[1]];
            let lower = |axis: usize| ((a[axis].min(b[axis]) - radius) * scale).floor().max(0.0) as u32;
            let upper = |axis: usize| ((a[axis].max(b[axis]) + radius) * scale).ceil().min(scale) as u32;
            for y in lower(1)..upper(1) {
                for x in lower(0)..upper(0) {
                    let p = [(x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale];
                    if on_segment(p, a, b) {
                        ink[(y * samples + x) as usize] = true;
                    }
                }
            }
        }
    }
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut colour = [0u32; 3];
            let mut coverage = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let sample_x = x * 4 + sx;
                    let sample_y = y * 4 + sy;
                    let p = [(sample_x as f32 + 0.5) / scale, (sample_y as f32 + 0.5) / scale];
                    let q = [
                        (p[0] - 0.5).abs() - (0.5 - CORNER_RADIUS),
                        (p[1] - 0.5).abs() - (0.5 - CORNER_RADIUS),
                    ];
                    if q[0].max(0.0).hypot(q[1].max(0.0)) > CORNER_RADIUS {
                        continue;
                    }
                    coverage += 1;
                    let rgb = if ink[(sample_y * samples + sample_x) as usize] { GREEN } else { BACKGROUND };
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
