struct SceneUniform {
    surface_scene_min: vec4<f32>,
    scene_size_screen_min: vec4<f32>,
    source_scale_time_pulse: vec4<f32>,
    grid_rect: vec4<f32>,
    scene_bits: vec4<u32>,
};

struct ShapeInstance {
    @location(0) min: vec2<f32>,
    @location(1) max: vec2<f32>,
    @location(2) charge: vec4<u32>,
    @location(3) shape_meta: vec4<u32>,
};

struct WireInstance {
    @location(0) start: vec2<f32>,
    @location(1) end: vec2<f32>,
    @location(2) path: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) charge: vec4<u32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) charge: vec4<u32>,
    @location(2) shape_meta: vec4<u32>,
};

struct WireVsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_px: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) charge: vec4<u32>,
    @location(3) path: vec4<f32>,
    @location(4) length_px: f32,
    @location(5) half_width_px: f32,
    @location(6) render_radius_px: f32,
};

@group(0) @binding(0)
var<uniform> uniforms: SceneUniform;

@group(0) @binding(1)
var<storage, read> current_charge: array<u32>;

@group(0) @binding(2)
var<storage, read> next_charge: array<u32>;

fn quad_pos(vertex_index: u32) -> vec2<f32> {
    let positions = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    return positions[vertex_index];
}

fn screen_to_clip(screen: vec2<f32>) -> vec4<f32> {
    let ndc = vec2<f32>(
        (screen.x / uniforms.surface_scene_min.x) * 2.0 - 1.0,
        1.0 - (screen.y / uniforms.surface_scene_min.y) * 2.0,
    );
    return vec4<f32>(ndc, 0.0, 1.0);
}

fn world_to_screen(world: vec2<f32>) -> vec2<f32> {
    return uniforms.scene_size_screen_min.zw
        + (world - uniforms.source_scale_time_pulse.xy) * uniforms.source_scale_time_pulse.z;
}

fn world_to_clip(world: vec2<f32>) -> vec4<f32> {
    return screen_to_clip(world_to_screen(world));
}

fn charge_active(charge: vec4<u32>) -> bool {
    let source_mode = charge.z;
    if source_mode == 2u {
        return false;
    }
    let absolute_word = charge.x * uniforms.scene_bits.x + (charge.y / 32u);
    let bit_in_word = charge.y % 32u;
    var word: u32 = current_charge[absolute_word];
    if source_mode == 1u {
        word = next_charge[absolute_word];
    }
    return ((word >> bit_in_word) & 1u) != 0u;
}

fn rounded_rect_alpha(local: vec2<f32>, radius: f32) -> f32 {
    let half_size = vec2<f32>(0.5, 0.5);
    let p = abs(local - half_size) - (half_size - vec2<f32>(radius, radius));
    let outside = length(max(p, vec2<f32>(0.0, 0.0)));
    let inside = min(max(p.x, p.y), 0.0);
    let distance = outside + inside - radius;
    return 1.0 - smoothstep(-0.01, 0.01, distance);
}

fn circle_alpha(local: vec2<f32>) -> f32 {
    let centered = local * 2.0 - vec2<f32>(1.0, 1.0);
    let dist = length(centered);
    return 1.0 - smoothstep(0.92, 1.0, dist);
}

fn mix_color(a: vec4<f32>, b: vec4<f32>, t: f32) -> vec4<f32> {
    return a * (1.0 - t) + b * t;
}

fn capsule_mask(local_px: vec2<f32>, length_px: f32, radius_px: f32) -> f32 {
    let closest_x = clamp(local_px.x, 0.0, length_px);
    let distance = length(vec2<f32>(local_px.x - closest_x, local_px.y));
    return 1.0 - smoothstep(radius_px - 1.0, radius_px + 1.0, distance);
}

fn gate_label_len(gate_tag: u32) -> u32 {
    switch gate_tag {
        case 1u, 6u { return 4u; }
        case 2u, 4u, 7u, 8u { return 3u; }
        case 3u { return 2u; }
        case 5u { return 3u; }
        default { return 0u; }
    }
}

fn gate_label_char(gate_tag: u32, index: u32) -> u32 {
    switch gate_tag {
        case 1u {
            let chars = array<u32, 4>(78u, 65u, 78u, 68u);
            return chars[index];
        }
        case 2u {
            let chars = array<u32, 3>(65u, 78u, 68u);
            return chars[index];
        }
        case 3u {
            let chars = array<u32, 2>(79u, 82u);
            return chars[index];
        }
        case 4u {
            let chars = array<u32, 3>(78u, 79u, 82u);
            return chars[index];
        }
        case 5u {
            let chars = array<u32, 3>(88u, 79u, 82u);
            return chars[index];
        }
        case 6u {
            let chars = array<u32, 4>(88u, 78u, 79u, 82u);
            return chars[index];
        }
        case 7u {
            let chars = array<u32, 3>(78u, 79u, 84u);
            return chars[index];
        }
        case 8u {
            let chars = array<u32, 3>(78u, 79u, 80u);
            return chars[index];
        }
        default { return 0u; }
    }
}

fn glyph_row_bits(ch: u32, row: u32) -> u32 {
    switch ch {
        case 65u {
            let rows = array<u32, 7>(14u, 17u, 17u, 31u, 17u, 17u, 17u);
            return rows[row];
        }
        case 68u {
            let rows = array<u32, 7>(30u, 17u, 17u, 17u, 17u, 17u, 30u);
            return rows[row];
        }
        case 78u {
            let rows = array<u32, 7>(17u, 25u, 21u, 19u, 17u, 17u, 17u);
            return rows[row];
        }
        case 79u {
            let rows = array<u32, 7>(14u, 17u, 17u, 17u, 17u, 17u, 14u);
            return rows[row];
        }
        case 80u {
            let rows = array<u32, 7>(30u, 17u, 17u, 30u, 16u, 16u, 16u);
            return rows[row];
        }
        case 82u {
            let rows = array<u32, 7>(30u, 17u, 17u, 30u, 20u, 18u, 17u);
            return rows[row];
        }
        case 84u {
            let rows = array<u32, 7>(31u, 4u, 4u, 4u, 4u, 4u, 4u);
            return rows[row];
        }
        case 88u {
            let rows = array<u32, 7>(17u, 17u, 10u, 4u, 10u, 17u, 17u);
            return rows[row];
        }
        default { return 0u; }
    }
}

fn gate_label_alpha(local: vec2<f32>, gate_tag: u32) -> f32 {
    let label_len = gate_label_len(gate_tag);
    if label_len == 0u || uniforms.source_scale_time_pulse.z < 0.45 {
        return 0.0;
    }

    let text_min = vec2<f32>(0.18, 0.28);
    let text_max = vec2<f32>(0.82, 0.72);
    if any(local < text_min) || any(local > text_max) {
        return 0.0;
    }

    let uv = (local - text_min) / (text_max - text_min);
    let total_columns = f32(label_len * 6u - 1u);
    let total_rows = 7.0;
    let pixel_x = uv.x * total_columns;
    let pixel_y = uv.y * total_rows;
    let char_index = u32(floor(pixel_x / 6.0));
    if char_index >= label_len {
        return 0.0;
    }

    let glyph_x = u32(floor(pixel_x - f32(char_index) * 6.0));
    let glyph_y = u32(floor(pixel_y));
    if glyph_x >= 5u || glyph_y >= 7u {
        return 0.0;
    }

    let row_bits = glyph_row_bits(gate_label_char(gate_tag, char_index), glyph_y);
    let column_mask = 1u << (4u - glyph_x);
    let filled = f32(select(0u, 1u, (row_bits & column_mask) != 0u));

    let grid = fract(vec2<f32>(pixel_x, pixel_y));
    let edge = min(min(grid.x, 1.0 - grid.x), min(grid.y, 1.0 - grid.y));
    return filled * smoothstep(0.02, 0.18, edge);
}

@vertex
fn vs_board(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    let local = quad_pos(vertex_index);
    let screen = uniforms.surface_scene_min.zw + local * uniforms.scene_size_screen_min.xy;
    var out: VsOut;
    out.clip_pos = screen_to_clip(screen);
    out.local = local;
    out.charge = vec4<u32>(0u);
    out.shape_meta = vec4<u32>(0u);
    return out;
}

@fragment
fn fs_board(in: VsOut) -> @location(0) vec4<f32> {
    let screen = uniforms.surface_scene_min.zw + in.local * uniforms.scene_size_screen_min.xy;
    let world = uniforms.source_scale_time_pulse.xy
        + (screen - uniforms.scene_size_screen_min.zw) / max(uniforms.source_scale_time_pulse.z, 0.0001);
    let panel_bg = vec4<f32>(0.0048, 0.0060, 0.0088, 1.0);
    let board_bg = vec4<f32>(0.0075, 0.0103, 0.0160, 1.0);
    let grid_line = vec4<f32>(0.035, 0.051, 0.086, 1.0);
    var color = panel_bg;
    if world.x >= uniforms.grid_rect.x && world.x <= uniforms.grid_rect.z && world.y >= uniforms.grid_rect.y && world.y <= uniforms.grid_rect.w {
        color = board_bg;
        let cell = 88.0;
        let local_grid = world - uniforms.grid_rect.xy;
        let gx = abs(fract(local_grid.x / cell) - 0.5);
        let gy = abs(fract(local_grid.y / cell) - 0.5);
        let line_w = 0.008 / max(uniforms.source_scale_time_pulse.z, 0.35);
        let line = 1.0 - smoothstep(0.5 - line_w, 0.5, min(gx, gy));
        color = mix_color(color, grid_line, line * 0.65);
    } else if uniforms.scene_bits.y != 0u {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return color;
}

@vertex
fn vs_shape(instance: ShapeInstance, @builtin(vertex_index) vertex_index: u32) -> VsOut {
    let local = quad_pos(vertex_index);
    let world = instance.min + (instance.max - instance.min) * local;
    var out: VsOut;
    out.clip_pos = world_to_clip(world);
    out.local = local;
    out.charge = instance.charge;
    out.shape_meta = instance.shape_meta;
    return out;
}

@fragment
fn fs_shape(in: VsOut) -> @location(0) vec4<f32> {
    let kind = in.shape_meta.x;
    let gate_tag = in.shape_meta.y;
    let is_active = charge_active(in.charge);
    var alpha = 1.0;
    var color = vec4<f32>(0.2, 0.24, 0.3, 1.0);

    if kind == 0u {
        alpha = rounded_rect_alpha(in.local, 0.16);
        let off = vec4<f32>(0.032, 0.046, 0.071, 0.96);
        let on = vec4<f32>(0.052, 0.30, 0.16, 0.96);
        color = mix_color(off, on, select(0.0, 1.0, is_active));
        let edge = 1.0 - rounded_rect_alpha(clamp((in.local - 0.04) / 0.92, vec2<f32>(0.0), vec2<f32>(1.0)), 0.16);
        color = mix_color(color, vec4<f32>(0.33, 0.39, 0.48, 1.0), edge * 0.65);
        let icon = select(0.35, 0.65, gate_tag == 7u || gate_tag == 8u);
        let stripe = 1.0 - smoothstep(0.0, 0.08, abs(in.local.y - icon));
        color = vec4<f32>(color.rgb + stripe * vec3<f32>(0.05, 0.06, 0.08), color.a);
        let text_alpha = gate_label_alpha(in.local, gate_tag);
        let text_color = mix_color(
            vec4<f32>(0.82, 0.87, 0.94, 1.0),
            vec4<f32>(0.94, 0.99, 0.96, 1.0),
            select(0.0, 1.0, is_active),
        );
        color = mix_color(color, text_color, text_alpha);
    } else if kind == 1u {
        alpha = rounded_rect_alpha(in.local, 0.14);
        color = vec4<f32>(0.023, 0.030, 0.043, 0.92);
    } else {
        alpha = circle_alpha(in.local);
        if kind == 2u {
            color = mix_color(vec4<f32>(0.05, 0.10, 0.18, 0.85), vec4<f32>(0.19, 0.39, 0.88, 1.0), select(0.0, 1.0, is_active));
        } else if kind == 3u {
            color = mix_color(vec4<f32>(0.05, 0.16, 0.10, 0.85), vec4<f32>(0.20, 0.78, 0.32, 1.0), select(0.0, 1.0, is_active));
        } else if kind == 4u {
            color = mix_color(vec4<f32>(0.16, 0.08, 0.03, 0.85), vec4<f32>(0.88, 0.50, 0.16, 1.0), select(0.0, 1.0, is_active));
        } else if kind == 5u {
            color = mix_color(vec4<f32>(0.11, 0.05, 0.16, 0.85), vec4<f32>(0.71, 0.36, 0.92, 1.0), select(0.0, 1.0, is_active));
        } else if kind == 6u {
            color = mix_color(vec4<f32>(0.12, 0.05, 0.16, 0.85), vec4<f32>(0.74, 0.41, 0.92, 1.0), select(0.0, 1.0, is_active));
        } else if kind == 7u {
            color = mix_color(vec4<f32>(0.09, 0.13, 0.20, 0.88), vec4<f32>(0.33, 0.61, 0.98, 1.0), select(0.0, 1.0, is_active));
        } else {
            color = mix_color(vec4<f32>(0.09, 0.18, 0.11, 0.88), vec4<f32>(0.38, 0.88, 0.52, 1.0), select(0.0, 1.0, is_active));
        }
    }

    if alpha <= 0.001 {
        discard;
    }
    return vec4<f32>(color.rgb, color.a * alpha);
}

@vertex
fn vs_wire(instance: WireInstance, @builtin(vertex_index) vertex_index: u32) -> WireVsOut {
    let local = quad_pos(vertex_index);
    let side = local.y * 2.0 - 1.0;
    let start_screen = world_to_screen(instance.start);
    let end_screen = world_to_screen(instance.end);
    let delta = end_screen - start_screen;
    let length_px = max(length(delta), 0.001);
    let dir = delta / length_px;
    let perp = vec2<f32>(-dir.y, dir.x);
    let thickness_px = instance.path.w;
    let render_radius_px = thickness_px * 2.4;
    let along_px = mix(-render_radius_px, length_px + render_radius_px, local.x);
    let across_px = side * render_radius_px;
    let screen = start_screen + dir * along_px + perp * across_px;

    var out: WireVsOut;
    out.clip_pos = screen_to_clip(screen);
    out.local_px = vec2<f32>(along_px, across_px);
    out.color = instance.color;
    out.charge = instance.charge;
    out.path = instance.path;
    out.length_px = length_px;
    out.half_width_px = thickness_px;
    out.render_radius_px = render_radius_px;
    return out;
}

@fragment
fn fs_wire(in: WireVsOut) -> @location(0) vec4<f32> {
    let is_active = charge_active(in.charge);
    var color = in.color;
    let half_width_px = in.half_width_px;
    let render_radius_px = in.render_radius_px;
    let wire_mask = capsule_mask(in.local_px, in.length_px, half_width_px);
    let render_mask = capsule_mask(in.local_px, in.length_px, render_radius_px);
    if render_mask <= 0.001 {
        discard;
    }

    let side_px = abs(in.local_px.y);
    let center_glow = 1.0 - smoothstep(0.0, half_width_px, side_px);
    if is_active {
        let pulse_time = uniforms.source_scale_time_pulse.w * bitcast<f32>(uniforms.scene_bits.w) * 1.85;
        let distance_along_px = clamp(in.local_px.x, 0.0, in.length_px)
            + in.path.x * max(in.path.z, 0.001) * uniforms.source_scale_time_pulse.z;
        let dot_spacing_px = 28.0;
        let dot_radius_px = half_width_px * 2.35;
        let along_to_center = abs(fract(distance_along_px / dot_spacing_px - pulse_time) - 0.5)
            * dot_spacing_px;
        let sphere_dist = length(vec2<f32>(along_to_center, side_px));
        let sphere = 1.0 - smoothstep(dot_radius_px * 0.18, dot_radius_px, sphere_dist);
        let sphere_core = 1.0 - smoothstep(dot_radius_px * 0.04, dot_radius_px * 0.48, sphere_dist);
        let sphere_highlight = 1.0
            - smoothstep(
                dot_radius_px * 0.02,
                dot_radius_px * 0.22,
                length(vec2<f32>(along_to_center - dot_radius_px * 0.22, side_px + dot_radius_px * 0.24)),
            );
        let growth = 1.0 - smoothstep(half_width_px * 0.6, dot_radius_px * 1.05, along_to_center);
        let ribbon = 1.0 - smoothstep(half_width_px * 0.28, half_width_px * 0.95, side_px);
        let base_wire = color.rgb * 0.78 + vec3<f32>(0.05, 0.08, 0.12) * center_glow;
        let energized_wire = color.rgb * (1.05 + growth * 0.18)
            + vec3<f32>(0.10, 0.16, 0.24) * ribbon
            + vec3<f32>(0.04, 0.07, 0.10) * center_glow;
        let orb_color = mix(
            energized_wire,
            vec3<f32>(0.88, 0.95, 1.0),
            sphere_core * 0.72 + sphere_highlight * 0.28,
        );
        color = vec4<f32>(
            mix(base_wire, energized_wire, wire_mask) + (orb_color - energized_wire) * sphere,
            color.a,
        );
        let alpha = max(wire_mask * 0.94, sphere * render_mask);
        return vec4<f32>(color.rgb, alpha);
    } else {
        color = vec4<f32>(color.rgb * 0.40 + vec3<f32>(0.01, 0.015, 0.02) * center_glow, color.a);
        return vec4<f32>(color.rgb, wire_mask * 0.55);
    }
}
