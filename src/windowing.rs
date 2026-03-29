use std::sync::Arc;

use winit::{dpi::PhysicalSize, event_loop::EventLoop, window::Window};

pub struct WindowState {
    pub event_loop: EventLoop<()>,
    pub window: Arc<Window>,
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub async fn prepare_window() -> WindowState {
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(Window::new(&event_loop).unwrap());

    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        })
        .await
        .unwrap();

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .unwrap();

    WindowState {
        event_loop,
        window,
        instance,
        adapter,
        device,
        queue,
    }
}

pub fn configure_surface(
    surface: &wgpu::Surface<'_>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    size: PhysicalSize<u32>,
) -> wgpu::SurfaceConfiguration {
    let caps = surface.get_capabilities(adapter);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: caps.formats[0],
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };

    surface.configure(device, &config);
    config
}

pub fn resize_surface(
    surface: &wgpu::Surface<'_>,
    device: &wgpu::Device,
    config: &mut wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
) {
    config.width = size.width;
    config.height = size.height;
    surface.configure(device, config);
}
