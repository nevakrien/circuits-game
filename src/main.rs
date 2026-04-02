use circuits_game::{demo_scene, editor, render, simulation, windowing, wire_render, wires};
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
const DEFAULT_RENDER_ARENA_Z: u32 = 0;
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
    arena_z: u32,
    steps: u32,
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
  --arena-z <index>         Packed arena z to render (default: 0)
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
        arena_z: DEFAULT_RENDER_ARENA_Z,
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
            "--arena-z" => {
                let value = args
                    .next()
                    .ok_or_else(|| "Missing value for --arena-z".to_string())?;
                options.arena_z = parse_u32_flag("--arena-z", &value)?;
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
        .request_device(&simulation::device_descriptor(&adapter))
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

    let board = simulation::BoardTextures::new(&device, &queue);
    let simulation = simulation::Simulation::new(&device);
    let renderer = render::Renderer::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        surface_size,
    );
    let camera = render::CameraState::new(surface_size);
    let arena_z = options
        .arena_z
        .min(simulation::BOARD_LAYERS.saturating_sub(1));
    renderer.update_view_arena_z(&queue, camera, arena_z);

    let mut current_buffer = 0;
    for _ in 0..options.steps {
        let next_buffer = (current_buffer + 1) % simulation::CHARGE_BUFFER_COUNT;
        let mut step_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene-capture-step"),
        });
        simulation.step(
            &device,
            &mut step_encoder,
            &board,
            current_buffer,
            next_buffer,
        );
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
        board.charge_view(current_buffer),
        board.circuit_view(),
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

    let board = simulation::BoardTextures::new(&device, &queue);
    let simulation = simulation::Simulation::new(&device);
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
    let mut displayed_arena_z = 0;
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
    let mut editor_session = editor::EditorSession::new(render::create_editor_tool_previews(
        &device,
        &queue,
        &mut egui_renderer,
    ));
    let demo_component = demo_scene::starter_component();
    let mut edited_component = wire_render::WireRenderInfo::new();
    for wire in demo_component.wires {
        edited_component.add_wire_edge(wire);
    }
    wire_overlay.replace_wires(
        &device,
        &queue,
        edited_component.wire_edges().cloned().collect(),
    );
    wire_overlay.set_visible_arena_z(&device, &queue, displayed_arena_z);

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
                            reset_camera = editor_session.show(ctx, displayed_arena_z);
                        });
                        editor_session.sync_tool_state(&mut wire_overlay, &device, &queue);
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
                        editor_session.advance_visual_feedback();

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

                        let display_camera = editor_session.camera_with_feedback(camera);

                        renderer.update_view_arena_z(&queue, display_camera, displayed_arena_z);
                        hover_preview.update(
                            &queue,
                            display_camera,
                            editor_session.hover_preview_state(
                                display_camera,
                                cursor_position,
                                !egui_ctx.is_pointer_over_area(),
                            ),
                        );
                        wire_overlay.set_draft_color(
                            &device,
                            &queue,
                            editor_session.selected_wire_color(),
                        );
                        wire_overlay.update_camera(&device, &queue, display_camera);

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
                            simulation.step(
                                &device,
                                &mut encoder,
                                &board,
                                current_buffer,
                                next_buffer,
                            );
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
                            board.charge_view(display_frame),
                            board.circuit_view(),
                        );

                        wire_overlay.draw(
                            &device,
                            &mut encoder,
                            &frame.texture,
                            board.charge_view(current_buffer),
                            board.charge_view(display_frame),
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
                                            editor_session.redo(
                                                &board,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            )
                                        } else {
                                            editor_session.undo(
                                                &board,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            )
                                        };
                                        if changed {
                                            window.request_redraw();
                                        }
                                        return;
                                    }

                                    if !event.repeat && control_down && code == KeyCode::KeyY {
                                        if editor_session.redo(
                                            &board,
                                            &mut edited_component,
                                            &mut wire_overlay,
                                            &device,
                                            &queue,
                                        ) {
                                            window.request_redraw();
                                        }
                                        return;
                                    }

                                    if !event.repeat && code == KeyCode::Space {
                                        step_requested = true;
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::ArrowUp {
                                        displayed_arena_z = (displayed_arena_z + 1)
                                            .min(simulation::BOARD_LAYERS.saturating_sub(1));
                                        wire_overlay.set_visible_arena_z(
                                            &device,
                                            &queue,
                                            displayed_arena_z,
                                        );
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
                                            hover,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::ArrowDown {
                                        displayed_arena_z = displayed_arena_z.saturating_sub(1);
                                        wire_overlay.set_visible_arena_z(
                                            &device,
                                            &queue,
                                            displayed_arena_z,
                                        );
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
                                            hover,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::Enter {
                                        if editor_session.selected_tool()
                                            == editor::EditorTool::Wire
                                        {
                                            if editor_session.finish_wire_attempt(
                                                &board,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            ) {}
                                            window.request_redraw();
                                        }
                                    }

                                    if !event.repeat && code == KeyCode::Escape {
                                        editor_session.cancel_wire_draft(
                                            &mut wire_overlay,
                                            &device,
                                            &queue,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::Backspace {
                                        if editor_session.selected_tool()
                                            == editor::EditorTool::Wire
                                        {
                                            editor_session.pop_wire_point(
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            );
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
                        wire_overlay.update_hover(&device, &queue, hover);
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
                            let extend_wire = pressed_keys.contains(&KeyCode::ShiftLeft)
                                || pressed_keys.contains(&KeyCode::ShiftRight);
                            if editor_session.handle_left_click(
                                &board,
                                &mut edited_component,
                                &mut wire_overlay,
                                &device,
                                &queue,
                                camera,
                                cursor,
                                displayed_arena_z,
                                extend_wire,
                            ) {
                                window.request_redraw();
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

                        let extend_wire = pressed_keys.contains(&KeyCode::ShiftLeft)
                            || pressed_keys.contains(&KeyCode::ShiftRight);
                        if editor_session.handle_right_click(
                            &board,
                            &mut edited_component,
                            &mut wire_overlay,
                            &device,
                            &queue,
                            camera,
                            cursor_position,
                            displayed_arena_z,
                            extend_wire,
                        ) {
                            window.request_redraw();
                        }
                    }
                    WindowEvent::Resized(size) => {
                        if size.width > 0 && size.height > 0 {
                            windowing::resize_surface(&surface, &device, &mut config, size);
                            camera.resize(size);
                            renderer.update_view_arena_z(&queue, camera, displayed_arena_z);
                            wire_overlay.resize(&device, &queue, camera, size);
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
