use super::frame::{hue, mix, scale, Cell, Colour, Frame};
use crate::{device::integer, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    f64::consts::PI,
};

pub const BREATH_PERIOD: f64 = 4.0;
pub const STARLIGHT_PERIOD: f64 = 2.5;
pub const KINDS: [&str; 8] = [
    "static",
    "gradient",
    "breath",
    "spectrum",
    "wave",
    "starlight",
    "reactive",
    "ripple",
];
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub kind: String,
    pub cells: Vec<Cell>,
    pub colours: Vec<Colour>,
    pub speed: f64,
    pub direction: u8,
    pub name: String,
    pub enabled: bool,
}
pub fn colour(value: &Value) -> Result<Colour> {
    let values = value
        .as_array()
        .filter(|v| v.len() >= 3)
        .ok_or_else(|| Error::Invalid("a colour is three numbers 0-255".into()))?;
    Ok([
        integer(&values[0])?,
        integer(&values[1])?,
        integer(&values[2])?,
    ]
    .map(|v| v.clamp(0, 255) as u8))
}
pub fn number(value: &Value) -> Result<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .or_else(|| value.as_bool().map(|v| u8::from(v) as f64))
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::Invalid("expected a finite number".into()))
}
pub fn clean_layers(value: &Value, rows: usize, columns: usize) -> Result<Vec<Layer>> {
    let layers = value
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or_else(|| Error::Invalid("layers must be a list of at most 16 layers".into()))?;
    layers
        .iter()
        .map(|layer| {
            let object = layer
                .as_object()
                .ok_or_else(|| Error::Invalid("a layer is an object".into()))?;
            let kind = layer["kind"]
                .as_str()
                .filter(|k| KINDS.contains(k))
                .ok_or_else(|| Error::Invalid("unknown layer kind".into()))?;
            let mut seen = BTreeSet::new();
            let mut cells = Vec::new();
            if !layer["cells"].is_null() {
                for cell in layer["cells"]
                    .as_array()
                    .ok_or_else(|| Error::Invalid("cells must be a list".into()))?
                {
                    let cell = cell
                        .as_array()
                        .filter(|c| c.len() >= 2)
                        .ok_or_else(|| Error::Invalid("a cell is [row,column]".into()))?;
                    let (r, c) = (integer(&cell[0])?, integer(&cell[1])?);
                    if r >= 0 && c >= 0 && (r as usize) < rows && (c as usize) < columns {
                        let position = [r as usize, c as usize];
                        if seen.insert(position) {
                            cells.push(position);
                        }
                    }
                }
            }
            let mut colours = Vec::new();
            if !layer["colours"].is_null() {
                for value in layer["colours"]
                    .as_array()
                    .ok_or_else(|| Error::Invalid("colours must be a list".into()))?
                    .iter()
                    .take(4)
                {
                    colours.push(colour(value)?);
                }
            }
            if colours.is_empty() {
                colours.push([0, 255, 0]);
            }
            let speed = object
                .get("speed")
                .map(number)
                .transpose()?
                .unwrap_or(1.0)
                .clamp(0.1, 5.0);
            let name = match &layer["name"] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                v => v.to_string(),
            };
            Ok(Layer {
                kind: kind.into(),
                cells,
                colours,
                speed,
                direction: if layer["direction"].as_f64() == Some(2.0) {
                    2
                } else {
                    1
                },
                name: name.chars().take(40).collect(),
                enabled: layer["enabled"] != Value::Bool(false),
            })
        })
        .collect()
}
pub fn twinkle_phase(row: usize, column: usize) -> f64 {
    ((row as u32)
        .wrapping_mul(31)
        .wrapping_add((column as u32).wrapping_mul(17))
        .wrapping_add(7)
        .wrapping_mul(2654435761)) as f64
        / 4294967296.0
}

pub struct LayerStack {
    layers: Vec<Layer>,
    spans: Vec<(usize, usize)>,
    pub animated: bool,
    pub key_driven: bool,
    pub longest_press: f64,
}
impl LayerStack {
    pub fn new(layers: Vec<Layer>) -> Self {
        let layers: Vec<_> = layers
            .into_iter()
            .filter(|l| l.enabled && !l.cells.is_empty())
            .collect();
        let animated = layers
            .iter()
            .any(|l| ["breath", "spectrum", "wave", "starlight"].contains(&l.kind.as_str()));
        let key_driven = layers
            .iter()
            .any(|l| ["reactive", "ripple"].contains(&l.kind.as_str()));
        let longest_press = layers
            .iter()
            .map(|l| match l.kind.as_str() {
                "reactive" => 1.0 / l.speed,
                "ripple" => 0.8 / l.speed,
                _ => 0.0,
            })
            .fold(0.0, f64::max);
        let spans = layers
            .iter()
            .map(|l| {
                (
                    l.cells.iter().map(|p| p[1]).min().unwrap(),
                    l.cells.iter().map(|p| p[1]).max().unwrap(),
                )
            })
            .collect();
        Self {
            layers,
            spans,
            animated,
            key_driven,
            longest_press,
        }
    }
    pub fn render(&self, frame: &mut Frame, elapsed: f64, presses: &[(f64, Cell)]) {
        frame.clear();
        for (layer, (left, right)) in self.layers.iter().zip(&self.spans) {
            let first = layer.colours[0];
            let speed = layer.speed;
            let width = right.saturating_sub(*left).max(1) as f64;
            let mut lit = BTreeMap::<Cell, f64>::new();
            if layer.kind == "reactive" {
                let fade = 1.0 / speed;
                for (age, cell) in presses {
                    if *age < fade {
                        let intensity = lit.entry(*cell).or_default();
                        *intensity = intensity.max(1.0 - age / fade);
                    }
                }
            }
            for &[row, column] in &layer.cells {
                let below = frame.get(row, column);
                let shown = match layer.kind.as_str() {
                    "static" => first,
                    "gradient" => {
                        let last = if layer.colours.len() > 1 {
                            *layer.colours.last().unwrap()
                        } else {
                            [0; 3]
                        };
                        let mut position = (column - left) as f64 / width;
                        if layer.direction == 2 {
                            position = 1.0 - position;
                        }
                        mix(first, last, position)
                    }
                    "breath" => {
                        let cycle = elapsed * speed / BREATH_PERIOD;
                        let colour = layer.colours[(cycle.trunc() as i64)
                            .rem_euclid(layer.colours.len() as i64)
                            as usize];
                        scale(colour, 0.5 - 0.5 * (cycle * 2.0 * PI).cos())
                    }
                    "spectrum" => hue(elapsed * speed / 6.0),
                    "wave" => {
                        let travel = elapsed * speed / 3.0;
                        let sign = if layer.direction == 2 { -1.0 } else { 1.0 };
                        hue((column - left) as f64 / (width + 1.0) - sign * travel)
                    }
                    "starlight" => {
                        let phase = twinkle_phase(row, column);
                        let cycle = elapsed * speed / STARLIGHT_PERIOD + phase * 7.0;
                        let level = (cycle * 2.0 * PI).sin().max(0.0).powf(6.0);
                        mix(
                            below,
                            layer.colours[(phase * 997.0) as usize % layer.colours.len()],
                            level,
                        )
                    }
                    "reactive" => mix(
                        below,
                        first,
                        lit.get(&[row, column]).copied().unwrap_or(0.0),
                    ),
                    "ripple" => {
                        let duration = 0.8 / speed;
                        let mut level = 0.0f64;
                        for (age, [origin_row, origin_column]) in presses {
                            if *age >= duration {
                                continue;
                            }
                            let radius = age / duration * frame.columns as f64;
                            let distance = super::frame::distance(
                                [row, column],
                                [*origin_row, *origin_column],
                            );
                            let delta = (distance - radius).abs();
                            if delta < 1.2 {
                                level = level.max((1.0 - delta / 1.2) * (1.0 - age / duration));
                            }
                        }
                        mix(below, first, level)
                    }
                    _ => below,
                };
                frame.set(row, column, shown);
            }
        }
    }
}
