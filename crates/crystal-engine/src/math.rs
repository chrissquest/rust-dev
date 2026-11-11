//! Linear algebra: [`glam`], plus the few things a renderer needs that it lacks.
//!
//! glam's matrices are column-major, which is both glTF's array order and WGSL's
//! `mat4x4<f32>` memory layout, so nothing is transposed on the way to the GPU:
//! `Mat4::to_cols_array` is exactly what a uniform buffer wants, and
//! `Mat4::from_cols_array_2d` reads glTF's `[[f32; 4]; 4]` as is.
//! `Mat4::perspective_rh` targets wgpu's 0..1 depth range.
//!
//! glam returns NaN for a few degenerate inputs that real assets do contain
//! (a zero-length normal, an all-zero rotation, a singular bind matrix). The
//! helpers below fall back to something drawable instead, so one bad value
//! cannot poison every vertex it touches.

pub use glam::{Mat4, Quat, Vec3};

/// Normalises a rotation, falling back to identity for a zero quaternion.
pub fn normalize_quat(rotation: Quat) -> Quat {
    let length = rotation.length();
    if length > 1e-20 && length.is_finite() {
        rotation / length
    } else {
        Quat::IDENTITY
    }
}

/// `translation * rotation * scale`, with the rotation normalised first so a
/// slightly denormalised keyframe does not also scale the model.
pub fn trs(translation: Vec3, rotation: Quat, scale: Vec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(scale, normalize_quat(rotation), translation)
}

/// General 4x4 inverse; returns identity for a singular matrix rather than NaN.
pub fn inverse_or_identity(matrix: Mat4) -> Mat4 {
    if matrix.determinant().abs() < 1e-20 {
        Mat4::IDENTITY
    } else {
        matrix.inverse()
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
    fn inverse_round_trips() {
        let m = trs(
            Vec3::new(4.0, -2.0, 7.0),
            Quat::from_xyzw(0.2, 0.4, -0.1, 0.88),
            Vec3::new(2.0, 0.5, 3.0),
        );
        let product = m * inverse_or_identity(m);
        assert!(product.abs_diff_eq(Mat4::IDENTITY, 1e-4), "{product}");
    }

    #[test]
    fn singular_inverse_is_identity() {
        assert_eq!(inverse_or_identity(Mat4::ZERO), Mat4::IDENTITY);
    }

    #[test]
    fn zero_rotation_is_identity() {
        assert_eq!(normalize_quat(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)), Quat::IDENTITY);
    }

    #[test]
    fn trs_places_points() {
        let m = trs(Vec3::new(0.0, 5.0, 0.0), Quat::IDENTITY, Vec3::splat(2.0));
        let point = m.transform_point3(Vec3::new(1.0, 1.0, 1.0));
        assert!(approx(point.x, 2.0) && approx(point.y, 7.0) && approx(point.z, 2.0));
    }

    #[test]
    fn trs_ignores_rotation_length() {
        // A rotation of length 2 must not scale the model by 4.
        let doubled = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
        let point = trs(Vec3::ZERO, doubled, Vec3::ONE).transform_point3(Vec3::X);
        assert!(approx(point.length(), 1.0), "{point}");
    }

    #[test]
    fn rotation_maps_x_to_y() {
        // 90 degrees about +Z sends +X to +Y.
        let half = std::f32::consts::FRAC_PI_4;
        let q = Quat::from_xyzw(0.0, 0.0, half.sin(), half.cos());
        let rotated = Mat4::from_quat(q).transform_point3(Vec3::X);
        assert!(approx(rotated.x, 0.0) && approx(rotated.y, 1.0));
    }

    #[test]
    fn perspective_maps_near_to_zero() {
        // The renderer relies on wgpu's 0..1 depth range.
        let projection = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0);
        let near = projection.project_point3(Vec3::new(0.0, 0.0, -0.1));
        let far = projection.project_point3(Vec3::new(0.0, 0.0, -100.0));
        assert!(approx(near.z, 0.0), "near was {}", near.z);
        assert!(approx(far.z, 1.0), "far was {}", far.z);
    }

    #[test]
    fn slerp_endpoints_are_exact() {
        let a = Quat::IDENTITY;
        let b = Quat::from_xyzw(0.0, 0.0, 1.0, 0.0);
        assert_eq!(a.slerp(b, 0.0), a);
        assert!(a.slerp(b, 1.0).dot(b).abs() > 0.9999);
    }

    #[test]
    fn gltf_matrices_are_column_major() {
        // glTF's `[[f32; 4]; 4]` is `m[column][row]`: the translation is the last column.
        let gltf = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [3.0, 4.0, 5.0, 1.0],
        ];
        let m = Mat4::from_cols_array_2d(&gltf);
        assert_eq!(m.transform_point3(Vec3::ZERO), Vec3::new(3.0, 4.0, 5.0));
    }
}
