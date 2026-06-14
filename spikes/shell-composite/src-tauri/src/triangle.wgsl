// 1b compositing probe: a bright cyan triangle drawn by native wgpu to the window surface, ROTATING
// (continuous viewport motion for the flicker-under-motion battery — Tauri #9220 territory).
// If it shows through the transparent webview region, Windows composites the native layer (PASS).

struct U { angle: f32, aspect: f32, _pad0: f32, _pad1: f32 };
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 0.6),
        vec2<f32>(-0.6, -0.5),
        vec2<f32>(0.6, -0.5),
    );
    let c = cos(u.angle);
    let s = sin(u.angle);
    var q = p[i];
    q = vec2<f32>(q.x * c - q.y * s, q.x * s + q.y * c);
    q.x = q.x / u.aspect; // keep shape square regardless of window aspect
    return vec4<f32>(q, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.15, 0.95, 1.0, 1.0); // bright cyan
}
