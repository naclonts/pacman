# Pac-Man (Rust) — Implementation Plan & Frozen API Contract

Faithful 1980-arcade Pac-Man clone in Rust. Headless, deterministic logic core
(`src/logic/`) + macroquad presentation layer. The logic core must be reusable
later for neural-network training (observable state + discrete action, no
rendering/I-O dependency).

## Binding boundary rules
- Fixed logical tick = 1/60 s. All timings in ticks.
- `src/logic/**` must NOT depend on macroquad or any I/O; no `std::time` inside logic. Deterministic: same seed + same action sequence => same state sequence. Avoid platform-varying float ops in movement paths where possible.
- Presentation code imports only from `crate::logic` (the contract below). It reads state; it never mutates `Game` fields except via `step`.
- Screen is 224×288 game-pixels (1 tile = 8 px, maze 28×36 tiles); render scales up.

## Logic/Render API Contract (src/logic/mod.rs re-exports these; nothing else guaranteed)

```rust
pub const TICK_RATE_HZ: u32 = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action { None, Up, Down, Left, Right } // desired direction; buffering/turning handled by logic

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GamePhase { Attract, Ready(u32), Playing, Dying, LevelComplete, GameOver }
// Ready = countdown ticks before play; Dying = death animation; LevelComplete = maze flash

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GhostId { Blinky, Pinky, Inky, Clyde }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GhostMode { InHouse, LeavingHouse, Scatter, Chase, Frightened, Eaten } // Eaten = returning home

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction { Up, Down, Left, Right }

#[derive(Clone, Copy, Debug)]
pub struct GhostState {
    pub id: GhostId,
    pub x: f32, pub y: f32,          // game-pixel space (0..224 x, 0..288 y)
    pub dir: Direction,
    pub mode: GhostMode,
    pub frightened_timer: u32,
    pub in_tunnel: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PacmanState {
    pub x: f32, pub y: f32,
    pub dir: Direction,
    pub mouth_phase: u32,        // animation frame 0..=3
    pub dead_anim_frame: u32,
}

pub const MAZE_W: usize = 28;
pub const MAZE_H: usize = 36;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tile { Wall, Empty, Dot, PowerPellet, Door, Tunnel }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FruitKind { Cherry, Strawberry, Orange, Apple, Melon, Galaxian, Bell, Key }

#[derive(Clone, Copy, Debug)]
pub struct FruitState { pub kind: FruitKind, pub active: bool, pub timer: u32, pub x: f32, pub y: f32 }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoundEvent { Waka, Dot, PowerPellet, GhostEaten, Fruit, Death, ExtraLife, Siren0, Siren1, Siren2, Siren3, None_ }

#[derive(Clone, Debug)]
pub struct Game {
    pub phase: GamePhase,
    pub level: u32,
    pub score: u32,
    pub high_score: u32,
    pub lives: u32,
    pub dots_eaten: u32,
    pub dots_remaining: u32,       // drives ghost-house releases & level end
    pub pacman: PacmanState,
    pub ghosts: [GhostState; 4],   // index = GhostId as usize
    pub fruit: FruitState,
    pub grid: [Tile; MAZE_W * MAZE_H],  // row-major, index = y * MAZE_W + x
    pub rng_state: u32,            // xorshift state
    pub global_fright_timer: u32,
    pub bonus_timer_active: bool,
    pub last_sound: SoundEvent,    // overwritten each tick
    pub ghost_combo: u32,          // 200/400/800/1600 chain per frightened window
    pub score_popup: Option<(f32, f32, u32)>,
    pub frames: u64,               // total ticks since Game::new
}

impl Game {
    pub fn new(level: u32) -> Self;
    pub fn new_with_seed(level: u32, seed: u32) -> Self;
    pub fn step(&mut self, action: Action);   // advance exactly one tick
    pub fn ghost(&self, id: GhostId) -> GhostState;
    pub fn tile_at(&self, tx: usize, ty: usize) -> Tile;
    pub fn observe(&self) -> Vec<f32>;        // flat observation vector for future RL use
}
```

## Gameplay faithfulness requirements (Worker A)
- Exact 28×36 maze layout, original-style colors (blue #2121ff walls, etc.).
- Ghost tile-target AI: Blinky (Pac tile), Pinky (4 tiles ahead), Inky (vector doubling from Blinky), Clyde (Pac if >8 tiles else scatter corner). Scatter/chase schedule 7s/20s/7s/20s/5s/20s/5s/forever (level-dependent), frightened reversal + per-level frightened duration table, ghost-house dot-counter releases (Blinky 0, Pinky 0, Inky 30, Clyde 60, with level overrides), forced reversals on mode change, house exit/entry paths.
- Speeds: base ≈ 80 px/s scaled per level; frightened ~40-50%; tunnel ~40%; elroy 80/85%; Pac-Man ~80% leveling to ~90%.
- Pac-Man cornering (pre-turn near tile center), direction buffering, wall collision.
- Scoring: dot 10, pellet 50, ghosts 200/400/800/1600, fruit per level table, extra life at 10,000.
- Phases: Attract → Ready(~2s) → Playing → Dying/LevelComplete → next level or GameOver.

## File ownership (STRICT — do not touch files you do not own)
- Worker A: `src/logic/**` only (`mod.rs`, `maze.rs`, `game.rs`, `pacman.rs`, `ghosts.rs`, `rng.rs`, `tests.rs`) + `tests/headless_sim.rs`.
- Worker B: `src/render.rs`, `src/audio.rs`, `src/input.rs` only.
- `Cargo.toml` and `src/main.rs` are pre-scaffolded and FROZEN.
