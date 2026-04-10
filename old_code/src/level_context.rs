use std::{collections::HashMap, fmt, path::Path};

use egui_wgpu::wgpu;
use serde::{Deserialize, Serialize};

use crate::{
    child_components::{ChildInstancePlan, validate_component_shapes},
    component_plan::{ComponentId, ComponentPlan},
    game_constants::GameConstants,
    simulation::{self, BoardTextures, CellSnapshot},
    wire_render::{StoredWireEdge, WireRenderInfo},
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
    InvalidComponentShape {
        component: ComponentId,
        message: String,
    },
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
            Self::InvalidComponentShape { component, message } => {
                write!(f, "component {:?} has invalid shape: {message}", component)
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

struct ComposedComponentView {
    cells: Vec<[u32; 4]>,
    wires: Vec<StoredWireEdge>,
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

    pub fn components(&self) -> impl Iterator<Item = &ComponentPlan> {
        self.components.values()
    }

    pub fn create_component(
        &mut self,
        name: impl Into<String>,
        board_size: [u32; 2],
    ) -> ComponentId {
        let id = ComponentId(self.next_component_id);
        self.next_component_id += 1;
        self.components
            .insert(id, ComponentPlan::new(id, name, board_size));
        id
    }

    pub fn validate(&self) -> Result<(), LevelContextError> {
        let constants = GameConstants::default();
        if !self.components.contains_key(&self.root_component_id) {
            return Err(LevelContextError::MissingRoot(self.root_component_id));
        }

        for plan in self.components.values() {
            validate_component_shapes(
                plan.outside_shape.as_array(),
                plan.internal_size,
                &constants,
            )
            .map_err(|error| LevelContextError::InvalidComponentShape {
                component: plan.id,
                message: format!("{error:?}"),
            })?;
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

        for component in components.values_mut() {
            component.recompute_outside_shape();
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
        self.validate()?;
        let plan = self
            .components
            .get(&component_id)
            .ok_or(LevelContextError::MissingRoot(component_id))?;
        let view = self.compose_component_view(component_id)?;

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
                circuit_cells[board_index] = view.cells[index];
                let startup_value = if view.cells[index][0] == 1 {
                    view.cells[index][1] as u8
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
        for wire in &view.wires {
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
        let child_regions = self
            .components
            .get(&component_id)
            .ok_or(LevelContextError::MissingRoot(component_id))
            .map(|plan| child_instance_regions(&plan.child_instances, self))?;
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
                if child_region_contains(&child_regions, GridCell { x, y }) {
                    plan.cells[index] = CellSnapshot::empty().words;
                    continue;
                }
                let grid_cell = GridCell { x, y };
                plan.cells[index] = board.read_cell(device, queue, grid_cell, 0).words;
            }
        }

        let _ = child_regions;
        plan.wires = wire_render_info.wire_edges().cloned().collect();
        plan.recompute_outside_shape();
        Ok(())
    }

    fn compose_component_view(
        &self,
        component_id: ComponentId,
    ) -> Result<ComposedComponentView, LevelContextError> {
        let plan = self
            .components
            .get(&component_id)
            .ok_or(LevelContextError::MissingRoot(component_id))?;
        let cell_count = (plan.board_size[0] * plan.board_size[1]) as usize;
        let mut cells = plan.cells.clone();
        cells.resize(cell_count, CellSnapshot::empty().words);
        let mut wires = plan.wires.clone();

        for child in &plan.child_instances {
            let child_plan = self.components.get(&child.component_id).ok_or(
                LevelContextError::MissingDependency {
                    component: component_id,
                    dependency: child.component_id,
                },
            )?;
            let child_view = self.compose_component_view(child.component_id)?;
            stamp_child_view(
                &mut cells,
                &mut wires,
                plan.board_size,
                child_plan,
                child,
                &child_view,
            );
        }

        Ok(ComposedComponentView { cells, wires })
    }
}

fn child_instance_regions(
    child_instances: &[ChildInstancePlan],
    context: &LevelContext,
) -> Vec<[u32; 4]> {
    child_instances
        .iter()
        .filter_map(|instance| {
            let Some(plan) = context.component(instance.component_id) else {
                return None;
            };
            Some([
                instance.origin[0],
                instance.origin[1],
                plan.outside_shape.width,
                plan.outside_shape.height,
            ])
        })
        .collect()
}

fn child_region_contains(child_regions: &[[u32; 4]], cell: GridCell) -> bool {
    child_regions.iter().any(|region| {
        let x_range = region[0]..region[0] + region[2];
        let y_range = region[1]..region[1] + region[3];
        x_range.contains(&cell.x) && y_range.contains(&cell.y)
    })
}

fn stamp_child_view(
    parent_cells: &mut [[u32; 4]],
    _parent_wires: &mut Vec<StoredWireEdge>,
    parent_board_size: [u32; 2],
    child_plan: &ComponentPlan,
    instance: &ChildInstancePlan,
    _child_view: &ComposedComponentView,
) {
    let width = child_plan
        .outside_shape
        .width
        .min(parent_board_size[0].saturating_sub(instance.origin[0]));
    let height = child_plan
        .outside_shape
        .height
        .min(parent_board_size[1].saturating_sub(instance.origin[1]));

    let footprint_cells =
        compact_child_footprint_cells(child_plan, child_plan.outside_shape.as_array());

    for y in 0..height {
        for x in 0..width {
            let child_index = (y * child_plan.outside_shape.width + x) as usize;
            let parent_index = ((instance.origin[1] + y) * parent_board_size[0]
                + (instance.origin[0] + x)) as usize;
            parent_cells[parent_index] = footprint_cells[child_index];
        }
    }
}

fn compact_child_footprint_cells(child_plan: &ComponentPlan, footprint: [u32; 2]) -> Vec<[u32; 4]> {
    let footprint_len = (footprint[0] * footprint[1]) as usize;
    let mut cells = vec![CellSnapshot::child_noop().words; footprint_len.max(1)];
    let input_count = child_plan
        .cells
        .iter()
        .filter(|words| CellSnapshot { words: **words }.kind() == simulation::CellKind::Input)
        .count() as u32;
    let output_count = child_plan
        .cells
        .iter()
        .filter(|words| CellSnapshot { words: **words }.kind() == simulation::CellKind::Output)
        .count() as u32;
    let read_write_count = input_count.min(output_count) as usize;
    let remaining_outputs = (output_count - read_write_count as u32) as usize;
    let remaining_inputs = (input_count - read_write_count as u32) as usize;
    let mut next = 0usize;

    for _ in 0..read_write_count {
        if next >= cells.len() {
            return cells;
        }
        cells[next] = CellSnapshot::child_read_write().words;
        next += 1;
    }

    for _ in 0..remaining_outputs {
        if next >= cells.len() {
            return cells;
        }
        cells[next] = CellSnapshot::child_read().words;
        next += 1;
    }

    let write_2_count = remaining_inputs / 2;
    for _ in 0..write_2_count {
        if next >= cells.len() {
            return cells;
        }
        cells[next] = CellSnapshot::child_write().words;
        next += 1;
    }

    if remaining_inputs % 2 == 1 && next < cells.len() {
        cells[next] = CellSnapshot::child_write_1().words;
    }

    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_child_instances_reflect_updated_child_footprint_when_uploaded() {
        let gpu = crate::test_gpu::shared_test_gpu();

        let mut context = LevelContext::with_starter_root();
        let root_id = context.root_component_id();
        let child_id =
            context.create_component("Child", [simulation::GRID_WIDTH, simulation::GRID_HEIGHT]);

        {
            let child = context.component_mut(child_id).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::output().words);
            child.recompute_outside_shape();
        }

        {
            let root = context.component_mut(root_id).unwrap();
            root.child_instances = vec![
                ChildInstancePlan {
                    component_id: child_id,
                    origin: [0, 0],
                },
                ChildInstancePlan {
                    component_id: child_id,
                    origin: [4, 0],
                },
            ];
            root.sync_child_links();
        }

        let board = BoardTextures::new(&gpu.device, &gpu.queue);
        context
            .upload_component_to_board(root_id, &board, &gpu.device, &gpu.queue)
            .unwrap();
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 0, y: 0 }, 0),
            CellSnapshot::child_read()
        );
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 4, y: 0 }, 0),
            CellSnapshot::child_read()
        );
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 1, y: 0 }, 0),
            CellSnapshot::empty()
        );

        {
            let child = context.component_mut(child_id).unwrap();
            child.set_cell_words_at(1, 0, CellSnapshot::output().words);
            child.set_cell_words_at(0, 1, CellSnapshot::output().words);
            child.recompute_outside_shape();
        }

        context
            .upload_component_to_board(root_id, &board, &gpu.device, &gpu.queue)
            .unwrap();
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 1, y: 0 }, 0),
            CellSnapshot::child_read()
        );
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 5, y: 0 }, 0),
            CellSnapshot::child_read()
        );
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 0, y: 1 }, 0),
            CellSnapshot::child_read()
        );
        assert_eq!(
            board.read_cell(&gpu.device, &gpu.queue, GridCell { x: 4, y: 1 }, 0),
            CellSnapshot::child_read()
        );
    }

    #[test]
    fn compact_child_proxy_uses_real_child_io_cell_mix() {
        let gpu = crate::test_gpu::shared_test_gpu();

        let mut context = LevelContext::with_starter_root();
        let root_id = context.root_component_id();
        let child_id =
            context.create_component("Child", [simulation::GRID_WIDTH, simulation::GRID_HEIGHT]);

        {
            let child = context.component_mut(child_id).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::input().words);
            child.set_cell_words_at(1, 0, CellSnapshot::input().words);
            child.set_cell_words_at(2, 0, CellSnapshot::input().words);
            child.set_cell_words_at(0, 1, CellSnapshot::output().words);
            child.set_cell_words_at(1, 1, CellSnapshot::output().words);
            child.recompute_outside_shape();
            assert_eq!(child.outside_shape.as_array(), [2, 2]);
        }

        {
            let root = context.component_mut(root_id).unwrap();
            root.child_instances = vec![ChildInstancePlan {
                component_id: child_id,
                origin: [0, 0],
            }];
            root.sync_child_links();
        }

        let board = BoardTextures::new(&gpu.device, &gpu.queue);
        context
            .upload_component_to_board(root_id, &board, &gpu.device, &gpu.queue)
            .unwrap();

        assert_eq!(
            board
                .read_cell(&gpu.device, &gpu.queue, GridCell { x: 0, y: 0 }, 0)
                .kind(),
            simulation::CellKind::ChildReadWrite
        );
        assert_eq!(
            board
                .read_cell(&gpu.device, &gpu.queue, GridCell { x: 1, y: 0 }, 0)
                .kind(),
            simulation::CellKind::ChildReadWrite
        );
        assert_eq!(
            board
                .read_cell(&gpu.device, &gpu.queue, GridCell { x: 0, y: 1 }, 0)
                .kind(),
            simulation::CellKind::ChildWrite1
        );
        assert_eq!(
            board
                .read_cell(&gpu.device, &gpu.queue, GridCell { x: 1, y: 1 }, 0)
                .kind(),
            simulation::CellKind::ChildNoop
        );
    }

    #[test]
    fn child_connected_parent_wires_survive_refresh_and_restore() {
        let gpu = crate::test_gpu::shared_test_gpu();

        let mut context = LevelContext::with_starter_root();
        let root_id = context.root_component_id();
        let child_id =
            context.create_component("Child", [simulation::GRID_WIDTH, simulation::GRID_HEIGHT]);

        {
            let child = context.component_mut(child_id).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::input().words);
            child.set_cell_words_at(1, 0, CellSnapshot::output().words);
            child.recompute_outside_shape();
            assert_eq!(child.outside_shape.as_array(), [1, 1]);
        }

        {
            let root = context.component_mut(root_id).unwrap();
            root.set_cell_words_at(0, 0, CellSnapshot::source(0xff).words);
            root.child_instances = vec![ChildInstancePlan {
                component_id: child_id,
                origin: [1, 0],
            }];
            root.sync_child_links();
        }

        let board = BoardTextures::new(&gpu.device, &gpu.queue);
        let mut wires = context
            .upload_component_to_board(root_id, &board, &gpu.device, &gpu.queue)
            .unwrap();

        let child_proxy = GridCell { x: 1, y: 0 };
        wires.add_wire_edge(StoredWireEdge {
            source_id: crate::wire_render::WireEndpointId::from_grid_cell(
                GridCell { x: 0, y: 0 },
                0,
            ),
            destination_id: crate::wire_render::WireEndpointId::from_grid_cell(child_proxy, 0),
            points: vec![
                crate::wires::WirePoint { x: 0.73, y: 0.5 },
                crate::wires::WirePoint { x: 1.11, y: 0.5 },
            ],
            color: crate::wires::DEFAULT_WIRE_COLOR,
        });

        context
            .refresh_component_from_board(root_id, &board, &wires, &gpu.device, &gpu.queue)
            .unwrap();

        let restored_wires = context
            .upload_component_to_board(root_id, &board, &gpu.device, &gpu.queue)
            .unwrap();
        let restored: Vec<_> = restored_wires.wire_edges().cloned().collect();

        assert!(restored.iter().any(|wire| {
            wire.source_id.as_grid_cell() == GridCell { x: 0, y: 0 }
                && wire.destination_id.as_grid_cell() == child_proxy
        }));
    }
}
