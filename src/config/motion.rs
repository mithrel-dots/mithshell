use serde::{Deserialize, Serialize};

/// Optional overrides are applied before the global duration scale.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionOverride {
    pub duration_ms: Option<u32>,
    pub easing: Option<MotionEasing>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MotionEasing {
    Standard,
    StandardDecelerate,
    StandardAccelerate,
    Emphasized,
    EmphasizedAccelerate,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PeekGeometryConfig {
    /// Density-scaled logical pixels; also retained by the pinned open header.
    #[serde(deserialize_with = "deserialize_peek_lift")]
    pub lift: f64,
    /// Multiplier of the reference peek width, subject to content minimums.
    #[serde(deserialize_with = "deserialize_peek_width_scale")]
    pub width_scale: f64,
}

impl Default for PeekGeometryConfig {
    fn default() -> Self {
        Self {
            lift: 8.0,
            width_scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionConfig {
    #[serde(deserialize_with = "deserialize_duration_scale")]
    pub duration_scale: f64,
    pub peek: PeekGeometryConfig,
    pub peek_enter: MotionOverride,
    pub peek_exit: MotionOverride,
    pub island_open: MotionOverride,
    pub island_close: MotionOverride,
    pub launcher_open: MotionOverride,
    pub launcher_close: MotionOverride,
    pub circle_enter: MotionOverride,
    pub circle_exit: MotionOverride,
    pub hover_enter: MotionOverride,
    pub container_expand: MotionOverride,
    pub container_collapse: MotionOverride,
    pub content_in: MotionOverride,
    pub content_out: MotionOverride,
    pub island_fade_in: MotionOverride,
    pub island_fade_out: MotionOverride,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            duration_scale: 1.0,
            peek: PeekGeometryConfig::default(),
            peek_enter: MotionOverride::default(),
            peek_exit: MotionOverride::default(),
            island_open: MotionOverride::default(),
            island_close: MotionOverride::default(),
            launcher_open: MotionOverride::default(),
            launcher_close: MotionOverride::default(),
            circle_enter: MotionOverride::default(),
            circle_exit: MotionOverride::default(),
            hover_enter: MotionOverride::default(),
            container_expand: MotionOverride::default(),
            container_collapse: MotionOverride::default(),
            content_in: MotionOverride::default(),
            content_out: MotionOverride::default(),
            island_fade_in: MotionOverride::default(),
            island_fade_out: MotionOverride::default(),
        }
    }
}

fn deserialize_duration_scale<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(d)?;
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "duration_scale must be finite and nonnegative",
        ))
    }
}

fn deserialize_peek_lift<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(d)?;
    if value.is_finite() && (0.0..=128.0).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "peek.lift must be between 0 and 128",
        ))
    }
}

fn deserialize_peek_width_scale<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(d)?;
    if value.is_finite() && value > 0.0 && value <= 4.0 {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "peek.width_scale must be greater than 0 and at most 4",
        ))
    }
}
