// Presentation layer: keyboard polling. Worker B owns this file.
use crate::logic::Action;
use macroquad::prelude::*;

/// Map currently-held arrow keys and/or WASD to a desired-direction action.
/// Priority: Up, Down, Left, Right (first held wins). No buffering here —
/// the logic core handles turning/buffering.
pub fn poll_action() -> Action {
    if is_key_down(KeyCode::Up) || is_key_down(KeyCode::W) {
        Action::Up
    } else if is_key_down(KeyCode::Down) || is_key_down(KeyCode::S) {
        Action::Down
    } else if is_key_down(KeyCode::Left) || is_key_down(KeyCode::A) {
        Action::Left
    } else if is_key_down(KeyCode::Right) || is_key_down(KeyCode::D) {
        Action::Right
    } else {
        Action::None
    }
}
