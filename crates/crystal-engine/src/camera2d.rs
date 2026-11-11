//! Straight-down orthographic camera for 2D scenes.
//!
//! World space is x right, y up, z towards the viewer (height above the ground).
//! The camera looks along -z, so a sprite with a larger z is nearer and wins the
//! depth test. Its size is given as a view *height* in world units; the width
//! follows from the target's aspect ratio.

use crate::math::Mat4;
use glam::Vec2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2d {
    /// World position at the middle of the view.
    pub center: Vec2,
    /// World units from the bottom of the view to the top.
    pub view_height: f32,
    /// Heights (world z) outside `z_min..=z_max` are clipped.
    pub z_min: f32,
    pub z_max: f32,
}

impl Default for Camera2d {
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            view_height: 10.0,
            z_min: -100.0,
            z_max: 100.0,
        }
    }
}

impl Camera2d {
    pub fn new(center: Vec2, view_height: f32) -> Self {
        Self {
            center,
            view_height,
            ..Self::default()
        }
    }

    /// A camera whose world units are the target's pixels, origin bottom-left and y
    /// up: the space for HUD and menu drawing. y points up, as in the world, so
    /// sprites are the right way up; a position that came from the window (y down)
    /// is flipped with `height - y`.
    pub fn pixels(width: f32, height: f32) -> PixelCamera {
        PixelCamera { width, height }
    }

    /// Half the view's width and height in world units.
    pub fn half_extent(&self, aspect: f32) -> Vec2 {
        let half_height = self.view_height.max(1e-6) * 0.5;
        Vec2::new(half_height * aspect.max(1e-6), half_height)
    }

    /// World to clip space, targeting wgpu's 0..1 depth range.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let half = self.half_extent(aspect);
        // Eye sits at z_max looking down -z, so view-space depth runs 0..(z_max - z_min).
        let depth = (self.z_max - self.z_min).max(1e-6);
        let projection = Mat4::orthographic_rh(-half.x, half.x, -half.y, half.y, 0.0, depth);
        let view = Mat4::from_translation(glam::Vec3::new(-self.center.x, -self.center.y, -self.z_max));
        projection * view
    }

    /// The world position on the ground under a point of the target, where `point`
    /// is in pixels from the target's top-left corner.
    pub fn screen_to_world(&self, point: Vec2, target_size: Vec2) -> Vec2 {
        let size = target_size.max(Vec2::splat(1.0));
        let half = self.half_extent(size.x / size.y);
        let ndc = Vec2::new(point.x / size.x * 2.0 - 1.0, 1.0 - point.y / size.y * 2.0);
        self.center + ndc * half
    }

    /// Keeps the view inside the rectangle `min..max` where it fits, and centres it
    /// on the rectangle along any axis where the view is larger.
    pub fn clamp_to(&mut self, min: Vec2, max: Vec2, aspect: f32) {
        let half = self.half_extent(aspect);
        self.center.x = clamp_axis(self.center.x, min.x, max.x, half.x);
        self.center.y = clamp_axis(self.center.y, min.y, max.y, half.y);
    }
}

fn clamp_axis(value: f32, min: f32, max: f32, half: f32) -> f32 {
    if max - min <= half * 2.0 {
        (min + max) * 0.5
    } else {
        value.clamp(min + half, max - half)
    }
}

/// See [`Camera2d::pixels`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelCamera {
    pub width: f32,
    pub height: f32,
}

impl PixelCamera {
    pub fn view_projection(&self) -> Mat4 {
        Mat4::orthographic_rh(0.0, self.width.max(1.0), 0.0, self.height.max(1.0), -1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec3, Vec4};

    fn project(camera: &Camera2d, aspect: f32, point: Vec3) -> Vec4 {
        camera.view_projection(aspect) * point.extend(1.0)
    }

    #[test]
    fn centre_maps_to_the_middle_and_edges_to_the_edges() {
        let camera = Camera2d::new(Vec2::new(8.0, 4.5), 9.0);
        let aspect = 16.0 / 9.0;
        let middle = project(&camera, aspect, Vec3::new(8.0, 4.5, 0.0));
        assert!(middle.x.abs() < 1e-6 && middle.y.abs() < 1e-6);
        let corner = project(&camera, aspect, Vec3::new(16.0, 9.0, 0.0));
        assert!((corner.x - 1.0).abs() < 1e-5 && (corner.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn higher_sprites_are_nearer() {
        let camera = Camera2d::default();
        let ground = project(&camera, 1.0, Vec3::ZERO).z;
        let raised = project(&camera, 1.0, Vec3::new(0.0, 0.0, 1.0)).z;
        assert!(raised < ground, "raised {raised} should be nearer than ground {ground}");
        assert!((0.0..=1.0).contains(&ground) && (0.0..=1.0).contains(&raised));
    }

    #[test]
    fn screen_to_world_inverts_the_projection() {
        let camera = Camera2d::new(Vec2::new(3.0, -2.0), 4.0);
        let size = Vec2::new(800.0, 400.0);
        assert_eq!(camera.screen_to_world(Vec2::new(400.0, 200.0), size), Vec2::new(3.0, -2.0));
        // Top-left pixel: 4 units left (aspect 2), 2 units up.
        assert_eq!(camera.screen_to_world(Vec2::ZERO, size), Vec2::new(-1.0, 0.0));
    }

    #[test]
    fn clamping_keeps_the_view_inside_or_centres_it() {
        let mut camera = Camera2d::new(Vec2::new(0.0, 100.0), 4.0);
        camera.clamp_to(Vec2::ZERO, Vec2::new(16.0, 3.0), 2.0);
        // Half width is 4: x clamps to 4. The room is shorter than the view: y centres.
        assert_eq!(camera.center, Vec2::new(4.0, 1.5));
    }

    #[test]
    fn pixel_camera_puts_the_origin_bottom_left() {
        let matrix = Camera2d::pixels(200.0, 100.0).view_projection();
        let bottom_left = matrix * Vec4::new(0.0, 0.0, 0.0, 1.0);
        assert!((bottom_left.x + 1.0).abs() < 1e-6 && (bottom_left.y + 1.0).abs() < 1e-6);
        let top_right = matrix * Vec4::new(200.0, 100.0, 0.0, 1.0);
        assert!((top_right.x - 1.0).abs() < 1e-6 && (top_right.y - 1.0).abs() < 1e-6);
    }
}
