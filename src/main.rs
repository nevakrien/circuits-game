use circuits_game::{render, simulation, windowing};

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

    let mut current_buffer = 0;
    let mut step_requested = false;
    let mut pressed_keys = HashSet::new();

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

                renderer.update_view(&queue, camera);

                let next_buffer = (current_buffer + 1) % simulation::CHARGE_BUFFER_COUNT;

                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,

                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        surface.configure(&device, &config);
                        return;
                    }

                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => return,

                    wgpu::CurrentSurfaceTexture::Validation => {
                        panic!("surface validation error");
                    }
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
                        }
                        ElementState::Released => {
                            pressed_keys.remove(&code);
                        }
                    }
                }
            }

            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                if size.width > 0 && size.height > 0 {
                    windowing::resize_surface(&surface, &device, &mut config, size);
                    camera.resize(size);
                    renderer.update_view(&queue, camera);
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
