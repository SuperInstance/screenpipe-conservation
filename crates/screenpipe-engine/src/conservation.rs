// screenpipe — AI that knows everything you've seen, said, or heard
// https://screenpi.pe
// This module integrates conservation-checker to make screenpipe
// resource-aware: CPU, memory, and recording quality obey one-sided
// conservation laws so the system degrades gracefully instead of crashing.

use conservation_checker::{ConservationChecker, Phase};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn, error};

// ── Conservation Thresholds ────────────────────────────────────────────

/// Maximum allowed CPU usage (%) before conservation is violated.
const CPU_MAX_PERCENT: f64 = 15.0;

/// Maximum allowed memory usage (MB) before conservation is violated.
const MEMORY_MAX_MB: f64 = 500.0;

/// CPU tolerance: momentary spikes up to 20% are acceptable before flagging.
const CPU_TOLERANCE: f64 = 5.0;

/// Memory tolerance: overshoot up to 50 MB is OK before flagging.
const MEMORY_TOLERANCE: f64 = 50.0;

/// How often (in snapshots) to check whether we can restore quality after
/// a resource strain event.
const RESTORE_CHECK_TICKS: u64 = 6;

// ── Recording Quality Levels ───────────────────────────────────────────

/// Quality tiers for graceful degradation / restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordingQuality {
    /// Full quality: normal capture interval, HD recording, full frame rate
    Full,
    /// Reduced quality: longer capture intervals, skip HD, lower JPEG quality
    Reduced,
    /// Minimal quality: only text/A11Y events, no screen captures, no audio transcription
    Minimal,
    /// Paused: suspend non-essential recording, keep conservation checker alive
    Paused,
}

impl RecordingQuality {
    pub fn label(&self) -> &'static str {
        match self {
            RecordingQuality::Full => "full",
            RecordingQuality::Reduced => "reduced",
            RecordingQuality::Minimal => "minimal",
            RecordingQuality::Paused => "paused",
        }
    }
}

// ── Resource Token ─────────────────────────────────────────────────────

/// Tracks the system's resource pressure state and applies conservation
/// decisions. One-sided conservation means resources can always increase
/// (CPU free, memory free), but decreases in headroom are tracked as
/// potential violations.
#[derive(Debug)]
pub struct ResourceToken {
    /// When we last degraded quality (for cooldown tracking)
    last_degraded_at: Instant,
    /// Current quality level
    quality: RecordingQuality,
    /// Consecutive ticks where resources were conserved (for restore check)
    conserved_ticks: u64,
    /// The underlying conservation checker
    checker: ConservationChecker,
}

impl ResourceToken {
    fn new(checker: ConservationChecker) -> Self {
        Self {
            last_degraded_at: Instant::now(),
            quality: RecordingQuality::Full,
            conserved_ticks: 0,
            checker,
        }
    }

    /// Decide which quality level to use based on the current conservation
    /// state and phase analysis.
    pub fn decide_quality(&mut self) -> RecordingQuality {
        self.checker.snapshot();

        // Phase detection gives us EARLY warning before violation
        let cpu_phase = self.checker.phase("cpu_headroom");
        let mem_phase = self.checker.phase("mem_headroom");

        // If headroom is increasing, consider restoring quality
        if cpu_phase == Phase::Resolving && mem_phase == Phase::Resolving {
            self.conserved_ticks += 1;
            if self.conserved_ticks >= RESTORE_CHECK_TICKS {
                self.conserved_ticks = 0;
                let new = self.restore_from(self.quality);
                if new != self.quality {
                    info!(
                        "resource conservation: restoring quality from {:?} to {:?} (headroom recovering)",
                        self.quality, new
                    );
                    self.quality = new;
                }
                return self.quality;
            }
        } else {
            self.conserved_ticks = 0;
        }

        // Check for active violations
        let violations = self.checker.violations();
        let cpu_violated = violations.contains(&"cpu_headroom".to_string());
        let mem_violated = violations.contains(&"mem_headroom".to_string());
        let transitioning = cpu_phase == Phase::Transitioning
            || mem_phase == Phase::Transitioning
            || cpu_phase == Phase::PreTransition
            || mem_phase == Phase::PreTransition;

        let new = if cpu_violated || mem_violated {
            // Both violated = minimal. One violated = reduced.
            if cpu_violated && mem_violated {
                RecordingQuality::Minimal
            } else {
                RecordingQuality::Reduced
            }
        } else if transitioning {
            // PreTransition / Transitioning without violation yet = reduced
            // (anticipatory degradation)
            RecordingQuality::Reduced
        } else {
            self.quality // stay at current
        };

        if new != self.quality {
            info!(
                "resource conservation: degrading quality from {:?} to {:?} \
                 (cpu_violated={}, mem_violated={}, cpu_phase={}, mem_phase={})",
                self.quality, new, cpu_violated, mem_violated, cpu_phase, mem_phase
            );
            self.last_degraded_at = Instant::now();
        }
        self.quality = new
    }

    /// Step back one quality tier from the current degraded state.
    fn restore_from(&self, current: RecordingQuality) -> RecordingQuality {
        match current {
            RecordingQuality::Paused => RecordingQuality::Minimal,
            RecordingQuality::Minimal => RecordingQuality::Reduced,
            RecordingQuality::Reduced => RecordingQuality::Full,
            RecordingQuality::Full => RecordingQuality::Full,
        }
    }

    /// Whether recording should be paused.
    pub fn is_paused(&self) -> bool {
        self.quality == RecordingQuality::Paused
    }

    /// Current quality level.
    pub fn quality(&self) -> RecordingQuality {
        self.quality
    }
}

// ── Shared Conservation State ─────────────────────────────────────────

/// Thread-safe shared state for the conservation checker, accessible from
/// the resource monitor (metrics collection) and the capture pipeline
/// (quality decisions).
pub struct ConservationState {
    /// The token, protected by a mutex for infrequent access.
    token: std::sync::Mutex<ResourceToken>,
    /// Atomic flags for fast-path checks in hot loops.
    quality: AtomicU32, // 0=Full, 1=Reduced, 2=Minimal, 3=Paused
    /// Whether we detected a resource violation this cycle
    has_violation: AtomicBool,
    /// Last tick when we logged summary
    last_log: std::sync::Mutex<Instant>,
}

impl ConservationState {
    pub fn new() -> Arc<Self> {
        let mut checker = ConservationChecker::new();
        checker.register("cpu_headroom", CPU_MAX_PERCENT, CPU_TOLERANCE);
        checker.register("mem_headroom", MEMORY_MAX_MB, MEMORY_TOLERANCE);

        Arc::new(Self {
            token: std::sync::Mutex::new(ResourceToken::new(checker)),
            quality: AtomicU32::new(0), // Full
            has_violation: AtomicBool::new(false),
            last_log: std::sync::Mutex::new(Instant::now()),
        })
    }

    /// Feed the latest CPU% and memory MB into the conservation checker.
    ///
    /// Conservation is *one-sided*: we track "headroom" — the gap between
    /// current usage and the max boundary. When headroom decreases, we
    /// detect potential violations. When it increases, all is well.
    pub fn update_metrics(&self, cpu_percent: f64, mem_mb: f64) {
        let cpu_headroom = (CPU_MAX_PERCENT - cpu_percent).max(0.0);
        let mem_headroom = (MEMORY_MAX_MB - mem_mb).max(0.0);

        let mut token = self.token.lock().unwrap();
        token.checker.update("cpu_headroom", cpu_headroom);
        token.checker.update("mem_headroom", mem_headroom);

        let new_quality = token.decide_quality();
        let qval = new_quality as u32;
        self.quality.store(qval, Ordering::Relaxed);
        self.has_violation
            .store(!token.checker.violations().is_empty(), Ordering::Relaxed);

        // Periodic logging
        let now = Instant::now();
        let should_log = {
            let mut last = self.last_log.lock().unwrap();
            if now.duration_since(*last) > Duration::from_secs(60) {
                *last = now;
                true
            } else {
                false
            }
        };

        if should_log {
            let cpu_phase = token.checker.phase("cpu_headroom");
            let mem_phase = token.checker.phase("mem_headroom");
            info!(
                "resource conservation: cpu={:.1}% cpu_headroom={:.1} cpu_phase={} \
                 mem={:.1}MB mem_headroom={:.1} mem_phase={} quality={} violations={:?}",
                cpu_percent,
                cpu_headroom,
                cpu_phase,
                mem_mb,
                mem_headroom,
                mem_phase,
                new_quality.label(),
                token.checker.violations(),
            );
        }
    }

    /// Fast-path: check whether quality has been degraded.
    pub fn is_degraded(&self) -> bool {
        self.quality.load(Ordering::Relaxed) > 0
    }

    /// Fast-path: check whether we're in minimal or paused mode.
    pub fn is_critical(&self) -> bool {
        self.quality.load(Ordering::Relaxed) >= 2
    }

    /// Get the recommended capture interval multiplier based on quality.
    /// Full = 1x, Reduced = 2x, Minimal = 5x, Paused = unlimited (return None).
    pub fn capture_interval_multiplier(&self) -> Option<u64> {
        match self.quality.load(Ordering::Relaxed) {
            0 => Some(1),  // Full
            1 => Some(2),  // Reduced
            2 => Some(5),  // Minimal
            3 => None,     // Paused — caller should skip captures
            _ => Some(1),
        }
    }

    /// Whether to skip HD recording.
    pub fn skip_hd_recording(&self) -> bool {
        self.quality.load(Ordering::Relaxed) >= 1 // Reduced or worse
    }

    /// Get JPEG quality reduction factor (0.0-1.0).
    /// Full = 1.0, Reduced = 0.7, Minimal = 0.4
    pub fn jpeg_quality_factor(&self) -> f64 {
        match self.quality.load(Ordering::Relaxed) {
            0 => 1.0,
            1 => 0.7,
            2 => 0.4,
            3 => 0.0,
            _ => 1.0,
        }
    }

    /// Whether to skip audio transcription.
    pub fn skip_transcription(&self) -> bool {
        self.quality.load(Ordering::Relaxed) >= 2 // Minimal or worse
    }
}

// ── High-level Conservation Monitor ────────────────────────────────────

/// Spawns a background task that periodically reads system resource usage
/// and feeds it into the conservation checker.
pub fn start_conservation_monitor(
    state: Arc<ConservationState>,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut sys = sysinfo::System::new_all();
        let pid = std::process::id();

        loop {
            tokio::time::sleep(interval).await;

            sys.refresh_cpu();
            sys.refresh_processes();
            sys.refresh_memory();

            let mut total_cpu = 0.0_f32;
            let mut total_mem_mb = 0.0_f64;

            // Sum up our process tree (main + children)
            let pid_sysinfo = sysinfo::Pid::from_u32(pid);
            if let Some(process) = sys.process(pid_sysinfo) {
                total_cpu += process.cpu_usage();
                total_mem_mb += process.memory() as f64 / (1024.0 * 1024.0);

                for child in sys.processes().values() {
                    if child.parent() == Some(pid_sysinfo) {
                        total_cpu += child.cpu_usage();
                        total_mem_mb += child.memory() as f64 / (1024.0 * 1024.0);
                    }
                }
            }

            state.update_metrics(total_cpu as f64, total_mem_mb);

            if state.has_violation.load(Ordering::Relaxed) {
                debug!(
                    "conservation: resource violation active — cpu={:.1}% mem={:.1}MB",
                    total_cpu, total_mem_mb
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_headroom_one_sided() {
        let mut checker = ConservationChecker::new();
        checker.register("cpu_headroom", 15.0, 5.0);

        // Start at full headroom
        assert!(checker.is_conserved("cpu_headroom"));
        assert_eq!(checker.current_value("cpu_headroom"), 15.0);

        // Using 10% CPU → 5% headroom — still conserved (within 5% tolerance)
        checker.update("cpu_headroom", 5.0);
        checker.snapshot();
        assert!(checker.is_conserved("cpu_headroom"));

        // Using 12% CPU → 3% headroom — still within tolerance from 15
        checker.update("cpu_headroom", 3.0);
        checker.snapshot();
        assert!(checker.is_conserved("cpu_headroom"));

        // Using 15% CPU — violated
        checker.update("cpu_headroom", 0.0);
        checker.snapshot();
        assert!(!checker.is_conserved("cpu_headroom"));
    }

    #[test]
    fn test_quality_decision_chain() {
        let state = ConservationState::new();
        assert_eq!(state.capture_interval_multiplier(), Some(1));
        assert!(!state.skip_hd_recording());
        assert!((state.jpeg_quality_factor() - 1.0).abs() < 0.01);

        // Simulate high CPU → reduced quality
        state.update_metrics(14.0, 100.0); // near CPU limit
        // It may or may not degrade based on phase — depends on snapshots

        // Now push over both thresholds
        state.update_metrics(20.0, 600.0); // violated
        assert!(state.is_degraded());
        assert!(state.is_critical());
        assert!(state.skip_hd_recording());
        assert!(state.skip_transcription());
        assert!(state.jpeg_quality_factor() < 0.5);
    }

    #[test]
    fn test_recovery_after_degradation() {
        let state = ConservationState::new();

        // Violate
        state.update_metrics(20.0, 600.0);
        assert!(state.is_degraded());

        // Recover — need RESTORE_CHECK_TICKS snapshots of resolving
        state.update_metrics(5.0, 200.0);

        // Feed several good ticks to trigger restore
        for _ in 0..RESTORE_CHECK_TICKS + 1 {
            state.update_metrics(5.0, 200.0);
        }

        // Should eventually restore
        assert!(!state.is_critical(), "should restore from critical");
    }

    #[test]
    fn test_register_and_violations() {
        let mut checker = ConservationChecker::new();
        checker.register("mem_headroom", 500.0, 50.0);

        assert!(checker.is_conserved("mem_headroom"));

        // Using 480 MB → 20 MB headroom — violates (500-50=450, current=20 < 450)
        checker.update("mem_headroom", 20.0);
        assert!(!checker.is_conserved("mem_headroom"));

        let violations = checker.violations();
        assert!(violations.contains(&"mem_headroom".to_string()));
    }
}
