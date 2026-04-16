@group(0) @binding(1)
var<storage, read> current_charge: array<u32>;

@group(0) @binding(2)
var<storage, read> next_charge: array<u32>;

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
