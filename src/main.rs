mod render;
mod simulation;
mod windowing;
mod allocator;

use winit::{
    event::*,
    keyboard::{KeyCode, PhysicalKey},
};

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
    let renderer = render::Renderer::new(&device, config.format);

    let mut current_buffer = 0;
    let mut step_requested = false;

    event_loop
        .run(|event, target| match event {
            Event::AboutToWait => window.request_redraw(),

            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
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
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Space))
                {
                    step_requested = true;
                    window.request_redraw();
                }
            }

            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                if size.width > 0 && size.height > 0 {
                    windowing::resize_surface(&surface, &device, &mut config, size);
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
