//! Orbit camera: drag to turn, wheel to zoom, right-drag to pan.
//!
//! Framing is derived from the loaded model's bounds, so a 2-unit grenade and a
//! 200-unit dinosaur both fill the window without a unit slider.

use crate::math::{Aabb, Mat4, Vec3};

#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    /// Radius of the framed model, kept so the clip planes can follow the zoom.
    pub radius: f32,
    /// Near/far follow `distance`, so depth precision stays usable whatever the
    /// model's scale or zoom level is.
    pub near: f32,
    pub far: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 5.0,
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: 0.35,
            fov_y: 45f32.to_radians(),
            radius: 1.0,
            near: 0.01,
            far: 1000.0,
        }
    }
}

impl OrbitCamera {
    /// Frames `bounds`, keeping any rotation the camera already had.
    pub fn frame(&mut self, bounds: &Aabb) {
        if bounds.is_empty() {
            self.target = Vec3::ZERO;
            self.distance = 5.0;
            self.radius = 1.0;
            self.refresh_clip();
            return;
        }

        let radius = bounds.radius().max(1e-3);
        self.target = bounds.center();
        // Fit the bounding sphere into the vertical field of view.
        self.distance = radius / (self.fov_y * 0.5).sin() * 1.15;
        self.radius = radius;
        self.refresh_clip();
    }

    /// Recomputes the clip planes from the current eye distance.
    ///
    /// These used to be set once, when the camera was framed, so zooming out past
    /// the original framing put the model beyond `far` and it vanished. Keeping
    /// them a fixed fraction of the distance means any zoom level stays visible,
    /// and the ratio stays small enough for good depth precision.
    fn refresh_clip(&mut self) {
        let radius = self.radius.max(1e-4);
        self.near = ((self.distance - radius * 2.0) * 0.5).max(radius * 1e-4).max(1e-5);
        self.far = (self.distance + radius * 8.0).max(radius * 2.0).max(1.0);
    }

    /// Places the camera explicitly: angles in radians, `target` in world space.
    /// Keeps the framed radius, so the clip planes still suit the scene.
    pub fn set_view(&mut self, yaw: f32, pitch: f32, distance: f32, target: Vec3) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-1.5, 1.5);
        self.distance = distance.clamp(1e-4, 1e7);
        self.target = target;
        self.refresh_clip();
    }

    /// A world-space ray through a point on screen, given in normalised device
    /// coordinates (-1..1, +Y up). Returns `(origin, unit direction)`.
    pub fn ray(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> (Vec3, Vec3) {
        let inverse = crate::math::inverse_or_identity(self.view_projection(aspect));
        // wgpu clip space: depth 0 is the near plane, 1 the far plane.
        let near = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        let direction = (far - near).normalize_or_zero();
        (near, direction)
    }

    /// Makes `point` the orbit centre without moving the eye: the view turns to
    /// look at it, and later orbiting and zooming happen around it.
    pub fn focus_on(&mut self, point: Vec3) {
        let offset = self.eye() - point;
        let distance = offset.length();
        if !distance.is_finite() || distance < 1e-5 {
            self.target = point;
            return;
        }
        self.target = point;
        self.distance = distance;
        self.pitch = (offset.y / distance).clamp(-1.0, 1.0).asin().clamp(-1.5, 1.5);
        self.yaw = offset.x.atan2(offset.z);
        self.refresh_clip();
    }

    pub fn reset(&mut self) {
        self.yaw = std::f32::consts::FRAC_PI_4;
        self.pitch = 0.35;
    }

    /// Re-points the camera at a new model while keeping the angle and zoom.
    ///
    /// Switching creatures used to reframe every time, which threw away the view
    /// the user had just set up. The target moves to the new model's centre so it
    /// is still the thing being orbited.
    pub fn retarget(&mut self, bounds: &Aabb) {
        if bounds.is_empty() {
            self.target = Vec3::ZERO;
            self.radius = 1.0;
            self.refresh_clip();
            return;
        }

        self.target = bounds.center();
        self.radius = bounds.radius().max(1e-3);
        self.refresh_clip();
    }

    pub fn eye(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        self.target
            + Vec3::new(
                self.distance * cos_pitch * sin_yaw,
                self.distance * sin_pitch,
                self.distance * cos_pitch * cos_yaw,
            )
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::new(0.0, 1.0, 0.0))
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-3);
        Mat4::perspective_rh(self.fov_y, aspect, self.near, self.far)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw -= delta_x * 0.01;
        self.pitch = (self.pitch + delta_y * 0.01).clamp(-1.5, 1.5);
    }

    pub fn zoom(&mut self, scroll: f32) {
        // Multiplicative so the step feels the same at every distance.
        self.distance = (self.distance * (1.0 - scroll * 0.1)).clamp(1e-4, 1e7);
        self.refresh_clip();
    }

    /// Pan in the camera's screen plane, scaled by distance so it tracks the
    /// cursor at any zoom level.
    pub fn pan(&mut self, delta_x: f32, delta_y: f32) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let scale = self.distance * 0.0025;
        self.target += right * (-delta_x * scale) + up * (delta_y * scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_centre_ray_points_at_the_target() {
        let camera = OrbitCamera {
            target: Vec3::new(1.0, 2.0, 3.0),
            distance: 10.0,
            ..Default::default()
        };
        let (origin, direction) = camera.ray(0.0, 0.0, 1.5);
        let to_target = (camera.target - origin).normalize();
        assert!(direction.dot(to_target) > 0.9999, "{direction} vs {to_target}");
    }

    #[test]
    fn focusing_keeps_the_eye_and_looks_at_the_point() {
        let mut camera = OrbitCamera {
            distance: 10.0,
            ..Default::default()
        };
        let eye = camera.eye();
        let point = Vec3::new(2.0, -1.0, 0.5);
        camera.focus_on(point);

        assert!((camera.eye() - eye).length() < 1e-4, "the eye moved");
        assert_eq!(camera.target, point);
        assert!((camera.distance - (eye - point).length()).abs() < 1e-4);
        let (_, direction) = camera.ray(0.0, 0.0, 1.0);
        assert!(direction.dot((point - eye).normalize()) > 0.9999);
    }

    #[test]
    fn framing_a_box_keeps_it_in_front_of_the_eye() {
        let mut camera = OrbitCamera::default();
        let mut bounds = Aabb::empty();
        bounds.expand(Vec3::new(-1.0, -1.0, -1.0));
        bounds.expand(Vec3::new(1.0, 1.0, 1.0));

        camera.frame(&bounds);
        assert!((camera.target.length()).abs() < 1e-5);
        assert!(camera.distance > bounds.radius());
        assert!(camera.near > 0.0 && camera.far > camera.distance);
    }

    #[test]
    fn empty_bounds_fall_back_to_a_sane_distance() {
        let mut camera = OrbitCamera::default();
        camera.frame(&Aabb::empty());
        assert!(camera.distance.is_finite() && camera.distance > 0.0);
        assert!(camera.near > 0.0 && camera.far > camera.near);
    }

    #[test]
    fn pitch_is_clamped() {
        let mut camera = OrbitCamera::default();
        camera.orbit(0.0, 1000.0);
        assert!(camera.pitch <= 1.5);
        camera.orbit(0.0, -1000.0);
        assert!(camera.pitch >= -1.5);
    }

    #[test]
    fn zooming_out_keeps_the_model_between_the_clip_planes() {
        let mut camera = OrbitCamera::default();
        let mut bounds = Aabb::empty();
        bounds.expand(Vec3::new(-1.0, -1.0, -1.0));
        bounds.expand(Vec3::new(1.0, 1.0, 1.0));
        camera.frame(&bounds);

        // Wheel out hard: the old code left `far` at its framed value, so the
        // model disappeared well before this.
        for _ in 0..40 {
            camera.zoom(-1.0);
        }

        let radius = bounds.radius();
        assert!(
            camera.near < camera.distance - radius,
            "near {} would clip the model at distance {}",
            camera.near,
            camera.distance
        );
        assert!(
            camera.far > camera.distance + radius,
            "far {} would clip the model at distance {}",
            camera.far,
            camera.distance
        );
    }

    #[test]
    fn zooming_in_keeps_the_clip_planes_ordered() {
        let mut camera = OrbitCamera::default();
        let mut bounds = Aabb::empty();
        bounds.expand(Vec3::new(-1.0, -1.0, -1.0));
        bounds.expand(Vec3::new(1.0, 1.0, 1.0));
        camera.frame(&bounds);

        for _ in 0..60 {
            camera.zoom(1.0);
        }

        assert!(camera.near > 0.0 && camera.near < camera.far);
        assert!(camera.far > camera.distance);
    }
}
