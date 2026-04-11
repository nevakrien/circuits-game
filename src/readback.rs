use foldhash::{HashMap, HashSet};
use std::ops::Range;

use crate::visual_ui::FocusedScene;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleBufferRange {
    pub buffer: u32,
    pub start_word: u32,
    pub word_len: u32,
}

#[derive(Debug, Clone)]
struct LoadedBufferRange {
    buffer: u32,
    start_word: u32,
    words: Box<[u32]>,
}

#[derive(Debug, Clone, Default)]
pub struct ReadManager {
    words_per_buffer: u32,
    required_words: HashSet<u32>,
    required_ranges: Vec<VisibleBufferRange>,
    plan_dirty: bool,
    loaded_ranges: Vec<LoadedBufferRange>,
    loaded_by_buffer: HashMap<u32, Range<usize>>,
}

impl ReadManager {
    pub fn for_scene(scene: &FocusedScene) -> Self {
        let mut manager = Self::new(scene.words_per_buffer);
        manager.require_scene(scene);
        manager
    }

    pub fn new(words_per_buffer: u32) -> Self {
        Self {
            words_per_buffer,
            required_words: HashSet::default(),
            required_ranges: Vec::new(),
            plan_dirty: false,
            loaded_ranges: Vec::new(),
            loaded_by_buffer: HashMap::default(),
        }
    }

    pub fn require_bit(&mut self, buffer: u32, bit: u32) {
        let absolute_word = buffer.saturating_mul(self.words_per_buffer) + (bit / 32);
        if self.required_words.insert(absolute_word) {
            self.plan_dirty = true;
        }
    }

    pub fn required_ranges(&mut self) -> &[VisibleBufferRange] {
        if self.plan_dirty {
            let mut indices: Vec<_> = self.required_words.iter().copied().collect();
            indices.sort_unstable();
            self.required_ranges = indices_to_buffer_ranges(self.words_per_buffer, &indices);
            self.plan_dirty = false;
        }
        &self.required_ranges
    }

    pub fn load_ranges(
        &mut self,
        ranges: impl IntoIterator<Item = (VisibleBufferRange, Box<[u32]>)>,
    ) {
        self.loaded_ranges = ranges
            .into_iter()
            .map(|(range, words)| LoadedBufferRange {
                buffer: range.buffer,
                start_word: range.start_word,
                words,
            })
            .collect();
        self.loaded_ranges
            .sort_by_key(|range| (range.buffer, range.start_word));

        self.loaded_by_buffer.clear();
        let mut start = 0usize;
        while start < self.loaded_ranges.len() {
            let buffer = self.loaded_ranges[start].buffer;
            let mut end = start + 1;
            while end < self.loaded_ranges.len() && self.loaded_ranges[end].buffer == buffer {
                end += 1;
            }
            self.loaded_by_buffer.insert(buffer, start..end);
            start = end;
        }
    }

    pub fn get_bit(&self, buffer: u32, bit: u32) -> Option<bool> {
        let word_in_buffer = bit / 32;
        let bit_in_word = bit % 32;
        self.word(buffer, word_in_buffer)
            .map(|word| ((word >> bit_in_word) & 1) != 0)
    }

    fn require_scene(&mut self, scene: &FocusedScene) {
        // Any new render-time bit dependency must be registered here so the
        // viewport readback stays sparse. Falling back to whole-buffer reads
        // would reintroduce the scaling bug for large simulations.
        for gate in &scene.gates {
            if let Some(store) = scene.gate_store.get(&(scene.node, gate.id)).copied() {
                self.require_bit(store.buffer.0, store.bit.0);
            }
        }

        for wire in &scene.wires {
            if let Some((node, gate)) = wire.source_gate {
                if let Some(store) = scene.gate_store.get(&(node, gate)).copied() {
                    self.require_bit(store.buffer.0, store.bit.0);
                }
            }
        }

        for child in &scene.children {
            self.require_scene(&child.scene);
        }
    }

    fn word(&self, buffer: u32, word_in_buffer: u32) -> Option<u32> {
        let range_indices = self.loaded_by_buffer.get(&buffer)?;
        for range in &self.loaded_ranges[range_indices.clone()] {
            let end_word = range.start_word + range.words.len() as u32;
            if (range.start_word..end_word).contains(&word_in_buffer) {
                return range
                    .words
                    .get((word_in_buffer - range.start_word) as usize)
                    .copied();
            }
        }
        None
    }
}

fn indices_to_buffer_ranges(words_per_buffer: u32, indices: &[u32]) -> Vec<VisibleBufferRange> {
    let mut ranges = Vec::new();
    let mut current: Option<VisibleBufferRange> = None;

    for &absolute_word in indices {
        let buffer = absolute_word / words_per_buffer;
        let word_in_buffer = absolute_word % words_per_buffer;

        match current.as_mut() {
            Some(range)
                if range.buffer == buffer
                    && word_in_buffer == range.start_word + range.word_len =>
            {
                range.word_len += 1;
            }
            _ => {
                if let Some(range) = current.take() {
                    ranges.push(range);
                }
                current = Some(VisibleBufferRange {
                    buffer,
                    start_word: word_in_buffer,
                    word_len: 1,
                });
            }
        }
    }

    if let Some(range) = current {
        ranges.push(range);
    }

    ranges
}
