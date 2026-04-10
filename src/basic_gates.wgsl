struct BasicGateWorker {
    tgt_word_index: u32,
    instruction_start: u32,
    instruction_len: u32,
}

struct BasicGateInstruction {
    op: u32,
    dst_bit_in_word: u32,
    src_a_bit_index: u32,
    src_b_bit_index: u32,
}

@group(0) @binding(0)
var<storage, read> read_words: array<u32>;

@group(0) @binding(1)
var<storage, read_write> write_words: array<u32>;

@group(0) @binding(2)
var<storage, read> workers: array<BasicGateWorker>;

@group(0) @binding(3)
var<storage, read> instructions: array<BasicGateInstruction>;

fn load_read_bit(bit_index: u32) -> u32 {
    let word = read_words[bit_index / 32u];
    return (word >> (bit_index % 32u)) & 1u;
}

fn write_bit(word: u32, bit_index: u32, bit_value: u32) -> u32 {
    let mask = 1u << bit_index;
    return (word & ~mask) | ((bit_value & 1u) << bit_index);
}

fn eval_basic_gate(op: u32, a: u32, b: u32) -> u32 {
    switch op {
        case 1u: {
            return 1u - (a & b);
        }
        case 2u: {
            return a & b;
        }
        case 3u: {
            return a | b;
        }
        case 4u: {
            return 1u - (a | b);
        }
        case 5u: {
            return a ^ b;
        }
        case 6u: {
            return 1u - (a ^ b);
        }
        case 7u: {
            return 1u - a;
        }
        default: {
            return a;
        }
    }
}

@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let worker = workers[id.x];
    var word = 0u;

    for (var i = 0u; i < worker.instruction_len; i++) {
        let instruction = instructions[worker.instruction_start + i];
        let a = load_read_bit(instruction.src_a_bit_index);
        let b = load_read_bit(instruction.src_b_bit_index);
        let bit = eval_basic_gate(instruction.op, a, b);
        word = write_bit(word, instruction.dst_bit_in_word, bit);
    }

    write_words[worker.tgt_word_index] = word;
}
