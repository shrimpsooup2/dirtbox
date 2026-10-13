//! Orbit camera around a unit-radius globe, plus the little matrix math it
//! needs. North (+z) stays up on screen.

/// Column-major 4×4 matrix, as OpenGL expects.
#[derive(Clone, Copy, Debug)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fov_y / 2.0).tan();
        let mut m = [0.0; 16];
        m[0] = f / aspect;
        m[5] = f;
        m[10] = (far + near) / (near - far);
        m[11] = -1.0;
        m[14] = 2.0 * far * near / (near - far);
        Mat4(m)
    }

    /// View matrix from an eye position and an orthonormal basis.
    fn view(eye: V3, right: V3, up: V3, forward: V3) -> Mat4 {
        Mat4([
            right.0,
            up.0,
            -forward.0,
            0.0,
            right.1,
            up.1,
            -forward.1,
            0.0,
            right.2,
            up.2,
            -forward.2,
            0.0,
            -dot(right, eye),
            -dot(up, eye),
            dot(forward, eye),
            1.0,
        ])
    }

    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [0.0; 16];
        for col in 0..4 {
            for row in 0..4 {
                r[col * 4 + row] = (0..4).map(|k| self.0[k * 4 + row] * o.0[col * 4 + k]).sum();
            }
        }
        Mat4(r)
    }
}

pub type V3 = (f32, f32, f32);

pub fn dot(a: V3, b: V3) -> f32 {
    a.0 * b.0 + a.1 * b.1 + a.2 * b.2
}

fn cross(a: V3, b: V3) -> V3 {
    (
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}

fn scale(a: V3, s: f32) -> V3 {
    (a.0 * s, a.1 * s, a.2 * s)
}

fn add(a: V3, b: V3) -> V3 {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

fn normalize(a: V3) -> V3 {
    scale(a, 1.0 / dot(a, a).sqrt())
}

/// Altitude limits, in planet radii above the surface.
const MIN_ALTITUDE: f32 = 0.002;
const MAX_ALTITUDE: f32 = 8.0;
const MAX_LATITUDE: f32 = 89.0 * std::f32::consts::PI / 180.0;

pub struct OrbitCamera {
    /// Latitude and longitude (radians) of the point directly below the camera.
    pub lat: f32,
    pub lon: f32,
    /// Altitude the camera is easing toward, and where it is now.
    target_altitude: f32,
    altitude: f32,
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            lat: 0.35,
            lon: 0.0,
            target_altitude: 2.2,
            altitude: 2.2,
            fov_y: 40f32.to_radians(),
        }
    }
}

impl OrbitCamera {
    pub fn altitude(&self) -> f32 {
        self.altitude
    }

    pub fn eye(&self) -> V3 {
        let (sl, cl) = self.lat.sin_cos();
        let (so, co) = self.lon.sin_cos();
        scale((cl * co, cl * so, sl), 1.0 + self.altitude)
    }

    /// Right, up and forward unit vectors. Forward points at the globe centre.
    fn basis(&self) -> (V3, V3, V3) {
        let forward = normalize(scale(self.eye(), -1.0));
        let right = normalize(cross(forward, (0.0, 0.0, 1.0)));
        let up = cross(right, forward);
        (right, up, forward)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let (right, up, forward) = self.basis();
        // Keep the depth range tight around what is visible, so precision holds
        // up from orbit down to the surface.
        let near = (self.altitude * 0.3).max(1e-4);
        let far = 1.0 + self.altitude + 1.5;
        Mat4::perspective(self.fov_y, aspect, near, far).mul(&Mat4::view(
            self.eye(),
            right,
            up,
            forward,
        ))
    }

    /// Drag by a pixel offset. Close to the ground the globe turns slowly so the
    /// surface tracks the cursor; from far away it turns faster.
    pub fn drag(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let radians_per_pixel =
            2.0 * (self.fov_y / 2.0).tan() * self.altitude.min(3.0) / viewport_height.max(1.0);
        self.lon -= dx * radians_per_pixel / self.lat.cos().max(0.25);
        self.lat = (self.lat + dy * radians_per_pixel).clamp(-MAX_LATITUDE, MAX_LATITUDE);
        self.lon = (self.lon + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
    }

    /// Zoom by a scroll amount (points). Zoom is exponential in altitude so it
    /// feels the same from orbit and near the ground.
    pub fn zoom(&mut self, scroll: f32) {
        self.target_altitude =
            (self.target_altitude * (-scroll * 0.002).exp()).clamp(MIN_ALTITUDE, MAX_ALTITUDE);
    }

    /// Eases the altitude toward its target. Returns true while still moving.
    pub fn update(&mut self, dt: f32) -> bool {
        let t = 1.0 - (-dt * 12.0).exp();
        let (a, b) = (self.altitude.ln(), self.target_altitude.ln());
        self.altitude = (a + (b - a) * t).exp();
        (b - a).abs() > 1e-4
    }

    /// Point on the unit sphere under a viewport position given in normalised
    /// device coordinates (−1..1, y up), if the ray hits the globe.
    pub fn pick(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> Option<V3> {
        let (right, up, forward) = self.basis();
        let t = (self.fov_y / 2.0).tan();
        let dir = normalize(add(
            forward,
            add(scale(right, ndc_x * t * aspect), scale(up, ndc_y * t)),
        ));
        let eye = self.eye();
        // |eye + s·dir|² = 1, nearest positive root.
        let b = dot(eye, dir);
        let c = dot(eye, eye) - 1.0;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let s = -b - disc.sqrt();
        (s > 0.0).then(|| add(eye, scale(dir, s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(m: &Mat4, p: V3) -> (f32, f32, f32) {
        let v = [p.0, p.1, p.2, 1.0];
        let r: Vec<f32> = (0..4)
            .map(|row| (0..4).map(|k| m.0[k * 4 + row] * v[k]).sum())
            .collect();
        (r[0] / r[3], r[1] / r[3], r[2] / r[3])
    }

    #[test]
    fn point_below_camera_projects_to_centre_and_picks_back() {
        let cam = OrbitCamera::default();
        let below = normalize(cam.eye());
        let (x, y, z) = project(&cam.view_proj(1.5), below);
        assert!(x.abs() < 1e-4 && y.abs() < 1e-4 && (-1.0..1.0).contains(&z));
        let p = cam.pick(0.0, 0.0, 1.5).unwrap();
        assert!((dot(p, below) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn pick_round_trips_through_projection() {
        let cam = OrbitCamera::default();
        let m = cam.view_proj(1.6);
        let p = cam.pick(0.3, -0.2, 1.6).unwrap();
        let (x, y, _) = project(&m, p);
        assert!((x - 0.3).abs() < 1e-3 && (y + 0.2).abs() < 1e-3);
        assert!(
            cam.pick(0.99, 0.99, 1.6).is_none(),
            "corner ray should miss the globe"
        );
    }

    #[test]
    fn north_is_up() {
        let cam = OrbitCamera::default();
        let m = cam.view_proj(1.0);
        let below = normalize(cam.eye());
        let north_of_it = normalize(add(below, (0.0, 0.0, 0.05)));
        assert!(project(&m, north_of_it).1 > project(&m, below).1);
    }
}
