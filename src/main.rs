use std::sync::mpsc;

use circuits_game::{
    gate_plans::{
        compile_component_tree, ChildId, ChildInputConnection, ChildPlacement, Component,
        ComponentPlan, ComponentPlans, ComponentPort, Gate, GateId, PortId, PortLocation,
        SignalRef,
    },
    kernel::{GateKernel, UploadedGpuPlan},
    setup,
    visual_ui::{
        build_focused_scene, child_ids_of, parent_stack_to, show_focused_scene, FocusedScene,
        SceneAction, ViewportState,
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
    let mut runtime = DemoRuntime::new(device, queue, root, plans).expect("demo runtime should build");
    let mut read_words = runtime.read_words(device, queue);
    let mut viewer = ViewerState::new(&runtime).expect("viewer should build initial scene");

    let egui_ctx = egui::Context::default();
    let mut egui_state = egui_winit::State::new(
        egui_ctx.clone(),
        egui::ViewportId::ROOT,
        &window,
        Some(window.scale_factor() as f32),
        window.theme(),
        None,
    );
    let mut egui_renderer = egui_wgpu::Renderer::new(
        device,
        config.format,
        egui_wgpu::RendererOptions::default(),
    );
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
                    let mut step_once = viewer.apply_hotkeys(&raw_input);
                    let should_step = match viewer.simulation_mode {
                        SimulationMode::Running => true,
                        SimulationMode::Paused => step_once,
                    };
                    if should_step {
                        runtime.step(device, queue);
                        read_words = runtime.read_words(device, queue);
                        step_once = false;
                    }

                    let full_output = egui_ctx.run(raw_input, |ctx| {
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
                                    viewer.simulation_mode = SimulationMode::Running;
                                }
                                if ui
                                    .selectable_label(
                                        matches!(viewer.simulation_mode, SimulationMode::Paused),
                                        "Pause",
                                    )
                                    .clicked()
                                {
                                    viewer.simulation_mode = SimulationMode::Paused;
                                }
                                if ui.button("Step").clicked() {
                                    step_once = true;
                                }
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
                            ui.label("Hotkeys: R run, P pause, Space single-step. This view shows one component at a time so nested components stay compressed.");
                        });

                        egui::CentralPanel::default().show(ctx, |ui| {
                            if let Some(action) = show_focused_scene(
                                ui,
                                &viewer.scene,
                                &read_words,
                                runtime.tick as f64,
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
    gate_store: foldhash::HashMap<(circuits_game::gate_plans::NodeId, GateId), circuits_game::gate_plans::GateStoreLocation>,
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
        let write_buffer = GateKernel::create_io_buffer(device, storage_words, "viewer-read-buffer-1");
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimulationMode {
    Running,
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
            simulation_mode: SimulationMode::Running,
        })
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

    fn apply_hotkeys(&mut self, raw_input: &egui::RawInput) -> bool {
        let pan_step = if raw_input.modifiers.shift { 72.0 } else { 28.0 };
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
                egui::Key::ArrowLeft => self.viewport.pan.x += pan_step,
                egui::Key::ArrowRight => self.viewport.pan.x -= pan_step,
                egui::Key::ArrowUp => self.viewport.pan.y += pan_step,
                egui::Key::ArrowDown => self.viewport.pan.y -= pan_step,
                egui::Key::R if !repeat => self.simulation_mode = SimulationMode::Running,
                egui::Key::P if !repeat => self.simulation_mode = SimulationMode::Paused,
                egui::Key::Space if !repeat => step_once = true,
                _ => {}
            }
        }
        step_once
    }
}

fn seed_demo_words(
    gate_store: &foldhash::HashMap<(circuits_game::gate_plans::NodeId, GateId), circuits_game::gate_plans::GateStoreLocation>,
    words_per_buffer: u32,
    storage_words: u32,
) -> Vec<u32> {
    let mut words = vec![0u32; storage_words as usize];

    set_gate_seed(gate_store, &mut words, words_per_buffer, GateId(0), true);
    set_gate_seed(gate_store, &mut words, words_per_buffer, GateId(1), false);
    words
}

fn set_gate_seed(
    gate_store: &foldhash::HashMap<(circuits_game::gate_plans::NodeId, GateId), circuits_game::gate_plans::GateStoreLocation>,
    words: &mut [u32],
    words_per_buffer: u32,
    gate: GateId,
    value: bool,
) {
    let Some((&(node, _gate), store)) = gate_store.iter().find(|((node, candidate), _)| {
        node.0 == 0 && *candidate == gate
    }) else {
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
                Gate::BitAND {
                    a: this_ref(0),
                    b: this_ref(1),
                },
                Gate::BitXOR {
                    a: this_ref(0),
                    b: this_ref(1),
                },
            ],
            vec![port(INPUT_A, 0, 0, 1), port(INPUT_B, 1, 0, 2)],
            vec![port(OUTPUT_Y, 2, u16::MAX, 1), port(OUTPUT_Z, 3, u16::MAX, 2)],
        )
        .with_grid_size([2, 2]),
        vec![],
    );

    let root = Component::with_child_input_connections(
        plans.insert(
            ComponentPlan::with_ports(
                vec![
                    Gate::BitNop { src: this_ref(0) },
                    Gate::BitNop { src: this_ref(1) },
                    Gate::BitNop {
                        src: child_output_ref(0, OUTPUT_Y),
                    },
                    Gate::BitNot {
                        src: child_output_ref(0, OUTPUT_Y),
                    },
                    Gate::BitNop {
                        src: child_output_ref(0, OUTPUT_Z),
                    },
                ],
                vec![],
                vec![port(OUTPUT_Z, 4, u16::MAX, u16::MAX)],
            )
            .with_grid_size([4, 4])
            .with_child_placements(vec![ChildPlacement {
                min: PortLocation {
                    x: u16::MAX / 2,
                    y: 0,
                },
                max: PortLocation {
                    x: u16::MAX,
                    y: u16::MAX / 2,
                },
            }]),
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
