struct ToolPreviewParams {
    circuit: vec4<u32>,
    charge: vec4<u32>,
}

@group(0) @binding(0)
var<uniform> preview_params: ToolPreviewParams;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0),
    );

    let clip = positions[index];

    var out: VsOut;
    out.position = vec4(clip, 0.0, 1.0);
    out.uv = vec2(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let local_uv = in.uv;
    let centered = local_uv - vec2(0.5, 0.5);
    let color = render_cell_color(vec2u(0u, 0u), local_uv, centered, preview_params.charge.x, preview_params.circuit);
    let border = select(0.0, 1.0, any(local_uv < vec2(0.035, 0.035)) || any(local_uv > vec2(0.965, 0.965)));
    let final_color = mix(color, vec3(0.95, 0.98, 1.0), border * 0.28);
    return vec4(final_color, 1.0);
}
