@group(0) @binding(0)
var history: texture_2d<u32>;

@group(0) @binding(1)
var out_tex: texture_storage_2d<r32uint, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);

    if (x >= 8 || y >= 8) {
        return;
    }

    let y_down = max(y - 1, 0);
    let below = textureLoad(history, vec2<i32>(x, y_down), 0).x;

    textureStore(out_tex, vec2<i32>(x, y), vec4<u32>(below, 0u, 0u, 0u));
}