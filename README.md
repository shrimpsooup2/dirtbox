# Dirtbox

A planet-scale life simulator. You set up the world (star, orbit, size, atmosphere,
terrain, disasters). Life starts as simple bacteria, builds bodies from physical
building blocks, thinks with neural networks, and evolves to survive in the world
you made. Behaviours are never scripted: they emerge from physics, chemistry and
energy costs.

See [docs/DESIGN.md](docs/DESIGN.md) for the full design and roadmap.

## Status

Phases 0 (foundations), 1 (planet and climate) and 2 (chemistry and the microbial era)
are done.

| Crate | What it does |
|---|---|
| `crates/core` | SI constants, clocks (sim / planet / geologic, with a season-speed knob), step levels, deterministic RNG streams, sphere math |
| `crates/grid` | Geodesic hex grid (10n²+2 cells), neighbours, Voronoi areas and corners, edge geometry, point → cell lookup |
| `crates/planet` | Star, orbit, tilt, rotation and tidal locking, size, atmosphere; seasons and sunlight; seven presets; noise terrain, vents and terraforming brushes |
| `crates/climate` | Energy balance with a gray greenhouse, circulation-cell winds, clouds, rain and snow, sea ice, rivers and lakes, climate zones, optional carbon cycle |
| `crates/chemistry` | Dissolved pools, microbial genomes with eight metabolic pathways, strain fields, metabolism, dispersal, mutation, milestones |
| `crates/sim` | The world (planet, terrain, climate, life), multi-rate scheduler, spin-up, live edits, fast-forward, state hashing |
| `crates/hw` | Hardware detection, calibration benchmark, population-budget recommendation |
| `crates/cli` | The `dirtbox` command-line tool |
| `crates/viewer` | The 3D globe: satellite view, climate and life overlays, Life tab, planet editor, terraforming, new-world dialog, cell inspector, calibration |

Calibrated against Earth: mean ~287 K, ~11% ice, ~64% cloud, ~2.4 mm/day of rain, and
+3.9 K for doubled CO₂. The tests check that seasons flip between hemispheres, deserts
form in the subtropics, mountains cast rain shadows, a dimmer sun freezes the planet,
a snowball stays frozen, and terraformed mountains turn cold.

Life starts as base bacteria (warm-water fermenters) at hydrothermal vents. Every other
metabolism has to evolve: methanogenesis, chemosynthesis, anoxygenic and oxygenic
photosynthesis, aerobic respiration, nitrogen fixation and engulfing other cells. Each is a
real reaction with a real energy yield, so what spreads depends on the planet: light,
vents, temperature, nutrients and oxygen. Oxygen comes only from life, poisons the
anaerobes, and may or may not fill the air. The tests check that bacteria spread, new
metabolisms evolve, oxygenic photosynthesis raises O₂, methanogens make methane, carbon
is conserved exactly and runs replay bit for bit.

Next: Phase 3 (multicellularity and producers).

## Build and run

Requires Rust 1.88 or newer.

```sh
cargo test                                         # run the test suite
cargo run --release -- calibrate                   # benchmark this machine, get a recommended population
cargo run --release -- calibrate --population 50000
cargo run --release -- grid --cells 160000         # geodesic grid statistics
cargo run --release -- hwinfo
cargo run --release -- climate --preset earth --years 8   # run a climate headless, print zonal means
cargo run --release -- life --years 60              # run the microbial era headless (Early Earth)
cargo run --release -- life --until oxygenic --structural 5   # fast-forward to a milestone
cargo run --release -p dirtbox-viewer               # the 3D globe
```

### Viewer controls

- Drag to rotate, scroll or pinch to zoom, click a cell to select it.
- **View** tab: overlays (satellite, elevation, temperature, rain, wind, clouds, snow and
  ice, rivers, climate zones, sunlight, biomass, main metabolism, diversity, dissolved
  oxygen, organic food, nutrients), wind arrows, relief exaggeration.
- **Life** tab: the biosphere's numbers, which metabolisms exist and when they appeared,
  the most common strains, an event log, a button to seed base bacteria, and
  **fast forward** to a milestone (e.g. oxygenic photosynthesis or oxygen in the air).
  Selecting a cell lists the strains living there; hover one for its genome.
- **Planet** tab: edit the star, orbit, tilt, rotation, size and atmosphere live.
- **Terraform** tab: turn the brush on, then left-drag to raise, lower, smooth or
  flatten (right-drag rotates). The sea-level slider floods or drains the planet.
- **New world…**: pick a preset, seed and grid size, and whether to start with life.
  New worlds default to Early Earth with base bacteria.
- Space pauses and resumes. The speed buttons set sim seconds per real second. At the
  highest speeds the climate is sampled every few days (the date's tooltip says so).
- **Settings** tab: mutation and structural-mutation multipliers (0–100×) to speed up
  or freeze evolution.
- On first launch the Performance window opens. Run calibration and pick a
  population budget. The choice is remembered.

The viewer needs OpenGL 3.3. On Linux it also needs the standard desktop
libraries `libxkbcommon-x11` and `libGL`, which almost every desktop install
already has.
