//! Cross-core coordination for a fail-safe, host-requested MCU reboot.

use core::sync::atomic::{AtomicU32, Ordering};

const IDLE: u32 = 0;
const REQUESTED: u32 = 1;
const QUIESCED: u32 = 2;

static STATE: AtomicU32 = AtomicU32::new(IDLE);

/// Ask core 1 to put experiment outputs into their reboot-safe state.
///
/// Repeated requests are intentionally idempotent, allowing a host or broker
/// to retry if the first response is lost before the ROM reset is scheduled.
pub fn request() {
    let _ = STATE.compare_exchange(IDLE, REQUESTED, Ordering::Release, Ordering::Relaxed);
}

#[inline(always)]
#[unsafe(link_section = ".data.ram_func")]
pub fn is_requested() -> bool {
    STATE.load(Ordering::Acquire) >= REQUESTED
}

#[inline(always)]
#[unsafe(link_section = ".data.ram_func")]
pub fn mark_quiesced() {
    STATE.store(QUIESCED, Ordering::Release);
}

pub fn is_quiesced() -> bool {
    STATE.load(Ordering::Acquire) == QUIESCED
}
