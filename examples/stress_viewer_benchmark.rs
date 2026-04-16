use std::{
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use circuits_game::{
    gate_plans::{
        ChildPlacement, Component, ComponentPlan, ComponentPlans, Gate, GateId, SignalRef,
        compile_component_tree,
    },
    kernel::{GateKernel, UploadedGpuPlan},
    scene_render::SceneRenderer,
    setup,
    viewer_frame::{ViewerRenderMode, render_viewer_frame},
    visual_ui::{FocusedScene, ViewportState, build_focused_scene, interact_focused_scene},
};
use egui_wgpu::wgpu;
use egui_winit::winit;
use wgpu::util::DeviceExt;
use winit::event::{Event, WindowEvent};

const STRESS_GATES_PER_COMPONENT: u32 = 8_192;
const STRESS_BRANCH_FACTOR: usize = 4;
const STRESS_DEPTH: u32 = 5;
const DEFAULT_FRAMES: u32 = 600;
const DEFAULT_TICKS_PER_FRAME: u32 = 1;
const DEFAULT_ZOOM: f32 = 2.0;
const DEFAULT_VISIBLE_SECONDS: f32 = 8.0;
const MIN_VIEWPORT_ZOOM: f32 = 0.01;

fn main() {
    let args = Args::parse();
    run(args);
}

#[allow(deprecated)]
fn run(args: Args) {
    let started_at = Instant::now();
    let setup::WindowState {
        event_loop,
        window,
        surface,
        mut config,
    } = setup::prepare_window();
    let gpu = setup::gpu();
    let device = &gpu.device;
    let queue = &gpu.queue;
    let adapter_info = gpu.adapter.get_info();

    window.set_title("Stress viewer benchmark");

    let caps = surface.get_capabilities(&gpu.adapter);
    if caps.formats.contains(&config.format) {
        config.present_mode = choose_present_mode(&caps.present_modes);
        surface.configure(device, &config);
    }

    let build_started_at = Instant::now();
    let scene = build_stress_demo_circuit();
    let scene_build = build_started_at.elapsed();

    let runtime_started_at = Instant::now();
    let runtime = BenchRuntime::new(device, queue, scene).expect("stress runtime should build");
    let runtime_build = runtime_started_at.elapsed();

    let focused_scene_started_at = Instant::now();
    let scene = build_focused_scene(
        &runtime.root,
        &runtime.plans,
        runtime.root.id,
        runtime.gate_store.clone(),
        runtime.words_per_buffer,
    )
    .expect("focused scene should build");
    let focused_scene_build = focused_scene_started_at.elapsed();

    let egui_ctx = egui::Context::default();
    egui_ctx.options_mut(|options| options.zoom_factor = 1.0);
    let egui_state = egui_winit::State::new(
        egui_ctx.clone(),
        egui::ViewportId::ROOT,
        &window,
        Some(window.scale_factor() as f32),
        window.theme(),
        None,
    );
    let egui_renderer =
        egui_wgpu::Renderer::new(device, config.format, egui_wgpu::RendererOptions::default());
    let mut scene_renderer = SceneRenderer::new(device, config.format);
    scene_renderer.upload_runtime_scene(device, queue, &scene);

    let mut bench = ViewerBenchState {
        args,
        adapter_name: adapter_info.name,
        backend: adapter_info.backend,
        startup: StartupMetrics {
            total_until_loop: started_at.elapsed(),
            scene_build,
            runtime_build,
            focused_scene_build,
        },
        scene,
        viewport: ViewportState::default(),
        viewport_initialized: false,
        frame_timings: Vec::with_capacity(args.frames as usize),
        frame_index: 0,
        first_present_at: None,
        config,
        egui_ctx,
        egui_state,
        egui_renderer,
        scene_renderer,
        runtime,
        app_started_at: started_at,
    };

    println!("stress viewer benchmark");
    println!("  adapter: {} ({:?})", bench.adapter_name, bench.backend);
    println!(
        "  present mode: {:?} | max frames: {} | visible seconds: {:.1} | ticks/frame: {} | zoom: {:.2}",
        bench.config.present_mode,
        bench.args.frames,
        bench.args.visible_seconds,
        bench.args.ticks_per_frame,
        bench.args.zoom
    );
    println!(
        "  scene: {} components, {} gates, depth {}",
        bench.runtime.component_count, bench.runtime.gate_count, bench.runtime.nesting_depth
    );
    println!(
        "  focused root scene: {} local gates, {} child previews, {} wires",
        bench.scene.gates.len(),
        bench.scene.children.len(),
        bench.scene.wires.len()
    );
    println!("startup");
    println!(
        "  build stress scene:      {}",
        fmt_duration(bench.startup.scene_build)
    );
    println!(
        "  compile/upload runtime:  {}",
        fmt_duration(bench.startup.runtime_build)
    );
    println!(
        "  build focused scene:     {}",
        fmt_duration(bench.startup.focused_scene_build)
    );
    println!(
        "  ready to first visible frame: {}",
        fmt_duration(bench.startup.total_until_loop)
    );

    window.request_redraw();

    let _ = event_loop.run(move |event, target| match event {
        Event::Resumed | Event::AboutToWait => {
            window.request_redraw();
        }
        Event::WindowEvent { event, .. } => {
            let response = bench.egui_state.on_window_event(&window, &event);
            match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => {
                    bench.config.width = size.width.max(1);
                    bench.config.height = size.height.max(1);
                    surface.configure(device, &bench.config);
                    bench.viewport_initialized = false;
                }
                WindowEvent::RedrawRequested => {
                    let raw_input = bench.egui_state.take_egui_input(&window);

                    let tick_started_at = Instant::now();
                    for _ in 0..bench.args.ticks_per_frame {
                        bench.runtime.step(device, queue);
                    }
                    let tick_time = tick_started_at.elapsed();

                    let ui_started_at = Instant::now();
                    let mut scene_rect = None;
                    let mut hover_world = None;
                    let full_output = bench.egui_ctx.run(raw_input, |ctx| {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
                            .show(ctx, |ui| {
                                if !bench.viewport_initialized {
                                    bench.viewport = centered_zoom_viewport(
                                        ui.available_size_before_wrap(),
                                        &bench.scene,
                                        bench.args.zoom,
                                    );
                                    bench.viewport_initialized = true;
                                }
                                let viewport_output =
                                    interact_focused_scene(ui, &bench.scene, &mut bench.viewport);
                                scene_rect = Some(viewport_output.rect);
                                hover_world = viewport_output.hover_world;
                            });
                    });
                    let ui_time = ui_started_at.elapsed();

                    let tessellate_started_at = Instant::now();
                    let egui::FullOutput {
                        platform_output,
                        textures_delta,
                        shapes,
                        pixels_per_point,
                        ..
                    } = full_output;
                    let paint_jobs = bench.egui_ctx.tessellate(shapes, pixels_per_point);
                    let tessellate_time = tessellate_started_at.elapsed();

                    let render_started_at = Instant::now();
                    let render_result = render_viewer_frame(
                        device,
                        queue,
                        &surface,
                        &bench.config,
                        &mut bench.egui_renderer,
                        &bench.scene_renderer,
                        scene_rect,
                        hover_world,
                        pixels_per_point,
                        &bench.viewport,
                        ViewerRenderMode::Run,
                        &bench.runtime.charge_buffers[bench.runtime.current_read],
                        &bench.runtime.charge_buffers
                            [(bench.runtime.current_read + 1) % bench.runtime.charge_buffers.len()],
                        bench.app_started_at.elapsed().as_secs_f32(),
                        60.0,
                        &textures_delta,
                        &paint_jobs,
                    );
                    let render_encode_time = render_started_at.elapsed();

                    let frame_stats = match render_result {
                        Ok(stats) => stats,
                        Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                            surface.configure(device, &bench.config);
                            return;
                        }
                        Err(wgpu::SurfaceError::Timeout) => return,
                        Err(wgpu::SurfaceError::OutOfMemory) => {
                            target.exit();
                            return;
                        }
                        Err(wgpu::SurfaceError::Other) => return,
                    };

                    bench
                        .egui_state
                        .handle_platform_output(&window, platform_output);

                    if bench.first_present_at.is_none() {
                        bench.first_present_at = Some(Instant::now());
                    }

                    let frame_total = tick_started_at.elapsed();
                    bench.frame_timings.push(FrameMetrics {
                        tick: tick_time,
                        ui: ui_time,
                        tessellate: tessellate_time,
                        texture_updates: frame_stats.texture_updates,
                        acquire_surface: frame_stats.acquire_surface,
                        update_buffers: frame_stats.update_buffers,
                        render_encode: render_encode_time,
                        submit_present: frame_stats.submit_present,
                        total: frame_total,
                    });
                    bench.frame_index += 1;

                    let visible_enough = bench.first_present_at.is_some_and(|started_at| {
                        started_at.elapsed().as_secs_f32() >= bench.args.visible_seconds
                    });
                    if bench.frame_index >= bench.args.frames || visible_enough {
                        print_frame_summary(&bench);
                        target.exit();
                        return;
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

#[derive(Clone, Copy)]
struct Args {
    frames: u32,
    ticks_per_frame: u32,
    zoom: f32,
    visible_seconds: f32,
}

impl Args {
    fn parse() -> Self {
        let mut frames = DEFAULT_FRAMES;
        let mut ticks_per_frame = DEFAULT_TICKS_PER_FRAME;
        let mut zoom = DEFAULT_ZOOM;
        let mut visible_seconds = DEFAULT_VISIBLE_SECONDS;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--frames" => {
                    if let Some(value) = args.next() {
                        frames = value.parse().expect("--frames must be a positive integer");
                    }
                }
                "--ticks-per-frame" => {
                    if let Some(value) = args.next() {
                        ticks_per_frame = value
                            .parse()
                            .expect("--ticks-per-frame must be a non-negative integer");
                    }
                }
                "--zoom" => {
                    if let Some(value) = args.next() {
                        zoom = value.parse().expect("--zoom must be a number");
                    }
                }
                "--visible-seconds" => {
                    if let Some(value) = args.next() {
                        visible_seconds = value
                            .parse()
                            .expect("--visible-seconds must be a positive number");
                    }
                }
                _ => {}
            }
        }
        Self {
            frames: frames.max(1),
            ticks_per_frame,
            zoom: zoom.max(MIN_VIEWPORT_ZOOM),
            visible_seconds: visible_seconds.max(0.1),
        }
    }
}

struct DemoSceneSpec {
    component_count: u64,
    gate_count: u64,
    nesting_depth: u32,
    root: Component,
    plans: ComponentPlans,
}

struct BenchRuntime {
    component_count: u64,
    gate_count: u64,
    nesting_depth: u32,
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
}

impl BenchRuntime {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: DemoSceneSpec,
    ) -> Result<Self, String> {
        let DemoSceneSpec {
            component_count,
            gate_count,
            nesting_depth,
            mut root,
            plans,
        } = scene;
        let bits_per_buffer = runtime_bits_per_buffer(device);
        let compiled = compile_component_tree(&mut root, &plans, bits_per_buffer)
            .map_err(|error| format!("failed to compile stress circuit: {error:?}"))?;
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
            label: Some("stress-viewer-read-buffer-0"),
            contents: bytemuck::cast_slice(&initial_words),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let write_buffer =
            GateKernel::create_io_buffer(device, storage_words, "stress-viewer-read-buffer-1");
        let output_buffer = GateKernel::create_io_buffer(
            device,
            compiled.gpu_plan.output_words,
            "stress-viewer-output",
        );
        queue.write_buffer(&write_buffer, 0, bytemuck::cast_slice(&initial_words));
        wait_for_gpu(device);

        Ok(Self {
            component_count,
            gate_count,
            nesting_depth,
            kernel,
            uploaded,
            charge_buffers: [read_buffer, write_buffer],
            output_buffer,
            current_read: 0,
            root,
            plans,
            gate_store: Arc::new(compiled.gate_store),
            words_per_buffer: compiled.gpu_plan.words_per_buffer,
        })
    }

    fn step(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let write_index = (self.current_read + 1) % self.charge_buffers.len();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("stress-viewer-step"),
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
        wait_for_gpu(device);
        self.current_read = write_index;
    }
}

struct ViewerBenchState {
    args: Args,
    adapter_name: String,
    backend: wgpu::Backend,
    startup: StartupMetrics,
    scene: FocusedScene,
    viewport: ViewportState,
    viewport_initialized: bool,
    frame_timings: Vec<FrameMetrics>,
    frame_index: u32,
    first_present_at: Option<Instant>,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    scene_renderer: SceneRenderer,
    runtime: BenchRuntime,
    app_started_at: Instant,
}

#[derive(Clone, Copy)]
struct StartupMetrics {
    total_until_loop: Duration,
    scene_build: Duration,
    runtime_build: Duration,
    focused_scene_build: Duration,
}

#[derive(Clone, Copy, Default)]
struct FrameMetrics {
    tick: Duration,
    ui: Duration,
    tessellate: Duration,
    texture_updates: Duration,
    acquire_surface: Duration,
    update_buffers: Duration,
    render_encode: Duration,
    submit_present: Duration,
    total: Duration,
}

fn centered_zoom_viewport(
    available: egui::Vec2,
    _scene: &FocusedScene,
    zoom: f32,
) -> ViewportState {
    const SCREENS_RIGHT: f32 = 2.5;
    const SCREENS_DOWN: f32 = 2.5;

    ViewportState::new(
        zoom,
        // Start a few viewport widths/heights into the scene so the benchmark lands on
        // the more populated interior instead of the sparse origin area.
        egui::vec2(24.0, 24.0)
            - egui::vec2(available.x * SCREENS_RIGHT, available.y * SCREENS_DOWN),
    )
}

fn choose_present_mode(modes: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    if modes.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else if modes.contains(&wgpu::PresentMode::AutoNoVsync) {
        wgpu::PresentMode::AutoNoVsync
    } else {
        wgpu::PresentMode::Fifo
    }
}

fn print_frame_summary(bench: &ViewerBenchState) {
    let frames = &bench.frame_timings;
    let warmup = frames.first().copied().unwrap_or_default();
    let steady = if frames.len() > 1 {
        &frames[1..]
    } else {
        frames.as_slice()
    };
    let avg = average_frame_metrics(steady);

    println!("frames");
    println!("  first frame total:      {}", fmt_duration(warmup.total));
    println!("    tick:                 {}", fmt_duration(warmup.tick));
    println!("    egui ui:              {}", fmt_duration(warmup.ui));
    println!(
        "    tessellate:           {}",
        fmt_duration(warmup.tessellate)
    );
    println!(
        "    texture updates:      {}",
        fmt_duration(warmup.texture_updates)
    );
    println!(
        "    surface acquire:      {}",
        fmt_duration(warmup.acquire_surface)
    );
    println!(
        "    update buffers:       {}",
        fmt_duration(warmup.update_buffers)
    );
    println!(
        "    render encode:        {}",
        fmt_duration(warmup.render_encode)
    );
    println!(
        "    submit/present:       {}",
        fmt_duration(warmup.submit_present)
    );

    println!("steady state");
    println!(
        "  avg frame over {} frames: {} ({:.1} fps)",
        steady.len(),
        fmt_duration(avg.total),
        duration_fps(avg.total)
    );
    println!("  avg tick:               {}", fmt_duration(avg.tick));
    println!("  avg egui ui:            {}", fmt_duration(avg.ui));
    println!("  avg tessellate:         {}", fmt_duration(avg.tessellate));
    println!(
        "  avg texture updates:    {}",
        fmt_duration(avg.texture_updates)
    );
    println!(
        "  avg surface acquire:    {}",
        fmt_duration(avg.acquire_surface)
    );
    println!(
        "  avg update buffers:     {}",
        fmt_duration(avg.update_buffers)
    );
    println!(
        "  avg render encode:      {}",
        fmt_duration(avg.render_encode)
    );
    println!(
        "  avg submit/present:     {}",
        fmt_duration(avg.submit_present)
    );
}

fn average_frame_metrics(frames: &[FrameMetrics]) -> FrameMetrics {
    if frames.is_empty() {
        return FrameMetrics::default();
    }
    let len = frames.len() as f64;
    FrameMetrics {
        tick: avg_duration(frames.iter().map(|frame| frame.tick), len),
        ui: avg_duration(frames.iter().map(|frame| frame.ui), len),
        tessellate: avg_duration(frames.iter().map(|frame| frame.tessellate), len),
        texture_updates: avg_duration(frames.iter().map(|frame| frame.texture_updates), len),
        acquire_surface: avg_duration(frames.iter().map(|frame| frame.acquire_surface), len),
        update_buffers: avg_duration(frames.iter().map(|frame| frame.update_buffers), len),
        render_encode: avg_duration(frames.iter().map(|frame| frame.render_encode), len),
        submit_present: avg_duration(frames.iter().map(|frame| frame.submit_present), len),
        total: avg_duration(frames.iter().map(|frame| frame.total), len),
    }
}

fn avg_duration(values: impl Iterator<Item = Duration>, len: f64) -> Duration {
    Duration::from_secs_f64(values.map(|value| value.as_secs_f64()).sum::<f64>() / len)
}

fn duration_fps(duration: Duration) -> f64 {
    if duration.is_zero() {
        0.0
    } else {
        1.0 / duration.as_secs_f64()
    }
}

fn fmt_duration(duration: Duration) -> String {
    if duration.as_secs_f64() >= 1.0 {
        format!("{:.3} s", duration.as_secs_f64())
    } else if duration.as_secs_f64() >= 0.001 {
        format!("{:.3} ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{:.3} us", duration.as_secs_f64() * 1_000_000.0)
    }
}

fn wait_for_gpu(device: &wgpu::Device) {
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
}

fn runtime_bits_per_buffer(device: &wgpu::Device) -> u32 {
    let max_storage_bytes = device.limits().max_storage_buffer_binding_size;
    let max_storage_bits = max_storage_bytes.saturating_mul(8);
    (max_storage_bits & !31).max(32)
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
    let Some((&(_, _), store)) = gate_store
        .iter()
        .find(|((node, candidate), _)| node.0 == 0 && *candidate == gate)
    else {
        return;
    };
    let word_index = store.buffer.0 * words_per_buffer + (store.bit.0 / 32);
    if let Some(word) = words.get_mut(word_index as usize) {
        if value {
            *word |= 1u32 << store.bit_in_word;
        } else {
            *word &= !(1u32 << store.bit_in_word);
        }
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
            .with_grid_size([256, 160]),
    );
    let root = build_stress_component_tree(branch_plan, leaf_plan, STRESS_DEPTH);

    DemoSceneSpec {
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
    Component::with_layout_and_child_input_connections(
        branch_plan,
        children,
        Vec::new(),
        circuits_game::gate_plans::ComponentLayout::default().with_child_placements(vec![
            ChildPlacement::at([0, 0]),
            ChildPlacement::at([128, 0]),
            ChildPlacement::at([0, 80]),
            ChildPlacement::at([128, 80]),
        ]),
    )
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
