# Dirtbox

A planet-scale life simulator. You set up the world (star, orbit, size, atmosphere,
terrain, disasters). Life starts as simple bacteria, builds bodies from physical
building blocks, thinks with neural networks, and evolves to survive in the world
you made. Behaviours are never scripted: they emerge from physics, chemistry and
energy costs.

See [docs/DESIGN.md](docs/DESIGN.md) for the full design and roadmap.

## Status

Phase 0 (foundations), headless core:

| Crate | What it does |
|---|---|
| `crates/core` | SI constants, clocks (sim / planet / geologic, with a season-speed knob), step levels, deterministic RNG streams, sphere math |
| `crates/grid` | Geodesic hex grid (10n²+2 cells, 12 pentagons), neighbour rings, exact Voronoi cell areas, point → cell lookup |
| `crates/sim` | World state, discrete-event multi-rate scheduler, state hashing for replay checks |
| `crates/hw` | Hardware detection, calibration benchmark, population-budget recommendation |
| `crates/cli` | The `dirtbox` command-line tool |

Next: the 3D globe viewer, then Phase 1 (planet and climate).

## Build and run

Requires Rust 1.85 or newer.

```sh
cargo test                                         # run the test suite
cargo run --release -- calibrate                   # benchmark this machine, get a recommended population
cargo run --release -- calibrate --population 50000
cargo run --release -- grid --cells 160000         # geodesic grid statistics
cargo run --release -- hwinfo
```
