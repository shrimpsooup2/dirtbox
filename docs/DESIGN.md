# Dirtbox — Design Plan

A planet-scale life simulator. You build and change a world: its star, orbit,
size, atmosphere, terrain and disasters. Life gets a kit of physical building
blocks and a neural brain, pays real energy and matter costs for everything it
does, and evolves to fit the world you made.

> Status: **planning draft v0.1**. Nothing here is final. Section 13 lists the
> decisions still open.

---

## 1. Design principles

1. **Simulate constraints, not behaviours.** Nothing like `if predator_near: flee`
   gets written. We write physics, chemistry, costs, sensors and actuators.
   Fleeing, herding, migration, hibernation, camouflage, trees, flight and
   predator–prey arms races should all *emerge* from those rules or not happen.
2. **Conserve energy and matter.** Every joule and every gram of carbon, nitrogen,
   phosphorus and water is accounted for. Organisms are built out of the
   environment and go back to it when they die. A debug check verifies totals
   every N ticks; a leak is a bug.
3. **Effects run both ways.** The planet shapes life, and life shapes the planet:
   plants change albedo and CO₂, decomposers change soil, grazers change
   vegetation, fires change carbon, respiration changes the greenhouse effect.
4. **Use real units and real-ish equations.** SI units throughout, with constants
   from physics and biology (kJ/g of fat, Stefan–Boltzmann, Kepler, Q10). Where
   we simplify, the simplification keeps the right *trends* (bigger means slower
   heat loss, more gravity means heavier skeletons, colder means slower
   metabolism).
5. **Every number is a parameter.** No magic constants hidden in code. Every
   tunable lives in a typed config with units and a comment, so the player and
   we can change it.
6. **Deterministic and inspectable.** The same seed and the same inputs give the
   same world. You can click any organism and see its genome, body, brain and
   full energy budget.
7. **Keep the sim core headless.** The simulation is a library with no rendering
   dependency. The viewer, the CLI batch runner and tests all drive the same
   core.

---

## 2. System overview

```
┌──────────────────────────────── Player / God tools ───────────────────────────────┐
│ planet params · terraform · climate overrides · disasters · seeding · time control │
└──────────────────────────────────────────┬─────────────────────────────────────────┘
                                           ▼
 Astronomy ──► Insolation ──► Climate (temp, wind, moisture, ice) ──► Hydrology (rain, rivers, soil water)
     │                              ▲   ▲                                    │
     │                     albedo,  │   │ CO₂/H₂O/CH₄                         ▼
     │                 transpiration│   │                      Geology & Soil (terrain, nutrients, minerals)
     ▼                              │   │                                    │
 Day/night, seasons, tides          │   └──── Biogeochemistry (C, N, P, O₂, water cycles) ◄──┤
                                    │                       ▲      │                          │
                                    │                       │      ▼                          │
                         ┌──────────┴───────────────────────┴──────────────────────────┐      │
                         │                       ORGANISMS                              │◄─────┘
                         │ Genome ─► Development ─► Body (parts) ─► Physiology/Metabolism│
                         │                              │  ▲                             │
                         │                 sensors ─► Brain (NN) ─► actuators            │
                         │ Reproduction ─► Mutation/Recombination ─► Speciation          │
                         └──────────────────────────────────────────────────────────────┘
                         Field life: microbes, decomposers, plankton, pathogens (grid-based)
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
| Eccentricity e | Distance varies over the year (Kepler's equation solved each step), so a season can come from distance as well as tilt |
| Axial tilt ε | Solar declination sin δ = sin ε · sin λ, which drives seasons; ε=0 gives none, ε≈90° gives extreme ones |
| Rotation period | Day length, Coriolis strength, number of atmospheric circulation cells (Earth has 3 per hemisphere; faster spin gives more, narrower bands) |
| Tidal locking | Rotation equals the orbit: a permanent day side, night side and terminator ring |
| Moons (optional) | Tides (coastal intertidal zones), night light, axial-tilt stability |

Insolation at each cell comes from the solar zenith angle:
`cos z = sin φ sin δ + cos φ cos δ cos h` (φ latitude, h hour angle). That gives
day/night, polar day and night, and seasons that follow from the geometry
instead of a calendar.

### 3.3 Planet body
| Parameter | Derived / effect |
|---|---|
| Radius R, mean density ρ | Mass, surface gravity g = GM/R², escape velocity (warns if the atmosphere will bleed away) |
| Gravity g | Structural (bone/wood) cost per kg, locomotion cost, max tree height, flight threshold, falling damage, rain and erosion speed |
| Ocean fraction, sea level | Land/ocean split, heat capacity, moisture supply |
| Terrain generator | Noise and plate seeds, continent count, mountain height |
| Internal heat / tectonic activity | Volcanism rate, CO₂ outgassing, earthquakes, new land |

### 3.4 Atmosphere
| Parameter | Effect |
|---|---|
| Surface pressure | Heat transport strength, how well air holds heat, sound range, drag, how easy flight is (lift ∝ air density) |
| CO₂ | Greenhouse forcing (logarithmic), photosynthesis rate |
| O₂ | Max aerobic metabolic rate, fire likelihood, max body size for passive breathers |
| H₂O vapour | Greenhouse forcing, clouds (albedo), rain; mostly emergent, not set directly |
| CH₄ and other trace gases | Optional extra greenhouse forcing |

The atmosphere is a set of global, well-mixed pools (CO₂, O₂, CH₄, N₂). Life and
geology move gas between those pools and the surface.

---

## 4. Climate and environment

The goal is a world that *behaves* right (seasons, deserts at ~30°, monsoons,
rain shadows, ice-albedo runaway) at a cost a desktop can run. It is not a
global circulation model.

### 4.1 Spatial grid
- **Recommendation:** a geodesic hex grid (subdivided icosahedron, ~40k–160k
  cells) for every environmental field. It has no pole singularities, every cell
  has a similar area, and each cell has 6 neighbours (12 have 5).
- Each cell holds: elevation, terrain type, soil layers, water (surface, soil,
  ground), ice/snow, temperature (air and surface), humidity, wind vector, light
  levels by canopy layer, nutrient pools, detritus pools, field-life populations,
  and scent/sound fields.
- Organisms have continuous positions on the sphere and use a spatial hash keyed
  by cell for neighbour queries.

### 4.2 Energy balance (per cell)
```
C · dT/dt = S_in·(1 − α) − ε_eff·σ·T⁴ + transport + latent_heat + biology_heat
```
- **C**: heat capacity. A deep ocean mixed layer is about 50× land, which gives
  continental versus maritime climates.
- **α**: albedo from ocean (~0.06), bare soil, vegetation type (life changes
  it), snow/ice (~0.6–0.85) and clouds.
- **ε_eff**: effective emissivity, lowered by greenhouse optical depth τ(CO₂, H₂O,
  CH₄, pressure).
- **Transport**: diffusion plus advection along the wind field. The coefficient
  scales with pressure and is damped by the Coriolis effect.
- **Feedbacks that fall out of this:** ice-albedo (a snowball planet is
  possible), water vapour, vegetation-albedo.

### 4.3 Wind and circulation
- Derive the band structure (Hadley/Ferrel/polar-style cells) from rotation rate
  and the latitudinal temperature gradient. Fast spinners get more bands; tidally
  locked worlds get substellar-to-antistellar flow.
- Add a pressure-gradient term from local temperature anomalies so land/sea
  breezes and monsoons can show up.
- Stochastic storm systems spawn where there is heat and moisture (cyclones over
  warm ocean).

### 4.4 Hydrology
- Evaporation comes from Clausius–Clapeyron saturation vapour pressure, wind and
  surface water (plus plant transpiration).
- Moisture is advected by the wind and rains out when saturated. Orographic lift
  over mountains gives rain shadows.
- Runoff flows to the lowest neighbour, which forms rivers, lakes and inland
  basins. Soil water uses a bucket model and recharges groundwater.
- Snowpack builds when cold and melts into seasonal floods. Persistent snow
  becomes glaciers.
- Ocean salinity is optional later (fresh vs. salt water as a constraint on life).

### 4.5 Geology and soil
- Terrain starts as a heightmap (noise plus optional plate seeds). Slow erosion
  and deposition reshape it. Optional plate tectonics run on long timescales.
- Soil per cell: depth, texture (water holding), mineral N/P, organic matter
  (fast and slow pools), and pH (optional).
- Carbon–silicate cycle: volcanic CO₂ outgassing against temperature-dependent
  weathering. That is the long-term thermostat, and it means the player can
  break it.

### 4.6 Light and the canopy
- Light reaching each vertical layer in a cell: `I(z) = I₀ · exp(−k · LAI_above(z))`
  (Beer–Lambert through leaf area). Tall plants shade short ones, so trees,
  understory and shade-tolerant strategies can emerge.
- Water absorbs light with depth, which gives an ocean photic zone.

### 4.7 Fire
Fire is emergent rather than scripted: lightning (from storms) or lava ignites
dry fuel (detritus plus living biomass with low water content). It spreads by
wind, fuel and moisture. Burnt biomass goes to CO₂, ash adds P to the soil, and
burnt ground is open for new growth.

---

## 5. Biogeochemistry (matter cycles)

Tracked element pools: **C, N, P, H₂O, O₂** (others can be added).

| Pool | Where it lives |
|---|---|
| Atmospheric CO₂, O₂ | Global |
| Dissolved nutrients | Ocean cells |
| Soil mineral N, P | Per land cell |
| Detritus (fast: leaves, flesh; slow: wood, bone) | Per cell |
| Living biomass | Inside organisms, as tissues with known composition |

- Photosynthesis: CO₂ + H₂O + light → sugar + O₂.
- Respiration: sugar + O₂ → CO₂ + H₂O + energy.
- Decomposition is done by field-life microbes. Its rate depends on temperature,
  moisture and O₂, and it releases N/P to the soil and CO₂ to the air.
- Nitrogen fixation is a tissue/microbe capability with an energy cost, which
  makes N a real limiting resource.
- Bodies of dead organisms stay as carcasses (food for scavengers) until they
  decompose.

---

## 6. Organisms: the building blocks

### 6.1 One kind of life, not "plants" and "animals"
There is a single organism model. "Plant", "animal", "fungus" and "filter
feeder" are just names for whatever combination of parts a lineage ends up with.
A creature with leaves and legs is allowed if it pays for both.

For performance the scheduler runs **sessile** organisms (no locomotion parts)
on a slower tick with no per-tick brain, and **motile** organisms on the fast
tick. This is a scheduling detail, not a rule of biology.

Microbes, decomposers, plankton and pathogens are **field life**: populations
per cell with evolving trait distributions, not individual agents (see §6.8).

### 6.2 Part catalog
Every part has a **mass**, a **material composition** (carbs, protein, fat,
fibre/cellulose, lignin, mineral, water), a **build cost** (energy plus elements),
a **maintenance cost** (basal W per kg, temperature dependent) and a **function**.

| Part | Function | Key parameters | Main trade-off |
|---|---|---|---|
| **Photosynthetic tissue** (leaf) | Light → sugar, using water and CO₂ | Area, pigment (tuned to the star's spectrum), thickness | Loses water through transpiration; edible; needs light |
| **Root** | Takes up water, N and P from the soil | Mass, depth | Underground cost; competes with neighbours |
| **Structural tissue** (wood, bone, chitin, cartilage) | Holds up mass and height; levers for muscles | Density, stiffness | Required mass scales with g × supported mass |
| **Storage** (starch, fat) | Energy reserve | Capacity | Carrying weight costs locomotion energy |
| **Muscle** | Force and power for movement, bites, grabs | Mass, fibre type (fast/slow) | Expensive to maintain; burns more when used |
| **Locomotor appendage** (limb, fin, wing, cilia) | Moves the body in a medium | Length, surface area, count | Efficiency differs by medium (land/water/air) |
| **Mouth / jaw** | Ingests food | Gape size, bite force, filter mode | Gape limits prey size |
| **Gut** | Digests food | Length, enzyme set (amylase, protease, lipase, cellulase), fermentation chamber | A long gut means better fibre digestion but more mass and maintenance |
| **Eye** | Vision | Resolution (ray count), range, field of view, low-light sensitivity | Needs ambient light; each ray adds a neural input |
| **Chemoreceptor** (smell/taste) | Senses scent fields and food chemistry | Sensitivity, channels | Slow, diffusion-limited information |
| **Ear** | Senses sound events and direction | Sensitivity | Only useful if things make noise |
| **Touch / pain receptors** | Contact and damage signals | Density | Cheap; key to learning |
| **Thermoreceptor** | Ambient and body temperature | — | Cheap |
| **Neural tissue** | Hosts brain neurons (see §7) | Neuron capacity | Very expensive per gram |
| **Integument** | Insulation (fur, feathers, fat layer), colour and pattern | Thickness, colour | Insulation helps in the cold and hurts in the heat; colour sets camouflage or display |
| **Armour / shell / bark** | Reduces damage | Thickness | Heavy and slow |
| **Spikes / thorns** | Damages attackers and eaters | Size | Build cost |
| **Toxin gland** | Makes tissues toxic; optional venom delivery | Toxin strength | Ongoing cost; eaters can evolve detox enzymes (an arms race) |
| **Signal emitter** | Sound, light or scent emission | Channel, power | Predators can hear it too |
| **Reproductive organ** | Gametes, eggs, seeds, spores, live young | Offspring size and count, dispersal mode | Investment against numbers (r/K emerges) |
| **Fruit** (optional sub-part) | A sugary reward wrapped around seeds | Sugar content | Costs energy; can buy animal dispersal (mutualism) |
| **Grabber** (optional, later) | Carries or places objects | Strength | Enables nest and burrow building (see §13) |

Parts can be **duplicated** by mutation and then diverge. That is the main route
to new complexity (two limbs become four, one eye becomes two).

### 6.3 Genome and development
- **Body genome:** a directed graph of part genes (type, parameters, attachment
  point, symmetry flags, repeat count), in the spirit of Karl Sims' creatures and
  Framsticks. Recursive and symmetric expansion gives segments and limb pairs
  cheaply.
- **Life-history genes:** adult size, growth rate, maturity threshold, offspring
  investment and count, reproduction mode, dormancy triggers, body temperature
  setpoint (endo/ectothermy), DNA repair investment (sets mutation rate and
  ageing).
- **Development:** organisms are born small and grow toward the adult
  plan. Growth costs real energy and nutrients, so a starving juvenile stalls.
  Juveniles are weaker, slower and more vulnerable.
- **Ageing:** not a fixed lifespan. Damage builds up and repair costs energy.
  Lineages trade repair against reproduction, so senescence can evolve.

### 6.4 Body physics (derived from parts)
- Mass is the sum of parts. Volume comes from tissue densities. Surface area is
  roughly k_shape · M^(2/3).
- **Heat:** `P_loss = h · A · (T_body − T_env) / insulation`, plus evaporative
  cooling, which costs water. Big bodies keep heat and small ones lose it, so
  Bergmann's rule and endotherm/ectotherm splits can emerge.
- **Locomotion cost per metre** comes from mass, g, slope, the medium (drag in
  water and air) and appendage efficiency. **Top speed** comes from muscle
  power divided by mass.
- **Flight** works only when wing lift (∝ air density × area × speed²) can carry
  M·g within the available muscle power. High-pressure, low-g worlds should
  evolve flyers more easily.
- **Size limits:** the structural mass needed grows faster than body mass, and
  scales with g.

### 6.5 Metabolism and energy
Each organism tracks internal pools:

| Pool | Notes |
|---|---|
| Gut contents | By component: sugar, starch, protein, fat, fibre, lignin, water, toxins |
| Blood sugar / ATP buffer | Small, fast; this is what everything spends |
| Glycogen / starch store | Medium-size, fast to access |
| Fat store | Large and dense (≈37 kJ/g); slower to mobilise; conversion losses |
| Structural protein (muscle and other tissue) | Burned during starvation, which shrinks parts and weakens the organism |
| Water | Lost by breathing, sweating, transpiration and excretion |
| Body temperature | From the heat balance |
| Damage and toxin load | Repair and detox cost energy |

**Food is the tissue of what was eaten.** A leaf is mostly water and fibre; a
seed is starch and fat; muscle is protein and fat. The energy gained per gram
depends on the composition (carbs ≈17 kJ/g, protein ≈17, fat ≈37, fibre ≈0
without cellulase) times the eater's digestion efficiency for each component.
That is how herbivore and carnivore guts become separate adaptive peaks.

**Energy spent per tick:**
```
E = Σ(part basal rate × mass) × Q10^((T_body − T_ref)/10)   // maintenance
  + locomotion work / muscle efficiency
  + brain cost (§7.4)
  + thermoregulation (heating or cooling to the setpoint)
  + growth + repair + reproduction investment
  + toxin / signal / other active part costs
```
Aerobic power is capped by available O₂ (both atmospheric O₂ and body
size/breathing). Exceeding the cap runs anaerobically, which builds up fatigue.

Sleep or torpor is an actuator: it lowers the metabolic rate and sensor
gain. Hibernation and nocturnality can emerge from it.

### 6.6 Sessile producers (plants and similar)
- Each tick: light captured per leaf layer → gross photosynthesis, limited by
  water, CO₂, temperature and N.
- Allocation genes (possibly with simple feedback such as "grow roots when water
  is short") decide where new mass goes: leaves, wood/height, roots, storage,
  defence or reproduction.
- Competition comes from shading (§4.6) and root overlap within the same cell.
- Reproduction: seeds dispersed by wind (direction from the wind field), by
  gravity, by animals (fruit eaten, seeds passed) or by water. Seeds can stay
  dormant until conditions are right.

### 6.7 Reproduction and inheritance
- Modes (genetic): asexual budding/fission, sexual with separate sexes, or
  hermaphrodite. Sexual reproduction requires two compatible individuals to
  meet, which is either a brain decision or, for sessile organisms, pollen/spore
  dispersal.
- **Recombination:** aligned crossover using historical gene markers (NEAT-style
  innovation numbers) so parts and neurons line up between parents.
- **Mutation:** parameter jitter, add/remove/duplicate part, change attachment,
  brain add-neuron/add-synapse/rewire. The rate is set by the DNA repair gene.
- Parental investment: egg/seed provisioning (energy and matter taken from the
  parent), plus optional post-birth care driven by the brain (feeding the young
  is a learnable action).

### 6.8 Field life
Field life is a grid of populations, not agents. Each cell has biomass per
functional group, and each group has a mean and variance for key traits
(temperature optimum, substrate preference, virulence) that drift under
selection.
- **Decomposers:** break detritus down to nutrients and CO₂.
- **Phytoplankton / algae:** ocean and lake primary production; they need light
  and dissolved nutrients.
- **Pathogens:** infect organisms through contact density, carcasses and water.
  The player can introduce plagues. Population density becomes a real cost and
  immune investment can evolve.

---

## 7. Brains (neural networks)

### 7.1 Architecture
- **Recommendation:** a recurrent neural net with evolvable topology (NEAT-style
  add-node/add-connection mutations with innovation numbers) and continuous-time
  neurons (CTRNN: each neuron has an evolvable time constant). Recurrence gives
  short-term memory and rhythm generators (gaits, circadian behaviour).
- **Lifetime learning:** neuromodulated Hebbian plasticity. Each synapse has
  evolved plasticity coefficients, and learning is gated by modulatory signals
  from internal states: pain, satiety, fear-related arousal, and so on. This is
  how danger awareness should arise: no "fear" is coded, but a pain signal that
  follows a smell can strengthen smell → turn-away pathways.
- Brain size is limited by the neural tissue the body has built (§6.2).

### 7.2 Inputs (only the sensors the body has evolved)
| Source | Neural inputs |
|---|---|
| Eyes | N rays across the field of view, each returning distance, brightness and colour (hue), relative size, relative motion. Low light adds noise. |
| Chemoreceptors | Concentration and left/right gradient per scent channel (food, kin, species markers, decay, blood) |
| Ears | Loudness and direction per frequency band |
| Touch / pain | Contact flags, damage this tick |
| Thermoreceptors | Ambient and body temperature |
| Internal (always available, cheap) | Energy reserve, hydration, gut fullness, fatigue, health, reproductive readiness, age |
| Clock-like signals | Ambient light level (which allows learned day/night rhythms); no free "time of day" input |
| Bias / noise | Constant input and a stochastic exploration input |

### 7.3 Outputs (actuators the body has)
Forward thrust, turn, eat/bite, attack (force), mate/release gametes, emit signal
(per channel), sleep/rest level, defensive posture (armour up, spikes out),
excrete/scent mark, care for offspring, and later grab/drop/place. An output only
does something if the matching part exists and has power.

### 7.4 What thinking costs
```
E_brain = n_neurons · c_neuron + n_synapses · c_synapse + Σ|activity| · c_activity
```
- Plus a one-off build cost for the neural tissue.
- **Brain tick rate** is an evolvable trait: faster reaction time costs more
  energy per second. A slow grazer can think at 2 Hz; a predator might pay for
  10 Hz.
- Calibration target: a large-brained, Earth-like endotherm should spend roughly
  10–25% of its basal budget on its brain. Selection decides whether that is
  worth it.

### 7.5 What we expect to emerge (and will test for)
Food seeking (chemotaxis, then vision), predator avoidance, learned avoidance of
toxic prey, herding (safety in numbers), pack hunting, ambush and camouflage
strategies, migration that follows the seasons, nocturnality, territoriality,
alarm calls and other proto-communication, parental care.

---

## 8. Evolution and ecology tracking

- **Species** are not declared. They are discovered by clustering genomes on
  genetic distance (compatible for crossover = same species), with a stable ID
  assigned when a cluster persists and splits.
- **Phylogeny:** every birth records its parent IDs. A compressed lineage tree
  is kept for the viewer: speciation events, extinctions, the trait at each
  node.
- **Ecology metrics:** population per species, biomass by trophic level (worked
  out from diet), food web (who ate whom, logged), energy flow per level,
  diversity indices, range maps.

---

## 9. Player tools (the "god" layer)

### 9.1 World creation
Star, orbit, rotation, planet size and density, atmosphere composition, ocean
fraction, terrain generator settings, starting life (see §13). Presets: Earth-like,
Mars-ish, tidally locked red-dwarf world, high-gravity super-Earth, ocean world,
snowball.

### 9.2 Live edits (at any time)
- Change any planet parameter while running: push the orbit out, add tilt, thin
  the atmosphere, pump CO₂.
- Terraform brush: raise or lower land, add water, change soil, place ice.
- Local climate overrides: heat or cool a region, change rainfall.
- Life editing: place or remove individuals or species, design an organism in
  a part editor, apply selective pressures (cull a trait).

### 9.3 Disasters (simulated through the same systems, not scripted outcomes)
| Disaster | How it works in the sim |
|---|---|
| Asteroid impact | Crater (terrain change), blast-radius kills, dust in the atmosphere (albedo spike gives an impact winter), wildfires |
| Volcano / supervolcano | Lava (kills, new land, ignites fires), CO₂ and aerosol injection (warming vs. cooling), ash fertilises soil |
| Earthquake / tsunami | Terrain shift; coastal flooding wave |
| Drought / flood | Forced change to precipitation |
| Ice age / heat wave | Forcing on luminosity or greenhouse; real feedbacks then amplify it |
| Plague | Seed a virulent pathogen strain in the field-life layer |
| Solar flare / stellar change | Luminosity or radiation pulse; raised mutation rate |
| Wildfire | Ignite at a point; spreads by fuel, wind and moisture |

### 9.4 Time
Pause, step, speeds from 1× up to "as fast as possible", and a fast-forward
mode that switches to coarser climate (daily means) to get through generations
quickly (§10.3).

### 9.5 Observation
- Map overlays: temperature, precipitation, biomes (classified from what is
  actually growing), nutrients, light, species ranges, population density.
- Organism inspector: body diagram, genome, live brain activity, energy budget
  as a Sankey diagram, stomach contents, lineage.
- Species panel: traits over time, population graph, range.
- Global charts: CO₂/O₂, mean temperature, biomass, species count, phylogenetic
  tree.
- Event log: speciation, extinction, disasters, firsts ("first flyer", "first
  endotherm").

---

## 10. Engine architecture

### 10.1 Stack (recommendation, see §13)
- **Rust** for the simulation core: native speed, safe parallelism (rayon),
  deterministic, and it can compile to WebAssembly later.
- **Viewer:** a separate crate (Bevy, or wgpu + egui) that only reads snapshots
  of sim state. Replacing it never touches the sim.
- Config and presets in **RON or TOML**; saves as a versioned binary
  (bincode/postcard) plus a JSON metadata header.

### 10.2 Workspace layout
```
crates/
  core/        units, time, deterministic RNG streams, config, math (sphere geometry)
  grid/        geodesic hex grid, neighbour tables, field storage, spatial hash
  planet/      astronomy, orbit, insolation, geology, terrain generation
  climate/     energy balance, wind, hydrology, ice, fire
  chemistry/   element pools, biogeochemical cycles, field life
  organism/    genome, development, parts catalog, body physics, metabolism
  brain/       NN representation, evaluation, plasticity, cost accounting
  evolution/   reproduction, recombination, mutation, speciation, phylogeny
  sim/         world state, multi-rate scheduler, disasters, save/load, invariants
  cli/         headless runner, batch experiments, stats export (CSV/Parquet)
  viewer/      rendering and UI
```

### 10.3 Time scales and the multi-rate scheduler
This is the hardest structural problem. Evolution needs thousands of
generations, while behaviour needs fine time steps.

| System | Default step | Fast-forward step |
|---|---|---|
| Astronomy | 1 sim-hour | 1 sim-day |
| Climate / hydrology | 1 sim-hour | 1 sim-day (daily-mean insolation) |
| Chemistry, soil, field life | 1 sim-day | 1 sim-day |
| Sessile organisms | 1 sim-hour | 1 sim-day |
| Motile organisms (body + brain) | Brain-rate dependent, base **behaviour tick** of a few sim-minutes | Same |
| Geology, tectonics | Sim-years | Sim-years |

- **Time compression:** a sim "day" has a configurable number of behaviour
  ticks (for example 240). Organism lifespans are measured in sim-days, so
  small, fast-breeding life gets through generations quickly.
- **Budget estimate:** 50k motile agents × 240 ticks/day × 365 days ≈ 4.4 × 10⁹
  agent-ticks per sim-year. At ~10M agent-ticks/s (multithreaded, small brains),
  that is ~7 wall-minutes per sim-year. Generation times of months to a few
  years then give hundreds of generations per day of running. Batch and headless
  runs should go much faster.
- **Determinism:** a fixed system order, per-entity RNG streams derived from
  (seed, entity id, tick), and parallel stages that write into per-thread buffers
  merged in sorted order.

### 10.4 Data layout
- Environment fields as structure-of-arrays over grid cells, so climate updates
  are cache-friendly and can move to the GPU later.
- Organisms in an ECS or SoA arena with generational IDs. Brains as flattened
  sparse arrays (topologically sorted node list plus edge list) for fast
  per-agent evaluation.
- Optional later: GPU compute for climate fields and batched brain evaluation.

### 10.5 Testing
- Unit tests for every equation (orbit periods, insolation integrals,
  conservation).
- **Invariant checks** (debug builds): mass and energy conservation, no NaNs, no
  negative pools.
- **Scenario tests** (headless, seeded): "Earth preset gives deserts near 30°",
  "ε=0 removes seasonal temperature swing", "removing CO₂ produces a snowball",
  "plants grow taller under competition".
- **Benchmarks** tracked in CI: agent-ticks/s, climate step time.

---

## 11. Roadmap

Each phase ends with a demo and an **emergence check**. That way we find out
early when a system is not producing interesting behaviour.

| Phase | Scope | Exit check |
|---|---|---|
| **0. Foundations** | Workspace, core types, RNG, config, geodesic grid, scheduler, headless runner, minimal globe viewer with overlays | Grid renders; deterministic replay of an empty world |
| **1. Planet & climate** | Orbit, insolation, energy balance, albedo, greenhouse, wind bands, hydrology, rivers, ice | Seasons flip between hemispheres; deserts and rain shadows appear; ice-albedo runaway can be triggered |
| **2. Chemistry & producers** | Element pools, soil, decomposers, sessile organisms with genome and allocation, canopy light, seeds | Vegetation follows climate (forests in wet areas, sparse in deserts); taller plants evolve under shading; CO₂ responds to biomass |
| **3. Motile bodies & metabolism** | Part catalog, development, body physics, digestion, full energy accounting, a hand-coded test controller (debug only) | Energy budgets balance; size/temperature trade-offs show up in tests; starvation and growth work |
| **4. Brains** | NN representation, sensors, actuators, plasticity, brain costs | Evolved agents find food better than random; learned avoidance of a toxic food |
| **5. Evolution & ecology** | Reproduction modes, recombination, mutation, speciation, phylogeny, field-life pathogens | A stable herbivore–predator system that survives 100+ generations; at least one speciation event |
| **6. God tools & disasters** | Live parameter editing, terraforming, disaster suite, presets | An impact winter causes a measurable extinction and recovery |
| **7. Observation & polish** | Inspector, charts, phylogenetic tree, event log, save/load UX | A new player can follow one lineage through a disaster |
| **8. Performance & scale** | Profiling, GPU climate, batched brains, LOD for distant regions | Hits the agent count target in §13 |

---

## 12. Main risks

| Risk | Mitigation |
|---|---|
| **Nothing interesting emerges** (life dies out or stays as one blob) | Start with viable seed organisms; run the per-phase emergence checks; tune costs with batch parameter sweeps in the CLI |
| **Performance** | Headless core, SoA data, multi-rate scheduler, field life for microbes, benchmarks in CI from Phase 0 |
| **Too many parameters to tune** | Parameters stay physically grounded (real units help sanity-check them); presets; sensitivity sweeps |
| **Scope creep** | Phase gates; anything not in the current phase goes into a backlog |
| **Evolution is too slow to watch** | Time compression, fast-forward mode, small fast-breeding starter life, background headless runs |
| **Hard to tell what's happening** | Observation tools are built alongside each system, not at the end |

---

## 13. Open decisions (need your input)

1. **Tech stack and platform.** I recommend Rust (core) plus a Bevy or wgpu
   viewer, desktop first. Alternatives: Godot or Unity with C#/C++, or a
   web-first TypeScript + WebGPU build. Do you have a preference or a target
   (PC, web)?
2. **World presentation.** I recommend a 3D globe you can zoom into a local
   surface view. A flat 2D map is simpler to build first, but seasons by
   latitude and polar effects need the sphere underneath anyway.
3. **Scale and hardware.** What should one session handle: about 10k, 50k or
   100k+ motile organisms? Should we assume a decent GPU?
4. **How life starts.** Seed simple viable organisms (a photosynthetic blob plus
   a simple grazer), design your own starters, or something closer to
   abiogenesis (much slower, and much riskier for anything emerging)?
5. **What "construct" means.** Body building blocks only (as planned), or should
   life also build things in the world: nests, burrows, dams, termite-style
   mounds? World construction is a big addition and would fit after Phase 5.
6. **Microbes as fields.** Is it acceptable that microbes, plankton and pathogens
   are simulated as per-cell populations instead of individuals? This is a large
   performance saving.
7. **Time model.** Is the time compression in §10.3 acceptable (behaviour ticks
   of a few sim-minutes, lifespans of sim-days to years)?
