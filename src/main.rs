use egui_wgpu::wgpu;
use egui_winit::winit;
use circuits_game::{render, simulation, windowing, wires};

use winit::{
    event::*,
    keyboard::{KeyCode, PhysicalKey},
};

use std::collections::HashSet;

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

    let mut current_buffer = 0;
    let mut step_requested = false;
    let mut pressed_keys = HashSet::new();
    let mut cursor_position = None;

    event_loop
        .run(|event, target| match event {
            Event::AboutToWait => window.request_redraw(),

            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
                let pan_step = 0.01 / camera.zoom;
                let zoom_step = 1.02;

                if pressed_keys.contains(&KeyCode::KeyW) {
                    camera.pan_by([0.0, -pan_step]);
                }
                if pressed_keys.contains(&KeyCode::KeyS) {
                    camera.pan_by([0.0, pan_step]);
                }
                if pressed_keys.contains(&KeyCode::KeyA) {
                    camera.pan_by([-pan_step, 0.0]);
                }
                if pressed_keys.contains(&KeyCode::KeyD) {
                    camera.pan_by([pan_step, 0.0]);
                }
                if pressed_keys.contains(&KeyCode::KeyQ) {
                    camera.zoom_by(zoom_step);
                }
                if pressed_keys.contains(&KeyCode::KeyE) {
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
                    Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::OutOfMemory) => return,
                    Err(wgpu::SurfaceError::Other) => return,
                };

                let display_frame = if step_requested {
                    next_buffer
                } else {
                    current_buffer
                };

                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                if step_requested {
                    simulation.step(&device, &mut encoder, current_buffer, next_buffer);
                }

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

                queue.submit(Some(encoder.finish()));
                frame.present();

                if step_requested {
                    current_buffer = next_buffer;
                    step_requested = false;
                }
            }

            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            pressed_keys.insert(code);

                            if !event.repeat && code == KeyCode::Space {
                                step_requested = true;
                                window.request_redraw();
                            }

                            if !event.repeat && code == KeyCode::ArrowUp {
                                displayed_layer = (displayed_layer + 1)
                                    .min(simulation::BOARD_LAYERS.saturating_sub(1));
                                let hover = cursor_position.and_then(|cursor| {
                                    wires::cursor_to_board_point(
                                        camera,
                                        cursor,
                                        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                                    )
                                });
                                wire_overlay.update_hover(&device, &queue, displayed_layer, hover);
                                window.request_redraw();
                            }

                            if !event.repeat && code == KeyCode::ArrowDown {
                                displayed_layer = displayed_layer.saturating_sub(1);
                                let hover = cursor_position.and_then(|cursor| {
                                    wires::cursor_to_board_point(
                                        camera,
                                        cursor,
                                        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                                    )
                                });
                                wire_overlay.update_hover(&device, &queue, displayed_layer, hover);
                                window.request_redraw();
                            }

                            if !event.repeat && code == KeyCode::Enter {
                                if wire_overlay.commit_draft(&device, &queue) {
                                    window.request_redraw();
                                }
                            }

                            if !event.repeat && code == KeyCode::Escape {
                                wire_overlay.cancel_draft(&device, &queue);
                                window.request_redraw();
                            }

                            if !event.repeat && code == KeyCode::Backspace {
                                wire_overlay.pop_point(&device, &queue);
                                window.request_redraw();
                            }
                        }
                        ElementState::Released => {
                            pressed_keys.remove(&code);
                        }
                    }
                }
            }

            Event::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => {
                let cursor = [position.x as f32, position.y as f32];
                cursor_position = Some(cursor);
                let hover = wires::cursor_to_board_point(
                    camera,
                    cursor,
                    [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                );
                wire_overlay.update_hover(&device, &queue, displayed_layer, hover);
                window.request_redraw();
            }

            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    },
                ..
            } => {
                if let Some(cursor) = cursor_position {
                    if let (Some(point), Some(source)) = (
                        wires::cursor_to_board_point(
                            camera,
                            cursor,
                            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                        ),
                        wires::snap_cursor_to_cell(
                            camera,
                            cursor,
                            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                        ),
                    ) {
                        wire_overlay.add_point(&device, &queue, displayed_layer, point, source);
                        window.request_redraw();
                    }
                }
            }

            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Right,
                        ..
                    },
                ..
            } => {
                wire_overlay.pop_point(&device, &queue);
                window.request_redraw();
            }

            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                if size.width > 0 && size.height > 0 {
                    windowing::resize_surface(&surface, &device, &mut config, size);
                    camera.resize(size);
                    renderer.update_view_layer(&queue, camera, displayed_layer);
                    wire_overlay.resize(&queue, camera, size);
                }
            }

            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => target.exit(),

            _ => {}
        })
        .unwrap();
}
