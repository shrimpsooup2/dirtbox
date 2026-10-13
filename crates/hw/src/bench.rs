//! Calibration benchmark: how many organism steps per second this machine can
//! sustain across all cores.
//!
//! **Proxy workload.** The organism and brain crates don't exist yet (Phases
//! 4–5), so this times a stand-in with the same shape as a real organism step:
//! - sense neighbours through a spatial hash (eight vision sectors),
//! - integrate a 50-neuron, 240-synapse CTRNN with its own weights per agent,
//! - move and pay an energy cost.
//!
//! Agents read the previous step's positions and write the next ones, which is
//! the double-buffered pattern the real stepper will use. Once real organisms
//! exist, this module will time those instead and the proxy will go away.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Barrier, Mutex, RwLock};
use std::time::{Duration, Instant};

use dirtbox_core::Rng;

const INPUTS: usize = 24;
const HIDDEN: usize = 16;
const OUTPUTS: usize = 10;
const NEURONS: usize = INPUTS + HIDDEN + OUTPUTS;
const SYNAPSES: usize = 240;
const SECTORS: usize = 8;
/// Sensing radius, m.
const SENSE_RADIUS: f32 = 10.0;
/// Average neighbours within the sensing radius; sets the world density.
const MEAN_NEIGHBOURS: f32 = 8.0;
/// Step length, s.
const DT: f32 = 0.5;
const RNG_DOMAIN: u64 = 0xBE_4C;

#[derive(Clone, Debug)]
pub struct BenchConfig {
    pub agents: usize,
    pub threads: usize,
    pub measure: Duration,
    pub seed: u64,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            agents: 20_000,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            measure: Duration::from_secs(2),
            seed: 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BenchResult {
    pub threads: usize,
    pub agents: usize,
    pub agent_steps: u64,
    pub seconds: f64,
}

impl BenchResult {
    pub fn steps_per_second(&self) -> f64 {
        self.agent_steps as f64 / self.seconds
    }
}

struct Brain {
    src: [u16; SYNAPSES],
    dst: [u16; SYNAPSES],
    weight: [f32; SYNAPSES],
    bias: [f32; NEURONS],
    inv_tau: [f32; NEURONS],
    state: [f32; NEURONS],
}

impl Brain {
    fn random(rng: &mut Rng) -> Box<Self> {
        let mut b = Box::new(Brain {
            src: [0; SYNAPSES],
            dst: [0; SYNAPSES],
            weight: [0.0; SYNAPSES],
            bias: [0.0; NEURONS],
            inv_tau: [0.0; NEURONS],
            state: [0.0; NEURONS],
        });
        for s in 0..SYNAPSES {
            b.src[s] = rng.below(NEURONS as u64) as u16;
            // Synapses feed hidden and output neurons only.
            b.dst[s] = (INPUTS as u64 + rng.below((HIDDEN + OUTPUTS) as u64)) as u16;
            b.weight[s] = rng.normal() as f32 * 0.5;
        }
        for n in 0..NEURONS {
            b.bias[n] = rng.normal() as f32 * 0.2;
            // τ between 0.5 s and 5 s; keep DT·(1/τ) ≤ 1 for a stable Euler step.
            b.inv_tau[n] = 1.0 / rng.range_f64(0.5, 5.0) as f32;
        }
        b
    }

    /// One Euler step of the CTRNN. Returns (thrust, turn) in [−1, 1].
    fn step(&mut self, inputs: &[f32; INPUTS]) -> (f32, f32) {
        let act: [f32; NEURONS] = std::array::from_fn(|n| squash(self.state[n] + self.bias[n]));
        let mut sum = [0.0f32; NEURONS];
        sum[..INPUTS].copy_from_slice(inputs);
        for s in 0..SYNAPSES {
            sum[self.dst[s] as usize] += self.weight[s] * act[self.src[s] as usize];
        }
        for ((y, &inv_tau), &input) in self.state.iter_mut().zip(&self.inv_tau).zip(&sum) {
            *y += DT * inv_tau * (input - *y);
        }
        let out = INPUTS + HIDDEN;
        (
            squash(self.state[out] + self.bias[out]),
            squash(self.state[out + 1] + self.bias[out + 1]),
        )
    }
}

/// Cheap sigmoid-like squashing into (−1, 1).
fn squash(x: f32) -> f32 {
    x / (1.0 + x.abs())
}

/// Positions and a uniform-grid spatial hash over a square torus.
struct Front {
    side: f32,
    cols: usize,
    x: Vec<f32>,
    y: Vec<f32>,
    heading: Vec<f32>,
    energy: Vec<f32>,
    cell_start: Vec<u32>,
    cell_items: Vec<u32>,
}

impl Front {
    fn cell_of(&self, x: f32, y: f32) -> (usize, usize) {
        let scale = self.cols as f32 / self.side;
        (
            ((x * scale) as usize).min(self.cols - 1),
            ((y * scale) as usize).min(self.cols - 1),
        )
    }

    fn rebuild_hash(&mut self) {
        let cells = self.cols * self.cols;
        self.cell_start.clear();
        self.cell_start.resize(cells + 1, 0);
        let mut cell_of_agent = Vec::with_capacity(self.x.len());
        for i in 0..self.x.len() {
            let (cx, cy) = self.cell_of(self.x[i], self.y[i]);
            let c = cy * self.cols + cx;
            cell_of_agent.push(c);
            self.cell_start[c + 1] += 1;
        }
        for c in 0..cells {
            self.cell_start[c + 1] += self.cell_start[c];
        }
        let mut fill = self.cell_start.clone();
        self.cell_items.resize(self.x.len(), 0);
        for (i, &c) in cell_of_agent.iter().enumerate() {
            self.cell_items[fill[c] as usize] = i as u32;
            fill[c] += 1;
        }
    }

    /// Sensor inputs for agent `i`: per-sector closeness and relative heading
    /// of the nearest neighbour, plus a few internal-state inputs.
    fn sense(&self, i: usize) -> [f32; INPUTS] {
        let mut nearest = [SENSE_RADIUS; SECTORS];
        let mut their_heading = [0.0f32; SECTORS];
        let mut count = 0u32;
        let (x, y, h) = (self.x[i], self.y[i], self.heading[i]);
        let (cx, cy) = self.cell_of(x, y);
        let r2 = SENSE_RADIUS * SENSE_RADIUS;
        let half = self.side / 2.0;
        for dy in [self.cols - 1, 0, 1] {
            for dx in [self.cols - 1, 0, 1] {
                let c = ((cy + dy) % self.cols) * self.cols + (cx + dx) % self.cols;
                let items =
                    &self.cell_items[self.cell_start[c] as usize..self.cell_start[c + 1] as usize];
                for &j in items {
                    let j = j as usize;
                    if j == i {
                        continue;
                    }
                    let mut ox = self.x[j] - x;
                    let mut oy = self.y[j] - y;
                    // Wrap to the nearest image on the torus.
                    if ox > half {
                        ox -= self.side;
                    } else if ox < -half {
                        ox += self.side;
                    }
                    if oy > half {
                        oy -= self.side;
                    } else if oy < -half {
                        oy += self.side;
                    }
                    let d2 = ox * ox + oy * oy;
                    if d2 >= r2 {
                        continue;
                    }
                    count += 1;
                    let rel = (oy.atan2(ox) - h).rem_euclid(std::f32::consts::TAU);
                    let s = ((rel / std::f32::consts::TAU) * SECTORS as f32) as usize % SECTORS;
                    let d = d2.sqrt();
                    if d < nearest[s] {
                        nearest[s] = d;
                        their_heading[s] = (self.heading[j] - h).cos();
                    }
                }
            }
        }
        let mut inputs = [0.0f32; INPUTS];
        for s in 0..SECTORS {
            inputs[s] = 1.0 - nearest[s] / SENSE_RADIUS;
            inputs[SECTORS + s] = their_heading[s];
        }
        inputs[2 * SECTORS] = count as f32 / 16.0;
        inputs[2 * SECTORS + 1] = self.energy[i];
        inputs[2 * SECTORS + 2] = 1.0; // bias
        inputs
    }
}

/// Output buffer for one worker's contiguous range of agents.
struct ChunkOut {
    x: Vec<f32>,
    y: Vec<f32>,
    heading: Vec<f32>,
    energy: Vec<f32>,
}

pub fn run_benchmark(cfg: &BenchConfig) -> BenchResult {
    assert!(cfg.agents > 0 && cfg.threads > 0);
    let n = cfg.agents;
    let threads = cfg.threads.min(n);
    let side =
        (n as f32 * std::f32::consts::PI * SENSE_RADIUS * SENSE_RADIUS / MEAN_NEIGHBOURS).sqrt();
    let cols = ((side / SENSE_RADIUS) as usize).max(3);

    let mut rng = Rng::stream(cfg.seed, RNG_DOMAIN, 0, 0);
    let mut front = Front {
        side,
        cols,
        x: (0..n)
            .map(|_| rng.range_f64(0.0, side as f64) as f32)
            .collect(),
        y: (0..n)
            .map(|_| rng.range_f64(0.0, side as f64) as f32)
            .collect(),
        heading: (0..n)
            .map(|_| rng.range_f64(0.0, std::f64::consts::TAU) as f32)
            .collect(),
        energy: vec![1.0; n],
        cell_start: Vec::new(),
        cell_items: Vec::new(),
    };
    front.rebuild_hash();

    let ranges: Vec<(usize, usize)> = (0..threads)
        .map(|t| (t * n / threads, (t + 1) * n / threads))
        .collect();
    let outputs: Vec<Mutex<ChunkOut>> = ranges
        .iter()
        .map(|&(a, b)| {
            Mutex::new(ChunkOut {
                x: vec![0.0; b - a],
                y: vec![0.0; b - a],
                heading: vec![0.0; b - a],
                energy: vec![0.0; b - a],
            })
        })
        .collect();
    let shared = RwLock::new(front);
    let barrier = Barrier::new(threads + 1);
    let stop = AtomicBool::new(false);

    let warmup = cfg.measure / 10;
    let mut timed_steps = 0u64;
    let mut seconds = 0.0;

    std::thread::scope(|scope| {
        for (t, &(start, end)) in ranges.iter().enumerate() {
            let (shared, outputs, barrier, stop) = (&shared, &outputs, &barrier, &stop);
            let seed = cfg.seed;
            scope.spawn(move || {
                let mut brains: Vec<Box<Brain>> = (start..end)
                    .map(|i| Brain::random(&mut Rng::stream(seed, RNG_DOMAIN, i as u64 + 1, 0)))
                    .collect();
                loop {
                    barrier.wait();
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    {
                        let front = shared.read().unwrap();
                        let mut out = outputs[t].lock().unwrap();
                        for (k, brain) in brains.iter_mut().enumerate() {
                            let i = start + k;
                            let inputs = front.sense(i);
                            let (thrust, turn) = brain.step(&inputs);
                            let speed = 2.0 * thrust.max(0.0); // m/s
                            let heading = front.heading[i] + turn * DT;
                            out.heading[k] = heading;
                            out.x[k] =
                                (front.x[i] + heading.cos() * speed * DT).rem_euclid(front.side);
                            out.y[k] =
                                (front.y[i] + heading.sin() * speed * DT).rem_euclid(front.side);
                            // Basal + locomotion + brain cost; refill when empty so
                            // the workload stays steady.
                            let cost = (0.002 + 0.01 * speed * speed + 0.001) * DT;
                            let e = front.energy[i] - cost;
                            out.energy[k] = if e <= 0.0 { 1.0 } else { e };
                        }
                    }
                    barrier.wait();
                }
            });
        }

        let begin = Instant::now();
        let mut timing_from: Option<Instant> = None;
        loop {
            barrier.wait(); // workers start the step
            barrier.wait(); // workers finished the step
            {
                let mut front = shared.write().unwrap();
                for (&(start, end), out) in ranges.iter().zip(&outputs) {
                    let out = out.lock().unwrap();
                    front.x[start..end].copy_from_slice(&out.x);
                    front.y[start..end].copy_from_slice(&out.y);
                    front.heading[start..end].copy_from_slice(&out.heading);
                    front.energy[start..end].copy_from_slice(&out.energy);
                }
                front.rebuild_hash();
            }
            match timing_from {
                None if begin.elapsed() >= warmup => timing_from = Some(Instant::now()),
                None => {}
                Some(t0) => {
                    timed_steps += n as u64;
                    if t0.elapsed() >= cfg.measure {
                        seconds = t0.elapsed().as_secs_f64();
                        break;
                    }
                }
            }
        }
        stop.store(true, Ordering::Release);
        barrier.wait(); // release workers so they see `stop`
    });

    BenchResult {
        threads,
        agents: n,
        agent_steps: timed_steps,
        seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_runs_and_counts_steps() {
        let r = run_benchmark(&BenchConfig {
            agents: 500,
            threads: 2,
            measure: Duration::from_millis(100),
            seed: 1,
        });
        assert_eq!(r.agents, 500);
        assert!(r.agent_steps > 0 && r.agent_steps % 500 == 0);
        assert!(r.steps_per_second() > 0.0);
    }

    #[test]
    fn brain_state_stays_bounded() {
        let mut rng = Rng::from_seed(5);
        let mut brain = Brain::random(&mut rng);
        let inputs = [1.0f32; INPUTS];
        for _ in 0..10_000 {
            brain.step(&inputs);
        }
        assert!(brain.state.iter().all(|s| s.is_finite() && s.abs() < 1e3));
    }
}
