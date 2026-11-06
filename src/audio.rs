// Presentation layer: procedural audio. Worker B owns this file.
//
// All sounds are synthesized in code (small PCM waveforms wrapped in a
// generated WAV container) and played through macroquad's audio module.
// Everything here is best-effort: if macroquad's `audio` feature is not
// enabled (macroquad 0.4.16 has default features = [], and Cargo.toml is
// frozen), macroquad::audio compiles as a silent dummy and every call is a
// harmless no-op. Nothing in this module ever blocks the game loop or panics.
use crate::logic::SoundEvent;
use macroquad::audio::{
    load_sound_from_bytes, play_sound, stop_sound, PlaySoundParams, Sound,
};

const SAMPLE_RATE: f32 = 44100.0;

// ---------------------------------------------------------------------------
// WAV container + waveform synthesis
// ---------------------------------------------------------------------------

/// Wrap mono 16-bit PCM samples (f32 in -1..1) into a minimal WAV byte buffer.
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

/// Linear frequency sweep, square or sine wave, optional linear decay.
/// `f0`/`f1` in Hz, `dur` in seconds, phase-continuous.
fn sweep(f0: f32, f1: f32, dur: f32, vol: f32, square: bool, decay: bool) -> Vec<u8> {
    let n = (dur * SAMPLE_RATE) as usize;
    let mut phase_cycles = 0.0f32;
    let samples: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            let f = f0 + (f1 - f0) * (t / dur);
            phase_cycles += f / SAMPLE_RATE;
            let wave = if square {
                if phase_cycles.fract() < 0.5 { 1.0 } else { -1.0 }
            } else {
                (phase_cycles * std::f32::consts::TAU).sin()
            };
            let env = if decay { 1.0 - t / dur } else { 1.0 };
            wave * vol * env
        })
        .collect();
    wav_from_samples(&samples)
}

/// Sound effects bank. `None` entries mean synthesis/loading failed — plays
/// become no-ops (silent fallback, never an error).
pub struct Audio {
    waka_a: Option<Sound>,
    waka_b: Option<Sound>,
    dot: Option<Sound>,
    power: Option<Sound>,
    ghost_eaten: Option<Sound>,
    fruit: Option<Sound>,
    death: Option<Sound>,
    extra_life: Option<Sound>,
    siren: Option<Sound>,
    siren_playing: bool,
    /// How many more ticks to keep the siren looping after the last SirenN
    /// event (logic may emit `None_` on most ticks; this avoids play/stop
    /// thrash).
    siren_hold: u32,
    waka_alt: bool,
}

async fn sfx(bytes: Vec<u8>) -> Option<Sound> {
    // Async (native resolves immediately). On Err we stay silent. The
    // futures are awaited during first-frame init — never blocks the loop.
    load_sound_from_bytes(&bytes).await.ok()
}

impl Audio {
    pub async fn new() -> Self {
        Audio {
            waka_a: sfx(sweep(520.0, 290.0, 0.07, 0.55, true, false)).await,
            waka_b: sfx(sweep(290.0, 520.0, 0.07, 0.55, true, false)).await,
            dot: sfx(sweep(560.0, 560.0, 0.03, 0.45, true, true)).await,
            power: sfx(sweep(180.0, 720.0, 0.25, 0.6, true, false)).await,
            ghost_eaten: sfx(sweep(320.0, 1450.0, 0.30, 0.6, false, true)).await,
            fruit: sfx(sweep(420.0, 950.0, 0.14, 0.6, false, true)).await,
            death: sfx(sweep(620.0, 70.0, 0.70, 0.65, true, true)).await,
            extra_life: sfx(sweep(620.0, 1250.0, 0.28, 0.55, false, false)).await,
            siren: sfx(sweep(320.0, 640.0, 0.50, 0.30, false, false)).await,
            siren_playing: false,
            siren_hold: 0,
            waka_alt: false,
        }
    }

    /// React to the sound event emitted by the most recent logic tick.
    /// Non-blocking, panic-free.
    pub fn update(&mut self, ev: SoundEvent) {
        let siren_ev = matches!(
            ev,
            SoundEvent::Siren0 | SoundEvent::Siren1 | SoundEvent::Siren2 | SoundEvent::Siren3
        );
        if siren_ev {
            self.siren_hold = 45; // keep the siren alive ~0.75s past last event
        } else if self.siren_hold > 0 {
            self.siren_hold = self.siren_hold.saturating_sub(1);
        }

        // Any non-siren sound interrupts the siren, like the arcade.
        let params = |vol: f32| PlaySoundParams { looped: false, volume: vol };
        match ev {
            SoundEvent::Waka => {
                self.stop_siren();
                let w = if self.waka_alt {
                    self.waka_a.as_ref()
                } else {
                    self.waka_b.as_ref()
                };
                self.waka_alt = !self.waka_alt;
                if let Some(s) = w {
                    play_sound(s, params(0.55));
                }
            }
            SoundEvent::Dot => {
                if let Some(s) = self.dot.as_ref() {
                    play_sound(s, params(0.45));
                }
            }
            SoundEvent::PowerPellet => {
                self.stop_siren();
                if let Some(s) = self.power.as_ref() {
                    play_sound(s, params(0.6));
                }
            }
            SoundEvent::GhostEaten => {
                self.stop_siren();
                if let Some(s) = self.ghost_eaten.as_ref() {
                    play_sound(s, params(0.6));
                }
            }
            SoundEvent::Fruit => {
                self.stop_siren();
                if let Some(s) = self.fruit.as_ref() {
                    play_sound(s, params(0.6));
                }
            }
            SoundEvent::Death => {
                self.stop_siren();
                if let Some(s) = self.death.as_ref() {
                    play_sound(s, params(0.65));
                }
            }
            SoundEvent::ExtraLife => {
                if let Some(s) = self.extra_life.as_ref() {
                    play_sound(s, params(0.55));
                }
            }
            SoundEvent::Siren0 | SoundEvent::Siren1 | SoundEvent::Siren2 | SoundEvent::Siren3 => {
                if !self.siren_playing {
                    if let Some(s) = self.siren.as_ref() {
                        play_sound(
                            s,
                            PlaySoundParams { looped: true, volume: 0.3 },
                        );
                        self.siren_playing = true;
                    }
                }
            }
            SoundEvent::None_ => {
                if self.siren_hold == 0 {
                    self.stop_siren();
                }
            }
        }
    }

    fn stop_siren(&mut self) {
        if self.siren_playing {
            if let Some(s) = self.siren.as_ref() {
                stop_sound(s);
            }
            self.siren_playing = false;
        }
    }
}
