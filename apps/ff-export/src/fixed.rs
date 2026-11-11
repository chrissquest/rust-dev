//! Fixed-point conversions used by the mesh, animation and GPU command formats.
//!
//! Four widths appear, all signed, and they do **not** share one scale:
//!
//! | Name | Storage | Fraction bits | Where |
//! | --- | --- | --- | --- |
//! | `FixedPoint2012` | 32-bit (20 integer bits) | 12 | bone matrices, animation keyframes, mesh scale |
//! | `FixedPoint412` | 16-bit (4 integer bits) | 12 | vertex coordinates |
//! | `FixedPoint124` | 16-bit (12 integer bits) | **4** | texture coordinates |
//!
//! The 12 fractional bits are not a guess: the reference recorded that the mesh header's first word
//! is `131072`, and `131072 / 4096 = 32.0` matches the "for ff1, always 32" scale note.
//!
//! Texture coordinates are the odd one out at four fractional bits, so a raw value is in units of
//! **1/16 texel**, and the reference then divides by the texture size (`textureImageParameter`, which
//! says 256x256 for Breme) to normalise it. That is where the trap was: `16 * 256 == 4096`, so the
//! reference's first exported coordinate, `0.77490234375`, looks exactly like `3174 / 4096` and
//! "confirmed" twelve fractional bits. It is really `3174 / 16 / 256`, and dividing by 4096 *and*
//! 256 shrank every creature's UVs into a fraction of a texel - which renders as one blurry smear of
//! colour over the whole model.

/// Fractional bits in the 20.12 and 4.12 formats.
pub const FRACTIONAL_BITS: u32 = 12;
/// `1 << FRACTIONAL_BITS`, as a float.
pub const ONE: f64 = 4096.0;
/// Fractional bits in the 12.4 format used by texture coordinates.
pub const UV_FRACTIONAL_BITS: u32 = 4;
/// `1 << UV_FRACTIONAL_BITS`: a texture coordinate raw value is in units of 1/16 texel.
pub const UV_ONE: f64 = 16.0;

/// 20.12 signed fixed point (32-bit storage).
pub fn from_2012(raw: u32) -> f64 {
    (raw as i32) as f64 / ONE
}

/// 4.12 signed fixed point (16-bit storage).
pub fn from_412(raw: i16) -> f64 {
    raw as f64 / ONE
}

/// 12.4 signed fixed point (16-bit storage): texture coordinates, in 1/16 texel units.
///
/// Still needs dividing by the texture size to become a normalised coordinate.
pub fn from_124(raw: i16) -> f64 {
    raw as f64 / UV_ONE
}

/// Encodes a value back into 20.12 form, for round-trip tests.
pub fn to_2012(value: f64) -> u32 {
    (value * ONE).round() as i32 as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mesh_scale_word_decodes_to_32() {
        // every mesh header starts with 0x00020000 and the format note says "always 32"
        assert_eq!(from_2012(0x0002_0000), 32.0);
    }

    #[test]
    fn texture_coordinates_are_in_sixteenths_of_a_texel() {
        assert_eq!(from_124(16), 1.0);
        assert_eq!(from_124(0), 0.0);
        assert_eq!(from_124(4046), 252.875);
        assert_eq!(from_124(-32), -2.0);
    }

    #[test]
    fn a_normalised_texture_coordinate_matches_the_reference() {
        // The reference's first exported coordinate for Breme is 0.77490234375. It is a 12.4 value
        // divided by the texture size (256, from `textureImageParameter`), and *not* a 4.12 value:
        // 3174 / 16 = 198.375 texels, 198.375 / 256 = 0.77490234375.
        let texels = from_124(3174);
        assert_eq!(texels, 198.375);
        assert!((texels / 256.0 - 0.774_902_343_75).abs() < f64::EPSILON);
        // dividing by 4096 instead would give a sixteenth of that - the bug this replaced
        assert!((texels / 4096.0 - 0.774_902_343_75 / 16.0).abs() < f64::EPSILON);
    }

    #[test]
    fn negative_values_are_signed() {
        assert_eq!(from_2012((-4096i32) as u32), -1.0);
        assert_eq!(from_412(-2048), -0.5);
    }

    #[test]
    fn round_trips_through_2012() {
        for value in [-10.5, -1.0, 0.0, 1.0, 32.0, 1234.25] {
            let raw = to_2012(value);
            assert!((from_2012(raw) - value).abs() < 1.0 / ONE);
        }
    }
}
