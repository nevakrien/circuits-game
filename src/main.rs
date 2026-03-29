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

    let mut frame_index = 0;
    let mut step_requested = false;

    event_loop
        .run(|event, target| match event {
            Event::AboutToWait => window.request_redraw(),

            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
                let current_frame = frame_index % simulation::FRAME_HISTORY;
                let next_frame = (frame_index + 1) % simulation::FRAME_HISTORY;

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
                    next_frame
                } else {
                    current_frame
                };

                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                if step_requested {
                    simulation.step(&device, &mut encoder, current_frame, next_frame);
                }

                renderer.draw(
                    &device,
                    &mut encoder,
                    &frame.texture,
                    simulation.frame_view(display_frame),
                );

                queue.submit(Some(encoder.finish()));
                frame.present();

                if step_requested {
                    frame_index = next_frame;
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
