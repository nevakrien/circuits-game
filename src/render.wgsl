@group(0) @binding(0)
var charge_tex: texture_3d<u32>;

@group(0) @binding(1)
var circuit_tex: texture_3d<u32>;

struct RenderParams {
    view: vec4<f32>,
    layer: vec4<u32>,
}

@group(0) @binding(2)
var<uniform> render_params: RenderParams;

fn byte_channel(coord: vec2<u32>) -> u32 {
    return (coord.y & 1u) * 2u + (coord.x & 1u);
}

fn read_byte(tex: texture_3d<u32>, coord: vec3<u32>) -> u32 {
    let packed_coord = vec3<i32>(i32(coord.x >> 1u), i32(coord.y >> 1u), i32(coord.z));
    let packed = textureLoad(tex, packed_coord, 0);

    switch (byte_channel(coord.xy)) {
        case 0u: {
            return packed.x;
        }
        case 1u: {
            return packed.y;
        }
        case 2u: {
            return packed.z;
        }
        default: {
            return packed.w;
        }
    }
}

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
    let dims = textureDimensions(circuit_tex);

    let uv = (in.uv - vec2(0.5, 0.5)) * render_params.view.xy
        + vec2(0.5, 0.5)
        + render_params.view.zw;

    if (any(uv < vec2(0.0, 0.0)) || any(uv >= vec2(1.0, 1.0))) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }

    let layer = min(render_params.layer.x, dims.z - 1u);
    let cell = uv * vec2<f32>(dims.xy);
    let coord = min(vec2<u32>(cell), dims.xy - vec2(1u, 1u));
    let local_uv = fract(cell);
    let centered = local_uv - vec2(0.5, 0.5);

    let sample_coord = vec3<i32>(vec2<i32>(coord), i32(layer));
    let charge = read_byte(charge_tex, vec3u(coord, layer)) & 0xffu;
    let circuit = textureLoad(circuit_tex, sample_coord, 0);

    var color = render_cell_color(coord, local_uv, centered, charge, circuit);

    let grid_mask = select(0.0, 1.0, any(local_uv < vec2(0.04, 0.04)) || any(local_uv > vec2(0.96, 0.96)));
    color = max(color, vec3(grid_mask * 0.08));

    return vec4(color, 1.0);
}
