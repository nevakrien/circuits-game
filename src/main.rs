use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use circuits_game::{
    editor::{ComponentDefId, EditableComponentDef, EditorDocument},
    editor_interaction::{
        EditInteractionState, EditSceneAction, apply_edit_scene_drag_preview, child_at_pointer,
        interact_edit_scene,
    },
    gate_plans::{
        ChildId, ChildInputConnection, ChildPlacement, Component, ComponentLayout, ComponentPlan,
        ComponentPlans, ComponentPort, Gate, GateId, PlanId, PortId, PortLocation, SignalRef,
        WireEndpoint, compile_component_tree,
    },
    kernel::{GateKernel, UploadedGpuPlan},
    scene_render::SceneRenderer,
    setup,
    ui_config::{
        DEFAULT_TICK_RATE, FAST_RATE_FACTOR, MAX_FRAME_STEP_BUDGET, PAUSED_VISUAL_RATE_FACTOR,
        STEP_RATE_FACTOR,
    },
    viewer_frame::{ViewerRenderMode, render_viewer_frame},
    visual_ui::{
        FocusedScene, ViewportState, build_focused_scene, child_ids_of, interact_focused_scene,
        parent_stack_to,
    },
};
use egui_wgpu::wgpu;
use egui_winit::winit;
use wgpu::util::DeviceExt;
use winit::event::{Event, WindowEvent};

const INPUT_A: PortId = PortId(10);
const INPUT_B: PortId = PortId(11);
const OUTPUT_Y: PortId = PortId(20);
const OUTPUT_Z: PortId = PortId(21);
const LABEL_A: &str = "A";
const LABEL_B: &str = "B";
const LABEL_SUM: &str = "sum";
const LABEL_CARRY: &str = "carry";
const STRESS_GATES_PER_COMPONENT: u32 = 8_192;
const STRESS_BRANCH_FACTOR: usize = 4;
const STRESS_DEPTH: u32 = 5;

fn runtime_bits_per_buffer(device: &wgpu::Device) -> u32 {
    let max_storage_bytes = device.limits().max_storage_buffer_binding_size;
    let max_storage_bits = max_storage_bytes.saturating_mul(8);
    (max_storage_bits & !31).max(32)
}

fn upload_viewer_scene(
    scene_renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    viewer: &ViewerState,
) {
    if viewer.is_editing() {
        scene_renderer.upload_edit_scene(device, queue, &viewer.scene);
    } else {
        scene_renderer.upload_runtime_scene(device, queue, &viewer.scene);
    }
}

fn rebuild_and_upload_viewer_scene(
    scene_renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    viewer: &mut ViewerState,
    runtime: Option<&DemoRuntime>,
    document: &EditorDocument,
    hover_world: Option<egui::Pos2>,
) -> Result<(), String> {
    viewer.rebuild_scene(runtime, document, hover_world)?;
    upload_viewer_scene(scene_renderer, device, queue, viewer);
    Ok(())
}

fn upload_drag_preview_if_needed(
    scene_renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    viewer: &mut ViewerState,
    pointer_world: Option<egui::Pos2>,
) {
    let interaction = viewer.edit_interaction;
    if apply_edit_scene_drag_preview(&mut viewer.scene, &interaction, pointer_world) {
        upload_viewer_scene(scene_renderer, device, queue, viewer);
    }
}

fn draw_edit_selection_overlay(ui: &egui::Ui, viewer: &ViewerState, viewport_rect: egui::Rect) {
    let Some(selection) = viewer.selected_edit_target else {
        return;
    };
    let stroke = egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 220, 120));
    match selection {
        EditSelection::Child(child_id) => {
            if let Some(child) = viewer
                .scene
                .children
                .iter()
                .find(|child| child.id == child_id)
            {
                ui.painter().rect_stroke(
                    world_rect_to_screen(viewport_rect, &viewer.viewport, child.rect.expand(6.0)),
                    8.0,
                    stroke,
                    egui::StrokeKind::Outside,
                );
            }
        }
        EditSelection::Gate(gate_id) => {
            if let Some(gate) = viewer.scene.gates.iter().find(|gate| gate.id == gate_id) {
                ui.painter().rect_stroke(
                    world_rect_to_screen(viewport_rect, &viewer.viewport, gate.rect.expand(4.0)),
                    6.0,
                    stroke,
                    egui::StrokeKind::Outside,
                );
            }
        }
        EditSelection::Wire { from, to } => {
            if let Some(wire) = viewer
                .scene
                .wires
                .iter()
                .find(|wire| wire.from == Some(from) && wire.to == Some(to))
            {
                let points = wire
                    .points
                    .iter()
                    .map(|point| world_to_screen(viewport_rect, &viewer.viewport, *point))
                    .collect::<Vec<_>>();
                if points.len() >= 2 {
                    ui.painter().add(egui::Shape::line(points, stroke));
                }
            }
        }
    }
}

fn world_to_screen(
    viewport_rect: egui::Rect,
    viewport: &ViewportState,
    point: egui::Pos2,
) -> egui::Pos2 {
    viewport_rect.min + viewport.pan + point.to_vec2() * viewport.zoom
}

fn world_rect_to_screen(
    viewport_rect: egui::Rect,
    viewport: &ViewportState,
    rect: egui::Rect,
) -> egui::Rect {
    egui::Rect::from_min_max(
        world_to_screen(viewport_rect, viewport, rect.min),
        world_to_screen(viewport_rect, viewport, rect.max),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoSceneKind {
    Starter,
    Stress,
}

impl DemoSceneKind {
    const ALL: [Self; 2] = [Self::Starter, Self::Stress];

    fn label(self) -> &'static str {
        match self {
            Self::Starter => "Starter",
            Self::Stress => "Stress",
        }
    }
}

#[derive(Debug, Clone)]
struct DemoSceneSpec {
    kind: DemoSceneKind,
    label: &'static str,
    document: EditorDocument,
}
fn main() {
    pollster::block_on(run());
}

#[allow(deprecated)]
async fn run() {
    let setup::WindowState {
        event_loop,
        window,
        surface,
        mut config,
    } = setup::prepare_window();
    let gpu = setup::gpu();
    let device = &gpu.device;
    let queue = &gpu.queue;

    let initial_scene = build_demo_circuit(DemoSceneKind::Starter);
    let mut scene_kind = initial_scene.kind;
    let mut scene_label = initial_scene.label;
    let mut document = initial_scene.document;
    let mut runtime: Option<DemoRuntime> = None;
    let mut runtime_error: Option<String> = None;
    let mut viewer = ViewerState::new(&document).expect("viewer should build initial scene");
    let animation_started_at = Instant::now();
    let mut scene_renderer = SceneRenderer::new(device, config.format);
    upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
    let zero_charge_buffer = GateKernel::create_io_buffer(device, 1, "viewer-zero-charge-buffer");

    let egui_ctx = egui::Context::default();
    let mut egui_state = egui_winit::State::new(
        egui_ctx.clone(),
        egui::ViewportId::ROOT,
        &window,
        Some(window.scale_factor() as f32),
        window.theme(),
        None,
    );
    let mut egui_renderer =
        egui_wgpu::Renderer::new(device, config.format, egui_wgpu::RendererOptions::default());
    window.request_redraw();

    let _ = event_loop.run(|event, target| match event {
        Event::Resumed | Event::AboutToWait => {
            window.request_redraw();
        }
        Event::WindowEvent { event, .. } => {
            let response = egui_state.on_window_event(&window, &event);
            match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => {
                    config.width = size.width.max(1);
                    config.height = size.height.max(1);
                    surface.configure(device, &config);
                }
                WindowEvent::RedrawRequested => {
                    let raw_input = egui_state.take_egui_input(&window);
                    let hotkeys = viewer.apply_hotkeys(&raw_input);
                    let mut requested_scene = None;
                    let mut scene_rect = None;
                    let mut hover_world = None;

                    if hotkeys.switch_to_edit && viewer.is_running_mode() {
                        runtime = None;
                        runtime_error = None;
                        viewer.enter_edit(&document).expect("edit scene should rebuild");
                        upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                    }
                    if hotkeys.switch_to_run && viewer.is_editing() {
                        match DemoRuntime::new(device, queue, scene_label, &document) {
                            Ok(compiled) => {
                                runtime_error = None;
                                viewer.enter_run(&compiled).expect("run scene should rebuild");
                                upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                runtime = Some(compiled);
                            }
                            Err(error) => runtime_error = Some(error),
                        }
                    }
                    if viewer.is_editing() && hotkeys.undo {
                        if document.undo().expect("undo should succeed") {
                            viewer
                                .rebuild_scene(runtime.as_ref(), &document, None)
                                .expect("scene should rebuild after undo");
                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                        }
                    }
                    if viewer.is_editing() && hotkeys.redo {
                        if document.redo().expect("redo should succeed") {
                            viewer
                                .rebuild_scene(runtime.as_ref(), &document, None)
                                .expect("scene should rebuild after redo");
                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                        }
                    }

                    let now = Instant::now();
                    let scheduled_steps = if viewer.can_step_runtime() {
                        viewer.scheduled_steps(now)
                    } else {
                        0
                    };
                    let total_steps = scheduled_steps.saturating_add(u32::from(hotkeys.step_once));
                    if let Some(runtime) = runtime.as_mut() {
                        for _ in 0..total_steps {
                            runtime.step(device, queue);
                        }
                    }

                    let full_output = egui_ctx.run(raw_input, |ctx| {
                        viewer.apply_continuous_input(ctx);
                        egui::TopBottomPanel::top("top-bar").show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                ui.heading("Circuit Viewer");
                                ui.separator();
                                match runtime.as_ref() {
                                    Some(runtime) if viewer.is_running_mode() => ui.monospace(format!(
                                        "{} | run | read buffer {} | {} logical buffers | node {} | {} child components | {} wires",
                                        runtime.scene_label,
                                        runtime.current_read,
                                        runtime.buffer_count,
                                        viewer.run_focus_node.0,
                                        viewer.child_ids.len(),
                                        viewer.scene.wires.len()
                                    )),
                                    _ => ui.monospace(format!(
                                        "{} | edit | component def {} | {} wires",
                                        scene_label,
                                        viewer.active_edit_component().0,
                                        viewer.scene.wires.len()
                                    )),
                                };
                                if let Some(error) = runtime_error.as_ref() {
                                    ui.separator();
                                    ui.colored_label(egui::Color32::from_rgb(255, 140, 140), error);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Scene:");
                                for kind in DemoSceneKind::ALL {
                                    if ui
                                        .selectable_label(scene_kind == kind, kind.label())
                                        .clicked()
                                    {
                                        requested_scene = Some(kind);
                                    }
                                }
                                ui.separator();
                                if let Some(runtime) = runtime.as_ref() {
                                    ui.monospace(format!(
                                        "{} comps | {} gates | depth {}",
                                        runtime.component_count,
                                        runtime.gate_count,
                                        runtime.nesting_depth
                                    ));
                                } else {
                                    ui.monospace(format!(
                                        "{} plans | {} shared defs",
                                        document.plans.len(),
                                        document.components.len()
                                    ));
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Mode:");
                                if ui
                                    .selectable_label(viewer.is_editing(), "Edit")
                                    .clicked()
                                    && viewer.is_running_mode()
                                {
                                    runtime = None;
                                    runtime_error = None;
                                    viewer.enter_edit(&document).expect("edit scene should rebuild");
                                    upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                }
                                if ui
                                    .selectable_label(viewer.is_running_mode(), "Run")
                                    .clicked()
                                    && viewer.is_editing()
                                {
                                    match DemoRuntime::new(device, queue, scene_label, &document) {
                                        Ok(compiled) => {
                                            runtime_error = None;
                                            viewer.enter_run(&compiled).expect("run scene should rebuild");
                                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                            runtime = Some(compiled);
                                        }
                                        Err(error) => runtime_error = Some(error),
                                    }
                                }
                            });
                            ui.add_enabled_ui(viewer.is_running_mode(), |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Simulation:");
                                    if ui
                                        .selectable_label(
                                            matches!(viewer.simulation_mode, SimulationMode::Running),
                                            "Run",
                                        )
                                        .clicked()
                                    {
                                        viewer.set_simulation_mode(SimulationMode::Running);
                                    }
                                    if ui
                                        .selectable_label(
                                            matches!(viewer.simulation_mode, SimulationMode::Stepping),
                                            "Step",
                                        )
                                        .clicked()
                                    {
                                        viewer.set_simulation_mode(SimulationMode::Stepping);
                                    }
                                    if ui
                                        .selectable_label(
                                            matches!(viewer.simulation_mode, SimulationMode::Paused),
                                            "Pause",
                                        )
                                        .clicked()
                                    {
                                        viewer.set_simulation_mode(SimulationMode::Paused);
                                    }
                                    if ui.button("Step Once").clicked() {
                                        viewer.request_single_step();
                                    }
                                    if ui
                                        .selectable_label(
                                            matches!(viewer.simulation_mode, SimulationMode::FastForward),
                                            "Fast",
                                        )
                                        .clicked()
                                    {
                                        viewer.set_simulation_mode(SimulationMode::FastForward);
                                    }
                                });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Tick speed:");
                                ui.add(
                                    egui::Slider::new(&mut viewer.tick_rate, 1..=25)
                                        .clamping(egui::SliderClamping::Always),
                                );
                                ui.monospace(format!("run {:.1}x", 1.0));
                                ui.separator();
                                ui.monospace(format!("step {:.2}x", STEP_RATE_FACTOR));
                                ui.separator();
                                ui.monospace(format!("fast {:.2}x", FAST_RATE_FACTOR));
                                ui.separator();
                                ui.monospace(format!(
                                    "effective {:.1} ticks/s",
                                    viewer.effective_tick_rate_hz(viewer.simulation_mode)
                                ));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Focus:");
                                if viewer.is_editing() {
                                    let breadcrumbs = viewer.edit_breadcrumbs().to_vec();
                                    for (i, component) in breadcrumbs.iter().copied().enumerate() {
                                        if i > 0 {
                                            ui.label("> ");
                                        }
                                        if ui.button(format!("def {}", component.0)).clicked() {
                                            viewer.pop_edit_focus_to(i + 1);
                                            viewer.reset_camera();
                                            viewer.rebuild_scene(runtime.as_ref(), &document, None)
                                                .expect("focused edit scene should rebuild");
                                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                        }
                                    }
                                } else if let Some(runtime) = runtime.as_ref() {
                                    let mut path = parent_stack_to(&runtime.root, viewer.run_focus_node)
                                        .unwrap_or_default();
                                    path.push(viewer.run_focus_node);
                                    for (i, node) in path.iter().copied().enumerate() {
                                        if i > 0 {
                                            ui.label("> ");
                                        }
                                        if ui.button(format!("{}", node.0)).clicked() {
                                            viewer.focus_runtime_child(node);
                                            viewer.reset_camera();
                                            viewer.rebuild_scene(Some(runtime), &document, None)
                                                .expect("focused runtime scene should rebuild");
                                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                        }
                                    }
                                }
                            });
                            if viewer.is_editing() {
                                ui.horizontal(|ui| {
                                    if ui.button("Undo").clicked() {
                                        if document.undo().expect("undo should succeed") {
                                            viewer.rebuild_scene(runtime.as_ref(), &document, None)
                                                .expect("scene should rebuild after undo");
                                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                        }
                                    }
                                    if ui.button("Redo").clicked() {
                                        if document.redo().expect("redo should succeed") {
                                            viewer.rebuild_scene(runtime.as_ref(), &document, None)
                                                .expect("scene should rebuild after redo");
                                            upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                        }
                                    }
                                    ui.monospace(format!(
                                        "{} applied | {} redo",
                                        document.history().applied_len(),
                                        document.history().redo_len()
                                    ));
                                });
                            }
                            ui.label("Drag child blocks to move them, middle-drag or WASD/arrow keys to pan, ctrl/cmd+wheel or trackpad pinch to zoom, double-click child blocks to drill into them.");
                            ui.label("Hotkeys: E edit mode, R run mode, ctrl/cmd+z undo, ctrl/cmd+y redo, T step, F fast, P pause, Space single-step.");
                        });

                        if viewer.is_editing() {
                            egui::SidePanel::right("edit-panel")
                                .resizable(false)
                                .default_width(220.0)
                                .show(ctx, |ui| {
                                    let active = viewer.active_edit_component();
                                    if let Some(component) = document.component(active) {
                                        let plan = document.plan(component.plan);
                                        ui.heading(format!("Def {}", active.0));
                                        ui.monospace(format!(
                                            "plan {:?} | {} children",
                                            component.plan,
                                            component.children.len()
                                        ));
                                        ui.separator();
                                        ui.label("Children:");
                                        for (index, child_component) in component.children.iter().copied().enumerate() {
                                            let child_id = ChildId(index as u32);
                                            let selected = viewer.selected_edit_child() == Some(child_id);
                                            if ui
                                                .selectable_label(
                                                    selected,
                                                    format!("Child {} -> def {}", index, child_component.0),
                                                )
                                                .clicked()
                                            {
                                                viewer.select_edit_child(Some(child_id));
                                            }
                                        }
                                        if let Some(child_id) = viewer.selected_edit_child() {
                                            ui.separator();
                                            ui.label(format!("Selected child {}", child_id.0));
                                            if let Some(child_component) =
                                                document.child_component(active, child_id)
                                            {
                                                ui.monospace(format!("def {}", child_component.0));
                                                if ui.button("Open").clicked() {
                                                    viewer.push_edit_focus(child_component);
                                                    viewer.reset_camera();
                                                    viewer.rebuild_scene(
                                                        runtime.as_ref(),
                                                        &document,
                                                        None,
                                                    )
                                                        .expect("child edit scene should rebuild");
                                                    upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                                                }
                                            }
                                            if let Some(placement) = component
                                                .layout
                                                .child_placements
                                                .get(child_id.0 as usize)
                                            {
                                                ui.monospace(format!(
                                                    "grid [{}, {}]",
                                                    placement.min[0], placement.min[1]
                                                ));
                                            }
                                        }
                                        if let Some(plan) = plan {
                                            ui.separator();
                                            ui.label("Gates:");
                                            for (gate_id, gate) in plan.ordered_gates() {
                                                if ui
                                                    .selectable_label(
                                                        viewer.selected_edit_gate() == Some(gate_id),
                                                        format!("Gate {}: {}", gate_id.0, gate.label()),
                                                    )
                                                    .clicked()
                                                {
                                                    viewer.select_edit_gate(Some(gate_id));
                                                }
                                            }
                                        }

                                        match viewer.selected_edit_target {
                                            Some(EditSelection::Gate(gate_id)) => {
                                                if let Some(plan) = plan {
                                                    if let Some(gate) = plan.gate(gate_id) {
                                                        ui.separator();
                                                        ui.label(format!("Selected gate {}", gate_id.0));
                                                        ui.monospace(gate.label());
                                                        let width = plan.grid_size[0].max(1);
                                                        ui.monospace(format!(
                                                            "grid [{}, {}]",
                                                            gate_id.0 % width,
                                                            gate_id.0 / width
                                                        ));
                                                    }
                                                }
                                            }
                                            Some(EditSelection::Wire { from, to }) => {
                                                ui.separator();
                                                ui.label("Selected wire");
                                                ui.monospace(format!("from {:?}", from));
                                                ui.monospace(format!("to   {:?}", to));
                                            }
                                            _ => {}
                                        }
                                    }
                                });
                        }

                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
                            .show(ctx, |ui| {
                                let available = ui.available_size_before_wrap();
                                let size_changed = viewer.edit_available_size != available;
                                viewer.edit_available_size = available;
                                viewer.fit_camera_if_needed(available);
                                let viewport_output =
                                    interact_focused_scene(ui, &viewer.scene, &mut viewer.viewport);
                                scene_rect = Some(viewport_output.rect);
                                hover_world = if viewer.is_editing() && viewer.edit_interaction.is_dragging() {
                                    None
                                } else {
                                    viewport_output.hover_world
                                };
                                let hover_changed = viewer.edit_hover_world != hover_world;
                                viewer.edit_hover_world = hover_world;
                                let mut scene_rebuilt = false;
                                let action = if viewer.is_editing() {
                                    interact_edit_scene(
                                        &viewer.scene,
                                        &viewer.viewport,
                                        &viewport_output,
                                        &mut viewer.edit_interaction,
                                    )
                                } else if viewport_output.primary_double_clicked {
                                    child_at_pointer(&viewer.scene, viewport_output.hover_world)
                                        .map(|child| EditSceneAction::FocusChild(child.node))
                                } else {
                                    None
                                };
                                if let Some(action) = action {
                                    match action {
                                        EditSceneAction::ClearSelection => {
                                            if viewer.is_editing() {
                                                viewer.clear_edit_selection();
                                            }
                                        }
                                        EditSceneAction::SelectChild(child) => {
                                            if viewer.is_editing() {
                                                viewer.select_edit_child(Some(child));
                                            }
                                        }
                                        EditSceneAction::SelectGate(gate) => {
                                            if viewer.is_editing() {
                                                viewer.select_edit_gate(Some(gate));
                                            }
                                        }
                                        EditSceneAction::SelectWire { from, to } => {
                                            if viewer.is_editing() {
                                                viewer.select_edit_wire(Some((from, to)));
                                            }
                                        }
                                        EditSceneAction::MoveChild { child, delta_cells } => {
                                            if viewer.is_editing()
                                                && document
                                                    .move_child_by(viewer.active_edit_component(), child, delta_cells)
                                                    .expect("drag move should succeed")
                                            {
                                                viewer.select_edit_child(Some(child));
                                                rebuild_and_upload_viewer_scene(
                                                    &mut scene_renderer,
                                                    device,
                                                    queue,
                                                    &mut viewer,
                                                    runtime.as_ref(),
                                                    &document,
                                                    hover_world,
                                                )
                                                .expect("scene should rebuild after drag move");
                                                scene_rebuilt = true;
                                            }
                                        }
                                        EditSceneAction::MoveGate { gate, delta_cells } => {
                                            if viewer.is_editing() {
                                                match document.move_gate_by(
                                                    viewer.active_edit_component(),
                                                    gate,
                                                    delta_cells,
                                                ) {
                                                    Ok(true) => {
                                                        if rebuild_and_upload_viewer_scene(
                                                            &mut scene_renderer,
                                                            device,
                                                            queue,
                                                            &mut viewer,
                                                        runtime.as_ref(),
                                                        &document,
                                                        hover_world,
                                                    )
                                                    .is_ok()
                                                    {
                                                        viewer.select_edit_gate(Some(gate));
                                                        scene_rebuilt = true;
                                                    } else {
                                                        viewer.edit_interaction.clear();
                                                    }
                                                    }
                                                    Ok(false) => {}
                                                    Err(_) => viewer.edit_interaction.clear(),
                                                }
                                            }
                                        }
                                        EditSceneAction::MoveWireBend {
                                            from,
                                            to,
                                            bend_index,
                                            point,
                                        } => {
                                            if viewer.is_editing() {
                                                match document.move_wire_bend_to(
                                                    viewer.active_edit_component(),
                                                    from,
                                                    to,
                                                    bend_index,
                                                    point,
                                                ) {
                                                    Ok(true) => {
                                                        if rebuild_and_upload_viewer_scene(
                                                            &mut scene_renderer,
                                                            device,
                                                            queue,
                                                            &mut viewer,
                                                        runtime.as_ref(),
                                                        &document,
                                                        hover_world,
                                                    )
                                                    .is_ok()
                                                    {
                                                        viewer.select_edit_wire(Some((from, to)));
                                                        scene_rebuilt = true;
                                                    } else {
                                                        viewer.edit_interaction.clear();
                                                    }
                                                    }
                                                    Ok(false) => {}
                                                    Err(_) => viewer.edit_interaction.clear(),
                                                }
                                            }
                                        }
                                        EditSceneAction::InsertWireBend {
                                            from,
                                            to,
                                            bend_index,
                                            point,
                                        } => {
                                            if viewer.is_editing() {
                                                match document.insert_wire_bend(
                                                    viewer.active_edit_component(),
                                                    from,
                                                    to,
                                                    bend_index,
                                                    point,
                                                ) {
                                                    Ok(Some(_)) => {
                                                        if rebuild_and_upload_viewer_scene(
                                                            &mut scene_renderer,
                                                            device,
                                                            queue,
                                                            &mut viewer,
                                                        runtime.as_ref(),
                                                        &document,
                                                        hover_world,
                                                    )
                                                    .is_ok()
                                                    {
                                                        viewer.select_edit_wire(Some((from, to)));
                                                        scene_rebuilt = true;
                                                    } else {
                                                        viewer.edit_interaction.clear();
                                                    }
                                                    }
                                                    Ok(None) => viewer.edit_interaction.clear(),
                                                    Err(_) => viewer.edit_interaction.clear(),
                                                }
                                            }
                                        }
                                        EditSceneAction::RewireWireSource { from, to, new_from } => {
                                            if viewer.is_editing() {
                                                match document.rewire_source_endpoint(
                                                    viewer.active_edit_component(),
                                                    from,
                                                    to,
                                                    new_from,
                                                ) {
                                                    Ok(Some(_)) => {
                                                        if rebuild_and_upload_viewer_scene(
                                                            &mut scene_renderer,
                                                            device,
                                                            queue,
                                                            &mut viewer,
                                                            runtime.as_ref(),
                                                            &document,
                                                            hover_world,
                                                        )
                                                        .is_ok()
                                                        {
                                                            viewer.select_edit_wire(Some((new_from, to)));
                                                            scene_rebuilt = true;
                                                        } else {
                                                            viewer.edit_interaction.clear();
                                                        }
                                                    }
                                                    Ok(None) => {}
                                                    Err(_) => viewer.edit_interaction.clear(),
                                                }
                                            }
                                        }
                                        EditSceneAction::RewireWireTarget { from, to, new_to } => {
                                            if viewer.is_editing() {
                                                match document.rewire_target_endpoint(
                                                    viewer.active_edit_component(),
                                                    from,
                                                    to,
                                                    new_to,
                                                ) {
                                                    Ok(Some(_)) => {
                                                        if rebuild_and_upload_viewer_scene(
                                                            &mut scene_renderer,
                                                            device,
                                                            queue,
                                                            &mut viewer,
                                                            runtime.as_ref(),
                                                            &document,
                                                            hover_world,
                                                        )
                                                        .is_ok()
                                                        {
                                                            viewer.select_edit_wire(Some((from, new_to)));
                                                            scene_rebuilt = true;
                                                        } else {
                                                            viewer.edit_interaction.clear();
                                                        }
                                                    }
                                                    Ok(None) => {}
                                                    Err(_) => viewer.edit_interaction.clear(),
                                                }
                                            }
                                        }
                                        EditSceneAction::FocusChild(node) => {
                                            if viewer.is_editing() {
                                                viewer.push_edit_focus(ComponentDefId(node.0 as usize));
                                            } else {
                                                viewer.focus_runtime_child(node);
                                            }
                                            viewer.reset_camera();
                                            rebuild_and_upload_viewer_scene(
                                                &mut scene_renderer,
                                                device,
                                                queue,
                                                &mut viewer,
                                                runtime.as_ref(),
                                                &document,
                                                None,
                                            )
                                            .expect("child focus scene should rebuild");
                                            scene_rebuilt = true;
                                        }
                                    }
                                }
                                if viewer.is_editing() && !scene_rebuilt {
                                    if viewer.edit_interaction.is_dragging() {
                                        upload_drag_preview_if_needed(
                                            &mut scene_renderer,
                                            device,
                                            queue,
                                            &mut viewer,
                                            viewport_output.hover_world,
                                        );
                                    } else if viewport_output.viewport_changed || size_changed || hover_changed {
                                        if document
                                            .refresh_edit_scene_drill_path(
                                                viewer.active_edit_component(),
                                                &mut viewer.scene,
                                                &viewer.viewport,
                                                viewer.edit_available_size,
                                                hover_world,
                                            )
                                            .expect("edit drill path should refresh")
                                        {
                                            upload_viewer_scene(
                                                &mut scene_renderer,
                                                device,
                                                queue,
                                                &viewer,
                                            );
                                        }
                                    }
                                }
                                if viewer.is_editing() {
                                    draw_edit_selection_overlay(ui, &viewer, viewport_output.rect);
                                }
                            });
                    });
                    let egui::FullOutput {
                        platform_output,
                        textures_delta,
                        shapes,
                        pixels_per_point,
                        ..
                    } = full_output;
                    let paint_jobs = egui_ctx.tessellate(shapes, pixels_per_point);
                    match render_viewer_frame(
                        device,
                        queue,
                        &surface,
                        &config,
                        &mut egui_renderer,
                        &scene_renderer,
                        scene_rect,
                        hover_world,
                        pixels_per_point,
                        &viewer.viewport,
                        viewer.current_render_mode(),
                        runtime
                            .as_ref()
                            .map(|runtime| &runtime.charge_buffers[runtime.current_read])
                            .unwrap_or(&zero_charge_buffer),
                        runtime
                            .as_ref()
                            .map(|runtime| {
                                &runtime.charge_buffers
                                    [(runtime.current_read + 1) % runtime.charge_buffers.len()]
                            })
                            .unwrap_or(&zero_charge_buffer),
                        animation_started_at.elapsed().as_secs_f32(),
                        viewer.visual_pulse_rate_hz(),
                        &textures_delta,
                        &paint_jobs,
                    ) {
                        Ok(_) => {}
                        Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                            surface.configure(device, &config);
                            return;
                        }
                        Err(wgpu::SurfaceError::Timeout) => return,
                        Err(wgpu::SurfaceError::OutOfMemory) => {
                            target.exit();
                            return;
                        }
                        Err(wgpu::SurfaceError::Other) => return,
                    }

                    egui_state.handle_platform_output(&window, platform_output);

                    if let Some(kind) = requested_scene {
                        let scene = build_demo_circuit(kind);
                        scene_kind = scene.kind;
                        scene_label = scene.label;
                        document = scene.document;
                        runtime = None;
                        viewer = ViewerState::new(&document)
                            .expect("viewer should rebuild for requested scene");
                        upload_viewer_scene(&mut scene_renderer, device, queue, &viewer);
                    }

                    if !response.repaint {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }
        _ => {}
    });
}

struct DemoRuntime {
    scene_label: &'static str,
    component_count: u64,
    gate_count: u64,
    nesting_depth: u32,
    buffer_count: u32,
    kernel: GateKernel,
    uploaded: UploadedGpuPlan,
    charge_buffers: [wgpu::Buffer; 2],
    output_buffer: wgpu::Buffer,
    current_read: usize,
    root: Component,
    plans: ComponentPlans,
    gate_store: Arc<
        foldhash::HashMap<
            (circuits_game::gate_plans::NodeId, GateId),
            circuits_game::gate_plans::GateStoreLocation,
        >,
    >,
    words_per_buffer: u32,
    tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Edit,
    Run,
}

#[derive(Debug, Default, Clone, Copy)]
struct ViewerHotkeys {
    step_once: bool,
    switch_to_edit: bool,
    switch_to_run: bool,
    undo: bool,
    redo: bool,
}

impl DemoRuntime {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        document: &EditorDocument,
    ) -> Result<Self, String> {
        let (mut root, plans) = document.build_runtime_root_and_plans()?;
        let component_count = count_components(&root);
        let gate_count = count_gates(&root, &plans)?;
        let nesting_depth = count_depth(&root);
        let bits_per_buffer = runtime_bits_per_buffer(device);
        let compiled = compile_component_tree(&mut root, &plans, bits_per_buffer)
            .map_err(|error| format!("failed to compile demo circuit: {error:?}"))?;
        let buffer_count = compiled
            .gate_store
            .values()
            .map(|store| store.buffer.0)
            .max()
            .unwrap_or(0)
            + 1;
        let storage_words = buffer_count * compiled.gpu_plan.words_per_buffer;
        let initial_words = seed_demo_words(
            &compiled.gate_store,
            compiled.gpu_plan.words_per_buffer,
            storage_words,
        );

        let kernel = GateKernel::new(device);
        let uploaded = GateKernel::upload_plan(device, &compiled.gpu_plan);
        let read_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("viewer-read-buffer-0"),
            contents: bytemuck::cast_slice(&initial_words),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let write_buffer =
            GateKernel::create_io_buffer(device, storage_words, "viewer-read-buffer-1");
        let output_buffer = GateKernel::create_io_buffer(
            device,
            compiled.gpu_plan.output_words,
            "viewer-output-buffer",
        );

        queue.write_buffer(&write_buffer, 0, bytemuck::cast_slice(&initial_words));

        Ok(Self {
            kernel,
            uploaded,
            charge_buffers: [read_buffer, write_buffer],
            output_buffer,
            current_read: 0,
            scene_label: label,
            component_count,
            gate_count,
            nesting_depth,
            buffer_count,
            root,
            plans,
            gate_store: Arc::new(compiled.gate_store),
            words_per_buffer: compiled.gpu_plan.words_per_buffer,
            tick: 0,
        })
    }

    fn step(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let write_index = (self.current_read + 1) % self.charge_buffers.len();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewer-step"),
        });
        self.kernel.encode(
            device,
            &mut encoder,
            &self.uploaded,
            &self.charge_buffers[self.current_read],
            &self.charge_buffers[write_index],
            &self.output_buffer,
        );
        queue.submit(Some(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        self.current_read = write_index;
        self.tick += 1;
    }
}

struct ViewerState {
    mode: AppMode,
    edit_focus_stack: Vec<ComponentDefId>,
    selected_edit_target: Option<EditSelection>,
    edit_hover_world: Option<egui::Pos2>,
    edit_interaction: EditInteractionState,
    run_focus_node: circuits_game::gate_plans::NodeId,
    child_ids: Vec<circuits_game::gate_plans::NodeId>,
    scene: FocusedScene,
    viewport: ViewportState,
    edit_available_size: egui::Vec2,
    pending_camera_fit: bool,
    simulation_mode: SimulationMode,
    tick_rate: u32,
    last_frame_at: Instant,
    step_accumulator: Duration,
    pending_single_steps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimulationMode {
    Running,
    Stepping,
    FastForward,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditSelection {
    Child(ChildId),
    Gate(GateId),
    Wire {
        from: WireEndpoint,
        to: WireEndpoint,
    },
}

impl ViewerState {
    fn new(document: &EditorDocument) -> Result<Self, String> {
        let mut viewer = Self {
            mode: AppMode::Edit,
            edit_focus_stack: vec![document.root],
            selected_edit_target: None,
            edit_hover_world: None,
            edit_interaction: EditInteractionState::default(),
            run_focus_node: circuits_game::gate_plans::NodeId(0),
            child_ids: Vec::new(),
            scene: document.build_edit_scene(
                document.root,
                &ViewportState::default(),
                egui::Vec2::ZERO,
                None,
            )?,
            viewport: ViewportState::default(),
            edit_available_size: egui::Vec2::ZERO,
            pending_camera_fit: true,
            simulation_mode: SimulationMode::Paused,
            tick_rate: DEFAULT_TICK_RATE,
            last_frame_at: Instant::now(),
            step_accumulator: Duration::ZERO,
            pending_single_steps: 0,
        };
        viewer.reset_to_document_root(document)?;
        Ok(viewer)
    }

    fn set_simulation_mode(&mut self, mode: SimulationMode) {
        if self.simulation_mode != mode {
            self.simulation_mode = mode;
            self.step_accumulator = Duration::ZERO;
        }
    }

    fn request_single_step(&mut self) {
        self.pending_single_steps = self.pending_single_steps.saturating_add(1);
    }

    fn effective_tick_rate_hz(&self, mode: SimulationMode) -> f32 {
        let factor = match mode {
            SimulationMode::Running => 1.0,
            SimulationMode::Stepping => STEP_RATE_FACTOR,
            SimulationMode::FastForward => FAST_RATE_FACTOR,
            SimulationMode::Paused => 0.0,
        };
        self.tick_rate as f32 * factor
    }

    fn visual_pulse_rate_hz(&self) -> f32 {
        let rate = match self.simulation_mode {
            SimulationMode::Paused => self.tick_rate as f32 * PAUSED_VISUAL_RATE_FACTOR,
            mode => self.effective_tick_rate_hz(mode),
        };
        rate.max(0.1)
    }

    fn scheduled_steps(&mut self, now: Instant) -> u32 {
        let frame_dt = now.saturating_duration_since(self.last_frame_at);
        self.last_frame_at = now;
        let frame_dt = frame_dt.min(Duration::from_millis(250));
        self.step_accumulator += frame_dt;

        let interval = rate_to_interval(self.effective_tick_rate_hz(self.simulation_mode));
        let mut steps = 0;
        if !interval.is_zero() {
            while self.step_accumulator >= interval && steps < MAX_FRAME_STEP_BUDGET {
                self.step_accumulator -= interval;
                steps += 1;
            }
        } else {
            self.step_accumulator = Duration::ZERO;
        }

        let pending = self.pending_single_steps;
        self.pending_single_steps = 0;
        steps.saturating_add(pending)
    }

    fn reset_camera(&mut self) {
        self.pending_camera_fit = true;
    }

    fn fit_camera_if_needed(&mut self, available: egui::Vec2) {
        if !self.pending_camera_fit || available.x <= 0.0 || available.y <= 0.0 {
            return;
        }
        self.viewport = fit_viewport_to_scene(available, &self.scene);
        self.pending_camera_fit = false;
    }

    fn active_edit_component(&self) -> ComponentDefId {
        *self
            .edit_focus_stack
            .last()
            .expect("edit focus stack should never be empty")
    }

    fn rebuild_edit_scene(
        &mut self,
        document: &EditorDocument,
        hover_world: Option<egui::Pos2>,
    ) -> Result<(), String> {
        self.scene = document.build_edit_scene(
            self.active_edit_component(),
            &self.viewport,
            self.edit_available_size,
            hover_world,
        )?;
        Ok(())
    }

    fn rebuild_run_scene(&mut self, runtime: &DemoRuntime) -> Result<(), String> {
        self.scene = build_focused_scene(
            &runtime.root,
            &runtime.plans,
            self.run_focus_node,
            runtime.gate_store.clone(),
            runtime.words_per_buffer,
        )
        .map_err(|error| format!("failed to rebuild focused scene: {error:?}"))?;
        self.child_ids = child_ids_of(&runtime.root, self.run_focus_node);
        Ok(())
    }

    fn enter_edit(&mut self, document: &EditorDocument) -> Result<(), String> {
        self.mode = AppMode::Edit;
        self.set_simulation_mode(SimulationMode::Paused);
        self.reset_to_document_root(document)
    }

    fn enter_run(&mut self, runtime: &DemoRuntime) -> Result<(), String> {
        self.mode = AppMode::Run;
        self.reset_to_runtime_root(runtime)
    }

    fn reset_to_document_root(&mut self, document: &EditorDocument) -> Result<(), String> {
        self.edit_focus_stack.clear();
        self.edit_focus_stack.push(document.root);
        self.selected_edit_target = None;
        self.edit_hover_world = None;
        self.edit_interaction.clear();
        self.child_ids.clear();
        self.rebuild_edit_scene(document, None)?;
        self.reset_camera();
        Ok(())
    }

    fn reset_to_runtime_root(&mut self, runtime: &DemoRuntime) -> Result<(), String> {
        self.run_focus_node = runtime.root.id;
        self.selected_edit_target = None;
        self.edit_hover_world = None;
        self.edit_interaction.clear();
        self.rebuild_run_scene(runtime)?;
        self.reset_camera();
        Ok(())
    }

    fn current_render_mode(&self) -> ViewerRenderMode {
        match self.mode {
            AppMode::Edit => ViewerRenderMode::EditPreview,
            AppMode::Run => ViewerRenderMode::Run,
        }
    }

    fn edit_breadcrumbs(&self) -> &[ComponentDefId] {
        &self.edit_focus_stack
    }

    fn pop_edit_focus_to(&mut self, len: usize) {
        self.edit_focus_stack.truncate(len);
        self.selected_edit_target = None;
        self.edit_hover_world = None;
        self.edit_interaction.clear();
    }

    fn push_edit_focus(&mut self, component: ComponentDefId) {
        self.edit_focus_stack.push(component);
        self.selected_edit_target = None;
        self.edit_hover_world = None;
        self.edit_interaction.clear();
    }

    fn select_edit_child(&mut self, child: Option<ChildId>) {
        self.selected_edit_target = child.map(EditSelection::Child);
    }

    fn selected_edit_child(&self) -> Option<ChildId> {
        match self.selected_edit_target {
            Some(EditSelection::Child(child)) => Some(child),
            _ => None,
        }
    }

    fn select_edit_gate(&mut self, gate: Option<GateId>) {
        self.selected_edit_target = gate.map(EditSelection::Gate);
    }

    fn selected_edit_gate(&self) -> Option<GateId> {
        match self.selected_edit_target {
            Some(EditSelection::Gate(gate)) => Some(gate),
            _ => None,
        }
    }

    fn select_edit_wire(&mut self, wire: Option<(WireEndpoint, WireEndpoint)>) {
        self.selected_edit_target = wire.map(|(from, to)| EditSelection::Wire { from, to });
    }

    fn clear_edit_selection(&mut self) {
        self.selected_edit_target = None;
    }

    fn rebuild_scene(
        &mut self,
        runtime: Option<&DemoRuntime>,
        document: &EditorDocument,
        hover_world: Option<egui::Pos2>,
    ) -> Result<(), String> {
        match self.mode {
            AppMode::Edit => self.rebuild_edit_scene(document, hover_world),
            AppMode::Run => self.rebuild_run_scene(
                runtime.ok_or_else(|| "missing runtime in run mode".to_owned())?,
            ),
        }
    }

    fn can_step_runtime(&self) -> bool {
        matches!(self.mode, AppMode::Run)
    }

    fn is_editing(&self) -> bool {
        matches!(self.mode, AppMode::Edit)
    }

    fn is_running_mode(&self) -> bool {
        matches!(self.mode, AppMode::Run)
    }

    fn focus_runtime_child(&mut self, node: circuits_game::gate_plans::NodeId) {
        self.run_focus_node = node;
    }

    fn apply_continuous_input(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|input| input.stable_dt).min(0.1);
        if dt <= 0.0 {
            return;
        }
        let (left, right, up, down, fast_pan) = ctx.input(|input| {
            (
                input.key_down(egui::Key::ArrowLeft) || input.key_down(egui::Key::A),
                input.key_down(egui::Key::ArrowRight) || input.key_down(egui::Key::D),
                input.key_down(egui::Key::ArrowUp) || input.key_down(egui::Key::W),
                input.key_down(egui::Key::ArrowDown) || input.key_down(egui::Key::S),
                input.modifiers.shift,
            )
        });
        let speed = if fast_pan { 720.0 } else { 420.0 };
        let distance = speed * dt;
        if left {
            self.viewport.pan.x += distance;
        }
        if right {
            self.viewport.pan.x -= distance;
        }
        if up {
            self.viewport.pan.y += distance;
        }
        if down {
            self.viewport.pan.y -= distance;
        }
    }

    fn apply_hotkeys(&mut self, raw_input: &egui::RawInput) -> ViewerHotkeys {
        let mut hotkeys = ViewerHotkeys::default();
        for event in &raw_input.events {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            if !pressed {
                continue;
            }
            match key {
                egui::Key::E if !repeat => hotkeys.switch_to_edit = true,
                egui::Key::R if !repeat => hotkeys.switch_to_run = true,
                egui::Key::T if !repeat => self.set_simulation_mode(SimulationMode::Stepping),
                egui::Key::F if !repeat => self.set_simulation_mode(SimulationMode::FastForward),
                egui::Key::P if !repeat => self.set_simulation_mode(SimulationMode::Paused),
                egui::Key::Space if !repeat => hotkeys.step_once = true,
                egui::Key::Z if !repeat && modifiers.command && modifiers.shift => {
                    hotkeys.redo = true
                }
                egui::Key::Z if !repeat && modifiers.command => hotkeys.undo = true,
                egui::Key::Y if !repeat && modifiers.command => hotkeys.redo = true,
                _ => {}
            }
        }
        hotkeys
    }
}

fn count_components(root: &Component) -> u64 {
    1 + root.children.iter().map(count_components).sum::<u64>()
}

fn count_gates(root: &Component, plans: &ComponentPlans) -> Result<u64, String> {
    let plan = plans
        .get(root.plan)
        .ok_or_else(|| format!("missing plan {:?}", root.plan))?;
    Ok(plan.gates.len() as u64
        + root
            .children
            .iter()
            .map(|child| count_gates(child, plans))
            .sum::<Result<u64, _>>()?)
}

fn count_depth(root: &Component) -> u32 {
    1 + root.children.iter().map(count_depth).max().unwrap_or(0)
}

fn rate_to_interval(rate_hz: f32) -> Duration {
    if rate_hz <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(1.0 / rate_hz as f64)
    }
}

fn fit_viewport_to_scene(available: egui::Vec2, scene: &FocusedScene) -> ViewportState {
    const VIEWPORT_MARGIN: f32 = 0.92;

    let scene_size = scene.bounds.size();
    let zoom = (available.x / scene_size.x.max(1.0)).min(available.y / scene_size.y.max(1.0))
        * VIEWPORT_MARGIN;
    let zoom = zoom.max(0.01);
    let content_size = scene_size * zoom;
    let pan = (available - content_size) * 0.5 - scene.bounds.min.to_vec2() * zoom;
    ViewportState::new(zoom, pan)
}

fn seed_demo_words(
    gate_store: &foldhash::HashMap<
        (circuits_game::gate_plans::NodeId, GateId),
        circuits_game::gate_plans::GateStoreLocation,
    >,
    words_per_buffer: u32,
    storage_words: u32,
) -> Vec<u32> {
    let mut words = vec![0u32; storage_words as usize];

    set_gate_seed(gate_store, &mut words, words_per_buffer, GateId(0), true);
    words
}

fn set_gate_seed(
    gate_store: &foldhash::HashMap<
        (circuits_game::gate_plans::NodeId, GateId),
        circuits_game::gate_plans::GateStoreLocation,
    >,
    words: &mut [u32],
    words_per_buffer: u32,
    gate: GateId,
    value: bool,
) {
    let Some((&(node, _gate), store)) = gate_store
        .iter()
        .find(|((node, candidate), _)| node.0 == 0 && *candidate == gate)
    else {
        return;
    };
    let _ = node;
    let word_index = store.buffer.0 * words_per_buffer + (store.bit.0 / 32);
    if let Some(word) = words.get_mut(word_index as usize) {
        if value {
            *word |= 1u32 << store.bit_in_word;
        } else {
            *word &= !(1u32 << store.bit_in_word);
        }
    }
}

fn build_demo_circuit(kind: DemoSceneKind) -> DemoSceneSpec {
    match kind {
        DemoSceneKind::Starter => build_starter_demo_circuit(),
        DemoSceneKind::Stress => build_stress_demo_circuit(),
    }
}

fn build_starter_demo_circuit() -> DemoSceneSpec {
    let child_plan = PlanId(0);
    let root_plan = PlanId(1);
    let mut plans = foldhash::HashMap::default();
    plans.insert(
        child_plan,
        ComponentPlan::with_ports(
            vec![
                Gate::BitNop {
                    src: input_ref(INPUT_A),
                },
                Gate::BitNop {
                    src: input_ref(INPUT_B),
                },
                Gate::BitXOR {
                    a: this_ref(0),
                    b: this_ref(1),
                },
                Gate::BitAND {
                    a: this_ref(0),
                    b: this_ref(1),
                },
                Gate::BitNot { src: this_ref(0) },
            ],
            vec![
                port_named(INPUT_A, 0, 0, 1, LABEL_A),
                port_named(INPUT_B, 1, 0, 2, LABEL_B),
            ],
            vec![
                port_named(OUTPUT_Y, 2, u16::MAX, 1, LABEL_SUM),
                port_named(OUTPUT_Z, 4, u16::MAX, 2, LABEL_CARRY),
            ],
        )
        .with_grid_size([3, 2]),
    );
    plans.insert(
        root_plan,
        ComponentPlan::with_ports(
            vec![
                Gate::BitNot { src: this_ref(0) },
                Gate::BitNop { src: this_ref(0) },
                Gate::BitXOR {
                    a: this_ref(0),
                    b: this_ref(1),
                },
                Gate::BitNop {
                    src: child_output_ref(0, OUTPUT_Y),
                },
                Gate::BitNot {
                    src: child_output_ref(0, OUTPUT_Y),
                },
                Gate::BitNop {
                    src: child_output_ref(0, OUTPUT_Z),
                },
                Gate::BitOR {
                    a: this_ref(2),
                    b: this_ref(5),
                },
            ],
            vec![],
            vec![
                port_named(OUTPUT_Y, 3, u16::MAX, 1, LABEL_SUM),
                port_named(OUTPUT_Z, 6, u16::MAX, 3, LABEL_CARRY),
            ],
        )
        .with_grid_size([5, 5]),
    );
    let document = EditorDocument::new(
        plans,
        vec![
            EditableComponentDef {
                plan: child_plan,
                children: Vec::new(),
                child_input_connections: Vec::new(),
                dangling_wires: Vec::new(),
                layout: ComponentLayout::default(),
            },
            EditableComponentDef {
                plan: root_plan,
                children: vec![ComponentDefId(0)],
                child_input_connections: vec![
                    ChildInputConnection {
                        child: ChildId(0),
                        input: INPUT_A,
                        src: this_ref(0),
                    },
                    ChildInputConnection {
                        child: ChildId(0),
                        input: INPUT_B,
                        src: this_ref(1),
                    },
                ],
                dangling_wires: Vec::new(),
                layout: ComponentLayout::default()
                    .with_child_placements(vec![ChildPlacement::at([2, 2])]),
            },
        ],
        ComponentDefId(1),
    )
    .expect("starter demo document should build");

    DemoSceneSpec {
        kind: DemoSceneKind::Starter,
        label: "Starter demo",
        document,
    }
}

fn build_stress_demo_circuit() -> DemoSceneSpec {
    let leaf_plan = PlanId(0);
    let branch_plan = PlanId(1);
    let mut plans = foldhash::HashMap::default();
    plans.insert(
        leaf_plan,
        ComponentPlan::new(build_stress_gates(STRESS_GATES_PER_COMPONENT))
            .with_grid_size([128, 64]),
    );
    plans.insert(
        branch_plan,
        ComponentPlan::new(build_stress_gates(STRESS_GATES_PER_COMPONENT))
            .with_grid_size([256, 160]),
    );

    let mut components = vec![EditableComponentDef {
        plan: leaf_plan,
        children: Vec::new(),
        child_input_connections: Vec::new(),
        dangling_wires: Vec::new(),
        layout: ComponentLayout::default(),
    }];
    for _ in 0..STRESS_DEPTH {
        let previous = ComponentDefId(components.len() - 1);
        components.push(EditableComponentDef {
            plan: branch_plan,
            children: vec![previous; STRESS_BRANCH_FACTOR],
            child_input_connections: Vec::new(),
            dangling_wires: Vec::new(),
            layout: ComponentLayout::default().with_child_placements(vec![
                ChildPlacement::at([0, 0]),
                ChildPlacement::at([128, 0]),
                ChildPlacement::at([0, 80]),
                ChildPlacement::at([128, 80]),
            ]),
        });
    }
    let root = ComponentDefId(components.len() - 1);
    let document =
        EditorDocument::new(plans, components, root).expect("stress demo document should build");

    DemoSceneSpec {
        kind: DemoSceneKind::Stress,
        label: "Stress demo",
        document,
    }
}

fn build_stress_gates(gate_count: u32) -> Vec<Gate> {
    let mut gates = Vec::with_capacity(gate_count as usize);
    gates.push(Gate::BitNot { src: this_ref(0) });
    for gate in 1..gate_count {
        let prev = gate - 1;
        let tap = gate.saturating_sub(37);
        let diag = gate.saturating_sub(113);
        gates.push(match gate % 6 {
            0 => Gate::BitNop {
                src: this_ref(prev),
            },
            1 => Gate::BitNot {
                src: this_ref(prev),
            },
            2 => Gate::BitXOR {
                a: this_ref(prev),
                b: this_ref(tap),
            },
            3 => Gate::BitAND {
                a: this_ref(prev),
                b: this_ref(diag),
            },
            4 => Gate::BitOR {
                a: this_ref(prev),
                b: this_ref(tap),
            },
            _ => Gate::BitXNOR {
                a: this_ref(prev),
                b: this_ref(diag),
            },
        });
    }
    gates
}

fn this_ref(gate: u32) -> SignalRef {
    SignalRef::ThisGate(GateId(gate))
}

fn input_ref(port: PortId) -> SignalRef {
    SignalRef::InputPort(port)
}

fn child_output_ref(child: u32, port: PortId) -> SignalRef {
    SignalRef::ChildOutput {
        child: ChildId(child),
        port,
    }
}

fn port_named(id: PortId, gate: u32, x: u16, y: u16, label: &'static str) -> ComponentPort {
    ComponentPort {
        id,
        gate: GateId(gate),
        location: PortLocation { x, y },
        label: Some(label.to_owned()),
    }
}
