//! Ghost AI: tile targeting (Blinky/Pinky/Inky/Clyde), scatter/chase schedule,
//! frightened mode with per-level duration, dot-counter house releases, forced
//! reversals, house exit/entry paths, tunnel slowdown and Cruise Elroy.
//!
//! Faithfulness notes (Pac-Man Dossier):
//!   * Ghosts pick a direction at each tile center among non-reverse walkable
//!     neighbors, minimizing Euclidean tile distance to their target, with
//!     tie-break priority Up > Left > Down > Right. Frightened ghosts pick a
//!     random legal direction instead (xorshift RNG).
//!   * Mode switches (scatter<->chase, power pellet) force a reversal of every
//!     hunting ghost.
//!   * Four tiles ((12,13),(15,13),(12,25),(15,25)) never allow a ghost in
//!     scatter/chase to turn upwards — the arcade's "no-up" quirk tiles.
//!   * Speeds (fraction of BASE_SPEED = 1.25 px/tick):
//!       level 1: ghosts 75%, frightened 50%, tunnel 40%
//!       levels 2-4: 85% / 55% / 45%
//!       levels 5+: 95% / 60% / 50%
//!       Cruise Elroy: 80% (level 1) / 85% (level 2)
//!       eaten eyes: 160%; house exit: 50%.
//!   * Dot-counter house releases: at level start Blinky 0, Pinky 0, Inky 30,
//!     Clyde 60 (level 2: Inky 0, Clyde 50; level 3+: all 0). After a life is
//!     lost, personal counters are used: Pinky 7, Inky 17, Clyde 17. A ghost
//!     waiting 5 s without a dot being eaten is force-released.

use super::maze::{ghost_walkable, grid_move, in_tunnel, tile_coords, wrap_x};
use super::pacman::BASE_SPEED;
use super::rng::rng_below;
use super::{Direction, Game, GhostId, GhostMode};

/// Pixel positions: ghost house center (between cols 13/14), inside-house
/// resting row (row 17), just-above-door row (row 14).
pub(crate) const HOUSE_X: f32 = 112.0;
pub(crate) const HOUSE_Y: f32 = 140.0; // row 17 center (inside house)
pub(crate) const DOOR_Y: f32 = 116.0; // row 14 center (above door)
const HOUSE_SPEED: f32 = BASE_SPEED * 0.5;
const EATEN_SPEED: f32 = BASE_SPEED * 1.6;

/// Door tiles are on this row (13 and 14 are the two door columns).
const DOOR_ROW: i32 = 15;

/// Scatter corner targets (tile x, tile y) — the arcade's exact values.
fn scatter_target(id: GhostId) -> (i32, i32) {
    match id {
        GhostId::Blinky => (25, 0),
        GhostId::Pinky => (2, 0),
        GhostId::Inky => (27, 34),
        GhostId::Clyde => (0, 34),
    }
}

/// The four arcade "no upward turn" tiles for hunting ghosts.
const GHOST_NO_UP: [(i32, i32); 4] = [(12, 13), (15, 13), (12, 25), (15, 25)];

/// Ghost speed as a percentage of full speed, per level (Dossier tables).
fn ghost_pct(level: u32) -> f32 {
    match level {
        1 => 0.75,
        2..=4 => 0.85,
        _ => 0.95,
    }
}

fn frightened_pct(level: u32) -> f32 {
    match level {
        1 => 0.50,
        2..=4 => 0.55,
        _ => 0.60,
    }
}

fn tunnel_pct(level: u32) -> f32 {
    match level {
        1 => 0.40,
        2..=4 => 0.45,
        _ => 0.50,
    }
}

/// Cruise Elroy dot thresholds: (elroy1, elroy2) dots remaining per level.
pub(crate) fn elroy_thresholds(level: u32) -> (u32, u32) {
    match level {
        1 => (20, 10),
        2 => (30, 15),
        3..=5 => (40, 20),
        6..=8 => (50, 25),
        9..=11 => (60, 30),
        12..=14 => (80, 40),
        15..=18 => (100, 50),
        _ => (120, 60),
    }
}

/// Current Cruise Elroy level for Blinky (0, 1 or 2). Elroy only activates
/// once Clyde has left the house.
pub(crate) fn elroy_level(game: &Game) -> u32 {
    if !game.clyde_out {
        return 0;
    }
    let (d1, d2) = elroy_thresholds(game.level);
    if game.dots_remaining <= d2 {
        2
    } else if game.dots_remaining <= d1 {
        1
    } else {
        0
    }
}

/// Ghost speed in px per tick for the current situation.
pub(crate) fn ghost_speed(game: &Game, id: GhostId) -> f32 {
    let g = &game.ghosts[id.index()];
    match g.mode {
        GhostMode::Eaten => EATEN_SPEED,
        _ if g.in_tunnel => BASE_SPEED * tunnel_pct(game.level),
        GhostMode::Frightened => BASE_SPEED * frightened_pct(game.level),
        GhostMode::Scatter | GhostMode::Chase => {
            if id == GhostId::Blinky {
                match elroy_level(game) {
                    2 => return BASE_SPEED * 0.85,
                    1 => return BASE_SPEED * 0.80,
                    _ => {}
                }
            }
            BASE_SPEED * ghost_pct(game.level)
        }
        // House movements are slow.
        GhostMode::InHouse | GhostMode::LeavingHouse => HOUSE_SPEED,
    }
}

/// Dot-counter release limits. `global` = fresh level (arcade level-start
/// counters); otherwise the post-death personal counters.
pub(crate) fn release_limit(level: u32, global: bool, id: GhostId) -> u32 {
    if global {
        match id {
            GhostId::Blinky => 0,
            GhostId::Pinky => 0,
            GhostId::Inky => {
                if level == 1 {
                    30
                } else {
                    0
                }
            }
            GhostId::Clyde => match level {
                1 => 60,
                2 => 50,
                _ => 0,
            },
        }
    } else {
        match id {
            GhostId::Blinky => 0,
            GhostId::Pinky => 7,
            _ => 17, // Inky and Clyde
        }
    }
}

/// Internal per-ghost AI state (not part of the render contract).
#[derive(Clone, Copy, Debug)]
pub struct GhostAi {
    /// Set on scatter<->chase switches and power pellets; the ghost reverses
    /// at the start of its next movement tick.
    pub reverse_pending: bool,
    /// Eaten ghost has reached the door and is descending into the house.
    pub entering: bool,
}

impl GhostAi {
    pub(crate) const fn new() -> Self {
        GhostAi {
            reverse_pending: false,
            entering: false,
        }
    }
}

/// Chase target tile for a ghost.
fn chase_target(game: &Game, id: GhostId) -> (i32, i32) {
    let pac = tile_coords(game.pacman.x, game.pacman.y);
    let pdx = game.pacman.dir.delta().0 as i32;
    let pdy = game.pacman.dir.delta().1 as i32;
    match id {
        // Blinky: Pac-Man's tile directly.
        GhostId::Blinky => pac,
        // Pinky: 4 tiles ahead of Pac-Man, including the famous "up" bug
        // (an extra 4-tile left shift when Pac-Man faces up).
        GhostId::Pinky => {
            let mut t = (pac.0 + pdx * 4, pac.1 + pdy * 4);
            if game.pacman.dir == Direction::Up {
                t.0 -= 4;
            }
            t
        }
        // Inky: double the vector from Blinky through the pivot 2 tiles ahead
        // of Pac-Man (with the same up-bug offset).
        GhostId::Inky => {
            let mut pivot = (pac.0 + pdx * 2, pac.1 + pdy * 2);
            if game.pacman.dir == Direction::Up {
                pivot.0 -= 2;
            }
            let b = tile_coords(game.ghosts[0].x, game.ghosts[0].y);
            (2 * pivot.0 - b.0, 2 * pivot.1 - b.1)
        }
        // Clyde: Pac-Man if more than 8 tiles away, else his scatter corner.
        GhostId::Clyde => {
            let c = tile_coords(game.ghosts[3].x, game.ghosts[3].y);
            let dx = (c.0 - pac.0) as f32;
            let dy = (c.1 - pac.1) as f32;
            if dx * dx + dy * dy > 64.0 {
                pac
            } else {
                scatter_target(GhostId::Clyde)
            }
        }
    }
}

/// Target tile for a ghost in its current mode.
fn ghost_target(game: &Game, id: GhostId) -> (i32, i32) {
    let g = &game.ghosts[id.index()];
    match g.mode {
        // Eaten eyes head for the door tile.
        GhostMode::Eaten => (13, DOOR_ROW),
        // Elroy Blinky ignores scatter and always hunts Pac-Man.
        GhostMode::Scatter
            if id == GhostId::Blinky && elroy_level(game) > 0 =>
        {
            chase_target(game, id)
        }
        GhostMode::Scatter => scatter_target(id),
        GhostMode::Chase => chase_target(game, id),
        // Frightened ghosts move randomly; target unused.
        GhostMode::Frightened => (0, 0),
        // Not used for house states.
        GhostMode::InHouse | GhostMode::LeavingHouse => (13, DOOR_ROW),
    }
}

/// Direction decision at a tile center for a grid-riding ghost.
#[allow(clippy::too_many_arguments)]
fn decide_ghost(
    grid: &[super::Tile; super::MAZE_W * super::MAZE_H],
    mode: GhostMode,
    target: (i32, i32),
    eaten: bool,
    rng_state: &mut u32,
    tx: i32,
    ty: i32,
    cur: Direction,
) -> Option<Direction> {
    // Eaten eyes arriving at the door: stop and switch to scripted entry.
    if eaten && ty == DOOR_ROW && (tx == 13 || tx == 14) {
        return None;
    }

    let mut best: Option<(f32, Direction)> = None;
    let mut candidates: Vec<Direction> = Vec::with_capacity(3);
    for d in [
        Direction::Up,
        Direction::Left,
        Direction::Down,
        Direction::Right,
    ] {
        if d == cur.opposite() {
            continue; // ghosts never voluntarily reverse
        }
        let (dx, dy) = d.delta();
        let nx = super::maze::wrap_col(tx + dx as i32);
        let ny = ty + dy as i32;
        if !ghost_walkable(grid, nx, ny, eaten) {
            continue;
        }
        // Arcade quirk: a ghost standing in one of the four special tiles may
        // not decide to move up out of it while hunting (scatter/chase).
        if d == Direction::Up
            && matches!(mode, GhostMode::Scatter | GhostMode::Chase)
            && GHOST_NO_UP.contains(&(tx, ty))
        {
            continue;
        }
        candidates.push(d);
        if !matches!(mode, GhostMode::Frightened) {
            let ddx = (nx - target.0) as f32;
            let ddy = (ny - target.1) as f32;
            let dist = ddx * ddx + ddy * ddy;
            // Strictly less: keeps the Up > Left > Down > Right priority.
            if best.map_or(true, |(bd, _)| dist < bd) {
                best = Some((dist, d));
            }
        }
    }
    if candidates.is_empty() {
        // Dead end: force a reversal (never occurs in the maze proper).
        return Some(cur.opposite());
    }
    if matches!(mode, GhostMode::Frightened) {
        let pick = rng_below(rng_state, candidates.len() as u32) as usize;
        return Some(candidates[pick]);
    }
    best.map(|(_, d)| d)
}

/// Move a hunting/eaten ghost one tick through the maze.
fn move_grid_ghost(game: &mut Game, id: GhostId) {
    let i = id.index();
    let mode = game.ghosts[i].mode;
    let eaten = mode == GhostMode::Eaten;

    // Forced reversal (mode switches): instant, like the arcade.
    if game.ai[i].reverse_pending {
        game.ai[i].reverse_pending = false;
        game.ghosts[i].dir = game.ghosts[i].dir.opposite();
    }

    let speed = ghost_speed(game, id);
    let target = ghost_target(game, id);
    let mut pos = (game.ghosts[i].x, game.ghosts[i].y);
    let dir = game.ghosts[i].dir;
    let grid = game.grid; // [Tile; 1008] is Copy
    let mut rng_state = game.rng_state;

    let (new_dir, _stopped) = grid_move(&mut pos, dir, speed, &mut |tx, ty, cur| {
        decide_ghost(&grid, mode, target, eaten, &mut rng_state, tx, ty, cur)
    });
    game.rng_state = rng_state;

    game.ghosts[i].x = pos.0;
    game.ghosts[i].y = pos.1;
    game.ghosts[i].dir = new_dir;

    // Eaten eyes that reached the door tile start the scripted house entry.
    if eaten && !game.ai[i].entering {
        let (tx, ty) = tile_coords(pos.0, pos.1);
        if ty == DOOR_ROW && (tx == 13 || tx == 14) {
            game.ai[i].entering = true;
        }
    }
}

/// Bob up and down while waiting inside the house.
fn bob_in_house(game: &mut Game, id: GhostId) {
    let g = &mut game.ghosts[id.index()];
    match g.dir {
        Direction::Up => {
            g.y -= 0.25;
            if g.y <= 136.0 {
                g.y = 136.0;
                g.dir = Direction::Down;
            }
        }
        _ => {
            g.y += 0.25;
            if g.y >= 144.0 {
                g.y = 144.0;
                g.dir = Direction::Up;
            }
        }
    }
}

/// Scripted exit: center on the house x, then rise through the door.
fn leave_house(game: &mut Game, id: GhostId) {
    let g = &mut game.ghosts[id.index()];
    if g.x != HOUSE_X {
        // Move horizontally toward the house center (exact dyadic steps).
        if g.x < HOUSE_X {
            g.x = (g.x + HOUSE_SPEED).min(HOUSE_X);
        } else {
            g.x = (g.x - HOUSE_SPEED).max(HOUSE_X);
        }
    } else if g.y > DOOR_Y {
        g.y -= HOUSE_SPEED;
        if g.y <= DOOR_Y {
            g.y = DOOR_Y;
            finish_exit(game, id);
        }
    } else {
        finish_exit(game, id);
    }
}

/// Completing the house exit: adopt the current schedule mode (or frightened).
fn finish_exit(game: &mut Game, id: GhostId) {
    let frightened = game.ghosts[id.index()].frightened_timer > 0;
    let next_mode = if frightened {
        GhostMode::Frightened
    } else {
        game.schedule_mode()
    };
    let g = &mut game.ghosts[id.index()];
    g.x = HOUSE_X;
    g.y = DOOR_Y;
    g.dir = Direction::Left; // arcade ghosts leave heading left
    g.mode = next_mode;
    if id == GhostId::Clyde {
        game.clyde_out = true; // enables Cruise Elroy
    }
}

/// Scripted entry for eaten eyes: center x, descend to the resting row,
/// then revive and immediately head back out.
fn enter_house(game: &mut Game, id: GhostId) {
    // A ghost revived while a fright window is still running comes back out
    // frightened for the remainder of the window (arcade behavior).
    let fright = game.global_fright_timer;
    let g = &mut game.ghosts[id.index()];
    if g.x != HOUSE_X {
        if g.x < HOUSE_X {
            g.x = (g.x + EATEN_SPEED).min(HOUSE_X);
        } else {
            g.x = (g.x - EATEN_SPEED).max(HOUSE_X);
        }
    } else if g.y < HOUSE_Y {
        g.y += EATEN_SPEED;
        if g.y >= HOUSE_Y {
            g.y = HOUSE_Y;
            // Revive: head straight back out.
            game.ai[id.index()].entering = false;
            g.frightened_timer = fright;
            g.mode = GhostMode::LeavingHouse;
            g.dir = Direction::Up;
        }
    } else {
        // Exactly at home: revive.
        game.ai[id.index()].entering = false;
        g.frightened_timer = fright;
        g.mode = GhostMode::LeavingHouse;
        g.dir = Direction::Up;
    }
}

/// Step all four ghosts for one tick.
pub(crate) fn step_ghosts(game: &mut Game) {
    for id in [
        GhostId::Blinky,
        GhostId::Pinky,
        GhostId::Inky,
        GhostId::Clyde,
    ] {
        let mode = game.ghosts[id.index()].mode;
        match mode {
            GhostMode::InHouse => bob_in_house(game, id),
            GhostMode::LeavingHouse => leave_house(game, id),
            GhostMode::Eaten => {
                if game.ai[id.index()].entering {
                    enter_house(game, id);
                } else {
                    move_grid_ghost(game, id);
                }
            }
            _ => move_grid_ghost(game, id),
        }
        // Update tunnel flag (drives the tunnel speed penalty).
        let g = &mut game.ghosts[id.index()];
        g.in_tunnel = in_tunnel(g.x, g.y);
        // Keep pixel positions inside the legal play strip.
        g.x = wrap_x(g.x);
    }
}

/// Bob the house ghosts only (used during the Ready countdown).
pub(crate) fn step_ghosts_house_only(game: &mut Game) {
    for id in [GhostId::Pinky, GhostId::Inky, GhostId::Clyde] {
        if game.ghosts[id.index()].mode == GhostMode::InHouse {
            bob_in_house(game, id);
        }
    }
}
