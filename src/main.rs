use circuits_game::{editor, render, simulation, windowing, wire_render, wires};
use egui_wgpu::wgpu;
use egui_winit::winit;

use winit::{
    event::*,
    keyboard::{KeyCode, PhysicalKey},
};

use std::{
    collections::HashSet,
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
};

const DEFAULT_RENDER_WIDTH: u32 = 1600;
const DEFAULT_RENDER_HEIGHT: u32 = 900;
const DEFAULT_RENDER_LAYER: u32 = 0;
const DEFAULT_RENDER_STEPS: u32 = 0;

enum CliMode {
    Interactive,
    RenderScene(RenderSceneOptions),
    Help,
}

struct RenderSceneOptions {
    output_path: PathBuf,
    width: u32,
    height: u32,
    layer: u32,
    steps: u32,
}

#[derive(Clone)]
enum EditorAction {
    AddWire(wire_render::StoredWireEdge),
    DeleteWire(wire_render::StoredWireEdge),
    PlaceCell {
        grid_cell: wires::GridCell,
        layer: u32,
        previous_cell: simulation::CellSnapshot,
        previous_charge_values: Vec<u8>,
        new_cell: simulation::CellSnapshot,
        new_charge_values: Vec<u8>,
    },
    DeleteCell {
        grid_cell: wires::GridCell,
        layer: u32,
        cell: simulation::CellSnapshot,
        charge_values: Vec<u8>,
    },
}

type EditorHistory = editor::EditorHistory<EditorAction>;

trait EditorHistoryExt {
    fn undo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool;

    fn redo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool;
}

impl EditorHistoryExt for EditorHistory {
    fn undo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(action) = self.pop_undo() else {
            return false;
        };
        apply_inverse_action(&action, simulation, component, wire_overlay, device, queue);
        self.push_redo(action);
        true
    }

    fn redo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(action) = self.pop_redo() else {
            return false;
        };
        apply_action(&action, simulation, component, wire_overlay, device, queue);
        self.push_undo(action);
        true
    }
}

fn main() {
    let cli_mode = match parse_cli_args(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("{error}\n\n{}", cli_usage());
            std::process::exit(2);
        }
    };

    match cli_mode {
        CliMode::Interactive => pollster::block_on(run()),
        CliMode::RenderScene(options) => {
            if let Err(error) = pollster::block_on(render_scene_to_png(&options)) {
                eprintln!("Failed to render scene: {error}");
                std::process::exit(1);
            }
            println!("Rendered scene to {}", options.output_path.display());
        }
        CliMode::Help => {
            println!("{}", cli_usage());
        }
    }
}

fn cli_usage() -> &'static str {
    "Usage:
  cargo run -- [--render-scene [options]]

Options:
  --render-scene            Render to an image instead of opening a window
  --output <path>           Output PNG path (default: target/render/scene.png)
  --width <pixels>          Output width (default: 1600)
  --height <pixels>         Output height (default: 900)
  --layer <index>           Layer to render (default: 0)
  --steps <count>           Simulation steps before capture (default: 0)
  -h, --help                Show this help"
}

fn parse_cli_args<I>(args: I) -> Result<CliMode, String>
where
    I: IntoIterator<Item = String>,
{
    let mut options = RenderSceneOptions {
        output_path: PathBuf::from("target/render/scene.png"),
        width: DEFAULT_RENDER_WIDTH,
        height: DEFAULT_RENDER_HEIGHT,
        layer: DEFAULT_RENDER_LAYER,
        steps: DEFAULT_RENDER_STEPS,
    };
    let mut render_scene = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--render-scene" => render_scene = true,
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "Missing value for --output".to_string())?;
                options.output_path = PathBuf::from(value);
            }
            "--width" => {
                let value = args
                    .next()
                    .ok_or_else(|| "Missing value for --width".to_string())?;
                options.width = parse_u32_flag("--width", &value)?;
            }
            "--height" => {
                let value = args
                    .next()
                    .ok_or_else(|| "Missing value for --height".to_string())?;
                options.height = parse_u32_flag("--height", &value)?;
            }
            "--layer" => {
                let value = args
                    .next()
                    .ok_or_else(|| "Missing value for --layer".to_string())?;
                options.layer = parse_u32_flag("--layer", &value)?;
            }
            "--steps" => {
                let value = args
                    .next()
                    .ok_or_else(|| "Missing value for --steps".to_string())?;
                options.steps = parse_u32_flag("--steps", &value)?;
            }
            "-h" | "--help" => return Ok(CliMode::Help),
            _ => return Err(format!("Unknown argument: {arg}")),
        }
    }

    if options.width == 0 {
        return Err("--width must be greater than 0".to_string());
    }
    if options.height == 0 {
        return Err("--height must be greater than 0".to_string());
    }

    if render_scene {
        Ok(CliMode::RenderScene(options))
    } else {
        Ok(CliMode::Interactive)
    }
}

fn parse_u32_flag(flag: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("Invalid value for {flag}: {value}"))
}

async fn render_scene_to_png(options: &RenderSceneOptions) -> Result<(), String> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|error| format!("Unable to create adapter: {error}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|error| format!("Unable to create device: {error}"))?;

    let surface_size = winit::dpi::PhysicalSize::new(options.width, options.height);
    let output_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene-capture"),
        size: wgpu::Extent3d {
            width: options.width,
            height: options.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let simulation = simulation::Simulation::new(&device, &queue);
    let renderer = render::Renderer::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        surface_size,
    );
    let camera = render::CameraState::new(surface_size);
    let layer = options
        .layer
        .min(simulation::BOARD_LAYERS.saturating_sub(1));
    renderer.update_view_layer(&queue, camera, layer);

    let mut current_buffer = 0;
    for _ in 0..options.steps {
        let next_buffer = (current_buffer + 1) % simulation::CHARGE_BUFFER_COUNT;
        let mut step_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene-capture-step"),
        });
        simulation.step(&device, &mut step_encoder, current_buffer, next_buffer);
        queue.submit(Some(step_encoder.finish()));
        current_buffer = next_buffer;
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("scene-capture-render"),
    });
    renderer.draw(
        &device,
        &mut encoder,
        &output_texture,
        simulation.charge_view(current_buffer),
        simulation.circuit_view(),
    );

    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = options.width * bytes_per_pixel;
    let padded_bytes_per_row =
        unpadded_bytes_per_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback_size = u64::from(padded_bytes_per_row) * u64::from(options.height);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("scene-capture-readback"),
        size: readback_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(options.height),
            },
        },
        wgpu::Extent3d {
            width: options.width,
            height: options.height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    receiver
        .recv()
        .map_err(|_| "Failed to receive readback result".to_string())?
        .map_err(|error| format!("Failed to map readback buffer: {error}"))?;

    let mapped = slice.get_mapped_range();
    let mut rgba = vec![0u8; (options.width * options.height * bytes_per_pixel) as usize];
    for row in 0..options.height as usize {
        let src_start = row * padded_bytes_per_row as usize;
        let src_end = src_start + unpadded_bytes_per_row as usize;
        let dst_start = row * unpadded_bytes_per_row as usize;
        let dst_end = dst_start + unpadded_bytes_per_row as usize;
        rgba[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
    }
    drop(mapped);
    readback.unmap();

    write_png_rgba(
        options.output_path.as_path(),
        options.width,
        options.height,
        &rgba,
    )
}

fn write_png_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create output directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let file = File::create(path)
        .map_err(|error| format!("Failed to create {}: {error}", path.display()))?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder
        .write_header()
        .map_err(|error| format!("Failed to write PNG header: {error}"))?;
    png_writer
        .write_image_data(rgba)
        .map_err(|error| format!("Failed to write PNG pixels: {error}"))
}

async fn run() {
    let windowing::WindowState {
        event_loop,
        window,
        instance,
        adapter,
        device,
        queue,
    } = windowing::prepare_window().await;

    let surface = instance.create_surface(window.clone()).unwrap();
    let mut config = windowing::configure_surface(&surface, &adapter, &device, window.inner_size());

    let simulation = simulation::Simulation::new(&device, &queue);
    let renderer = render::Renderer::new(&device, &queue, config.format, window.inner_size());
    let hover_preview = render::HoverPreviewRenderer::new(&device, &queue, config.format);
    let mut camera = render::CameraState::new(window.inner_size());
    let mut wire_overlay = wires::WireOverlay::new(
        &device,
        &queue,
        config.format,
        window.inner_size(),
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    );
    let mut displayed_layer = 0;
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
        &device,
        config.format,
        egui_wgpu::RendererOptions::default(),
    );
    let mut editor_ui = editor::EditorUi::new(render::create_editor_tool_previews(
        &device,
        &queue,
        &mut egui_renderer,
    ));
    let mut history = EditorHistory::default();
    let mut edited_component = wire_render::WireRenderInfo::new(wire_render::WireBufferId {
        texture_index: 0,
        layer: displayed_layer,
    });

    let mut current_buffer = 0;
    let mut step_requested = false;
    let mut pressed_keys = HashSet::new();
    let mut cursor_position = None;

    event_loop
        .run(|event, target| match event {
            Event::AboutToWait => window.request_redraw(),

            Event::WindowEvent { event, .. } => {
                let egui_response = egui_state.on_window_event(&window, &event);

                match event {
                    WindowEvent::RedrawRequested => {
                        let raw_input = egui_state.take_egui_input(&window);
                        let mut reset_camera = false;
                        let full_output = egui_ctx.run(raw_input, |ctx| {
                            reset_camera = editor_ui.show(ctx, displayed_layer, &history);
                        });
                        if editor_ui.selected_tool() != editor::EditorTool::Wire {
                            wire_overlay.cancel_draft(&device, &queue);
                        }
                        egui_state.handle_platform_output(&window, full_output.platform_output);
                        let paint_jobs =
                            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
                        let screen_descriptor = egui_wgpu::ScreenDescriptor {
                            size_in_pixels: [config.width, config.height],
                            pixels_per_point: full_output.pixels_per_point,
                        };

                        for (id, image_delta) in &full_output.textures_delta.set {
                            egui_renderer.update_texture(&device, &queue, *id, image_delta);
                        }

                        let pan_step = 0.01 / camera.zoom;
                        let zoom_step = 1.02;

                        if reset_camera {
                            camera.reset_to_fit();
                        }

                        let wants_keyboard = egui_ctx.wants_keyboard_input();
                        if !wants_keyboard && pressed_keys.contains(&KeyCode::KeyW) {
                            camera.pan_by([0.0, -pan_step]);
                        }
                        if !wants_keyboard && pressed_keys.contains(&KeyCode::KeyS) {
                            camera.pan_by([0.0, pan_step]);
                        }
                        if !wants_keyboard && pressed_keys.contains(&KeyCode::KeyA) {
                            camera.pan_by([-pan_step, 0.0]);
                        }
                        if !wants_keyboard && pressed_keys.contains(&KeyCode::KeyD) {
                            camera.pan_by([pan_step, 0.0]);
                        }
                        if !wants_keyboard && pressed_keys.contains(&KeyCode::KeyQ) {
                            camera.zoom_by(zoom_step);
                        }
                        if !wants_keyboard && pressed_keys.contains(&KeyCode::KeyE) {
                            camera.zoom_by(1.0 / zoom_step);
                        }

                        renderer.update_view_layer(&queue, camera, displayed_layer);
                        hover_preview.update(
                            &queue,
                            camera,
                            hover_preview_state_with_visibility(
                                camera,
                                cursor_position,
                                editor_ui.selected_tool(),
                                !egui_ctx.is_pointer_over_area(),
                            ),
                        );
                        wire_overlay.update_camera(&queue, camera);

                        let next_buffer = (current_buffer + 1) % simulation::CHARGE_BUFFER_COUNT;

                        let frame = match surface.get_current_texture() {
                            Ok(frame) => frame,
                            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                                surface.configure(&device, &config);
                                return;
                            }
                            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::OutOfMemory) => {
                                return;
                            }
                            Err(wgpu::SurfaceError::Other) => return,
                        };

                        let display_frame = if step_requested {
                            next_buffer
                        } else {
                            current_buffer
                        };

                        let mut encoder = device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                        if step_requested {
                            simulation.step(&device, &mut encoder, current_buffer, next_buffer);
                        }

                        egui_renderer.update_buffers(
                            &device,
                            &queue,
                            &mut encoder,
                            &paint_jobs,
                            &screen_descriptor,
                        );

                        renderer.draw(
                            &device,
                            &mut encoder,
                            &frame.texture,
                            simulation.charge_view(display_frame),
                            simulation.circuit_view(),
                        );

                        wire_overlay.draw(
                            &device,
                            &mut encoder,
                            &frame.texture,
                            simulation.charge_view(current_buffer),
                            simulation.charge_view(display_frame),
                        );

                        hover_preview.draw(&device, &mut encoder, &frame.texture);

                        {
                            let output_view = frame
                                .texture
                                .create_view(&wgpu::TextureViewDescriptor::default());
                            let mut pass = encoder
                                .begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("egui-pass"),
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

                        if step_requested {
                            current_buffer = next_buffer;
                            edited_component.set_buffer_id(wire_render::WireBufferId {
                                texture_index: current_buffer,
                                layer: displayed_layer,
                            });
                            step_requested = false;
                        }
                    }
                    WindowEvent::KeyboardInput { event, .. } => {
                        if let PhysicalKey::Code(code) = event.physical_key {
                            match event.state {
                                ElementState::Pressed => {
                                    pressed_keys.insert(code);

                                    if egui_response.consumed {
                                        return;
                                    }

                                    let control_down = pressed_keys.contains(&KeyCode::ControlLeft)
                                        || pressed_keys.contains(&KeyCode::ControlRight);
                                    let shift_down = pressed_keys.contains(&KeyCode::ShiftLeft)
                                        || pressed_keys.contains(&KeyCode::ShiftRight);

                                    if !event.repeat && control_down && code == KeyCode::KeyZ {
                                        let changed = if shift_down {
                                            history.redo(
                                                &simulation,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            )
                                        } else {
                                            history.undo(
                                                &simulation,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            )
                                        };
                                        if changed {
                                            wire_overlay.cancel_draft(&device, &queue);
                                            window.request_redraw();
                                        }
                                        return;
                                    }

                                    if !event.repeat && control_down && code == KeyCode::KeyY {
                                        if history.redo(
                                            &simulation,
                                            &mut edited_component,
                                            &mut wire_overlay,
                                            &device,
                                            &queue,
                                        ) {
                                            wire_overlay.cancel_draft(&device, &queue);
                                            window.request_redraw();
                                        }
                                        return;
                                    }

                                    if !event.repeat && code == KeyCode::Space {
                                        step_requested = true;
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::ArrowUp {
                                        displayed_layer = (displayed_layer + 1)
                                            .min(simulation::BOARD_LAYERS.saturating_sub(1));
                                        edited_component.set_buffer_id(wire_render::WireBufferId {
                                            texture_index: current_buffer,
                                            layer: displayed_layer,
                                        });
                                        let hover = cursor_position.and_then(|cursor| {
                                            wires::cursor_to_board_point(
                                                camera,
                                                cursor,
                                                [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                                            )
                                        });
                                        wire_overlay.update_hover(
                                            &device,
                                            &queue,
                                            displayed_layer,
                                            hover,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::ArrowDown {
                                        displayed_layer = displayed_layer.saturating_sub(1);
                                        edited_component.set_buffer_id(wire_render::WireBufferId {
                                            texture_index: current_buffer,
                                            layer: displayed_layer,
                                        });
                                        let hover = cursor_position.and_then(|cursor| {
                                            wires::cursor_to_board_point(
                                                camera,
                                                cursor,
                                                [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                                            )
                                        });
                                        wire_overlay.update_hover(
                                            &device,
                                            &queue,
                                            displayed_layer,
                                            hover,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::Enter {
                                        if editor_ui.selected_tool() == editor::EditorTool::Wire {
                                            if let Some(action) = finish_wire_attempt(
                                                &mut wire_overlay,
                                                &mut edited_component,
                                                &device,
                                                &queue,
                                            ) {
                                                history.push(action);
                                            }
                                            window.request_redraw();
                                        }
                                    }

                                    if !event.repeat && code == KeyCode::Escape {
                                        wire_overlay.cancel_draft(&device, &queue);
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::Backspace {
                                        if editor_ui.selected_tool() == editor::EditorTool::Wire {
                                            wire_overlay.pop_point(&device, &queue);
                                        }
                                        window.request_redraw();
                                    }
                                }
                                ElementState::Released => {
                                    pressed_keys.remove(&code);
                                }
                            }
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let cursor = [position.x as f32, position.y as f32];
                        cursor_position = Some(cursor);
                        if egui_response.consumed || egui_ctx.wants_pointer_input() {
                            window.request_redraw();
                            return;
                        }
                        let hover = wires::cursor_to_board_point(
                            camera,
                            cursor,
                            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                        );
                        wire_overlay.update_hover(&device, &queue, displayed_layer, hover);
                        window.request_redraw();
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } => {
                        if egui_response.consumed || egui_ctx.wants_pointer_input() {
                            window.request_redraw();
                            return;
                        }

                        if let Some(cursor) = cursor_position {
                            if let Some(source) = wires::snap_cursor_to_cell(
                                camera,
                                cursor,
                                [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                            ) {
                                match editor_ui.selected_tool() {
                                    editor::EditorTool::Wire => {
                                        if let Some(point) = wires::cursor_to_board_point(
                                            camera,
                                            cursor,
                                            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                                        ) {
                                            let had_draft = wire_overlay.has_draft();
                                            wire_overlay.add_point(
                                                &device,
                                                &queue,
                                                displayed_layer,
                                                point,
                                                source,
                                            );
                                            let extend_wire = pressed_keys
                                                .contains(&KeyCode::ShiftLeft)
                                                || pressed_keys.contains(&KeyCode::ShiftRight);
                                            if had_draft && !extend_wire {
                                                if let Some(action) = finish_wire_attempt(
                                                    &mut wire_overlay,
                                                    &mut edited_component,
                                                    &device,
                                                    &queue,
                                                ) {
                                                    history.push(action);
                                                }
                                            }
                                            window.request_redraw();
                                        }
                                    }
                                    tool => {
                                        if let Some(action) = place_cell_at_cursor(
                                            &simulation,
                                            &device,
                                            &queue,
                                            camera,
                                            cursor,
                                            displayed_layer,
                                            tool,
                                        ) {
                                            history.push(action);
                                        }
                                        window.request_redraw();
                                    }
                                }
                            }
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Right,
                        ..
                    } => {
                        if egui_response.consumed || egui_ctx.wants_pointer_input() {
                            window.request_redraw();
                            return;
                        }

                        if editor_ui.selected_tool() != editor::EditorTool::Wire {
                            editor_ui.reset_to_default_tool();
                            wire_overlay.cancel_draft(&device, &queue);
                            window.request_redraw();
                            return;
                        }

                        if let Some(action) = delete_at_cursor(
                            &simulation,
                            &mut edited_component,
                            &mut wire_overlay,
                            &device,
                            &queue,
                            camera,
                            cursor_position,
                            displayed_layer,
                        ) {
                            history.push(action);
                        }
                        window.request_redraw();
                    }
                    WindowEvent::Resized(size) => {
                        if size.width > 0 && size.height > 0 {
                            windowing::resize_surface(&surface, &device, &mut config, size);
                            camera.resize(size);
                            renderer.update_view_layer(&queue, camera, displayed_layer);
                            wire_overlay.resize(&queue, camera, size);
                        }
                    }
                    WindowEvent::CloseRequested => target.exit(),
                    _ => {}
                }
            }

            _ => {}
        })
        .unwrap();
}

fn sync_component_wires(
    wire_overlay: &mut wires::WireOverlay,
    component: &wire_render::WireRenderInfo,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    wire_overlay.replace_wires(device, queue, component.wire_edges().cloned().collect());
}

fn finish_wire_attempt(
    wire_overlay: &mut wires::WireOverlay,
    component: &mut wire_render::WireRenderInfo,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Option<EditorAction> {
    if let Some(edge) = wire_overlay.commit_draft(device, queue) {
        component.add_wire_edge(edge.clone());
        sync_component_wires(wire_overlay, component, device, queue);
        Some(EditorAction::AddWire(edge))
    } else {
        wire_overlay.cancel_draft(device, queue);
        None
    }
}

fn delete_at_cursor(
    simulation: &simulation::Simulation,
    component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: render::CameraState,
    cursor_position: Option<[f32; 2]>,
    displayed_layer: u32,
) -> Option<EditorAction> {
    let Some(cursor) = cursor_position else {
        return None;
    };
    let Some(grid_cell) = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    ) else {
        return None;
    };
    let Some(point) = wires::cursor_to_board_point(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    ) else {
        return None;
    };

    if let Some(edge) = component.remove_wire_at_point(displayed_layer, point) {
        sync_component_wires(wire_overlay, component, device, queue);
        Some(EditorAction::DeleteWire(edge))
    } else {
        let cell = simulation.read_cell(device, queue, grid_cell, displayed_layer);
        let charge_values = (0..simulation::CHARGE_BUFFER_COUNT)
            .map(|buffer_index| {
                pollster::block_on(simulation.read_charge_value(
                    device,
                    queue,
                    buffer_index,
                    grid_cell.x,
                    grid_cell.y,
                    displayed_layer,
                ))
            })
            .collect::<Vec<_>>();
        if cell.bytes == [0, 0, 0, 0] && charge_values.iter().all(|value| *value == 0) {
            return None;
        }
        simulation.clear_cell(queue, grid_cell, displayed_layer);
        simulation.clear_charge_at(device, queue, grid_cell, displayed_layer);
        Some(EditorAction::DeleteCell {
            grid_cell,
            layer: displayed_layer,
            cell,
            charge_values,
        })
    }
}

fn place_cell_at_cursor(
    simulation: &simulation::Simulation,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: render::CameraState,
    cursor: [f32; 2],
    displayed_layer: u32,
    tool: editor::EditorTool,
) -> Option<EditorAction> {
    let grid_cell = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    )?;
    let new_cell = snapshot_for_tool(tool)?;
    let previous_cell = simulation.read_cell(device, queue, grid_cell, displayed_layer);

    if previous_cell == new_cell {
        return None;
    }

    let previous_charge_values = (0..simulation::CHARGE_BUFFER_COUNT)
        .map(|buffer_index| {
            pollster::block_on(simulation.read_charge_value(
                device,
                queue,
                buffer_index,
                grid_cell.x,
                grid_cell.y,
                displayed_layer,
            ))
        })
        .collect::<Vec<_>>();

    let new_charge_values = charge_values_for_tool(tool);

    simulation.write_cell(queue, grid_cell, displayed_layer, new_cell);
    write_charge_values(
        simulation,
        device,
        queue,
        grid_cell,
        displayed_layer,
        &new_charge_values,
    );

    Some(EditorAction::PlaceCell {
        grid_cell,
        layer: displayed_layer,
        previous_cell,
        previous_charge_values,
        new_cell,
        new_charge_values,
    })
}

fn hover_preview_state_with_visibility(
    camera: render::CameraState,
    cursor_position: Option<[f32; 2]>,
    tool: editor::EditorTool,
    visible: bool,
) -> Option<render::HoverPreviewState> {
    if !visible || !tool.is_placeable() {
        return None;
    }

    let cursor = cursor_position?;
    let grid_cell = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    )?;

    Some(render::HoverPreviewState {
        cell: [grid_cell.x, grid_cell.y],
        circuit: snapshot_for_tool(tool)?.bytes,
        charge: charge_values_for_tool(tool).into_iter().next().unwrap_or(0),
    })
}

fn snapshot_for_tool(tool: editor::EditorTool) -> Option<simulation::CellSnapshot> {
    match tool {
        editor::EditorTool::Wire => None,
        editor::EditorTool::Source => Some(simulation::CellSnapshot::source(0xff)),
        editor::EditorTool::Not => Some(simulation::CellSnapshot::gate(simulation::GateKind::Not)),
        editor::EditorTool::And => Some(simulation::CellSnapshot::gate(simulation::GateKind::And)),
        editor::EditorTool::Or => Some(simulation::CellSnapshot::gate(simulation::GateKind::Or)),
        editor::EditorTool::Xor => Some(simulation::CellSnapshot::gate(simulation::GateKind::Xor)),
        editor::EditorTool::Nand => {
            Some(simulation::CellSnapshot::gate(simulation::GateKind::Nand))
        }
        editor::EditorTool::Nor => Some(simulation::CellSnapshot::gate(simulation::GateKind::Nor)),
        editor::EditorTool::Xnor => {
            Some(simulation::CellSnapshot::gate(simulation::GateKind::Xnor))
        }
    }
}

fn charge_values_for_tool(tool: editor::EditorTool) -> Vec<u8> {
    let value = match tool {
        editor::EditorTool::Source => 0xff,
        _ => 0x00,
    };

    vec![value; simulation::CHARGE_BUFFER_COUNT as usize]
}

fn write_charge_values(
    simulation: &simulation::Simulation,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid_cell: wires::GridCell,
    layer: u32,
    charge_values: &[u8],
) {
    for (buffer_index, value) in charge_values.iter().copied().enumerate() {
        simulation.write_charge_value(device, queue, buffer_index as u32, grid_cell, layer, value);
    }
}

fn apply_action(
    action: &EditorAction,
    simulation: &simulation::Simulation,
    component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    match action {
        EditorAction::AddWire(edge) => {
            component.add_wire_edge(edge.clone());
            sync_component_wires(wire_overlay, component, device, queue);
        }
        EditorAction::DeleteWire(edge) => {
            component.remove_matching_wire_edge(edge);
            sync_component_wires(wire_overlay, component, device, queue);
        }
        EditorAction::PlaceCell {
            grid_cell,
            layer,
            new_cell,
            new_charge_values,
            ..
        } => {
            simulation.write_cell(queue, *grid_cell, *layer, *new_cell);
            write_charge_values(
                simulation,
                device,
                queue,
                *grid_cell,
                *layer,
                new_charge_values,
            );
        }
        EditorAction::DeleteCell {
            grid_cell, layer, ..
        } => {
            simulation.clear_cell(queue, *grid_cell, *layer);
            simulation.clear_charge_at(device, queue, *grid_cell, *layer);
        }
    }
}

fn apply_inverse_action(
    action: &EditorAction,
    simulation: &simulation::Simulation,
    component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    match action {
        EditorAction::AddWire(edge) => {
            component.remove_matching_wire_edge(edge);
            sync_component_wires(wire_overlay, component, device, queue);
        }
        EditorAction::DeleteWire(edge) => {
            component.add_wire_edge(edge.clone());
            sync_component_wires(wire_overlay, component, device, queue);
        }
        EditorAction::PlaceCell {
            grid_cell,
            layer,
            previous_cell,
            previous_charge_values,
            ..
        } => {
            simulation.write_cell(queue, *grid_cell, *layer, *previous_cell);
            write_charge_values(
                simulation,
                device,
                queue,
                *grid_cell,
                *layer,
                previous_charge_values,
            );
        }
        EditorAction::DeleteCell {
            grid_cell,
            layer,
            cell,
            charge_values,
        } => {
            simulation.write_cell(queue, *grid_cell, *layer, *cell);
            for (buffer_index, value) in charge_values.iter().copied().enumerate() {
                simulation.write_charge_value(
                    device,
                    queue,
                    buffer_index as u32,
                    *grid_cell,
                    *layer,
                    value,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use winit::dpi::PhysicalSize;

    const TEST_SURFACE_SIZE: PhysicalSize<u32> = PhysicalSize::new(1600, 900);

    async fn create_headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;

        adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()
    }

    fn create_editor_test_context() -> Option<(
        wgpu::Device,
        wgpu::Queue,
        simulation::Simulation,
        wire_render::WireRenderInfo,
        wires::WireOverlay,
        render::CameraState,
    )> {
        let (device, queue) = pollster::block_on(create_headless_device())?;
        let simulation = simulation::Simulation::new(&device, &queue);
        let component = wire_render::WireRenderInfo::new(wire_render::WireBufferId {
            texture_index: 0,
            layer: 0,
        });
        let wire_overlay = wires::WireOverlay::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            TEST_SURFACE_SIZE,
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        );
        let camera = render::CameraState::new(TEST_SURFACE_SIZE);

        Some((device, queue, simulation, component, wire_overlay, camera))
    }

    fn cursor_for_cell(cell: wires::GridCell) -> [f32; 2] {
        [
            (cell.x as f32 + 0.5) / simulation::GRID_WIDTH as f32 * TEST_SURFACE_SIZE.width as f32,
            (cell.y as f32 + 0.5) / simulation::GRID_HEIGHT as f32
                * TEST_SURFACE_SIZE.height as f32,
        ]
    }

    fn read_charge_values(
        simulation: &simulation::Simulation,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cell: wires::GridCell,
        layer: u32,
    ) -> Vec<u8> {
        (0..simulation::CHARGE_BUFFER_COUNT)
            .map(|buffer_index| {
                pollster::block_on(simulation.read_charge_value(
                    device,
                    queue,
                    buffer_index,
                    cell.x,
                    cell.y,
                    layer,
                ))
            })
            .collect()
    }

    #[test]
    fn placing_tools_writes_expected_cell_and_charge_values() {
        let Some((device, queue, simulation, _, _, camera)) = create_editor_test_context() else {
            return;
        };

        let cases = [
            (
                editor::EditorTool::Source,
                wires::GridCell { x: 0, y: 0 },
                simulation::CellSnapshot::source(0xff),
                vec![0xff; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::Not,
                wires::GridCell { x: 1, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::Not),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::And,
                wires::GridCell { x: 2, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::And),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::Or,
                wires::GridCell { x: 3, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::Or),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::Xor,
                wires::GridCell { x: 4, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::Xor),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::Nand,
                wires::GridCell { x: 5, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::Nand),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::Nor,
                wires::GridCell { x: 6, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::Nor),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                editor::EditorTool::Xnor,
                wires::GridCell { x: 7, y: 0 },
                simulation::CellSnapshot::gate(simulation::GateKind::Xnor),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
        ];

        for (tool, grid_cell, expected_cell, expected_charge_values) in cases {
            let action = place_cell_at_cursor(
                &simulation,
                &device,
                &queue,
                camera,
                cursor_for_cell(grid_cell),
                0,
                tool,
            )
            .unwrap();

            match action {
                EditorAction::PlaceCell {
                    grid_cell: action_grid_cell,
                    layer,
                    previous_cell,
                    previous_charge_values,
                    new_cell,
                    new_charge_values,
                } => {
                    assert_eq!(action_grid_cell, grid_cell);
                    assert_eq!(layer, 0);
                    assert_eq!(previous_cell, simulation::CellSnapshot::empty());
                    assert_eq!(
                        previous_charge_values,
                        vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize]
                    );
                    assert_eq!(new_cell, expected_cell);
                    assert_eq!(new_charge_values, expected_charge_values);
                }
                _ => panic!("expected place-cell action"),
            }

            assert_eq!(
                simulation.read_cell(&device, &queue, grid_cell, 0),
                expected_cell
            );
            assert_eq!(
                read_charge_values(&simulation, &device, &queue, grid_cell, 0),
                expected_charge_values
            );
        }
    }

    #[test]
    fn undo_and_redo_restore_previous_cell_and_charge_values() {
        let Some((device, queue, simulation, mut component, mut wire_overlay, camera)) =
            create_editor_test_context()
        else {
            return;
        };

        let grid_cell = wires::GridCell { x: 3, y: 4 };
        let layer = 0;
        let previous_cell = simulation::CellSnapshot::gate(simulation::GateKind::Not);
        let previous_charge_values = [0x12, 0x34];

        simulation.write_cell(&queue, grid_cell, layer, previous_cell);
        for (buffer_index, value) in previous_charge_values.into_iter().enumerate() {
            simulation.write_charge_value(
                &device,
                &queue,
                buffer_index as u32,
                grid_cell,
                layer,
                value,
            );
        }

        let action = place_cell_at_cursor(
            &simulation,
            &device,
            &queue,
            camera,
            cursor_for_cell(grid_cell),
            layer,
            editor::EditorTool::Source,
        )
        .unwrap();

        let mut history = EditorHistory::default();
        history.push(action);

        assert_eq!(
            simulation.read_cell(&device, &queue, grid_cell, layer),
            simulation::CellSnapshot::source(0xff)
        );
        assert_eq!(
            read_charge_values(&simulation, &device, &queue, grid_cell, layer),
            vec![0xff; simulation::CHARGE_BUFFER_COUNT as usize]
        );

        assert!(history.undo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            simulation.read_cell(&device, &queue, grid_cell, layer),
            previous_cell
        );
        assert_eq!(
            read_charge_values(&simulation, &device, &queue, grid_cell, layer),
            previous_charge_values.to_vec()
        );

        assert!(history.redo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            simulation.read_cell(&device, &queue, grid_cell, layer),
            simulation::CellSnapshot::source(0xff)
        );
        assert_eq!(
            read_charge_values(&simulation, &device, &queue, grid_cell, layer),
            vec![0xff; simulation::CHARGE_BUFFER_COUNT as usize]
        );
    }
}
