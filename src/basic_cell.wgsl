@group(0) @binding(0)
var history: texture_3d<u32>;

@group(0) @binding(1)
var circuits: texture_3d<u32>;

@group(0) @binding(2)
var out_tex: texture_storage_3d<rgba8uint, write>;

fn update_noop(
    dims: vec3<u32>,
    coord: vec3<i32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    _ = dims;
    _ = coord;
    _ = current_charge;
    _ = circuit;
    _ = payload;
    return 0u;
}

fn update_source(
    dims: vec3<u32>,
    coord: vec3<i32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    _ = dims;
    _ = coord;
    _ = current_charge;
    _ = circuit;
    return payload.x;
}

fn update_wire(
    dims: vec3<u32>,
    coord: vec3<i32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    _ = coord;
    _ = current_charge;
    _ = circuit;

    let src = min(payload, dims - vec3u(1u, 1u, 1u));
    return textureLoad(history, vec3<i32>(src), 0).x & 0xffu;
}

fn update_tag(
    dims: vec3<u32>,
    coord: vec3<i32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    switch (circuit.x & 0xffu) {
        case 0u: {
            return update_noop(dims, coord, current_charge, circuit, payload);
        }
        case 1u: {
            return update_source(dims, coord, current_charge, circuit, payload);
        }
        case 2u: {
            return update_wire(dims, coord, current_charge, circuit, payload);
        }
        default: {
            return current_charge;
        }
    }
}

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
    let next_charge = update_tag(dims, coord, current_charge, circuit, payload);

    textureStore(out_tex, coord, vec4<u32>(next_charge, 0u, 0u, 0u));
}
