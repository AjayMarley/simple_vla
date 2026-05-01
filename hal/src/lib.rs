//! HAL — Hardware Abstraction Layer
//!
//! # Safety Contract
//! - `no_std`: zero heap allocations anywhere in this crate.
//! - `tick()` is called at 1 kHz from a dedicated RT thread; it MUST NOT block.
//! - Any fallible path must resolve to a `SafetyState` transition, never a panic.
//! - `unsafe` is banned (`#![deny(unsafe_code)]`); document the rare exception here.

#![no_std]
#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(clippy::unwrap_used)]   // .unwrap() is a panic — blocked by persona
#![deny(clippy::expect_used)]   // .expect() likewise

use core::sync::atomic::{AtomicU8, Ordering};
use zerocopy::{AsBytes, FromBytes, FromZeroes};

// ── Safety States ─────────────────────────────────────────────────────────────

/// All actuators must be in exactly one of these states at all times.
///
/// Transition rules (monotonically increasing severity — no "downgrade" allowed
/// without an explicit hardware-confirmed reset handshake):
///   Nominal → HoldPosition → EStop
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SafetyState {
    Nominal      = 0,
    HoldPosition = 1,
    EStop        = 2,
}

impl SafetyState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Nominal,
            1 => Self::HoldPosition,
            _ => Self::EStop,
        }
    }
}

// ── Wire Types ────────────────────────────────────────────────────────────────

/// Joint command transmitted at 1 kHz. Repr(C) + zerocopy = safe DMA mapping.
///
/// AUDITOR NOTE: All fields are plain numerics — no padding, no pointers.
/// `AsBytes`/`FromBytes` are safe to derive here.
#[derive(Debug, Clone, Copy, AsBytes, FromZeroes, FromBytes)]
#[repr(C)]
pub struct JointCommand {
    pub position_rad:  f32,
    pub velocity_rad_s: f32,
    pub torque_nm:     f32,
}

/// Sensor feedback — read from hardware registers each tick.
#[derive(Debug, Clone, Copy, AsBytes, FromZeroes, FromBytes)]
#[repr(C)]
pub struct JointFeedback {
    pub position_rad:  f32,
    pub velocity_rad_s: f32,
    pub torque_nm:     f32,
    pub temperature_c: f32,
}

// ── HAL ───────────────────────────────────────────────────────────────────────

/// Singleton HAL handle. Intended to be placed in a `static`.
///
/// ```rust
/// static HAL: Hal = Hal::new();
/// ```
pub struct Hal {
    // AtomicU8 encodes SafetyState — lock-free read/write across threads.
    safety: AtomicU8,
}

impl Hal {
    pub const fn new() -> Self {
        Self {
            safety: AtomicU8::new(SafetyState::Nominal as u8),
        }
    }

    /// 1 kHz control tick. **Zero allocations. No blocking.**
    ///
    /// # Errors
    /// Returns `Err(SafetyState)` if the system is not in `Nominal`; the
    /// caller must NOT send actuator commands and must propagate the state.
    pub fn tick(&self, cmd: &JointCommand) -> Result<JointFeedback, SafetyState> {
        match SafetyState::from_u8(self.safety.load(Ordering::Acquire)) {
            SafetyState::Nominal => {
                // ── BSP INTEGRATION POINT ──────────────────────────────────
                // Replace with: write cmd bytes to memory-mapped register bank.
                // `cmd.as_bytes()` gives a &[u8] view — zero-copy, no alloc.
                let _ = cmd.as_bytes();

                // Replace with: read feedback from ADC / encoder registers.
                Ok(JointFeedback {
                    position_rad:   0.0,
                    velocity_rad_s: 0.0,
                    torque_nm:      0.0,
                    temperature_c:  0.0,
                })
            }
            state => Err(state),
        }
    }

    /// Transition to HoldPosition. Idempotent, lock-free.
    /// Call this when a recoverable fault is detected (e.g., comm timeout).
    pub fn hold(&self) {
        // Only escalate — never downgrade safety level.
        let _ = self.safety.compare_exchange(
            SafetyState::Nominal as u8,
            SafetyState::HoldPosition as u8,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }

    /// Hard E-Stop. Irreversible without external reset. Lock-free.
    /// Call this on any unrecoverable fault (e.g., joint temperature limit).
    pub fn estop(&self) {
        self.safety.store(SafetyState::EStop as u8, Ordering::Release);
    }

    pub fn safety_state(&self) -> SafetyState {
        SafetyState::from_u8(self.safety.load(Ordering::Acquire))
    }
}

// ── Allocation audit ──────────────────────────────────────────────────────────
// Run: RUSTFLAGS="-Z print-all-candidates" cargo build --release -p hal
// to confirm no alloc symbols are linked.
