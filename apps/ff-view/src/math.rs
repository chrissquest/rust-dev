//! The handful of linear-algebra operations a viewer needs.
//!
//! Matrices are column-major, which is both glTF's array order and WGSL's
//! `mat4x4<f32>` memory layout, so nothing has to be transposed on the way to
//! the GPU. This deliberately is not a general-purpose math library: if the
//! engine later wants one, this module is the thing that gets replaced.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(value: f32) -> Self {
        Self::new(value, value, value)
    }

    pub const fn from_array(value: [f32; 3]) -> Self {
        Self::new(value[0], value[1], value[2])
    }

    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns zero for a zero-length vector rather than NaN.
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length > 1e-20 {
            self / length
        } else {
            Self::ZERO
        }
    }

    pub fn min(self, other: Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    pub fn max(self, other: Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        self + (other - self) * t
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Mul<Vec3> for f32 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Vec3 {
        rhs * self
    }
}

impl Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(value: [f32; 3]) -> Self {
        Self::from_array(value)
    }
}

impl From<Vec3> for [f32; 3] {
    fn from(value: Vec3) -> Self {
        value.to_array()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub const fn from_xyzw(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// glTF stores rotations as `[x, y, z, w]`.
    pub const fn from_array(value: [f32; 4]) -> Self {
        Self::from_xyzw(value[0], value[1], value[2], value[3])
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn normalize(self) -> Self {
        let length = self.length();
        if length > 1e-20 {
            Self::from_xyzw(
                self.x / length,
                self.y / length,
                self.z / length,
                self.w / length,
            )
        } else {
            Self::IDENTITY
        }
    }

    /// Shortest-arc spherical interpolation. Falls back to a normalised lerp
    /// when the two rotations are close enough that the division is unstable.
    pub fn slerp(a: Self, b: Self, t: f32) -> Self {
        let mut dot = a.dot(b);
        let mut b = b;
        if dot < 0.0 {
            // Take the short way round: q and -q are the same rotation.
            b = -b;
            dot = -dot;
        }

        if dot > 0.9995 {
            return Self::from_xyzw(
                a.x + (b.x - a.x) * t,
                a.y + (b.y - a.y) * t,
                a.z + (b.z - a.z) * t,
                a.w + (b.w - a.w) * t,
            )
            .normalize();
        }

        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin_theta = theta.sin();
        let weight_a = ((1.0 - t) * theta).sin() / sin_theta;
        let weight_b = (t * theta).sin() / sin_theta;

        Self::from_xyzw(
            a.x * weight_a + b.x * weight_b,
            a.y * weight_a + b.y * weight_b,
            a.z * weight_a + b.z * weight_b,
            a.w * weight_a + b.w * weight_b,
        )
    }

    /// Upper-left 3x3 of the rotation, as a column-major 4x4 with no translation.
    pub fn to_mat4(self) -> Mat4 {
        let q = self.normalize();
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);

        Mat4([
            1.0 - 2.0 * (yy + zz),
            2.0 * (xy + wz),
            2.0 * (xz - wy),
            0.0,
            2.0 * (xy - wz),
            1.0 - 2.0 * (xx + zz),
            2.0 * (yz + wx),
            0.0,
            2.0 * (xz + wy),
            2.0 * (yz - wx),
            1.0 - 2.0 * (xx + yy),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }
}

impl Neg for Quat {
    type Output = Self;
    fn neg(self) -> Self {
        Self::from_xyzw(-self.x, -self.y, -self.z, -self.w)
    }
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A column-major 4x4 matrix: `m[column * 4 + row]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub const IDENTITY: Self = Self([
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]);

    /// glTF's `[[f32; 4]; 4]` is `m[column][row]`, so flattening in order gives
    /// the column-major array this type stores.
    pub fn from_gltf(value: [[f32; 4]; 4]) -> Self {
        let mut out = [0.0; 16];
        for column in 0..4 {
            for row in 0..4 {
                out[column * 4 + row] = value[column][row];
            }
        }
        Self(out)
    }

    pub fn from_trs(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        let mut matrix = rotation.to_mat4();
        for row in 0..3 {
            matrix.0[row] *= scale.x;
            matrix.0[4 + row] *= scale.y;
            matrix.0[8 + row] *= scale.z;
        }
        matrix.0[12] = translation.x;
        matrix.0[13] = translation.y;
        matrix.0[14] = translation.z;
        matrix
    }

    /// `self * rhs`, i.e. `rhs` is applied first.
    pub fn mul_mat4(&self, rhs: &Mat4) -> Mat4 {
        let a = &self.0;
        let b = &rhs.0;
        let mut out = [0.0f32; 16];
        for column in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += a[k * 4 + row] * b[column * 4 + k];
                }
                out[column * 4 + row] = sum;
            }
        }
        Mat4(out)
    }

    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        let m = &self.0;
        let x = m[0] * point.x + m[4] * point.y + m[8] * point.z + m[12];
        let y = m[1] * point.x + m[5] * point.y + m[9] * point.z + m[13];
        let z = m[2] * point.x + m[6] * point.y + m[10] * point.z + m[14];
        let w = m[3] * point.x + m[7] * point.y + m[11] * point.z + m[15];
        if w != 0.0 && w != 1.0 {
            Vec3::new(x / w, y / w, z / w)
        } else {
            Vec3::new(x, y, z)
        }
    }

    /// Right-handed perspective projection into wgpu's 0..1 depth range.
    pub fn perspective_rh(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / (fov_y_radians * 0.5).tan();
        let range = near - far;
        Self([
            f / aspect,
            0.0,
            0.0,
            0.0, //
            0.0,
            f,
            0.0,
            0.0, //
            0.0,
            0.0,
            far / range,
            -1.0, //
            0.0,
            0.0,
            near * far / range,
            0.0,
        ])
    }

    pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        let forward = (target - eye).normalize();
        let side = forward.cross(up).normalize();
        let up = side.cross(forward);

        Self([
            side.x,
            up.x,
            -forward.x,
            0.0,
            side.y,
            up.y,
            -forward.y,
            0.0,
            side.z,
            up.z,
            -forward.z,
            0.0,
            -side.dot(eye),
            -up.dot(eye),
            forward.dot(eye),
            1.0,
        ])
    }

    /// General 4x4 inverse; returns identity for a singular matrix rather than
    /// poisoning every vertex with NaN.
    pub fn inverse(&self) -> Mat4 {
        let m = &self.0;
        let (a00, a01, a02, a03) = (m[0], m[1], m[2], m[3]);
        let (a10, a11, a12, a13) = (m[4], m[5], m[6], m[7]);
        let (a20, a21, a22, a23) = (m[8], m[9], m[10], m[11]);
        let (a30, a31, a32, a33) = (m[12], m[13], m[14], m[15]);

        let b00 = a00 * a11 - a01 * a10;
        let b01 = a00 * a12 - a02 * a10;
        let b02 = a00 * a13 - a03 * a10;
        let b03 = a01 * a12 - a02 * a11;
        let b04 = a01 * a13 - a03 * a11;
        let b05 = a02 * a13 - a03 * a12;
        let b06 = a20 * a31 - a21 * a30;
        let b07 = a20 * a32 - a22 * a30;
        let b08 = a20 * a33 - a23 * a30;
        let b09 = a21 * a32 - a22 * a31;
        let b10 = a21 * a33 - a23 * a31;
        let b11 = a22 * a33 - a23 * a32;

        let determinant = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
        if determinant.abs() < 1e-20 {
            return Mat4::IDENTITY;
        }
        let inv = 1.0 / determinant;

        Mat4([
            (a11 * b11 - a12 * b10 + a13 * b09) * inv,
            (a02 * b10 - a01 * b11 - a03 * b09) * inv,
            (a31 * b05 - a32 * b04 + a33 * b03) * inv,
            (a22 * b04 - a21 * b05 - a23 * b03) * inv,
            (a12 * b08 - a10 * b11 - a13 * b07) * inv,
            (a00 * b11 - a02 * b08 + a03 * b07) * inv,
            (a32 * b02 - a30 * b05 - a33 * b01) * inv,
            (a20 * b05 - a22 * b02 + a23 * b01) * inv,
            (a10 * b10 - a11 * b08 + a13 * b06) * inv,
            (a01 * b08 - a00 * b10 - a03 * b06) * inv,
            (a30 * b04 - a31 * b02 + a33 * b00) * inv,
            (a21 * b02 - a20 * b04 - a23 * b00) * inv,
            (a11 * b07 - a10 * b09 - a12 * b06) * inv,
            (a00 * b09 - a01 * b07 + a02 * b06) * inv,
            (a31 * b01 - a30 * b03 - a32 * b00) * inv,
            (a20 * b03 - a21 * b01 + a22 * b00) * inv,
        ])
    }
}

impl Mul for Mat4 {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        self.mul_mat4(&rhs)
    }
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Axis-aligned bounding box, used to frame the camera on whatever was loaded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    pub fn expand(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    /// The box containing both, ignoring empty boxes.
    pub fn union(self, other: Self) -> Self {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// Half the diagonal: a sphere radius that contains the whole box.
    pub fn radius(&self) -> f32 {
        self.size().length() * 0.5
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn identity_is_neutral() {
        let m = Mat4::from_trs(Vec3::new(1.0, 2.0, 3.0), Quat::IDENTITY, Vec3::ONE);
        assert_eq!(m * Mat4::IDENTITY, m);
        assert_eq!(Mat4::IDENTITY * m, m);
    }

    #[test]
    fn inverse_round_trips() {
        let m = Mat4::from_trs(
            Vec3::new(4.0, -2.0, 7.0),
            Quat::from_xyzw(0.2, 0.4, -0.1, 0.88).normalize(),
            Vec3::new(2.0, 0.5, 3.0),
        );
        let product = m * m.inverse();
        for (index, value) in product.0.iter().enumerate() {
            assert!(
                approx(*value, Mat4::IDENTITY.0[index]),
                "element {index} was {value}"
            );
        }
    }

    #[test]
    fn trs_places_points() {
        let m = Mat4::from_trs(Vec3::new(0.0, 5.0, 0.0), Quat::IDENTITY, Vec3::splat(2.0));
        let point = m.transform_point(Vec3::new(1.0, 1.0, 1.0));
        assert!(approx(point.x, 2.0) && approx(point.y, 7.0) && approx(point.z, 2.0));
    }

    #[test]
    fn rotation_maps_x_to_y() {
        // 90 degrees about +Z sends +X to +Y.
        let half = std::f32::consts::FRAC_PI_4;
        let q = Quat::from_xyzw(0.0, 0.0, half.sin(), half.cos());
        let rotated = q.to_mat4().transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!(approx(rotated.x, 0.0) && approx(rotated.y, 1.0));
    }

    #[test]
    fn perspective_maps_near_to_zero() {
        let projection = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0);
        let near = projection.transform_point(Vec3::new(0.0, 0.0, -0.1));
        let far = projection.transform_point(Vec3::new(0.0, 0.0, -100.0));
        assert!(approx(near.z, 0.0), "near was {}", near.z);
        assert!(approx(far.z, 1.0), "far was {}", far.z);
    }

    #[test]
    fn slerp_endpoints_are_exact() {
        let a = Quat::from_xyzw(0.0, 0.0, 0.0, 1.0);
        let b = Quat::from_xyzw(0.0, 0.0, 1.0, 0.0);
        assert_eq!(Quat::slerp(a, b, 0.0), a);
        assert!(Quat::slerp(a, b, 1.0).dot(b).abs() > 0.9999);
    }
}
