//! A registry of genotypes (microbial strains, plant lineages…): each is
//! stored once with its ancestry and referred to by a slot index from the
//! per-cell fields that hold its populations.

/// Index into a registry. Indices are reused after a lineage dies out; use
/// [`Lineage::serial`] to refer to one over time.
pub type LineageId = u32;

/// Marks an empty slot in a per-cell field.
pub const EMPTY: LineageId = LineageId::MAX;

#[derive(Clone, Debug)]
pub struct Lineage<G> {
    pub genome: G,
    /// Unique, never reused: #1 is the first ever registered.
    pub serial: u64,
    pub parent: Option<u64>,
    /// Planet time when it arose, s.
    pub born: f64,
    /// Total living biomass across the planet, mol C.
    pub biomass: f64,
    /// Number of cells it lives in.
    pub cells: u32,
    /// Largest total biomass it has reached, mol C.
    pub peak_biomass: f64,
}

#[derive(Clone, Debug)]
pub struct Lineages<G> {
    records: Vec<Option<Lineage<G>>>,
    free: Vec<LineageId>,
    next_serial: u64,
}

impl<G> Default for Lineages<G> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            free: Vec::new(),
            next_serial: 0,
        }
    }
}

impl<G> Lineages<G> {
    pub fn insert(&mut self, genome: G, parent: Option<u64>, born: f64) -> LineageId {
        self.next_serial += 1;
        let record = Lineage {
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
                (self.records.len() - 1) as LineageId
            }
        }
    }

    pub fn get(&self, id: LineageId) -> Option<&Lineage<G>> {
        self.records.get(id as usize).and_then(|r| r.as_ref())
    }

    /// The genome of a live lineage. Panics on a dead or unknown id, which
    /// would mean a field points at a freed slot.
    pub fn genome(&self, id: LineageId) -> &G {
        &self.records[id as usize]
            .as_ref()
            .expect("field refers to a dead lineage")
            .genome
    }

    /// Live lineages, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (LineageId, &Lineage<G>)> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i as LineageId, r)))
    }

    pub fn len(&self) -> usize {
        self.records.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many lineages have ever existed.
    pub fn total_ever(&self) -> u64 {
        self.next_serial
    }

    /// One more than the highest id in use (for per-id lookup tables).
    pub fn capacity(&self) -> usize {
        self.records.len()
    }

    /// Updates each lineage's totals from per-id sums of (biomass, cells), and
    /// frees lineages that no longer live anywhere.
    pub fn census(&mut self, totals: &[(f64, u32)]) {
        for (id, slot) in self.records.iter_mut().enumerate() {
            let Some(r) = slot else { continue };
            let (biomass, cells) = totals.get(id).copied().unwrap_or((0.0, 0));
            if cells == 0 {
                *slot = None;
                self.free.push(id as LineageId);
            } else {
                r.biomass = biomass;
                r.cells = cells;
                r.peak_biomass = r.peak_biomass.max(biomass);
            }
        }
        // Reuse low ids first, deterministically.
        self.free.sort_unstable_by(|a, b| b.cmp(a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_reused_but_serials_are_not() {
        let mut r: Lineages<&str> = Lineages::default();
        let a = r.insert("a", None, 0.0);
        let b = r.insert("b", Some(1), 1.0);
        r.census(&[(1.0, 1), (0.0, 0)]);
        assert_eq!(r.len(), 1);
        assert!(r.get(b).is_none());
        let c = r.insert("c", Some(1), 2.0);
        assert_eq!(c, b, "freed slot reused");
        assert_eq!(r.get(c).unwrap().serial, 3);
        assert_eq!(*r.genome(a), "a");
        assert_eq!(r.total_ever(), 3);
    }
}
