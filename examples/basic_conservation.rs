//! Basic conservation example — run with:
//! cargo run --example basic_conservation -p screenpipe-engine
//!
//! Shows how screenpipe's resource conservation detects resource pressure
//! and degrades recording quality before the system crashes.

use screenpipe_engine::{ConservationState, RecordingQuality};
use std::sync::Arc;
use std::time::Duration;

fn main() {
    println!("═══════════════════════════════════════════");
    println!("  screenpipe Resource Conservation Demo");
    println!("═══════════════════════════════════════════");
    println!();
    println!("This simulates the conservation checker detecting resource");
    println!("pressure and degrading recording quality accordingly.\n");

    let state = ConservationState::new();

    // Phase 1: Normal operation (low resource usage)
    println!("📊 Phase 1: Normal operation");
    println!("   CPU: 3%  | RAM: 200 MB");
    state.update_metrics(3.0, 200.0);
    print_quality(&state);
    println!();

    // Phase 2: Compilation starts (pre-transition)
    println!("📊 Phase 2: Build/compilation starts");
    for (cpu, mem) in &[(7.0, 300.0), (10.0, 400.0), (13.0, 480.0), (14.5, 520.0)] {
        println!("   CPU: {:.0}% | RAM: {:.0} MB", cpu, mem);
        state.update_metrics(*cpu, *mem);
        std::thread::sleep(Duration::from_millis(50));
    }
    print_quality(&state);
    println!();

    // Phase 3: Resource pressure (violation)
    println!("📊 Phase 3: High resource pressure");
    state.update_metrics(25.0, 800.0);
    print_quality(&state);
    println!();

    // Phase 4: Build finishes (recovery)
    println!("📊 Phase 4: Build finished, recovering");
    for (cpu, mem) in &[(8.0, 350.0), (5.0, 250.0), (3.0, 200.0)] {
        println!("   CPU: {:.0}% | RAM: {:.0} MB", cpu, mem);
        state.update_metrics(*cpu, *mem);
        std::thread::sleep(Duration::from_millis(50));
    }
    print_quality(&state);
    println!();

    println!("✅ Conservation demo complete");
    println!();
    println!("Key takeaway: screenpipe anticipates resource pressure");
    println!("and degrades gracefully instead of crashing your system.");
}

fn print_quality(state: &Arc<ConservationState>) {
    let quality = |q: u32| match q {
        0 => "Full    ",
        1 => "Reduced ",
        2 => "Minimal ",
        3 => "Paused  ",
        _ => "Unknown ",
    };

    println!(
        "   → Quality: {} | Degraded: {} | Critical: {} | HD: {} | Transcribe: {}",
        quality(state.raw_quality()),
        state.is_degraded(),
        state.is_critical(),
        state.skip_hd_recording(),
        state.skip_transcription(),
    );
}
