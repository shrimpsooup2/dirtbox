//! Order-sensitive 64-bit hashing of simulation state, used to check that two
//! runs with the same seed and inputs are bit-for-bit identical.

/// FNV-1a over little-endian encodings. Not cryptographic; it only needs to
/// catch divergence.
#[derive(Clone, Debug)]
pub struct StateHasher(u64);

impl Default for StateHasher {
    fn default() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }
}

impl StateHasher {
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Hashes the exact bit pattern, so −0.0 and 0.0 differ and so do NaN payloads.
    pub fn write_f64(&mut self, v: f64) {
        self.write_u64(v.to_bits());
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}
