struct HoverPreviewParams {
    view: vec4<f32>,
    board: vec4<u32>,
    cell: vec4<u32>,
    circuit: vec4<u32>,
    charge: vec4<u32>,
    overlay: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> hover_params: HoverPreviewParams;

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
    if (hover_params.cell.w == 0u) {
        return vec4(0.0);
    }

    let uv = (in.uv - vec2(0.5, 0.5)) * hover_params.view.xy
        + vec2(0.5, 0.5)
        + hover_params.view.zw;

    if (any(uv < vec2(0.0, 0.0)) || any(uv >= vec2(1.0, 1.0))) {
        return vec4(0.0);
    }

    let board = vec2<f32>(f32(hover_params.board.x), f32(hover_params.board.y));
    let cell_min = vec2<f32>(f32(hover_params.cell.x), f32(hover_params.cell.y)) / board;
    let cell_max = vec2<f32>(f32(hover_params.cell.x + 1u), f32(hover_params.cell.y + 1u)) / board;

    if (any(uv < cell_min) || any(uv >= cell_max)) {
        return vec4(0.0);
    }

    let local_uv = (uv - cell_min) * board;
    let centered = local_uv - vec2(0.5, 0.5);
    let color = render_cell_color(hover_params.cell.xy, local_uv, centered, hover_params.charge.x, hover_params.circuit);

    let border = select(0.0, 1.0, any(local_uv < vec2(0.035, 0.035)) || any(local_uv > vec2(0.965, 0.965)));
    var final_color = mix(color, vec3(0.95, 0.98, 1.0), border * 0.28);
    final_color = mix(final_color, hover_params.overlay.rgb, hover_params.overlay.a);
    return vec4(final_color, max(0.58, hover_params.overlay.a));
}
