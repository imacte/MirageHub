//! Deterministic renderer. Time, key presses, audio level and accent colour
//! are supplied by the worker, allowing every frame to be compared offline.
use super::{
    frame::{self, Cell, Colour, Frame},
    layers::{self, LayerStack, BREATH_PERIOD, STARLIGHT_PERIOD},
};
use crate::{Error, Result};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, f64::consts::PI};

pub const SOFTWARE_EFFECTS: [&str; 10] = [
    "ripple",
    "ripple_random",
    "wave_soft",
    "spectrum_soft",
    "layers",
    "music",
    "zones",
    "breath_soft",
    "starlight_soft",
    "accent",
];
pub const ZONE_EFFECTS: [&str; 8] = [
    "none",
    "static",
    "spectrum_soft",
    "wave_soft",
    "music",
    "breath_soft",
    "starlight_soft",
    "accent",
];
pub const WINDOWS_BLUE: Colour = [0, 0x78, 0xd4];
pub const RIPPLE_DURATION: f64 = 0.6;

pub fn dimensions(capabilities: &Value) -> [usize; 2] {
    let value = &capabilities["matrix_dimensions"];
    [
        value[0].as_u64().unwrap_or(1) as usize,
        value[1].as_u64().unwrap_or(1) as usize,
    ]
}
pub fn can_render(capabilities: &Value) -> bool {
    let [r, c] = dimensions(capabilities);
    capabilities["custom_frame"] == true && r.saturating_mul(c) > 1
}
pub fn can_render_keys(capabilities: &Value) -> bool {
    let [r, c] = dimensions(capabilities);
    can_render(capabilities) && r.saturating_mul(c) >= 8
}
pub fn zone_order(capabilities: &Value) -> Vec<String> {
    capabilities["zone_order"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
pub fn clean_colour(value: &Value) -> Result<Colour> {
    if value.is_null() || value.as_array().is_some_and(Vec::is_empty) {
        Ok([0, 255, 0])
    } else {
        layers::colour(value)
    }
}
pub fn clean_zones(order: &[String], value: &Value) -> Result<Value> {
    if order.is_empty() {
        return Err(Error::NotSupported(
            "device has no zones to give effects of their own".into(),
        ));
    }
    let zones = value
        .as_object()
        .ok_or_else(|| Error::Invalid("zones must map zone names to effect and colour".into()))?;
    if let Some(unknown) = zones.keys().find(|key| !order.contains(key)) {
        return Err(Error::Invalid(format!("device has no zone {unknown}")));
    }
    let mut result = Map::new();
    for zone in order {
        let wanted = zones.get(zone).unwrap_or(&Value::Null);
        if !wanted.is_null() && !wanted.is_object() {
            return Err(Error::Invalid(format!("zone {zone} must be an object")));
        }
        let effect = wanted["effect"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("none");
        if !ZONE_EFFECTS.contains(&effect) {
            return Err(Error::Invalid(format!("unknown zone effect {effect}")));
        }
        result.insert(
            zone.clone(),
            json!({"effect":effect,"colour":clean_colour(&wanted["colour"])?}),
        );
    }
    Ok(Value::Object(result))
}
#[derive(Clone, Debug)]
pub struct Press {
    pub at: f64,
    pub cell: Cell,
    pub colour: Colour,
}

pub struct Renderer {
    pub effect: String,
    pub options: Map<String, Value>,
    pub frame: Frame,
    pub refresh: f64,
    pub colour: Colour,
    pub still: bool,
    pub stack: Option<LayerStack>,
    pub phase: f64,
    pub recent_peak: f64,
    pub level: f64,
    pub presses: Vec<Press>,
    keymap: &'static BTreeMap<String, Cell>,
    zone_effects: Vec<(String, Colour)>,
    shown: Option<Frame>,
    accent: Colour,
}
impl Renderer {
    pub fn new(effect: &str, options: Map<String, Value>, capabilities: &Value) -> Result<Self> {
        if !SOFTWARE_EFFECTS.contains(&effect) {
            return Err(Error::NotSupported(format!(
                "unknown software effect: {effect}"
            )));
        }
        let [rows, columns] = dimensions(capabilities);
        let frame = Frame::new(rows, columns)?;
        let refresh = options
            .get("refresh_rate")
            .map(layers::number)
            .transpose()?
            .unwrap_or(0.04);
        if !(0.001..=3600.0).contains(&refresh) {
            return Err(Error::Invalid(
                "refresh_rate must be between 0.001 and 3600 seconds".into(),
            ));
        }
        let colour = clean_colour(options.get("colour").unwrap_or(&Value::Null))?;
        let mut zone_effects: Vec<(String, Colour)> = Vec::new();
        if effect == "zones" {
            let order = zone_order(capabilities);
            let clean = clean_zones(&order, options.get("zones").unwrap_or(&Value::Null))?;
            for zone in order {
                zone_effects.push((
                    clean[&zone]["effect"].as_str().unwrap().into(),
                    layers::colour(&clean[&zone]["colour"])?,
                ));
            }
        }
        let still = effect == "accent"
            || (effect == "zones"
                && zone_effects
                    .iter()
                    .all(|(effect, _)| ["none", "static", "accent"].contains(&effect.as_str())));
        let stack = if effect == "layers" {
            Some(LayerStack::new(layers::clean_layers(
                options.get("layers").unwrap_or(&Value::Null),
                rows,
                columns,
            )?))
        } else {
            None
        };
        Ok(Self {
            effect: effect.into(),
            options,
            frame,
            refresh,
            colour,
            still,
            stack,
            phase: 0.0,
            recent_peak: 0.0,
            level: 0.0,
            presses: vec![],
            keymap: frame::keymap(capabilities["name"].as_str().unwrap_or("")),
            zone_effects,
            shown: None,
            accent: WINDOWS_BLUE,
        })
    }
    pub fn needs_keys(&self) -> bool {
        self.stack
            .as_ref()
            .map(|s| s.key_driven)
            .unwrap_or_else(|| ["ripple", "ripple_random"].contains(&self.effect.as_str()))
    }
    pub fn needs_audio(&self) -> bool {
        self.effect == "music"
            || self
                .zone_effects
                .iter()
                .any(|(effect, _)| effect == "music")
    }
    pub fn needs_accent(&self) -> bool {
        self.effect == "accent"
            || self
                .zone_effects
                .iter()
                .any(|(effect, _)| effect == "accent")
    }
    pub fn interval(&self) -> f64 {
        if self
            .stack
            .as_ref()
            .is_some_and(|s| !s.animated && !s.key_driven)
        {
            1.0
        } else {
            self.refresh
        }
    }
    pub fn on_key(&mut self, key: &str, elapsed: f64, random_hue: f64) {
        let Some(&cell) = self.keymap.get(key) else {
            return;
        };
        if self.stack.is_some() && (cell[0] >= self.frame.rows || cell[1] >= self.frame.columns) {
            return;
        }
        let colour = if self.effect == "ripple_random" {
            frame::hue(random_hue)
        } else {
            self.colour
        };
        self.presses.push(Press {
            at: elapsed,
            cell,
            colour,
        });
    }
    pub fn step(&mut self, elapsed: f64, audio_peak: f64, accent: Option<Colour>) -> bool {
        if let Some(colour) = accent {
            self.accent = colour;
        }
        if let Some(stack) = &self.stack {
            self.presses
                .retain(|press| elapsed - press.at < stack.longest_press);
            let presses: Vec<_> = self
                .presses
                .iter()
                .map(|p| (elapsed - p.at, p.cell))
                .collect();
            stack.render(&mut self.frame, elapsed, &presses);
            let changed = self.shown.as_ref() != Some(&self.frame);
            self.shown = Some(self.frame.clone());
            return changed;
        }
        let before = self.frame.clone();
        let changed = match self.effect.as_str() {
            "ripple" | "ripple_random" => self.ripple(elapsed),
            "spectrum_soft" => {
                for column in 0..self.frame.columns {
                    self.fill_column(column, self.spectrum_at(column));
                }
                true
            }
            "wave_soft" => {
                for column in 0..self.frame.columns {
                    self.fill_column(column, self.wave_at(column, self.colour));
                }
                true
            }
            "music" => {
                let level = self.hear(audio_peak);
                for column in 0..self.frame.columns {
                    self.fill_column(column, self.music_at(column, level, self.colour));
                }
                self.frame != before
            }
            "zones" => {
                let level = if self.needs_audio() {
                    self.hear(audio_peak)
                } else {
                    0.0
                };
                for column in 0..self.zone_effects.len() {
                    let (effect, colour) = self.zone_effects[column].clone();
                    if ["breath_soft", "starlight_soft", "accent"].contains(&effect.as_str()) {
                        for row in 0..self.frame.rows {
                            let shown = self.cell_at(&effect, colour, row, column, elapsed);
                            self.frame.set(row, column, shown);
                        }
                    } else {
                        let shown = match effect.as_str() {
                            "static" => colour,
                            "spectrum_soft" => self.spectrum_at(column),
                            "wave_soft" => self.wave_at(column, colour),
                            "music" => self.music_at(column, level, colour),
                            _ => [0; 3],
                        };
                        self.fill_column(column, shown);
                    }
                }
                self.frame != before
            }
            effect => {
                for row in 0..self.frame.rows {
                    for column in 0..self.frame.columns {
                        let shown = self.cell_at(effect, self.colour, row, column, elapsed);
                        self.frame.set(row, column, shown);
                    }
                }
                self.frame != before
            }
        };
        if changed
            || ["zones", "breath_soft", "starlight_soft", "accent"].contains(&self.effect.as_str())
        {
            self.phase = (self.phase + self.refresh) % 10.0;
        }
        changed
    }
    fn fill_column(&mut self, column: usize, colour: Colour) {
        for row in 0..self.frame.rows {
            self.frame.set(row, column, colour);
        }
    }
    fn spectrum_at(&self, column: usize) -> Colour {
        frame::hue(self.phase / 5.0 + column as f64 / self.frame.columns as f64)
    }
    fn wave_at(&self, column: usize, colour: Colour) -> Colour {
        let position = column as f64 / self.frame.columns as f64 - (self.phase / 2.0) % 1.0;
        frame::scale(colour, ((position * PI * 2.0).cos() * 0.5 + 0.5).max(0.0))
    }
    fn hear(&mut self, peak: f64) -> f64 {
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        self.recent_peak = peak.max(self.recent_peak * 0.998).max(0.02);
        let heard = (peak / self.recent_peak).powf(2.0);
        self.level = heard.max(self.level * 0.88);
        self.level.max(0.06)
    }
    fn music_at(&self, column: usize, level: f64, colour: Colour) -> Colour {
        let centre = (self.frame.columns - 1) as f64 / 2.0;
        let distance = (column as f64 - centre).abs() / centre.max(1.0);
        frame::scale(colour, level * (1.0 - distance * (1.0 - level)))
    }
    fn cell_at(
        &self,
        effect: &str,
        colour: Colour,
        row: usize,
        column: usize,
        elapsed: f64,
    ) -> Colour {
        match effect {
            "breath_soft" => frame::scale(
                colour,
                0.5 - 0.5 * (elapsed / BREATH_PERIOD * 2.0 * PI).cos(),
            ),
            "starlight_soft" => frame::scale(
                colour,
                ((elapsed / STARLIGHT_PERIOD + layers::twinkle_phase(row, column) * 7.0)
                    * 2.0
                    * PI)
                    .sin()
                    .max(0.0)
                    .powf(6.0),
            ),
            "accent" => self.accent,
            _ => [0; 3],
        }
    }
    fn ripple(&mut self, elapsed: f64) -> bool {
        self.presses.retain(|p| elapsed - p.at < RIPPLE_DURATION);
        if self.presses.is_empty() {
            let changed = self.frame.pixels().iter().any(|c| *c != [0; 3]);
            self.frame.clear();
            return changed;
        }
        self.frame.clear();
        for press in &self.presses {
            let age = elapsed - press.at;
            let radius = age * (self.frame.columns as f64 / RIPPLE_DURATION);
            for row in 0..self.frame.rows {
                for column in 0..self.frame.columns {
                    let distance = frame::distance([row, column], press.cell);
                    let delta = (distance - radius).abs();
                    if delta > 1.2 {
                        continue;
                    }
                    let intensity =
                        (1.0 - delta / 1.2).max(0.0) * (1.0 - age / RIPPLE_DURATION).max(0.0);
                    let lit = frame::scale(press.colour, intensity);
                    let previous = self.frame.get(row, column);
                    self.frame.set(
                        row,
                        column,
                        std::array::from_fn(|i| lit[i].max(previous[i])),
                    );
                }
            }
        }
        true
    }
}
