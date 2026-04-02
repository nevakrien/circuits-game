use std::{collections::HashMap, fmt, path::Path};

use egui_wgpu::wgpu;
use serde::{Deserialize, Serialize};

use crate::{
    component_plan::{ComponentId, ComponentPlan},
    simulation::{self, BoardTextures},
    wire_render::WireRenderInfo,
    wires::GridCell,
};

pub const SCHEMATIC_FILE_EXTENSION: &str = "circuit_schematic";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SchematicFile {
    version: u32,
    root_component_id: ComponentId,
    next_component_id: u32,
    components: Vec<ComponentPlan>,
}

#[derive(Debug)]
pub enum LevelContextError {
    MissingRoot(ComponentId),
    MissingDependency {
        component: ComponentId,
        dependency: ComponentId,
    },
    DirectSelfCycle(ComponentId),
    DependencyCycle(Vec<ComponentId>),
    Io(String),
    Encode(String),
    Decode(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Visiting,
    Done,
}

impl fmt::Display for LevelContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoot(root) => write!(f, "missing root component {:?}", root),
            Self::MissingDependency {
                component,
                dependency,
            } => write!(
                f,
                "component {:?} references missing dependency {:?}",
                component, dependency
            ),
            Self::DirectSelfCycle(id) => {
                write!(f, "component {:?} depends on itself", id)
            }
            Self::DependencyCycle(path) => {
                write!(f, "component dependency cycle detected: ")?;
                for (index, component_id) in path.iter().enumerate() {
                    if index > 0 {
                        write!(f, " -> ")?;
                    }
                    write!(f, "{:?}", component_id)?;
                }
                Ok(())
            }
            Self::Io(message) => write!(f, "{message}"),
            Self::Encode(message) => write!(f, "{message}"),
            Self::Decode(message) => write!(f, "{message}"),
        }
    }
}

pub struct LevelContext {
    root_component_id: ComponentId,
    next_component_id: u32,
    components: HashMap<ComponentId, ComponentPlan>,
}

impl LevelContext {
    pub fn with_starter_root() -> Self {
        let root_component_id = ComponentId(1);
        let root_plan = ComponentPlan::starter_root(root_component_id);
        let mut components = HashMap::new();
        components.insert(root_component_id, root_plan);
        Self {
            root_component_id,
            next_component_id: root_component_id.0 + 1,
            components,
        }
    }

    pub fn root_component_id(&self) -> ComponentId {
        self.root_component_id
    }

    pub fn root_component(&self) -> Option<&ComponentPlan> {
        self.components.get(&self.root_component_id)
    }

    pub fn component(&self, id: ComponentId) -> Option<&ComponentPlan> {
        self.components.get(&id)
    }

    pub fn component_mut(&mut self, id: ComponentId) -> Option<&mut ComponentPlan> {
        self.components.get_mut(&id)
    }

    pub fn validate(&self) -> Result<(), LevelContextError> {
        if !self.components.contains_key(&self.root_component_id) {
            return Err(LevelContextError::MissingRoot(self.root_component_id));
        }

        for plan in self.components.values() {
            for dependency in &plan.direct_dependencies {
                if *dependency == plan.id {
                    return Err(LevelContextError::DirectSelfCycle(plan.id));
                }
                if !self.components.contains_key(dependency) {
                    return Err(LevelContextError::MissingDependency {
                        component: plan.id,
                        dependency: *dependency,
                    });
                }
            }
        }

        let mut marks: HashMap<ComponentId, Mark> = HashMap::new();
        let mut stack = Vec::new();

        for component_id in self.components.keys().copied() {
            if marks.contains_key(&component_id) {
                continue;
            }
            self.detect_cycle(component_id, &mut marks, &mut stack)?;
        }

        Ok(())
    }

    fn detect_cycle(
        &self,
        component_id: ComponentId,
        marks: &mut HashMap<ComponentId, Mark>,
        stack: &mut Vec<ComponentId>,
    ) -> Result<(), LevelContextError> {
        marks.insert(component_id, Mark::Visiting);
        stack.push(component_id);

        let plan = self
            .components
            .get(&component_id)
            .ok_or(LevelContextError::MissingRoot(component_id))?;
        for dependency in &plan.direct_dependencies {
            match marks.get(dependency).copied() {
                Some(Mark::Done) => continue,
                Some(Mark::Visiting) => {
                    if let Some(start_index) = stack.iter().position(|id| id == dependency) {
                        let mut cycle = stack[start_index..].to_vec();
                        cycle.push(*dependency);
                        return Err(LevelContextError::DependencyCycle(cycle));
                    }
                }
                None => self.detect_cycle(*dependency, marks, stack)?,
            }
        }

        stack.pop();
        marks.insert(component_id, Mark::Done);
        Ok(())
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), LevelContextError> {
        self.validate()?;
        let file = SchematicFile {
            version: 1,
            root_component_id: self.root_component_id,
            next_component_id: self.next_component_id,
            components: self.components.values().cloned().collect(),
        };
        let encoded = bincode::serialize(&file).map_err(|error| {
            LevelContextError::Encode(format!("failed to encode schematic: {error}"))
        })?;
        std::fs::write(path, encoded).map_err(|error| {
            LevelContextError::Io(format!(
                "failed to write schematic {}: {error}",
                path.display()
            ))
        })
    }

    pub fn load_from_path(path: &Path) -> Result<Self, LevelContextError> {
        let bytes = std::fs::read(path).map_err(|error| {
            LevelContextError::Io(format!(
                "failed to read schematic {}: {error}",
                path.display()
            ))
        })?;
        let file: SchematicFile = bincode::deserialize(&bytes).map_err(|error| {
            LevelContextError::Decode(format!("failed to decode schematic: {error}"))
        })?;

        let mut components = HashMap::new();
        for component in file.components {
            components.insert(component.id, component);
        }

        let context = Self {
            root_component_id: file.root_component_id,
            next_component_id: file.next_component_id.max(file.root_component_id.0 + 1),
            components,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn upload_component_to_board(
        &self,
        component_id: ComponentId,
        board: &BoardTextures,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<WireRenderInfo, LevelContextError> {
        let Some(plan) = self.components.get(&component_id) else {
            return Err(LevelContextError::MissingRoot(component_id));
        };

        let mut circuit_cells = vec![[0u32; 4]; simulation::board_cell_count()];
        let mut startup_charge_texels = vec![[0u8; 4]; simulation::packed_charge_texel_count()];

        let width = plan.board_size[0].min(simulation::GRID_WIDTH);
        let height = plan.board_size[1].min(simulation::GRID_HEIGHT);
        for y in 0..height {
            for x in 0..width {
                let Some(index) = plan.index_of(x, y) else {
                    continue;
                };
                let board_index = simulation::output_slot_index(x, y, 0);
                circuit_cells[board_index] = plan.cells[index];
                let startup_value = if plan.cells[index][0] == 1 {
                    plan.cells[index][1] as u8
                } else {
                    0
                };
                simulation::write_packed_charge(&mut startup_charge_texels, x, y, 0, startup_value);
            }
        }

        board.write_all_circuit_cells(queue, &circuit_cells);
        for buffer_index in 0..simulation::CHARGE_BUFFER_COUNT {
            board.write_all_charge_texels(queue, buffer_index, &startup_charge_texels);
        }

        let mut wire_render_info = WireRenderInfo::new();
        for wire in &plan.wires {
            wire_render_info.add_wire_edge(wire.clone());
        }
        Ok(wire_render_info)
    }

    pub fn refresh_component_from_board(
        &mut self,
        component_id: ComponentId,
        board: &BoardTextures,
        wire_render_info: &WireRenderInfo,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), LevelContextError> {
        let Some(plan) = self.components.get_mut(&component_id) else {
            return Err(LevelContextError::MissingRoot(component_id));
        };

        let width = plan.board_size[0].min(simulation::GRID_WIDTH);
        let height = plan.board_size[1].min(simulation::GRID_HEIGHT);
        for y in 0..height {
            for x in 0..width {
                let Some(index) = plan.index_of(x, y) else {
                    continue;
                };
                let grid_cell = GridCell { x, y };
                plan.cells[index] = board.read_cell(device, queue, grid_cell, 0).words;
            }
        }

        plan.wires = wire_render_info.wire_edges().cloned().collect();
        Ok(())
    }
}
