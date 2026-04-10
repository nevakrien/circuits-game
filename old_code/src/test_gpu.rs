use egui_wgpu::wgpu;
use std::sync::OnceLock;

pub struct GpuState {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

static GPU: OnceLock<GpuState> = OnceLock::new();

pub fn gpu() -> &'static GpuState {
    GPU.get_or_init(|| {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();

            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: None,
                    ..Default::default()
                })
                .await
                .expect("Failed to get adapter");

            let (device, queue) = adapter
                .request_device(&crate::simulation::device_descriptor(&adapter))
                .await
                .expect("Failed to create device");

            GpuState {
                instance,
                adapter,
                device,
                queue,
            }
        })
    })
}

use egui_wgpu::wgpu;
use egui_winit::winit;

use std::sync::Arc;

use winit::{
    dpi::PhysicalSize,
    event_loop::EventLoop,
    window::{Window, WindowAttributes},
};

/* =========================
   FINAL WINDOW STATE
   ========================= */

pub struct WindowState {
    pub event_loop: EventLoop<()>,
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

/* =========================
   PREPARE EVERYTHING
   ========================= */

#[allow(deprecated)]
pub fn prepare_window() -> WindowState {
    let gpu = crate::windowing::gpu(); // global GPU

    let event_loop = EventLoop::new().unwrap();

    let window = Arc::new(
        event_loop
            .create_window(WindowAttributes::default())
            .unwrap(),
    );

    let surface = gpu
        .instance
        .create_surface(window.clone())
        .expect("Failed to create surface");

    let size = window.inner_size();

    let caps = surface.get_capabilities(&gpu.adapter);

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

    surface.configure(&gpu.device, &config);

    WindowState {
        event_loop,
        window,
        surface,
        config,
    }
}