use egui_wgpu::wgpu;

use crate::{
    component_plan::ComponentId,
    level_context::{LevelContext, LevelContextError},
    simulation::{BoardTextures, Simulation},
    wire_render::WireRenderInfo,
};

pub struct RuntimeComponent {
    pub source_component_id: ComponentId,
    pub board: BoardTextures,
    pub wires: WireRenderInfo,
}

pub struct CircuitRuntime {
    pub simulation: Simulation,
    pub root: RuntimeComponent,
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
        let root_board = BoardTextures::new(device, queue);
        let root_wires =
            context.upload_component_to_board(root_component_id, &root_board, device, queue)?;

        // TODO: Build child runtimes from child_mentions once nested execution semantics are ready.
        // TODO: Link parent/child IO mapping and patch runtime input/output indices.
        Ok(Self {
            simulation,
            root: RuntimeComponent {
                source_component_id: root_component_id,
                board: root_board,
                wires: root_wires,
            },
        })
    }
}
