@group(0) @binding(0)
var charge_tex: texture_3d<u32>;

@group(0) @binding(1)
var circuit_tex: texture_3d<u32>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn segment_mask(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half_width: f32) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0);
    let closest = a + ab * t;
    let distance = length(p - closest);
    return 1.0 - smoothstep(half_width, half_width + 0.02, distance);
}

fn render_noop(
    in: VsOut,
    dims: vec3<u32>,
    layer: u32,
    coord: vec2<u32>,
    local_uv: vec2<f32>,
    centered: vec2<f32>,
    charge: u32,
    circuit: vec4<u32>,
    circle: f32,
    glow: f32,
) -> vec3<f32> {
    _ = in;
    _ = dims;
    _ = layer;
    _ = coord;
    _ = local_uv;
    _ = centered;
    _ = circuit;

    let base = vec3(0.04, 0.04, 0.05);
    let charge_level = f32(charge) / 255.0;
    let node_color = mix(base, vec3(0.85, 0.9, 0.95), charge_level * 0.8);

    var color = base * (1.0 - circle) + node_color * circle;
    color += vec3(0.18, 0.32, 0.42) * (charge_level * glow);
    return color;
}

fn render_source(
    in: VsOut,
    dims: vec3<u32>,
    layer: u32,
    coord: vec2<u32>,
    local_uv: vec2<f32>,
    centered: vec2<f32>,
    charge: u32,
    circuit: vec4<u32>,
    circle: f32,
    glow: f32,
) -> vec3<f32> {
    _ = in;
    _ = dims;
    _ = layer;
    _ = coord;
    _ = local_uv;
    _ = centered;
    _ = circuit;

    let base = vec3(0.04, 0.04, 0.05);
    let source_color = vec3(0.28, 0.16, 0.10);
    let charge_level = f32(charge) / 255.0;
    let node_color = mix(source_color, vec3(0.95, 0.82, 0.68), charge_level * 0.8);

    var color = base;
    color = mix(color, node_color, circle);
    color += vec3(0.24, 0.18, 0.08) * (charge_level * glow);
    return color;
}

fn render_wire(
    in: VsOut,
    dims: vec3<u32>,
    layer: u32,
    coord: vec2<u32>,
    local_uv: vec2<f32>,
    centered: vec2<f32>,
    charge: u32,
    circuit: vec4<u32>,
    circle: f32,
    glow: f32,
) -> vec3<f32> {
    _ = in;
    _ = dims;
    _ = layer;
    _ = local_uv;

    let base = vec3(0.04, 0.04, 0.05);
    let wire_color = vec3(0.18, 0.44, 0.64);
    let charge_level = f32(charge) / 255.0;
    let node_color = mix(wire_color, vec3(0.88, 0.96, 1.0), charge_level * 0.8);

    let src = vec2<f32>(f32(circuit.y & 0xffu), f32(circuit.z & 0xffu));
    var flow = vec2<f32>(coord) - src;
    if (all(flow == vec2(0.0, 0.0))) {
        flow = vec2(0.0, -1.0);
    }
    flow = normalize(flow);

    let tip = flow * 0.14;
    let tail = -flow * 0.14;
    let wing = vec2(-flow.y, flow.x) * 0.055;
    let shaft = segment_mask(centered, tail, tip, 0.02);
    let head_left = segment_mask(centered, tip, tip - flow * 0.085 + wing, 0.016);
    let head_right = segment_mask(centered, tip, tip - flow * 0.085 - wing, 0.016);
    let arrow = max(shaft, max(head_left, head_right)) * circle;
    let arrow_color = mix(vec3(0.02, 0.03, 0.04), vec3(0.97, 0.99, 1.0), 0.35 + charge_level * 0.65);

    var color = base;
    color = mix(color, node_color, circle);
    color = mix(color, arrow_color, arrow);
    color += vec3(0.16, 0.30, 0.42) * (charge_level * glow * circle);
    return color;
}

fn render_tag(
    in: VsOut,
    dims: vec3<u32>,
    layer: u32,
    coord: vec2<u32>,
    local_uv: vec2<f32>,
    centered: vec2<f32>,
    charge: u32,
    circuit: vec4<u32>,
    circle: f32,
    glow: f32,
) -> vec3<f32> {
    switch (circuit.x & 0xffu) {
        case 1u: {
            return render_source(in, dims, layer, coord, local_uv, centered, charge, circuit, circle, glow);
        }
        case 2u: {
            return render_wire(in, dims, layer, coord, local_uv, centered, charge, circuit, circle, glow);
        }
        default: {
            return render_noop(in, dims, layer, coord, local_uv, centered, charge, circuit, circle, glow);
        }
    }
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
    let dims = textureDimensions(charge_tex);
    let board_count = dims.z;
    let cols = max(1u, u32(ceil(sqrt(f32(board_count)))));
    let rows = max(1u, (board_count + cols - 1u) / cols);

    let uv = min(in.uv, vec2(0.99999994, 0.99999994));
    let tiled_uv = uv * vec2<f32>(f32(cols), f32(rows));
    let tile = vec2<u32>(tiled_uv);
    let layer = tile.y * cols + tile.x;

    if (layer >= board_count) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }

    let board_uv = fract(tiled_uv);
    let cell = board_uv * vec2<f32>(dims.xy);
    let coord = min(vec2<u32>(cell), dims.xy - vec2(1u, 1u));
    let local_uv = fract(cell);
    let centered = local_uv - vec2(0.5, 0.5);

    let sample_coord = vec3<i32>(vec2<i32>(coord), i32(layer));
    let charge = textureLoad(charge_tex, sample_coord, 0).x & 0xffu;
    let circuit = textureLoad(circuit_tex, sample_coord, 0);

    let radius = length(centered);
    let circle = 1.0 - smoothstep(0.18, 0.28, radius);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);

    var color = render_tag(in, dims, layer, coord, local_uv, centered, charge, circuit, circle, glow);

    let grid_mask = select(0.0, 1.0, any(local_uv < vec2(0.04, 0.04)) || any(local_uv > vec2(0.96, 0.96)));
    color = max(color, vec3(grid_mask * 0.08));

    return vec4(color, 1.0);
}
