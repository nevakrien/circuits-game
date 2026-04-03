use serde::{Deserialize, Serialize};

use crate::{
    child_components::{ChildInstancePlan, ComponentFootprint},
    demo_scene, simulation,
    wire_render::StoredWireEdge,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ComponentId(pub u32);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentPlan {
    pub id: ComponentId,
    pub name: String,
    #[serde(default = "default_outside_shape")]
    pub outside_shape: ComponentFootprint,
    pub board_size: [u32; 2],
    #[serde(default = "default_internal_size")]
    pub internal_size: [u32; 2],
    pub cells: Vec<[u32; 4]>,
    pub wires: Vec<StoredWireEdge>,
    #[serde(default)]
    pub child_instances: Vec<ChildInstancePlan>,
    pub direct_dependencies: Vec<ComponentId>,
    pub child_mentions: Vec<ComponentId>,
}

fn default_outside_shape() -> ComponentFootprint {
    ComponentFootprint {
        width: 1,
        height: 1,
    }
}

fn default_internal_size() -> [u32; 2] {
    [simulation::GRID_WIDTH, simulation::GRID_HEIGHT]
}

impl ComponentPlan {
    pub fn new(id: ComponentId, name: impl Into<String>, board_size: [u32; 2]) -> Self {
        let len = (board_size[0] * board_size[1]) as usize;
        Self {
            id,
            name: name.into(),
            outside_shape: default_outside_shape(),
            board_size,
            internal_size: board_size,
            cells: vec![simulation::CellSnapshot::empty().words; len],
            wires: Vec::new(),
            child_instances: Vec::new(),
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
        plan.recompute_outside_shape();
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

    pub fn sync_child_links(&mut self) {
        let mut dependencies: Vec<_> = self
            .child_instances
            .iter()
            .map(|instance| instance.component_id)
            .collect();
        dependencies.sort_unstable_by_key(|id| id.0);
        dependencies.dedup();
        self.direct_dependencies = dependencies.clone();
        self.child_mentions = dependencies;
    }

    pub fn recompute_outside_shape(&mut self) {
        self.outside_shape =
            compact_footprint_for_port_counts(self.input_count(), self.output_count());
    }

    pub fn input_count(&self) -> u32 {
        self.cells
            .iter()
            .filter(|words| {
                simulation::CellSnapshot { words: **words }.kind() == simulation::CellKind::Input
            })
            .count() as u32
    }

    pub fn output_count(&self) -> u32 {
        self.cells
            .iter()
            .filter(|words| {
                simulation::CellSnapshot { words: **words }.kind() == simulation::CellKind::Output
            })
            .count() as u32
    }
}

fn compact_footprint_for_port_counts(input_count: u32, output_count: u32) -> ComponentFootprint {
    let read_write_cells = input_count.min(output_count);
    let remaining_inputs = input_count - read_write_cells;
    let remaining_outputs = output_count - read_write_cells;
    let slot_count = (read_write_cells + remaining_outputs + remaining_inputs.div_ceil(2)).max(1);
    let width = (slot_count as f32).sqrt().ceil() as u32;
    let height = slot_count.div_ceil(width);
    ComponentFootprint { width, height }
}
