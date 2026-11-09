//! INZONE's 0–30 hardware volume levels, presented as percentages in the UI.
const MAX_LEVEL: f64 = 30.0;
pub const PERCENT_STEP: f64 = 100.0 / MAX_LEVEL;

pub fn to_percent(level: f64) -> f64 {
    level.clamp(0.0, MAX_LEVEL) * 100.0 / MAX_LEVEL
}

pub fn from_percent(percent: f64) -> f64 {
    (percent.clamp(0.0, 100.0) * MAX_LEVEL / 100.0).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_half_volume_use_the_percentage_scale() {
        for (level, percent) in [(0.0, 0.0), (15.0, 50.0), (30.0, 100.0)] {
            assert_eq!(to_percent(level), percent);
            assert_eq!(from_percent(percent), level);
        }
    }

    #[test]
    fn every_hardware_level_survives_the_rounded_percentage_display() {
        for level in 0..=30 {
            let displayed = format!("{:.0}", to_percent(f64::from(level))).parse().unwrap();
            assert_eq!(from_percent(displayed), f64::from(level));
        }
    }

    #[test]
    fn typed_percentages_snap_to_a_bounded_hardware_level() {
        assert_eq!(from_percent(25.0), 8.0);
        assert_eq!(from_percent(-10.0), 0.0);
        assert_eq!(from_percent(110.0), 30.0);
        for percent in 0..=100 {
            let level = from_percent(f64::from(percent));
            assert_eq!(level.fract(), 0.0);
            assert!((to_percent(level) - f64::from(percent)).abs() <= PERCENT_STEP / 2.0 + 1e-9);
        }
    }
}
