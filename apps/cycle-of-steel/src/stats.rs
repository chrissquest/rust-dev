//! Measuring the game as it runs: frame times, simulation step times, memory.
//!
//! Step 8 of the port records these against the Unity build. F3 shows them
//! over the game; `--measure` prints a summary when the game closes. Nothing
//! here allocates per frame: the samples go into fixed rings.

use std::fmt::Write;
use std::time::Duration;

/// Frames kept for the running figures: two seconds at 60 frames a second.
const RING: usize = 120;

/// Times of recent frames, and every frame's for the summary.
pub struct FrameStats {
    /// Seconds between one frame and the next.
    intervals: Ring,
    /// Seconds of work in a frame: everything but waiting for the screen.
    busy: Ring,
    /// Seconds spent stepping the simulation, per step.
    steps: Ring,
    /// Over the whole run, for `--measure`.
    pub frames: u64,
    pub sim_steps: u64,
    all_intervals: Vec<f32>,
    all_busy: Vec<f32>,
    step_seconds: f64,
    /// The longest frame interval and which frame ended it.
    longest: (f32, u64),
    /// From launch to the first frame shown.
    pub load: Option<Duration>,
    /// Whether every frame is kept for the summary.
    keep_all: bool,
}

struct Ring {
    samples: [f32; RING],
    next: usize,
    filled: usize,
}

impl Ring {
    fn new() -> Self {
        Self { samples: [0.0; RING], next: 0, filled: 0 }
    }

    fn push(&mut self, value: f32) {
        self.samples[self.next] = value;
        self.next = (self.next + 1) % RING;
        self.filled = (self.filled + 1).min(RING);
    }

    fn mean(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        self.samples[..self.filled].iter().sum::<f32>() / self.filled as f32
    }

    fn max(&self) -> f32 {
        self.samples[..self.filled].iter().copied().fold(0.0, f32::max)
    }
}

impl FrameStats {
    /// `keep_all`: keep every frame's time for [`FrameStats::summary`], which
    /// grows a list; off unless measuring.
    pub fn new(keep_all: bool) -> Self {
        Self {
            intervals: Ring::new(),
            busy: Ring::new(),
            steps: Ring::new(),
            frames: 0,
            sim_steps: 0,
            all_intervals: Vec::new(),
            all_busy: Vec::new(),
            step_seconds: 0.0,
            longest: (0.0, 0),
            load: None,
            keep_all,
        }
    }

    /// One frame: the seconds since the last and the seconds of work in it.
    pub fn frame(&mut self, interval: f32, busy: f32) {
        self.frames += 1;
        // The first frame's interval is the load, not a frame.
        if self.frames > 1 {
            if interval > self.longest.0 {
                self.longest = (interval, self.frames);
            }
            self.intervals.push(interval);
            if self.keep_all {
                self.all_intervals.push(interval);
            }
        }
        self.busy.push(busy);
        if self.keep_all {
            self.all_busy.push(busy);
        }
    }

    /// `count` simulation steps that took `seconds` together.
    pub fn steps(&mut self, count: u32, seconds: f32) {
        if count == 0 {
            return;
        }
        self.sim_steps += count as u64;
        self.step_seconds += seconds as f64;
        self.steps.push(seconds / count as f32);
    }

    /// The overlay's line: frames a second, frame and work times, step time
    /// and memory.
    pub fn line(&self, out: &mut String) {
        out.clear();
        let interval = self.intervals.mean();
        let fps = if interval > 0.0 { 1.0 / interval } else { 0.0 };
        let _ = write!(
            out,
            "{fps:.0} fps  frame {:.1} max {:.1} ms  work {:.2} ms  step {:.3} ms",
            interval * 1000.0,
            self.intervals.max() * 1000.0,
            self.busy.mean() * 1000.0,
            self.steps.mean() * 1000.0,
        );
        if let Some(memory) = memory() {
            let _ = write!(out, "  ram {:.0} MB", megabytes(memory.resident));
        }
    }

    /// The `--measure` report.
    pub fn summary(&mut self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "measured: {} frames, {} simulation steps", self.frames, self.sim_steps);
        if let Some(load) = self.load {
            let _ = writeln!(out, "  load to first frame  {:.0} ms", load.as_secs_f64() * 1000.0);
        }
        for (name, samples) in [("frame interval", &mut self.all_intervals), ("work per frame", &mut self.all_busy)] {
            if samples.is_empty() {
                continue;
            }
            samples.sort_by(f32::total_cmp);
            let at = |fraction: f32| samples[((samples.len() - 1) as f32 * fraction) as usize] * 1000.0;
            let mean = samples.iter().sum::<f32>() / samples.len() as f32 * 1000.0;
            let _ = writeln!(
                out,
                "  {name:<20} mean {mean:.2} ms, median {:.2}, 99th {:.2}, max {:.2}",
                at(0.5),
                at(0.99),
                at(1.0)
            );
        }
        if self.longest.1 > 0 {
            let _ = writeln!(out, "  longest frame        {:.1} ms, frame {}", self.longest.0 * 1000.0, self.longest.1);
        }
        if self.sim_steps > 0 {
            let _ = writeln!(out, "  simulation step      mean {:.3} ms", self.step_seconds / self.sim_steps as f64 * 1000.0);
        }
        match memory() {
            Some(memory) => {
                let _ = writeln!(
                    out,
                    "  memory               {:.1} MB resident (peak {:.1} MB), {:.1} MB private",
                    megabytes(memory.resident),
                    megabytes(memory.peak_resident),
                    megabytes(memory.private)
                );
            }
            None => out.push_str("  memory               not available here\n"),
        }
        out
    }
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Logs the memory in use at a point of loading, to see what takes it.
pub fn log_memory(point: &str) {
    if let Some(memory) = memory() {
        log::info!("memory after {point}: {:.1} MB resident, {:.1} MB private", megabytes(memory.resident), megabytes(memory.private));
    }
}

/// The process's memory, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// In RAM now: Windows' working set, Linux's and Android's RSS.
    pub resident: u64,
    pub peak_resident: u64,
    /// Memory the process has asked for that no other process shares: Windows'
    /// private bytes; on Linux and Android, resident less what is file-backed
    /// or shared (anonymous RSS).
    pub private: u64,
}

/// The process's memory now, where the system says.
#[cfg(windows)]
pub fn memory() -> Option<Memory> {
    // `PROCESS_MEMORY_COUNTERS` from psapi.h.
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut counters = Counters { cb: size_of::<Counters>() as u32, ..Counters::default() };
    // SAFETY: the pseudo-handle needs no closing, and `counters` is the size
    // it claims.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (ok != 0).then_some(Memory {
        resident: counters.working_set_size as u64,
        peak_resident: counters.peak_working_set_size as u64,
        private: counters.pagefile_usage as u64,
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn memory() -> Option<Memory> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kilobytes = |key: &str| -> Option<u64> {
        let line = status.lines().find(|line| line.starts_with(key))?;
        Some(line[key.len()..].trim().trim_end_matches("kB").trim().parse::<u64>().ok()? * 1024)
    };
    Some(Memory {
        resident: kilobytes("VmRSS:")?,
        peak_resident: kilobytes("VmHWM:")?,
        private: kilobytes("RssAnon:").unwrap_or(0),
    })
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub fn memory() -> Option<Memory> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_figures_and_the_summary_add_up() {
        let mut stats = FrameStats::new(true);
        stats.frame(0.5, 0.001);
        for _ in 0..10 {
            stats.frame(0.016, 0.004);
        }
        stats.frame(0.033, 0.004);
        stats.steps(2, 0.0002);
        let mut line = String::new();
        stats.line(&mut line);
        assert!(line.starts_with("57 fps"), "{line}");
        assert!(line.contains("max 33.0 ms") && line.contains("step 0.100 ms"), "{line}");
        let summary = stats.summary();
        assert!(summary.contains("12 frames, 2 simulation steps"), "{summary}");
        assert!(summary.contains("median 16.00, 99th 16.00, max 33.00"), "{summary}");
    }

    #[test]
    fn this_process_reports_its_memory() {
        let memory = memory().expect("Windows, Linux and Android all report it");
        assert!(memory.resident > 1024 * 1024 && memory.peak_resident >= memory.resident);
    }
}
