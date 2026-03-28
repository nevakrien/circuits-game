@group(0) @binding(0)
var tex: texture_2d<u32>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) v: u32) -> VsOut {
    var pos = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0),
        vec2( 3.0, -1.0),
        vec2(-1.0,  3.0),
    );

    let clip = pos[v];
    var out: VsOut;
    out.position = vec4(clip, 0.0, 1.0);
    out.uv = vec2(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    let coord = min(vec2<i32>(in.uv * vec2<f32>(size)), size - vec2(1, 1));
    let v = textureLoad(tex, coord, 0).x;

    if (v == 1u) {
        return vec4(1.0, 1.0, 1.0, 1.0);
    } else {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
}
