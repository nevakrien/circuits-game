const GATE_LABEL_LENGTHS: array<u32, 8> = array<u32, 8>(4u, 3u, 3u, 2u, 3u, 4u, 3u, 4u);
const GATE_LABEL_LETTERS: array<vec4<u32>, 8> = array<vec4<u32>, 8>(
    vec4<u32>(3u, 4u, 4u, 5u),
    vec4<u32>(3u, 4u, 7u, 0u),
    vec4<u32>(0u, 3u, 1u, 0u),
    vec4<u32>(4u, 6u, 0u, 0u),
    vec4<u32>(8u, 4u, 6u, 0u),
    vec4<u32>(3u, 0u, 3u, 1u),
    vec4<u32>(3u, 4u, 6u, 0u),
    vec4<u32>(8u, 3u, 4u, 6u),
);

// Glyph columns sourced from Adafruit_GFX classic 5x7 font (glcdfont.c).
const GATE_GLYPH_ATLAS: array<array<u32, 5>, 9> = array<array<u32, 5>, 9>(
    array<u32, 5>(0x7Cu, 0x12u, 0x11u, 0x12u, 0x7Cu),
    array<u32, 5>(0x7Fu, 0x41u, 0x41u, 0x41u, 0x3Eu),
    array<u32, 5>(0x7Fu, 0x04u, 0x08u, 0x10u, 0x7Fu),
    array<u32, 5>(0x7Fu, 0x08u, 0x10u, 0x20u, 0x7Fu),
    array<u32, 5>(0x3Eu, 0x41u, 0x41u, 0x41u, 0x3Eu),
    array<u32, 5>(0x7Fu, 0x09u, 0x09u, 0x09u, 0x06u),
    array<u32, 5>(0x7Fu, 0x09u, 0x19u, 0x29u, 0x46u),
    array<u32, 5>(0x03u, 0x01u, 0x7Fu, 0x01u, 0x03u),
    array<u32, 5>(0x63u, 0x14u, 0x08u, 0x14u, 0x63u),
);

const CELL_BASE_COLOR: vec3<f32> = vec3(0.04, 0.04, 0.05);
const GATE_INPUT_RADIUS: f32 = 0.085;
const GATE_INPUT_SINGLE_CENTER: vec2<f32> = vec2(-0.39, 0.0);
const GATE_INPUT_TOP_CENTER: vec2<f32> = vec2(-0.39, -0.26);
const GATE_INPUT_BOTTOM_CENTER: vec2<f32> = vec2(-0.39, 0.26);

fn sharp_cut(distance: f32, epsilon: f32) -> f32 {
    if (distance <= -epsilon) {
        return 1.0;
    }
    if (distance >= epsilon) {
        return 0.0;
    }
    let t = smoothstep(-epsilon, epsilon, distance);
    let sharpened = t * t * t;
    return 1.0 - sharpened;
}

fn segment_mask(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half_width: f32) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0);
    let closest = a + ab * t;
    let distance = length(p - closest);
    return sharp_cut(distance - half_width, 0.012);
}

fn rounded_box_mask(centered: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(centered) - half_size + vec2(radius, radius);
    let outside = length(max(q, vec2(0.0, 0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    let distance = outside + inside - radius;
    return sharp_cut(distance, 0.01);
}

fn circle_mask(centered: vec2<f32>, center: vec2<f32>, radius: f32) -> f32 {
    let distance = length(centered - center);
    return sharp_cut(distance - radius, 0.006);
}

fn diamond_mask(centered: vec2<f32>, center: vec2<f32>, radius: f32) -> f32 {
    let delta = abs(centered - center);
    let distance = delta.x + delta.y - radius;
    return sharp_cut(distance, 0.006);
}

fn render_lit_circle(
    circle: f32,
    glow: f32,
    charge_level: f32,
    uncharged_color: vec3<f32>,
    charged_color: vec3<f32>,
    glow_color: vec3<f32>,
) -> vec3<f32> {
    let node_color = mix(uncharged_color, charged_color, charge_level * 0.8);
    var color = mix(CELL_BASE_COLOR, node_color, circle);
    color += glow_color * (charge_level * glow);
    return color;
}

fn gate_input_mask(centered: vec2<f32>, tag: u32) -> f32 {
    let dual_input_mask = max(
        circle_mask(centered, GATE_INPUT_TOP_CENTER, GATE_INPUT_RADIUS),
        circle_mask(centered, GATE_INPUT_BOTTOM_CENTER, GATE_INPUT_RADIUS),
    );
    let single_input_mask = circle_mask(centered, GATE_INPUT_SINGLE_CENTER, GATE_INPUT_RADIUS);
    return select(dual_input_mask, single_input_mask, tag == 2u || tag == 3u);
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

    let gate_ix = tag - 2u;
    let len = GATE_LABEL_LENGTHS[gate_ix];
    let glyphs = GATE_LABEL_LETTERS[gate_ix];
    // Keep labels tighter and more central so the gate body has side room for connectors.
    let uv = (local_uv - vec2(0.24, 0.2)) / vec2(0.52, 0.6);

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

fn render_empty(local_uv: vec2<f32>, centered: vec2<f32>, charge: u32) -> vec3<f32> {
    _ = local_uv;
    let radius = length(centered);
    let circle = sharp_cut(radius - 0.23, 0.01);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);
    let charge_level = f32(charge) / 255.0;
    return render_lit_circle(circle, glow, charge_level, CELL_BASE_COLOR, vec3(0.85, 0.9, 0.95), vec3(0.18, 0.32, 0.42));
}

fn render_source(local_uv: vec2<f32>, centered: vec2<f32>, charge: u32) -> vec3<f32> {
    _ = local_uv;

    let radius = length(centered);
    let circle = sharp_cut(radius - 0.23, 0.01);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);
    let charge_level = f32(charge) / 255.0;
    return render_lit_circle(circle, glow, charge_level, vec3(0.28, 0.16, 0.10), vec3(0.95, 0.82, 0.68), vec3(0.24, 0.18, 0.08));
}

fn render_basic_gate(local_uv: vec2<f32>, centered: vec2<f32>, charge: u32, circuit: vec4<u32>) -> vec3<f32> {
    let radius = length(centered);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);
    let tag = circuit.x & 0xffu;
    let charge_level = f32(charge) / 255.0;
    let body_mask = rounded_box_mask(centered, vec2(0.39, 0.27), 0.08);
    let border_mask = body_mask - rounded_box_mask(centered, vec2(0.35, 0.23), 0.07);
    let wire_blue = vec3(0.18, 0.44, 0.64);
    let body_color = mix(vec3(0.24, 0.28, 0.32), vec3(0.92, 0.96, 1.0), charge_level * 0.42);
    let label = gate_label_mask(local_uv, tag) * body_mask;
    let output_center = vec2(0.385, 0.0);

    let output_outer = diamond_mask(centered, output_center, 0.1);
    let gate_border_mask = border_mask - output_outer;
    let input_mask = gate_input_mask(centered, tag);

    var color = vec3(0.035, 0.035, 0.045);
    color = mix(color, body_color, body_mask);
    color = mix(color, wire_blue, gate_border_mask * 0.82);
    color = mix(color, vec3(0.34, 0.92, 0.42), input_mask);
    color = mix(color, vec3(1.0, 0.88, 0.22), output_outer);
    color = mix(color, vec3(0.6, 0.07, 0.08), label);
    color += vec3(0.22, 0.28, 0.34) * (charge_level * glow * 0.24);
    return color;
}

fn render_output(local_uv: vec2<f32>, centered: vec2<f32>, charge: u32) -> vec3<f32> {
    _ = local_uv;

    let radius = length(centered);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);
    let charge_level = f32(charge) / 255.0;
    let body_mask = rounded_box_mask(centered, vec2(0.39, 0.27), 0.08);
    let border_mask = body_mask - rounded_box_mask(centered, vec2(0.35, 0.23), 0.07);
    let input_mask = circle_mask(centered, GATE_INPUT_SINGLE_CENTER, GATE_INPUT_RADIUS);
    let monitor_mask = rounded_box_mask(centered - vec2(0.08, 0.0), vec2(0.16, 0.12), 0.04);
    let bar_mask = rounded_box_mask(centered + vec2(0.18, 0.0), vec2(0.055, 0.19), 0.03);
    let body_color = mix(vec3(0.20, 0.24, 0.18), vec3(0.82, 0.96, 0.76), charge_level * 0.4);

    var color = vec3(0.035, 0.035, 0.045);
    color = mix(color, body_color, body_mask);
    color = mix(color, vec3(0.18, 0.44, 0.64), border_mask * 0.82);
    color = mix(color, vec3(0.34, 0.92, 0.42), input_mask);
    color = mix(color, vec3(0.11, 0.13, 0.10), monitor_mask);
    color = mix(color, vec3(0.99, 0.84, 0.30), monitor_mask * charge_level);
    color = mix(color, vec3(0.42, 0.92, 0.56), bar_mask * (0.45 + charge_level * 0.55));
    color += vec3(0.22, 0.30, 0.20) * (charge_level * glow * 0.22);
    return color;
}

fn render_input(local_uv: vec2<f32>, centered: vec2<f32>, charge: u32) -> vec3<f32> {
    _ = local_uv;

    let radius = length(centered);
    let glow = 1.0 - smoothstep(0.12, 0.38, radius);
    let charge_level = f32(charge) / 255.0;
    let body_mask = rounded_box_mask(centered, vec2(0.39, 0.27), 0.08);
    let border_mask = body_mask - rounded_box_mask(centered, vec2(0.35, 0.23), 0.07);
    let monitor_mask = rounded_box_mask(centered + vec2(0.08, 0.0), vec2(0.16, 0.12), 0.04);
    let bar_mask = rounded_box_mask(centered - vec2(0.18, 0.0), vec2(0.055, 0.19), 0.03);
    let output_mask = diamond_mask(centered, vec2(0.385, 0.0), 0.1);
    let body_color = mix(vec3(0.18, 0.20, 0.26), vec3(0.72, 0.82, 0.98), charge_level * 0.4);

    var color = vec3(0.035, 0.035, 0.045);
    color = mix(color, body_color, body_mask);
    color = mix(color, vec3(0.18, 0.44, 0.64), border_mask * 0.82);
    color = mix(color, vec3(0.99, 0.84, 0.30), monitor_mask * charge_level);
    color = mix(color, vec3(0.11, 0.13, 0.10), monitor_mask * (1.0 - charge_level));
    color = mix(color, vec3(0.42, 0.92, 0.56), bar_mask * (0.45 + charge_level * 0.55));
    color = mix(color, vec3(1.0, 0.88, 0.22), output_mask);
    color += vec3(0.20, 0.24, 0.34) * (charge_level * glow * 0.22);
    return color;
}

fn render_child_cell(local_uv: vec2<f32>, centered: vec2<f32>, charge: u32, circuit: vec4<u32>) -> vec3<f32> {
    _ = local_uv;
    let tag = circuit.x & 0xffu;
    let charge_level = f32(charge) / 255.0;
    let body_mask = rounded_box_mask(centered, vec2(0.39, 0.27), 0.08);
    let border_mask = body_mask - rounded_box_mask(centered, vec2(0.35, 0.23), 0.07);
    let left_top = circle_mask(centered, GATE_INPUT_TOP_CENTER, GATE_INPUT_RADIUS);
    let left_middle = circle_mask(centered, GATE_INPUT_SINGLE_CENTER, GATE_INPUT_RADIUS);
    let left_bottom = circle_mask(centered, GATE_INPUT_BOTTOM_CENTER, GATE_INPUT_RADIUS);
    let right_output = diamond_mask(centered, vec2(0.385, 0.0), 0.1);
    let core_mask = rounded_box_mask(centered, vec2(0.10, 0.10), 0.04);

    let has_top_input = tag == 12u;
    let has_middle_input = tag == 14u || tag == 15u;
    let has_bottom_input = tag == 12u;
    let has_output = tag == 13u || tag == 14u;
    let body_color = mix(vec3(0.17, 0.15, 0.28), vec3(0.72, 0.70, 0.96), charge_level * 0.3);

    var color = vec3(0.035, 0.035, 0.045);
    color = mix(color, body_color, body_mask);
    color = mix(color, vec3(0.36, 0.34, 0.72), border_mask * 0.82);
    color = mix(color, vec3(0.34, 0.92, 0.42), left_top * select(0.0, 1.0, has_top_input));
    color = mix(color, vec3(0.34, 0.92, 0.42), left_middle * select(0.0, 1.0, has_middle_input));
    color = mix(color, vec3(0.34, 0.92, 0.42), left_bottom * select(0.0, 1.0, has_bottom_input));
    color = mix(color, vec3(1.0, 0.88, 0.22), right_output * select(0.0, 1.0, has_output));
    color = mix(color, vec3(0.82, 0.78, 0.98), core_mask * 0.8);
    color += vec3(0.20, 0.18, 0.34) * (charge_level * 0.18);
    return color;
}

fn render_cell_color(coord: vec2<u32>, local_uv: vec2<f32>, centered: vec2<f32>, charge: u32, circuit: vec4<u32>) -> vec3<f32> {
    _ = coord;
    switch (circuit.x & 0xffu) {
        case 1u: {
            return render_source(local_uv, centered, charge);
        }
        case 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u: {
            return render_basic_gate(local_uv, centered, charge, circuit);
        }
        case 10u: {
            return render_output(local_uv, centered, charge);
        }
        case 11u: {
            return render_input(local_uv, centered, charge);
        }
        case 12u, 13u, 14u, 15u, 16u: {
            return render_child_cell(local_uv, centered, charge, circuit);
        }
        default: {
            return render_empty(local_uv, centered, charge);
        }
    }
}
