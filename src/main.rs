mod audio;
mod input;
mod logic;
mod render;

fn main() {
    render::run(logic::Game::new(1));
}
