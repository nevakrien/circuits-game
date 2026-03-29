@group(0) @binding(0)
var read_charge_tex: texture_3d<u32>;

@group(0) @binding(1)
var write_charge_tex: texture_3d<u32>;

struct WireRenderParams {
    view: vec4<f32>,
    surface: vec4<f32>,
    board: vec4<f32>,
}

@group(0) @binding(2)
var<uniform> params: WireRenderParams;

struct WireInstance {
    @location(0) start: vec2<f32>,
    @location(1) end: vec2<f32>,
    @location(2) source_coord: vec4<u32>,
    @location(3) path: vec4<f32>,
    @location(4) color: vec4<f32>,
}

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local_px: vec2<f32>,
    @location(1) @interpolate(flat) path_range: vec2<f32>,
    @location(2) @interpolate(flat) segment_length_px: f32,
    @location(3) @interpolate(flat) thickness_px: f32,
    @location(4) @interpolate(flat) source_coord: vec3<u32>,
    @location(5) @interpolate(flat) color: vec4<f32>,
}

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

fn board_to_clip(board_pos: vec2<f32>) -> vec2<f32> {
    let uv = board_pos / params.board.xy;
    let camera_uv = (uv - vec2(0.5, 0.5) - params.view.zw) / params.view.xy + vec2(0.5, 0.5);
    return vec2(camera_uv.x * 2.0 - 1.0, 1.0 - camera_uv.y * 2.0);
}

@vertex
fn vs_main(instance: WireInstance, @builtin(vertex_index) vertex_index: u32) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2(0.0, -1.0),
        vec2(1.0, -1.0),
        vec2(1.0, 1.0),
        vec2(0.0, -1.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0),
    );

    let local = quad[vertex_index];
    let start_clip = board_to_clip(instance.start);
    let end_clip = board_to_clip(instance.end);
    let clip_delta = end_clip - start_clip;
    let clip_to_screen = vec2(params.surface.x * 0.5, -params.surface.y * 0.5);
    let screen_to_clip = vec2(2.0 / params.surface.x, -2.0 / params.surface.y);
    let screen_delta = clip_delta * clip_to_screen;
    let length_px = max(length(screen_delta), 0.0001);
    let direction = screen_delta / length_px;
    let perp = vec2(-direction.y, direction.x);
    let half_width = instance.path.w * 0.5;
    let along_px = mix(-half_width, length_px + half_width, local.x);
    let across_px = local.y * half_width;
    let screen_pos = screen_delta / length_px * along_px + perp * across_px;
    let clip_pos = start_clip + screen_pos * screen_to_clip;

    var out: VsOut;
    out.position = vec4(clip_pos, 0.0, 1.0);
    out.local_px = vec2(along_px, across_px);
    out.path_range = instance.path.xy;
    out.segment_length_px = length_px;
    out.thickness_px = instance.path.w;
    out.source_coord = instance.source_coord.xyz;
    out.color = instance.color;
    return out;
}

fn capsule_mask(local_px: vec2<f32>, length_px: f32, radius_px: f32) -> f32 {
    let closest_x = clamp(local_px.x, 0.0, length_px);
    let distance = length(vec2(local_px.x - closest_x, local_px.y));
    return 1.0 - smoothstep(radius_px - 1.0, radius_px + 1.0, distance);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let radius_px = in.thickness_px * 0.5;
    let mask = capsule_mask(in.local_px, in.segment_length_px, radius_px);
    if (mask <= 0.0) {
        discard;
    }

    let read_level = f32(read_byte(read_charge_tex, in.source_coord) & 0xffu) / 255.0;
    let write_level = f32(read_byte(write_charge_tex, in.source_coord) & 0xffu) / 255.0;
    let segment_t = clamp(in.local_px.x / max(in.segment_length_px, 0.0001), 0.0, 1.0);
    let path_t = mix(in.path_range.x, in.path_range.y, segment_t);
    let charge = mix(read_level, write_level, path_t);

    let dark_blue = in.color.rgb * 0.28;
    let bright_blue = in.color.rgb;
    let center_glow = 1.0 - smoothstep(0.0, radius_px, abs(in.local_px.y));
    let color = mix(dark_blue, bright_blue, charge) + vec3(0.03, 0.08, 0.14) * center_glow * charge * 0.35;
    let alpha = mask * in.color.a;
    return vec4(color, alpha);
}
