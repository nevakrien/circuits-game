@group(0) @binding(0)
var history: texture_3d<u32>;

@group(0) @binding(1)
var circuits: texture_3d<u32>;

@group(0) @binding(2)
var out_tex: texture_storage_3d<rgba8uint, write>;

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

fn store_byte(packed: ptr<function, vec4<u32>>, coord: vec2<u32>, value: u32) {
    switch (byte_channel(coord)) {
        case 0u: {
            (*packed).x = value;
        }
        case 1u: {
            (*packed).y = value;
        }
        case 2u: {
            (*packed).z = value;
        }
        default: {
            (*packed).w = value;
        }
    }
}

fn update_empty(
    dims: vec3<u32>,
    coord: vec3<u32>,
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

fn update_noop(
    dims: vec3<u32>,
    coord: vec3<u32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    _ = current_charge;
    _ = circuit;
    return read_input_charge(dims, coord, payload.x);
}

fn update_source(
    dims: vec3<u32>,
    coord: vec3<u32>,
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

fn charge_bool(value: bool) -> u32 {
    return select(0u, 0xffu, value);
}

fn read_bool(coord: vec3<u32>) -> bool {
    return (read_byte(history, coord) & 0xffu) != 0u;
}

fn decode_same_layer_input(input_ref: u32, coord: vec3<u32>) -> vec3<u32> {
    return vec3u(input_ref & 0xffffu, (input_ref >> 16u) & 0xffffu, coord.z);
}

fn read_input_charge(dims: vec3<u32>, coord: vec3<u32>, input_ref: u32) -> u32 {
    if (input_ref == 0xffffffffu) {
        return 0u;
    }

    let src = min(decode_same_layer_input(input_ref, coord), dims - vec3u(1u, 1u, 1u));
    return read_byte(history, src) & 0xffu;
}

fn read_input_bool(dims: vec3<u32>, coord: vec3<u32>, input_ref: u32) -> bool {
    return read_input_charge(dims, coord, input_ref) != 0u;
}

fn update_not(
    dims: vec3<u32>,
    coord: vec3<u32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    _ = current_charge;
    _ = circuit;
    return charge_bool(!read_input_bool(dims, coord, payload.x));
}

fn update_binary_gate(
    dims: vec3<u32>,
    coord: vec3<u32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    _ = current_charge;
    let lhs = read_input_bool(dims, coord, payload.x);
    let rhs = read_input_bool(dims, coord, payload.y);

    switch (circuit.x & 0xffu) {
        case 4u: {
            return charge_bool(lhs && rhs);
        }
        case 5u: {
            return charge_bool(lhs || rhs);
        }
        case 6u: {
            return charge_bool(lhs != rhs);
        }
        case 7u: {
            return charge_bool(!(lhs && rhs));
        }
        case 8u: {
            return charge_bool(!(lhs || rhs));
        }
        case 9u: {
            return charge_bool(lhs == rhs);
        }
        default: {
            return 0u;
        }
    }
}

fn update_tag(
    dims: vec3<u32>,
    coord: vec3<u32>,
    current_charge: u32,
    circuit: vec4<u32>,
    payload: vec3<u32>,
) -> u32 {
    switch (circuit.x & 0xffu) {
        case 0u: {
            return update_empty(dims, coord, current_charge, circuit, payload);
        }
        case 1u: {
            return update_source(dims, coord, current_charge, circuit, payload);
        }
        case 2u: {
            return update_noop(dims, coord, current_charge, circuit, payload);
        }
        case 3u: {
            return update_not(dims, coord, current_charge, circuit, payload);
        }
        case 4u, 5u, 6u, 7u, 8u, 9u: {
            return update_binary_gate(dims, coord, current_charge, circuit, payload);
        }
        default: {
            return current_charge;
        }
    }
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(circuits);
    let packed_dims = textureDimensions(history);

    if (any(id >= packed_dims)) {
        return;
    }

    let base_coord = vec3u(id.x * 2u, id.y * 2u, id.z);
    var next_packed = vec4<u32>(0u, 0u, 0u, 0u);

    for (var dy = 0u; dy < 2u; dy += 1u) {
        for (var dx = 0u; dx < 2u; dx += 1u) {
            let coord = base_coord + vec3u(dx, dy, 0u);

            if (any(coord >= dims)) {
                continue;
            }

            let current_charge = read_byte(history, coord) & 0xffu;
            let circuit = textureLoad(circuits, vec3<i32>(coord), 0);
            let payload = circuit.yzw;
            let next_charge = update_tag(dims, coord, current_charge, circuit, payload);
            store_byte(&next_packed, coord.xy, next_charge & 0xffu);
        }
    }

    textureStore(out_tex, vec3<i32>(id), next_packed);
}
