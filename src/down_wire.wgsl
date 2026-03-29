@group(0) @binding(0)
var history: texture_3d<u32>;

@group(0) @binding(1)
var circuits: texture_3d<u32>;

@group(0) @binding(2)
var out_tex: texture_storage_3d<rgba8uint, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(history);

    if (any(id >= dims)) {
        return;
    }

    let coord = vec3<i32>(id);
    let current_charge = textureLoad(history, coord, 0).x & 0xffu;
    let circuit = textureLoad(circuits, coord, 0);
    let payload = circuit.yzw & vec3u(0xffu, 0xffu, 0xffu);

    var next_charge = current_charge;
    switch (circuit.x & 0xffu) {
        case 0u: {
            next_charge = 0u;
        }
        case 1u: {
            next_charge = payload.x;
        }
        case 2u: {
            let src = min(payload, dims - vec3u(1u, 1u, 1u));
            next_charge = textureLoad(history, vec3<i32>(src), 0).x & 0xffu;
        }
        default: {
            next_charge = current_charge;
        }
    }

    textureStore(out_tex, coord, vec4<u32>(next_charge, 0u, 0u, 0u));
}
