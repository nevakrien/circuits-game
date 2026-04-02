use egui_wgpu::wgpu;
use egui_winit::winit;
use std::sync::Arc;

use winit::{
    dpi::PhysicalSize,
    event_loop::EventLoop,
    window::{Window, WindowAttributes},
};

pub struct GpuState {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub struct WindowState {
    pub event_loop: EventLoop<()>,
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub gpu: GpuState,
}

pub async fn prepare_gpu(compatible_surface: Option<&wgpu::Surface<'_>>) -> Result<GpuState, String> {
    prepare_gpu_with_instance(wgpu::Instance::default(), compatible_surface).await
}

pub async fn prepare_gpu_with_instance(
    instance: wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Result<GpuState, String> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface,
            ..Default::default()
        })
        .await
        .map_err(|error| format!("Unable to create adapter: {error}"))?;

    let (device, queue) = adapter
        .request_device(&crate::simulation::device_descriptor(&adapter))
        .await
        .map_err(|error| format!("Unable to create device: {error}"))?;

    Ok(GpuState {
        instance,
        adapter,
        device,
        queue,
    })
}

pub fn create_surface(
    instance: &wgpu::Instance,
    window: &Arc<Window>,
) -> Result<wgpu::Surface<'static>, wgpu::CreateSurfaceError> {
    instance.create_surface(window.clone())
}

#[allow(deprecated)]
pub async fn prepare_window() -> WindowState {
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(
        event_loop
            .create_window(WindowAttributes::default())
            .unwrap(),
    );
    let instance = wgpu::Instance::default();
    let surface = create_surface(&instance, &window).unwrap();
    let gpu = prepare_gpu_with_instance(instance, Some(&surface))
        .await
        .unwrap();

    WindowState {
        event_loop,
        window,
        surface,
        gpu,
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

pub fn reconfigure_surface(
    surface: &wgpu::Surface<'_>,
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) {
    surface.configure(device, config);
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
