// SPDX-License-Identifier: GPL-3.0-only
//! Edit the verified Sony schema without rewriting unrelated DSP settings.
use crate::{Error, Result};
use serde_json::{json, Value};
use std::ops::Range;

pub const FREQUENCIES: [f64; 10] = [
    31.5, 63., 125., 250., 500., 1000., 2000., 4000., 8000., 16000.,
];
const ENABLES: [f64; 12] = [1., 1., 1., 1., 1., 1., 1., 1., 1., 1., 0., 0.];

// Sony INZONE Hub 1.0.20.0: Game DynamicRangeCompression and microphone
// AutoGainControl parameters. Preserve all other APO sections and endpoint roles.
const DRC_KEYS: [&str; 12] = [
    "uthreshold",
    "uratio",
    "uattackTime",
    "ureleaseTime",
    "lthreshold",
    "lratio",
    "lattackTime",
    "lreleaseTime",
    "sthreshold",
    "sratio",
    "sattackTime",
    "sreleaseTime",
];
const DRC_LOW: [f64; 12] = [
    -15., 2.5, 0.015, 0.05, -35., 2.5, 0.005, 0.1, -50., 1.9, 0.03, 0.05,
];
const DRC_HIGH: [f64; 12] = [
    -15., 5., 0.015, 0.05, -35., 5., 0.005, 0.1, -50., 2.2, 0.03, 0.05,
];
const MIC_AGC: [f64; 12] = [
    -12., 2., 0.01, 0.2, -44., 2., 0.01, 0.2, -64., 3., 0.01, 0.2,
];

fn invalid() -> Error {
    Error::NotSupported("Unsupported Sony APO configuration".into())
}

pub struct Document {
    lines: Vec<String>,
    newline: &'static str,
    trailing: bool,
}
impl Document {
    pub fn parse(text: &str) -> Result<Self> {
        let doc = Self::read(text)?;
        doc.bands()?;
        doc.boolean("equalizer", "enable")?;
        doc.get("virtualizer", "cef_name_hki")?;
        Ok(doc)
    }
    pub fn parse_microphone(text: &str) -> Result<Self> {
        let doc = Self::read(text)?;
        doc.compression_values()?;
        Ok(doc)
    }
    fn read(text: &str) -> Result<Self> {
        if text.len() > 128 * 1024 || text.contains('\0') {
            return Err(invalid());
        }
        let doc = Self {
            lines: text.lines().map(str::to_owned).collect(),
            newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
            trailing: text.ends_with('\n'),
        };
        Ok(doc)
    }
    fn section(&self, name: &str) -> Result<Range<usize>> {
        let header = format!("{name}:");
        let found: Vec<_> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.split('#').next().unwrap_or_default().trim_end() == header)
            .map(|(i, _)| i)
            .collect();
        if found.len() != 1 {
            return Err(invalid());
        }
        let start = found[0] + 1;
        let end = (start..self.lines.len())
            .find(|&i| {
                let l = &self.lines[i];
                !l.is_empty() && !l.starts_with([' ', '\t', '#'])
            })
            .unwrap_or(self.lines.len());
        Ok(start..end)
    }
    fn index(&self, section: &str, key: &str) -> Result<usize> {
        let mut found = self.section(section)?.filter(|&i| {
            self.lines[i]
                .trim_start()
                .strip_prefix("- ")
                .unwrap_or(self.lines[i].trim_start())
                .split_once(':')
                .is_some_and(|(k, _)| k == key)
        });
        let first = found.next().ok_or_else(invalid)?;
        if found.next().is_some() {
            return Err(invalid());
        }
        Ok(first)
    }
    fn get(&self, section: &str, key: &str) -> Result<&str> {
        Ok(self.lines[self.index(section, key)?]
            .split_once(':')
            .ok_or_else(invalid)?
            .1
            .split('#')
            .next()
            .unwrap_or_default()
            .trim())
    }
    fn set(&mut self, section: &str, key: &str, value: &str) -> Result<()> {
        let i = self.index(section, key)?;
        let (prefix, old) = self.lines[i].split_once(':').ok_or_else(invalid)?;
        let comment = old
            .find('#')
            .map(|at| format!(" {}", &old[at..]))
            .unwrap_or_default();
        self.lines[i] = format!("{prefix}: {value}{comment}");
        Ok(())
    }
    fn array(&self, group: &str, key: &str) -> Result<[f64; 12]> {
        let range = self.section("equalizer")?;
        let heading = format!("  {group}:");
        let headings: Vec<_> = range
            .clone()
            .filter(|&i| self.lines[i].trim_end() == heading)
            .collect();
        if headings.len() != 1 {
            return Err(invalid());
        }
        let start = headings[0] + 1;
        let end = (start..range.end)
            .find(|&i| {
                self.lines[i].starts_with("  ")
                    && !self.lines[i].starts_with("   ")
                    && !self.lines[i].trim().is_empty()
            })
            .unwrap_or(range.end);
        let prefix = format!("- {key}:");
        let matches: Vec<_> = (start..end)
            .filter(|&i| self.lines[i].trim_start().starts_with(&prefix))
            .collect();
        if matches.len() != 1 {
            return Err(invalid());
        }
        let value = self.lines[matches[0]]
            .split_once(':')
            .ok_or_else(invalid)?
            .1
            .split('#')
            .next()
            .unwrap_or_default();
        let values: [f64; 12] = serde_json::from_str(value).map_err(|_| invalid())?;
        if values.iter().any(|v| !v.is_finite()) {
            return Err(invalid());
        }
        Ok(values)
    }
    fn set_array(&mut self, group: &str, key: &str, value: &[f64; 12]) -> Result<()> {
        self.array(group, key)?; // Validate uniqueness and shape before editing.
        let range = self.section("equalizer")?;
        let start = range
            .clone()
            .find(|&i| self.lines[i].trim_end() == format!("  {group}:"))
            .ok_or_else(invalid)?
            + 1;
        let prefix = format!("- {key}:");
        let i = (start..range.end)
            .find(|&i| self.lines[i].trim_start().starts_with(&prefix))
            .ok_or_else(invalid)?;
        let (head, old) = self.lines[i].split_once(':').ok_or_else(invalid)?;
        let comment = old
            .find('#')
            .map(|at| format!(" {}", &old[at..]))
            .unwrap_or_default();
        self.lines[i] = format!("{head}: {}{comment}", serde_json::to_string(value)?);
        Ok(())
    }
    fn boolean(&self, section: &str, key: &str) -> Result<bool> {
        match self.get(section, key)? {
            "true" | "yes" => Ok(true),
            "false" | "no" => Ok(false),
            _ => Err(invalid()),
        }
    }
    pub fn bands(&self) -> Result<[f64; 10]> {
        let fc = self.array("params", "fc")?;
        let kinds = self.array("params", "type")?;
        let q = self.array("params", "q")?;
        let gains = self.array("params", "gain")?;
        if fc[..10] != FREQUENCIES
            || fc[10..] != [0., 0.]
            || kinds[..10] != [2.; 10]
            || q[..10].iter().any(|v| !(0.1..=20.).contains(v))
            || gains[..10].iter().any(|v| !(-12.0..=12.0).contains(v))
        {
            return Err(invalid());
        }
        // Unknown extra active filters cannot be represented by the ten-band UI.
        for group in ["params", "coeffs"] {
            let enabled = self.array(group, "enables")?;
            if enabled != ENABLES {
                return Err(invalid());
            }
        }
        Ok(gains[..10].try_into().unwrap())
    }
    pub fn state(&self) -> Result<Value> {
        let hki = self.get("virtualizer", "cef_name_hki")?;
        let spatial = if !self.boolean("virtualizer", "enable")? {
            "disabled"
        } else if hki == "downmix.hki" {
            "off"
        } else if hki == "standard_hrtf.hki"
            && self.get("virtualizer", "coef_name_ba")? == "WH-G500_standard.ba"
        {
            "standard"
        } else {
            "custom"
        };
        Ok(
            json!({"equalizer":self.boolean("equalizer","enable")?,"bands":self.bands()?,"frequencies":FREQUENCIES,"spatial":spatial,"drc":self.dynamic_range().unwrap_or("unavailable")}),
        )
    }
    fn compression_values(&self) -> Result<[f64; 12]> {
        self.boolean("drc", "enable")?;
        let mut values = [0f64; 12];
        for (value, key) in values.iter_mut().zip(DRC_KEYS) {
            *value = self.get("drc", key)?.parse().map_err(|_| invalid())?;
            if !value.is_finite() {
                return Err(invalid());
            }
        }
        Ok(values)
    }
    pub fn dynamic_range(&self) -> Result<&'static str> {
        let values = self.compression_values()?;
        if !self.boolean("drc", "enable")? {
            return Ok("off");
        }
        if values == DRC_LOW {
            Ok("low")
        } else if values == DRC_HIGH {
            Ok("high")
        } else {
            Ok("custom")
        }
    }
    pub fn auto_gain(&self) -> Result<bool> {
        self.boolean("drc", "enable")
    }
    fn compression(&mut self, enabled: bool, values: Option<&[f64; 12]>) -> Result<()> {
        self.compression_values()?;
        self.set("drc", "enable", if enabled { "true" } else { "false" })?;
        if let Some(values) = values {
            for (key, value) in DRC_KEYS.into_iter().zip(values) {
                self.set("drc", key, &value.to_string())?;
            }
        }
        Ok(())
    }
    pub fn set_dynamic_range(&mut self, mode: &str) -> Result<()> {
        match mode {
            "off" => self.compression(false, None),
            "low" => self.compression(true, Some(&DRC_LOW)),
            "high" => self.compression(true, Some(&DRC_HIGH)),
            _ => Err(Error::Invalid(
                "Dynamic range must be off, low or high".into(),
            )),
        }
    }
    pub fn set_auto_gain(&mut self, enabled: bool) -> Result<()> {
        self.compression(enabled, Some(&MIC_AGC))
    }
    pub fn equalizer(&mut self, enabled: bool) -> Result<()> {
        self.set(
            "equalizer",
            "enable",
            if enabled { "true" } else { "false" },
        )
    }
    pub fn set_bands(&mut self, bands: &[f64]) -> Result<()> {
        if bands.len() != 10
            || bands
                .iter()
                .any(|v| !v.is_finite() || !(-12.0..=12.0).contains(v))
        {
            return Err(Error::Invalid(
                "Sony EQ requires ten finite gains in -12..12 dB".into(),
            ));
        }
        let q = self.array("params", "q")?;
        let mut gains = self.array("params", "gain")?;
        gains[..10].copy_from_slice(bands);
        let mut coefficients = [[0.; 12]; 5];
        for i in 0..10 {
            for (column, value) in
                coefficients
                    .iter_mut()
                    .zip(peaking(FREQUENCIES[i], q[i], bands[i]))
            {
                column[i] = value;
            }
        }
        self.set_array("params", "gain", &gains)?;
        for (key, values) in ["a1", "a2", "b0", "b1", "b2"].into_iter().zip(coefficients) {
            self.set_array("coeffs", key, &values)?;
        }
        Ok(())
    }
    pub fn spatial(&mut self, enabled: bool, root: &str) -> Result<()> {
        if root.contains(['\r', '\n', '\0', '#', '"']) || root.is_empty() {
            return Err(Error::Invalid("Invalid Sony resource directory".into()));
        }
        self.set("virtualizer", "enable", "true")?;
        self.set("virtualizer", "rootpath", root)?;
        self.set(
            "virtualizer",
            "cef_name_hki",
            if enabled {
                "standard_hrtf.hki"
            } else {
                "downmix.hki"
            },
        )?;
        self.set(
            "virtualizer",
            "coef_name_ba",
            if enabled { "WH-G500_standard.ba" } else { "" },
        )?;
        self.set("virtualizer", "alc_cfg_name", "alc.cfg")
    }
    pub fn render(&self) -> String {
        let mut value = self.lines.join(self.newline);
        if self.trailing {
            value.push_str(self.newline);
        }
        value
    }
}

// RBJ peaking EQ, normalized by a0. Verified against Sony's 48 kHz coefficients.
// Formula: https://www.w3.org/TR/audio-eq-cookbook/#peakingeq
fn peaking(frequency: f64, q: f64, gain: f64) -> [f64; 5] {
    let a = 10f64.powf(gain / 40.);
    let omega = std::f64::consts::TAU * frequency / 48000.;
    let alpha = omega.sin() / (2. * q);
    let a0 = 1. + alpha / a;
    let a1 = -2. * omega.cos() / a0;
    [
        a1,
        (1. - alpha / a) / a0,
        (1. + alpha * a) / a0,
        a1,
        (1. - alpha * a) / a0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drc_fixture() -> String {
        let mut text = String::from("drc:\n  - enable: false # preserve\n");
        for (key, value) in DRC_KEYS.into_iter().zip(MIC_AGC) {
            text += &format!("  - {key}: {value}\n");
        }
        text + "noise_reduction:\n  - enable: true\n  - level: 9\n"
    }
    #[test]
    fn compression_levels_preserve_eq_spatial_and_vendor_sections() {
        let original = fixture().replace("drc:\n  - enable: false\n", &drc_fixture());
        let mut doc = Document::parse(&original).unwrap();
        for (mode, ratios) in [("low", [2.5, 2.5, 1.9]), ("high", [5., 5., 2.2])] {
            doc.set_dynamic_range(mode).unwrap();
            let readback = Document::parse(&doc.render()).unwrap();
            assert_eq!(readback.dynamic_range().unwrap(), mode);
            for (key, ratio) in ["uratio", "lratio", "sratio"].into_iter().zip(ratios) {
                assert_eq!(
                    readback.get("drc", key).unwrap().parse::<f64>().unwrap(),
                    ratio
                );
            }
            assert_eq!(
                doc.render().split("drc:").next(),
                original.split("drc:").next()
            );
            assert!(doc
                .render()
                .contains("noise_reduction:\n  - enable: true\n  - level: 9\n"));
        }
        let values = doc.compression_values().unwrap();
        doc.set_dynamic_range("off").unwrap();
        assert_eq!(doc.dynamic_range().unwrap(), "off");
        assert_eq!(doc.compression_values().unwrap(), values);
        let before = doc.render();
        assert!(doc.set_dynamic_range("maximum").is_err());
        assert_eq!(doc.render(), before);
    }
    #[test]
    fn microphone_agc_uses_capture_parameters_and_preserves_other_effects() {
        let original = drc_fixture().replace('\n', "\r\n");
        let mut doc = Document::parse_microphone(&original).unwrap();
        doc.set_auto_gain(true).unwrap();
        assert!(Document::parse_microphone(&doc.render())
            .unwrap()
            .auto_gain()
            .unwrap());
        assert_eq!(
            doc.render(),
            original.replace("false # preserve", "true # preserve")
        );
        doc.set_auto_gain(false).unwrap();
        assert_eq!(doc.render(), original);
        assert!(
            Document::parse_microphone(&original.replace("- uratio: 2", "- uratio: NaN")).is_err()
        );
        assert!(Document::parse_microphone(&(original + "drc:\n  - enable: true\n")).is_err());
    }
    fn fixture() -> String {
        let mut text=String::from("# Keep vendor settings\nvirtualizer:\n  - enable: true\n  - rootpath: C:\\Sony\n  - cef_name_hki: downmix.hki\n  - coef_name_ba: \n  - alc_cfg_name: alc.cfg\nmode_equalizer:\n  enable: false\nequalizer:\n  enable: true # switch\n  coeffs:\n");
        for key in ["enables", "a1", "a2", "b0", "b1", "b2"] {
            text += &format!(
                "    - {key}: {}\n",
                json!(if key == "enables" { ENABLES } else { [0.; 12] })
            );
        }
        text += "  params:\n";
        for (key, values) in [
            ("enables", ENABLES),
            ("type", [2., 2., 2., 2., 2., 2., 2., 2., 2., 2., 0., 0.]),
            (
                "fc",
                [
                    31.5, 63., 125., 250., 500., 1000., 2000., 4000., 8000., 16000., 0., 0.,
                ],
            ),
            (
                "q",
                [
                    1.8, 1.56, 1.56, 1.35, 1., 1.35, 1.56, 1.56, 2.85, 2.85, 0., 0.,
                ],
            ),
            ("gain", [0.; 12]),
        ] {
            text += &format!("    - {key}: {}\n", json!(values));
        }
        text + "drc:\n  - enable: false\nunknown: untouched\n"
    }
    #[test]
    fn preserves_unrelated_sections_comments_and_line_endings() {
        for newline in ["\n", "\r\n"] {
            let original = fixture().replace('\n', newline);
            let mut doc = Document::parse(&original).unwrap();
            assert_eq!(doc.render(), original);
            doc.equalizer(false).unwrap();
            assert_eq!(
                doc.render(),
                original.replace("true # switch", "false # switch")
            );
            doc.set_bands(&[3.; 10]).unwrap();
            let rendered = doc.render();
            assert!(rendered.ends_with(&format!(
                "drc:{newline}  - enable: false{newline}unknown: untouched{newline}"
            )));
            assert_eq!(
                Document::parse(&rendered).unwrap().bands().unwrap(),
                [3.; 10]
            );
        }
    }
    #[test]
    fn rejects_ambiguous_schema_and_bad_inputs() {
        assert!(Document::parse(&(fixture() + "equalizer:\n  enable: true\n")).is_err());
        assert!(Document::parse(&fixture().replace("    - q:", "    - fc:")).is_err());
        let mut doc = Document::parse(&fixture()).unwrap();
        for bands in [vec![0.; 9], vec![f64::NAN; 10], vec![12.1; 10]] {
            assert!(doc.set_bands(&bands).is_err());
        }
        assert_eq!(doc.render(), fixture());
    }
    #[test]
    fn center_frequency_has_requested_gain_and_zero_is_flat() {
        for f in FREQUENCIES {
            for g in [-12., 0., 6., 12.] {
                let [a1, a2, b0, b1, b2] = peaking(f, 1.35, g);
                let w = std::f64::consts::TAU * f / 48000.;
                let power = |c0: f64, c1: f64, c2: f64| {
                    (c0 + c1 * w.cos() + c2 * (2. * w).cos()).powi(2)
                        + (c1 * w.sin() + c2 * (2. * w).sin()).powi(2)
                };
                assert!((10. * (power(b0, b1, b2) / power(1., a1, a2)).log10() - g).abs() < 1e-6);
                if g == 0. {
                    assert_eq!([b0, b1, b2], [1., a1, a2]);
                }
            }
        }
    }
    #[test]
    fn spatial_off_keeps_downmix_and_eq_changes_preserve_spatial() {
        let mut doc = Document::parse(&fixture()).unwrap();
        doc.spatial(true, "C:\\Mirage Hub\\Sony").unwrap();
        doc.set_bands(&[-1.; 10]).unwrap();
        assert_eq!(doc.state().unwrap()["spatial"], "standard");
        doc.spatial(false, "C:\\Mirage Hub\\Sony").unwrap();
        assert_eq!(doc.state().unwrap()["spatial"], "off");
        assert!(doc.boolean("virtualizer", "enable").unwrap());
    }
}
