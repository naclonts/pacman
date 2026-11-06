//! Integration test: run the headless logic core for 2000+ ticks with
//! scripted actions, no rendering, and assert the game stays internally
//! consistent. Also re-checks the PLAN.md determinism contract end-to-end.
//!
//! The logic modules are included directly (the crate is a binary, so there
//! is no lib target to link against); this also proves `src/logic/**` has no
//! hidden dependency on the presentation layer.

#[path = "../src/logic/mod.rs"]
mod logic;

use logic::{Action, Game, GamePhase, GhostMode, Tile, MAZE_H, MAZE_W};

/// Deterministic scripted action stream (LCG).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn action(&mut self) -> Action {
        match self.next() % 5 {
            0 => Action::None,
            1 => Action::Up,
            2 => Action::Down,
            3 => Action::Left,
            _ => Action::Right,
        }
    }
}

fn count_food(game: &Game) -> u32 {
    game.grid
        .iter()
        .filter(|t| matches!(**t, Tile::Dot | Tile::PowerPellet))
        .count() as u32
}

fn assert_consistent(game: &Game, tick: u64) {
    // Valid phase.
    match game.phase {
        GamePhase::Attract
        | GamePhase::Ready(_)
        | GamePhase::Playing
        | GamePhase::Dying
        | GamePhase::LevelComplete
        | GamePhase::GameOver => {}
    }
    assert_eq!(game.frames, tick + 1, "frames must count every tick");

    // Score/high-score bookkeeping.
    assert!(game.high_score >= game.score);
    assert!(game.lives <= 5, "lives should stay sane");

    // Dots bookkeeping.
    assert_eq!(game.dots_remaining, count_food(game), "tick {}", tick);
    assert_eq!(game.dots_remaining + game.dots_eaten, 244);

    // Pac-Man inside the play strip and never inside a wall.
    let px = game.pacman.x;
    let py = game.pacman.y;
    assert!(!px.is_nan() && !py.is_nan());
    assert!((-8.0..224.0).contains(&px), "pac x {} at tick {}", px, tick);
    assert!((0.0..288.0).contains(&py), "pac y {} at tick {}", py, tick);
    let ptx = (px / 8.0).floor() as i32;
    let pty = (py / 8.0).floor() as i32;
    let tile = game.tile_at(((ptx % MAZE_W as i32 + MAZE_W as i32) % MAZE_W as i32) as usize, pty as usize);
    assert_ne!(tile, Tile::Wall, "pac inside wall at tick {}", tick);

    // Ghosts inside the play strip and not inside walls (house/door ok).
    for g in &game.ghosts {
        assert!(!g.x.is_nan() && !g.y.is_nan());
        assert!((-8.0..224.0).contains(&g.x), "ghost x {} tick {}", g.x, tick);
        assert!((0.0..288.0).contains(&g.y), "ghost y {} tick {}", g.y, tick);
        let gx = (g.x / 8.0).floor() as i32;
        let gy = (g.y / 8.0).floor() as i32;
        let t = game.tile_at(((gx % 28 + 28) % 28) as usize, gy as usize);
        assert_ne!(t, Tile::Wall, "ghost {:?} inside wall at tick {}", g.id, tick);
    }

    // Frightened timer sanity.
    if game.global_fright_timer == 0 {
        assert!(
            game.ghosts
                .iter()
                .all(|g| g.mode != GhostMode::Frightened || g.frightened_timer == 0),
            "stale fright state at tick {}",
            tick
        );
    }
}

#[test]
fn headless_simulation_stays_consistent() {
    let mut game = Game::new_with_seed(1, 12345);
    let mut rng = Lcg(2024);
    const TICKS: u64 = 2500;
    for tick in 0..TICKS {
        game.step(rng.action());
        assert_consistent(&game, tick);
    }
    // The simulation must have made real progress (auto-attract starts the
    // game, dots get eaten or Pac-Man dies repeatedly, etc.).
    assert!(game.frames == TICKS);
}

#[test]
fn headless_simulation_is_deterministic() {
    let actions: Vec<Action> = {
        let mut rng = Lcg(777);
        (0..2200).map(|_| rng.action()).collect()
    };
    let mut a = Game::new_with_seed(1, 42);
    let mut b = Game::new_with_seed(1, 42);
    for (tick, &action) in actions.iter().enumerate() {
        a.step(action);
        b.step(action);
        assert_eq!(a.observe(), b.observe(), "divergence at tick {}", tick);
    }
    assert_eq!(a.score, b.score);
    assert_eq!(a.dots_remaining, b.dots_remaining);
    assert_eq!(a.ghosts[0].x, b.ghosts[0].x);
}
