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
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Colors (original arcade palette)
// ---------------------------------------------------------------------------
const WALL_BLUE: Color = Color::from_rgba(0x21, 0x21, 0xff, 0xff);
const DOOR_PINK: Color = Color::from_rgba(0xff, 0xb8, 0xff, 0xff);
const DOT_PEACH: Color = Color::from_rgba(0xfe, 0xbd, 0xb4, 0xff);
const PAC_YELLOW: Color = Color::from_rgba(0xff, 0xff, 0x00, 0xff);
const FRUIT_RED: Color = Color::from_rgba(0xff, 0x00, 0x00, 0xff);
const SCORE_CYAN: Color = Color::from_rgba(0x00, 0xff, 0xde, 0xff);
const EYE_WHITE: Color = Color::from_rgba(0xff, 0xff, 0xff, 0xff);
const PUPIL_BLUE: Color = Color::from_rgba(0x21, 0x21, 0xff, 0xff);
const FRIGHT_BODY: Color = Color::from_rgba(0x21, 0x21, 0xff, 0xff);
const STEM_GREEN: Color = Color::from_rgba(0x3f, 0xb7, 0x3f, 0xff);
const STEM_BROWN: Color = Color::from_rgba(0x8b, 0x45, 0x13, 0xff);

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
    // Screenshot hook for automated testing: if PACMAN_SCREENSHOT is set to a
    // path, capture the scaled-up window image mid-gameplay and exit once.
    let screenshot_path = std::env::var("PACMAN_SCREENSHOT").ok();

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

    // The maze layout is constant for the whole run: precompute the wall
    // outlines (continuous rounded boundary loops).
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

        // --- optional one-shot screenshot (mid-gameplay) -------------------
        if let Some(path) = &screenshot_path {
            if game.phase == GamePhase::Playing && game.frames >= 240 {
                let _ = get_screen_data().export_png(path);
                std::process::exit(0);
            }
        }

        next_frame().await;
    }
}

// ---------------------------------------------------------------------------
// Maze geometry (precomputed)
// ---------------------------------------------------------------------------

struct MazeShape {
    /// Straight outline runs (already offset into the wall; two parallel
    /// lines per boundary — the classic double-line outline).
    lines: Vec<[Vec2; 2]>,
    /// Rounded corners: (center, radius, start angle, sweep angle).
    arcs: Vec<(Vec2, f32, f32, f32)>,
    door_segs: Vec<Rect>,
}

/// Outline stroke thickness (each of the two parallel lines is 1 px wide).
const WALL_LINE_W: f32 = 1.0;

/// The classic double-line outline: for each wall boundary we draw two
/// parallel 1 px lines. Each entry is `(centerline offset from the
/// wall-region boundary in px, corner fillet radius in px)` — the outer
/// line gets the big, clearly visible quarter-circle corners, the inner
/// line a smaller one, exactly like the arcade maze.
const WALL_OUTLINES: [(f32, f32); 2] = [(0.5, 3.0), (2.5, 1.5)];

/// Traces the boundaries of all wall regions and converts them into
/// continuous rounded outlines (the classic double-line arcade look: every
/// wall region gets an outlined border on each side facing open space, with
/// filleted corners; thin walls end up outlined on both faces).
///
/// Method: collect the directed boundary edges between wall and non-wall
/// tiles (oriented so the wall region is always on the left of travel),
/// chain them into closed loops, then render each loop twice (once per
/// outline line) as an offset polyline with circular-arc fillets at every
/// 90-degree turn — convex corners get a true quarter-circle fillet, so
/// every wall corner reads as rounded like the arcade. Diagonally adjacent
/// wall tiles share a pinch vertex and their outlines connect smoothly.
fn build_maze_shape(grid: &[Tile; MAZE_W * MAZE_H]) -> MazeShape {
    let is_wall = |tx: i32, ty: i32| -> bool {
        tx >= 0
            && tx < MAZE_W as i32
            && ty >= 0
            && ty < MAZE_H as i32
            && grid[ty as usize * MAZE_W + tx as usize] == Tile::Wall
    };

    // Directed boundary edges: (start vertex, unit direction). Vertex (i, j)
    // is the tile corner at pixel (i*8, j*8). Each edge is oriented so the
    // wall tile lies on the left of the travel direction.
    let mut edges: HashSet<((i32, i32), (i32, i32))> = HashSet::new();
    for ty in 0..MAZE_H {
        for tx in 0..MAZE_W {
            if grid[ty * MAZE_W + tx] != Tile::Wall {
                continue;
            }
            let (tx, ty) = (tx as i32, ty as i32);
            if !is_wall(tx, ty - 1) {
                edges.insert(((tx + 1, ty), (-1, 0))); // top face
            }
            if !is_wall(tx, ty + 1) {
                edges.insert(((tx, ty + 1), (1, 0))); // bottom face
            }
            if !is_wall(tx - 1, ty) {
                edges.insert(((tx, ty), (0, 1))); // left face
            }
            if !is_wall(tx + 1, ty) {
                edges.insert(((tx + 1, ty + 1), (0, -1))); // right face
            }
        }
    }

    // Chain edges into closed loops. At vertices where four edges meet
    // (diagonal wall pinch points) pick the sharpest turn so each diagonal
    // pair stays part of the same visual outline.
    let dirs = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)];
    let mut visited: HashSet<((i32, i32), (i32, i32))> = HashSet::new();
    let mut loops: Vec<Vec<((i32, i32), (i32, i32))>> = Vec::new();
    for &start in &edges {
        if visited.contains(&start) {
            continue;
        }
        let mut lp = Vec::new();
        let mut cur = start;
        loop {
            visited.insert(cur);
            lp.push(cur);
            let ((vx, vy), (dx, dy)) = cur;
            // Choose the outgoing edge at the head vertex with the sharpest
            // turn (minimum cross product); if it is already visited the
            // loop is closed.
            let mut best: Option<((i32, i32), (i32, i32))> = None;
            let mut best_cross = i32::MAX;
            for &nd in &dirs {
                let cand = ((vx + dx, vy + dy), nd);
                if edges.contains(&cand) {
                    let cross = dx * nd.1 - dy * nd.0;
                    if cross < best_cross {
                        best_cross = cross;
                        best = Some(cand);
                    }
                }
            }
            match best {
                Some(next) if !visited.contains(&next) => cur = next,
                _ => break,
            }
        }
        loops.push(lp);
    }

    // Render each loop: two offset outlines, straight runs trimmed back
    // where the corner fillets tangent onto them + circular fillet arcs at
    // every corner, so lines and arcs join with no gaps or overshoot.
    let px = |v: (i32, i32)| vec2(v.0 as f32 * 8.0, v.1 as f32 * 8.0);
    let wall_normal = |d: (i32, i32)| vec2(d.1 as f32, -(d.0 as f32)); // left of travel
    let cross = |a: (i32, i32), b: (i32, i32)| a.0 * b.1 - a.1 * b.0;
    let mut lines = Vec::new();
    let mut arcs = Vec::new();
    for lp in &loops {
        let n = lp.len();
        for &(off, rad) in &WALL_OUTLINES {
            for (i, &((vx, vy), d)) in lp.iter().enumerate() {
                let nrm = wall_normal(d);
                let head = (vx + d.0, vy + d.1);
                let p0 = px((vx, vy)) + nrm * off;
                let p1 = px(head) + nrm * off;
                let (_, pd) = lp[(i + n - 1) % n];
                let (_, nd) = lp[(i + 1) % n];
                let turn_in = pd != d;
                let turn_out = nd != d;
                // Distance back from a corner where the fillet arc tangent
                // meets this edge (convex corner: off + rad; concave: the
                // arc simply bridges the two offset points, radius = off).
                let trim = |convex: bool| if convex { off + rad } else { (off - rad).max(0.0) };
                let t_in = if turn_in { trim(cross(pd, d) < 0) } else { 0.0 };
                let t_out = if turn_out { trim(cross(d, nd) < 0) } else { 0.0 };
                let dir_f = vec2(d.0 as f32, d.1 as f32);
                // Straight run along this edge, offset toward the wall side,
                // trimmed to the fillet tangent points on both ends.
                if t_in + t_out <= 8.0 {
                    let start = if turn_in { p0 + dir_f * t_in } else { p0 };
                    let end = if turn_out { p1 - dir_f * t_out } else { p1 };
                    lines.push([start, end]);
                }
                // Fillet arc at the head corner if the path turns.
                if turn_out {
                    let c = px(head);
                    let n_in = nrm;
                    let n_out = wall_normal(nd);
                    let (center, radius, pin, pout) = if cross(d, nd) < 0 {
                        // Convex corner: quarter-circle fillet of radius `rad`.
                        (
                            c + (n_in + n_out) * (off + rad),
                            rad,
                            c + n_in * off + n_out * (off + rad),
                            c + n_out * off + n_in * (off + rad),
                        )
                    } else {
                        // Concave corner: bridge arc joining the two offset
                        // points around the inside of the turn.
                        (
                            c + (n_in + n_out) * off,
                            off,
                            c + n_in * off,
                            c + n_out * off,
                        )
                    };
                    let a0 = f32::atan2((pin - center).y, (pin - center).x);
                    let a1 = f32::atan2((pout - center).y, (pout - center).x);
                    let mut sweep = a1 - a0;
                    while sweep > std::f32::consts::PI {
                        sweep -= std::f32::consts::TAU;
                    }
                    while sweep < -std::f32::consts::PI {
                        sweep += std::f32::consts::TAU;
                    }
                    arcs.push((center, radius, a0, sweep));
                }
            }
        }
    }

    // Ghost-house door: pink bar across the tile.
    let mut door_segs = Vec::new();
    for ty in 0..MAZE_H {
        for tx in 0..MAZE_W {
            if grid[ty * MAZE_W + tx] == Tile::Door {
                door_segs.push(Rect::new(tx as f32 * 8.0, ty as f32 * 8.0 + 3.0, 8.0, 2.0));
            }
        }
    }

    MazeShape {
        lines,
        arcs,
        door_segs,
    }
}

// ---------------------------------------------------------------------------
// World drawing
// ---------------------------------------------------------------------------

fn draw_world(game: &Game, maze: &MazeShape) {
    let flash = game.phase == GamePhase::LevelComplete && (game.frames / 8) % 2 == 0;
    let wall_col = if flash { EYE_WHITE } else { WALL_BLUE };

    // Maze: continuous rounded outlines
    for l in &maze.lines {
        draw_line(l[0].x, l[0].y, l[1].x, l[1].y, WALL_LINE_W, wall_col);
    }
    for &(center, r, a0, sweep) in &maze.arcs {
        const STEPS: usize = 6;
        let mut prev = a0;
        for i in 1..=STEPS {
            let a = a0 + sweep * (i as f32 / STEPS as f32);
            draw_line(
                center.x + r * prev.cos(),
                center.y + r * prev.sin(),
                center.x + r * a.cos(),
                center.y + r * a.sin(),
                WALL_LINE_W,
                wall_col,
            );
            prev = a;
        }
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
        let w = px_text_width(&text, 1.0);
        draw_px_text(&text, px - w / 2.0, py + 3.0, 1.0, SCORE_CYAN);
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
// Pixel font (procedural 8x8 arcade-style bitmap font, '#' = lit pixel)
// ---------------------------------------------------------------------------

const GLYPH_0: [&str; 7] = [".####.", "#....#", "#....#", "#....#", "#....#", "#....#", ".####."];
const GLYPH_1: [&str; 7] = ["...#..", "..##..", "...#..", "...#..", "...#..", "...#..", ".###.."];
const GLYPH_2: [&str; 7] = [".####.", "#....#", ".....#", "...##.", "..#...", ".#....", "######"];
const GLYPH_3: [&str; 7] = [".####.", "#....#", ".....#", "..###.", ".....#", "#....#", ".####."];
const GLYPH_4: [&str; 7] = ["...##.", "..#.#.", ".#..#.", "#...#.", "######", "....#.", "....#."];
const GLYPH_5: [&str; 7] = ["######", "#.....", "#####.", ".....#", ".....#", "#....#", ".####."];
const GLYPH_6: [&str; 7] = ["..###.", ".#....", "#.....", "####..", "#....#", "#....#", ".####."];
const GLYPH_7: [&str; 7] = ["######", ".....#", "....#.", "...#..", "..#...", "..#...", "..#..."];
const GLYPH_8: [&str; 7] = [".####.", "#....#", "#....#", ".####.", "#....#", "#....#", ".####."];
const GLYPH_9: [&str; 7] = [".####.", "#....#", "#....#", ".#####", ".....#", "....#.", ".###.."];
const GLYPH_A: [&str; 7] = ["..##..", ".#..#.", "#....#", "#....#", "######", "#....#", "#....#"];
const GLYPH_B: [&str; 7] = ["#####.", "#....#", "#....#", "#####.", "#....#", "#....#", "#####."];
const GLYPH_C: [&str; 7] = [".####.", "#....#", "#.....", "#.....", "#.....", "#....#", ".####."];
const GLYPH_D: [&str; 7] = ["#####.", "#....#", "#....#", "#....#", "#....#", "#....#", "#####."];
const GLYPH_E: [&str; 7] = ["######", "#.....", "#.....", "#####.", "#.....", "#.....", "######"];
const GLYPH_F: [&str; 7] = ["######", "#.....", "#.....", "#####.", "#.....", "#.....", "#....."];
const GLYPH_G: [&str; 7] = [".####.", "#....#", "#.....", "#..###", "#....#", "#....#", ".####."];
const GLYPH_H: [&str; 7] = ["#....#", "#....#", "#....#", "######", "#....#", "#....#", "#....#"];
const GLYPH_I: [&str; 7] = [".####.", "..##..", "..##..", "..##..", "..##..", "..##..", ".####."];
const GLYPH_J: [&str; 7] = ["..####", "...#..", "...#..", "...#..", "...#..", "..#...", ".##..."];
const GLYPH_K: [&str; 7] = ["#....#", "#...#.", "#..#..", "#.#...", "##....", "#..#..", "#...#."];
const GLYPH_L: [&str; 7] = ["#.....", "#.....", "#.....", "#.....", "#.....", "#.....", "######"];
const GLYPH_M: [&str; 7] = ["#.....#", "##...##", "#.#.#.#", "#..#..#", "#.....#", "#.....#", "#.....#"];
const GLYPH_N: [&str; 7] = ["#....#", "##...#", "#.#..#", "#..#.#", "#...##", "#....#", "#....#"];
const GLYPH_O: [&str; 7] = [".####.", "#....#", "#....#", "#....#", "#....#", "#....#", ".####."];
const GLYPH_P: [&str; 7] = ["#####.", "#....#", "#....#", "#####.", "#.....", "#.....", "#....."];
const GLYPH_Q: [&str; 7] = [".####.", "#....#", "#....#", "#....#", "#..#.#", "#...#.", ".##.#."];
const GLYPH_R: [&str; 7] = ["#####.", "#....#", "#....#", "#####.", "#..#..", "#...#.", "#....#"];
const GLYPH_S: [&str; 7] = [".#####", "#.....", "#.....", ".####.", ".....#", ".....#", "#####."];
const GLYPH_T: [&str; 7] = ["######", "..#...", "..#...", "..#...", "..#...", "..#...", "..#..."];
const GLYPH_U: [&str; 7] = ["#....#", "#....#", "#....#", "#....#", "#....#", "#....#", ".####."];
const GLYPH_V: [&str; 7] = ["#....#", "#....#", "#....#", "#....#", ".#..#.", ".#..#.", "..##.."];
const GLYPH_W: [&str; 7] = ["#.....#", "#.....#", "#.....#", "#.#.#.#", "#.#.#.#", "#.#.#.#", ".#.#.#."];
const GLYPH_X: [&str; 7] = ["#....#", "#....#", ".#..#.", "..##..", "..##..", ".#..#.", "#....#"];
const GLYPH_Y: [&str; 7] = ["#....#", "#....#", ".#..#.", "..##..", "..#...", "..#...", "..#..."];
const GLYPH_Z: [&str; 7] = ["######", "....#.", "...#..", "..#...", ".#....", "#.....", "######"];
const GLYPH_BANG: [&str; 7] = ["..#...", "..#...", "..#...", "..#...", "..#...", "......", "..#..."];
const GLYPH_SPACE: [&str; 7] = ["", "", "", "", "", "", ""];

fn glyph(ch: char) -> Option<&'static [&'static str; 7]> {
    match ch {
        '0' => Some(&GLYPH_0),
        '1' => Some(&GLYPH_1),
        '2' => Some(&GLYPH_2),
        '3' => Some(&GLYPH_3),
        '4' => Some(&GLYPH_4),
        '5' => Some(&GLYPH_5),
        '6' => Some(&GLYPH_6),
        '7' => Some(&GLYPH_7),
        '8' => Some(&GLYPH_8),
        '9' => Some(&GLYPH_9),
        'A' => Some(&GLYPH_A),
        'B' => Some(&GLYPH_B),
        'C' => Some(&GLYPH_C),
        'D' => Some(&GLYPH_D),
        'E' => Some(&GLYPH_E),
        'F' => Some(&GLYPH_F),
        'G' => Some(&GLYPH_G),
        'H' => Some(&GLYPH_H),
        'I' => Some(&GLYPH_I),
        'J' => Some(&GLYPH_J),
        'K' => Some(&GLYPH_K),
        'L' => Some(&GLYPH_L),
        'M' => Some(&GLYPH_M),
        'N' => Some(&GLYPH_N),
        'O' => Some(&GLYPH_O),
        'P' => Some(&GLYPH_P),
        'Q' => Some(&GLYPH_Q),
        'R' => Some(&GLYPH_R),
        'S' => Some(&GLYPH_S),
        'T' => Some(&GLYPH_T),
        'U' => Some(&GLYPH_U),
        'V' => Some(&GLYPH_V),
        'W' => Some(&GLYPH_W),
        'X' => Some(&GLYPH_X),
        'Y' => Some(&GLYPH_Y),
        'Z' => Some(&GLYPH_Z),
        '!' => Some(&GLYPH_BANG),
        ' ' => Some(&GLYPH_SPACE),
        _ => None,
    }
}

/// Draw a string with the 8x8 pixel font. `x`/`y` follow `draw_text`
/// conventions: `y` is the text baseline, glyphs are 7 px tall above it and
/// advance 8 px per character. `size` is the scale (1.0 = one font pixel per
/// game pixel).
fn draw_px_text(text: &str, x: f32, y: f32, size: f32, col: Color) {
    let mut cx = x;
    for ch in text.chars() {
        if let Some(g) = glyph(ch) {
            for (r, row) in g.iter().enumerate() {
                for (c, p) in row.chars().enumerate() {
                    if p == '#' {
                        draw_rectangle(
                            cx + c as f32 * size,
                            y - (7 - r) as f32 * size,
                            size,
                            size,
                            col,
                        );
                    }
                }
            }
        }
        cx += 8.0 * size;
    }
}

/// Pixel width of a string rendered by [`draw_px_text`].
fn px_text_width(text: &str, size: f32) -> f32 {
    let n = text.chars().count();
    if n == 0 {
        return 0.0;
    }
    (n as f32 * 8.0 - 1.0) * size
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
        let p1 = vec2(
            x + r * 1.2 * (a + half_mouth).cos(),
            y + r * 1.2 * (a + half_mouth).sin(),
        );
        let p2 = vec2(
            x + r * 1.2 * (a - half_mouth).cos(),
            y + r * 1.2 * (a - half_mouth).sin(),
        );
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

/// Death animation: Pac-Man's mouth opens progressively wider until the wedge
/// consumes the whole body, then a brief "pop" burst of sparks.
/// `dead_anim_frame` runs 1..=45 over the collapse (see logic/pacman.rs);
/// the phase exits Dying the same tick frame 45 repeats, so the pop shows on
/// the final two ticks.
fn draw_pacman_death(p: &PacmanState) {
    const MAX_FRAME: f32 = 45.0;
    let t = (p.dead_anim_frame as f32 / MAX_FRAME).min(1.0);
    if p.dead_anim_frame as f32 >= MAX_FRAME - 0.5 {
        // Final pop: tiny burst of sparks around where Pac-Man stood.
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::FRAC_PI_4;
            draw_circle(p.x + 9.0 * a.cos(), p.y + 9.0 * a.sin(), 1.2, PAC_YELLOW);
        }
        return;
    }
    let half = 0.25 + t * (std::f32::consts::PI - 0.25);
    wedge(p.x, p.y, 7.0, -std::f32::consts::FRAC_PI_2, half, PAC_YELLOW);
}

// ---------------------------------------------------------------------------
// Ghosts (14x14 pixel bitmap sprites with a two-frame wave skirt, like the
// arcade ROM sprites)
// ---------------------------------------------------------------------------

/// Ghost body bitmaps, 14x14 ('B' = body colour, '.' = transparent).
/// The dome sides drop vertically from a flat rounded top, and the skirt
/// alternates between two wave frames every ~10 ticks, as in the arcade.
const GHOST_BODY_A: [&str; 14] = [
    "....BBBBBB....",
    "..BBBBBBBBBB..",
    ".BBBBBBBBBBBB.",
    ".BBBBBBBBBBBB.",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BB.BB.BB.BB.BB",
    "BB.BB.BB.BB.BB",
];
const GHOST_BODY_B: [&str; 14] = [
    "....BBBBBB....",
    "..BBBBBBBBBB..",
    ".BBBBBBBBBBBB.",
    ".BBBBBBBBBBBB.",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBB",
    "BBB.BBBBBB.BBB",
    "BB..BBBBBB..BB",
];

/// White of the eye: 5x5 oval ('#' = lit).
const EYE_OVAL: [&str; 5] = [".###.", "#####", "#####", "#####", ".###."];

/// Draw a bitmap sprite at 1 game pixel per cell, top-left at (x0, y0).
fn draw_bitmap(rows: &[&str], x0: f32, y0: f32, lit: char, col: Color) {
    for (r, row) in rows.iter().enumerate() {
        for (c, p) in row.chars().enumerate() {
            if p == lit {
                draw_rectangle(x0 + c as f32, y0 + r as f32, 1.0, 1.0, col);
            }
        }
    }
}

fn draw_ghost(g: &GhostState, frames: u64) {
    let frightened = g.mode == GhostMode::Frightened;
    let eaten = g.mode == GhostMode::Eaten;

    // 14x14 sprite origin, snapped to whole game pixels so it stays crisp.
    let sx = g.x.round() - 7.0;
    let sy = g.y.round() - 7.0;

    // Eyes only while returning home after being eaten.
    if eaten {
        draw_eyes(sx, sy, g.dir);
        return;
    }

    let flashing = frightened && g.frightened_timer < 120 && (g.frightened_timer / 10) % 2 == 0;
    let body_col = if frightened {
        if flashing { EYE_WHITE } else { FRIGHT_BODY }
    } else {
        ghost_color(g.id)
    };

    // Dome + wavy skirt from the 14x14 bitmap; two-frame wave animation,
    // flipping every 10 ticks like the original.
    let frame = if (frames / 10) % 2 == 0 {
        &GHOST_BODY_A
    } else {
        &GHOST_BODY_B
    };
    draw_bitmap(frame, sx, sy, 'B', body_col);

    if frightened {
        draw_frightened_face(sx, sy, flashing);
    } else {
        draw_eyes(sx, sy, g.dir);
    }
}

/// The big arcade eyes: two 5x5 white ovals (sprite cols 1-5 / 8-12, rows
/// 3-7) with 2x2 dark-blue pupils that shift within the eye toward the
/// looking direction. Coordinates are relative to the 14x14 sprite whose
/// top-left is at (sx, sy).
fn draw_eyes(sx: f32, sy: f32, dir: Direction) {
    // Pupil offset (eye-local px, top-left of the 2x2 pupil) by direction.
    let (pdx, pdy) = match dir {
        Direction::Up => (2.0, 0.0),
        Direction::Down => (2.0, 3.0),
        Direction::Left => (0.0, 2.0),
        Direction::Right => (3.0, 2.0),
    };
    for ex in [1.0f32, 8.0] {
        for (r, row) in EYE_OVAL.iter().enumerate() {
            for (c, p) in row.chars().enumerate() {
                if p == '#' {
                    draw_rectangle(sx + ex + c as f32, sy + 3.0 + r as f32, 1.0, 1.0, EYE_WHITE);
                }
            }
        }
        draw_rectangle(sx + ex + pdx, sy + 3.0 + pdy, 2.0, 2.0, PUPIL_BLUE);
    }
}

fn draw_frightened_face(sx: f32, sy: f32, flashing: bool) {
    let face = if flashing { FRUIT_RED } else { EYE_WHITE };
    // Square-ish eyes: 2x2 px each (sprite cols 3-4 / 9-10, rows 4-5).
    for ex in [3.0f32, 9.0] {
        draw_rectangle(sx + ex, sy + 4.0, 2.0, 2.0, face);
    }
    // Wavy mouth: 1 px zigzag across sprite rows 9-10.
    for i in 0..5 {
        let x0 = sx + 2.0 + i as f32 * 2.0;
        let (y0, y1) = if i % 2 == 0 {
            (sy + 10.0, sy + 9.0)
        } else {
            (sy + 9.0, sy + 10.0)
        };
        draw_line(x0, y0, x0 + 2.0, y1, 1.0, face);
    }
}

// ---------------------------------------------------------------------------
// HUD
// ---------------------------------------------------------------------------

fn centered_px_text(text: &str, cx: f32, baseline: f32, size: f32, col: Color) {
    draw_px_text(text, cx - px_text_width(text, size) / 2.0, baseline, size, col);
}

fn draw_hud(game: &Game) {
    // Top labels + scores. "1UP" blinks (~every 16 frames) during play.
    let oneup_on = game.phase != GamePhase::Playing || (game.frames / 16) % 2 == 0;
    if oneup_on {
        draw_px_text("1UP", 26.0, 12.0, 1.0, EYE_WHITE);
    }
    centered_px_text("HIGH SCORE", 112.0, 12.0, 1.0, EYE_WHITE);
    let score = game.score.to_string();
    draw_px_text(&score, 40.0 - px_text_width(&score, 1.0), 22.0, 1.0, EYE_WHITE);
    let high = game.high_score.to_string();
    centered_px_text(&high, 112.0, 22.0, 1.0, EYE_WHITE);

    // Lives as mini pac icons, bottom-left (facing left, like the arcade).
    // The arcade shows spare lives only: lives - 1 icons.
    let n = game.lives.saturating_sub(1).min(5) as i32;
    for i in 0..n {
        let cx = 14.0 + i as f32 * 16.0;
        wedge(cx, 281.0, 5.0, std::f32::consts::PI, 0.5, PAC_YELLOW);
    }

    // Recent-level fruit trail, bottom-right: the last 7 fruits.
    let first = game.level.saturating_sub(6).max(1);
    for lvl in first..=game.level {
        let i = lvl - first;
        draw_fruit_icon(fruit_for_level(lvl), 210.0 - i as f32 * 16.0, 281.0);
    }

    // Phase text
    match game.phase {
        GamePhase::Ready(_) => centered_px_text("READY!", 112.0, 169.0, 1.0, PAC_YELLOW),
        GamePhase::GameOver => centered_px_text("GAME  OVER", 112.0, 169.0, 1.0, FRUIT_RED),
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
// Pixel-sprite fruit icons (8x8, centered at cx, cy; '#' = lit pixel)
// ---------------------------------------------------------------------------

const FRUIT_CHERRY: [&str; 8] = [
    "...M....", "..M.M...", ".M...M..", ".M...M..", "RRR.RRR.", "RRR.RRR.", "RRR.RRR.",
    ".RR.RR..",
];
const FRUIT_STRAWBERRY: [&str; 8] = [
    "..GGGG..", ".GGGGGG.", ".RRRRRR.", "RRWRRWRR", "RRRRWRRR", ".RWRRWR.", "..RRRR..",
    "...RR...",
];
const FRUIT_ORANGE: [&str; 8] = [
    "....M...", "...GG...", "..OOOO..", ".OOOOOO.", ".OWOOOO.", ".OOOOOO.", "..OOOO..",
    "........",
];
const FRUIT_APPLE: [&str; 8] = [
    "...M....", "...M....", ".RRRRR..", "RRRRRRR.", "RWRRRRR.", "RRRRRRR.", ".RRRRR..",
    "..RRR...",
];
const FRUIT_MELON: [&str; 8] = [
    "....M...", "..LLLL..", ".LDLLDL.", "LLDLLDLL", "LLDLLDLL", ".LDLLDL.", "..LLLL..",
    "........",
];
const FRUIT_GALAXIAN: [&str; 8] = [
    "RR....RR", "RRRYYRRR", ".RRYYRR.", "..RYYR..", "...YY...", "...YY...", "...YY...",
    "........",
];
const FRUIT_BELL: [&str; 8] = [
    "...YY...", "...YY...", "..YYYY..", ".YYYYYY.", ".YYYYYY.", "YYYYYYYY", "WWWWWWWW",
    "...WW...",
];
const FRUIT_KEY: [&str; 8] = [
    "..SSSS..", ".SS..SS.", ".SS..SS.", "..SSSS..", "...SS...", "...SS...", "..SSSS..",
    "...S..S.",
];

fn draw_fruit_icon(kind: FruitKind, cx: f32, cy: f32) {
    let sprite: &[&str; 8] = match kind {
        FruitKind::Cherry => &FRUIT_CHERRY,
        FruitKind::Strawberry => &FRUIT_STRAWBERRY,
        FruitKind::Orange => &FRUIT_ORANGE,
        FruitKind::Apple => &FRUIT_APPLE,
        FruitKind::Melon => &FRUIT_MELON,
        FruitKind::Galaxian => &FRUIT_GALAXIAN,
        FruitKind::Bell => &FRUIT_BELL,
        FruitKind::Key => &FRUIT_KEY,
    };
    let x0 = cx - 4.0;
    let y0 = cy - 4.0;
    for (r, row) in sprite.iter().enumerate() {
        for (c, p) in row.chars().enumerate() {
            let col = match p {
                'R' => FRUIT_RED,
                'G' => STEM_GREEN,
                'L' => Color::from_rgba(0x7c, 0xfc, 0x00, 0xff),
                'D' => Color::from_rgba(0x2e, 0x8b, 0x2e, 0xff),
                'Y' => Color::from_rgba(0xff, 0xd7, 0x00, 0xff),
                'W' => EYE_WHITE,
                'M' => STEM_BROWN,
                'O' => Color::from_rgba(0xff, 0xa5, 0x00, 0xff),
                'S' => Color::from_rgba(0xc0, 0xc0, 0xc0, 0xff),
                _ => continue,
            };
            draw_rectangle(x0 + c as f32, y0 + r as f32, 1.0, 1.0, col);
        }
    }
}
