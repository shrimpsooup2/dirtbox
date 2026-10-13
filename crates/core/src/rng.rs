//! Deterministic random numbers.
//!
//! Every random decision in the simulation draws from a stream derived from
//! `(world seed, domain, entity, tick)`. Streams are independent of thread
//! scheduling and iteration order, so parallel stages stay reproducible.

/// One SplitMix64 step. Used for seeding and for mixing stream keys.
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hashes a seed and a list of keys into one well-mixed 64-bit value.
pub fn derive_key(seed: u64, keys: &[u64]) -> u64 {
    let mut h = seed;
    for &k in keys {
        let mut s = h ^ k.wrapping_mul(0xD6E8_FEB8_6659_FD93);
        h = splitmix64(&mut s);
    }
    h
}

/// xoshiro256** generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seeds the full state from one 64-bit value via SplitMix64.
    pub fn from_seed(seed: u64) -> Self {
        let mut sm = seed;
        let s = [
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
            splitmix64(&mut sm),
        ];
        Self::from_state(s)
    }

    /// Builds a generator from raw state. An all-zero state would be stuck at
    /// zero forever, so it is replaced by a fixed non-zero state.
    pub fn from_state(s: [u64; 4]) -> Self {
        if s == [0; 4] {
            return Self::from_seed(0x5EED);
        }
        Self { s }
    }

    /// Independent stream for one (domain, entity, tick) within a world seed.
    pub fn stream(seed: u64, domain: u64, entity: u64, tick: u64) -> Self {
        Self::from_seed(derive_key(seed, &[domain, entity, tick]))
    }

    pub fn state(&self) -> [u64; 4] {
        self.s
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in [lo, hi).
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }

    /// Uniform integer in [0, n). Unbiased (Lemire's method). `n` must be > 0.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng::below(0)");
        let mut m = (self.next_u64() as u128) * (n as u128);
        if (m as u64) < n {
            let threshold = n.wrapping_neg() % n;
            while (m as u64) < threshold {
                m = (self.next_u64() as u128) * (n as u128);
            }
        }
        (m >> 64) as u64
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    /// Standard normal sample (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = 1.0 - self.f64(); // (0, 1], keeps ln finite
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_xoshiro256starstar() {
        // Reference outputs for state {1, 2, 3, 4}.
        let mut r = Rng::from_state([1, 2, 3, 4]);
        assert_eq!(r.next_u64(), 11520);
        assert_eq!(r.next_u64(), 0);
    }

    #[test]
    fn streams_are_reproducible_and_distinct() {
        let a1: Vec<u64> = {
            let mut r = Rng::stream(42, 1, 7, 100);
            (0..4).map(|_| r.next_u64()).collect()
        };
        let a2: Vec<u64> = {
            let mut r = Rng::stream(42, 1, 7, 100);
            (0..4).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = Rng::stream(42, 1, 7, 101);
            (0..4).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
    }

    #[test]
    fn uniform_and_normal_moments() {
        let mut r = Rng::from_seed(9);
        let n = 200_000;
        let (mut su, mut sn, mut sn2) = (0.0, 0.0, 0.0);
        for _ in 0..n {
            let u = r.f64();
            assert!((0.0..1.0).contains(&u));
            su += u;
            let z = r.normal();
            sn += z;
            sn2 += z * z;
        }
        let n = n as f64;
        assert!((su / n - 0.5).abs() < 0.005);
        assert!((sn / n).abs() < 0.01);
        assert!((sn2 / n - 1.0).abs() < 0.02);
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut r = Rng::from_seed(3);
        let mut seen = [false; 7];
        for _ in 0..1000 {
            let v = r.below(7) as usize;
            seen[v] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }
}
