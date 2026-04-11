struct CrossWriteWorker {
    tgt_word_index: u32,
    instruction_start: u32,
    instruction_len: u32,
}

struct CrossWriteInstruction {
    src_bit_index: u32,
    tgt_bit_in_word: u32,
}

@group(0) @binding(0)
var<storage, read> src_words: array<u32>;

@group(0) @binding(1)
var<storage, read_write> dst_words: array<u32>;

@group(0) @binding(2)
var<storage, read> workers: array<CrossWriteWorker>;

@group(0) @binding(3)
var<storage, read> instructions: array<CrossWriteInstruction>;

fn load_src_bit(bit_index: u32) -> u32 {
    let word = src_words[bit_index / 32u];
    return (word >> (bit_index % 32u)) & 1u;
}

fn write_bit(word: u32, bit_index: u32, bit_value: u32) -> u32 {
    let mask = 1u << bit_index;
    return (word & ~mask) | ((bit_value & 1u) << bit_index);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&workers)) {
        return;
    }

    let worker = workers[id.x];
    var word = dst_words[worker.tgt_word_index];

    for (var i = 0u; i < worker.instruction_len; i++) {
        let instruction = instructions[worker.instruction_start + i];
        let bit = load_src_bit(instruction.src_bit_index);
        word = write_bit(word, instruction.tgt_bit_in_word, bit);
    }

    dst_words[worker.tgt_word_index] = word;
}
