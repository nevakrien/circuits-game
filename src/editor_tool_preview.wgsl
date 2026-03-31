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

fn preview_wire_mask(centered: vec2<f32>) -> f32 {
    let shaft = segment_mask(centered, vec2(-0.30, 0.0), vec2(0.30, 0.0), 0.055);
    let left_cap = circle_mask(centered, vec2(-0.30, 0.0), 0.055);
    let right_cap = circle_mask(centered, vec2(0.30, 0.0), 0.055);
    return max(shaft, max(left_cap, right_cap));
}

fn preview_wire_color(centered: vec2<f32>) -> vec3<f32> {
    let mask = preview_wire_mask(centered);
    let core = preview_wire_mask(centered * vec2(1.0, 1.8));
    var color = vec3(0.03, 0.03, 0.04);
    color = mix(color, vec3(0.18, 0.44, 0.64), mask);
    color = mix(color, vec3(0.88, 0.96, 1.0), core * 0.65);
    return color;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let local_uv = in.uv;
    let centered = local_uv - vec2(0.5, 0.5);
    let color = select(
        render_cell_color(vec2u(0u, 0u), local_uv, centered, preview_params.charge.x, preview_params.circuit),
        preview_wire_color(centered),
        preview_params.circuit.x == 255u,
    );
    let border = select(0.0, 1.0, any(local_uv < vec2(0.035, 0.035)) || any(local_uv > vec2(0.965, 0.965)));
    let final_color = mix(color, vec3(0.95, 0.98, 1.0), border * 0.28);
    return vec4(final_color, 1.0);
}
