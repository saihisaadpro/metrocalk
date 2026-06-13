// 1b compositing probe: a single bright triangle drawn by native wgpu to the window surface.
// If it shows through the transparent webview region, Windows composites the native layer (PASS).

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 0.6),
        vec2<f32>(-0.6, -0.5),
        vec2<f32>(0.6, -0.5),
    );
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // bright cyan — unmistakable against the dark clear color and the webview chrome
    return vec4<f32>(0.15, 0.95, 1.0, 1.0);
}
