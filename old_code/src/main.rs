use circuits_game::{
    child_components, circuit_runtime, component_plan, editor, level_context, render, simulation,
    windowing, wire_render, wires,
};
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

enum AppMode {
    Edit,
    Run {
        runtime: circuit_runtime::CircuitRuntime,
        current_buffer: u32,
        step_requested: bool,
    },
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

fn schematic_base_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_schematic_path() -> PathBuf {
    schematic_base_directory().join(format!(
        "default.{}",
        level_context::SCHEMATIC_FILE_EXTENSION
    ))
}

fn dialogs_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn ensure_schematic_extension(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(extension) if extension == level_context::SCHEMATIC_FILE_EXTENSION => path,
        _ => path.with_extension(level_context::SCHEMATIC_FILE_EXTENSION),
    }
}

fn choose_schematic_save_path() -> Option<PathBuf> {
    if dialogs_available() {
        let file = rfd::FileDialog::new()
            .set_directory(schematic_base_directory())
            .add_filter(
                "Circuit schematic",
                &[level_context::SCHEMATIC_FILE_EXTENSION],
            )
            .set_file_name(&format!(
                "default.{}",
                level_context::SCHEMATIC_FILE_EXTENSION
            ))
            .save_file()?;
        return Some(ensure_schematic_extension(file));
    }

    Some(default_schematic_path())
}

fn choose_schematic_load_path() -> Option<PathBuf> {
    if dialogs_available() {
        return rfd::FileDialog::new()
            .set_directory(schematic_base_directory())
            .add_filter(
                "Circuit schematic",
                &[level_context::SCHEMATIC_FILE_EXTENSION],
            )
            .pick_file();
    }

    let fallback = default_schematic_path();
    fallback.exists().then_some(fallback)
}

fn sync_wire_overlay(
    wire_overlay: &mut wires::WireOverlay,
    wire_render_info: &wire_render::WireRenderInfo,
    visible_arena_z: u32,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    wire_overlay.replace_wires(
        device,
        queue,
        wire_render_info.wire_edges().cloned().collect(),
    );
    wire_overlay.set_visible_arena_z(device, queue, visible_arena_z);
    *displayed_arena_z = visible_arena_z;
}

fn build_run_mode(
    level_context: &level_context::LevelContext,
    root_component_id: component_plan::ComponentId,
    wire_overlay: &mut wires::WireOverlay,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    error_prefix: &str,
) -> Result<AppMode, String> {
    let runtime = circuit_runtime::CircuitRuntime::build_and_link(
        level_context,
        root_component_id,
        device,
        queue,
    )
    .map_err(|error| format!("{error_prefix}: {error}"))?;
    sync_wire_overlay(
        wire_overlay,
        &runtime.root.wires,
        0,
        displayed_arena_z,
        device,
        queue,
    );
    Ok(AppMode::Run {
        runtime,
        current_buffer: 0,
        step_requested: false,
    })
}

fn start_run_mode_from_edit(
    level_context: &mut level_context::LevelContext,
    root_component_id: component_plan::ComponentId,
    edit_board: &simulation::BoardTextures,
    edited_component: &wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<AppMode, String> {
    level_context
        .refresh_component_from_board(
            root_component_id,
            edit_board,
            edited_component,
            device,
            queue,
        )
        .map_err(|error| format!("Failed to refresh plan from edit board: {error}"))?;
    build_run_mode(
        level_context,
        root_component_id,
        wire_overlay,
        displayed_arena_z,
        device,
        queue,
        "Failed to build runtime",
    )
}

fn restore_edit_component(
    level_context: &level_context::LevelContext,
    component_id: component_plan::ComponentId,
    edit_board: &simulation::BoardTextures,
    edited_component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(), String> {
    *edited_component = level_context
        .upload_component_to_board(component_id, edit_board, device, queue)
        .map_err(|error| format!("Failed to restore edit mode board: {error}"))?;
    sync_wire_overlay(
        wire_overlay,
        edited_component,
        0,
        displayed_arena_z,
        device,
        queue,
    );
    Ok(())
}

fn switch_edited_component(
    level_context: &mut level_context::LevelContext,
    current_component_id: component_plan::ComponentId,
    next_component_id: component_plan::ComponentId,
    edit_board: &simulation::BoardTextures,
    edited_component: &mut wire_render::WireRenderInfo,
    editor_session: &mut editor::EditorSession,
    wire_overlay: &mut wires::WireOverlay,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(), String> {
    level_context
        .refresh_component_from_board(
            current_component_id,
            edit_board,
            edited_component,
            device,
            queue,
        )
        .map_err(|error| format!("Failed to save current component before switch: {error}"))?;
    restore_edit_component(
        level_context,
        next_component_id,
        edit_board,
        edited_component,
        wire_overlay,
        displayed_arena_z,
        device,
        queue,
    )?;
    editor_session.reset_for_loaded_schematic(wire_overlay, device, queue);
    Ok(())
}

fn editor_component_entries(
    level_context: &level_context::LevelContext,
) -> Vec<editor::EditorComponentListEntry> {
    let mut entries: Vec<_> = level_context
        .components()
        .map(|component| editor::EditorComponentListEntry {
            id: component.id,
            name: component.name.clone(),
            outside_shape: component.outside_shape.as_array(),
        })
        .collect();
    entries.sort_by_key(|entry| entry.id.0);
    entries
}

fn editor_child_overlays(
    level_context: &level_context::LevelContext,
    component_id: component_plan::ComponentId,
) -> Vec<editor::EditorChildInstanceOverlay> {
    let Some(component) = level_context.component(component_id) else {
        return Vec::new();
    };

    component
        .child_instances
        .iter()
        .filter_map(|instance| {
            let child = level_context.component(instance.component_id)?;
            Some(editor::EditorChildInstanceOverlay {
                component_id: instance.component_id,
                name: child.name.clone(),
                origin: instance.origin,
                size: child.outside_shape.as_array(),
            })
        })
        .collect()
}

fn place_child_instance_at_cursor(
    level_context: &mut level_context::LevelContext,
    current_component_id: component_plan::ComponentId,
    child_component_id: component_plan::ComponentId,
    edit_board: &simulation::BoardTextures,
    edited_component: &mut wire_render::WireRenderInfo,
    editor_session: &mut editor::EditorSession,
    wire_overlay: &mut wires::WireOverlay,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: render::CameraState,
    cursor: [f32; 2],
) -> Result<bool, String> {
    let Some(origin_cell) = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    ) else {
        return Ok(false);
    };

    let child_shape = level_context
        .component(child_component_id)
        .map(|component| component.outside_shape.as_array())
        .ok_or_else(|| format!("Missing child component {:?}", child_component_id))?;
    if origin_cell.x + child_shape[0] > simulation::GRID_WIDTH
        || origin_cell.y + child_shape[1] > simulation::GRID_HEIGHT
    {
        return Ok(false);
    }

    for y in 0..child_shape[1] {
        for x in 0..child_shape[0] {
            let cell = wires::GridCell {
                x: origin_cell.x + x,
                y: origin_cell.y + y,
            };
            if edit_board.read_cell(device, queue, cell, 0) != simulation::CellSnapshot::empty() {
                return Ok(false);
            }
        }
    }

    let plan = level_context
        .component_mut(current_component_id)
        .ok_or_else(|| format!("Missing edited component {:?}", current_component_id))?;
    if plan.child_instances.iter().any(|instance| {
        instance.component_id == child_component_id
            && instance.origin == [origin_cell.x, origin_cell.y]
    }) {
        return Ok(false);
    }
    plan.child_instances
        .push(child_components::ChildInstancePlan {
            component_id: child_component_id,
            origin: [origin_cell.x, origin_cell.y],
        });
    plan.sync_child_links();

    restore_edit_component(
        level_context,
        current_component_id,
        edit_board,
        edited_component,
        wire_overlay,
        displayed_arena_z,
        device,
        queue,
    )?;
    editor_session.reset_for_loaded_schematic(wire_overlay, device, queue);
    Ok(true)
}

fn delete_child_instance_at_cursor(
    level_context: &mut level_context::LevelContext,
    current_component_id: component_plan::ComponentId,
    edit_board: &simulation::BoardTextures,
    edited_component: &mut wire_render::WireRenderInfo,
    editor_session: &mut editor::EditorSession,
    wire_overlay: &mut wires::WireOverlay,
    displayed_arena_z: &mut u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: render::CameraState,
    cursor_position: Option<[f32; 2]>,
) -> Result<bool, String> {
    let Some(cursor) = cursor_position else {
        return Ok(false);
    };
    let Some(cell) = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    ) else {
        return Ok(false);
    };

    let removal_index = {
        let Some(plan) = level_context.component(current_component_id) else {
            return Ok(false);
        };
        plan.child_instances.iter().position(|instance| {
            let Some(child) = level_context.component(instance.component_id) else {
                return false;
            };
            let x_range = instance.origin[0]..instance.origin[0] + child.outside_shape.width;
            let y_range = instance.origin[1]..instance.origin[1] + child.outside_shape.height;
            x_range.contains(&cell.x) && y_range.contains(&cell.y)
        })
    };

    let Some(removal_index) = removal_index else {
        return Ok(false);
    };

    let plan = level_context
        .component_mut(current_component_id)
        .ok_or_else(|| format!("Missing edited component {:?}", current_component_id))?;
    plan.child_instances.remove(removal_index);
    plan.sync_child_links();

    restore_edit_component(
        level_context,
        current_component_id,
        edit_board,
        edited_component,
        wire_overlay,
        displayed_arena_z,
        device,
        queue,
    )?;
    editor_session.reset_for_loaded_schematic(wire_overlay, device, queue);
    Ok(true)
}

fn cursor_hits_child_instance(
    level_context: &level_context::LevelContext,
    component_id: component_plan::ComponentId,
    camera: render::CameraState,
    cursor_position: Option<[f32; 2]>,
) -> bool {
    let Some(cursor) = cursor_position else {
        return false;
    };
    let Some(cell) = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    ) else {
        return false;
    };
    let Some(component) = level_context.component(component_id) else {
        return false;
    };
    component.child_instances.iter().any(|instance| {
        let Some(child) = level_context.component(instance.component_id) else {
            return false;
        };
        let x_range = instance.origin[0]..instance.origin[0] + child.outside_shape.width;
        let y_range = instance.origin[1]..instance.origin[1] + child.outside_shape.height;
        x_range.contains(&cell.x) && y_range.contains(&cell.y)
    })
}

fn blocks_builtin_left_click_over_child_instance(
    selection: editor::EditorPlacementSelection,
) -> bool {
    matches!(
        selection,
        editor::EditorPlacementSelection::BuiltIn(tool) if tool != editor::EditorTool::Wire
    )
}

async fn render_scene_to_png(options: &RenderSceneOptions) -> Result<(), String> {
    let windowing::GpuState { device, queue, .. } = windowing::prepare_gpu(None).await?;

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
        surface,
        gpu:
            windowing::GpuState {
                adapter,
                device,
                queue,
                ..
            },
    } = windowing::prepare_window().await;
    let mut config = windowing::configure_surface(&surface, &adapter, &device, window.inner_size());

    let edit_board = simulation::BoardTextures::new(&device, &queue);
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
    let mut level_context = level_context::LevelContext::with_starter_root();
    let mut root_component_id = level_context.root_component_id();
    let mut edited_component_id = root_component_id;
    let mut edited_component = level_context
        .upload_component_to_board(edited_component_id, &edit_board, &device, &queue)
        .expect("starter context should upload to board");
    sync_wire_overlay(
        &mut wire_overlay,
        &edited_component,
        0,
        &mut displayed_arena_z,
        &device,
        &queue,
    );

    let mut app_mode = AppMode::Edit;
    let mut pressed_keys = HashSet::new();
    let mut cursor_position = None;

    window.request_redraw();

    event_loop
        .run(|event, target| match event {
            Event::Resumed => window.request_redraw(),
            Event::AboutToWait => window.request_redraw(),

            Event::WindowEvent { event, .. } => {
                let egui_response = egui_state.on_window_event(&window, &event);

                match event {
                    WindowEvent::RedrawRequested => {
                        let raw_input = egui_state.take_egui_input(&window);
                        let editor_mode = match app_mode {
                            AppMode::Edit => editor::EditorMode::Edit,
                            AppMode::Run { .. } => editor::EditorMode::Run,
                        };
                        let mut panel_output = editor::EditorFrameOutput {
                            reset_camera: false,
                            action: None,
                        };
                        let component_entries = editor_component_entries(&level_context);
                        let child_overlays = editor_child_overlays(&level_context, edited_component_id);
                        let edited_component_name = level_context
                            .component(edited_component_id)
                            .map(|component| component.name.clone())
                            .unwrap_or_else(|| format!("Component {}", edited_component_id.0));
                        let full_output = egui_ctx.run(raw_input, |ctx| {
                            panel_output = editor_session.show(
                                ctx,
                                camera,
                                cursor_position,
                                displayed_arena_z,
                                editor_mode,
                                edited_component_id,
                                &edited_component_name,
                                &component_entries,
                                &child_overlays,
                            );
                        });
                        if matches!(app_mode, AppMode::Edit) {
                            editor_session.sync_tool_state(&mut wire_overlay, &device, &queue);
                        }

                        match panel_output.action {
                            Some(editor::EditorPanelAction::StartRunning) => {
                                match start_run_mode_from_edit(
                                    &mut level_context,
                                    edited_component_id,
                                    &edit_board,
                                    &edited_component,
                                    &mut wire_overlay,
                                    &mut displayed_arena_z,
                                    &device,
                                    &queue,
                                ) {
                                    Ok(next_mode) => {
                                        app_mode = next_mode;
                                    }
                                    Err(error) => {
                                        eprintln!("{error}");
                                    }
                                }
                            }
                            Some(editor::EditorPanelAction::RestartRunning) => {
                                if let AppMode::Run { .. } = app_mode {
                                    match build_run_mode(
                                        &level_context,
                                        root_component_id,
                                        &mut wire_overlay,
                                        &mut displayed_arena_z,
                                        &device,
                                        &queue,
                                        "Failed to rebuild runtime",
                                    ) {
                                        Ok(next_mode) => {
                                            app_mode = next_mode;
                                        }
                                        Err(error) => {
                                            eprintln!("{error}");
                                        }
                                    }
                                }
                            }
                            Some(editor::EditorPanelAction::BackToEdit) => {
                                if let AppMode::Run { .. } = app_mode {
                                    match restore_edit_component(
                                        &level_context,
                                        edited_component_id,
                                        &edit_board,
                                        &mut edited_component,
                                        &mut wire_overlay,
                                        &mut displayed_arena_z,
                                        &device,
                                        &queue,
                                    ) {
                                        Ok(()) => {
                                            app_mode = AppMode::Edit;
                                        }
                                        Err(error) => {
                                            eprintln!("{error}");
                                        }
                                    }
                                }
                            }
                            Some(editor::EditorPanelAction::SaveSchematic) => {
                                if matches!(app_mode, AppMode::Edit) {
                                    if let Err(error) = level_context.refresh_component_from_board(
                                        edited_component_id,
                                        &edit_board,
                                        &edited_component,
                                        &device,
                                        &queue,
                                    ) {
                                        eprintln!(
                                            "Failed to refresh plan before saving schematic: {error}"
                                        );
                                        return;
                                    }
                                }

                                if let Some(path) = choose_schematic_save_path() {
                                    if let Some(parent) = path.parent() {
                                        if let Err(error) = std::fs::create_dir_all(parent) {
                                            eprintln!(
                                                "Failed to create schematic directory {}: {error}",
                                                parent.display()
                                            );
                                            return;
                                        }
                                    }

                                    if let Err(error) = level_context.save_to_path(&path) {
                                        eprintln!(
                                            "Failed to save schematic to {}: {error}",
                                            path.display()
                                        );
                                    }
                                }
                            }
                            Some(editor::EditorPanelAction::LoadSchematic) => {
                                if let Some(path) = choose_schematic_load_path() {
                                    match level_context::LevelContext::load_from_path(&path) {
                                        Ok(loaded_context) => {
                                            level_context = loaded_context;
                                            root_component_id = level_context.root_component_id();
                                            edited_component_id = root_component_id;
                                            editor_session.reset_for_loaded_schematic(
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            );
                                            match restore_edit_component(
                                                &level_context,
                                                edited_component_id,
                                                &edit_board,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &mut displayed_arena_z,
                                                &device,
                                                &queue,
                                            ) {
                                                Ok(()) => {
                                                    app_mode = AppMode::Edit;
                                                }
                                                Err(error) => {
                                                    eprintln!(
                                                        "Loaded schematic but failed to upload root component: {error}"
                                                    );
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            eprintln!(
                                                "Failed to load schematic from {}: {error}",
                                                path.display()
                                            );
                                        }
                                    }
                                }
                            }
                            Some(editor::EditorPanelAction::CreateComponent) => {
                                let new_component_id = level_context.create_component(
                                    format!("Component {}", level_context.components().count() + 1),
                                    [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                                );
                                match switch_edited_component(
                                    &mut level_context,
                                    edited_component_id,
                                    new_component_id,
                                    &edit_board,
                                    &mut edited_component,
                                    &mut editor_session,
                                    &mut wire_overlay,
                                    &mut displayed_arena_z,
                                    &device,
                                    &queue,
                                ) {
                                    Ok(()) => {
                                        edited_component_id = new_component_id;
                                    }
                                    Err(error) => eprintln!("{error}"),
                                }
                            }
                            Some(editor::EditorPanelAction::EditComponent(next_component_id)) => {
                                if next_component_id != edited_component_id {
                                    match switch_edited_component(
                                        &mut level_context,
                                        edited_component_id,
                                        next_component_id,
                                        &edit_board,
                                        &mut edited_component,
                                        &mut editor_session,
                                        &mut wire_overlay,
                                        &mut displayed_arena_z,
                                        &device,
                                        &queue,
                                    ) {
                                        Ok(()) => {
                                            edited_component_id = next_component_id;
                                        }
                                        Err(error) => eprintln!("{error}"),
                                    }
                                }
                            }
                            None => {}
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
                        editor_session.advance_visual_feedback();

                        if panel_output.reset_camera {
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

                        let display_camera = if matches!(app_mode, AppMode::Edit) {
                            editor_session.camera_with_feedback(camera)
                        } else {
                            camera
                        };

                        renderer.update_view_arena_z(&queue, display_camera, displayed_arena_z);
                        hover_preview.update(
                            &queue,
                            display_camera,
                            if matches!(app_mode, AppMode::Edit) {
                                editor_session.hover_preview_state(
                                    display_camera,
                                    cursor_position,
                                    !egui_ctx.is_pointer_over_area(),
                                )
                            } else {
                                None
                            },
                        );
                        wire_overlay.set_draft_color(
                            &device,
                            &queue,
                            editor_session.selected_wire_color(),
                        );
                        wire_overlay.update_camera(&device, &queue, display_camera);

                        let mut maybe_next_buffer = None;
                        let display_frame;

                        match &app_mode {
                            AppMode::Edit => {
                                display_frame = 0;
                            }
                            AppMode::Run {
                                current_buffer,
                                step_requested,
                                ..
                            } => {
                                let next_buffer =
                                    (current_buffer + 1) % simulation::CHARGE_BUFFER_COUNT;
                                maybe_next_buffer = Some(next_buffer);
                                display_frame = if *step_requested {
                                    next_buffer
                                } else {
                                    *current_buffer
                                };
                            }
                        }

                        let frame = match surface.get_current_texture() {
                            Ok(frame) => frame,
                            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                                windowing::reconfigure_surface(&surface, &device, &config);
                                return;
                            }
                            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::OutOfMemory) => {
                                return;
                            }
                            Err(wgpu::SurfaceError::Other) => return,
                        };

                        let mut encoder = device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                        if let AppMode::Run {
                            runtime,
                            current_buffer,
                            step_requested,
                        } = &app_mode
                        {
                            if *step_requested {
                                runtime.step(
                                    &device,
                                    &queue,
                                    *current_buffer,
                                    maybe_next_buffer
                                        .expect("run mode should have next buffer"),
                                );
                            }
                        }

                        egui_renderer.update_buffers(
                            &device,
                            &queue,
                            &mut encoder,
                            &paint_jobs,
                            &screen_descriptor,
                        );

                        match &app_mode {
                            AppMode::Edit => {
                                renderer.draw(
                                    &device,
                                    &mut encoder,
                                    &frame.texture,
                                    edit_board.charge_view(0),
                                    edit_board.circuit_view(),
                                );

                                wire_overlay.draw(
                                    &device,
                                    &mut encoder,
                                    &frame.texture,
                                    edit_board.charge_view(0),
                                    edit_board.charge_view(display_frame),
                                );
                            }
                            AppMode::Run {
                                runtime,
                                current_buffer,
                                ..
                            } => {
                                renderer.draw(
                                    &device,
                                    &mut encoder,
                                    &frame.texture,
                                    runtime.root.board.charge_view(display_frame),
                                    runtime.root.board.circuit_view(),
                                );

                                wire_overlay.draw(
                                    &device,
                                    &mut encoder,
                                    &frame.texture,
                                    runtime.root.board.charge_view(*current_buffer),
                                    runtime.root.board.charge_view(display_frame),
                                );
                            }
                        }

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

                        if let AppMode::Run {
                            current_buffer,
                            step_requested,
                            ..
                        } = &mut app_mode
                        {
                            if *step_requested {
                                *current_buffer =
                                    maybe_next_buffer.expect("run mode should have next buffer");
                                *step_requested = false;
                            }
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

                                    if matches!(app_mode, AppMode::Edit) {
                                        if !event.repeat && control_down && code == KeyCode::KeyZ {
                                            let changed = if shift_down {
                                                editor_session.redo(
                                                    &edit_board,
                                                    &mut edited_component,
                                                    &mut wire_overlay,
                                                    &device,
                                                    &queue,
                                                )
                                            } else {
                                                editor_session.undo(
                                                    &edit_board,
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
                                                &edit_board,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            ) {
                                                window.request_redraw();
                                            }
                                            return;
                                        }
                                    }

                                    if !event.repeat && code == KeyCode::Space {
                                        if let AppMode::Run { step_requested, .. } = &mut app_mode {
                                            *step_requested = true;
                                        } else if matches!(app_mode, AppMode::Edit) {
                                            match start_run_mode_from_edit(
                                                &mut level_context,
                                                edited_component_id,
                                                &edit_board,
                                                &edited_component,
                                                &mut wire_overlay,
                                                &mut displayed_arena_z,
                                                &device,
                                                &queue,
                                            ) {
                                                Ok(next_mode) => {
                                                    app_mode = next_mode;
                                                }
                                                Err(error) => {
                                                    eprintln!("{error}");
                                                }
                                            }
                                        }
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::ArrowUp {
                                        if matches!(app_mode, AppMode::Run { .. }) {
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
                                                    [
                                                        simulation::GRID_WIDTH,
                                                        simulation::GRID_HEIGHT,
                                                    ],
                                                )
                                            });
                                            wire_overlay.update_hover(&device, &queue, hover);
                                            window.request_redraw();
                                        }
                                    }

                                    if !event.repeat && code == KeyCode::ArrowDown {
                                        if matches!(app_mode, AppMode::Run { .. }) {
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
                                                    [
                                                        simulation::GRID_WIDTH,
                                                        simulation::GRID_HEIGHT,
                                                    ],
                                                )
                                            });
                                            wire_overlay.update_hover(&device, &queue, hover);
                                            window.request_redraw();
                                        }
                                    }

                                    if !event.repeat
                                        && code == KeyCode::Enter
                                        && matches!(app_mode, AppMode::Edit)
                                    {
                                        if editor_session.selected_placement()
                                            == editor::EditorPlacementSelection::BuiltIn(
                                                editor::EditorTool::Wire,
                                            )
                                        {
                                            if editor_session.finish_wire_attempt(
                                                &edit_board,
                                                &mut edited_component,
                                                &mut wire_overlay,
                                                &device,
                                                &queue,
                                            ) {}
                                            window.request_redraw();
                                        }
                                    }

                                    if !event.repeat
                                        && code == KeyCode::Escape
                                        && matches!(app_mode, AppMode::Edit)
                                    {
                                        editor_session.cancel_wire_draft(
                                            &mut wire_overlay,
                                            &device,
                                            &queue,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat
                                        && code == KeyCode::Backspace
                                        && matches!(app_mode, AppMode::Edit)
                                    {
                                        if editor_session.selected_placement()
                                            == editor::EditorPlacementSelection::BuiltIn(
                                                editor::EditorTool::Wire,
                                            )
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
                            if !matches!(app_mode, AppMode::Edit) {
                                return;
                            }
                            let extend_wire = pressed_keys.contains(&KeyCode::ShiftLeft)
                                || pressed_keys.contains(&KeyCode::ShiftRight);
                            let changed = match editor_session.selected_placement() {
                                editor::EditorPlacementSelection::ChildComponent(child_component_id) => {
                                    match place_child_instance_at_cursor(
                                        &mut level_context,
                                        edited_component_id,
                                        child_component_id,
                                        &edit_board,
                                        &mut edited_component,
                                        &mut editor_session,
                                        &mut wire_overlay,
                                        &mut displayed_arena_z,
                                        &device,
                                        &queue,
                                        camera,
                                        cursor,
                                    ) {
                                        Ok(changed) => changed,
                                        Err(error) => {
                                            eprintln!("{error}");
                                            false
                                        }
                                    }
                                }
                                editor::EditorPlacementSelection::BuiltIn(_) => {
                                    if blocks_builtin_left_click_over_child_instance(
                                        editor_session.selected_placement(),
                                    ) && cursor_hits_child_instance(
                                        &level_context,
                                        edited_component_id,
                                        camera,
                                        Some(cursor),
                                    ) {
                                        false
                                    } else {
                                        editor_session.handle_left_click(
                                            &edit_board,
                                            &mut edited_component,
                                            &mut wire_overlay,
                                            &device,
                                            &queue,
                                            camera,
                                            cursor,
                                            displayed_arena_z,
                                            extend_wire,
                                        )
                                    }
                                }
                            };
                            if changed {
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

                        if !matches!(app_mode, AppMode::Edit) {
                            window.request_redraw();
                            return;
                        }

                        let extend_wire = pressed_keys.contains(&KeyCode::ShiftLeft)
                            || pressed_keys.contains(&KeyCode::ShiftRight);
                        let deleted_child = match delete_child_instance_at_cursor(
                            &mut level_context,
                            edited_component_id,
                            &edit_board,
                            &mut edited_component,
                            &mut editor_session,
                            &mut wire_overlay,
                            &mut displayed_arena_z,
                            &device,
                            &queue,
                            camera,
                            cursor_position,
                        ) {
                            Ok(changed) => changed,
                            Err(error) => {
                                eprintln!("{error}");
                                false
                            }
                        };
                        if deleted_child
                            || editor_session.handle_right_click(
                                &edit_board,
                                &mut edited_component,
                                &mut wire_overlay,
                                &device,
                                &queue,
                                camera,
                                cursor_position,
                                displayed_arena_z,
                                extend_wire,
                            )
                        {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_tool_clicks_are_not_blocked_over_child_instances() {
        assert!(!blocks_builtin_left_click_over_child_instance(
            editor::EditorPlacementSelection::BuiltIn(editor::EditorTool::Wire)
        ));
    }

    #[test]
    fn non_wire_builtin_clicks_are_blocked_over_child_instances() {
        assert!(blocks_builtin_left_click_over_child_instance(
            editor::EditorPlacementSelection::BuiltIn(editor::EditorTool::Source)
        ));
        assert!(blocks_builtin_left_click_over_child_instance(
            editor::EditorPlacementSelection::BuiltIn(editor::EditorTool::And)
        ));
        assert!(!blocks_builtin_left_click_over_child_instance(
            editor::EditorPlacementSelection::ChildComponent(component_plan::ComponentId(1))
        ));
    }
}
