//! Best-effort hardware detection. Anything that cannot be determined is
//! `None`, and the caller asks the player instead of guessing.

use std::fs;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct HardwareInfo {
    pub os: &'static str,
    pub cpu_model: Option<String>,
    pub logical_cores: usize,
    pub total_ram_bytes: Option<u64>,
    /// RAM the simulation can use right now (free + reclaimable, and within
    /// any container memory limit).
    pub available_ram_bytes: Option<u64>,
}

pub fn detect() -> HardwareInfo {
    let logical_cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut info = HardwareInfo {
        os: std::env::consts::OS,
        cpu_model: None,
        logical_cores,
        total_ram_bytes: None,
        available_ram_bytes: None,
    };
    match info.os {
        "linux" => detect_linux(&mut info),
        "macos" => detect_macos(&mut info),
        _ => {}
    }
    info
}

fn detect_linux(info: &mut HardwareInfo) {
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        info.cpu_model = cpuinfo
            .lines()
            .find(|l| l.starts_with("model name"))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string());
    }
    if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
        info.total_ram_bytes = meminfo_kib(&meminfo, "MemTotal:").map(|k| k * 1024);
        info.available_ram_bytes = meminfo_kib(&meminfo, "MemAvailable:").map(|k| k * 1024);
    }
    // Containers often limit memory below what /proc/meminfo reports.
    if let Some(limit) = fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        info.total_ram_bytes = Some(info.total_ram_bytes.map_or(limit, |t| t.min(limit)));
        info.available_ram_bytes = Some(info.available_ram_bytes.map_or(limit, |a| a.min(limit)));
    }
}

fn meminfo_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo
        .lines()
        .find(|l| l.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn detect_macos(info: &mut HardwareInfo) {
    let sysctl = |name: &str| -> Option<String> {
        let out = Command::new("sysctl").args(["-n", name]).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    info.cpu_model = sysctl("machdep.cpu.brand_string");
    info.total_ram_bytes = sysctl("hw.memsize").and_then(|s| s.parse().ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo() {
        let sample = "MemTotal:       16384000 kB\nMemFree:  100 kB\nMemAvailable:   8000000 kB\n";
        assert_eq!(meminfo_kib(sample, "MemTotal:"), Some(16_384_000));
        assert_eq!(meminfo_kib(sample, "MemAvailable:"), Some(8_000_000));
        assert_eq!(meminfo_kib(sample, "SwapTotal:"), None);
    }

    #[test]
    fn detects_at_least_one_core() {
        assert!(detect().logical_cores >= 1);
    }
}
