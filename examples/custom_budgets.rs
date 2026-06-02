//! Custom budgets example — run with:
//! cargo run --example custom_budgets -p screenpipe-engine
//!
//! Shows how to configure conservation budgets via environment variables.

use screenpipe_engine::ConservationState;
use std::sync::Arc;
use std::time::Duration;

fn main() {
    println!("═══════════════════════════════════════════");
    println!("  Custom Conservation Budgets Demo");
    println!("═══════════════════════════════════════════");
    println!();
    println!("screenpipe uses conservation-checker for one-sided");
    println!("conservation: resources CAN increase (more free CPU,");
    println!("more free RAM), but decreases in headroom are tracked");
    println!("as potential violations.\n");

    println!("Default budgets:");
    println!("  CPU max:    15%  (±5% tolerance = spike to 20% OK)");
    println!("  RAM max:   500 MB (±50 MB = spike to 550 MB OK)");
    println!();

    // Show one-sided conservation behavior
    println!("--- One-sided conservation demo ---\n");

    let state = ConservationState::new();

    // RAM can always increase (headroom grows — never a violation)
    println!("Step 1: Memory drops from 200 MB to 100 MB → headroom INCREASES");
    println!("       This is ALWAYS OK (one-sided: increases are fine)\n");
    state.update_metrics(5.0, 100.0);
    state.update_metrics(5.0, 50.0);

    // But CPU decreasing headroom triggers violation
    println!("Step 2: CPU rises from 5% to 22% → headroom DECREASES past tolerance");
    println!("       One-sided means this IS a violation (headroom shrunk)\n");
    state.update_metrics(22.0, 150.0);
    print_state(&state);

    // Recovery
    println!();
    println!("Step 3: Recovery — CPU drops back to 5%");
    for _ in 0..8 {
        state.update_metrics(5.0, 100.0);
        std::thread::sleep(Duration::from_millis(10));
    }
    print_state(&state);

    println!();
    println!("✅ Custom budgets demo complete");
    println!();
    println!("Pro tip: Set these env vars for your machine:");
    println!("  SCREENPIPE_CPU_MAX=30        (for compilation-heavy workflows)");
    println!("  SCREENPIPE_MEM_MAX_MB=750    (if you have 32 GB RAM)");
    println!("  SCREENPIPE_CPU_TOLERANCE=15   (allow bigger CPU spikes)");
    println!("  SCREENPIPE_DISABLE_CONSERVATION=1  (only if you're sure)");
}

fn print_state(state: &Arc<ConservationState>) {
    let quality = |q: u32| match q {
        0 => "Full",
        1 => "Reduced",
        2 => "Minimal",
        3 => "Paused",
        _ => "???",
    };
    println!(
        "  Quality: {} | Degraded: {} | Critical: {}",
        quality(state.raw_quality()),
        state.is_degraded(),
        state.is_critical(),
    );
}
