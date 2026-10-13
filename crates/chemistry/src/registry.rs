//! Every strain alive anywhere on the planet, stored once and referred to by
//! a slot index from the per-cell strain fields.

use crate::genome::Genome;

/// Index into the registry. Indices are reused after a strain dies out; use
/// [`StrainRecord::serial`] to refer to a strain over time.
pub type StrainId = u32;

/// Marks an empty slot in a strain field.
pub const EMPTY: StrainId = StrainId::MAX;

#[derive(Clone, Debug)]
pub struct StrainRecord {
    pub genome: Genome,
    /// Unique, never reused: strain #1 is the first base bacterium.
    pub serial: u64,
    pub parent: Option<u64>,
    /// Planet time when the strain arose, s.
    pub born: f64,
    /// Total living biomass across the planet, mol C.
    pub biomass: f64,
    /// Number of cells it lives in.
    pub cells: u32,
    /// Largest total biomass it has reached, mol C.
    pub peak_biomass: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    records: Vec<Option<StrainRecord>>,
    free: Vec<StrainId>,
    next_serial: u64,
}

impl Registry {
    pub fn insert(&mut self, genome: Genome, parent: Option<u64>, born: f64) -> StrainId {
        self.next_serial += 1;
        let record = StrainRecord {
            genome,
            serial: self.next_serial,
            parent,
            born,
            biomass: 0.0,
            cells: 0,
            peak_biomass: 0.0,
        };
        match self.free.pop() {
            Some(id) => {
                self.records[id as usize] = Some(record);
                id
            }
            None => {
                self.records.push(Some(record));
                (self.records.len() - 1) as StrainId
            }
        }
    }

    pub fn get(&self, id: StrainId) -> Option<&StrainRecord> {
        self.records.get(id as usize).and_then(|r| r.as_ref())
    }

    /// The genome of a live strain. Panics on a dead or unknown id, which
    /// would mean a strain field points at a freed slot.
    pub fn genome(&self, id: StrainId) -> &Genome {
        &self.records[id as usize]
            .as_ref()
            .expect("strain field refers to a dead strain")
            .genome
    }

    /// Live strains, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (StrainId, &StrainRecord)> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i as StrainId, r)))
    }

    pub fn len(&self) -> usize {
        self.records.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many strains have ever existed.
    pub fn total_ever(&self) -> u64 {
        self.next_serial
    }

    /// Updates each strain's totals from per-strain sums, and frees strains
    /// that no longer live anywhere. `totals` is indexed by id.
    pub(crate) fn census(&mut self, totals: &[(f64, u32)]) {
        for (id, slot) in self.records.iter_mut().enumerate() {
            let Some(r) = slot else { continue };
            let (biomass, cells) = totals.get(id).copied().unwrap_or((0.0, 0));
            if cells == 0 {
                *slot = None;
                self.free.push(id as StrainId);
            } else {
                r.biomass = biomass;
                r.cells = cells;
                r.peak_biomass = r.peak_biomass.max(biomass);
            }
        }
        // Reuse low ids first, deterministically.
        self.free.sort_unstable_by(|a, b| b.cmp(a));
    }

    pub(crate) fn capacity(&self) -> usize {
        self.records.len()
    }
}
