//! Seeded 3D gradient noise (improved Perlin) and fractal sums of it. Sampling
//! on the unit sphere's 3D coordinates avoids any seams or polar pinching.

use dirtbox_core::Rng;

pub struct Noise3 {
    perm: [u8; 512],
}

const GRADIENTS: [[f64; 3]; 12] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
];

impl Noise3 {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::from_seed(seed);
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        for i in (1..256).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            p.swap(i, j);
        }
        Self {
            perm: std::array::from_fn(|i| p[i & 255]),
        }
    }

    /// Noise in roughly [−1, 1].
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
        let (xf, yf, zf) = (x - xi, y - yi, z - zi);
        let (xi, yi, zi) = (
            (xi as i64 & 255) as usize,
            (yi as i64 & 255) as usize,
            (zi as i64 & 255) as usize,
        );
        let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let (u, v, w) = (fade(xf), fade(yf), fade(zf));
        let p = &self.perm;
        let hash =
            |a: usize, b: usize, c: usize| p[p[p[a] as usize + b] as usize + c] as usize % 12;
        let grad = |h: usize, x: f64, y: f64, z: f64| {
            let g = GRADIENTS[h];
            g[0] * x + g[1] * y + g[2] * z
        };
        let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
        let c000 = grad(hash(xi, yi, zi), xf, yf, zf);
        let c100 = grad(hash(xi + 1, yi, zi), xf - 1.0, yf, zf);
        let c010 = grad(hash(xi, yi + 1, zi), xf, yf - 1.0, zf);
        let c110 = grad(hash(xi + 1, yi + 1, zi), xf - 1.0, yf - 1.0, zf);
        let c001 = grad(hash(xi, yi, zi + 1), xf, yf, zf - 1.0);
        let c101 = grad(hash(xi + 1, yi, zi + 1), xf - 1.0, yf, zf - 1.0);
        let c011 = grad(hash(xi, yi + 1, zi + 1), xf, yf - 1.0, zf - 1.0);
        let c111 = grad(hash(xi + 1, yi + 1, zi + 1), xf - 1.0, yf - 1.0, zf - 1.0);
        lerp(
            w,
            lerp(v, lerp(u, c000, c100), lerp(u, c010, c110)),
            lerp(v, lerp(u, c001, c101), lerp(u, c011, c111)),
        )
    }

    /// Fractal Brownian motion: `octaves` layers, each double the frequency and
    /// `gain` times the amplitude. Normalised to roughly [−1, 1].
    pub fn fbm(&self, x: f64, y: f64, z: f64, octaves: u32, gain: f64) -> f64 {
        let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for o in 0..octaves {
            // Offset each octave so their lattices don't line up.
            let off = o as f64 * 17.13;
            sum += amp * self.sample(x * freq + off, y * freq - off, z * freq + 2.0 * off);
            norm += amp;
            amp *= gain;
            freq *= 2.0;
        }
        sum / norm
    }

    /// Ridged noise in [0, 1]: sharp crests where the base noise crosses zero,
    /// which reads as mountain chains and mid-ocean ridges.
    pub fn ridged(&self, x: f64, y: f64, z: f64, octaves: u32) -> f64 {
        let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for o in 0..octaves {
            let off = o as f64 * 31.7;
            let n = 1.0 - self.sample(x * freq - off, y * freq + off, z * freq).abs() * 1.6;
            sum += amp * n.max(0.0).powi(2);
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_seed_dependent() {
        let a = Noise3::new(1);
        let b = Noise3::new(1);
        let c = Noise3::new(2);
        let p = (0.3, 1.7, -2.2);
        assert_eq!(a.sample(p.0, p.1, p.2), b.sample(p.0, p.1, p.2));
        assert_ne!(a.fbm(p.0, p.1, p.2, 5, 0.5), c.fbm(p.0, p.1, p.2, 5, 0.5));
    }

    #[test]
    fn bounded_and_continuous() {
        let n = Noise3::new(7);
        let mut prev = n.sample(0.0, 0.5, 0.25);
        for i in 1..10_000 {
            let x = i as f64 * 0.001;
            let v = n.sample(x, 0.5, 0.25);
            assert!(v.abs() <= 1.01);
            assert!((v - prev).abs() < 0.01, "jump at {x}");
            prev = v;
        }
        // Integer lattice points are zero by construction.
        assert_eq!(n.sample(3.0, -4.0, 5.0), 0.0);
    }
}
