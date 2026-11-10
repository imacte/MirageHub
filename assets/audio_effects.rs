//! Portable H5 Game-output effects, shared by the UI and daemon.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DynamicRange {
    Off,
    Low,
    High,
}
impl DynamicRange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Spatial {
    Off,
    Standard,
}
impl Spatial {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Standard => "standard",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioEffects {
    pub equalizer: bool,
    pub bands: [f64; 10],
    pub drc: DynamicRange,
    pub spatial: Spatial,
}

impl AudioEffects {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .bands
            .iter()
            .any(|v| !v.is_finite() || !(-12.0..=12.0).contains(v))
        {
            return Err("Audio effects require ten finite EQ gains in -12..12 dB");
        }
        Ok(())
    }

    pub fn from_state(state: &serde_json::Value) -> Option<Self> {
        let effects = Self {
            equalizer: state["equalizer"].as_bool()?,
            bands: serde_json::from_value(state["bands"].clone()).ok()?,
            drc: serde_json::from_value(state["drc"].clone()).ok()?,
            spatial: serde_json::from_value(state["spatial"].clone()).ok()?,
        };
        effects.validate().ok()?;
        Some(effects)
    }
}
