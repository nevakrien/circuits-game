struct GateRenderStyle {
    text_scale_floor: f32,
    stripe_strength: f32,
    border_strength: f32,
    fill_boost: f32,
};

fn rounded_rect_alpha(local: vec2<f32>, radius: f32) -> f32 {
    let half_size = vec2<f32>(0.5, 0.5);
    let p = abs(local - half_size) - (half_size - vec2<f32>(radius, radius));
    let outside = length(max(p, vec2<f32>(0.0, 0.0)));
    let inside = min(max(p.x, p.y), 0.0);
    let distance = outside + inside - radius;
    return 1.0 - smoothstep(-0.01, 0.01, distance);
}

fn mix_color(a: vec4<f32>, b: vec4<f32>, t: f32) -> vec4<f32> {
    return a * (1.0 - t) + b * t;
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

fn gate_label_alpha(
    local: vec2<f32>,
    gate_tag: u32,
    text_visibility_scale: f32,
    text_scale_floor: f32,
) -> f32 {
    let label_len = gate_label_len(gate_tag);
    if label_len == 0u || text_visibility_scale < text_scale_floor {
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

fn render_gate(
    local: vec2<f32>,
    gate_tag: u32,
    is_active: bool,
    text_visibility_scale: f32,
    style: GateRenderStyle,
) -> vec4<f32> {
    let alpha = rounded_rect_alpha(local, 0.16);
    if alpha <= 0.001 {
        discard;
    }

    let off = vec4<f32>(0.032, 0.046, 0.071, 0.96);
    let on = vec4<f32>(0.052, 0.30, 0.16, 0.96);
    var color = mix_color(off, on, select(0.0, 1.0, is_active));
    color = vec4<f32>(color.rgb + style.fill_boost, color.a);

    let inset_local = clamp((local - 0.04) / 0.92, vec2<f32>(0.0), vec2<f32>(1.0));
    let edge = 1.0 - rounded_rect_alpha(inset_local, 0.16);
    color = mix_color(color, vec4<f32>(0.33, 0.39, 0.48, 1.0), edge * style.border_strength);

    let icon = select(0.35, 0.65, gate_tag == 7u || gate_tag == 8u);
    let stripe = 1.0 - smoothstep(0.0, 0.08, abs(local.y - icon));
    color = vec4<f32>(
        color.rgb + stripe * vec3<f32>(0.05, 0.06, 0.08) * style.stripe_strength,
        color.a,
    );

    let text_alpha = gate_label_alpha(local, gate_tag, text_visibility_scale, style.text_scale_floor);
    let text_color = mix_color(
        vec4<f32>(0.82, 0.87, 0.94, 1.0),
        vec4<f32>(0.94, 0.99, 0.96, 1.0),
        select(0.0, 1.0, is_active),
    );
    color = mix_color(color, text_color, text_alpha);
    return vec4<f32>(color.rgb, color.a * alpha);
}

fn scene_gate_style() -> GateRenderStyle {
    return GateRenderStyle(
        0.0,
        1.0,
        0.65,
        0.0,
    );
}

fn ui_gate_style() -> GateRenderStyle {
    return GateRenderStyle(
        0.0,
        1.15,
        0.82,
        0.016,
    );
}
