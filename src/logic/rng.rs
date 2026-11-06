//! Deterministic xorshift32 RNG.
//!
//! The RNG state lives in `Game::rng_state` (part of the public contract) so
//! that the whole simulation state — including randomness — is a plain value
//! that can be cloned/compared. Only used for frightened ghost direction
//! choices, keeping every other part of the simulation fully table-driven.

/// Advance the xorshift32 state and return the new raw value. Seed 0 is
/// disallowed (it is a fixed point); callers must store a non-zero state.
#[inline]
pub fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Uniform-ish value in 0..n (n > 0). Modulo bias is irrelevant for gameplay.
#[inline]
pub fn rng_below(state: &mut u32, n: u32) -> u32 {
    xorshift32(state) % n.max(1)
}

/// Default seed for `Game::new`.
pub const DEFAULT_SEED: u32 = 0x5EED_1234;
