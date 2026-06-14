//! Native entry. Visual: `cargo run --release --bin scene` (set `SCENE_N=20000`). Headless bench:
//! `SPIKE_SECS=60 SCENE_N=5000 cargo run --release --bin scene` → prints the frame-time table.
fn main() {
    metrocalk_render_spike::run();
}
