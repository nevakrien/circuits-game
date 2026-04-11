use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use circuits_game::{
    gate_plans::{
        ChildId, ChildInputConnection, ChildPlacement, Component, ComponentPlan, ComponentPlans,
        ComponentPort, Gate, GateId, PortId, PortLocation, SignalRef, compile_component_tree,
    },
    kernel::{GateKernel, UploadedGpuPlan},
    setup,
    ui_config::{
        DEFAULT_TICK_RATE, FAST_RATE_FACTOR, MAX_FRAME_STEP_BUDGET, PAUSED_VISUAL_RATE_FACTOR,
        STEP_RATE_FACTOR,
    },
    visual_ui::{
        FocusedScene, SceneAction, ViewportState, build_focused_scene, child_ids_of,
        parent_stack_to, show_focused_scene,
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
const BITS_PER_BUFFER: u32 = 64;
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

    let (root, plans) = build_demo_circuit();
    let mut runtime =
        DemoRuntime::new(device, queue, root, plans).expect("demo runtime should build");
    let mut read_words = runtime.read_words(device, queue);
    let mut viewer = ViewerState::new(&runtime).expect("viewer should build initial scene");
    let animation_started_at = Instant::now();

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
                    let now = Instant::now();
                    let scheduled_steps = viewer.scheduled_steps(now);
                    let total_steps = scheduled_steps.saturating_add(u32::from(step_once));
                    for _ in 0..total_steps {
                        runtime.step(device, queue);
                    }
                    if total_steps > 0 {
                        read_words = runtime.read_words(device, queue);
                    }

                    let full_output = egui_ctx.run(raw_input, |ctx| {
                        viewer.apply_continuous_input(ctx);
                        egui::TopBottomPanel::top("top-bar").show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                ui.heading("Circuit Viewer");
                                ui.separator();
                                ui.monospace(format!(
                                    "read buffer {} | node {} | {} child components | {} wires",
                                    runtime.current_read,
                                    viewer.focus_node.0,
                                    viewer.child_ids.len(),
                                    viewer.scene.wires.len()
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
                                    }
                                }
                            });
                            ui.label("Drag to pan, use arrow keys to move, ctrl/cmd+wheel or trackpad pinch to zoom, click child blocks to drill into them.");
                            ui.label("Hotkeys: R run, T slow-step, F fast, P pause, Space single-step. Pause freezes the simulation state but keeps charge flow animated on screen.");
                        });

                        egui::CentralPanel::default().show(ctx, |ui| {
                            if let Some(action) = show_focused_scene(
                                ui,
                                &viewer.scene,
                                &read_words,
                                animation_started_at.elapsed().as_secs_f64(),
                                viewer.visual_pulse_rate_hz(),
                                &mut viewer.viewport,
                            ) {
                                match action {
                                    SceneAction::FocusChild(node) => {
                                        viewer.focus_node = node;
                                        viewer.reset_camera();
                                        viewer.rebuild_scene(&runtime).expect("child focus scene should rebuild");
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

                    {
                        let output_view = frame
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default());
                        let mut pass = encoder
                            .begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("viewer-pass"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &output_view,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(wgpu::Color {
                                            r: 0.04,
                                            g: 0.05,
                                            b: 0.07,
                                            a: 1.0,
                                        }),
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
    kernel: GateKernel,
    uploaded: UploadedGpuPlan,
    charge_buffers: [wgpu::Buffer; 2],
    output_buffer: wgpu::Buffer,
    current_read: usize,
    storage_words: u32,
    root: Component,
    plans: ComponentPlans,
    gate_store: foldhash::HashMap<
        (circuits_game::gate_plans::NodeId, GateId),
        circuits_game::gate_plans::GateStoreLocation,
    >,
    words_per_buffer: u32,
    tick: u64,
}

impl DemoRuntime {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut root: Component,
        plans: ComponentPlans,
    ) -> Result<Self, String> {
        let compiled = compile_component_tree(&mut root, &plans, BITS_PER_BUFFER)
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
            storage_words,
            root,
            plans,
            gate_store: compiled.gate_store,
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

    fn read_words(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u32> {
        let size = self.storage_words.max(1) as u64 * std::mem::size_of::<u32>() as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewer-readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewer-readback-copy"),
        });
        encoder.copy_buffer_to_buffer(
            &self.charge_buffers[self.current_read],
            0,
            &readback,
            0,
            size,
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        receiver
            .recv()
            .expect("readback channel should receive a result")
            .expect("readback buffer should map successfully");

        let mapped = slice.get_mapped_range();
        let words = bytemuck::cast_slice::<u8, u32>(&mapped).to_vec();
        drop(mapped);
        readback.unmap();
        words
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
            &runtime.gate_store,
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
            &runtime.gate_store,
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

fn build_demo_circuit() -> (Component, ComponentPlans) {
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
            vec![port(INPUT_A, 0, 0, 1), port(INPUT_B, 1, 0, 2)],
            vec![
                port(OUTPUT_Y, 2, u16::MAX, 1),
                port(OUTPUT_Z, 4, u16::MAX, 2),
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
                    port(OUTPUT_Y, 3, u16::MAX, 1),
                    port(OUTPUT_Z, 6, u16::MAX, 3),
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

    (root, plans)
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

fn port(id: PortId, gate: u32, x: u16, y: u16) -> ComponentPort {
    ComponentPort {
        id,
        gate: GateId(gate),
        location: PortLocation { x, y },
    }
}
