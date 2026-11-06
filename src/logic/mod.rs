//! Headless, deterministic Pac-Man game logic — a faithful recreation of the
//! 1980 arcade mechanics.
//!
//! This module is the frozen Logic/Render API contract (see PLAN.md):
//!   * fixed logical tick = 1/60 s, all timings expressed in ticks
//!   * no macroquad / I/O / std::time dependencies
//!   * deterministic: same seed + same action sequence => same state sequence
//!   * coordinate space: game-pixels, 1 tile = 8 px, screen 224x288
//!
//! Submodules:
//!   * [`maze`]   — exact 28x36 arcade maze, palette constants, tile/pixel helpers
//!   * [`rng`]    — xorshift32 deterministic RNG
//!   * [`pacman`] — Pac-Man movement, cornering, eating, death
//!   * [`ghosts`] — original ghost AI (targets, scatter/chase, fright, house, Elroy)
//!   * [`game`]   — `Game` state machine + scoring
//!   * [`tests`]  — headless unit tests (cfg(test))

pub const TICK_RATE_HZ: u32 = 60;

pub const MAZE_W: usize = 28;
pub const MAZE_H: usize = 36;

mod game;
mod ghosts;
mod maze;
mod pacman;
mod rng;
#[cfg(test)]
mod tests;

pub use game::Game;
// Palette/tile helpers are consumed by the presentation layer (render.rs);
// the binary target itself does not reference them yet.
#[allow(unused_imports)]
pub use maze::{
    parse_maze, COLOR_BLINKY, COLOR_CLYDE, COLOR_DOOR, COLOR_DOT, COLOR_FRIGHTENED,
    COLOR_FRIGHTENED_FLASH, COLOR_INKY, COLOR_PAC, COLOR_PELLET, COLOR_PINKY, COLOR_TEXT,
    COLOR_WALL, MAZE_MAP, SCREEN_H, SCREEN_W, TILE_PX,
};

// ---------------------------------------------------------------------------
// Contract types (exactly as specified in PLAN.md)
// ---------------------------------------------------------------------------

/// Player input for one tick. Turning/buffering is handled by the logic core.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Up,
    Down,
    Left,
    Right,
}

/// High-level game state.
/// `Ready(n)` = countdown ticks before play; `Dying` = death animation;
/// `LevelComplete` = maze flash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GamePhase {
    Attract,
    Ready(u32),
    Playing,
    Dying,
    LevelComplete,
    GameOver,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GhostId {
    Blinky, // index 0 (red)
    Pinky,  // index 1 (pink)
    Inky,   // index 2 (cyan)
    Clyde,  // index 3 (orange)
}

impl GhostId {
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GhostMode {
    InHouse,      // resting in the ghost house (bobbing)
    LeavingHouse, // scripted exit through the door
    Scatter,      // heading for the personal scatter corner
    Chase,        // using the personal chase targeting
    Frightened,   // blue, edible, random movement
    Eaten,        // eyes returning to the house
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    /// Unit step in game-pixels for one full tile of travel.
    #[inline]
    pub fn delta(self) -> (f32, f32) {
        match self {
            Direction::Up => (0.0, -1.0),
            Direction::Down => (0.0, 1.0),
            Direction::Left => (-1.0, 0.0),
            Direction::Right => (1.0, 0.0),
        }
    }

    #[inline]
    pub fn opposite(self) -> Direction {
        match self {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }

    #[inline]
    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }

    /// Map an input action to a direction, if it is a directional input.
    #[inline]
    pub fn from_action(action: Action) -> Option<Direction> {
        match action {
            Action::Up => Some(Direction::Up),
            Action::Down => Some(Direction::Down),
            Action::Left => Some(Direction::Left),
            Action::Right => Some(Direction::Right),
            Action::None => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GhostState {
    pub id: GhostId,
    pub x: f32,
    pub y: f32, // game-pixel space (0..224 x, 0..288 y)
    pub dir: Direction,
    pub mode: GhostMode,
    pub frightened_timer: u32,
    pub in_tunnel: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PacmanState {
    pub x: f32,
    pub y: f32,
    pub dir: Direction,
    pub mouth_phase: u32, // animation frame 0..=3
    pub dead_anim_frame: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tile {
    Wall,
    Empty,
    Dot,
    PowerPellet,
    Door,
    Tunnel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FruitKind {
    Cherry,
    Strawberry,
    Orange,
    Apple,
    Melon,
    Galaxian,
    Bell,
    Key,
}

#[derive(Clone, Copy, Debug)]
pub struct FruitState {
    pub kind: FruitKind,
    pub active: bool,
    pub timer: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)] // variants are consumed by the presentation layer
pub enum SoundEvent {
    Waka,
    Dot,
    PowerPellet,
    GhostEaten,
    Fruit,
    Death,
    ExtraLife,
    Siren0,
    Siren1,
    Siren2,
    Siren3,
    None_,
}
