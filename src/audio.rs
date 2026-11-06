// Presentation layer: procedural audio. Worker B owns this file.
//
// All sounds are synthesized in code (small PCM waveforms wrapped in a
// generated WAV container) and played through macroquad's audio module.
// NOTE: macroquad's `audio` feature IS enabled (Cargo.toml: `features =
// ["audio"]`, backed by quad-snd). Sound synthesis/loading failures are
// logged to stderr and degrade to silence — they never panic or block.
// The game loop never waits on audio: everything in this module is
// fire-and-forget.
use crate::logic::SoundEvent;
use macroquad::audio::{
    load_sound_from_bytes, play_sound, stop_sound, PlaySoundParams, Sound,
};

const SAMPLE_RATE: f32 = 44100.0;
/// Siren variants (Siren0..Siren3). Siren0 doubles as the fright sound
/// (logic emits Siren0 while `global_fright_timer > 0`).
const N_SIRENS: usize = 4;
/// Keep the siren alive this many logic ticks past the last SirenN event
/// (~0.5 s at 60 Hz) so brief gaps do not cause a restart.
const SIREN_HOLD_TICKS: u32 = 30;
/// Minimum spacing between waka plays, in logic ticks (~80 ms at 60 Hz) so
/// rapid dot-eating cannot stack overlapping chomps into a buzz.
const WAKA_COOLDOWN_TICKS: u32 = 5;

// ---------------------------------------------------------------------------
// WAV container + waveform synthesis
// ---------------------------------------------------------------------------

/// Wrap mono 16-bit PCM samples (f32 in -1..1) into a minimal WAV byte
/// buffer: 44-byte canonical header, PCM format tag 1, mono, 44100 Hz,
/// 16-bit little-endian — exactly what audrey/quad-snd's decode chain
/// expects.
fn wav_from_samples(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + samples.len() * 2);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    v.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes()); // byte rate
    v.extend_from_slice(&2u16.to_le_bytes()); // block align
    v.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let s = s.clamp(-1.0, 1.0);
        v.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
    }
    v
}

/// Render one sample of a phase-continuous oscillator.
/// `phase_cycles` accumulates cycles (fractional part = phase in 0..1).
#[inline]
fn osc(phase_cycles: f32, square: bool) -> f32 {
    if square {
        // Symmetric ±1 square: zero mean, no DC offset.
        if phase_cycles.fract() < 0.5 { 1.0 } else { -1.0 }
    } else {
        (phase_cycles * std::f32::consts::TAU).sin()
    }
}

/// Linear fade-in/out envelope (~3 ms) to kill edge clicks/pops on short
/// square-wave samples.
#[inline]
fn edge_fade(t: f32, dur: f32) -> f32 {
    const FADE: f32 = 0.003; // 3 ms
    let att = (t / FADE).min(1.0);
    let rel = ((dur - t) / FADE).min(1.0);
    att.max(0.0) * rel.max(0.0)
}

/// Linear frequency sweep, square or sine wave, optional linear decay.
/// `f0`/`f1` in Hz, `dur` in seconds, phase-continuous. Returns raw
/// samples so callers can chain segments into longer effects.
fn sweep_samples(f0: f32, f1: f32, dur: f32, vol: f32, square: bool, decay: bool) -> Vec<f32> {
    let n = ((dur * SAMPLE_RATE) as usize).max(1);
    let mut phase_cycles = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            let k = if dur > 0.0 { t / dur } else { 0.0 };
            let f = f0 + (f1 - f0) * k;
            phase_cycles += f / SAMPLE_RATE;
            let env = edge_fade(t, dur) * if decay { 1.0 - k } else { 1.0 };
            osc(phase_cycles, square) * vol * env
        })
        .collect()
}

/// A held note (constant pitch), same envelope treatment as `sweep_samples`.
fn note_samples(f: f32, dur: f32, vol: f32, square: bool) -> Vec<f32> {
    sweep_samples(f, f, dur, vol, square, false)
}

/// Concatenate envelope-faded segment parts into a single sample buffer.
fn concat_segments(segments: Vec<Vec<f32>>) -> Vec<f32> {
    let mut out = Vec::new();
    for s in segments {
        out.extend_from_slice(&s);
    }
    out
}

// ---------------------------------------------------------------------------
// Sound bank
// ---------------------------------------------------------------------------

/// Sound effects bank. `None` entries mean synthesis/loading failed — plays
/// become no-ops (silent fallback, never an error). Load failures are
/// reported on stderr.
pub struct Audio {
    waka_a: Option<Sound>,
    waka_b: Option<Sound>,
    dot: Option<Sound>,
    power: Option<Sound>,
    ghost_eaten: Option<Sound>,
    fruit: Option<Sound>,
    death: Option<Sound>,
    extra_life: Option<Sound>,
    /// Looping siren variants, indexed by tier 0..=3 (variant 0 doubles as
    /// the fright siren).
    sirens: [Option<Sound>; N_SIRENS],
    /// Which siren variant is currently looping, if any.
    siren_active: Option<usize>,
    /// How many more ticks to keep the siren looping after the last SirenN
    /// event (logic emits `None_` on most non-playing ticks; this avoids
    /// play/stop thrash).
    siren_hold: u32,
    waka_alt: bool,
    waka_cooldown: u32,
}

/// Load a synthesized WAV, reporting any failure loudly on stderr instead of
/// silently swallowing it. Never panics.
async fn sfx(name: &'static str, bytes: Vec<u8>) -> Option<Sound> {
    match load_sound_from_bytes(&bytes).await {
        Ok(s) => Some(s),
        Err(e) => {
            // quad-snd's Linux ALSA backend can fail (or its audio thread can
            // die) per-sound; surface it so the problem is visible instead of
            // a mysteriously silent game.
            eprintln!("[audio] failed to load '{name}': {e}");
            None
        }
    }
}

impl Audio {
    pub async fn new() -> Self {
        // --- one-shot effects ------------------------------------------------
        // Waka-waka: two alternating chomp sweeps (down / up), like the arcade
        // chomp pair.
        let waka_a = sfx("waka_a", wav_from_samples(&sweep_samples(
            540.0, 260.0, 0.075, 0.55, true, false,
        ))).await;
        let waka_b = sfx("waka_b", wav_from_samples(&sweep_samples(
            260.0, 540.0, 0.075, 0.55, true, false,
        ))).await;
        let dot = sfx("dot", wav_from_samples(&sweep_samples(
            560.0, 560.0, 0.03, 0.45, true, true,
        ))).await;
        // Power pellet: a burst of rising pulses, each starting higher than
        // the last — the arcade "energize" feel in one distinct longer effect.
        let power = {
            let mut segs = Vec::new();
            for k in 0..6 {
                let base = 160.0 + k as f32 * 70.0;
                segs.push(sweep_samples(base, base + 220.0, 0.09, 0.55, true, false));
            }
            sfx("power", wav_from_samples(&concat_segments(segs))).await
        };
        // Ghost eaten: classic rising sine "whine" with a bright finish.
        let ghost_eaten = {
            let segs = vec![
                sweep_samples(220.0, 880.0, 0.22, 0.5, false, false),
                sweep_samples(880.0, 1500.0, 0.14, 0.5, false, true),
            ];
            sfx("ghost_eaten", wav_from_samples(&concat_segments(segs))).await
        };
        // Fruit: two quick ascending tones.
        let fruit = {
            let segs = vec![
                note_samples(660.0, 0.07, 0.5, false),
                note_samples(990.0, 0.10, 0.5, false),
            ];
            sfx("fruit", wav_from_samples(&concat_segments(segs))).await
        };
        // Death: the classic descending warble — a series of downward sweeps
        // that start higher and fall faster each time, then two short rising
        // "boops" at the end.
        let death = {
            let mut segs = Vec::new();
            for k in 0..4 {
                let top = 900.0 - k as f32 * 180.0;
                let bot = 380.0 - k as f32 * 80.0;
                segs.push(sweep_samples(top, bot, 0.22 - k as f32 * 0.03, 0.6, true, true));
            }
            segs.push(sweep_samples(110.0, 460.0, 0.13, 0.6, true, false));
            segs.push(sweep_samples(110.0, 460.0, 0.13, 0.6, true, true));
            sfx("death", wav_from_samples(&concat_segments(segs))).await
        };
        // Extra life: rising arpeggio.
        let extra_life = {
            let segs = vec![
                note_samples(523.0, 0.08, 0.5, false),
                note_samples(659.0, 0.08, 0.5, false),
                note_samples(784.0, 0.08, 0.5, false),
                note_samples(1047.0, 0.14, 0.5, false),
            ];
            sfx("extra_life", wav_from_samples(&concat_segments(segs))).await
        };

        // --- looping sirens ---------------------------------------------------
        // Each variant is a seamless up-down wail loop (rises then falls back
        // to its base pitch, so looping never clicks). Higher tiers wail
        // higher and faster, like the arcade's escalating siren.
        let siren_def = [
            (320.0, 640.0, 0.35), // tier 0 / fright
            (400.0, 800.0, 0.32), // tier 1
            (480.0, 960.0, 0.28), // tier 2
            (560.0, 1120.0, 0.24), // tier 3
        ];
        let mut sirens = [None, None, None, None];
        for (i, (f0, f1, half)) in siren_def.iter().enumerate() {
            let segs = vec![
                sweep_samples(*f0, *f1, *half, 0.28, false, false),
                sweep_samples(*f1, *f0, *half, 0.28, false, false),
            ];
            sirens[i] = sfx(match i {
                0 => "siren0",
                1 => "siren1",
                2 => "siren2",
                _ => "siren3",
            }, wav_from_samples(&concat_segments(segs))).await;
        }

        Audio {
            waka_a,
            waka_b,
            dot,
            power,
            ghost_eaten,
            fruit,
            death,
            extra_life,
            sirens,
            siren_active: None,
            siren_hold: 0,
            waka_alt: false,
            waka_cooldown: 0,
        }
    }

    /// React to the sound event emitted by the most recent logic tick.
    /// Non-blocking, panic-free.
    pub fn update(&mut self, ev: SoundEvent) {
        if self.waka_cooldown > 0 {
            self.waka_cooldown -= 1;
        }

        let params = |vol: f32, looped: bool| PlaySoundParams { looped, volume: vol };

        match ev {
            SoundEvent::Waka => {
                // The siren keeps running underneath (like the arcade); the
                // chomps overlay it. Throttle so fast dot-eating cannot stack
                // overlapping wakas into a buzz.
                if self.waka_cooldown == 0 {
                    let w = if self.waka_alt {
                        self.waka_a.as_ref()
                    } else {
                        self.waka_b.as_ref()
                    };
                    self.waka_alt = !self.waka_alt;
                    if let Some(s) = w {
                        play_sound(s, params(0.55, false));
                    }
                    self.waka_cooldown = WAKA_COOLDOWN_TICKS;
                }
            }
            SoundEvent::Dot => {
                if let Some(s) = self.dot.as_ref() {
                    play_sound(s, params(0.45, false));
                }
            }
            SoundEvent::PowerPellet => {
                self.stop_siren();
                if let Some(s) = self.power.as_ref() {
                    play_sound(s, params(0.6, false));
                }
            }
            SoundEvent::GhostEaten => {
                self.stop_siren();
                if let Some(s) = self.ghost_eaten.as_ref() {
                    play_sound(s, params(0.6, false));
                }
            }
            SoundEvent::Fruit => {
                self.stop_siren();
                if let Some(s) = self.fruit.as_ref() {
                    play_sound(s, params(0.6, false));
                }
            }
            SoundEvent::Death => {
                self.stop_siren();
                if let Some(s) = self.death.as_ref() {
                    play_sound(s, params(0.65, false));
                }
            }
            SoundEvent::ExtraLife => {
                if let Some(s) = self.extra_life.as_ref() {
                    play_sound(s, params(0.55, false));
                }
            }
            SoundEvent::Siren0 | SoundEvent::Siren1 | SoundEvent::Siren2 | SoundEvent::Siren3 => {
                let tier = match ev {
                    SoundEvent::Siren1 => 1,
                    SoundEvent::Siren2 => 2,
                    SoundEvent::Siren3 => 3,
                    _ => 0,
                };
                self.siren_hold = SIREN_HOLD_TICKS;
                // Only (re)start when the variant actually changes; the loop
                // keeps running untouched while the same variant repeats, so
                // there is no restart-from-sample-0 stutter.
                if self.siren_active != Some(tier) {
                    self.stop_siren();
                    if let Some(s) = self.sirens[tier].as_ref() {
                        play_sound(s, params(0.28, true));
                        self.siren_active = Some(tier);
                    }
                }
            }
            SoundEvent::None_ => {
                // Outside Playing (menus, death animation, level complete) no
                // siren events arrive; let the loop ride out the hold, then
                // stop once.
                if self.siren_hold > 0 {
                    self.siren_hold -= 1;
                }
                if self.siren_hold == 0 {
                    self.stop_siren();
                }
            }
        }
    }

    fn stop_siren(&mut self) {
        if let Some(tier) = self.siren_active {
            if let Some(s) = self.sirens[tier].as_ref() {
                stop_sound(s);
            }
            self.siren_active = None;
        }
    }
}
