//! Optional frame-time statistics, enabled with `FOTARO_PERF=1`.
//! Logs every 2 seconds: frames drawn, average / worst CPU time per frame,
//! and how many frames missed a 60 Hz budget.

use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(2);
const BUDGET_MS: f32 = 16.7;

pub struct FrameStats {
    enabled: bool,
    start: Instant,
    frames: u32,
    total_ms: f32,
    max_ms: f32,
    slow: u32,
}

impl FrameStats {
    pub fn from_env() -> Self {
        FrameStats {
            enabled: std::env::var_os("FOTARO_PERF").is_some(),
            start: Instant::now(),
            frames: 0,
            total_ms: 0.0,
            max_ms: 0.0,
            slow: 0,
        }
    }

    /// Record one frame's CPU time (update + tessellation), in seconds.
    pub fn record(&mut self, cpu_seconds: Option<f32>) {
        if !self.enabled {
            return;
        }
        if let Some(s) = cpu_seconds {
            let ms = s * 1000.0;
            self.frames += 1;
            self.total_ms += ms;
            self.max_ms = self.max_ms.max(ms);
            self.slow += u32::from(ms > BUDGET_MS);
        }
        if self.start.elapsed() >= WINDOW {
            if self.frames > 0 {
                log::info!(
                    "perf: {} frames, avg {:.1} ms, max {:.1} ms, {} over {BUDGET_MS} ms",
                    self.frames,
                    self.total_ms / self.frames as f32,
                    self.max_ms,
                    self.slow
                );
            }
            *self = FrameStats { enabled: true, start: Instant::now(), ..FrameStats::from_env() };
        }
    }
}
