# Dirtbox — Design Plan

A planet-scale life simulator. You build and change a world: its star, orbit,
size, atmosphere, terrain and disasters. Life starts as simple bacteria, gets a
kit of physical building blocks and a neural brain, pays real energy and matter
costs for everything it does, and evolves to fit the world you made.

> Status: **design v0.2**. The open decisions from v0.1 are settled (see §16).
> Phase 0 headless core is done: core, grid, sim scheduler, hardware
> calibration and CLI. The 3D globe viewer is next. It needs crates.io access
> for its graphics dependencies.

---

## 1. Design principles

1. **Simulate constraints, not behaviours.** Nothing like `if predator_near: flee`
   or `if tired: build_nest` gets written. We write physics, chemistry, costs,
   sensors and generic actuators. Fleeing, herding, migration, trees, flight,
   burrows, nests and dams either *emerge* or don't happen.
2. **Conserve energy and matter.** Every joule and every gram of carbon, nitrogen,
   phosphorus and water is accounted for. Organisms are built out of the
   environment and go back to it when they die. A debug check verifies totals
   every N ticks; a leak is a bug.
3. **Effects run both ways.** The planet shapes life, and life shapes the planet:
   plants change albedo and CO₂, oxygen-making microbes change the atmosphere,
   decomposers change soil, burrowers move earth, dam builders move water.
4. **Use real units and real-ish equations.** SI units throughout, with constants
   from physics and biology. Where we simplify, the simplification keeps the
   right *trends*.
5. **Every number is a parameter.** Every tunable lives in a typed config with
   units and a comment, so the player and we can change it.
6. **Deterministic and inspectable.** The same seed and the same inputs give the
   same world. You can click anything and see why it is the way it is.
7. **Keep the sim core headless.** The simulation is a library with no rendering
   dependency. The viewer, the CLI batch runner and tests all drive the same
   core.
8. **Detail where you are looking; speed where nothing is happening.** Slow
   stretches such as the microbial era run at a coarse level of detail by
   default. The full-detail version of every stage still exists as an option.

---

## 2. System overview

```
┌──────────────────────────────── Player / God tools ───────────────────────────────┐
│ planet params · terraform · climate overrides · disasters · seeding · time control │
└──────────────────────────────────────────┬─────────────────────────────────────────┘
                                           ▼
 Astronomy ──► Insolation ──► Climate (temp, wind, moisture, ice) ──► Hydrology (rain, rivers, soil water)
     │                              ▲   ▲                                    │
     │                     albedo,  │   │ CO₂/O₂/H₂O/CH₄                      ▼
     │                 transpiration│   │                      Geology, soil & world material (§9)
     ▼                              │   │                                    │  ▲ dig / carry / place
 Day/night, seasons, tides          │   └──── Biogeochemistry (C, N, P, O₂, water cycles) ◄──┤  │
                                    │                       ▲      │                          │  │
                         ┌──────────┴───────────────────────┴──────▼─────────────────────┐    │  │
                         │  MICROBIAL LAYER (strain fields per cell, §6)                  │    │  │
                         │        │ multicellularity                                      │    │  │
                         │        ▼                                                       │    │  │
                         │  ORGANISMS (§7): Genome ─► Development ─► Body ─► Metabolism   │◄───┘  │
                         │                 sensors ─► Brain (§8) ─► actuators ────────────┼───────┘
                         │  Reproduction ─► Mutation/Recombination ─► Speciation          │
                         └────────────────────────────────────────────────────────────────┘
```

---

## 3. The planet (player controlled)

### 3.1 Star
| Parameter | Effect |
|---|---|
| Mass M★ | Luminosity (L ∝ M★^3.5 on the main sequence), lifetime, orbital period |
| Effective temperature | Spectrum (blackbody), which sets the photosynthetically usable fraction of light and how much UV reaches the surface |
| Variability / flares | Random luminosity changes and radiation events |

### 3.2 Orbit and rotation
| Parameter | Derived / effect |
|---|---|
| Semi-major axis a | Year length T = 2π√(a³/GM★); mean stellar flux S = L/4πr² |
| Eccentricity e | Distance varies over the year (Kepler's equation solved each step) |
| Axial tilt ε | Solar declination sin δ = sin ε · sin λ, which drives seasons |
| Rotation period | Day length, Coriolis strength, number of circulation cells |
| Tidal locking | Permanent day side, night side and terminator ring |
| Moons (optional) | Tides (intertidal zones), night light, tilt stability |

Insolation at each cell comes from the solar zenith angle:
`cos z = sin φ sin δ + cos φ cos δ cos h`. Day/night, polar day and night, and
seasons follow from geometry, not from a calendar.

### 3.3 Planet body
| Parameter | Derived / effect |
|---|---|
| Radius R, mean density ρ | Mass, surface gravity g = GM/R², escape velocity (warns if the atmosphere will bleed away) |
| Gravity g | Skeleton cost, locomotion cost, max tree height, flight threshold, erosion |
| Ocean fraction, sea level | Land/ocean split, heat capacity, moisture supply |
| Terrain generator | Noise and plate seeds, continents, mountains |
| Internal heat | Volcanism, CO₂ outgassing, hydrothermal vents (energy for early life), earthquakes |

### 3.4 Atmosphere
| Parameter | Effect |
|---|---|
| Surface pressure | Heat transport, drag, sound range, how easy flight is |
| CO₂ | Greenhouse forcing (logarithmic), photosynthesis |
| O₂ | Starts near zero on a "from scratch" world; rises only if life makes it. Caps aerobic metabolism and body size; makes fire possible |
| H₂O vapour | Greenhouse, clouds, rain; emergent |
| CH₄ | Greenhouse; methanogenic microbes can produce it |

The atmosphere is a set of global, well-mixed pools. Life and geology move gas
between those pools and the surface.

---

## 4. Climate and environment

The goal is a world that *behaves* right (seasons, deserts at ~30°, monsoons,
rain shadows, ice-albedo runaway) at a cost a desktop can run. It is not a
global circulation model.

### 4.1 Spatial grid
- A **geodesic hex grid** (subdivided icosahedron; level n gives 10n²+2 cells)
  for every environmental field. It has no pole singularities, cells have similar
  areas, and each has 6 neighbours (12 have 5). *(Implemented in Phase 0.)*
- Each cell holds: elevation, soil, water, ice, temperature, humidity, wind,
  light by canopy layer, nutrients, detritus, microbial strains, producer
  cohorts, scent/sound fields.
- Physics (gravity, Coriolis, heat transport) uses the planet's real radius. The
  living surface is a scale model of it (§12.4).

### 4.2 Energy balance (per cell)
```
C · dT/dt = S_in·(1 − α) − ε_eff·σ·T⁴ + transport + latent_heat + biology_heat
```
- **C** is heat capacity; ocean is much larger than land, which gives maritime
  vs. continental climates.
- **α** is albedo from ocean, soil, vegetation, snow/ice and clouds.
- **ε_eff** is effective emissivity, lowered by greenhouse optical depth
  τ(CO₂, H₂O, CH₄, pressure).
- **Transport** is diffusion plus advection along the wind.

Ice-albedo, water-vapour and vegetation-albedo feedbacks all fall out of this.

### 4.3 Wind and circulation
Circulation bands are derived from the rotation rate and the temperature
gradient. A pressure-gradient term adds breezes and monsoons. Storm systems are
spawned stochastically where heat and moisture allow.

### 4.4 Hydrology
- Evaporation uses Clausius–Clapeyron, and wind advects the moisture.
- Rain falls when air is saturated, with orographic lift over mountains.
- Runoff routes downhill into rivers, lakes and basins. Soil water uses a bucket
  model and feeds groundwater.
- Snowpack, glaciers and seasonal floods are included. Salinity is optional.
- Water can be **blocked by placed material** (§9), so dams can work.

### 4.5 Geology and soil
- A heightmap plus slow erosion and deposition, with optional plate tectonics.
- Soil per cell: depth, texture, mineral N/P, organic matter, hardness (how
  costly it is to dig).
- The carbon–silicate cycle (outgassing against weathering) acts as a long-term
  thermostat.
- Hydrothermal vents supply chemical energy (H₂, H₂S, Fe²⁺) for early life.

### 4.6 Light and the canopy
Light reaching each vertical layer follows `I(z) = I₀ · exp(−k · LAI_above(z))`.
Water absorbs light with depth, which gives an ocean photic zone.

### 4.7 Fire
Lightning or lava ignites dry fuel. Fire needs O₂, so a planet without oxygen
cannot burn.

---

## 5. Biogeochemistry (matter cycles)

Tracked pools: **C, N, P, H₂O, O₂, plus abiotic organics and chemical reductants
(H₂, H₂S, Fe²⁺) for the early era.**

| Pool | Where it lives |
|---|---|
| Atmospheric CO₂, O₂, CH₄ | Global |
| Dissolved nutrients, organics, reductants | Ocean/lake cells |
| Soil mineral N, P | Per land cell |
| Detritus (fast and slow) | Per cell |
| Living biomass | Inside organisms, cohorts and strains, with known composition |
| Placed material | World material layer (§9) |

- **Abiotic organics** are produced slowly by lightning, UV and vents. They are
  the first food.
- **Oxygen** comes only from oxygenic photosynthesis. It is toxic to anaerobes
  and it oxidises reduced iron and methane. A Great Oxidation Event can happen,
  or never happen.

---

## 6. Origin of life and the eras of life

The world starts **from scratch**: an empty planet seeded with base bacteria.
Each era has a **fast representation** (the default) and a **detailed
representation** (opt-in). Era changes are never scheduled; they happen when the
right traits evolve.

| Era | Fast representation (default) | Detailed option | Ends when |
|---|---|---|---|
| **0. Prebiotic chemistry** | Skipped; the world starts with base bacteria | Artificial chemistry in vent/pond cells: polymers, autocatalytic sets, compartments. Seeds the first strain when a self-replicator forms | A replicator exists |
| **1. Microbial** | **Strain fields** (§6.1) | **Microscope mode**: an individual-cell sample region you can open anywhere (§6.3) | A strain evolves stable adhesion and differentiation |
| **2. Multicellular** | Individual organisms and producer cohorts (§7) | — | (ongoing) |

### 6.1 Strain fields
Each grid cell holds up to K strains (default 8). A strain is a genotype with a
population (biomass and count). A strain genome has:
- **Metabolic modules.** It starts with one and can gain more by duplication and
  divergence. Each module is a real redox reaction with a real energy yield:

  | Module | Inputs → outputs | Notes |
  |---|---|---|
  | Fermentation | Organics → CO₂, acids | Low yield; base bacteria start here |
  | Chemosynthesis | H₂/H₂S/Fe²⁺ + CO₂ → biomass | Vent ecosystems |
  | Anoxygenic photosynthesis | Light + H₂S/Fe²⁺ + CO₂ → biomass | Needs reductants |
  | Oxygenic photosynthesis | Light + H₂O + CO₂ → biomass + **O₂** | Changes the planet |
  | Aerobic respiration | Organics + O₂ → CO₂ | About 15× the yield of fermentation; requires O₂ |
  | Methanogenesis | H₂ + CO₂ → CH₄ | Greenhouse warming |
  | Nitrogen fixation | N₂ → NH₃ | Costly; poisoned by O₂ unless protected |
  | Phagocytosis | Eats other strains | The first predators |

- **Traits:** cell size, temperature and pH optimum, O₂ tolerance, motility,
  dormancy (spores), adhesion, toxin production.
- **Endosymbiosis:** with low probability, a phagocyte keeps a prey strain's
  module. This gives complex cells: larger, with combined metabolisms.

Each step: growth from available energy and nutrients, then death, competition
for shared pools, diffusion and advection to neighbouring cells, and mutation
that spawns new strains. When there are more than K strains in a cell, the
smallest is merged into its nearest genetic relative.

This runs as cheap grid math at planet-day steps, so the microbial era can get
through thousands of microbial generations per wall-minute.

### 6.2 Multicellularity transition
A strain with high adhesion and a differentiation module (cells specialising
into types) that forms large colonies can **graduate**. Individual organisms are
then spawned from it, each with a minimal body genome derived from the strain:
photosynthetic tissue if it photosynthesises, a gut-like cavity if it eats,
plus a structural matrix. From then on the body genome evolves under the full
organism model (§7). Microbes don't go away: the strain layer stays as
decomposers, plankton, pathogens and gut symbionts.

### 6.3 Microscope mode (optional detail)
You can open a small region (e.g. 1 mm–1 cm across) at any time. Strains there
become individual cells with simple bodies (size, flagella, membrane), real
diffusion of chemicals, and simple chemotaxis rules encoded in their genes.
Results flow back into the strain field as rates. It is meant for watching and
experiments; it is not needed for the world to evolve.

### 6.4 Speed controls for early eras
- **Run until milestone:** fast-forward until something specific happens, for
  example the first oxygenic photosynthesis, O₂ above x%, the first predator
  strain, or the first multicellular organism.
- **Coarse step size** in eras with no individual organisms (planet-day steps,
  daily-mean climate).
- **Start-later presets** that begin from a saved microbial world, for players
  who want to skip straight to multicellular life.

---

## 7. Organisms: the building blocks

### 7.1 One kind of life
There is a single organism model. "Plant", "animal" and "fungus" are just names
for combinations of parts.

**Representation:**
- **Motile organisms** are individual agents with brains (§8).
- **Sessile producers** are **cohorts**: a genotype × size class × count within
  one cell patch, following the approach of forest "gap models". That lets a
  meadow contain millions of plants at the cost of a few dozen records. A grazer
  eats from a cohort and lowers its count or biomass.
- Microbes stay as strain fields (§6.1).

### 7.2 Part catalog
Every part has a **mass**, a **material composition** (carbs, protein, fat,
fibre/cellulose, lignin, mineral, water), a **build cost**, a **maintenance
cost** and a **function**.

| Part | Function | Main trade-off |
|---|---|---|
| **Photosynthetic tissue** | Light → sugar | Loses water; edible |
| **Root** | Takes up water, N and P | Competes with neighbours |
| **Structural tissue** (wood, bone, chitin) | Supports mass and height; levers for muscles | Required mass scales with g |
| **Storage** (starch, fat) | Energy reserve | Carrying it costs locomotion energy |
| **Muscle** | Force and power | Expensive to maintain |
| **Locomotor appendage** (limb, fin, wing, cilia) | Moves the body through land, water or air | Efficiency differs by medium |
| **Digging appendage** (claw, snout, mandible) | Moves soil (§9) | Mass; a poor limb for running |
| **Grabber** (jaw grip, hand, tentacle) | Picks up and carries objects (§9) | Mass, neural inputs |
| **Secretion gland** (silk, wax, mucus, cement) | Turns body matter into a building material | Matter and energy cost |
| **Mouth / jaw** | Ingests food; gape limits prey size | — |
| **Gut** | Digests food; enzyme set and length set efficiency per food type | Mass |
| **Eye** | Vision | Needs light; adds neural inputs |
| **Chemoreceptor** | Smell and taste | Slow information |
| **Ear** | Sound | Only useful if things make noise |
| **Touch / pain receptors** | Contact and damage signals | Cheap; key to learning |
| **Thermoreceptor** | Ambient and body temperature | Cheap |
| **Neural tissue** | Hosts brain neurons | Very expensive per gram |
| **Integument** | Insulation, colour, pattern | Helps in the cold, hurts in the heat |
| **Armour / shell / bark** | Reduces damage | Heavy |
| **Spikes / thorns** | Damages attackers | Build cost |
| **Toxin gland** | Toxic tissue or venom | Ongoing cost; eaters can evolve detox (arms race) |
| **Signal emitter** | Sound, light or scent | Predators can sense it too |
| **Reproductive organ** | Gametes, eggs, seeds, spores, live young | Investment against numbers |
| **Fruit** | Sugar reward around seeds | Can buy animal dispersal |

Parts can be **duplicated** by mutation and then diverge. That is the main route
to new complexity.

### 7.3 Genome and development
- **Body genome:** a graph of part genes (type, parameters, attachment, symmetry,
  repeat count), in the spirit of Karl Sims' creatures and Framsticks.
- **Life-history genes:** adult size, growth rate, maturity threshold, offspring
  investment and count, reproduction mode, dormancy triggers, body temperature
  setpoint, DNA repair investment (sets mutation rate and ageing).
- **Development:** organisms are born small, and growth costs real energy and
  matter.
- **Ageing:** damage builds up and repair costs energy, so senescence can evolve.

### 7.4 Body physics
- Mass, volume and surface area (≈ k·M^(2/3)) are derived from the parts.
- **Heat:** `P_loss = h·A·(T_body − T_env)/insulation`, plus evaporative cooling.
  `T_env` includes shelter (§9).
- **Locomotion cost per metre** comes from mass, g, slope, medium and drag.
- **Flight** works only when lift can carry M·g within the available power.
- **Size limits:** the structural mass needed grows faster than body mass, and
  scales with g.

### 7.5 Metabolism and energy
Internal pools: gut contents (by component), blood sugar/ATP, glycogen, fat,
structural protein, water, body temperature, damage, toxin load.

**Food is the tissue of what was eaten.** Energy gained per gram depends on the
food's composition (carbs ≈17 kJ/g, protein ≈17, fat ≈37, fibre ≈0 without
cellulase) and the eater's digestion efficiency for each component.

```
E = Σ(part basal rate × mass) × Q10^((T_body − T_ref)/10)   // maintenance
  + locomotion work / muscle efficiency
  + work on the world (digging, carrying, secreting)         // §9
  + brain cost (§8.4)
  + thermoregulation + growth + repair + reproduction + active part costs
```
Aerobic power is capped by O₂. Above the cap, metabolism goes anaerobic and
fatigue builds. Sleep or torpor is an actuator.

### 7.6 Producer cohorts
Light per canopy layer gives photosynthesis, limited by water, CO₂, temperature
and N. Allocation genes split new growth between leaves, wood, roots, storage,
defence and reproduction. Seeds disperse by wind, gravity, animals or water.

### 7.7 Reproduction and inheritance
- Asexual, sexual or hermaphrodite, set by genes.
- NEAT-style aligned crossover.
- Mutation rate is set by the DNA repair gene.
- Parental care is a brain decision.

---

## 8. Brains (neural networks)

### 8.1 Architecture
- **Continuous-time recurrent neural networks (CTRNN)** whose topology evolves
  NEAT-style. Each neuron has a time constant τ.
  - Continuous time also suits the variable step size (§12.3): the network is
    integrated over whatever dt the organism is taking, with sub-steps when
    dt > τ/2.
- **Neuromodulated Hebbian plasticity** gives learning within a lifetime.
  Learning is gated by internal signals such as pain and satiety. Danger
  awareness is learned or evolved; it is never coded.

### 8.2 Inputs (only from evolved sensors)
- **Eyes:** rays returning distance, colour, size and motion, including of
  **placed material and carried objects**.
- **Chemical gradients**, **sound direction**, **touch/pain**, **temperature**.
- **Internal states:** energy, hydration, gut fullness, fatigue, health,
  reproductive readiness, carried mass.
- **Ambient light.**
- A bias and a noise input.

### 8.3 Outputs (only from evolved actuators)
Thrust, turn, eat, attack, mate, signal, sleep, defensive posture, scent mark,
offspring care, **dig, grab/drop, place, push, secrete** (§9).

### 8.4 What thinking costs
`E_brain = n_neurons·c_neuron + n_synapses·c_synapse + Σ|activity|·c_activity`,
plus a build cost. Brain update rate is an evolvable trait. Calibration target:
a large-brained endotherm spends roughly 10–25% of its basal budget on its
brain.

---

## 9. Interacting with the environment (unscripted construction)

Creatures can change the world, but only through **generic physical actions**
whose effects come out of the same physics as everything else. There is no
"build nest" action, no nest object, and no code that knows what a dam is.

### 9.1 World material layer
- A **sparse fine-resolution layer** (default 0.25 m voxels on the living
  surface) is stored only where something has been changed. Elsewhere the coarse
  cell's terrain is implied.
- **Materials** each have real properties: density, hardness (dig cost),
  cohesion (stability, so piles slump), thermal conductivity (insulation),
  permeability (water flow), opacity, and edibility/decay.
  - Examples: soil, sand, clay, rock, mud, water, plant litter, wood,
    silk/wax/cement (secreted), dung, and organic remains.
- **Loose objects** are discrete items such as stones, sticks, seeds, fruit,
  carcass parts and eggs. Each has a mass, a material and a position.

### 9.2 Generic actions
| Action | Physical effect | Cost and limits |
|---|---|---|
| **Dig** | Removes material in front of or below the organism and carries it or drops it nearby | Work ∝ hardness × volume; limited by digging appendage and muscle power |
| **Grab / drop** | Picks up an object or a chunk of loose material; drops it | Mass ≤ grip strength; carrying raises locomotion cost |
| **Place** | Puts the carried item at a specific spot | Stays put only if cohesion allows |
| **Push** | Applies force to an object or material | Muscle power, friction |
| **Secrete** | Converts body matter into a material (silk, wax, cement) | Real matter and energy cost |
| **Excrete / mark** | Leaves dung and scent | Fertilises soil; leaves a smell trail |
| **Strike while holding** | Hit with momentum = held mass × speed | Tool use is possible but not designed in |

### 9.3 Why structures work (the physics, not labels)
- **Thermal:** a creature's `T_env` comes from the conductivity and thickness of
  material around it. In a burrow 1 m down, the daily and seasonal temperature
  swing is damped, as in real soil. Piled litter insulates, so nests can emerge
  for cold or for eggs.
- **Visibility:** opaque material blocks eye rays. Hiding emerges.
- **Movement:** solid material blocks movement, and narrow gaps admit only small
  bodies. Defensible burrows emerge.
- **Water:** placed material feeds into hydrology as a reduction in channel
  conductance. Enough blockage across a channel ponds water upstream, so beaver
  dams are possible.
- **Food storage:** buried seeds decay more slowly when cool and dry. Caching can
  emerge.
- **Weather:** wind and rain exposure are reduced by overhead and windward
  material.
- **Persistence:** structures erode, slump and decay over time unless
  maintained.

---

## 10. Evolution and ecology tracking

- **Species** are discovered by clustering genomes on genetic distance, never
  declared. The same applies to microbial strains.
- **Phylogeny:** a lineage tree recording speciation, extinction and traits.
- **Ecology metrics:** populations, biomass by trophic level, the food web,
  energy flow, diversity, ranges, and the amount of world material modified.

---

## 11. Player tools (the "god" layer)

### 11.1 World creation
- Star, orbit, rotation, size and density, atmosphere, oceans, terrain.
- Presets: Earth-like, Mars-ish, tidally locked red-dwarf world, super-Earth,
  ocean world, snowball.
- **Life start:** from scratch with base bacteria (default), full prebiotic
  chemistry (detailed), or a saved microbial or multicellular world.

### 11.2 Live edits
- Any planet parameter, while the sim is running.
- Terraform brush, climate overrides.
- Place or remove organisms, design organisms, and selective pressures.

### 11.3 Disasters
These run through the same systems as everything else: asteroid impacts,
volcanoes and supervolcanoes, earthquakes and tsunamis, droughts and floods,
ice ages and heat waves, plagues, solar flares, wildfires.

### 11.4 Time
Pause, step, a speed slider, "run until milestone" (§6.4), fast-forward.

### 11.5 Observation (3D globe)
- A **3D globe** with smooth zoom from orbit down to the living surface.
- Overlays: climate, biomes, nutrients, strains, species ranges.
- An organism inspector showing body, genome, live brain activity, and an energy
  Sankey diagram.
- Species and strain panels, global charts, a phylogenetic tree, and an event log
  ("first oxygenic photosynthesis", "first burrow", and so on).

---

## 12. Engine architecture

### 12.1 Stack
- **Rust** simulation core with zero rendering dependencies, multithreaded.
- **Viewer:** Bevy or wgpu + egui, 3D globe, reading snapshots of sim state.
- Config and presets in TOML or RON. Saves are versioned binary.

### 12.2 Workspace layout
```
crates/
  core/        units, constants, clocks & time model, deterministic RNG, sphere math   [Phase 0 ✓]
  grid/        geodesic hex grid, neighbours, cell areas, point → cell lookup          [Phase 0 ✓]
  sim/         world state, multi-rate scheduler, invariants, state hashing            [Phase 0 ✓]
  hw/          hardware detection, calibration benchmark, population recommendation    [Phase 0 ✓]
  cli/         `dirtbox` headless binary                                               [Phase 0 ✓]
  planet/      astronomy, orbit, insolation, geology, terrain
  climate/     energy balance, wind, hydrology, ice, fire
  chemistry/   element pools, cycles, strain fields, microscope mode
  material/    sparse world material layer, loose objects (§9)
  organism/    genome, development, parts, body physics, metabolism, producer cohorts
  brain/       CTRNN, plasticity, cost accounting
  evolution/   reproduction, recombination, mutation, speciation, phylogeny
  viewer/      3D globe, UI
```

### 12.3 Time model (decided)
Evolution needs thousands of generations. Creatures need fine-grained moments
(a chase lasts seconds). Seasons need to matter within a creature's lifetime.
Running everything at once in real time can't do all three, so time is split:

**1. One sim clock, real physiology.**
- All organism physics, metabolism, movement and brains run on the **sim clock**
  in real seconds.
- A creature burns real watts, runs at real speeds and starves in real time.
- This is never compressed, so energy budgets stay honest.

**2. Planet clock with a "season speed" knob (k).**
- Astronomy, day/night, seasons, climate, hydrology and soil processes run on
  the **planet clock**, `τ = k · t`. Climate equations are integrated in planet
  time, so thermal inertia and seasonal amplitude stay correct *relative to the
  planet's own day and year*.
- Only how fast the sky turns changes, compared with how fast creatures live.
- **Default k = 72:** an Earth day lasts 20 sim-minutes and an Earth year about
  5 sim-days. Small creatures with real physiology therefore live through real
  seasons. k = 1 is fully realistic, for purists.
- Averaged over time, light per sim-second is unchanged by k, so primary
  production per sim-second is still realistic.

**3. Geological clock.** Slow geology (tectonics, the carbon–silicate cycle,
erosion) runs at a further acceleration (default 1,000 geological years per
planet year), so the long-term thermostat still operates.

**4. Per-organism adaptive steps (block time-stepping).**
- Each motile organism steps at dt ∈ {0.25, 0.5, 1, 2, 4, 8, 16} s, chosen each
  step from:
  - time-to-contact with the nearest relevant thing (distance ÷ closing speed),
  - its fastest neuron's τ,
  - its state (asleep or idle means coarse steps).
- Organisms are bucketed by step level, as in N-body codes. Fine buckets
  synchronise with coarse ones at shared boundaries, so a chase gets 0.25 s
  resolution while a sleeping animal next to it costs 64× less.

**5. Rate-matched layers.**
| Layer | Step |
|---|---|
| Motile organisms | 0.25–16 s sim (adaptive) |
| Producer cohorts, strain fields | 1 planet-hour (50 s sim at k = 72); 1 planet-day in fast eras |
| Climate, hydrology | 1 planet-hour; daily mean in fast mode |
| Soil, chemistry | 1 planet-day |
| Geology | 1 planet-year |

**Consequences we accept:**
- Large, slow-breeding lineages evolve slowly in wall-clock time, as in reality.
- With k > 1, creatures experience shorter days and years than their
  physiology "expects". Evolution adapts to that (for example sleeping through
  20-minute nights), and the effect is consistent.

### 12.4 Space scale and population budget
- A real-sized planet surface would leave 50,000 creatures absurdly far apart.
  So the **living surface** is a uniformly scaled-down copy of the planet (scale
  factor s).
- Climate and physics still use the real radius. Organisms live at real body
  sizes and speeds on the scaled surface.
- s is chosen **automatically from the population budget** (§12.5) so that
  densities are sensible (overridable). Travel distances shrink with s, so a
  continent-crossing migration still has to fit inside a season.
- **The population itself is never capped by code.** Ecology decides how many
  organisms exist. The budget sets the size of the world (and therefore its
  carrying capacity).
- If life outgrows the hardware anyway, the sim slows down and the viewer warns.
  An optional, clearly labelled "hard cap" applies extra mortality; it is off by
  default because it is a non-physical rule.

### 12.5 Hardware calibration and recommendation *(Phase 0 ✓)*
`dirtbox calibrate` (and the first-run screen in the viewer):
1. **Detects hardware:** CPU model, core count, total and available RAM. GPU
   information comes from the viewer's wgpu adapter once the viewer exists.
2. **Benchmarks the agent step.** It runs the real per-organism step on synthetic
   organisms for a few seconds across all cores and measures agent-steps per
   second.
   - Until the organism and brain crates exist, a clearly labelled proxy workload
     stands in for it: CTRNN evaluation at a typical size, plus neighbour lookup
     and metabolism update.
3. **Recommends a population budget** for three target speeds and prints the
   expected wall-clock time per planet-year for each:
   ```
   N = throughput × mean_dt × headroom / target_speed
   ```
   - headroom ≈ 0.6 leaves time for climate, fields and the viewer.
   - The result is capped by RAM (bytes per organism × N ≤ 50% of available
     RAM).
4. You pick a tier or type any number. Re-run calibration whenever you like; the
   recommendation is also re-estimated as brains grow during evolution.

First measurement (proxy workload, 4-vCPU cloud VM): 0.66M steps/s on one thread
and 1.9M on 4 threads. That gives budgets of 230k (Watch), 23k (Balanced) and
2.3k (Evolve). The proxy calls trigonometry once per neighbour, and real
steppers will optimise that. Desktop CPUs with 8–16 cores should land several
times higher.

### 12.6 Data layout and determinism
- Environment fields are stored as structure-of-arrays over grid cells.
- Organisms live in an arena with generational IDs; brains are flattened sparse
  arrays.
- Deterministic RNG streams come from (seed, domain, entity, tick). Parallel
  stages write to per-thread buffers that are merged in sorted order.
- A **state hash** lets you verify replays bit for bit.

### 12.7 Testing
Unit tests for every equation, invariant checks (conservation, no NaNs, no
negative pools), headless scenario tests, and benchmarks.

---

## 13. Roadmap

| Phase | Scope | Exit check |
|---|---|---|
| **0. Foundations** | Workspace, core (units, clocks, RNG), geodesic grid, scheduler, state hash, hardware calibration, headless CLI. *Then:* minimal 3D globe viewer | Deterministic replay; grid areas sum to 4πR²; calibration produces a recommendation; globe renders cells |
| **1. Planet & climate** | Orbit, insolation, energy balance, albedo, greenhouse, wind, hydrology, ice, vents | Seasons flip by hemisphere; deserts and rain shadows; snowball can be triggered |
| **2. Chemistry & microbial era** | Element pools, abiotic organics, strain fields, metabolic modules, endosymbiosis, run-until-milestone | Base bacteria spread; at least one new metabolism evolves; O₂ rises if oxygenic photosynthesis appears |
| **3. Multicellularity & producers** | Graduation from strains, producer cohorts, canopy light, seeds | Vegetation follows climate; taller producers evolve under shading |
| **4. Motile bodies & metabolism** | Part catalog, development, body physics, digestion, adaptive block stepping | Energy budgets balance; starvation and growth work |
| **5. Brains** | CTRNN, sensors, actuators, plasticity, brain costs | Agents beat random at finding food; learned avoidance of toxic food |
| **6. Evolution & ecology** | Reproduction, recombination, speciation, phylogeny, pathogens | Stable predator–prey over 100+ generations; a speciation event |
| **7. Environment interaction** | Material layer, loose objects, dig/grab/place/secrete, shelter physics, dam hydrology | A dug burrow measurably damps temperature; any lineage that evolves to use it |
| **8. God tools & disasters** | Live edits, terraforming, disasters, presets | An impact winter gives an extinction and a recovery |
| **9. Observation & polish** | Inspector, charts, tree, event log, save/load UX | A new player can follow one lineage through a disaster |
| **10. Detail modes & scale** | Prebiotic chemistry, microscope mode, GPU climate, batched brains | The detailed eras run; budget targets are met |

---

## 14. Main risks

| Risk | Mitigation |
|---|---|
| Nothing interesting emerges | Viable base bacteria, per-phase emergence checks, CLI parameter sweeps |
| Microbial era stalls (no innovation) | Tunable innovation rates, run-until-milestone, saved-world starts |
| Performance | Headless core, SoA, block time-stepping, cohorts and fields, calibration |
| Too many parameters | Physically grounded units, presets, sensitivity sweeps |
| Construction never emerges | Make shelter physics strong and legible; report material-use statistics so we can tell |
| Scope creep | Phase gates and a backlog |

---

## 15. Glossary of clocks and scales

| Symbol | Name | Default |
|---|---|---|
| t | Sim clock (real seconds for organisms) | — |
| k | Season speed (planet clock = k·t) | 72 |
| g_geo | Geological acceleration | 1,000 |
| s | Living-surface scale | Auto from budget |
| dt | Organism step | 0.25–16 s, adaptive |

---

## 16. Decisions log

| # | Question | Decision |
|---|---|---|
| 1 | Stack | Rust core, wgpu/Bevy viewer, desktop first |
| 2 | Presentation | 3D globe |
| 3 | Scale | Player chooses. `calibrate` recommends a number from measured hardware performance |
| 4 | How life starts | From scratch with base bacteria. Early eras use fast representations by default, with detailed modes available |
| 5 | Construction | Yes, through generic physical actions and material physics only (§9) |
| 6 | Microbes as fields | Yes (strain fields), with an optional microscope mode |
| 7 | Time model | §12.3: real physiology on the sim clock, a season-speed knob for the planet, adaptive block time-stepping |
