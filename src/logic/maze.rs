//! The exact 28x36 arcade maze, palette constants, and tile/pixel helpers.
//!
//! The maze below is transcribed from the original 1980 arcade ROM tile data
//! (verified: 240 dots + 4 power pellets, tunnel row 17, ghost-house door at
//! tiles (13,15)-(14,15), fruit tile row 20, Pac-Man start row 26).
//!
//! Legend: `#` wall, `.` dot, `o` power pellet, `-` ghost-house door,
//!         `T` tunnel (side wrap corridor), ` ` empty walkable tile.
//!
//! Grid coordinates: (tx, ty) with tx in 0..28, ty in 0..36, row-major.
//! Pixel space: 1 tile = 8 px; the tile occupying pixel column x is
//! floor(x / 8). Tile centers sit at (tx*8 + 4, ty*8 + 4). Entities may
//! straddle tile boundaries (e.g. x = 112 is between columns 13 and 14).

use super::{Direction, Tile, MAZE_H, MAZE_W};

pub const TILE_PX: f32 = 8.0;
#[allow(dead_code)] // consumed by the presentation layer
pub const SCREEN_W: f32 = 224.0; // 28 tiles
#[allow(dead_code)]
pub const SCREEN_H: f32 = 288.0; // 36 tiles

/// The exact arcade maze, 36 rows of 28 tiles.
pub const MAZE_MAP: [&str; 36] = [
    "                            ",
    "                            ",
    "                            ",
    "############################",
    "#............##............#",
    "#.####.#####.##.#####.####.#",
    "#o#  #.#   #.##.#   #.#  #o#",
    "#.####.#####.##.#####.####.#",
    "#..........................#",
    "#.####.##.########.##.####.#",
    "#.####.##.########.##.####.#",
    "#......##....##....##......#",
    "######.##### ## #####.######",
    "     #.##### ## #####.#     ",
    "     #.##          ##.#     ",
    "     #.## ###--### ##.#     ",
    "######.## #      # ##.######",
    "TTTTTT.   #      #   .TTTTTT",
    "######.## #      # ##.######",
    "     #.## ######## ##.#     ",
    "     #.##          ##.#     ",
    "     #.## ######## ##.#     ",
    "######.## ######## ##.######",
    "#............##............#",
    "#.####.#####.##.#####.####.#",
    "#.####.#####.##.#####.####.#",
    "#o..##.......  .......##..o#",
    "###.##.##.########.##.##.###",
    "###.##.##.########.##.##.###",
    "#......##....##....##......#",
    "#.##########.##.##########.#",
    "#.##########.##.##########.#",
    "#..........................#",
    "############################",
    "                            ",
    "                            ",
];

/// Parse [`MAZE_MAP`] into the row-major tile grid (length 1008).
pub fn parse_maze() -> [Tile; MAZE_W * MAZE_H] {
    let mut grid = [Tile::Empty; MAZE_W * MAZE_H];
    for (ty, row) in MAZE_MAP.iter().enumerate() {
        debug_assert_eq!(row.chars().count(), MAZE_W);
        for (tx, c) in row.chars().enumerate() {
            grid[ty * MAZE_W + tx] = match c {
                '#' => Tile::Wall,
                '.' => Tile::Dot,
                'o' => Tile::PowerPellet,
                '-' => Tile::Door,
                'T' => Tile::Tunnel,
                _ => Tile::Empty,
            };
        }
    }
    grid
}

// ---------------------------------------------------------------------------
// Original-style palette (Worker B renders with these)
// ---------------------------------------------------------------------------

#[allow(dead_code)] // presentation layer
pub const COLOR_BG: (u8, u8, u8) = (0, 0, 0);
/// Maze wall stroke blue: #2121ff
#[allow(dead_code)] // presentation layer
pub const COLOR_WALL: (u8, u8, u8) = (0x21, 0x21, 0xff);
/// Ghost-house door pink: #fcb5ff
#[allow(dead_code)] // presentation layer
pub const COLOR_DOOR: (u8, u8, u8) = (0xfc, 0xb5, 0xff);
/// Dots / power pellets (peach): #febdb4-ish
#[allow(dead_code)] // presentation layer
pub const COLOR_DOT: (u8, u8, u8) = (0xfe, 0xbd, 0xb4);
#[allow(dead_code)] // presentation layer
pub const COLOR_PELLET: (u8, u8, u8) = COLOR_DOT;
/// Pac-Man yellow
#[allow(dead_code)] // presentation layer
pub const COLOR_PAC: (u8, u8, u8) = (0xff, 0xff, 0x00);
#[allow(dead_code)] // presentation layer
pub const COLOR_BLINKY: (u8, u8, u8) = (0xff, 0x00, 0x00);
#[allow(dead_code)] // presentation layer
pub const COLOR_PINKY: (u8, u8, u8) = (0xff, 0xb8, 0xff);
#[allow(dead_code)] // presentation layer
pub const COLOR_INKY: (u8, u8, u8) = (0x00, 0xff, 0xff);
#[allow(dead_code)] // presentation layer
pub const COLOR_CLYDE: (u8, u8, u8) = (0xff, 0xb8, 0x52);
/// Frightened ghost body (dark blue) and flash (white)
#[allow(dead_code)] // presentation layer
pub const COLOR_FRIGHTENED: (u8, u8, u8) = (0x21, 0x21, 0xde);
#[allow(dead_code)] // presentation layer
pub const COLOR_FRIGHTENED_FLASH: (u8, u8, u8) = (0xff, 0xff, 0xff);
/// HUD / READY! text yellow
#[allow(dead_code)] // presentation layer
pub const COLOR_TEXT: (u8, u8, u8) = (0xff, 0xff, 0x00);

// ---------------------------------------------------------------------------
// Coordinate helpers
// ---------------------------------------------------------------------------

/// Tile column of a game-pixel x coordinate, wrapping the tunnel (columns
/// wrap modulo 28, so x = -1 is column 27 and x = 228 is column 0).
#[inline]
pub fn wrap_col(tx: i32) -> i32 {
    tx.rem_euclid(MAZE_W as i32)
}

/// Horizontal tunnel wrap for entity pixel positions: the legal play strip is
/// x in [-8, 224+8); moving off one side re-enters on the other.
#[inline]
pub fn wrap_x(x: f32) -> f32 {
    if x < -TILE_PX {
        x + SCREEN_W + 2.0 * TILE_PX
    } else if x >= SCREEN_W + TILE_PX {
        x - SCREEN_W - 2.0 * TILE_PX
    } else {
        x
    }
}

/// Tile coordinates (column already tunnel-wrapped) of a game-pixel position.
#[inline]
pub fn tile_coords(x: f32, y: f32) -> (i32, i32) {
    let tx = (x / TILE_PX).floor() as i32;
    let ty = (y / TILE_PX).floor() as i32;
    (wrap_col(tx), ty)
}

/// Center pixel of a tile (already tunnel-wrapped column).
#[inline]
pub fn tile_center(tx: i32, ty: i32) -> (f32, f32) {
    (
        wrap_col(tx) as f32 * TILE_PX + 4.0,
        ty as f32 * TILE_PX + 4.0,
    )
}

#[inline]
pub fn tile_at(grid: &[Tile; MAZE_W * MAZE_H], tx: i32, ty: i32) -> Tile {
    if ty < 0 || ty >= MAZE_H as i32 {
        return Tile::Empty;
    }
    grid[ty as usize * MAZE_W + wrap_col(tx) as usize]
}

/// True when the tile is the ghost-house door.
#[inline]
#[allow(dead_code)] // helper for the presentation/AI layers
pub fn is_door(grid: &[Tile; MAZE_W * MAZE_H], tx: i32, ty: i32) -> bool {
    tile_at(grid, tx, ty) == Tile::Door
}

/// Pac-Man may enter: empty corridors, dots, pellets and tunnel tiles
/// (never the ghost-house door).
pub fn pac_walkable(grid: &[Tile; MAZE_W * MAZE_H], tx: i32, ty: i32) -> bool {
    matches!(
        tile_at(grid, tx, ty),
        Tile::Empty | Tile::Dot | Tile::PowerPellet | Tile::Tunnel
    )
}

/// Ghost AI may enter: like Pac-Man plus the door (only when returning home
/// as eaten eyes). Ghost-house interior tiles are never AI-enterable; house
/// entry/exit is handled by scripted movement.
pub fn ghost_walkable(
    grid: &[Tile; MAZE_W * MAZE_H],
    tx: i32,
    ty: i32,
    allow_door: bool,
) -> bool {
    let (tx, ty) = (wrap_col(tx), ty);
    // Ghost house interior (rows 16..=18, cols 11..=16): scripted only.
    if (16..=18).contains(&ty) && (11..=16).contains(&tx) {
        return false;
    }
    match tile_at(grid, tx, ty) {
        Tile::Wall => false,
        Tile::Door => allow_door,
        Tile::Empty | Tile::Dot | Tile::PowerPellet | Tile::Tunnel => true,
    }
}

/// Is the pixel position inside the tunnel region (row 17, outer columns)?
pub fn in_tunnel(x: f32, y: f32) -> bool {
    let (tx, ty) = tile_coords(x, y);
    ty == 17 && (tx <= 5 || tx >= 22)
}

// ---------------------------------------------------------------------------
// Grid movement primitive shared by Pac-Man and the ghosts
// ---------------------------------------------------------------------------

/// Advance a position along a grid corridor.
///
/// Movement is axis-locked to the maze tiles: `decide` is consulted exactly
/// when the entity reaches a tile center (including the very start if it
/// already stands on one, e.g. after being stopped or handed off from a
/// scripted house move). It receives the tile whose center we are on plus the
/// current direction, and returns the direction to continue in — or `None` to
/// stop at the center (wall ahead / no valid turn).
///
/// Returns the (possibly changed) direction and whether the entity ended the
/// tick stopped at a tile center. `speed` must be < 8 px so at most one tile
/// center is crossed per tick.
pub fn grid_move(
    pos: &mut (f32, f32),
    mut dir: Direction,
    speed: f32,
    decide: &mut dyn FnMut(i32, i32, Direction) -> Option<Direction>,
) -> (Direction, bool) {
    // Decide immediately when already standing exactly on a tile center.
    {
        let (tx, ty) = tile_coords(pos.0, pos.1);
        let (cx, cy) = tile_center(tx, ty);
        if pos.0 == cx && pos.1 == cy {
            match decide(tx, ty, dir) {
                Some(d) => dir = d,
                None => return (dir, true),
            }
        }
    }

    let mut rem = speed;
    loop {
        if rem <= 1e-4 {
            return (dir, false);
        }
        let (dx, dy) = dir.delta();
        let horiz = dx != 0.0;
        let s = if horiz { pos.0 } else { pos.1 };
        let v = if horiz { dx } else { dy };
        let tile_s = (s / TILE_PX).floor() as i32;
        let center = tile_s as f32 * TILE_PX + 4.0;
        // Distance to the next tile center ahead along the direction of travel.
        let mut d = (center - s) * v;
        if d < 1e-4 {
            d += TILE_PX;
        }
        if d > rem {
            // No center crossed this tick; advance freely.
            if horiz {
                pos.0 = wrap_x(s + v * rem);
            } else {
                pos.1 = s + v * rem;
            }
            return (dir, false);
        }
        // Cross the center of the tile we are entering/leaving.
        if horiz {
            pos.0 = wrap_x(s + v * d);
        } else {
            pos.1 = s + v * d;
        }
        rem -= d;
        let (tx, ty) = tile_coords(pos.0, pos.1);
        match decide(tx, ty, dir) {
            Some(nd) => dir = nd,
            None => return (dir, true),
        }
    }
}
