#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferAllocRange {
    pub page: u32,
    pub offset_words: u32,
    pub len_words: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferAllocHandle {
    pub page: u32,
    pub offset_words: u32,
    pub len_words: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferAllocResult {
    pub range: BufferAllocRange,
    pub handle: BufferAllocHandle,
    pub grew_pages: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferAllocError {
    EmptyAllocation,
    RequestedPageOverflow { requested: u32, capacity: u32 },
}

#[derive(Clone, Debug)]
struct LiveAlloc {
    offset_words: u32,
    len_words: u32,
    generation: u32,
    live: bool,
}

#[derive(Clone, Debug)]
struct BufferPage {
    free_ranges: Vec<(u32, u32)>,
    live_allocs: Vec<LiveAlloc>,
}

impl BufferPage {
    fn new(page_words: u32) -> Self {
        Self {
            free_ranges: vec![(0, page_words)],
            live_allocs: Vec::new(),
        }
    }

    fn alloc(&mut self, len_words: u32) -> Option<(u32, u32)> {
        let range_ix = self
            .free_ranges
            .iter()
            .position(|&(_, free_len)| free_len >= len_words)?;
        let (offset_words, free_len) = self.free_ranges[range_ix];
        let remaining = free_len - len_words;
        if remaining == 0 {
            self.free_ranges.remove(range_ix);
        } else {
            self.free_ranges[range_ix] = (offset_words + len_words, remaining);
        }

        let generation = self.live_allocs.len() as u32 + 1;
        self.live_allocs.push(LiveAlloc {
            offset_words,
            len_words,
            generation,
            live: true,
        });
        Some((offset_words, generation))
    }

    fn free(&mut self, handle: BufferAllocHandle) -> bool {
        let Some(alloc) = self.live_allocs.iter_mut().find(|alloc| {
            alloc.live
                && alloc.offset_words == handle.offset_words
                && alloc.len_words == handle.len_words
                && alloc.generation == handle.generation
        }) else {
            return false;
        };

        alloc.live = false;
        self.free_ranges.push((alloc.offset_words, alloc.len_words));
        self.free_ranges
            .sort_by_key(|&(offset_words, _)| offset_words);

        let mut merged = Vec::with_capacity(self.free_ranges.len());
        for (offset_words, len_words) in self.free_ranges.drain(..) {
            if let Some((prev_offset, prev_len)) = merged.last_mut() {
                if *prev_offset + *prev_len == offset_words {
                    *prev_len += len_words;
                    continue;
                }
            }
            merged.push((offset_words, len_words));
        }
        self.free_ranges = merged;
        true
    }

    fn used_words(&self) -> u32 {
        self.live_allocs
            .iter()
            .filter(|alloc| alloc.live)
            .map(|alloc| alloc.len_words)
            .sum()
    }
}

#[derive(Clone, Debug)]
pub struct BufferAllocator {
    page_words: u32,
    pages: Vec<BufferPage>,
}

impl BufferAllocator {
    pub fn new(page_words: u32) -> Self {
        assert!(page_words > 0);
        Self {
            page_words,
            pages: Vec::new(),
        }
    }

    pub fn page_words(&self) -> u32 {
        self.page_words
    }

    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    pub fn alloc(&mut self, len_words: u32) -> Result<BufferAllocResult, BufferAllocError> {
        if len_words == 0 {
            return Err(BufferAllocError::EmptyAllocation);
        }
        if len_words > self.page_words {
            return Err(BufferAllocError::RequestedPageOverflow {
                requested: len_words,
                capacity: self.page_words,
            });
        }

        let mut best_page = None;
        let mut best_used = 0;
        for (page_ix, page) in self.pages.iter().enumerate() {
            if page
                .free_ranges
                .iter()
                .any(|&(_, free_len)| free_len >= len_words)
            {
                let used_words = page.used_words();
                if best_page.is_none() || used_words > best_used {
                    best_page = Some(page_ix as u32);
                    best_used = used_words;
                }
            }
        }

        let (page, grew_pages) = match best_page {
            Some(page) => (page, false),
            None => {
                let page = self.pages.len() as u32;
                self.pages.push(BufferPage::new(self.page_words));
                (page, true)
            }
        };

        let (offset_words, generation) = self.pages[page as usize]
            .alloc(len_words)
            .expect("page should have enough space for allocation");
        Ok(BufferAllocResult {
            range: BufferAllocRange {
                page,
                offset_words,
                len_words,
            },
            handle: BufferAllocHandle {
                page,
                offset_words,
                len_words,
                generation,
            },
            grew_pages,
        })
    }

    pub fn free(&mut self, handle: BufferAllocHandle) {
        let Some(page) = self.pages.get_mut(handle.page as usize) else {
            return;
        };
        page.free(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_allocator_reuses_holes_before_growing_pages() {
        let mut alloc = BufferAllocator::new(8);

        let a = alloc.alloc(4).unwrap();
        let b = alloc.alloc(4).unwrap();
        assert_eq!(alloc.page_count(), 1);

        let c = alloc.alloc(4).unwrap();
        assert_eq!(c.range.page, 1);
        assert!(c.grew_pages);

        alloc.free(a.handle);
        let d = alloc.alloc(4).unwrap();
        assert_eq!(d.range.page, 0);
        assert!(!d.grew_pages);
        assert_eq!(d.range.offset_words, 0);

        let _ = b;
    }

    #[test]
    fn buffer_allocator_merges_adjacent_frees() {
        let mut alloc = BufferAllocator::new(8);
        let a = alloc.alloc(2).unwrap();
        let b = alloc.alloc(3).unwrap();
        let c = alloc.alloc(3).unwrap();

        alloc.free(b.handle);
        alloc.free(c.handle);

        let d = alloc.alloc(6).unwrap();
        assert_eq!(d.range.page, 0);
        assert_eq!(d.range.offset_words, 2);

        let _ = a;
    }
}
