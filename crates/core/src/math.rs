//! Minimal 3D vector math for positions on the unit sphere.

use std::ops::{Add, AddAssign, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn normalized(self) -> Vec3 {
        self * (1.0 / self.length())
    }

    /// Latitude in radians, for a point on the unit sphere (+z is north).
    pub fn latitude(self) -> f64 {
        self.z.clamp(-1.0, 1.0).asin()
    }

    /// Longitude in radians in (−π, π].
    pub fn longitude(self) -> f64 {
        self.y.atan2(self.x)
    }

    /// Unit vector for a latitude/longitude in radians.
    pub fn from_lat_lon(lat: f64, lon: f64) -> Vec3 {
        let (sl, cl) = lat.sin_cos();
        let (so, co) = lon.sin_cos();
        Vec3::new(cl * co, cl * so, sl)
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// Area (steradians) of the spherical triangle with unit-vector corners a, b, c.
pub fn spherical_triangle_area(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    // Van Oosterom & Strackee: tan(E/2) = |a·(b×c)| / (1 + a·b + b·c + c·a)
    let numer = a.dot(b.cross(c)).abs();
    let denom = 1.0 + a.dot(b) + b.dot(c) + c.dot(a);
    2.0 * numer.atan2(denom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn octant_triangle_is_one_eighth_of_sphere() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        let c = Vec3::new(0.0, 0.0, 1.0);
        let area = spherical_triangle_area(a, b, c);
        assert!((area - 4.0 * PI / 8.0).abs() < 1e-12);
    }

    #[test]
    fn lat_lon_round_trip() {
        for &(lat, lon) in &[(0.3, -2.0), (-1.2, 0.5), (FRAC_PI_2 - 1e-6, 3.0)] {
            let v = Vec3::from_lat_lon(lat, lon);
            assert!((v.length() - 1.0).abs() < 1e-12);
            assert!((v.latitude() - lat).abs() < 1e-9);
            assert!((v.longitude() - lon).abs() < 1e-6);
        }
    }
}
