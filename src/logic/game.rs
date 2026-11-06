//! The `Game` state machine: Attract → Ready → Playing → Dying/LevelComplete
//! → GameOver, scoring, extra life at 10 000, fruit spawning, ghost mode
//! schedule, dot-counter house releases and sound-event bookkeeping.
//!
//! Timing constants (1 tick = 1/60 s):
//!   * READY: 120 ticks (2 s countdown)
//!   * DYING: 120 ticks (30 pause + 90 animation)
//!   * LEVEL_COMPLETE: 180 ticks of maze flash
//!   * FRUIT: 540 ticks (9 s) on the field; spawns at 70 and 170 dots eaten
//!   * ghost-eat freeze: 60 ticks (arcade ~1 s pause)
//!   * force-release: 300 ticks (5 s) without a dot eaten

use super::ghosts::{release_limit, GhostAi};
use super::maze::parse_maze;
use super::pacman::{
    fruit_kind, step_death_anim, step_pacman, FRUIT_X, FRUIT_Y, PAC_START,
};
use super::rng::DEFAULT_SEED;
use super::{
    Action, Direction, FruitState, GamePhase, GhostId, GhostMode, GhostState, PacmanState,
    SoundEvent, Tile, MAZE_H, MAZE_W, TICK_RATE_HZ,
};

pub(crate) const READY_TICKS: u32 = 120;
pub(crate) const DYING_TICKS: u32 = 120;
pub(crate) const LEVEL_COMPLETE_TICKS: u32 = 180;
pub(crate) const FRUIT_TICKS: u32 = 540; // 9 s
const GHOST_FREEZE_TICKS: u32 = 60;
const POPUP_TICKS: u32 = 60;
const FORCE_RELEASE_TICKS: u32 = 300; // 5 s without a dot
pub(crate) const TOTAL_DOTS: u32 = 244; // 240 dots + 4 power pellets

/// Scatter/chase phase durations in ticks per level bracket. Even indices are
/// Scatter phases, odd indices are Chase phases; `u32::MAX` = forever.
///   * level 1:   7s / 20s / 7s / 20s / 5s / 20s / 5s / forever
///   * level 2-4: 7s / 20s / 7s / 20s / 5s / forever
///   * level 5+:  5s / 20s / 5s / 20s / 5s / forever
fn schedule(level: u32) -> [u32; 8] {
    const FOREVER: u32 = u32::MAX;
    if level == 1 {
        [420, 1200, 420, 1200, 300, 1200, 300, FOREVER]
    } else if level <= 4 {
        [420, 1200, 420, 1200, 300, FOREVER, 0, 0]
    } else {
        [300, 1200, 300, 1200, 300, FOREVER, 0, 0]
    }
}

/// Frightened duration in ticks per level (arcade table in seconds; 0 = no
/// frightened mode from level 19 on).
pub(crate) fn fright_ticks(level: u32) -> u32 {
    const SECS: [u32; 19] = [
        6, 5, 4, 3, 2, 5, 2, 2, 1, 5, 2, 1, 5, 2, 1, 5, 2, 1, 0,
    ];
    let idx = ((level as usize).saturating_sub(1)).min(SECS.len() - 1);
    SECS[idx] * TICK_RATE_HZ as u32
}

#[derive(Clone, Debug)]
pub struct Game {
    // ----- public contract state ------------------------------------------
    pub phase: GamePhase,
    pub level: u32,
    pub score: u32,
    pub high_score: u32,
    pub lives: u32,
    pub dots_eaten: u32,
    pub dots_remaining: u32, // drives ghost-house releases & level end
    pub pacman: PacmanState,
    pub ghosts: [GhostState; 4], // index = GhostId as usize
    pub fruit: FruitState,
    pub grid: [Tile; MAZE_W * MAZE_H], // row-major, index = y * MAZE_W + x
    pub rng_state: u32,                // xorshift state
    pub global_fright_timer: u32,
    pub bonus_timer_active: bool,
    pub last_sound: SoundEvent, // overwritten each tick
    pub ghost_combo: u32,       // 200/400/800/1600 chain per frightened window
    pub score_popup: Option<(f32, f32, u32)>,
    pub frames: u64, // total ticks since Game::new

    // ----- internal state (not part of the render contract) ---------------
    pub(crate) state_timer: u32,
    pub(crate) desired: Option<Direction>, // buffered Pac-Man direction
    pub(crate) pac_pause: u32,             // dot-eating pause ticks
    pub(crate) mode_index: usize,          // scatter/chase schedule index
    pub(crate) mode_timer: u32,
    pub(crate) use_global_counter: bool, // level-start vs post-death releases
    pub(crate) global_counter: u32,
    pub(crate) personal_counters: [u32; 4],
    pub(crate) no_dot_timer: u32,
    pub(crate) clyde_out: bool,
    pub(crate) extra_life_given: bool,
    pub(crate) freeze_timer: u32,
    pub(crate) popup_timer: u32,
    pub(crate) ai: [GhostAi; 4],
}

impl Game {
    /// New game at `level` with the default seed.
    pub fn new(level: u32) -> Self {
        Self::new_with_seed(level, DEFAULT_SEED)
    }

    /// New game at `level` with an explicit RNG seed (determinism tests).
    pub fn new_with_seed(level: u32, seed: u32) -> Self {
        let level = level.max(1);
        let mut game = Game {
            phase: GamePhase::Attract,
            level,
            score: 0,
            high_score: 10000, // arcade boot high score
            lives: 3,
            dots_eaten: 0,
            dots_remaining: TOTAL_DOTS,
            pacman: PacmanState {
                x: PAC_START.0,
                y: PAC_START.1,
                dir: Direction::Left,
                mouth_phase: 0,
                dead_anim_frame: 0,
            },
            ghosts: [
                ghost_state(GhostId::Blinky),
                ghost_state(GhostId::Pinky),
                ghost_state(GhostId::Inky),
                ghost_state(GhostId::Clyde),
            ],
            fruit: FruitState {
                kind: fruit_kind(level),
                active: false,
                timer: 0,
                x: FRUIT_X,
                y: FRUIT_Y,
            },
            grid: parse_maze(),
            rng_state: if seed == 0 { 1 } else { seed },
            global_fright_timer: 0,
            bonus_timer_active: false,
            last_sound: SoundEvent::None_,
            ghost_combo: 0,
            score_popup: None,
            frames: 0,
            state_timer: 0,
            desired: None,
            pac_pause: 0,
            mode_index: 0,
            mode_timer: schedule(level)[0],
            use_global_counter: true,
            global_counter: 0,
            personal_counters: [0; 4],
            no_dot_timer: 0,
            clyde_out: false,
            extra_life_given: false,
            freeze_timer: 0,
            popup_timer: 0,
            ai: [GhostAi::new(), GhostAi::new(), GhostAi::new(), GhostAi::new()],
        };
        game.reset_level();
        game
    }

    /// Advance the simulation by exactly one tick.
    pub fn step(&mut self, action: Action) {
        self.frames += 1;
        self.last_sound = SoundEvent::None_;

        match self.phase {
            GamePhase::Attract => {
                self.state_timer += 1;
                // Auto-attract cycle, or start immediately on player input.
                if action != Action::None || self.state_timer >= 120 {
                    self.start_new_game();
                }
            }
            GamePhase::Ready(_) => {
                // Ghosts bob in the house while the countdown runs.
                super::ghosts::step_ghosts_house_only(self);
                self.state_timer = self.state_timer.saturating_sub(1);
                if self.state_timer == 0 {
                    self.phase = GamePhase::Playing;
                } else {
                    self.phase = GamePhase::Ready(self.state_timer);
                }
            }
            GamePhase::Playing => self.step_playing(action),
            GamePhase::Dying => {
                self.state_timer = self.state_timer.saturating_sub(1);
                step_death_anim(self);
                if self.state_timer == 0 {
                    self.lives -= 1;
                    if self.lives == 0 {
                        self.phase = GamePhase::GameOver;
                    } else {
                        self.reset_after_death();
                        self.phase = GamePhase::Ready(READY_TICKS);
                        self.state_timer = READY_TICKS;
                    }
                }
            }
            GamePhase::LevelComplete => {
                self.state_timer = self.state_timer.saturating_sub(1);
                if self.state_timer == 0 {
                    self.level += 1;
                    self.reset_level();
                    self.phase = GamePhase::Ready(READY_TICKS);
                    self.state_timer = READY_TICKS;
                }
            }
            GamePhase::GameOver => {
                // Any input restarts a fresh game.
                if action != Action::None {
                    self.level = 1;
                    self.start_new_game();
                }
            }
        }

        if self.score > self.high_score {
            self.high_score = self.score;
        }
    }

    /// State of one ghost by id (render/RL accessor).
    #[allow(dead_code)]
    pub fn ghost(&self, id: GhostId) -> GhostState {
        self.ghosts[id.index()]
    }

    /// Tile at grid coordinates (tunnel columns wrap; out-of-range is empty).
    pub fn tile_at(&self, tx: usize, ty: usize) -> Tile {
        if ty >= MAZE_H {
            return Tile::Empty;
        }
        self.grid[ty * MAZE_W + (tx % MAZE_W)]
    }

    /// Flat observation vector for RL use:
    /// `[pac_x, pac_y, pac_dir(4 one-hot), per-ghost (x, y, mode, fright
    /// timer, tunnel), fruit_active, fruit_timer, dots_remaining, level,
    /// score, fright_timer, rng_state]`
    #[allow(dead_code)]
    pub fn observe(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(2 + 4 + 4 * 5 + 6);
        v.push(self.pacman.x);
        v.push(self.pacman.y);
        for d in [Direction::Up, Direction::Down, Direction::Left, Direction::Right] {
            v.push(if self.pacman.dir == d { 1.0 } else { 0.0 });
        }
        for g in &self.ghosts {
            v.push(g.x);
            v.push(g.y);
            v.push(g.mode as usize as f32);
            v.push(g.frightened_timer as f32);
            v.push(if g.in_tunnel { 1.0 } else { 0.0 });
        }
        v.push(if self.fruit.active { 1.0 } else { 0.0 });
        v.push(self.fruit.timer as f32);
        v.push(self.dots_remaining as f32);
        v.push(self.level as f32);
        v.push(self.score as f32);
        v.push(self.global_fright_timer as f32);
        v.push(self.rng_state as f32);
        v
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    /// Current schedule mode (even index = scatter, odd = chase).
    pub(crate) fn schedule_mode(&self) -> GhostMode {
        if self.mode_index % 2 == 0 {
            GhostMode::Scatter
        } else {
            GhostMode::Chase
        }
    }

    /// Add points, awarding the one-time extra life at 10 000.
    pub(crate) fn add_score(&mut self, points: u32) {
        self.score += points;
        if !self.extra_life_given && self.score >= 10000 {
            self.extra_life_given = true;
            self.lives += 1;
            if self.last_sound == SoundEvent::None_ {
                self.last_sound = SoundEvent::ExtraLife;
            }
        }
    }

    /// Effects of eating a regular dot (called from the Pac-Man module).
    pub(crate) fn on_dot_eaten(&mut self) {
        self.no_dot_timer = 0;
        if self.use_global_counter {
            self.global_counter += 1;
        } else {
            // Post-death: only the next waiting ghost's personal counter grows.
            for id in [GhostId::Pinky, GhostId::Inky, GhostId::Clyde] {
                if self.ghosts[id.index()].mode == GhostMode::InHouse {
                    self.personal_counters[id.index()] += 1;
                    break;
                }
            }
        }
        // Bonus fruit appears at 70 and 170 dots eaten.
        if self.dots_eaten == 70 || self.dots_eaten == 170 {
            self.fruit.kind = fruit_kind(self.level);
            self.fruit.active = true;
            self.fruit.timer = FRUIT_TICKS;
            self.bonus_timer_active = true;
        }
    }

    /// Effects of eating a power pellet.
    pub(crate) fn on_power_pellet(&mut self) {
        let duration = fright_ticks(self.level);
        self.ghost_combo = 0;
        for i in 0..4 {
            let mode = self.ghosts[i].mode;
            let mut reverse = false;
            match mode {
                GhostMode::Scatter | GhostMode::Chase => {
                    if duration > 0 {
                        self.ghosts[i].mode = GhostMode::Frightened;
                    }
                    reverse = true;
                }
                _ => {}
            }
            if self.ghosts[i].mode != GhostMode::Eaten {
                self.ghosts[i].frightened_timer = duration;
            }
            self.ai[i].reverse_pending |= reverse;
        }
        self.global_fright_timer = duration;
    }

    /// One tick of the Playing phase.
    fn step_playing(&mut self, action: Action) {
        // Keep the input buffer fresh.
        if let Some(d) = Direction::from_action(action) {
            self.desired = Some(d);
        }

        // Score popup lifetime.
        if self.score_popup.is_some() {
            self.popup_timer = self.popup_timer.saturating_sub(1);
            if self.popup_timer == 0 {
                self.score_popup = None;
            }
        }

        // Arcade freeze after eating a ghost: everything holds still.
        if self.freeze_timer > 0 {
            self.freeze_timer -= 1;
            return;
        }

        // Fruit timer.
        if self.fruit.active {
            self.fruit.timer -= 1;
            if self.fruit.timer == 0 {
                self.fruit.active = false;
                self.bonus_timer_active = false;
            }
        }

        // Pac-Man moves and eats (may trigger LevelComplete).
        step_pacman(self, action);
        if self.phase != GamePhase::Playing {
            return;
        }

        // Frightened timer / scatter-chase schedule.
        if self.global_fright_timer > 0 {
            self.global_fright_timer -= 1;
            for g in &mut self.ghosts {
                g.frightened_timer = g.frightened_timer.saturating_sub(1);
            }
            if self.global_fright_timer == 0 {
                let m = self.schedule_mode();
                for g in &mut self.ghosts {
                    if g.mode == GhostMode::Frightened {
                        g.mode = m;
                        g.frightened_timer = 0;
                    }
                }
                self.ghost_combo = 0;
            }
        } else {
            self.advance_schedule();
        }

        self.update_releases();
        super::ghosts::step_ghosts(self);
        self.check_collisions();

        // Continuous siren (overwritten by any event above).
        if self.last_sound == SoundEvent::None_ {
            self.last_sound = if self.global_fright_timer > 0 {
                SoundEvent::Siren0
            } else {
                let tier = self.dots_eaten * 4 / TOTAL_DOTS;
                match tier {
                    0 => SoundEvent::Siren0,
                    1 => SoundEvent::Siren1,
                    2 => SoundEvent::Siren2,
                    _ => SoundEvent::Siren3,
                }
            };
        }
    }

    /// Advance the scatter/chase schedule, forcing reversals on switches.
    fn advance_schedule(&mut self) {
        let sched = schedule(self.level);
        if sched[self.mode_index] == u32::MAX {
            return; // final chase
        }
        self.mode_timer -= 1;
        if self.mode_timer == 0 {
            self.mode_index = (self.mode_index + 1).min(sched.len() - 1);
            self.mode_timer = sched[self.mode_index];
            for g in &mut self.ghosts {
                if matches!(g.mode, GhostMode::Scatter | GhostMode::Chase) {
                    self.ai[g.id.index()].reverse_pending = true;
                }
            }
        }
    }

    /// Dot-counter and timer-based ghost house releases.
    fn update_releases(&mut self) {
        // Force-release after 5 s without a dot.
        self.no_dot_timer += 1;
        if self.no_dot_timer >= FORCE_RELEASE_TICKS {
            self.no_dot_timer = 0;
            for id in [GhostId::Pinky, GhostId::Inky, GhostId::Clyde] {
                if self.ghosts[id.index()].mode == GhostMode::InHouse {
                    self.ghosts[id.index()].mode = GhostMode::LeavingHouse;
                    break;
                }
            }
        }
        // Counter-based release (one ghost per tick, priority order).
        for id in [GhostId::Pinky, GhostId::Inky, GhostId::Clyde] {
            let i = id.index();
            if self.ghosts[i].mode == GhostMode::InHouse {
                let count = if self.use_global_counter {
                    self.global_counter
                } else {
                    self.personal_counters[i]
                };
                if count >= release_limit(self.level, self.use_global_counter, id) {
                    self.ghosts[i].mode = GhostMode::LeavingHouse;
                    break;
                }
            }
        }
    }

    /// Ghost/Pac-Man collisions (arcade-style tile equality — including the
    /// famous pass-through possibility when both straddle a boundary).
    fn check_collisions(&mut self) {
        let pt = super::maze::tile_coords(self.pacman.x, self.pacman.y);
        for i in 0..4 {
            let g = self.ghosts[i];
            if !matches!(
                g.mode,
                GhostMode::Scatter | GhostMode::Chase | GhostMode::Frightened
            ) {
                continue;
            }
            let gt = super::maze::tile_coords(g.x, g.y);
            if pt != gt {
                continue;
            }
            if g.mode == GhostMode::Frightened {
                // Eat the ghost: 200/400/800/1600 chain.
                let points = 200u32 << self.ghost_combo.min(3);
                self.ghost_combo += 1;
                self.score_popup = Some((g.x, g.y, points));
                self.popup_timer = POPUP_TICKS;
                self.add_score(points);
                self.last_sound = SoundEvent::GhostEaten;
                self.freeze_timer = GHOST_FREEZE_TICKS;
                self.ghosts[i].mode = GhostMode::Eaten;
                self.ghosts[i].frightened_timer = 0;
                self.ai[i].entering = false;
            } else {
                // Death!
                self.phase = GamePhase::Dying;
                self.state_timer = DYING_TICKS;
                self.pacman.dead_anim_frame = 0;
                self.last_sound = SoundEvent::Death;
                return;
            }
        }
    }

    /// Shared reset of schedule/counters/effects (after death and per level).
    fn reset_run_state(&mut self) {
        self.mode_index = 0;
        self.mode_timer = schedule(self.level)[0];
        self.global_fright_timer = 0;
        self.ghost_combo = 0;
        self.desired = None;
        self.pac_pause = 0;
        self.freeze_timer = 0;
        self.score_popup = None;
        self.popup_timer = 0;
        self.no_dot_timer = 0;
        self.fruit.active = false;
        self.fruit.timer = 0;
        self.bonus_timer_active = false;
        for a in &mut self.ai {
            a.reverse_pending = false;
            a.entering = false;
        }
    }

    /// Reset actor positions/modes without touching the dots (after death).
    fn reset_after_death(&mut self) {
        self.reset_actors();
        self.reset_run_state();
        self.use_global_counter = false;
        self.global_counter = 0;
        // Post-death personal release counters: Pinky 7, Inky 17, Clyde 17.
        self.personal_counters = [0, 7, 17, 17];
        self.clyde_out = false;
    }

    /// Full level reset (fresh dots, fruit, schedule and actors).
    pub(crate) fn reset_level(&mut self) {
        self.grid = parse_maze();
        self.dots_eaten = 0;
        self.dots_remaining = TOTAL_DOTS;
        self.fruit.kind = fruit_kind(self.level);
        self.reset_actors();
        self.reset_run_state();
        self.use_global_counter = true;
        self.global_counter = 0;
        self.personal_counters = [0; 4];
        self.clyde_out = false;
    }

    /// Position and mode reset for Pac-Man and all ghosts.
    fn reset_actors(&mut self) {
        self.pacman.x = PAC_START.0;
        self.pacman.y = PAC_START.1;
        self.pacman.dir = Direction::Left;
        self.pacman.mouth_phase = 0;
        self.pacman.dead_anim_frame = 0;

        for (i, g) in self.ghosts.iter_mut().enumerate() {
            let id = ghost_index_id(i);
            g.id = id;
            g.frightened_timer = 0;
            g.in_tunnel = false;
            if id == GhostId::Blinky {
                // Blinky starts outside the house, above the door.
                g.x = super::ghosts::HOUSE_X;
                g.y = super::ghosts::DOOR_Y;
                g.dir = Direction::Left;
                g.mode = GhostMode::Scatter;
            } else {
                g.mode = GhostMode::InHouse;
                g.dir = Direction::Up;
                g.y = super::ghosts::HOUSE_Y;
                g.x = match id {
                    GhostId::Pinky => super::ghosts::HOUSE_X,
                    GhostId::Inky => 96.0,
                    GhostId::Clyde => 128.0,
                    _ => unreachable!(),
                };
            }
        }
    }

    /// Start a fresh game (from Attract or GameOver): score/lives reset, the
    /// level's dots restored, straight into the Ready countdown.
    fn start_new_game(&mut self) {
        self.score = 0;
        self.lives = 3;
        self.extra_life_given = false;
        self.reset_level();
        self.phase = GamePhase::Ready(READY_TICKS);
        self.state_timer = READY_TICKS;
    }
}

/// Ghost id by array index.
fn ghost_index_id(i: usize) -> GhostId {
    match i {
        0 => GhostId::Blinky,
        1 => GhostId::Pinky,
        2 => GhostId::Inky,
        _ => GhostId::Clyde,
    }
}

/// Initial ghost state (positions filled in by `reset_actors`).
fn ghost_state(id: GhostId) -> GhostState {
    GhostState {
        id,
        x: PAC_START.0,
        y: PAC_START.1,
        dir: Direction::Left,
        mode: GhostMode::InHouse,
        frightened_timer: 0,
        in_tunnel: false,
    }
}

