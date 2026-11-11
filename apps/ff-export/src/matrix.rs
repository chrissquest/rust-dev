//! `Matrix4x3_2012`: the affine transform used for bone bind poses, animation keyframes and the
//! mesh's `matrixLoad4x3` command.
//!
//! Twelve 20.12 fixed-point words: three basis vectors followed by the translation, i.e. the file
//! order is `x.x x.y x.z  y.x y.y y.z  z.x z.y z.z  t.x t.y t.z`. Because the basis vectors are
//! stored as `SIMD3` in the reference, they are consecutive - so this is a flat 12-word read.
//!
//! Transforming a point is `v.x * x + v.y * y + v.z * z + translation`, which means the basis
//! vectors are the *columns* of the equivalent 4x4 matrix.

use anyhow::Result;

use crate::bytes::Reader;
use crate::fixed;

pub type Vec3 = [f64; 3];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix4x3 {
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
    pub translation: Vec3,
}

impl Default for Matrix4x3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix4x3 {
    pub const IDENTITY: Self = Self {
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        z: [0.0, 0.0, 1.0],
        translation: [0.0, 0.0, 0.0],
    };

    /// Reads 12 consecutive 20.12 words.
    pub fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let mut values = [0.0f64; 12];
        for value in &mut values {
            *value = fixed::from_2012(reader.u32()?);
        }
        Ok(Self {
            x: [values[0], values[1], values[2]],
            y: [values[3], values[4], values[5]],
            z: [values[6], values[7], values[8]],
            translation: [values[9], values[10], values[11]],
        })
    }

    /// Builds one from 12 already-decoded values, in the same order `read` consumes them.
    pub fn from_values(values: &[f64; 12]) -> Self {
        Self {
            x: [values[0], values[1], values[2]],
            y: [values[3], values[4], values[5]],
            z: [values[6], values[7], values[8]],
            translation: [values[9], values[10], values[11]],
        }
    }

    /// `v.x * x + v.y * y + v.z * z + translation`.
    pub fn transform(&self, v: Vec3) -> Vec3 {
        [
            v[0] * self.x[0] + v[1] * self.y[0] + v[2] * self.z[0] + self.translation[0],
            v[0] * self.x[1] + v[1] * self.y[1] + v[2] * self.z[1] + self.translation[1],
            v[0] * self.x[2] + v[1] * self.y[2] + v[2] * self.z[2] + self.translation[2],
        ]
    }

    /// The equivalent 4x4 matrix in glTF's column-major order.
    ///
    /// glTF stores matrices column by column, and our basis vectors are already the columns, so the
    /// translation lands in the last four entries.
    pub fn to_gltf_matrix(&self) -> [f32; 16] {
        [
            self.x[0] as f32,
            self.x[1] as f32,
            self.x[2] as f32,
            0.0,
            self.y[0] as f32,
            self.y[1] as f32,
            self.y[2] as f32,
            0.0,
            self.z[0] as f32,
            self.z[1] as f32,
            self.z[2] as f32,
            0.0,
            self.translation[0] as f32,
            self.translation[1] as f32,
            self.translation[2] as f32,
            1.0,
        ]
    }

    /// True when every component is finite, which the defective exported rotations were not.
    pub fn is_finite(&self) -> bool {
        self.x.iter().chain(&self.y).chain(&self.z).chain(&self.translation).all(|value| value.is_finite())
    }

    /// The inverse of this affine transform, or `None` when the basis is degenerate.
    ///
    /// The three basis vectors are the columns of the 3x3 part, so inverting means inverting that
    /// 3x3 by cofactors and then mapping the translation through it: the inverse of `[R | t]` is
    /// `[R^-1 | -R^-1 t]`. glTF wants this for a skin's inverse bind matrices.
    pub fn inverse(&self) -> Option<Self> {
        let (a, b, c) = (self.x, self.y, self.z);

        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - b[0] * (a[1] * c[2] - a[2] * c[1])
            + c[0] * (a[1] * b[2] - a[2] * b[1]);
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let scale = 1.0 / det;

        // cofactors of the 3x3 part, already transposed into inverse-column order
        let ix = [
            (b[1] * c[2] - b[2] * c[1]) * scale,
            -(a[1] * c[2] - a[2] * c[1]) * scale,
            (a[1] * b[2] - a[2] * b[1]) * scale,
        ];
        let iy = [
            -(b[0] * c[2] - b[2] * c[0]) * scale,
            (a[0] * c[2] - a[2] * c[0]) * scale,
            -(a[0] * b[2] - a[2] * b[0]) * scale,
        ];
        let iz = [
            (b[0] * c[1] - b[1] * c[0]) * scale,
            -(a[0] * c[1] - a[1] * c[0]) * scale,
            (a[0] * b[1] - a[1] * b[0]) * scale,
        ];

        let t = self.translation;
        let translation = [
            -(ix[0] * t[0] + iy[0] * t[1] + iz[0] * t[2]),
            -(ix[1] * t[0] + iy[1] * t[1] + iz[1] * t[2]),
            -(ix[2] * t[0] + iy[2] * t[1] + iz[2] * t[2]),
        ];

        Some(Self {
            x: ix,
            y: iy,
            z: iz,
            translation,
        })
    }
}

/// A transform split into the form glTF animates: translation, rotation, scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    /// Quaternion as `(x, y, z, w)`.
    pub rotation: [f64; 4],
    pub scale: Vec3,
}

fn length(v: Vec3) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn scale_vector(v: Vec3, factor: f64) -> Vec3 {
    [v[0] * factor, v[1] * factor, v[2] * factor]
}

fn cross_product(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot_product(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Quaternion of a pure rotation given as three orthonormal columns.
///
/// Uses the largest-diagonal branch, which is numerically stable; the plain trace formula divides by
/// a quantity that goes to zero as the rotation approaches 180 degrees.
fn quaternion_from_columns(x: Vec3, y: Vec3, z: Vec3) -> [f64; 4] {
    // the vectors are columns, so these are the matrix rows
    let (m00, m10, m20) = (x[0], x[1], x[2]);
    let (m01, m11, m21) = (y[0], y[1], y[2]);
    let (m02, m12, m22) = (z[0], z[1], z[2]);
    let trace = m00 + m11 + m22;

    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    }
}

impl Matrix4x3 {
    /// Splits the transform into translation, rotation and scale - what glTF animates.
    ///
    /// The basis vectors are the columns of a rotation, each scaled by one factor, so their lengths
    /// are the scale and normalising them leaves a pure rotation. A negative determinant means the
    /// basis is mirrored, so the x scale is negated and its column flipped to keep the quaternion a
    /// proper rotation.
    ///
    /// Normalising first is the whole point. The reference extracts the quaternion from the raw,
    /// still-scaled matrix, where the trace is not the rotation's trace - that is where its `-nan`
    /// rotations came from.
    pub fn decompose(&self) -> Transform {
        let mut scale = [length(self.x), length(self.y), length(self.z)];

        let normalized = |v: Vec3, len: f64, fallback: Vec3| {
            if len > 1e-12 {
                scale_vector(v, 1.0 / len)
            } else {
                fallback
            }
        };

        let mut x = normalized(self.x, scale[0], [1.0, 0.0, 0.0]);
        let y = normalized(self.y, scale[1], [0.0, 1.0, 0.0]);
        let z = normalized(self.z, scale[2], [0.0, 0.0, 1.0]);

        if dot_product(cross_product(x, y), z) < 0.0 {
            x = scale_vector(x, -1.0);
            scale[0] = -scale[0];
        }

        // A source matrix with any shear in it does not decompose into a rotation at all, so the
        // quaternion comes out slightly short (the validator flagged lengths around 0.98). glTF
        // requires unit quaternions, and TRS cannot express shear anyway, so normalise.
        let mut rotation = quaternion_from_columns(x, y, z);
        let norm = (rotation[0] * rotation[0]
            + rotation[1] * rotation[1]
            + rotation[2] * rotation[2]
            + rotation[3] * rotation[3])
            .sqrt();
        if norm > 1e-12 {
            for component in &mut rotation {
                *component /= norm;
            }
        } else {
            rotation = [0.0, 0.0, 0.0, 1.0];
        }

        Transform {
            translation: self.translation,
            rotation,
            scale,
        }
    }

    /// Builds a transform from translation, rotation (quaternion `x, y, z, w`) and scale.
    ///
    /// This is the inverse of [`Matrix4x3::decompose`], and exists so the two can be tested against
    /// each other.
    pub fn from_trs(translation: Vec3, rotation: [f64; 4], scale: Vec3) -> Self {
        let [x, y, z, w] = rotation;
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);

        let rotation = [
            [1.0 - (yy + zz), xy + wz, xz - wy],
            [xy - wz, 1.0 - (xx + zz), yz + wx],
            [xz + wy, yz - wx, 1.0 - (xx + yy)],
        ];

        Self {
            x: scale_vector(rotation[0], scale[0]),
            y: scale_vector(rotation[1], scale[1]),
            z: scale_vector(rotation[2], scale[2]),
            translation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(values: &[f64; 12]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(&fixed::to_2012(*value).to_le_bytes());
        }
        bytes
    }

    #[test]
    fn reads_and_transforms_the_identity() {
        let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let bytes = reader(&identity);
        let matrix = Matrix4x3::read(&mut Reader::new(&bytes)).unwrap();
        assert!(matrix.is_finite());
        assert_eq!(matrix.transform([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn applies_translation_and_scale() {
        // scale x2 in x, and translate by (10, 0, -5)
        let values = [2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 10.0, 0.0, -5.0];
        let bytes = reader(&values);
        let matrix = Matrix4x3::read(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(matrix.transform([1.0, 1.0, 1.0]), [12.0, 1.0, -4.0]);
    }

    #[test]
    fn gltf_matrix_is_column_major() {
        let values = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0];
        let matrix = Matrix4x3::from_values(&values);
        let gltf = matrix.to_gltf_matrix();
        assert_eq!(&gltf[12..15], &[1.0, 2.0, 3.0]);
        assert_eq!(gltf[15], 1.0);
    }

    #[test]
    fn the_inverse_undoes_the_transform() {
        // a scale, a shear and a translation - nothing special, just not the identity
        let values = [2.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.25, 3.0, 10.0, -4.0, 2.0];
        let matrix = Matrix4x3::from_values(&values);
        let inverse = matrix.inverse().expect("an invertible basis");

        for point in [[0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [-5.5, 0.25, 7.0]] {
            let there = matrix.transform(point);
            let back = inverse.transform(there);
            for axis in 0..3 {
                assert!(
                    (back[axis] - point[axis]).abs() < 1e-9,
                    "axis {axis}: {back:?} should round-trip to {point:?}"
                );
            }
        }
    }

    #[test]
    fn a_degenerate_basis_has_no_inverse() {
        // every basis vector is zero
        let mut degenerate = Matrix4x3::IDENTITY;
        degenerate.x = [0.0, 0.0, 0.0];
        degenerate.y = [0.0, 0.0, 0.0];
        degenerate.z = [0.0, 0.0, 0.0];
        assert!(degenerate.inverse().is_none());
    }

    #[test]
    fn decompose_and_build_agree() {
        let translation = [10.0, -3.0, 0.25];
        let scale = [2.0, 1.0, 0.5];
        let half_angle = std::f64::consts::FRAC_PI_4;
        let rotation = [0.0, 0.0, half_angle.sin(), half_angle.cos()]; // 90 degrees about z

        let matrix = Matrix4x3::from_trs(translation, rotation, scale);
        let split = matrix.decompose();

        for axis in 0..3 {
            assert!((split.translation[axis] - translation[axis]).abs() < 1e-9);
            assert!((split.scale[axis] - scale[axis]).abs() < 1e-9);
        }

        // quaternions are only defined up to sign, so compare the rebuilt transform instead
        let rebuilt = Matrix4x3::from_trs(split.translation, split.rotation, split.scale);
        for point in [[0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [-4.0, 0.5, 7.0]] {
            let a = matrix.transform(point);
            let b = rebuilt.transform(point);
            for axis in 0..3 {
                assert!((a[axis] - b[axis]).abs() < 1e-9, "axis {axis}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn decompose_handles_a_mirrored_basis() {
        // a negative determinant: x is negated
        let matrix = Matrix4x3::from_trs([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [-1.0, 1.0, 1.0]);
        let split = matrix.decompose();
        assert!(split.rotation.iter().all(|value| value.is_finite()));

        let rebuilt = Matrix4x3::from_trs(split.translation, split.rotation, split.scale);
        let point = [1.0, 2.0, 3.0];
        let a = matrix.transform(point);
        let b = rebuilt.transform(point);
        for axis in 0..3 {
            assert!((a[axis] - b[axis]).abs() < 1e-9, "axis {axis}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn decompose_never_produces_non_finite_values() {
        // the shape that broke the reference: scaled, and not orthonormal
        let values = [2.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.25, 3.0, 10.0, -4.0, 2.0];
        let split = Matrix4x3::from_values(&values).decompose();
        assert!(
            split.rotation.iter().all(|value| value.is_finite()),
            "rotation {:?}",
            split.rotation
        );
        assert!(split.scale.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn decompose_always_returns_a_unit_quaternion() {
        // glTF requires unit quaternions, and the validator checks - a sheared matrix decomposes to a
        // short one unless it is normalised.
        let cases: [[f64; 12]; 3] = [
            [2.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.25, 3.0, 10.0, -4.0, 2.0],
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            [0.0, -3.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1.5, 1.0, 2.0, 3.0],
        ];

        for values in cases {
            let rotation = Matrix4x3::from_values(&values).decompose().rotation;
            let length = (rotation[0] * rotation[0]
                + rotation[1] * rotation[1]
                + rotation[2] * rotation[2]
                + rotation[3] * rotation[3])
                .sqrt();
            assert!(
                (length - 1.0).abs() < 1e-12,
                "quaternion length was {length} for {values:?}"
            );
        }
    }
}
