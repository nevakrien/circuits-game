use egui_wgpu::wgpu;

use crate::{
    child_components::{
        ChildInstancePlan, ChildPortLayout, ChildResourcePlanNode, plan_child_runtime,
    },
    component_plan::{ComponentId, ComponentPlan},
    game_constants::GameConstants,
    level_context::{LevelContext, LevelContextError},
    simulation::{self, BoardTextures, CellKind, CellSnapshot, Simulation},
    wire_render::WireRenderInfo,
    wires::GridCell,
};

const CHILD_LINK_BUFFER_BASE: u32 = simulation::OUTPUT_BUFFER_LEN;

pub struct RuntimeComponent {
    pub source_component_id: ComponentId,
    pub board: BoardTextures,
    pub wires: WireRenderInfo,
    children: Vec<RuntimeChildInstance>,
    input_port_count: u32,
    output_port_count: u32,
}

struct RuntimeChildInstance {
    parent_input_start: u32,
    parent_output_start: u32,
    plan: ChildInstancePlan,
    component: Box<RuntimeComponent>,
}

pub struct CircuitRuntime {
    pub simulation: Simulation,
    pub root: RuntimeComponent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedBufferLayout {
    page_starts: Vec<u32>,
    total_len: u32,
}

impl CircuitRuntime {
    pub fn build_and_link(
        context: &LevelContext,
        root_component_id: ComponentId,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Self, LevelContextError> {
        context.validate()?;

        let simulation = Simulation::new(device);
        let constants = GameConstants::default();
        let root =
            build_runtime_component(context, root_component_id, device, queue, &constants, true)?;
        Ok(Self { simulation, root })
    }

    pub fn step(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        current_buffer: u32,
        next_buffer: u32,
    ) {
        self.step_component(&self.root, device, queue, current_buffer, next_buffer);
    }

    fn step_component(
        &self,
        component: &RuntimeComponent,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        current_buffer: u32,
        next_buffer: u32,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("runtime-component-step"),
        });
        self.simulation.step(
            device,
            &mut encoder,
            &component.board,
            current_buffer,
            next_buffer,
        );
        queue.submit(Some(encoder.finish()));

        for child in &component.children {
            if child.component.input_port_count > 0 {
                let parent_outputs = pollster::block_on(component.board.read_output_range(
                    device,
                    queue,
                    child.parent_output_start,
                    child.component.input_port_count,
                ));
                child
                    .component
                    .board
                    .write_input_range(queue, 0, &parent_outputs);
            }

            self.step_component(&child.component, device, queue, current_buffer, next_buffer);

            if child.component.output_port_count > 0 {
                let child_outputs = pollster::block_on(child.component.board.read_output_range(
                    device,
                    queue,
                    0,
                    child.component.output_port_count,
                ));
                component
                    .board
                    .write_input_range(queue, child.parent_input_start, &child_outputs);
            }
        }
    }
}

fn build_runtime_component(
    context: &LevelContext,
    component_id: ComponentId,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    constants: &GameConstants,
    is_root: bool,
) -> Result<RuntimeComponent, LevelContextError> {
    let plan = context
        .component(component_id)
        .ok_or(LevelContextError::MissingRoot(component_id))?;
    let input_ports = collect_port_cells(plan, CellKind::Input);
    let output_ports = collect_port_cells(plan, CellKind::Output);
    let child_runtime_plan = build_child_runtime_plan(context, &plan.child_instances, constants)?;
    let parent_output_layout = planned_buffer_layout(
        child_runtime_plan
            .root_children
            .iter()
            .map(|child| child.io.input),
    );
    let parent_input_layout = planned_buffer_layout(
        child_runtime_plan
            .root_children
            .iter()
            .map(|child| child.io.output),
    );

    let mut children = Vec::with_capacity(plan.child_instances.len());

    for (child, planned_child) in plan
        .child_instances
        .iter()
        .zip(child_runtime_plan.root_children.iter())
    {
        let runtime =
            build_runtime_component(context, child.component_id, device, queue, constants, false)?;
        let parent_input_start = CHILD_LINK_BUFFER_BASE
            + planned_buffer_offset(&parent_input_layout, planned_child.io.output);
        let parent_output_start = CHILD_LINK_BUFFER_BASE
            + planned_buffer_offset(&parent_output_layout, planned_child.io.input);
        children.push(RuntimeChildInstance {
            parent_input_start,
            parent_output_start,
            plan: child.clone(),
            component: Box::new(runtime),
        });
    }

    let board = BoardTextures::with_buffer_lengths(
        device,
        queue,
        (CHILD_LINK_BUFFER_BASE + parent_input_layout.total_len).max(simulation::INPUT_BUFFER_LEN),
        (CHILD_LINK_BUFFER_BASE + parent_output_layout.total_len)
            .max(simulation::OUTPUT_BUFFER_LEN),
    );
    let wires = context.upload_component_to_board(component_id, &board, device, queue)?;

    if !is_root {
        patch_component_ports(&board, queue, plan, &input_ports, &output_ports);
    }
    patch_child_proxy_cells(&board, queue, plan, &children, context);

    Ok(RuntimeComponent {
        source_component_id: component_id,
        board,
        wires,
        children,
        input_port_count: input_ports.len() as u32,
        output_port_count: output_ports.len() as u32,
    })
}

fn build_child_runtime_plan(
    context: &LevelContext,
    child_instances: &[ChildInstancePlan],
    constants: &GameConstants,
) -> Result<crate::child_components::ChildRuntimePlan, LevelContextError> {
    let mut root_children = Vec::with_capacity(child_instances.len());
    for child in child_instances {
        root_children.push(build_child_resource_plan_node(context, child.component_id)?);
    }
    Ok(plan_child_runtime(&root_children, constants))
}

fn build_child_resource_plan_node(
    context: &LevelContext,
    component_id: ComponentId,
) -> Result<ChildResourcePlanNode, LevelContextError> {
    let plan = context
        .component(component_id)
        .ok_or(LevelContextError::MissingDependency {
            component: component_id,
            dependency: component_id,
        })?;
    let mut children = Vec::with_capacity(plan.child_instances.len());
    for child in &plan.child_instances {
        children.push(build_child_resource_plan_node(context, child.component_id)?);
    }

    Ok(ChildResourcePlanNode {
        grid_size: [plan.internal_size[0] as u16, plan.internal_size[1] as u16],
        port_layout: ChildPortLayout {
            input_words: plan.input_count(),
            output_words: plan.output_count(),
        },
        children,
    })
}

fn planned_buffer_layout(
    ranges: impl Iterator<Item = crate::buffer_allocator::BufferAllocRange>,
) -> PlannedBufferLayout {
    let mut page_spans = Vec::<u32>::new();
    for range in ranges {
        let page = range.page as usize;
        if page_spans.len() <= page {
            page_spans.resize(page + 1, 0);
        }
        page_spans[page] = page_spans[page].max(range.offset_words + range.len_words);
    }

    let mut page_starts = Vec::with_capacity(page_spans.len());
    let mut total_len = 0;
    for span in page_spans {
        page_starts.push(total_len);
        total_len += span;
    }

    PlannedBufferLayout {
        page_starts,
        total_len,
    }
}

fn planned_buffer_offset(
    layout: &PlannedBufferLayout,
    range: crate::buffer_allocator::BufferAllocRange,
) -> u32 {
    layout.page_starts[range.page as usize] + range.offset_words
}

fn patch_component_ports(
    board: &BoardTextures,
    queue: &wgpu::Queue,
    plan: &ComponentPlan,
    input_ports: &[GridCell],
    output_ports: &[GridCell],
) {
    for (offset, cell) in input_ports.iter().copied().enumerate() {
        let Some(words) = plan.cell_words_at(cell.x, cell.y) else {
            continue;
        };
        board.write_cell(
            queue,
            cell,
            0,
            CellSnapshot { words }.with_buffer_offset(offset as u32),
        );
    }

    for (offset, cell) in output_ports.iter().copied().enumerate() {
        let Some(words) = plan.cell_words_at(cell.x, cell.y) else {
            continue;
        };
        board.write_cell(
            queue,
            cell,
            0,
            CellSnapshot { words }.with_output_offset(offset as u32),
        );
    }
}

fn patch_child_proxy_cells(
    board: &BoardTextures,
    queue: &wgpu::Queue,
    plan: &ComponentPlan,
    children: &[RuntimeChildInstance],
    context: &LevelContext,
) {
    for child in children {
        let Some(child_plan) = context.component(child.plan.component_id) else {
            continue;
        };
        let specs = compact_child_proxy_specs(child_plan);
        for (index, spec) in specs.into_iter().enumerate() {
            let x = index as u32 % child_plan.outside_shape.width;
            let y = index as u32 / child_plan.outside_shape.width;
            let grid_cell = GridCell {
                x: child.plan.origin[0] + x,
                y: child.plan.origin[1] + y,
            };
            let snapshot = patch_proxy_snapshot(
                spec,
                child.parent_input_start,
                child.parent_output_start,
                wire_sources_for_destination(plan, grid_cell),
            );
            board.write_cell(queue, grid_cell, 0, snapshot);
        }
    }
}

fn patch_proxy_snapshot(
    spec: CompactChildProxyCell,
    parent_input_start: u32,
    parent_output_start: u32,
    wire_sources: Vec<GridCell>,
) -> CellSnapshot {
    match spec {
        CompactChildProxyCell::Read { output_index } => {
            CellSnapshot::child_read().with_buffer_offset(parent_input_start + output_index)
        }
        CompactChildProxyCell::ReadWrite {
            input_index,
            output_index,
        } => {
            let mut snapshot = CellSnapshot::child_read_write()
                .with_buffer_offset(parent_input_start + output_index)
                .with_output_offset(parent_output_start + input_index);
            if let Some(source) = wire_sources.first().copied() {
                snapshot = snapshot.with_child_wire_input(source);
            }
            snapshot
        }
        CompactChildProxyCell::Write { first_input_index } => {
            let mut snapshot = CellSnapshot::child_write()
                .with_output_offset(parent_output_start + first_input_index);
            if let Some(source) = wire_sources.first().copied() {
                snapshot = snapshot.with_primary_input(source);
            }
            if let Some(source) = wire_sources.get(1).copied() {
                snapshot = snapshot.with_secondary_input(source);
            }
            snapshot
        }
        CompactChildProxyCell::Write1 { input_index } => {
            let mut snapshot =
                CellSnapshot::child_write_1().with_output_offset(parent_output_start + input_index);
            if let Some(source) = wire_sources.first().copied() {
                snapshot = snapshot.with_primary_input(source);
            }
            snapshot
        }
        CompactChildProxyCell::Noop => CellSnapshot::child_noop(),
    }
}

fn collect_port_cells(plan: &ComponentPlan, kind: CellKind) -> Vec<GridCell> {
    let mut cells = Vec::new();
    for y in 0..plan.board_size[1] {
        for x in 0..plan.board_size[0] {
            let Some(words) = plan.cell_words_at(x, y) else {
                continue;
            };
            if (CellSnapshot { words }).kind() == kind {
                cells.push(GridCell { x, y });
            }
        }
    }
    cells
}

fn wire_sources_for_destination(plan: &ComponentPlan, destination: GridCell) -> Vec<GridCell> {
    let mut sources = Vec::new();
    for wire in &plan.wires {
        if wire.destination_id.arena_z == 0 && wire.destination_id.as_grid_cell() == destination {
            sources.push(wire.source_id.as_grid_cell());
        }
    }
    sources
}

#[derive(Clone, Copy)]
enum CompactChildProxyCell {
    Read { output_index: u32 },
    ReadWrite { input_index: u32, output_index: u32 },
    Write { first_input_index: u32 },
    Write1 { input_index: u32 },
    Noop,
}

fn compact_child_proxy_specs(child_plan: &ComponentPlan) -> Vec<CompactChildProxyCell> {
    let footprint = child_plan.outside_shape.as_array();
    let mut specs =
        vec![CompactChildProxyCell::Noop; (footprint[0] * footprint[1]).max(1) as usize];
    let input_count = child_plan.input_count();
    let output_count = child_plan.output_count();
    let read_write_count = input_count.min(output_count) as usize;
    let remaining_outputs = (output_count - read_write_count as u32) as usize;
    let remaining_inputs = (input_count - read_write_count as u32) as usize;
    let mut next = 0usize;

    for index in 0..read_write_count {
        if next >= specs.len() {
            return specs;
        }
        specs[next] = CompactChildProxyCell::ReadWrite {
            input_index: index as u32,
            output_index: index as u32,
        };
        next += 1;
    }

    for index in 0..remaining_outputs {
        if next >= specs.len() {
            return specs;
        }
        specs[next] = CompactChildProxyCell::Read {
            output_index: read_write_count as u32 + index as u32,
        };
        next += 1;
    }

    let remaining_input_base = read_write_count as u32;
    for pair in 0..(remaining_inputs / 2) {
        if next >= specs.len() {
            return specs;
        }
        let first = remaining_input_base + (pair * 2) as u32;
        specs[next] = CompactChildProxyCell::Write {
            first_input_index: first,
        };
        next += 1;
    }

    if remaining_inputs % 2 == 1 && next < specs.len() {
        specs[next] = CompactChildProxyCell::Write1 {
            input_index: remaining_input_base + (remaining_inputs as u32 - 1),
        };
    }

    specs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire_render::{StoredWireEdge, WireEndpointId};

    fn constants_with_child_page_words(words_per_page: u32) -> GameConstants {
        let mut constants = GameConstants::default();
        constants.child_io_sizing.words_per_page = words_per_page;
        constants
    }

    #[test]
    fn child_passthrough_runtime_links_parent_and_child_ports() {
        // We keep the actual runtime test tiny on purpose. The planner-side tests cover massive
        // allocation counts cheaply; this test covers the other half of the contract by proving a
        // nontrivial child runtime can be built and stepped end-to-end on real GPU resources.
        let gpu = crate::test_gpu::shared_test_gpu();

        let mut context = LevelContext::with_starter_root();
        let root_id = context.root_component_id();
        let child_id =
            context.create_component("Child", [simulation::GRID_WIDTH, simulation::GRID_HEIGHT]);

        {
            let child = context.component_mut(child_id).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::input().words);
            child.set_cell_words_at(
                1,
                0,
                CellSnapshot::output()
                    .with_primary_input(GridCell { x: 0, y: 0 })
                    .words,
            );
            child.recompute_outside_shape();
        }

        {
            let root = context.component_mut(root_id).unwrap();
            root.cells.fill(CellSnapshot::empty().words);
            root.wires.clear();
            root.child_instances = vec![ChildInstancePlan {
                component_id: child_id,
                origin: [1, 0],
            }];
            root.set_cell_words_at(0, 0, CellSnapshot::source(0xff).words);
            root.set_cell_words_at(
                2,
                0,
                CellSnapshot::output()
                    .with_primary_input(GridCell { x: 1, y: 0 })
                    .words,
            );
            root.wires.push(StoredWireEdge {
                source_id: WireEndpointId::from_grid_cell(GridCell { x: 0, y: 0 }, 0),
                destination_id: WireEndpointId::from_grid_cell(GridCell { x: 1, y: 0 }, 0),
                points: vec![],
                color: [1.0, 1.0, 1.0, 1.0],
            });
            root.sync_child_links();
        }

        let runtime =
            CircuitRuntime::build_and_link(&context, root_id, &gpu.device, &gpu.queue).unwrap();

        let mut current = 0;
        for _ in 0..4 {
            let next = (current + 1) % simulation::CHARGE_BUFFER_COUNT;
            runtime.step(&gpu.device, &gpu.queue, current, next);
            current = next;
        }

        let output = pollster::block_on(runtime.root.board.read_output_value(
            &gpu.device,
            &gpu.queue,
            2,
            0,
            0,
        ));
        assert_eq!(output, 0xff);
    }

    #[test]
    fn runtime_child_link_buffers_fit_guaranteed_storage_binding_limits() {
        // This is the small real-build counterpart to the planner stress tests. It proves that a
        // runtime with planner-derived child-link IO buffers still builds against guaranteed wgpu
        // limits. What is still missing is the texture-side equivalent for child runtime textures:
        // once a planned z page count exceeds one texture's depth, runtime should materialize
        // multiple textures instead of keeping the current fixed-size `BoardTextures` model.
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
                    origin: [3, 0],
                },
            ];
            root.sync_child_links();
        }

        let runtime =
            CircuitRuntime::build_and_link(&context, root_id, &gpu.device, &gpu.queue).unwrap();

        assert_eq!(
            runtime.root.board.input_len(),
            simulation::OUTPUT_BUFFER_LEN + 4
        );
        assert_eq!(
            runtime.root.board.output_len(),
            simulation::OUTPUT_BUFFER_LEN + 6
        );
        assert!(
            runtime.root.board.input_len() <= simulation::max_guaranteed_storage_buffer_words()
        );
        assert!(
            runtime.root.board.output_len() <= simulation::max_guaranteed_storage_buffer_words()
        );
    }

    #[test]
    fn runtime_child_link_offsets_follow_planner_allocations() {
        let gpu = crate::test_gpu::shared_test_gpu();
        let constants = constants_with_child_page_words(4);

        let mut context = LevelContext::with_starter_root();
        let root_id = context.root_component_id();
        let read_only_child = context.create_component(
            "ReadOnlyChild",
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        );
        let write_only_child = context.create_component(
            "WriteOnlyChild",
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        );
        let read_write_child = context.create_component(
            "ReadWriteChild",
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        );

        {
            let child = context.component_mut(read_only_child).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::output().words);
            child.recompute_outside_shape();
        }

        {
            let child = context.component_mut(write_only_child).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::input().words);
            child.recompute_outside_shape();
        }

        {
            let child = context.component_mut(read_write_child).unwrap();
            child.set_cell_words_at(0, 0, CellSnapshot::input().words);
            child.set_cell_words_at(1, 0, CellSnapshot::output().words);
            child.recompute_outside_shape();
        }

        let child_instances = vec![
            ChildInstancePlan {
                component_id: read_only_child,
                origin: [0, 0],
            },
            ChildInstancePlan {
                component_id: write_only_child,
                origin: [2, 0],
            },
            ChildInstancePlan {
                component_id: read_write_child,
                origin: [4, 0],
            },
        ];

        {
            let root = context.component_mut(root_id).unwrap();
            root.cells.fill(CellSnapshot::empty().words);
            root.wires.clear();
            root.child_instances = child_instances.clone();
            root.sync_child_links();
        }

        let planned = build_child_runtime_plan(&context, &child_instances, &constants).unwrap();
        let parent_output_layout =
            planned_buffer_layout(planned.root_children.iter().map(|child| child.io.input));
        let parent_input_layout =
            planned_buffer_layout(planned.root_children.iter().map(|child| child.io.output));

        let runtime =
            build_runtime_component(&context, root_id, &gpu.device, &gpu.queue, &constants, true)
                .unwrap();

        assert_eq!(runtime.children.len(), planned.root_children.len());
        assert_eq!(
            runtime.board.input_len(),
            CHILD_LINK_BUFFER_BASE + parent_input_layout.total_len
        );
        assert_eq!(
            runtime.board.output_len(),
            CHILD_LINK_BUFFER_BASE + parent_output_layout.total_len
        );

        for (runtime_child, planned_child) in
            runtime.children.iter().zip(planned.root_children.iter())
        {
            assert_eq!(
                runtime_child.parent_input_start,
                CHILD_LINK_BUFFER_BASE
                    + planned_buffer_offset(&parent_input_layout, planned_child.io.output)
            );
            assert_eq!(
                runtime_child.parent_output_start,
                CHILD_LINK_BUFFER_BASE
                    + planned_buffer_offset(&parent_output_layout, planned_child.io.input)
            );
        }

        let read_only_proxy =
            runtime
                .board
                .read_cell(&gpu.device, &gpu.queue, GridCell { x: 0, y: 0 }, 0);
        assert_eq!(read_only_proxy.kind(), CellKind::ChildRead);
        assert_eq!(
            read_only_proxy.buffer_offset(),
            runtime.children[0].parent_input_start
        );

        let write_only_proxy =
            runtime
                .board
                .read_cell(&gpu.device, &gpu.queue, GridCell { x: 2, y: 0 }, 0);
        assert_eq!(write_only_proxy.kind(), CellKind::ChildWrite1);
        assert_eq!(
            write_only_proxy.output_offset(),
            runtime.children[1].parent_output_start
        );
    }
}
