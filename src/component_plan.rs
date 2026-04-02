use serde::{Deserialize, Serialize};

use crate::{demo_scene, simulation, wire_render::StoredWireEdge};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ComponentId(pub u32);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentPlan {
    pub id: ComponentId,
    pub name: String,
    pub board_size: [u32; 2],
    pub cells: Vec<[u32; 4]>,
    pub wires: Vec<StoredWireEdge>,
    pub direct_dependencies: Vec<ComponentId>,
    pub child_mentions: Vec<ComponentId>,
}

impl ComponentPlan {
    pub fn new(id: ComponentId, name: impl Into<String>, board_size: [u32; 2]) -> Self {
        let len = (board_size[0] * board_size[1]) as usize;
        Self {
            id,
            name: name.into(),
            board_size,
            cells: vec![simulation::CellSnapshot::empty().words; len],
            wires: Vec::new(),
            direct_dependencies: Vec::new(),
            child_mentions: Vec::new(),
        }
    }

    pub fn starter_root(id: ComponentId) -> Self {
        let mut plan = Self::new(
            id,
            "Starter",
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        );
        let demo = demo_scene::starter_component();
        for placed_cell in demo.cells {
            if let Some(index) = plan.index_of(placed_cell.grid_cell.x, placed_cell.grid_cell.y) {
                plan.cells[index] = placed_cell.snapshot.words;
            }
        }
        plan.wires = demo.wires;
        plan
    }

    pub fn index_of(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.board_size[0] || y >= self.board_size[1] {
            return None;
        }
        Some((y * self.board_size[0] + x) as usize)
    }

    pub fn cell_words_at(&self, x: u32, y: u32) -> Option<[u32; 4]> {
        self.index_of(x, y).map(|index| self.cells[index])
    }

    pub fn set_cell_words_at(&mut self, x: u32, y: u32, words: [u32; 4]) -> bool {
        let Some(index) = self.index_of(x, y) else {
            return false;
        };
        self.cells[index] = words;
        true
    }
}
