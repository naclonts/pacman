//! Headless unit tests for the logic core, including the PLAN.md determinism
//! requirement: same seed + same action sequence => same state sequence.

use super::*;

/// Simple deterministic LCG for scripted test actions.
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

fn run(game: &mut Game, ticks: u32, actions: &mut Lcg) {
    for _ in 0..ticks {
        game.step(actions.action());
    }
}


/// Park all ghosts inside the house with release counters unreached so they
/// cannot interfere with movement tests.
fn park_ghosts(game: &mut Game) {
    game.use_global_counter = false;
    game.personal_counters = [0; 4];
    game.no_dot_timer = 0;
    for (i, g) in game.ghosts.iter_mut().enumerate() {
        g.mode = GhostMode::InHouse;
        g.frightened_timer = 0;
        g.y = 140.0;
        g.x = match i {
            1 => 112.0,
            2 => 96.0,
            _ => 128.0,
        };
    }
}

/// Start a game and skip the Attract + Ready phases.
fn into_playing(game: &mut Game) {
    for _ in 0..250 {
        game.step(Action::None); // Attract auto-starts, then 2 s Ready
    }
    assert_eq!(game.phase, GamePhase::Playing);
}

// ---------------------------------------------------------------------
// Maze
// ---------------------------------------------------------------------

#[test]
fn maze_has_exact_arcade_contents() {
    let grid = parse_maze();
    let dots = grid.iter().filter(|t| **t == Tile::Dot).count();
    let pellets = grid.iter().filter(|t| **t == Tile::PowerPellet).count();
    assert_eq!(dots, 240, "arcade level has exactly 240 dots");
    assert_eq!(pellets, 4, "arcade level has exactly 4 power pellets");
    // Tunnel row 17: outer columns are tunnel tiles.
    assert_eq!(grid[17 * 28], Tile::Tunnel);
    assert_eq!(grid[17 * 28 + 27], Tile::Tunnel);
    // Ghost house door at (13,15),(14,15).
    assert_eq!(grid[15 * 28 + 13], Tile::Door);
    assert_eq!(grid[15 * 28 + 14], Tile::Door);
    // Border wall.
    assert_eq!(grid[3 * 28], Tile::Wall);
}

// ---------------------------------------------------------------------
// Determinism (PLAN.md requirement)
// ---------------------------------------------------------------------

#[test]
fn same_seed_and_actions_produce_identical_state_sequence() {
    let mut a = Game::new_with_seed(1, 0xDEAD_BEEF);
    let mut b = Game::new_with_seed(1, 0xDEAD_BEEF);
    let mut rng = Lcg(42);
    for tick in 0..1200 {
        let action = rng.action();
        a.step(action);
        b.step(action);
        assert_eq!(a.observe(), b.observe(), "divergence at tick {}", tick);
        assert_eq!(a.frames, b.frames);
        assert_eq!(a.score, b.score);
        assert_eq!(a.dots_remaining, b.dots_remaining);
        assert_eq!(a.rng_state, b.rng_state);
        for i in 0..4 {
            assert_eq!(a.ghosts[i].x, b.ghosts[i].x, "ghost {} x, tick {}", i, tick);
            assert_eq!(a.ghosts[i].y, b.ghosts[i].y, "ghost {} y, tick {}", i, tick);
            assert_eq!(a.ghosts[i].mode, b.ghosts[i].mode);
        }
    }
}

#[test]
fn different_seeds_diverge() {
    let mut a = Game::new_with_seed(1, 1);
    let mut b = Game::new_with_seed(1, 2);
    let mut rng = Lcg(7);
    let mut diverged = false;
    for _ in 0..600 {
        let action = rng.action();
        a.step(action);
        b.step(action);
        if a.rng_state != b.rng_state {
            diverged = true;
            break;
        }
    }
    assert!(diverged, "different seeds should eventually diverge");
}

// ---------------------------------------------------------------------
// Phase flow
// ---------------------------------------------------------------------

#[test]
fn phase_flow_attract_ready_playing() {
    let mut game = Game::new(1);
    assert_eq!(game.phase, GamePhase::Attract);
    // Any input starts the game.
    game.step(Action::Right);
    assert!(matches!(game.phase, GamePhase::Ready(_)));
    // 2 s countdown.
    for _ in 0..120 {
        game.step(Action::None);
    }
    assert_eq!(game.phase, GamePhase::Playing);
    // Auto-advance also works from Attract without input: 120 attract
    // ticks + 120 Ready ticks.
    let mut idle = Game::new(1);
    for _ in 0..250 {
        idle.step(Action::None);
    }
    assert_eq!(idle.phase, GamePhase::Playing);
}

#[test]
fn game_over_after_losing_all_lives() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    for _ in 0..4 {
        // Force a death by dropping a ghost onto Pac-Man's exact position.
        let g = &mut game.ghosts[0];
        g.mode = GhostMode::Chase;
        g.x = game.pacman.x;
        g.y = game.pacman.y;
        game.step(Action::None);
        if game.lives == 0 {
            assert_eq!(game.phase, GamePhase::GameOver);
            // Any input starts a fresh game.
            game.step(Action::Right);
            assert!(matches!(game.phase, GamePhase::Ready(_)));
            return;
        }
        assert!(matches!(game.phase, GamePhase::Dying | GamePhase::Ready(_)));
        // Skip through Dying/Ready back to Playing (or reach GameOver).
        let mut guard = 0;
        while !matches!(game.phase, GamePhase::Playing | GamePhase::GameOver) && guard < 500 {
            game.step(Action::None);
            guard += 1;
        }
        if game.phase == GamePhase::GameOver {
            assert_eq!(game.lives, 0);
            game.step(Action::Right); // any input starts a fresh game
            assert!(matches!(game.phase, GamePhase::Ready(_)));
            return;
        }
        assert_eq!(game.phase, GamePhase::Playing);
    }
    panic!("never reached GameOver");
}

// ---------------------------------------------------------------------
// Pac-Man movement
// ---------------------------------------------------------------------

#[test]
fn pacman_moves_and_eats_dots_on_row_23() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    // Teleport onto the left end of the row-23 dot corridor (col 1).
    game.pacman.x = 12.0;
    game.pacman.y = (23 * 8 + 4) as f32;
    game.pacman.dir = Direction::Left;
    game.desired = Some(Direction::Right);
    let dots0 = game.dots_remaining;
    let eaten0 = game.dots_eaten;
    let score0 = game.score;
    for _ in 0..30 {
        park_ghosts(&mut game);
        game.step(Action::Right);
    }
    assert!(game.pacman.x > 20.0, "Pac-Man should move right, x={}", game.pacman.x);
    assert!(game.dots_remaining < dots0, "dots should be eaten");
    assert!(game.score > score0);
    assert_eq!(game.dots_eaten - eaten0, dots0 - game.dots_remaining);
}

#[test]
fn pacman_stops_at_wall() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    // Run right along row 23: the corridor is blocked by walls at columns
    // 13/14, so Pac-Man must clamp to the center of column 12 (x = 100).
    game.pacman.x = 12.0;
    game.pacman.y = (23 * 8 + 4) as f32;
    game.pacman.dir = Direction::Right;
    game.desired = Some(Direction::Right);
    for _ in 0..130 {
        park_ghosts(&mut game);
        game.step(Action::Right);
    }
    assert_eq!(game.phase, GamePhase::Playing);
    assert!((game.pacman.x - 100.0).abs() < 0.01, "x={}", game.pacman.x);
    let x = game.pacman.x;
    for _ in 0..30 {
        park_ghosts(&mut game);
        game.step(Action::Right);
    }
    assert!((game.pacman.x - x).abs() < 0.01, "must stay put against the wall");
}

#[test]
fn pacman_wraps_through_tunnel() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    // Stand at the tunnel-row dot (col 6), heading left into the tunnel.
    game.pacman.x = (6 * 8 + 4) as f32;
    game.pacman.y = (17 * 8 + 4) as f32;
    game.pacman.dir = Direction::Left;
    game.desired = Some(Direction::Left);
    for _ in 0..180 {
        park_ghosts(&mut game);
        game.step(Action::Left);
    }
    // After wrapping, Pac-Man travels the right-hand tunnel corridor and
    // stops at the center of column 18 (house wall at col 17).
    assert!((game.pacman.x - 148.0).abs() < 0.01, "x={}", game.pacman.x);
    let (tx, ty) = super::maze::tile_coords(game.pacman.x, game.pacman.y);
    assert!(super::maze::pac_walkable(&game.grid, tx, ty));
}

#[test]
fn pacman_turns_with_buffered_input() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    // Row 23 corridor, moving right; request Up — should turn at a tile
    // center where the tile above is open (e.g. column 4? row 22 above is
    // wall; column 9/18 have open tiles above at rows 22? Use column 1:
    // above row 23 col 1 is row 22 col 1 = wall... use col 21 (row 22 open)).
    game.pacman.x = (21 * 8 + 4) as f32; // column 21 center, row 23
    game.pacman.y = (23 * 8 + 4) as f32;
    game.pacman.dir = Direction::Right;
    game.desired = Some(Direction::Up);
    for _ in 0..10 {
        game.step(Action::None); // buffered Up stays latched
    }
    assert_eq!(game.pacman.dir, Direction::Up);
    let row23y = (23 * 8 + 4) as f32;
    assert!(game.pacman.y < row23y, "must have turned upward");
    assert!(game.pacman.y > row23y - 15.0, "but not too far");
}

// ---------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------

#[test]
fn power_pellet_frightens_and_ghost_eaten_scores() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    game.reset_level(); // fresh level so no pellet was eaten during setup
    park_ghosts(&mut game);
    // Blinky waits far away in the top-right corridor.
    let g = &mut game.ghosts[0];
    g.mode = GhostMode::Scatter;
    g.x = (26 * 8 + 4) as f32;
    g.y = (7 * 8 + 4) as f32;
    // Eat the top-left power pellet at (1,6): approach from below.
    game.pacman.x = 12.0;
    game.pacman.y = (7 * 8 + 4) as f32;
    game.pacman.dir = Direction::Up;
    game.desired = Some(Direction::Up);
    for _ in 0..20 {
        park_ghosts(&mut game);
        game.ghosts[0].mode = GhostMode::Scatter;
        game.ghosts[0].frightened_timer = 0;
        game.step(Action::Up);
        if game.global_fright_timer > 0 {
            break;
        }
    }
    // Pellet was eaten this tick: the timer already ticked down once.
    assert_eq!(super::game::fright_ticks(1), 360, "level 1 fright = 6 s");
    assert_eq!(game.global_fright_timer, 359);
    assert_eq!(game.ghost_combo, 0);
    assert_eq!(game.ghosts[0].mode, GhostMode::Frightened);

    // Drop Blinky onto Pac-Man: eaten for 200.
    let g = &mut game.ghosts[0];
    g.x = game.pacman.x;
    g.y = game.pacman.y;
    let score0 = game.score;
    game.step(Action::None);
    assert_eq!(game.ghosts[0].mode, GhostMode::Eaten);
    assert_eq!(game.score, score0 + 200);
    assert_eq!(game.ghost_combo, 1);

    // Let the arcade freeze elapse, then eat a second ghost: 400.
    for _ in 0..65 {
        game.step(Action::None);
        if game.freeze_timer == 0 {
            break;
        }
    }
    let g = &mut game.ghosts[1];
    g.mode = GhostMode::Frightened;
    g.frightened_timer = game.global_fright_timer;
    g.x = game.pacman.x;
    g.y = game.pacman.y;
    game.step(Action::None);
    assert_eq!(game.score, score0 + 600);
    assert_eq!(game.ghost_combo, 2);
}

#[test]
fn fruit_spawns_at_70_dots_and_scores() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    // White-box: pretend 69 dots have been eaten, then eat one more.
    game.dots_eaten = 69;
    game.dots_remaining -= 1; // keep the consistency (one dot was "eaten")
    // Eat a dot on row 23.
    game.pacman.x = 12.0;
    game.pacman.y = (23 * 8 + 4) as f32;
    game.pacman.dir = Direction::Right;
    game.desired = Some(Direction::Right);
    game.step(Action::Right);
    assert!(game.fruit.active, "fruit should spawn at 70 dots");
    assert_eq!(game.fruit.kind, FruitKind::Cherry);
    assert_eq!(game.fruit.timer, 540);

    // Walk into the fruit at (13/14, 20).
    game.pacman.x = 100.0;
    game.pacman.y = (20 * 8 + 4) as f32;
    game.pacman.dir = Direction::Right;
    game.desired = Some(Direction::Right);
    let score0 = game.score;
    for _ in 0..20 {
        game.step(Action::Right);
        if !game.fruit.active {
            break;
        }
    }
    assert!(!game.fruit.active);
    assert_eq!(game.score, score0 + 100, "cherry = 100");
    assert!(game.score_popup.is_some());
}

#[test]
fn extra_life_awarded_once_at_10000() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    game.score = 9990;
    game.pacman.x = 12.0;
    game.pacman.y = (23 * 8 + 4) as f32;
    game.pacman.dir = Direction::Right;
    game.desired = Some(Direction::Right);
    game.step(Action::Right); // +10 -> 10000
    assert_eq!(game.lives, 4);
    game.score = 15000;
    game.step(Action::Right);
    assert_eq!(game.lives, 4, "extra life only once");
}

// ---------------------------------------------------------------------
// Ghost house / modes
// ---------------------------------------------------------------------

#[test]
fn ghost_release_counters_level_1() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    // Blinky out, Pinky leaves immediately.
    assert_ne!(game.ghosts[0].mode, GhostMode::InHouse);
    let mut guard = 0;
    while game.ghosts[1].mode == GhostMode::InHouse && guard < 60 {
        game.step(Action::None);
        guard += 1;
    }
    assert_ne!(game.ghosts[1].mode, GhostMode::InHouse, "Pinky leaves at once");
    assert_eq!(game.ghosts[2].mode, GhostMode::InHouse, "Inky waits for 30 dots");
    // White-box: feed the global counter to 30.
    game.global_counter = 30;
    game.step(Action::None);
    assert_ne!(game.ghosts[2].mode, GhostMode::InHouse, "Inky released at 30");
    assert_eq!(game.ghosts[3].mode, GhostMode::InHouse, "Clyde waits for 60");
    game.global_counter = 60;
    game.step(Action::None);
    assert_ne!(game.ghosts[3].mode, GhostMode::InHouse, "Clyde released at 60");
}

#[test]
fn eaten_ghost_returns_home_and_exits() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    // Frighten, then eat Blinky next to the door area and watch him return.
    game.global_fright_timer = 300;
    let g = &mut game.ghosts[0];
    g.mode = GhostMode::Frightened;
    g.x = game.pacman.x;
    g.y = game.pacman.y;
    game.step(Action::None);
    assert_eq!(game.ghosts[0].mode, GhostMode::Eaten);
    // Park Pac-Man far away, then let the eyes fly home (long but bounded).
    game.pacman.x = 12.0;
    game.pacman.y = (23 * 8 + 4) as f32;
    game.desired = None;
    let mut was_eaten = false;
    let mut exit_mode = None;
    for _ in 0..1200 {
        game.step(Action::None);
        let g = game.ghosts[0];
        if g.mode == GhostMode::Eaten {
            was_eaten = true;
        }
        // Back above the door in a post-exit mode => exited again.
        if was_eaten
            && g.y <= 117.0
            && matches!(
                g.mode,
                GhostMode::Frightened | GhostMode::Scatter | GhostMode::Chase
            )
        {
            exit_mode = Some(g.mode);
            break;
        }
    }
    assert!(was_eaten, "ghost must have been eaten");
    assert!(exit_mode.is_some(), "eaten ghost must return to the house and exit again");
    // Arcade: revived during an active fright window, it comes out frightened.
    if game.global_fright_timer > 0 {
        assert_eq!(exit_mode, Some(GhostMode::Frightened));
    }
}

#[test]
fn scatter_chase_schedule_forces_reversal() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    assert_eq!(game.mode_index, 0, "game starts in scatter");
    let mut guard = 0;
    while game.mode_index == 0 && guard < 500 {
        game.step(Action::None);
        guard += 1;
    }
    // into_playing already consumed ~10 playing ticks of the phase.
    assert!((400..=420).contains(&guard), "first scatter lasts ~7 s: {}", guard);
    assert_eq!(game.mode_index, 1, "then switches to chase");
    assert_eq!(game.schedule_mode(), GhostMode::Chase);
}

#[test]
fn cruise_elroy_activates_on_dot_threshold() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    // Elroy needs Clyde out of the house.
    assert_eq!(super::ghosts::elroy_level(&game), 0);
    game.clyde_out = true;
    game.dots_remaining = 20; // elroy 1 threshold on level 1
    assert_eq!(super::ghosts::elroy_level(&game), 1);
    game.dots_remaining = 10; // elroy 2
    assert_eq!(super::ghosts::elroy_level(&game), 2);
    // Speed bump: Blinky faster than the normal level-1 ghost speed.
    let blinky = super::ghosts::ghost_speed(&game, GhostId::Blinky);
    let pinky = super::ghosts::ghost_speed(&game, GhostId::Pinky);
    assert!(blinky > pinky, "elroy Blinky must outspeed other ghosts");
}

#[test]
fn tunnel_slows_ghosts() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    let g = &mut game.ghosts[1];
    g.mode = GhostMode::Chase;
    g.x = 100.0;
    g.y = (17 * 8 + 4) as f32; // tunnel row
    g.in_tunnel = true;
    drop(g);
    let fast = super::ghosts::ghost_speed(&game, GhostId::Pinky);
    let g = &mut game.ghosts[1];
    g.in_tunnel = false;
    g.x = 112.0;
    drop(g);
    let normal = super::ghosts::ghost_speed(&game, GhostId::Pinky);
    assert!(fast < normal, "tunnel speed must be reduced");
}


#[test]
fn ghost_no_up_quirk_uses_current_tile() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    // Blinky standing inside restricted tile (12,25) may not decide to move
    // up out of it, even with Pac-Man far above (making Up the greedy pick).
    let g = &mut game.ghosts[0];
    g.mode = GhostMode::Chase;
    g.x = (12 * 8 + 4) as f32;
    g.y = (25 * 8 + 4) as f32;
    g.dir = Direction::Up;
    game.pacman.x = (12 * 8 + 4) as f32;
    game.pacman.y = (13 * 8 + 4) as f32;
    game.step(Action::None);
    let (tx, ty) = super::maze::tile_coords(game.ghosts[0].x, game.ghosts[0].y);
    assert!(
        !(tx == 12 && ty == 24),
        "ghost must not move up out of the restricted tile, now at ({},{})",
        tx,
        ty
    );
}

#[test]
fn pacman_has_no_up_restriction() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    park_ghosts(&mut game);
    // Pac-Man may move up through the restricted tiles (ghost-only quirk).
    game.pacman.x = (12 * 8 + 4) as f32;
    game.pacman.y = (26 * 8 + 4) as f32;
    game.pacman.dir = Direction::Up;
    game.desired = Some(Direction::Up);
    for _ in 0..20 {
        park_ghosts(&mut game);
        game.step(Action::Up);
    }
    let (tx, ty) = super::maze::tile_coords(game.pacman.x, game.pacman.y);
    assert!(ty <= 24, "pac must pass (12,25) upward, now at ({},{})", tx, ty);
}

// ---------------------------------------------------------------------
// Consistency
// ---------------------------------------------------------------------

#[test]
fn dots_remaining_matches_grid() {
    let mut game = Game::new(1);
    into_playing(&mut game);
    let mut rng = Lcg(99);
    run(&mut game, 900, &mut rng);
    let dots_in_grid = game
        .grid
        .iter()
        .filter(|t| matches!(**t, Tile::Dot | Tile::PowerPellet))
        .count() as u32;
    assert_eq!(dots_in_grid, game.dots_remaining);
}
