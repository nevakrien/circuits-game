use egui_wgpu::wgpu;
use foldhash::HashMap;
use foldhash::HashSet;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub u32);

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Bits(pub u32);

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BitsIndex(pub BufferId, pub Bits);

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ByteIndex(pub BufferId, pub u32);

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WordIndex(pub BufferId, pub u32);

pub struct WorkingMem {
    pub mem: Vec<ChargeBuffer>,
    //src->tgt
    pub bit_cross: HashMap<(BufferId, WordIndex), HashSet<(Bits, u8)>>,
    //TODO add these later
    // pub byte_cross:HashMap<(BufferId,WordIndex),HashSet<(u32,u8)>>,
    // pub word_cross:HashSet<(WordIndex,WordIndex)>,
}

// TODO actually figure out the correct size
const MAX_INSTRUCTIONS: usize = 1 << 8;
const MAX_WORKERS: usize = 1 << 8;

impl WorkingMem {
    pub fn queue_bit_write(
        &mut self,
        src: BitsIndex,
        tgt: BitsIndex,
    ) -> Option<(BufferId, WordIndex)> {
        if src == tgt {
            return None;
        }

        let tgt_word_byte_offset = (tgt.1.0 >> 5) << 2;
        let word = WordIndex(tgt.0, tgt_word_byte_offset);
        let key = (src.0, word);
        self.bit_cross
            .entry(key)
            .or_default()
            .insert((src.1, (tgt.1.0 % 32) as u8));
        Some(key)
    }
    pub fn make_bit_cross(&self) -> Vec<PreparedBitCross> {
        let mut grouped: HashMap<(BufferId, BufferId), HashMap<u32, HashSet<(Bits, u8)>>> =
            HashMap::default();

        for (&(src_buffer, WordIndex(tgt_buffer, tgt_word_byte_offset)), set) in &self.bit_cross {
            let by_word = grouped.entry((src_buffer, tgt_buffer)).or_default();
            let merged = by_word.entry(tgt_word_byte_offset).or_default();
            merged.extend(set.iter().copied());
        }

        let mut out = Vec::new();

        for ((src_buffer, tgt_buffer), by_word) in grouped {
            let mut by_word: Vec<(u32, HashSet<(Bits, u8)>)> = by_word.into_iter().collect();
            by_word.sort_by_key(|(tgt_word_byte_offset, _)| *tgt_word_byte_offset);

            let mut cur = PreparedBitCross {
                src_buffer,
                tgt_buffer,
                workers: Vec::with_capacity(MAX_WORKERS.min(by_word.len())),
                instructions: Vec::with_capacity(MAX_INSTRUCTIONS),
            };

            for (tgt_word_byte_offset, set) in &by_word {
                let mut local: Vec<(Bits, u8)> = set.iter().copied().collect();
                local.sort_by_key(|(Bits(src_bit), tgt_bit_in_word)| {
                    (*tgt_bit_in_word as u32, *src_bit)
                });

                if local.is_empty() {
                    continue;
                }

                let mut local_i = 0usize;

                while local_i < local.len() {
                    if cur.workers.len() == MAX_WORKERS
                        || cur.instructions.len() == MAX_INSTRUCTIONS
                    {
                        if !cur.workers.is_empty() || !cur.instructions.is_empty() {
                            out.push(cur);
                        }
                        cur = PreparedBitCross {
                            src_buffer,
                            tgt_buffer,
                            workers: Vec::with_capacity(MAX_WORKERS.min(by_word.len())),
                            instructions: Vec::with_capacity(MAX_INSTRUCTIONS),
                        };
                    }

                    let remaining_instruction_slots = MAX_INSTRUCTIONS - cur.instructions.len();
                    let remaining_worker_slots = MAX_WORKERS - cur.workers.len();

                    if remaining_instruction_slots == 0 || remaining_worker_slots == 0 {
                        out.push(cur);
                        cur = PreparedBitCross {
                            src_buffer,
                            tgt_buffer,
                            workers: Vec::with_capacity(MAX_WORKERS.min(by_word.len())),
                            instructions: Vec::with_capacity(MAX_INSTRUCTIONS),
                        };
                        continue;
                    }

                    let take = remaining_instruction_slots.min(local.len() - local_i);

                    let instruction_start = cur.instructions.len() as u32;

                    for &(src_bit, tgt_bit_in_word) in &local[local_i..local_i + take] {
                        debug_assert!(tgt_bit_in_word < 32);
                        cur.instructions.push(BitCrossInstruction {
                            src_bit,
                            tgt_bit_in_word: Bits(tgt_bit_in_word as u32),
                        });
                    }

                    cur.workers.push(BitCrossWorker {
                        tgt_word_byte_offset: *tgt_word_byte_offset,
                        instruction_start,
                        instruction_len: take as u32,
                    });

                    local_i += take;
                }
            }

            if !cur.workers.is_empty() || !cur.instructions.is_empty() {
                out.push(cur);
            }
        }

        out
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BitCrossWorker {
    pub tgt_word_byte_offset: u32,
    pub instruction_start: u32,
    pub instruction_len: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BitCrossInstruction {
    pub src_bit: Bits,
    pub tgt_bit_in_word: Bits,
}

#[derive(Debug, Clone)]
pub struct PreparedBitCross {
    pub src_buffer: BufferId,
    pub tgt_buffer: BufferId,
    pub workers: Vec<BitCrossWorker>,
    pub instructions: Vec<BitCrossInstruction>,
}

pub struct ChargeBuffer {
    pub memory: [wgpu::Buffer; 2],
    pub size: u32,
}

pub struct ChargeAlloc {
    pub cur_buffer: BufferId,
    pub bit_idx: u32,
    pub total_bits: u32,
}

fn __align_up(x: u32, a: u32) -> u32 {
    debug_assert!(a.is_power_of_two());
    debug_assert!(a != 0);
    debug_assert!(x <= u32::MAX - (a - 1));
    (x + (a - 1)) & !(a - 1)
}

impl ChargeAlloc {
    pub fn new(total_bits: u32) -> Self {
        assert!(total_bits >= 32);
        Self {
            cur_buffer: BufferId(0),
            bit_idx: 0,
            total_bits,
        }
    }

    fn next_buffer(&mut self) {
        self.cur_buffer.0 += 1;
        self.bit_idx = 0;
    }

    pub fn reserve_alloc(&mut self, size: u32) {
        if self.bit_idx.saturating_add(size) > self.total_bits {
            self.next_buffer()
        }
    }

    fn prepare_alloc(&mut self, size: u32) {
        assert!(size <= self.total_bits);
        let aligned = __align_up(self.bit_idx, size);
        if aligned + size > self.total_bits {
            self.next_buffer();
        } else {
            self.bit_idx = aligned;
        }
    }

    pub fn alloc_bit(&mut self) -> BitsIndex {
        self.prepare_alloc(1);
        let i = self.bit_idx;
        self.bit_idx += 1;
        BitsIndex(self.cur_buffer, Bits(i))
    }

    pub fn alloc_byte(&mut self) -> ByteIndex {
        self.prepare_alloc(8);
        let i = self.bit_idx;
        self.bit_idx += 8;
        ByteIndex(self.cur_buffer, i >> 3)
    }

    pub fn alloc_word(&mut self) -> WordIndex {
        self.prepare_alloc(32);
        let i = self.bit_idx;
        self.bit_idx += 32;
        WordIndex(self.cur_buffer, i >> 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_mem() -> WorkingMem {
        WorkingMem {
            mem: Vec::new(),
            bit_cross: HashMap::default(),
        }
    }

    #[test]
    fn alloc_aligns_byte_and_word_requests_and_rolls_to_next_buffer() {
        let mut alloc = ChargeAlloc::new(32);

        let bit = alloc.alloc_bit();
        let byte = alloc.alloc_byte();
        let word = alloc.alloc_word();

        assert_eq!(bit, BitsIndex(BufferId(0), Bits(0)));
        assert_eq!(byte, ByteIndex(BufferId(0), 1));
        assert_eq!(word, WordIndex(BufferId(1), 0));
        assert_eq!(alloc.cur_buffer, BufferId(1));
        assert_eq!(alloc.bit_idx, 32);
    }

    #[test]
    fn queue_bit_write_tracks_word_start_and_bit_offset_within_word() {
        let mut mem = empty_mem();

        let src = BitsIndex(BufferId(0), Bits(5));
        let tgt = BitsIndex(BufferId(1), Bits(13));

        let queued = mem.queue_bit_write(src, tgt);

        assert_eq!(queued, Some((BufferId(0), WordIndex(BufferId(1), 0))));

        let entries = mem
            .bit_cross
            .get(&(BufferId(0), WordIndex(BufferId(1), 0)))
            .expect("cross-buffer write should be queued");

        assert!(entries.contains(&(Bits(5), 13)));
    }

    #[test]
    fn make_bit_cross_groups_workers_by_buffer_pair_and_target_word() {
        let mut mem = empty_mem();

        let _ = mem.queue_bit_write(
            BitsIndex(BufferId(0), Bits(9)),
            BitsIndex(BufferId(1), Bits(13)),
        );
        let _ = mem.queue_bit_write(
            BitsIndex(BufferId(0), Bits(5)),
            BitsIndex(BufferId(1), Bits(7)),
        );
        let _ = mem.queue_bit_write(
            BitsIndex(BufferId(0), Bits(17)),
            BitsIndex(BufferId(1), Bits(35)),
        );
        let _ = mem.queue_bit_write(
            BitsIndex(BufferId(0), Bits(5)),
            BitsIndex(BufferId(1), Bits(7)),
        );
        let _ = mem.queue_bit_write(
            BitsIndex(BufferId(2), Bits(1)),
            BitsIndex(BufferId(1), Bits(2)),
        );

        let mut prepared = mem.make_bit_cross();
        prepared.sort_by_key(|cross| (cross.src_buffer, cross.tgt_buffer));

        assert_eq!(prepared.len(), 2);

        let src0_to_tgt1 = &prepared[0];
        assert_eq!(src0_to_tgt1.src_buffer, BufferId(0));
        assert_eq!(src0_to_tgt1.tgt_buffer, BufferId(1));
        assert_eq!(src0_to_tgt1.workers.len(), 2);
        assert_eq!(src0_to_tgt1.instructions.len(), 3);

        assert_eq!(
            src0_to_tgt1.workers,
            vec![
                BitCrossWorker {
                    tgt_word_byte_offset: 0,
                    instruction_start: 0,
                    instruction_len: 2,
                },
                BitCrossWorker {
                    tgt_word_byte_offset: 4,
                    instruction_start: 2,
                    instruction_len: 1,
                },
            ]
        );
        assert_eq!(
            src0_to_tgt1.instructions,
            vec![
                BitCrossInstruction {
                    src_bit: Bits(5),
                    tgt_bit_in_word: Bits(7),
                },
                BitCrossInstruction {
                    src_bit: Bits(9),
                    tgt_bit_in_word: Bits(13),
                },
                BitCrossInstruction {
                    src_bit: Bits(17),
                    tgt_bit_in_word: Bits(3),
                },
            ]
        );

        let src2_to_tgt1 = &prepared[1];
        assert_eq!(src2_to_tgt1.src_buffer, BufferId(2));
        assert_eq!(src2_to_tgt1.tgt_buffer, BufferId(1));
        assert_eq!(
            src2_to_tgt1.workers,
            vec![BitCrossWorker {
                tgt_word_byte_offset: 0,
                instruction_start: 0,
                instruction_len: 1,
            }]
        );
        assert_eq!(
            src2_to_tgt1.instructions,
            vec![BitCrossInstruction {
                src_bit: Bits(1),
                tgt_bit_in_word: Bits(2),
            }]
        );
    }
}
