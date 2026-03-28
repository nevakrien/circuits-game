@group(0) @binding(0)
var history: texture_2d_array<u32>;

@group(0) @binding(1)
var out_tex: texture_storage_2d<r32uint, write>;

@group(0) @binding(2)
var<uniform> u: vec4<u32>;

fn wrap(v: i32, m: i32) -> i32 {
    return (v + m) % m;
}

@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);

    let current = i32(u.x);
    let len = i32(u.y);
    let prev = wrap(current - 1, len);

    let y_down = max(y - 1, 0);
    let below = textureLoad(history, vec2<i32>(x, y_down), prev,0).x;

    textureStore(out_tex, vec2<i32>(x, y), vec4<u32>(below, 0, 0, 0));
}