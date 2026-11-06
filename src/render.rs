// Presentation layer: macroquad renderer. Worker B owns this file.
//
// Renders the 224x288 game-pixel screen into an offscreen render target at
// 1 game pixel = 1 texel, then blits it to the window scaled up with nearest
// filtering and letterboxed. All drawing is procedural (no sprite assets).
// Reads game state read-only; the only mutation is via `game.step()`.
use crate::audio;
use crate::input;
use crate::logic::{
    Direction, FruitKind, Game, GamePhase, GhostId, GhostMode, GhostState, PacmanState, Tile,
    MAZE_H, MAZE_W, TICK_RATE_HZ,
};
use macroquad::prelude::*;

// ---------------------------------------------------------------------------
// Colors (original arcade palette)
// ---------------------------------------------------------------------------
const WALL_BLUE: Color = Color::from_rgba(0x21, 0x21, 0xff, 0xff);
const DOOR_PINK: Color = Color::from_rgba(0xff, 0xb8, 0xff, 0xff);
const DOT_PEACH: Color = Color::from_rgba(0xff, 0xb8, 0xae, 0xff);
const PAC_YELLOW: Color = Color::from_rgba(0xff, 0xff, 0x00, 0xff);
const FRUIT_RED: Color = Color::from_rgba(0xff, 0x00, 0x00, 0xff);
const SCORE_CYAN: Color = Color::from_rgba(0x00, 0xff, 0xde, 0xff);
const EYE_WHITE: Color = Color::from_rgba(0xff, 0xff, 0xff, 0xff);
const PUPIL_BLUE: Color = Color::from_rgba(0x21, 0x21, 0xff, 0xff);
const FRIGHT_BODY: Color = Color::from_rgba(0x21, 0x21, 0xff, 0xff);

fn ghost_color(id: GhostId) -> Color {
    match id {
        GhostId::Blinky => Color::from_rgba(0xff, 0x00, 0x00, 0xff),
        GhostId::Pinky => Color::from_rgba(0xff, 0xb8, 0xff, 0xff),
        GhostId::Inky => Color::from_rgba(0x00, 0xff, 0xff, 0xff),
        GhostId::Clyde => Color::from_rgba(0xff, 0xb8, 0x52, 0xff),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Sync entry point called from `main`. Boots the macroquad window and drives
/// the fixed-timestep game loop inside its event loop.
pub fn run(game: Game) {
    let mut mq_conf = macroquad::miniquad::conf::Conf::default();
    mq_conf.window_width = 672; // 224 * 3
    mq_conf.window_height = 864; // 288 * 3
    mq_conf.window_title = "PAC-MAN".to_string();
    mq_conf.window_resizable = true;
    let conf = macroquad::conf::Conf {
        miniquad_conf: mq_conf,
        ..Default::default()
    };
    macroquad::Window::from_config(conf, game_loop(game));
}

// ---------------------------------------------------------------------------
// Fixed-timestep game loop
// ---------------------------------------------------------------------------

async fn game_loop(mut game: Game) {
    // Offscreen target at exact game resolution (224x288, 8 px per tile).
    let rt = render_target((MAZE_W * 8) as u32, (MAZE_H * 8) as u32);
    rt.texture.set_filter(FilterMode::Nearest);
    let mut cam = Camera2D::from_display_rect(Rect::new(
        0.0,
        0.0,
        (MAZE_W * 8) as f32,
        (MAZE_H * 8) as f32,
    ));
    cam.render_target = Some(rt.clone());

    // The maze layout is constant for the whole run: precompute wall segments.
    let maze = build_maze_shape(&game.grid);
    let mut audio = audio::Audio::new().await;
    let mut acc = 0.0f32;
    let step_len = 1.0 / TICK_RATE_HZ as f32;

    loop {
        // --- fixed 60 Hz logic ticks --------------------------------------
        acc += get_frame_time().min(0.1); // clamp to avoid spiral of death
        let action = input::poll_action(); // sample keys once per frame
        let mut steps = 0;
        while acc >= step_len && steps < 6 {
            game.step(action);
            audio.update(game.last_sound);
            acc -= step_len;
            steps += 1;
        }
        if acc > step_len {
            acc = 0.0; // drop backlog rather than fast-forward
        }

        // --- draw into the 224x288 target ---------------------------------
        set_camera(&cam);
        clear_background(BLACK);
        draw_world(&game, &maze);

        // --- blit to window, scaled + letterboxed --------------------------
        set_default_camera();
        clear_background(BLACK);
        let scale = (screen_width() / (MAZE_W * 8) as f32)
            .min(screen_height() / (MAZE_H * 8) as f32);
        let w = (MAZE_W * 8) as f32 * scale;
        let h = (MAZE_H * 8) as f32 * scale;
        draw_texture_ex(
            &rt.texture,
            (screen_width() - w) * 0.5,
            (screen_height() - h) * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(w, h)),
                flip_y: true, // render targets come out upside down otherwise
                ..Default::default()
            },
        );

        next_frame().await;
    }
}

// ---------------------------------------------------------------------------
// Maze geometry (precomputed)
// ---------------------------------------------------------------------------

struct MazeShape {
    wall_segs: Vec<Rect>,
    door_segs: Vec<Rect>,
}

/// For every wall tile, emit 2px-thick line segments on each side facing a
/// non-wall tile (out-of-bounds counts as open, so the outer double border is
/// drawn). Consecutive wall tiles merge their segments, and segments overlap
/// at corners, producing the classic continuous double-line outlines: a
/// one-tile-thick wall renders as two parallel blue lines with black between.
fn build_maze_shape(grid: &[Tile; MAZE_W * MAZE_H]) -> MazeShape {
    let open = |tx: i32, ty: i32| -> bool {
        if tx < 0 || tx >= MAZE_W as i32 || ty < 0 || ty >= MAZE_H as i32 {
            return true; // outside counts as open -> border lines get drawn
        }
        grid[ty as usize * MAZE_W + tx as usize] == Tile::Wall
    };
    let mut wall_segs = Vec::new();
    let mut door_segs = Vec::new();
    for ty in 0..MAZE_H {
        for tx in 0..MAZE_W {
            let tile = grid[ty * MAZE_W + tx];
            let x = (tx * 8) as f32;
            let y = (ty * 8) as f32;
            match tile {
                Tile::Wall => {
                    let (up, down, left, right) = (
                        open(tx as i32, ty as i32 - 1),
                        open(tx as i32, ty as i32 + 1),
                        open(tx as i32 - 1, ty as i32),
                        open(tx as i32 + 1, ty as i32),
                    );
                    if up {
                        wall_segs.push(Rect::new(x, y + 1.0, 8.0, 2.0));
                    }
                    if down {
                        wall_segs.push(Rect::new(x, y + 5.0, 8.0, 2.0));
                    }
                    if left {
                        wall_segs.push(Rect::new(x + 1.0, y, 2.0, 8.0));
                    }
                    if right {
                        wall_segs.push(Rect::new(x + 5.0, y, 2.0, 8.0));
                    }
                }
                Tile::Door => {
                    door_segs.push(Rect::new(x, y + 3.0, 8.0, 2.0));
                }
                _ => {}
            }
        }
    }
    MazeShape { wall_segs, door_segs }
}

// ---------------------------------------------------------------------------
// World drawing
// ---------------------------------------------------------------------------

fn draw_world(game: &Game, maze: &MazeShape) {
    let flash = game.phase == GamePhase::LevelComplete && (game.frames / 8) % 2 == 0;
    let wall_col = if flash { EYE_WHITE } else { WALL_BLUE };

    // Maze
    for r in &maze.wall_segs {
        draw_rectangle(r.x, r.y, r.w, r.h, wall_col);
    }
    for r in &maze.door_segs {
        draw_rectangle(r.x, r.y, r.w, r.h, DOOR_PINK);
    }

    // Dots & power pellets
    let pellets_on = (game.frames / 16) % 2 == 0;
    for ty in 0..MAZE_H {
        for tx in 0..MAZE_W {
            let cx = (tx * 8 + 4) as f32;
            let cy = (ty * 8 + 4) as f32;
            match game.tile_at(tx, ty) {
                Tile::Dot => draw_rectangle(cx - 1.0, cy - 1.0, 2.0, 2.0, DOT_PEACH),
                Tile::PowerPellet => {
                    if pellets_on {
                        draw_circle(cx, cy, 3.5, DOT_PEACH);
                    }
                }
                _ => {}
            }
        }
    }

    // Bonus fruit on the board
    if game.fruit.active {
        draw_fruit_icon(game.fruit.kind, game.fruit.x, game.fruit.y);
    }

    // Ghost score popups (200/400/800/1600 shown in cyan where a ghost died)
    if let Some((px, py, pts)) = game.score_popup {
        let pts = pts as u32; // contract: u32; cast keeps inference unambiguous
        let text = pts.to_string();
        let dims = measure_text(&text, None, 8, 1.0);
        draw_text(&text, px - dims.width / 2.0, py + 3.0, 8.0, SCORE_CYAN);
    }

    // Pac-Man (hidden ghosts during the death animation, as in the arcade)
    let dying = game.phase == GamePhase::Dying;
    if dying {
        draw_pacman_death(&game.pacman);
    } else {
        draw_pacman(&game.pacman);
        for g in &game.ghosts {
            draw_ghost(g, game.frames);
        }
    }

    // HUD
    draw_hud(game);
}

// ---------------------------------------------------------------------------
// Pac-Man
// ---------------------------------------------------------------------------

fn dir_angle(d: Direction) -> f32 {
    match d {
        Direction::Right => 0.0,
        Direction::Down => std::f32::consts::FRAC_PI_2,
        Direction::Left => std::f32::consts::PI,
        Direction::Up => -std::f32::consts::FRAC_PI_2,
    }
}

/// Yellow circle with a black wedge cut for the mouth, rotated to `dir`.
fn wedge(x: f32, y: f32, r: f32, angle: f32, half_mouth: f32, col: Color) {
    draw_circle(x, y, r, col);
    if half_mouth > 0.01 {
        let a = angle;
        let p1 = vec2(x + r * 1.2 * (a + half_mouth).cos(), y + r * 1.2 * (a + half_mouth).sin());
        let p2 = vec2(x + r * 1.2 * (a - half_mouth).cos(), y + r * 1.2 * (a - half_mouth).sin());
        draw_triangle(vec2(x, y), p1, p2, BLACK);
    }
}

fn draw_pacman(p: &PacmanState) {
    // mouth_phase 0..=3: open -> mid -> closed -> mid
    let half = match p.mouth_phase {
        0 => 0.55,
        1 => 0.28,
        2 => 0.02,
        _ => 0.28,
    };
    wedge(p.x, p.y, 7.0, dir_angle(p.dir), half, PAC_YELLOW);
}

/// Death animation: the mouth wedge widens every frame until Pac-Man
/// disappears entirely.
fn draw_pacman_death(p: &PacmanState) {
    let half = 0.25 + p.dead_anim_frame as f32 * 0.14;
    if half >= std::f32::consts::PI {
        return; // fully "eaten" — nothing left to draw
    }
    wedge(p.x, p.y, 7.0, -std::f32::consts::FRAC_PI_2, half, PAC_YELLOW);
}

// ---------------------------------------------------------------------------
// Ghosts
// ---------------------------------------------------------------------------

fn draw_ghost(g: &GhostState, frames: u64) {
    let x = g.x;
    let y = g.y;
    let frightened = g.mode == GhostMode::Frightened;
    let eaten = g.mode == GhostMode::Eaten;

    // Eyes only while returning home after being eaten.
    if eaten {
        draw_eyes(x, y, g.dir);
        return;
    }

    let flashing = frightened && g.frightened_timer < 120 && (g.frightened_timer / 10) % 2 == 0;
    let body_col = if frightened {
        if flashing { EYE_WHITE } else { FRIGHT_BODY }
    } else {
        ghost_color(g.id)
    };

    // Dome body: half circle + straight sides down to the skirt line.
    draw_circle(x, y - 1.5, 6.5, body_col);
    draw_rectangle(x - 6.5, y - 1.5, 13.0, 6.5, body_col);

    // Wavy skirt: alternating teeth, two animation frames.
    let skirt_y = y + 5.0;
    let tooth_w = 13.0 / 3.0;
    let frame = ((frames / 8) % 2) as usize;
    for i in 0..3 {
        let tx0 = x - 6.5 + i as f32 * tooth_w;
        if (i + frame) % 2 == 0 {
            draw_triangle(
                vec2(tx0, skirt_y),
                vec2(tx0 + tooth_w, skirt_y),
                vec2(tx0 + tooth_w / 2.0, skirt_y + 2.5),
                body_col,
            );
        } else {
            // notch: black triangle biting up into the skirt
            draw_triangle(
                vec2(tx0, skirt_y),
                vec2(tx0 + tooth_w, skirt_y),
                vec2(tx0 + tooth_w / 2.0, skirt_y - 2.5),
                BLACK,
            );
        }
    }

    if frightened {
        draw_frightened_face(x, y, flashing);
    } else {
        draw_eyes(x, y, g.dir);
    }
}

fn draw_eyes(x: f32, y: f32, dir: Direction) {
    let (dx, dy) = match dir {
        Direction::Up => (0.0, -1.2),
        Direction::Down => (0.0, 1.2),
        Direction::Left => (-1.2, 0.0),
        Direction::Right => (1.2, 0.0),
    };
    for ex in [x - 2.8, x + 2.8] {
        draw_circle(ex, y - 2.5, 2.4, EYE_WHITE);
        draw_circle(ex + dx, y - 2.5 + dy, 1.3, PUPIL_BLUE);
    }
}

fn draw_frightened_face(x: f32, y: f32, flashing: bool) {
    let face = if flashing { FRUIT_RED } else { EYE_WHITE };
    // eyes: two small squares
    draw_rectangle(x - 4.2, y - 3.5, 2.4, 2.4, face);
    draw_rectangle(x + 1.8, y - 3.5, 2.4, 2.4, face);
    // wavy mouth: zigzag segments
    let y0 = y + 2.0;
    for (i, sx) in [x - 5.0, x - 2.5, x, x + 2.5].iter().enumerate() {
        let sx = *sx;
        let up = i % 2 == 0;
        let dy = if up { -1.5 } else { 1.5 };
        draw_line(sx, y0 - dy, sx + 2.5, y0 + dy, 1.0, face);
    }
}

// ---------------------------------------------------------------------------
// HUD
// ---------------------------------------------------------------------------

fn centered_text(text: &str, cx: f32, baseline: f32, size: f32, col: Color) {
    let dims = measure_text(text, None, size as u16, 1.0);
    draw_text(text, cx - dims.width / 2.0, baseline, size, col);
}

fn draw_hud(game: &Game) {
    // Top labels + scores
    draw_text("1UP", 26.0, 12.0, 8.0, EYE_WHITE);
    centered_text("HIGH SCORE", 112.0, 12.0, 8.0, EYE_WHITE);
    let score = game.score.to_string();
    let dims = measure_text(&score, None, 8, 1.0);
    draw_text(&score, 40.0 - dims.width, 22.0, 8.0, EYE_WHITE);
    let high = game.high_score.to_string();
    centered_text(&high, 112.0, 22.0, 8.0, EYE_WHITE);

    // Lives as mini pac icons, bottom-left (facing left, like the arcade)
    let n = game.lives.min(5) as i32;
    for i in 0..n {
        let cx = 14.0 + i as f32 * 16.0;
        wedge(cx, 281.0, 5.0, std::f32::consts::PI, 0.5, PAC_YELLOW);
    }

    // Recent-level fruit trail, bottom-right
    let first = game.level.saturating_sub(3).max(1);
    for lvl in first..=game.level {
        let i = lvl - first;
        draw_fruit_icon(fruit_for_level(lvl), 210.0 - i as f32 * 16.0, 281.0);
    }

    // Phase text
    match game.phase {
        GamePhase::Ready(_) => centered_text("READY!", 112.0, 169.0, 8.0, PAC_YELLOW),
        GamePhase::GameOver => centered_text("GAME  OVER", 112.0, 169.0, 8.0, FRUIT_RED),
        _ => {}
    }
}

fn fruit_for_level(level: u32) -> FruitKind {
    match level {
        1 => FruitKind::Cherry,
        2 => FruitKind::Strawberry,
        3 | 4 => FruitKind::Orange,
        5 | 6 => FruitKind::Apple,
        7 | 8 => FruitKind::Melon,
        9 | 10 => FruitKind::Galaxian,
        11 | 12 => FruitKind::Bell,
        _ => FruitKind::Key,
    }
}

// ---------------------------------------------------------------------------
// Procedural fruit icons (8x8-ish, centered at cx, cy)
// ---------------------------------------------------------------------------

fn draw_fruit_icon(kind: FruitKind, cx: f32, cy: f32) {
    match kind {
        FruitKind::Cherry => {
            draw_circle(cx - 2.0, cy + 2.0, 2.2, FRUIT_RED);
            draw_circle(cx + 2.5, cy + 1.0, 2.2, FRUIT_RED);
            draw_line(cx + 3.0, cy + 0.5, cx + 0.5, cy - 3.5, 1.0, Color::from_rgba(0x3f, 0xb7, 0x3f, 0xff));
            draw_line(cx - 2.0, cy - 0.5, cx + 0.5, cy - 3.5, 1.0, Color::from_rgba(0x3f, 0xb7, 0x3f, 0xff));
        }
        FruitKind::Strawberry => {
            draw_circle(cx, cy + 0.5, 3.2, FRUIT_RED);
            draw_rectangle(cx - 2.0, cy - 3.5, 4.0, 2.0, Color::from_rgba(0x3f, 0xb7, 0x3f, 0xff));
            draw_rectangle(cx - 1.0, cy, 1.0, 1.0, EYE_WHITE);
            draw_rectangle(cx + 0.5, cy - 1.0, 1.0, 1.0, EYE_WHITE);
            draw_rectangle(cx - 2.0, cy + 1.5, 1.0, 1.0, EYE_WHITE);
        }
        FruitKind::Orange => {
            draw_circle(cx, cy + 0.5, 3.4, Color::from_rgba(0xff, 0xa5, 0x00, 0xff));
            draw_rectangle(cx - 0.5, cy - 4.0, 1.5, 2.0, Color::from_rgba(0x3f, 0xb7, 0x3f, 0xff));
        }
        FruitKind::Apple => {
            draw_circle(cx, cy + 0.5, 3.4, FRUIT_RED);
            draw_line(cx, cy - 3.0, cx + 1.0, cy - 4.5, 1.0, Color::from_rgba(0x8b, 0x45, 0x13, 0xff));
        }
        FruitKind::Melon => {
            draw_circle(cx, cy, 3.8, Color::from_rgba(0x7c, 0xfc, 0x00, 0xff));
            draw_line(cx - 2.0, cy - 2.5, cx - 2.0, cy + 2.5, 1.0, Color::from_rgba(0x2e, 0x8b, 0x2e, 0xff));
            draw_line(cx + 2.0, cy - 2.5, cx + 2.0, cy + 2.5, 1.0, Color::from_rgba(0x2e, 0x8b, 0x2e, 0xff));
        }
        FruitKind::Galaxian => {
            draw_triangle(
                vec2(cx - 3.5, cy + 3.0),
                vec2(cx + 3.5, cy + 3.0),
                vec2(cx, cy - 3.5),
                Color::from_rgba(0x00, 0xbf, 0xff, 0xff),
            );
            draw_rectangle(cx - 3.5, cy, 7.0, 1.5, FRUIT_RED);
        }
        FruitKind::Bell => {
            draw_triangle(
                vec2(cx - 3.5, cy + 2.5),
                vec2(cx + 3.5, cy + 2.5),
                vec2(cx, cy - 3.5),
                Color::from_rgba(0xff, 0xd7, 0x00, 0xff),
            );
            draw_rectangle(cx - 3.5, cy + 2.5, 7.0, 1.2, EYE_WHITE);
        }
        FruitKind::Key => {
            draw_circle(cx - 2.0, cy - 2.0, 2.0, Color::from_rgba(0xc0, 0xc0, 0xc0, 0xff));
            draw_rectangle(cx - 1.0, cy - 0.5, 1.5, 5.0, Color::from_rgba(0xc0, 0xc0, 0xc0, 0xff));
            draw_rectangle(cx - 0.5, cy + 3.0, 2.0, 1.0, Color::from_rgba(0xc0, 0xc0, 0xc0, 0xff));
        }
    }
}
