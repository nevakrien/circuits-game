use circuits_game::{components, editor, render, simulation, windowing, wires};
use egui_wgpu::wgpu;
use egui_winit::winit;

use winit::{
    event::*,
    keyboard::{KeyCode, PhysicalKey},
};

use std::collections::HashSet;

#[derive(Clone)]
enum EditorAction {
    AddWire(components::StoredWireEdge),
    DeleteWire(components::StoredWireEdge),
    DeleteCell {
        grid_cell: wires::GridCell,
        layer: u32,
        cell: simulation::CellSnapshot,
        charge_values: Vec<u8>,
    },
}

#[derive(Default)]
struct EditorHistory {
    undo_stack: Vec<EditorAction>,
    redo_stack: Vec<EditorAction>,
}

impl EditorHistory {
    fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    fn push(&mut self, action: EditorAction) {
        self.undo_stack.push(action);
        self.redo_stack.clear();
    }

    fn undo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut components::ComponentInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(action) = self.undo_stack.pop() else {
            return false;
        };
        apply_inverse_action(&action, simulation, component, wire_overlay, device, queue);
        self.redo_stack.push(action);
        true
    }

    fn redo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut components::ComponentInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(action) = self.redo_stack.pop() else {
            return false;
        };
        apply_action(&action, simulation, component, wire_overlay, device, queue);
        self.undo_stack.push(action);
        true
    }
}

fn main() {
    pollster::block_on(run());
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
    let mut editor_ui = editor::EditorUi::new();
    let mut history = EditorHistory::default();
    let mut edited_component = components::ComponentInfo::new(components::ComponentBufferId {
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
                            reset_camera = editor_ui.show(
                                ctx,
                                displayed_layer,
                                history.can_undo(),
                                history.can_redo(),
                            );
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
                            edited_component.set_buffer_id(components::ComponentBufferId {
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
                                        edited_component.set_buffer_id(
                                            components::ComponentBufferId {
                                                texture_index: current_buffer,
                                                layer: displayed_layer,
                                            },
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
                                            displayed_layer,
                                            hover,
                                        );
                                        window.request_redraw();
                                    }

                                    if !event.repeat && code == KeyCode::ArrowDown {
                                        displayed_layer = displayed_layer.saturating_sub(1);
                                        edited_component.set_buffer_id(
                                            components::ComponentBufferId {
                                                texture_index: current_buffer,
                                                layer: displayed_layer,
                                            },
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
                                    _ => {}
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
    component: &components::ComponentInfo,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    wire_overlay.replace_wires(device, queue, component.wire_edges().cloned().collect());
}

fn finish_wire_attempt(
    wire_overlay: &mut wires::WireOverlay,
    component: &mut components::ComponentInfo,
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
    component: &mut components::ComponentInfo,
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

fn apply_action(
    action: &EditorAction,
    simulation: &simulation::Simulation,
    component: &mut components::ComponentInfo,
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
    component: &mut components::ComponentInfo,
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
