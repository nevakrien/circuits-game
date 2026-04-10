use std::collections::BTreeSet;

pub const DEFAULT_PAGE_WIDTH: u32 = 1024;
pub const DEFAULT_PAGE_HEIGHT: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageTag {
    Empty,
    Class(ClassId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocRange {
    pub x: u16,
    pub y: u16,
    pub z: u32,
    pub w: u16,
    pub h: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocHandle {
    pub page_id: u32,
    pub slot_index: u32,

    // changes only when an empty page is assigned to a class
    pub page_generation: u32,

    // changes each time this exact slot is allocated
    pub slot_generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocResult {
    pub range: AllocRange,
    pub handle: AllocHandle,
    pub grew_z: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotClass {
    pub id: ClassId,
    pub w: u16,
    pub h: u16,
    pub cols: u16,
    pub rows: u16,
    pub capacity: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllocError {
    UnsupportedSize { w: u16, h: u16 },
}

impl SlotClass {
    fn new(id: u16, page_width: u32, page_height: u32, w: u16, h: u16) -> Self {
        assert!(w.is_power_of_two());
        assert!(h.is_power_of_two());
        assert!(w as u32 <= page_width);
        assert!(h as u32 <= page_height);
        assert_eq!(page_width % w as u32, 0);
        assert_eq!(page_height % h as u32, 0);

        let cols = (page_width / w as u32) as u16;
        let rows = (page_height / h as u32) as u16;
        let capacity = cols as u32 * rows as u32;

        Self {
            id: ClassId(id),
            w,
            h,
            cols,
            rows,
            capacity,
        }
    }
}

#[derive(Debug)]
struct Page {
    z: u32,
    tag: PageTag,

    // increments only when assigning an empty page to a class
    page_generation: u32,

    free_count: u32,
    capacity: u32,

    // 1 = free, 0 = allocated
    free_bits: Vec<u64>,

    // per-slot generation; incremented on each successful allocation of that slot
    slot_generations: Vec<u32>,
}

impl Page {
    fn new_empty(z: u32) -> Self {
        Self {
            z,
            tag: PageTag::Empty,
            page_generation: 0,
            free_count: 0,
            capacity: 0,
            free_bits: Vec::new(),
            slot_generations: Vec::new(),
        }
    }

    fn assign_to_class(&mut self, class_id: ClassId, capacity: u32) {
        self.tag = PageTag::Class(class_id);
        self.page_generation = self.page_generation.wrapping_add(1);
        self.free_count = capacity;
        self.capacity = capacity;

        let words = (capacity as usize + 63) / 64;
        self.free_bits = vec![u64::MAX; words];

        let total_bits = words * 64;
        let used_bits = capacity as usize;
        let extra = total_bits - used_bits;
        if extra > 0 {
            let valid_mask = u64::MAX >> extra;
            *self.free_bits.last_mut().unwrap() = valid_mask;
        }

        self.slot_generations.clear();
        self.slot_generations.resize(capacity as usize, 0);
    }

    fn make_empty(&mut self) {
        self.tag = PageTag::Empty;
        self.free_count = 0;
        self.capacity = 0;
        self.free_bits.clear();
        self.slot_generations.clear();
    }

    fn is_full(&self) -> bool {
        matches!(self.tag, PageTag::Class(_)) && self.free_count == 0
    }

    fn is_empty_assigned(&self) -> bool {
        matches!(self.tag, PageTag::Class(_)) && self.free_count == self.capacity
    }

    fn fullness_key(&self) -> (u32, u32) {
        // more used is better; older z is better
        (self.capacity - self.free_count, u32::MAX - self.z)
    }

    fn slot_is_free(&self, slot: u32) -> bool {
        debug_assert!(slot < self.capacity);
        let word_idx = (slot / 64) as usize;
        let bit = slot % 64;
        (self.free_bits[word_idx] & (1u64 << bit)) != 0
    }

    // fn mark_slot_allocated(&mut self, slot: u32) {
    //     debug_assert!(slot < self.capacity);
    //     let word_idx = (slot / 64) as usize;
    //     let bit = slot % 64;
    //     self.free_bits[word_idx] &= !(1u64 << bit);
    //     self.free_count -= 1;
    // }

    fn mark_slot_free(&mut self, slot: u32) {
        debug_assert!(slot < self.capacity);
        let word_idx = (slot / 64) as usize;
        let bit = slot % 64;
        self.free_bits[word_idx] |= 1u64 << bit;
        self.free_count += 1;
    }

    fn alloc_slot(&mut self) -> (u32, u32) {
        debug_assert!(matches!(self.tag, PageTag::Class(_)));
        debug_assert!(self.free_count > 0);

        for (word_idx, word) in self.free_bits.iter_mut().enumerate() {
            if *word != 0 {
                let bit = word.trailing_zeros() as usize;
                let slot = (word_idx * 64 + bit) as u32;
                debug_assert!(slot < self.capacity);

                *word &= !(1u64 << bit);
                self.free_count -= 1;

                let genr = self.slot_generations[slot as usize].wrapping_add(1);
                self.slot_generations[slot as usize] = genr;

                return (slot, genr);
            }
        }

        unreachable!("free_count > 0 but no free bit found")
    }

    fn free_if_matching(&mut self, handle: AllocHandle) -> bool {
        if !matches!(self.tag, PageTag::Class(_)) {
            return false;
        }

        if self.page_generation != handle.page_generation {
            return false;
        }

        if handle.slot_index >= self.capacity {
            return false;
        }

        let slot = handle.slot_index;
        if self.slot_generations[slot as usize] != handle.slot_generation {
            return false;
        }

        if self.slot_is_free(slot) {
            return false;
        }

        self.mark_slot_free(slot);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CandidatePage {
    used: u32,
    age_key: u32,
    page_id: u32,
}

#[derive(Debug)]
struct ClassState {
    slot: SlotClass,

    // only partially filled pages live here
    partial_pages: BTreeSet<CandidatePage>,
}

impl ClassState {
    fn new(slot: SlotClass) -> Self {
        Self {
            slot,
            partial_pages: BTreeSet::new(),
        }
    }
}

pub struct TextureAllocator {
    page_width: u32,
    page_height: u32,
    classes: Vec<ClassState>,
    pages: Vec<Page>,

    // page ids whose tag is Empty
    global_empty_pages: Vec<u32>,

    // number of actual z slices created so far
    z_len: u32,
}

impl TextureAllocator {
    pub fn new(slot_classes: Vec<(u16, u16)>) -> Self {
        Self::with_page_size(DEFAULT_PAGE_WIDTH, DEFAULT_PAGE_HEIGHT, slot_classes)
    }

    pub fn with_page_size(
        page_width: u32,
        page_height: u32,
        slot_classes: Vec<(u16, u16)>,
    ) -> Self {
        assert!(page_width.is_power_of_two());
        assert!(page_height.is_power_of_two());
        let classes = slot_classes
            .into_iter()
            .enumerate()
            .map(|(i, (w, h))| {
                ClassState::new(SlotClass::new(i as u16, page_width, page_height, w, h))
            })
            .collect();

        Self {
            page_width,
            page_height,
            classes,
            pages: Vec::new(),
            global_empty_pages: Vec::new(),
            z_len: 0,
        }
    }

    pub fn page_size(&self) -> [u32; 2] {
        [self.page_width, self.page_height]
    }

    pub fn z_len(&self) -> u32 {
        self.z_len
    }

    pub fn alloc_exact(&mut self, w: u16, h: u16) -> Result<AllocResult, AllocError> {
        let class_id = self
            .find_exact_class(w, h)
            .ok_or(AllocError::UnsupportedSize { w, h })?;

        Ok(self.alloc_in_class(class_id))
    }

    pub fn free(&mut self, handle: AllocHandle) {
        let page_ix = handle.page_id as usize;
        if page_ix >= self.pages.len() {
            debug_assert!(false, "free called with out-of-range page_id");
            return;
        }

        let class_id = match self.pages[page_ix].tag {
            PageTag::Empty => {
                debug_assert!(false, "free called on empty page");
                return;
            }
            PageTag::Class(class_id) => class_id,
        };
        let class_ix = class_id.0 as usize;

        debug_assert!(class_ix < self.classes.len());

        // If this page is currently indexed as partial, remove that entry before mutating it.
        self.remove_page_from_partial_index(class_ix, handle.page_id);

        let did_free = {
            let page = &mut self.pages[page_ix];
            page.free_if_matching(handle)
        };

        if !did_free {
            // Stale / duplicate / mismatched handle. Put the page back into the
            // partial index if it still belongs there, then ignore.
            let page = &self.pages[page_ix];
            if matches!(page.tag, PageTag::Class(_)) && !page.is_full() && !page.is_empty_assigned()
            {
                self.reindex_partial_page(class_ix, handle.page_id);
            }
            return;
        }

        let page = &mut self.pages[page_ix];
        if page.is_empty_assigned() {
            page.make_empty();
            self.global_empty_pages.push(handle.page_id);
        } else if !page.is_full() {
            self.reindex_partial_page(class_ix, handle.page_id);
        } else {
            // full pages are intentionally not kept in the partial index
            debug_assert!(page.free_count == 0);
        }
    }

    fn find_exact_class(&self, w: u16, h: u16) -> Option<ClassId> {
        self.classes
            .iter()
            .find(|c| c.slot.w == w && c.slot.h == h)
            .map(|c| c.slot.id)
    }

    fn alloc_in_class(&mut self, class_id: ClassId) -> AllocResult {
        let class_ix = class_id.0 as usize;

        let (page_id, grew_z, came_from_partial) =
            if let Some(pid) = self.best_partial_page(class_ix) {
                (pid, false, true)
            } else if let Some(pid) = self.global_empty_pages.pop() {
                self.assign_empty_page_to_class(pid, class_id);
                (pid, false, false)
            } else {
                (self.grow_z_with_new_page(class_id), true, false)
            };

        // remove stale index entry before mutating
        if came_from_partial {
            self.remove_page_from_partial_index(class_ix, page_id);
        }

        let (slot_index, slot_generation, page_generation) = {
            let page = &mut self.pages[page_id as usize];
            let (slot_index, slot_generation) = page.alloc_slot();
            (slot_index, slot_generation, page.page_generation)
        };

        // if still partial after allocation, index it again
        if !self.pages[page_id as usize].is_full() {
            self.reindex_partial_page(class_ix, page_id);
        }

        let class = self.classes[class_ix].slot;
        let z = self.pages[page_id as usize].z;
        let (x, y) = self.slot_xy(&class, slot_index);

        AllocResult {
            range: AllocRange {
                x,
                y,
                z,
                w: class.w,
                h: class.h,
            },
            handle: AllocHandle {
                page_id,
                slot_index,
                page_generation,
                slot_generation,
            },
            grew_z,
        }
    }

    fn slot_xy(&self, class: &SlotClass, slot: u32) -> (u16, u16) {
        let col = slot % class.cols as u32;
        let row = slot / class.cols as u32;
        ((col * class.w as u32) as u16, (row * class.h as u32) as u16)
    }

    fn best_partial_page(&self, class_ix: usize) -> Option<u32> {
        self.classes[class_ix]
            .partial_pages
            .iter()
            .next_back()
            .map(|c| c.page_id)
    }

    fn candidate_for_page(&self, page_id: u32) -> CandidatePage {
        let page = &self.pages[page_id as usize];
        let (used, age_key) = page.fullness_key();
        CandidatePage {
            used,
            age_key,
            page_id,
        }
    }

    fn remove_page_from_partial_index(&mut self, class_ix: usize, page_id: u32) {
        let page = &self.pages[page_id as usize];
        if matches!(page.tag, PageTag::Class(_)) && !page.is_full() && !page.is_empty_assigned() {
            let candidate = self.candidate_for_page(page_id);
            self.classes[class_ix].partial_pages.remove(&candidate);
        }
    }

    fn reindex_partial_page(&mut self, class_ix: usize, page_id: u32) {
        let page = &self.pages[page_id as usize];
        debug_assert!(matches!(page.tag, PageTag::Class(_)));
        debug_assert!(!page.is_full());
        debug_assert!(!page.is_empty_assigned());

        let candidate = self.candidate_for_page(page_id);
        self.classes[class_ix].partial_pages.insert(candidate);
    }

    fn assign_empty_page_to_class(&mut self, page_id: u32, class_id: ClassId) {
        let class = self.classes[class_id.0 as usize].slot;
        let page = &mut self.pages[page_id as usize];

        debug_assert!(matches!(page.tag, PageTag::Empty));
        page.assign_to_class(class_id, class.capacity);
    }

    fn grow_z_with_new_page(&mut self, class_id: ClassId) -> u32 {
        let z = self.z_len;
        let page_id = self.pages.len() as u32;

        let mut page = Page::new_empty(z);
        let class = self.classes[class_id.0 as usize].slot;
        page.assign_to_class(class_id, class.capacity);

        self.pages.push(page);
        self.z_len += 1;
        page_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn make_alloc(classes: &[(u16, u16)]) -> TextureAllocator {
        TextureAllocator::new(classes.to_vec())
    }

    fn make_alloc_with_page(
        page_width: u32,
        page_height: u32,
        classes: &[(u16, u16)],
    ) -> TextureAllocator {
        TextureAllocator::with_page_size(page_width, page_height, classes.to_vec())
    }

    fn rects_overlap(a: AllocRange, b: AllocRange) -> bool {
        if a.z != b.z {
            return false;
        }

        let ax0 = a.x as u32;
        let ay0 = a.y as u32;
        let ax1 = ax0 + a.w as u32;
        let ay1 = ay0 + a.h as u32;

        let bx0 = b.x as u32;
        let by0 = b.y as u32;
        let bx1 = bx0 + b.w as u32;
        let by1 = by0 + b.h as u32;

        ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
    }

    fn assert_no_overlaps(ranges: &[AllocRange]) {
        for i in 0..ranges.len() {
            for j in (i + 1)..ranges.len() {
                assert!(
                    !rects_overlap(ranges[i], ranges[j]),
                    "overlap between {:?} and {:?}",
                    ranges[i],
                    ranges[j],
                );
            }
        }
    }

    #[test]
    fn fills_single_page_without_overlap() {
        let mut alloc = make_alloc(&[(512, 512)]);

        let a = alloc.alloc_exact(512, 512).unwrap();
        let b = alloc.alloc_exact(512, 512).unwrap();
        let c = alloc.alloc_exact(512, 512).unwrap();
        let d = alloc.alloc_exact(512, 512).unwrap();

        // One 1024x1024 page with 512x512 slots has 4 slots total.
        // These four allocations should all land in z=0 and cover the four cells exactly once.
        assert_eq!(a.range.z, 0);
        assert_eq!(b.range.z, 0);
        assert_eq!(c.range.z, 0);
        assert_eq!(d.range.z, 0);

        let ranges = [a.range, b.range, c.range, d.range];
        assert_no_overlaps(&ranges);

        let mut seen = HashSet::new();
        for r in ranges {
            assert!(seen.insert((r.z, r.x, r.y, r.w, r.h)));
        }

        let expected: HashSet<_> = [
            (0u32, 0u16, 0u16, 512u16, 512u16),
            (0u32, 512u16, 0u16, 512u16, 512u16),
            (0u32, 0u16, 512u16, 512u16, 512u16),
            (0u32, 512u16, 512u16, 512u16, 512u16),
        ]
        .into_iter()
        .collect();

        assert_eq!(seen, expected);

        // The page is now full, so the next allocation must spill into a new z slice.
        let e = alloc.alloc_exact(512, 512).unwrap();
        assert_eq!(e.range.z, 1);
        assert!(e.grew_z);
    }

    #[test]
    fn free_slot_is_reused_before_growing_z() {
        let mut alloc = make_alloc(&[(512, 512)]);

        let a = alloc.alloc_exact(512, 512).unwrap();
        let b = alloc.alloc_exact(512, 512).unwrap();
        let c = alloc.alloc_exact(512, 512).unwrap();
        let d = alloc.alloc_exact(512, 512).unwrap();

        // Page 0 is full at this point.
        assert_eq!(alloc.z_len(), 1);

        alloc.free(c.handle);

        // There is now exactly one hole back in z=0.
        // The next allocation should reuse that hole instead of opening z=1.
        let e = alloc.alloc_exact(512, 512).unwrap();
        assert_eq!(e.range.z, 0);
        assert!(!e.grew_z);

        let live = [a.range, b.range, d.range, e.range];
        assert_no_overlaps(&live);

        let all: HashSet<_> = live
            .into_iter()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();

        let expected: HashSet<_> = [
            (0u32, 0u16, 0u16, 512u16, 512u16),
            (0u32, 512u16, 0u16, 512u16, 512u16),
            (0u32, 0u16, 512u16, 512u16, 512u16),
            (0u32, 512u16, 512u16, 512u16, 512u16),
        ]
        .into_iter()
        .collect();

        assert_eq!(all, expected);
        assert_eq!(alloc.z_len(), 1);
    }

    #[test]
    fn grows_z_only_when_existing_capacity_is_exhausted() {
        let mut alloc = make_alloc(&[(1024, 1024)]);

        let a = alloc.alloc_exact(1024, 1024).unwrap();

        // First allocation needs the allocator to materialize z=0.
        assert_eq!(a.range.z, 0);
        assert!(a.grew_z);
        assert_eq!(alloc.z_len(), 1);

        let b = alloc.alloc_exact(1024, 1024).unwrap();

        // 1024x1024 consumes an entire page, so the second one must create z=1.
        assert_eq!(b.range.z, 1);
        assert!(b.grew_z);
        assert_eq!(alloc.z_len(), 2);

        alloc.free(a.handle);

        let c = alloc.alloc_exact(1024, 1024).unwrap();

        // Reusing the freed whole page should not increase z any further.
        assert_eq!(c.range.z, 0);
        assert!(!c.grew_z);
        assert_eq!(alloc.z_len(), 2);

        assert_no_overlaps(&[b.range, c.range]);
    }

    #[test]
    fn empty_page_can_be_reused_after_all_slots_are_freed() {
        let mut alloc = make_alloc(&[(512, 512)]);

        let a = alloc.alloc_exact(512, 512).unwrap();
        let b = alloc.alloc_exact(512, 512).unwrap();
        let c = alloc.alloc_exact(512, 512).unwrap();
        let d = alloc.alloc_exact(512, 512).unwrap();

        // z=0 has now been completely filled.
        assert_eq!(alloc.z_len(), 1);

        alloc.free(a.handle);
        alloc.free(b.handle);
        alloc.free(c.handle);
        alloc.free(d.handle);

        // The page is now completely empty and should be reusable as page storage.
        // The next allocation should come back from an existing slice, not create a new one.
        let e = alloc.alloc_exact(512, 512).unwrap();
        assert_eq!(e.range.z, 0);
        assert!(!e.grew_z);
        assert_eq!(alloc.z_len(), 1);
    }

    #[test]
    fn multiple_classes_do_not_overlap_ranges_within_the_same_class_usage() {
        let mut alloc = make_alloc(&[(256, 256), (512, 512)]);

        let mut allocs_256 = Vec::new();
        for _ in 0..16 {
            let r = alloc.alloc_exact(256, 256).unwrap();
            // A 1024x1024 page holds exactly 16 of these.
            // They should all live in z=0 and cover all 4x4 lattice positions.
            assert_eq!(r.range.z, 0);
            allocs_256.push(r.range);
        }
        assert_no_overlaps(&allocs_256);

        let seen_256: HashSet<_> = allocs_256
            .iter()
            .copied()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();
        assert_eq!(seen_256.len(), 16);

        let spill_256 = alloc.alloc_exact(256, 256).unwrap();
        assert_eq!(spill_256.range.z, 1);
        assert!(spill_256.grew_z);

        let mut allocs_512 = Vec::new();
        for _ in 0..4 {
            let r = alloc.alloc_exact(512, 512).unwrap().range;
            // 512x512 has its own class and therefore its own page assignment.
            // These four should pack into one page without overlap.
            allocs_512.push(r);
        }
        assert_no_overlaps(&allocs_512);

        let seen_512: HashSet<_> = allocs_512
            .iter()
            .copied()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();
        assert_eq!(seen_512.len(), 4);

        // Also check that all currently live returned ranges across both classes do not overlap.
        let mut live_all = allocs_256;
        live_all.push(spill_256.range);
        live_all.extend(allocs_512);
        assert_no_overlaps(&live_all);
    }

    #[test]
    fn freeing_middle_holes_allows_reuse_without_duplicate_ranges() {
        let mut alloc = make_alloc(&[(256, 256)]);

        let mut hs = Vec::new();
        for _ in 0..16 {
            hs.push(alloc.alloc_exact(256, 256).unwrap());
        }

        // z=0 is full with a 4x4 grid of 256x256 slots.
        assert_eq!(alloc.z_len(), 1);

        // Create a few scattered holes.
        alloc.free(hs[3].handle);
        alloc.free(hs[7].handle);
        alloc.free(hs[12].handle);

        let n1 = alloc.alloc_exact(256, 256).unwrap();
        let n2 = alloc.alloc_exact(256, 256).unwrap();
        let n3 = alloc.alloc_exact(256, 256).unwrap();

        // All three replacements should come from the existing holes in z=0.
        assert_eq!(n1.range.z, 0);
        assert_eq!(n2.range.z, 0);
        assert_eq!(n3.range.z, 0);
        assert!(!n1.grew_z);
        assert!(!n2.grew_z);
        assert!(!n3.grew_z);

        let live_ranges: Vec<_> = hs
            .into_iter()
            .enumerate()
            .filter(|(i, _)| ![3usize, 7usize, 12usize].contains(i))
            .map(|(_, a)| a.range)
            .chain([n1.range, n2.range, n3.range])
            .collect();

        assert_eq!(live_ranges.len(), 16);
        assert_no_overlaps(&live_ranges);

        let live_set: HashSet<_> = live_ranges
            .iter()
            .copied()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();

        assert_eq!(live_set.len(), 16);
        assert_eq!(alloc.z_len(), 1);
    }

    #[test]
    fn prefers_reusing_partial_page_over_opening_new_page() {
        let mut alloc = make_alloc(&[(512, 512)]);

        let a = alloc.alloc_exact(512, 512).unwrap();
        let b = alloc.alloc_exact(512, 512).unwrap();
        let c = alloc.alloc_exact(512, 512).unwrap();
        let d = alloc.alloc_exact(512, 512).unwrap();

        let e = alloc.alloc_exact(512, 512).unwrap();

        // After four allocations, z=0 is full; the fifth opens z=1.
        assert_eq!(e.range.z, 1);
        assert!(e.grew_z);
        assert_eq!(alloc.z_len(), 2);

        alloc.free(b.handle);

        // There is now a hole in the older page z=0.
        // The allocator should use that partial page rather than continuing on z=1.
        let f = alloc.alloc_exact(512, 512).unwrap();
        assert_eq!(f.range.z, 0);
        assert!(!f.grew_z);
        assert_eq!(alloc.z_len(), 2);

        let live = [a.range, c.range, d.range, e.range, f.range];
        assert_no_overlaps(&live);

        let live_set: HashSet<_> = live
            .into_iter()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();

        assert_eq!(live_set.len(), 5);
    }

    #[test]
    fn slot_coordinates_match_requested_class_dimensions() {
        let mut alloc = make_alloc(&[(128, 512)]);

        let mut ranges = Vec::new();
        for _ in 0..16 {
            let r = alloc.alloc_exact(128, 512).unwrap().range;

            // A 1024x1024 page with 128x512 slots has 8 columns and 2 rows.
            // So x should be a multiple of 128, y should be either 0 or 512,
            // and every valid slot should appear exactly once before spilling.
            assert_eq!(r.z, 0);
            assert_eq!(r.w, 128);
            assert_eq!(r.h, 512);
            assert_eq!(r.x % 128, 0);
            assert!(r.y == 0 || r.y == 512);

            ranges.push(r);
        }

        assert_no_overlaps(&ranges);

        let seen: HashSet<_> = ranges
            .iter()
            .copied()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();

        assert_eq!(seen.len(), 16);

        let spill = alloc.alloc_exact(128, 512).unwrap();
        assert_eq!(spill.range.z, 1);
        assert!(spill.grew_z);
    }

    #[test]
    fn free_then_refill_preserves_complete_cover_of_page() {
        let mut alloc = make_alloc(&[(256, 512)]);

        let mut items = Vec::new();
        for _ in 0..8 {
            items.push(alloc.alloc_exact(256, 512).unwrap());
        }

        // 256x512 gives 4 columns and 2 rows, so 8 slots total in z=0.
        assert_eq!(alloc.z_len(), 1);

        alloc.free(items[1].handle);
        alloc.free(items[5].handle);

        let r1 = alloc.alloc_exact(256, 512).unwrap();
        let r2 = alloc.alloc_exact(256, 512).unwrap();

        // These two allocations should refill the two holes exactly.
        let live: Vec<_> = items
            .into_iter()
            .enumerate()
            .filter(|(i, _)| ![1usize, 5usize].contains(i))
            .map(|(_, a)| a.range)
            .chain([r1.range, r2.range])
            .collect();

        assert_eq!(live.len(), 8);
        assert_no_overlaps(&live);

        let live_set: HashSet<_> = live
            .iter()
            .copied()
            .map(|r| (r.z, r.x, r.y, r.w, r.h))
            .collect();

        let expected: HashSet<_> = [
            (0u32, 0u16, 0u16, 256u16, 512u16),
            (0u32, 256u16, 0u16, 256u16, 512u16),
            (0u32, 512u16, 0u16, 256u16, 512u16),
            (0u32, 768u16, 0u16, 256u16, 512u16),
            (0u32, 0u16, 512u16, 256u16, 512u16),
            (0u32, 256u16, 512u16, 256u16, 512u16),
            (0u32, 512u16, 512u16, 256u16, 512u16),
            (0u32, 768u16, 512u16, 256u16, 512u16),
        ]
        .into_iter()
        .collect();

        assert_eq!(live_set, expected);
    }

    #[test]
    fn configurable_page_size_supports_small_test_limits() {
        let mut alloc = make_alloc_with_page(16, 16, &[(8, 8)]);

        let a = alloc.alloc_exact(8, 8).unwrap();
        let b = alloc.alloc_exact(8, 8).unwrap();
        let c = alloc.alloc_exact(8, 8).unwrap();
        let d = alloc.alloc_exact(8, 8).unwrap();

        assert_eq!(alloc.page_size(), [16, 16]);
        assert_eq!(a.range.z, 0);
        assert_eq!(b.range.z, 0);
        assert_eq!(c.range.z, 0);
        assert_eq!(d.range.z, 0);

        let spill = alloc.alloc_exact(8, 8).unwrap();
        assert_eq!(spill.range.z, 1);
    }
}
