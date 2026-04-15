use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use circuits_game::{
    gate_plans::{
        ChildId, ChildInputConnection, ChildPlacement, Component, ComponentPlan, ComponentPlans,
        ComponentPort, Gate, GateId, PortId, PortLocation, SignalRef, compile_component_tree,
    },
    kernel::{GateKernel, UploadedGpuPlan},
    scene_render::SceneRenderer,
    setup,
    ui_config::{
        DEFAULT_TICK_RATE, FAST_RATE_FACTOR, MAX_FRAME_STEP_BUDGET, PAUSED_VISUAL_RATE_FACTOR,
        STEP_RATE_FACTOR,
    },
    visual_ui::{
        FocusedScene, SceneAction, ViewportState, build_focused_scene, child_ids_of,
        interact_focused_scene, parent_stack_to,
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
    component_count: u64,
    gate_count: u64,
    nesting_depth: u32,
    root: Component,
    plans: ComponentPlans,
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

    let mut runtime = DemoRuntime::new(device, queue, build_demo_circuit(DemoSceneKind::Starter))
        .expect("demo runtime should build");
    let mut viewer = ViewerState::new(&runtime).expect("viewer should build initial scene");
    let animation_started_at = Instant::now();
    let mut scene_renderer = SceneRenderer::new(device, config.format);
    scene_renderer.upload_scene(device, queue, &viewer.scene);

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
                    let step_once = viewer.apply_hotkeys(&raw_input);
                    let mut requested_scene = None;
                    let mut scene_rect = None;
                    let now = Instant::now();
                    let scheduled_steps = viewer.scheduled_steps(now);
                    let total_steps = scheduled_steps.saturating_add(u32::from(step_once));
                    for _ in 0..total_steps {
                        runtime.step(device, queue);
                    }

                    let full_output = egui_ctx.run(raw_input, |ctx| {
                        viewer.apply_continuous_input(ctx);
                        egui::TopBottomPanel::top("top-bar").show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                ui.heading("Circuit Viewer");
                                ui.separator();
                                ui.monospace(format!(
                                    "{} | read buffer {} | {} logical buffers | node {} | {} child components | {} wires",
                                    runtime.scene_label,
                                    runtime.current_read,
                                    runtime.buffer_count,
                                    viewer.focus_node.0,
                                    viewer.child_ids.len(),
                                    viewer.scene.wires.len()
                                ));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Scene:");
                                for kind in DemoSceneKind::ALL {
                                    if ui
                                        .selectable_label(runtime.scene_kind == kind, kind.label())
                                        .clicked()
                                    {
                                        requested_scene = Some(kind);
                                    }
                                }
                                ui.separator();
                                ui.monospace(format!(
                                    "{} comps | {} gates | depth {}",
                                    runtime.component_count,
                                    runtime.gate_count,
                                    runtime.nesting_depth
                                ));
                            });
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
                                let mut path = parent_stack_to(&runtime.root, viewer.focus_node)
                                    .unwrap_or_default();
                                path.push(viewer.focus_node);
                                for (i, node) in path.iter().copied().enumerate() {
                                    if i > 0 {
                                        ui.label("> ");
                                    }
                                    if ui.button(format!("{}", node.0)).clicked() {
                                        viewer.focus_node = node;
                                        viewer.reset_camera();
                                        viewer.rebuild_scene(&runtime).expect("focused scene should rebuild");
                                        scene_renderer.upload_scene(device, queue, &viewer.scene);
                                    }
                                }
                            });
                            ui.label("Drag to pan, use arrow keys to move, ctrl/cmd+wheel or trackpad pinch to zoom, click child blocks to drill into them.");
                            ui.label("Hotkeys: R run, T slow-step, F fast, P pause, Space single-step. Pause freezes the simulation state but keeps charge flow animated on screen.");
                        });

                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
                            .show(ctx, |ui| {
                            let viewport_output = interact_focused_scene(
                                ui,
                                &viewer.scene,
                                &mut viewer.viewport,
                            );
                            scene_rect = Some(viewport_output.rect);
                            if let Some(action) = viewport_output.action {
                                match action {
                                    SceneAction::FocusChild(node) => {
                                        viewer.focus_node = node;
                                        viewer.reset_camera();
                                        viewer.rebuild_scene(&runtime).expect("child focus scene should rebuild");
                                        scene_renderer.upload_scene(device, queue, &viewer.scene);
                                    }
                                }
                            }
                        });
                    });
                    egui_state.handle_platform_output(&window, full_output.platform_output);
                    let paint_jobs =
                        egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
                    let screen_descriptor = egui_wgpu::ScreenDescriptor {
                        size_in_pixels: [config.width, config.height],
                        pixels_per_point: full_output.pixels_per_point,
                    };

                    for (id, image_delta) in &full_output.textures_delta.set {
                        egui_renderer.update_texture(device, queue, *id, image_delta);
                    }

                    let frame = match surface.get_current_texture() {
                        Ok(frame) => frame,
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
                    };

                    let mut encoder =
                        device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                    egui_renderer.update_buffers(
                        device,
                        queue,
                        &mut encoder,
                        &paint_jobs,
                        &screen_descriptor,
                    );

                    let output_view = frame
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    scene_renderer.draw(
                        device,
                        queue,
                        &mut encoder,
                        &output_view,
                        [config.width, config.height],
                        scene_rect,
                        full_output.pixels_per_point,
                        &viewer.viewport,
                        &runtime.charge_buffers[runtime.current_read],
                        &runtime.charge_buffers[(runtime.current_read + 1) % runtime.charge_buffers.len()],
                        animation_started_at.elapsed().as_secs_f32(),
                        viewer.visual_pulse_rate_hz(),
                    );

                    {
                        let mut pass = encoder
                            .begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("viewer-egui-pass"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &output_view,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Load,
                                        store: wgpu::StoreOp::Store,
                                    },
                                    depth_slice: None,
                                })],
                                ..Default::default()
                            })
                            .forget_lifetime();
                        egui_renderer.render(&mut pass, &paint_jobs, &screen_descriptor);
                    }

                    for id in &full_output.textures_delta.free {
                        egui_renderer.free_texture(id);
                    }

                    queue.submit(Some(encoder.finish()));
                    frame.present();

                    if let Some(kind) = requested_scene {
                        runtime = DemoRuntime::new(device, queue, build_demo_circuit(kind))
                            .expect("requested demo scene should build");
                        viewer = ViewerState::new(&runtime)
                            .expect("viewer should rebuild for requested scene");
                        scene_renderer.upload_scene(device, queue, &viewer.scene);
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
    scene_kind: DemoSceneKind,
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

impl DemoRuntime {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: DemoSceneSpec,
    ) -> Result<Self, String> {
        let DemoSceneSpec {
            kind,
            label,
            component_count,
            gate_count,
            nesting_depth,
            mut root,
            plans,
        } = scene;
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
            scene_kind: kind,
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
    focus_node: circuits_game::gate_plans::NodeId,
    child_ids: Vec<circuits_game::gate_plans::NodeId>,
    scene: FocusedScene,
    viewport: ViewportState,
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

impl ViewerState {
    fn new(runtime: &DemoRuntime) -> Result<Self, String> {
        let focus_node = runtime.root.id;
        let scene = build_focused_scene(
            &runtime.root,
            &runtime.plans,
            focus_node,
            runtime.gate_store.clone(),
            runtime.words_per_buffer,
        )
        .map_err(|error| format!("failed to build initial focused scene: {error:?}"))?;
        Ok(Self {
            focus_node,
            child_ids: child_ids_of(&runtime.root, focus_node),
            scene,
            viewport: ViewportState::default(),
            simulation_mode: SimulationMode::Paused,
            tick_rate: DEFAULT_TICK_RATE,
            last_frame_at: Instant::now(),
            step_accumulator: Duration::ZERO,
            pending_single_steps: 0,
        })
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
        self.viewport = ViewportState::default();
    }

    fn rebuild_scene(&mut self, runtime: &DemoRuntime) -> Result<(), String> {
        self.scene = build_focused_scene(
            &runtime.root,
            &runtime.plans,
            self.focus_node,
            runtime.gate_store.clone(),
            runtime.words_per_buffer,
        )
        .map_err(|error| format!("failed to rebuild focused scene: {error:?}"))?;
        self.child_ids = child_ids_of(&runtime.root, self.focus_node);
        Ok(())
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

    fn apply_hotkeys(&mut self, raw_input: &egui::RawInput) -> bool {
        let mut step_once = false;
        for event in &raw_input.events {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                ..
            } = event
            else {
                continue;
            };
            if !pressed {
                continue;
            }
            match key {
                egui::Key::R if !repeat => self.set_simulation_mode(SimulationMode::Running),
                egui::Key::T if !repeat => self.set_simulation_mode(SimulationMode::Stepping),
                egui::Key::F if !repeat => self.set_simulation_mode(SimulationMode::FastForward),
                egui::Key::P if !repeat => self.set_simulation_mode(SimulationMode::Paused),
                egui::Key::Space if !repeat => step_once = true,
                _ => {}
            }
        }
        step_once
    }
}

fn rate_to_interval(rate_hz: f32) -> Duration {
    if rate_hz <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(1.0 / rate_hz as f64)
    }
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
    let mut plans = ComponentPlans::new();
    let child = Component::from_plan(
        &mut plans,
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
        vec![],
    );

    let root = Component::with_child_input_connections(
        plans.insert(
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
            .with_grid_size([5, 5])
            .with_child_placements(vec![ChildPlacement::at([2, 2])]),
        ),
        vec![child],
        vec![
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
    );

    DemoSceneSpec {
        kind: DemoSceneKind::Starter,
        label: "Starter demo",
        component_count: 2,
        gate_count: 12,
        nesting_depth: 2,
        root,
        plans,
    }
}

fn build_stress_demo_circuit() -> DemoSceneSpec {
    let mut plans = ComponentPlans::new();
    let leaf_plan = plans.insert(
        ComponentPlan::new(build_stress_gates(STRESS_GATES_PER_COMPONENT))
            .with_grid_size([128, 64]),
    );
    let branch_plan = plans.insert(
        ComponentPlan::new(build_stress_gates(STRESS_GATES_PER_COMPONENT))
            .with_grid_size([256, 160])
            .with_child_placements(vec![
                ChildPlacement::at([0, 0]),
                ChildPlacement::at([128, 0]),
                ChildPlacement::at([0, 80]),
                ChildPlacement::at([128, 80]),
            ]),
    );
    let root = build_stress_component_tree(branch_plan, leaf_plan, STRESS_DEPTH);

    DemoSceneSpec {
        kind: DemoSceneKind::Stress,
        label: "Stress demo",
        component_count: geometric_series_total(STRESS_BRANCH_FACTOR as u64, STRESS_DEPTH),
        gate_count: geometric_series_total(STRESS_BRANCH_FACTOR as u64, STRESS_DEPTH)
            * STRESS_GATES_PER_COMPONENT as u64,
        nesting_depth: STRESS_DEPTH + 1,
        root,
        plans,
    }
}

fn build_stress_component_tree(
    branch_plan: circuits_game::gate_plans::PlanId,
    leaf_plan: circuits_game::gate_plans::PlanId,
    depth: u32,
) -> Component {
    if depth == 0 {
        return Component::new(leaf_plan, Vec::new());
    }

    let children = (0..STRESS_BRANCH_FACTOR)
        .map(|_| build_stress_component_tree(branch_plan, leaf_plan, depth - 1))
        .collect();
    Component::new(branch_plan, children)
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

fn geometric_series_total(branch_factor: u64, depth: u32) -> u64 {
    let mut total = 0u64;
    let mut layer = 1u64;
    for _ in 0..=depth {
        total += layer;
        layer *= branch_factor;
    }
    total
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
        label: Some(label),
    }
}
