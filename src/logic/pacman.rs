//! Pac-Man: movement at 60 Hz, direction buffering, cornering near tile
//! centers, wall collision, dot/pellet/fruit eating, death sequence.
//!
//! Speed model (fractions of BASE_SPEED, per the Pac-Man Dossier level
//! tables; 100% = 1.25 px/tick ≈ 75 px/s):
//!   * level 1: 80%, levels 2-4: 90%, levels 5-20: 100%, levels 21+: 90%
//!   * arcade "dot pause": Pac-Man freezes 1 tick per dot and 3 ticks per
//!     power pellet eaten (this produces the authentic waka-waka rhythm).
//!
//! Cornering: a buffered perpendicular turn is applied when the tile center
//! lies ahead within `CORNER_WINDOW` px of travel (position snapped onto the
//! center line), approximating the arcade's early-corner-cut window. Instant
//! reversals are always allowed, exactly like the arcade.

use super::maze::{grid_move, pac_walkable, tile_center, tile_coords, wrap_col, TILE_PX};
use super::{
    Action, Direction, FruitKind, Game, GamePhase, SoundEvent, Tile, MAZE_H, MAZE_W,
};

/// Full speed in px per tick (60 Hz): ≈ the arcade's 100% speed.
pub(crate) const BASE_SPEED: f32 = 1.25;
/// Corner-cut window in px around a tile center.
const CORNER_WINDOW: f32 = 3.0;

/// Fruit spawn tile row (directly below the ghost house), pixel position.
pub(crate) const FRUIT_X: f32 = 112.0;
pub(crate) const FRUIT_Y: f32 = 20.0 * TILE_PX + 4.0;

/// Pac-Man start position: between columns 13/14 on row 26, facing left.
pub(crate) const PAC_START: (f32, f32) = (112.0, 26.0 * TILE_PX + 4.0);

pub(crate) fn pac_speed(level: u32) -> f32 {
    let pct = match level {
        1 => 0.80,
        2..=4 => 0.90,
        5..=20 => 1.00,
        _ => 0.90,
    };
    BASE_SPEED * pct
}

/// Fruit type per level (arcade bonus table).
pub(crate) fn fruit_kind(level: u32) -> FruitKind {
    match level {
        1 => FruitKind::Cherry,
        2 => FruitKind::Strawberry,
        3..=4 => FruitKind::Orange,
        5..=6 => FruitKind::Apple,
        7..=8 => FruitKind::Melon,
        9..=10 => FruitKind::Galaxian,
        11..=12 => FruitKind::Bell,
        _ => FruitKind::Key,
    }
}

/// Fruit points per kind (arcade table).
pub(crate) fn fruit_points(kind: FruitKind) -> u32 {
    match kind {
        FruitKind::Cherry => 100,
        FruitKind::Strawberry => 300,
        FruitKind::Orange => 500,
        FruitKind::Apple => 700,
        FruitKind::Melon => 1000,
        FruitKind::Galaxian => 2000,
        FruitKind::Bell => 3000,
        FruitKind::Key => 5000,
    }
}

/// Can Pac-Man enter the neighbor tile in direction `d` from tile (tx, ty)?
/// Honors the arcade "no moving up" tiles.
fn pac_can_enter(
    grid: &[Tile; MAZE_W * MAZE_H],
    tx: i32,
    ty: i32,
    d: Direction,
) -> bool {
    let (dx, dy) = d.delta();
    let nx = wrap_col(tx + dx as i32);
    let ny = ty + dy as i32;
    pac_walkable(grid, nx, ny)
}

/// Advance Pac-Man one tick (movement + eating).
pub(crate) fn step_pacman(game: &mut Game, action: Action) {
    // Buffer the desired direction; it persists until executed or replaced.
    if let Some(d) = Direction::from_action(action) {
        game.desired = Some(d);
    }

    // Arcade dot-eating pause: Pac-Man stands still for 1 tick per dot
    // (3 per power pellet) right after eating it.
    if game.pac_pause > 0 {
        game.pac_pause -= 1;
        return;
    }

    let speed = pac_speed(game.level);
    let mut pos = (game.pacman.x, game.pacman.y);
    let mut dir = game.pacman.dir;
    let desired = game.desired;

    // Instant reversal (arcade: pressing the opposite direction turns
    // Pac-Man around immediately, mid-tile).
    if let Some(d) = desired {
        if d == dir.opposite() && d != dir {
            dir = d;
        }
    }

    // Cornering: apply a buffered perpendicular turn when the tile center
    // lies AHEAD within the corner-cut window (never snaps backward if the
    // center was already passed this or a previous tick).
    if let Some(d) = desired {
        if d != dir && d != dir.opposite() {
            let (tx, ty) = tile_coords(pos.0, pos.1);
            let (cx, cy) = tile_center(tx, ty);
            let (dvx, dvy) = dir.delta();
            let along = if dir.is_horizontal() {
                (cx - pos.0) * dvx
            } else {
                (cy - pos.1) * dvy
            };
            if along >= 0.0 && along <= CORNER_WINDOW && pac_can_enter(&game.grid, tx, ty, d) {
                if dir.is_horizontal() {
                    pos.0 = cx;
                } else {
                    pos.1 = cy;
                }
                dir = d;
            }
        }
    }

    let grid = game.grid;
    let (new_dir, stopped) = grid_move(&mut pos, dir, speed, &mut |tx, ty, cur| {
        // Decision at a tile center:
        // 1. buffered turn, if legal;
        // 2. else continue straight if legal;
        // 3. else stop (wall).
        if let Some(d) = desired {
            if d != cur {
                if pac_can_enter(&grid, tx, ty, d) {
                    return Some(d);
                }
            } else if pac_can_enter(&grid, tx, ty, cur) {
                return Some(cur);
            } else {
                return None;
            }
        }
        if pac_can_enter(&grid, tx, ty, cur) {
            Some(cur)
        } else {
            None
        }
    });
    dir = new_dir;

    game.pacman.x = pos.0;
    game.pacman.y = pos.1;
    game.pacman.dir = dir;

    // Mouth animation: chomp while moving (4-frame cycle).
    if !stopped && game.frames % 3 == 0 {
        game.pacman.mouth_phase = (game.pacman.mouth_phase + 1) % 4;
    }

    // Eating: whatever tile contains Pac-Man's center.
    let (tx, ty) = tile_coords(pos.0, pos.1);
    eat_at(game, tx, ty);
}

/// Consume the item (if any) at tile (tx, ty) and handle scoring/effects.
fn eat_at(game: &mut Game, tx: i32, ty: i32) {
    let idx = (ty as usize) * MAZE_W + wrap_col(tx) as usize;
    match game.grid[idx] {
        Tile::Dot => {
            game.grid[idx] = Tile::Empty;
            game.dots_eaten += 1;
            game.dots_remaining -= 1;
            game.pac_pause = 1;
            game.last_sound = SoundEvent::Waka;
            game.add_score(10); // dot = 10 points
            game.on_dot_eaten();
        }
        Tile::PowerPellet => {
            game.grid[idx] = Tile::Empty;
            game.dots_eaten += 1;
            game.dots_remaining -= 1;
            game.pac_pause = 3;
            game.last_sound = SoundEvent::PowerPellet;
            game.add_score(50); // power pellet = 50 points
            game.on_power_pellet();
        }
        _ => {}
    }

    // Fruit collision (fruit sits between columns 13/14 on row 20).
    let fruit = game.fruit;
    if fruit.active && ty == 20 && (tx == 13 || tx == 14) {
        let points = fruit_points(fruit.kind);
        game.fruit.active = false;
        game.bonus_timer_active = false;
        game.fruit.timer = 0;
        game.score_popup = Some((FRUIT_X, FRUIT_Y, points));
        game.popup_timer = 60;
        game.add_score(points);
        game.last_sound = SoundEvent::Fruit;
    }
    // Level complete?
    if game.dots_remaining == 0 && game.phase == GamePhase::Playing {
        game.fruit.active = false;
        game.bonus_timer_active = false;
        game.phase = GamePhase::LevelComplete;
        game.state_timer = super::game::LEVEL_COMPLETE_TICKS;
    }
}

/// Death animation bookkeeping (called by the Game state machine each tick
/// of the Dying phase): 30 pause ticks, then a 45-frame collapse.
pub(crate) fn step_death_anim(game: &mut Game) {
    let t = game.state_timer;
    if t < super::game::DYING_TICKS - 30 {
        game.pacman.dead_anim_frame = (super::game::DYING_TICKS - 30 - t) / 2 + 1;
    }
}
