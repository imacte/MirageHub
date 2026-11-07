use crate::{Error, Result};
use std::{collections::BTreeMap, sync::OnceLock};

pub type Colour = [u8; 3];
pub type Cell = [usize; 2];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub rows: usize,
    pub columns: usize,
    data: Vec<Colour>,
}
impl Frame {
    pub fn new(rows: usize, columns: usize) -> Result<Self> {
        let (rows, columns) = (rows.max(1), columns.max(1));
        if rows > 256 || columns > 256 {
            return Err(Error::Invalid(
                "matrix dimensions exceed the byte-addressed protocol".into(),
            ));
        }
        Ok(Self {
            rows,
            columns,
            data: vec![[0; 3]; rows * columns],
        })
    }
    pub fn clear(&mut self) {
        self.data.fill([0; 3]);
    }
    pub fn fill(&mut self, colour: Colour) {
        self.data.fill(colour);
    }
    pub fn set(&mut self, row: usize, column: usize, colour: Colour) {
        if row < self.rows && column < self.columns {
            self.data[row * self.columns + column] = colour;
        }
    }
    pub fn get(&self, row: usize, column: usize) -> Colour {
        if row < self.rows && column < self.columns {
            self.data[row * self.columns + column]
        } else {
            [0; 3]
        }
    }
    pub fn pixels(&self) -> &[Colour] {
        &self.data
    }
    pub fn payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.rows * (3 + self.columns * 3));
        for row in 0..self.rows {
            bytes.extend_from_slice(&[row as u8, 0, (self.columns - 1) as u8]);
            for colour in &self.data[row * self.columns..(row + 1) * self.columns] {
                bytes.extend_from_slice(colour);
            }
        }
        bytes
    }
}

pub fn keymap(name: &str) -> &'static BTreeMap<String, Cell> {
    static MAPS: OnceLock<BTreeMap<String, BTreeMap<String, Cell>>> = OnceLock::new();
    let maps = MAPS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../openrazer_win/devices/data/keymap.json"
        ))
        .expect("embedded keymap")
    });
    let name = name.to_lowercase();
    let family = if name.contains("tartarus") {
        "tartarus"
    } else if name.contains("orbweaver") {
        "orbweaver"
    } else if name.contains("naga hex v2") {
        "naga_hex_v2"
    } else {
        "default"
    };
    &maps[family]
}

pub fn hue(hue: f64) -> Colour {
    let h = hue.rem_euclid(1.0) * 6.0;
    let i = h as usize;
    let f = h - i as f64;
    // Keep colorsys.hsv_to_rgb's operation order, including cancellation in t.
    let q = 1.0 - f;
    let t = 1.0 - (1.0 - f);
    let (r, g, b) = match i % 6 {
        0 => (1.0, t, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, t),
        3 => (0.0, q, 1.0),
        4 => (t, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}
pub fn scale(colour: Colour, amount: f64) -> Colour {
    colour.map(|v| (f64::from(v) * amount) as u8)
}
pub fn mix(below: Colour, above: Colour, amount: f64) -> Colour {
    let amount = amount.clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        (f64::from(below[i]) + (f64::from(above[i]) - f64::from(below[i])) * amount) as u8
    })
}

pub fn distance(a: Cell, b: Cell) -> f64 {
    // Small integer matrix coordinates give an exact squared distance before sqrt.
    let y = (a[0] as f64 - b[0] as f64) * 2.0;
    let x = a[1] as f64 - b[1] as f64;
    (x * x + y * y).sqrt()
}
