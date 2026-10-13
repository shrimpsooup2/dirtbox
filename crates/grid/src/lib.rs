//! Geodesic hex grid over the unit sphere (design §4.1).
//!
//! Built by subdividing each face of an icosahedron into an `n × n` triangular
//! lattice and projecting onto the sphere. The lattice vertices are the cell
//! centres; each cell is the Voronoi region of its centre, so it is a hexagon,
//! except the 12 pentagons that sit on the icosahedron's corners.
//! Subdivision `n` gives `10n² + 2` cells.
//!
//! Positions are unit vectors; multiply areas by R² and distances by R for a
//! planet of radius R.

use std::collections::HashMap;

use dirtbox_core::Vec3;
use dirtbox_core::math::spherical_triangle_area;

pub type CellId = u32;

pub struct GeodesicGrid {
    subdivision: u32,
    positions: Vec<Vec3>,
    /// CSR offsets into `neighbour_list`; cell `c` owns
    /// `neighbour_list[neighbour_offsets[c]..neighbour_offsets[c + 1]]`.
    neighbour_offsets: Vec<u32>,
    /// Neighbours of each cell, counter-clockwise seen from outside.
    neighbour_list: Vec<CellId>,
    /// Cell areas in steradians (sum to 4π).
    areas: Vec<f64>,
    /// Corners of each cell polygon, as indices into `circumcentres`, in the
    /// same CSR layout as the neighbours: corner k lies between neighbours
    /// k and k + 1.
    corner_list: Vec<u32>,
    /// Circumcentre of each mesh triangle on the unit sphere; these are the
    /// Voronoi (cell polygon) corners.
    circumcentres: Vec<Vec3>,
    /// Triangles of the cell-centre mesh, counter-clockwise seen from outside.
    triangles: Vec<[CellId; 3]>,
    /// Per neighbour entry (CSR): great-circle length of the shared edge and
    /// distance between the two centres, both on the unit sphere, plus the unit
    /// tangent at the cell centre pointing toward the neighbour.
    edge_lengths: Vec<f64>,
    centre_distances: Vec<f64>,
    edge_normals: Vec<Vec3>,
    locator: Locator,
}

impl GeodesicGrid {
    /// Number of cells produced by a given subdivision.
    pub fn cell_count_for(subdivision: u32) -> usize {
        10 * (subdivision as usize).pow(2) + 2
    }

    /// The smallest subdivision giving at least `cells` cells.
    pub fn subdivision_for_cells(cells: usize) -> u32 {
        let mut n = 1;
        while Self::cell_count_for(n) < cells {
            n += 1;
        }
        n
    }

    pub fn new(subdivision: u32) -> Self {
        assert!(subdivision >= 1, "subdivision must be at least 1");
        let (positions, triangles) = build_mesh(subdivision);
        let cells = build_cells(&positions, &triangles);
        let mut grid = GeodesicGrid {
            subdivision,
            positions,
            neighbour_offsets: cells.offsets,
            neighbour_list: cells.neighbours,
            areas: cells.areas,
            corner_list: cells.corners,
            circumcentres: cells.circumcentres,
            triangles,
            edge_lengths: Vec::new(),
            centre_distances: Vec::new(),
            edge_normals: Vec::new(),
            locator: Locator::default(),
        };
        grid.build_edge_geometry();
        grid.locator = Locator::build(&grid);
        grid
    }

    pub fn subdivision(&self) -> u32 {
        self.subdivision
    }

    pub fn cell_count(&self) -> usize {
        self.positions.len()
    }

    pub fn cells(&self) -> impl Iterator<Item = CellId> + use<> {
        0..self.positions.len() as CellId
    }

    /// Cell centre on the unit sphere.
    pub fn position(&self, cell: CellId) -> Vec3 {
        self.positions[cell as usize]
    }

    pub fn latitude(&self, cell: CellId) -> f64 {
        self.position(cell).latitude()
    }

    pub fn longitude(&self, cell: CellId) -> f64 {
        self.position(cell).longitude()
    }

    /// Neighbours, counter-clockwise seen from outside the sphere.
    pub fn neighbours(&self, cell: CellId) -> &[CellId] {
        let c = cell as usize;
        let start = self.neighbour_offsets[c] as usize;
        let end = self.neighbour_offsets[c + 1] as usize;
        &self.neighbour_list[start..end]
    }

    /// Cell area in steradians.
    pub fn area(&self, cell: CellId) -> f64 {
        self.areas[cell as usize]
    }

    /// Corners of the cell polygon on the unit sphere, counter-clockwise seen
    /// from outside.
    pub fn corners(&self, cell: CellId) -> impl ExactSizeIterator<Item = Vec3> + '_ {
        let c = cell as usize;
        let start = self.neighbour_offsets[c] as usize;
        let end = self.neighbour_offsets[c + 1] as usize;
        self.corner_list[start..end]
            .iter()
            .map(|&t| self.circumcentres[t as usize])
    }

    /// Index range of a cell's entries in the per-neighbour (CSR) arrays.
    pub fn neighbour_range(&self, cell: CellId) -> std::ops::Range<usize> {
        let c = cell as usize;
        self.neighbour_offsets[c] as usize..self.neighbour_offsets[c + 1] as usize
    }

    /// Flat per-neighbour arrays, indexed by [`neighbour_range`](Self::neighbour_range).
    pub fn neighbour_ids(&self) -> &[CellId] {
        &self.neighbour_list
    }

    /// Great-circle length of each shared edge (unit sphere).
    pub fn edge_lengths(&self) -> &[f64] {
        &self.edge_lengths
    }

    /// Great-circle distance between each pair of neighbouring centres (unit sphere).
    pub fn centre_distances(&self) -> &[f64] {
        &self.centre_distances
    }

    /// Unit tangent at the cell centre pointing toward each neighbour.
    pub fn edge_normals(&self) -> &[Vec3] {
        &self.edge_normals
    }

    /// Local east and north unit vectors at a cell centre.
    pub fn east_north(&self, cell: CellId) -> (Vec3, Vec3) {
        east_north_at(self.position(cell))
    }

    fn build_edge_geometry(&mut self) {
        let n = self.neighbour_list.len();
        let mut edge_lengths = Vec::with_capacity(n);
        let mut centre_distances = Vec::with_capacity(n);
        let mut edge_normals = Vec::with_capacity(n);
        for cell in self.cells() {
            let p = self.position(cell);
            let corners: Vec<Vec3> = self.corners(cell).collect();
            let degree = corners.len();
            for (k, &nb) in self.neighbours(cell).iter().enumerate() {
                // The edge shared with neighbour k runs between corners k−1 and k.
                edge_lengths.push(arc(corners[(k + degree - 1) % degree], corners[k]));
                let q = self.position(nb);
                centre_distances.push(arc(p, q));
                edge_normals.push((q - p * p.dot(q)).normalized());
            }
        }
        self.edge_lengths = edge_lengths;
        self.centre_distances = centre_distances;
        self.edge_normals = edge_normals;
    }

    pub fn triangles(&self) -> &[[CellId; 3]] {
        &self.triangles
    }

    /// The cell containing a point (any non-zero vector; it is projected onto
    /// the sphere).
    pub fn locate(&self, point: Vec3) -> CellId {
        let p = point.normalized();
        self.walk(self.locator.start_cell(p), p)
    }

    /// Like [`locate`](Self::locate), starting the search from `hint`. Cheap
    /// when the point is near the hint, as it is for a moving organism.
    pub fn locate_from(&self, hint: CellId, point: Vec3) -> CellId {
        self.walk(hint, point.normalized())
    }

    /// Greedy walk to the cell centre nearest `p`. The mesh is the convex hull
    /// of the centres (a spherical Delaunay triangulation), so any centre that
    /// is not the nearest has a neighbour that is nearer, and the walk cannot
    /// get stuck.
    fn walk(&self, start: CellId, p: Vec3) -> CellId {
        let mut current = start;
        let mut best = self.position(current).dot(p);
        loop {
            let mut next = current;
            for &n in self.neighbours(current) {
                let d = self.position(n).dot(p);
                if d > best {
                    best = d;
                    next = n;
                }
            }
            if next == current {
                return current;
            }
            current = next;
        }
    }
}

/// Builds the projected lattice: vertex positions and triangles.
fn build_mesh(n: u32) -> (Vec<Vec3>, Vec<[CellId; 3]>) {
    let (corners, faces) = icosahedron();
    let nn = n as usize;

    // Ids: 12 corners, then (n − 1) points per edge, then face interiors.
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    let mut next_id = corners.len() as u32;
    for f in &faces {
        for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[0])] {
            let key = (a.min(b), a.max(b));
            edges.entry(key).or_insert_with(|| {
                let base = next_id;
                next_id += n - 1;
                base
            });
        }
    }
    let interior_per_face = (nn.saturating_sub(1) * nn.saturating_sub(2) / 2) as u32;
    let interior_base = next_id;
    let total = interior_base as usize + faces.len() * interior_per_face as usize;
    debug_assert_eq!(total, GeodesicGrid::cell_count_for(n));

    let mut positions = vec![Vec3::ZERO; total];
    for (i, &c) in corners.iter().enumerate() {
        positions[i] = c;
    }
    // Edge points are computed once, from the lower-indexed corner, so the two
    // faces sharing an edge agree on them bit for bit.
    for (&(lo, hi), &base) in &edges {
        let (a, b) = (corners[lo as usize], corners[hi as usize]);
        for t in 1..n {
            positions[(base + t - 1) as usize] = slerp(a, b, t as f64 / n as f64);
        }
    }

    let edge_point = |u: u32, v: u32, steps_from_u: u32| -> u32 {
        let base = edges[&(u.min(v), u.max(v))];
        let t = if u < v {
            steps_from_u
        } else {
            n - steps_from_u
        };
        base + t - 1
    };

    let mut triangles = Vec::with_capacity(20 * nn * nn);
    for (fi, f) in faces.iter().enumerate() {
        let [a, b, c] = *f;
        let face_base = interior_base + fi as u32 * interior_per_face;
        // Id of lattice point (i, j): i steps toward b, j steps toward c.
        let mut ids = vec![0u32; (nn + 1) * (nn + 1)];
        let mut interior = 0;
        for j in 0..=n {
            for i in 0..=(n - j) {
                let id = match (i, j) {
                    (0, 0) => a,
                    (i, 0) if i == n => b,
                    (0, j) if j == n => c,
                    (i, 0) => edge_point(a, b, i),
                    (0, j) => edge_point(a, c, j),
                    (i, j) if i + j == n => edge_point(b, c, j),
                    (i, j) => {
                        let id = face_base + interior;
                        interior += 1;
                        // Great-circle construction: the row j runs from the
                        // point j/n along a→c to the point j/n along b→c.
                        let (fa, fb, fc) = (
                            corners[a as usize],
                            corners[b as usize],
                            corners[c as usize],
                        );
                        let fj = j as f64 / n as f64;
                        let (left, right) = (slerp(fa, fc, fj), slerp(fb, fc, fj));
                        positions[id as usize] = slerp(left, right, i as f64 / (n - j) as f64);
                        id
                    }
                };
                ids[j as usize * (nn + 1) + i as usize] = id;
            }
        }
        let at = |i: u32, j: u32| ids[j as usize * (nn + 1) + i as usize];
        for j in 0..n {
            for i in 0..(n - j) {
                triangles.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
                if i + j + 1 < n {
                    triangles.push([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
    }
    (positions, triangles)
}

/// Great-circle distance between two unit vectors.
fn arc(a: Vec3, b: Vec3) -> f64 {
    a.cross(b).length().atan2(a.dot(b))
}

/// East and north unit vectors at a point on the unit sphere.
pub fn east_north_at(p: Vec3) -> (Vec3, Vec3) {
    let east = Vec3::new(0.0, 0.0, 1.0).cross(p);
    let east = if east.length() < 1e-12 {
        Vec3::new(0.0, 1.0, 0.0)
    } else {
        east.normalized()
    };
    (east, p.cross(east))
}

/// Point a fraction `t` of the way along the great circle from `a` to `b`
/// (both unit vectors, not antipodal).
fn slerp(a: Vec3, b: Vec3, t: f64) -> Vec3 {
    let omega = a.dot(b).clamp(-1.0, 1.0).acos();
    let s = omega.sin();
    (a * (((1.0 - t) * omega).sin() / s) + b * ((t * omega).sin() / s)).normalized()
}

struct Cells {
    offsets: Vec<u32>,
    neighbours: Vec<CellId>,
    corners: Vec<u32>,
    areas: Vec<f64>,
    circumcentres: Vec<Vec3>,
}

/// Neighbour rings, polygon corners (CSR) and Voronoi cell areas from the
/// triangle mesh.
fn build_cells(positions: &[Vec3], triangles: &[[CellId; 3]]) -> Cells {
    let count = positions.len();
    // For each vertex v, map neighbour x → (next neighbour y, triangle index)
    // for every triangle (v, x, y) in counter-clockwise order.
    let mut fans: Vec<Vec<(CellId, CellId, usize)>> = vec![Vec::with_capacity(6); count];
    for (ti, &[a, b, c]) in triangles.iter().enumerate() {
        fans[a as usize].push((b, c, ti));
        fans[b as usize].push((c, a, ti));
        fans[c as usize].push((a, b, ti));
    }

    let circumcentres: Vec<Vec3> = triangles
        .iter()
        .map(|&[a, b, c]| {
            let (pa, pb, pc) = (
                positions[a as usize],
                positions[b as usize],
                positions[c as usize],
            );
            (pb - pa).cross(pc - pa).normalized()
        })
        .collect();

    let mut offsets = Vec::with_capacity(count + 1);
    let mut list = Vec::with_capacity(count * 6);
    let mut corners = Vec::with_capacity(count * 6);
    let mut areas = Vec::with_capacity(count);
    offsets.push(0u32);
    for (v, fan) in fans.iter().enumerate() {
        let degree = fan.len();
        assert!(degree == 5 || degree == 6, "vertex {v} has degree {degree}");
        let centre = positions[v];
        let mut current = fan[0].0;
        let mut area = 0.0;
        for _ in 0..degree {
            let &(_, next, ti) = fan.iter().find(|e| e.0 == current).expect("open fan");
            list.push(current);
            corners.push(ti as u32);
            // Voronoi cell = union of triangles (centre, cc_k, cc_k+1); the
            // triangle (v, current, next) contributes circumcentre cc_ti, and
            // the following one shares the edge (v, next).
            let (_, _, ti_next) = *fan.iter().find(|e| e.0 == next).expect("open fan");
            area += spherical_triangle_area(centre, circumcentres[ti], circumcentres[ti_next]);
            current = next;
        }
        debug_assert_eq!(current, fan[0].0, "fan around {v} did not close");
        offsets.push(list.len() as u32);
        areas.push(area);
    }
    Cells {
        offsets,
        neighbours: list,
        corners,
        areas,
        circumcentres,
    }
}

/// Regular icosahedron: 12 unit vertices and 20 counter-clockwise faces.
fn icosahedron() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let phi = (1.0 + 5f64.sqrt()) / 2.0;
    let mut v = Vec::with_capacity(12);
    for &s1 in &[-1.0, 1.0] {
        for &s2 in &[-1.0, 1.0] {
            v.push(Vec3::new(0.0, s1, s2 * phi));
            v.push(Vec3::new(s1, s2 * phi, 0.0));
            v.push(Vec3::new(s2 * phi, 0.0, s1));
        }
    }
    // Faces are the triples of mutually adjacent vertices (edge length 2
    // before normalising), oriented to face outward.
    let adjacent = |i: usize, j: usize| ((v[i] - v[j]).length() - 2.0).abs() < 1e-9;
    let mut faces = Vec::with_capacity(20);
    for i in 0..12 {
        for j in (i + 1)..12 {
            for k in (j + 1)..12 {
                if adjacent(i, j) && adjacent(j, k) && adjacent(i, k) {
                    let (a, b, c) = (v[i], v[j], v[k]);
                    let outward = (b - a).cross(c - a).dot(a + b + c) > 0.0;
                    let (j, k) = if outward { (j, k) } else { (k, j) };
                    faces.push([i as u32, j as u32, k as u32]);
                }
            }
        }
    }
    assert_eq!(faces.len(), 20);
    (v.into_iter().map(Vec3::normalized).collect(), faces)
}

/// Coarse latitude/longitude table of starting cells for `locate`, so a cold
/// lookup only walks a few steps.
#[derive(Default)]
struct Locator {
    rows: usize,
    cols: usize,
    start: Vec<CellId>,
}

impl Locator {
    fn build(grid: &GeodesicGrid) -> Self {
        let rows = (2 * grid.subdivision as usize).max(4);
        let cols = 2 * rows;
        let mut start = Vec::with_capacity(rows * cols);
        let mut hint = 0;
        for r in 0..rows {
            for c in 0..cols {
                let (lat, lon) = Self::bucket_centre(rows, cols, r, c);
                hint = grid.walk(hint, Vec3::from_lat_lon(lat, lon));
                start.push(hint);
            }
        }
        Locator { rows, cols, start }
    }

    fn bucket_centre(rows: usize, cols: usize, r: usize, c: usize) -> (f64, f64) {
        use std::f64::consts::{PI, TAU};
        let lat = -PI / 2.0 + (r as f64 + 0.5) * PI / rows as f64;
        let lon = -PI + (c as f64 + 0.5) * TAU / cols as f64;
        (lat, lon)
    }

    fn start_cell(&self, p: Vec3) -> CellId {
        use std::f64::consts::{PI, TAU};
        let r = (((p.latitude() + PI / 2.0) / PI) * self.rows as f64) as usize;
        let c = (((p.longitude() + PI) / TAU) * self.cols as f64) as usize;
        self.start[r.min(self.rows - 1) * self.cols + c.min(self.cols - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirtbox_core::Rng;
    use std::f64::consts::PI;

    #[test]
    fn cell_counts_match_formula() {
        for n in [1, 2, 3, 7, 16] {
            let g = GeodesicGrid::new(n);
            assert_eq!(g.cell_count(), GeodesicGrid::cell_count_for(n));
            assert_eq!(g.triangles().len(), 20 * (n as usize).pow(2));
        }
    }

    #[test]
    fn exactly_twelve_pentagons() {
        let g = GeodesicGrid::new(9);
        let pentagons = g.cells().filter(|&c| g.neighbours(c).len() == 5).count();
        assert_eq!(pentagons, 12);
        assert!(g.cells().all(|c| matches!(g.neighbours(c).len(), 5 | 6)));
    }

    #[test]
    fn neighbours_are_symmetric_and_ccw() {
        let g = GeodesicGrid::new(6);
        for c in g.cells() {
            let p = g.position(c);
            let ring = g.neighbours(c);
            for (k, &n) in ring.iter().enumerate() {
                assert!(g.neighbours(n).contains(&c), "{c} -> {n} not symmetric");
                let m = ring[(k + 1) % ring.len()];
                let turn = (g.position(n) - p).cross(g.position(m) - p).dot(p);
                assert!(turn > 0.0, "ring of {c} not counter-clockwise");
            }
        }
    }

    #[test]
    fn areas_sum_to_sphere_and_are_fairly_uniform() {
        for n in [1, 4, 32] {
            let g = GeodesicGrid::new(n);
            let total: f64 = g.cells().map(|c| g.area(c)).sum();
            assert!((total - 4.0 * PI).abs() < 1e-9, "n={n}: total area {total}");
        }
        let g = GeodesicGrid::new(32);
        let (min, max) = g
            .cells()
            .map(|c| g.area(c))
            .fold((f64::MAX, 0.0f64), |(lo, hi), a| (lo.min(a), hi.max(a)));
        assert!(max / min < 1.3, "area ratio {}", max / min);
    }

    #[test]
    fn locate_matches_brute_force() {
        let g = GeodesicGrid::new(12);
        let mut rng = Rng::from_seed(1);
        for _ in 0..2000 {
            let p = Vec3::new(rng.normal(), rng.normal(), rng.normal()).normalized();
            let found = g.locate(p);
            let best = g
                .cells()
                .map(|c| g.position(c).dot(p))
                .fold(f64::MIN, f64::max);
            assert!(
                (g.position(found).dot(p) - best).abs() < 1e-12,
                "locate picked a non-nearest cell"
            );
            // Walking from an arbitrary far hint lands on the same answer.
            let from_far = g.locate_from(0, p);
            assert!((g.position(from_far).dot(p) - best).abs() < 1e-12);
        }
    }

    #[test]
    fn corners_bound_the_cell() {
        let g = GeodesicGrid::new(5);
        for c in g.cells() {
            let p = g.position(c);
            let corners: Vec<Vec3> = g.corners(c).collect();
            assert_eq!(corners.len(), g.neighbours(c).len());
            // Each corner is equidistant from the cell centre and the two
            // neighbours it sits between (it is their circumcentre).
            for (k, &q) in corners.iter().enumerate() {
                let ring = g.neighbours(c);
                let a = g.position(ring[k]);
                let b = g.position(ring[(k + 1) % ring.len()]);
                assert!((q.dot(p) - q.dot(a)).abs() < 1e-12);
                assert!((q.dot(p) - q.dot(b)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn edge_geometry_is_consistent() {
        let g = GeodesicGrid::new(6);
        for c in g.cells() {
            let range = g.neighbour_range(c);
            let mut closure = Vec3::ZERO;
            let mut perimeter = 0.0;
            for i in range.clone() {
                let nb = g.neighbour_ids()[i];
                // The same edge seen from the other side matches.
                let j = g
                    .neighbour_range(nb)
                    .find(|&j| g.neighbour_ids()[j] == c)
                    .unwrap();
                assert!((g.edge_lengths()[i] - g.edge_lengths()[j]).abs() < 1e-12);
                assert!((g.centre_distances()[i] - g.centre_distances()[j]).abs() < 1e-12);
                let n = g.edge_normals()[i];
                assert!(n.dot(g.position(c)).abs() < 1e-12, "normal not tangent");
                assert!(
                    n.dot(g.position(nb) - g.position(c)) > 0.0,
                    "normal points away"
                );
                closure += n * g.edge_lengths()[i];
                perimeter += g.edge_lengths()[i];
            }
            // Σ L·n around a closed cell is ~0 (discrete divergence theorem).
            assert!(
                closure.length() < 0.02 * perimeter,
                "{}",
                closure.length() / perimeter
            );
        }
    }

    #[test]
    fn east_and_north_are_orthonormal_tangents() {
        let p = Vec3::from_lat_lon(0.4, 1.1);
        let (e, n) = east_north_at(p);
        assert!(e.dot(p).abs() < 1e-12 && n.dot(p).abs() < 1e-12 && e.dot(n).abs() < 1e-12);
        assert!((e.length() - 1.0).abs() < 1e-12 && (n.length() - 1.0).abs() < 1e-12);
        assert!(n.z > 0.0, "north points north");
        assert!(e.y > 0.0 || e.x.abs() > 0.0);
    }

    #[test]
    fn subdivision_for_cells_is_minimal() {
        assert_eq!(GeodesicGrid::subdivision_for_cells(40_962), 64);
        assert_eq!(GeodesicGrid::subdivision_for_cells(40_963), 65);
    }
}
