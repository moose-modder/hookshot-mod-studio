//! sRGB conversions for 8-bit values.

/// An sRGB-encoded byte as linear light, 0..1.
pub fn srgb8_to_linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light, 0..1, as an sRGB-encoded byte.
pub fn linear_to_srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_round_trips() {
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb8(srgb8_to_linear(v)), v);
        }
    }
}
