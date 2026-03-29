@group(0) @binding(0)
var charge_tex: texture_3d<u32>;

@group(0) @binding(1)
var circuit_tex: texture_3d<u32>;

struct RenderParams {
    uv_scale: vec2<f32>,
    uv_offset: vec2<f32>,
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

const GATE_LABEL_LENGTHS: array<u32, 7> = array<u32, 7>(3u, 3u, 2u, 3u, 4u, 3u, 4u);
const GATE_LABEL_LETTERS: array<vec4<u32>, 7> = array<vec4<u32>, 7>(
    vec4<u32>(2u, 3u, 5u, 0u),
    vec4<u32>(0u, 2u, 1u, 0u),
    vec4<u32>(3u, 4u, 0u, 0u),
    vec4<u32>(6u, 3u, 4u, 0u),
    vec4<u32>(2u, 0u, 2u, 1u),
    vec4<u32>(2u, 3u, 4u, 0u),
    vec4<u32>(6u, 2u, 3u, 4u),
);
// Glyph columns sourced from Adafruit_GFX classic 5x7 font (glcdfont.c).
const GATE_GLYPH_ATLAS: array<array<u32, 5>, 7> = array<array<u32, 5>, 7>(
    array<u32, 5>(0x7Cu, 0x12u, 0x11u, 0x12u, 0x7Cu),
    array<u32, 5>(0x7Fu, 0x41u, 0x41u, 0x41u, 0x3Eu),
    array<u32, 5>(0x7Fu, 0x04u, 0x08u, 0x10u, 0x7Fu),
    array<u32, 5>(0x3Eu, 0x41u, 0x41u, 0x41u, 0x3Eu),
    array<u32, 5>(0x7Fu, 0x09u, 0x19u, 0x29u, 0x46u),
    array<u32, 5>(0x03u, 0x01u, 0x7Fu, 0x01u, 0x03u),
    array<u32, 5>(0x63u, 0x14u, 0x08u, 0x14u, 0x63u),
);

fn segment_mask(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half_width: f32) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0);
    let closest = a + ab * t;
    let distance = length(p - closest);
    return 1.0 - smoothstep(half_width, half_width + 0.02, distance);
}

fn rounded_box_mask(centered: vec2<f32>, half_size: vec2<f32>, radius: f32, softness: f32) -> f32 {
    let q = abs(centered) - half_size + vec2(radius, radius);
    let outside = length(max(q, vec2(0.0, 0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    let distance = outside + inside - radius;
    return 1.0 - smoothstep(0.0, softness, distance);
}

fn glyph_mask(local_uv: vec2<f32>, glyph_ix: u32) -> f32 {
    if (any(local_uv < vec2(0.0, 0.0)) || any(local_uv >= vec2(1.0, 1.0))) {
        return 0.0;
    }

    let padded_uv = local_uv * vec2(5.8, 7.8) - vec2(0.4, 0.4);
    if (any(padded_uv < vec2(0.0, 0.0)) || any(padded_uv >= vec2(5.0, 7.0))) {
        return 0.0;
    }

    let col = u32(padded_uv.x);
    let row = u32(padded_uv.y);
    let bits = GATE_GLYPH_ATLAS[glyph_ix][col];
    return select(0.0, 1.0, ((bits >> row) & 1u) == 1u);
}

fn gate_label_mask(local_uv: vec2<f32>, tag: u32) -> f32 {
    if (tag < 3u || tag > 9u) {
        return 0.0;
    }

    let gate_ix = tag - 3u;
    let len = GATE_LABEL_LENGTHS[gate_ix];
    let glyphs = GATE_LABEL_LETTERS[gate_ix];
    let uv = (local_uv - vec2(0.11, 0.17)) / vec2(0.78, 0.66);

    if (any(uv < vec2(0.0, 0.0)) || any(uv >= vec2(1.0, 1.0))) {
        return 0.0;
    }

    let glyph_uv = vec2(uv.x * f32(len), uv.y);
    let ix = min(u32(glyph_uv.x), len - 1u);
    let char_uv = vec2(fract(glyph_uv.x), glyph_uv.y);

    var glyph_ix = glyphs.x;
    switch (ix) {
        case 0u: { glyph_ix = glyphs.x; }
        case 1u: { glyph_ix = glyphs.y; }
        case 2u: { glyph_ix = glyphs.z; }
        default: { glyph_ix = glyphs.w; }
    }

    return glyph_mask(char_uv, glyph_ix);
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
    let arrow_color = mix(vec3(0.5, 0.03, 0.04), vec3(1.0, 0.2, 0.1), 0.35 + charge_level * 0.65);

    var color = base;
    color = mix(color, node_color, circle);
    color = mix(color, arrow_color, arrow);
    color += vec3(0.16, 0.30, 0.42) * (charge_level * glow * circle);
    return color;
}

fn render_gate(
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
    _ = circle;

    let base = vec3(0.035, 0.035, 0.045);
    let tag = circuit.x & 0xffu;
    let charge_level = f32(charge) / 255.0;
    let body_mask = rounded_box_mask(centered, vec2(0.39, 0.27), 0.08, 0.025);
    let border_mask = body_mask - rounded_box_mask(centered, vec2(0.35, 0.23), 0.07, 0.025);
    let body_color = mix(vec3(0.24, 0.28, 0.32), vec3(0.92, 0.96, 1.0), charge_level * 0.42);
    let label = gate_label_mask(local_uv, tag) * body_mask;

    var color = base;
    color = mix(color, body_color, body_mask);
    color = mix(color, vec3(0.92, 0.95, 1.0), border_mask * 0.7);
    color = mix(color, vec3(0.06, 0.07, 0.08), label);
    color += vec3(0.22, 0.28, 0.34) * (charge_level * glow * 0.24);
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
        case 3u, 4u, 5u, 6u, 7u, 8u, 9u: {
            return render_gate(in, dims, layer, coord, local_uv, centered, charge, circuit, circle, glow);
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
    let dims = textureDimensions(circuit_tex);
    let board_count = dims.z;
    let cols = max(1u, u32(ceil(sqrt(f32(board_count)))));
    let rows = max(1u, (board_count + cols - 1u) / cols);

    let uv = (in.uv - vec2(0.5, 0.5)) * render_params.uv_scale
        + vec2(0.5, 0.5)
        + render_params.uv_offset;

    if (any(uv < vec2(0.0, 0.0)) || any(uv >= vec2(1.0, 1.0))) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
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
    let charge = read_byte(charge_tex, vec3u(coord, layer)) & 0xffu;
    let circuit = textureLoad(circuit_tex, sample_coord, 0);

    let radius = length(centered);
    let circle = 1.0 - smoothstep(0.18, 0.28, radius);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);

    var color = render_tag(in, dims, layer, coord, local_uv, centered, charge, circuit, circle, glow);

    let grid_mask = select(0.0, 1.0, any(local_uv < vec2(0.04, 0.04)) || any(local_uv > vec2(0.96, 0.96)));
    color = max(color, vec3(grid_mask * 0.08));

    return vec4(color, 1.0);
}
