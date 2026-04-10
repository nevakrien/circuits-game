use egui_wgpu::wgpu;
use egui_winit::winit;

use std::sync::{Arc, OnceLock};

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
                .expect("Failed to create adapter");

            let limits = adapter.limits();

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("global device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                    memory_hints: wgpu::MemoryHints::Performance,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    trace: wgpu::Trace::Off,
                })
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

pub struct WindowState {
    pub event_loop: EventLoop<()>,
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl WindowState {
    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn window_arc(&self) -> Arc<Window> {
        self.window.clone()
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        self.window.inner_size()
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.config.width = new_size.width.max(1);
        self.config.height = new_size.height.max(1);
        self.surface.configure(&gpu().device, &self.config);
    }
}

#[allow(deprecated)]
pub fn prepare_window() -> WindowState {
    let event_loop = EventLoop::new().unwrap();

    let window = Arc::new(
        event_loop
            .create_window(WindowAttributes::default())
            .unwrap(),
    );

    let gpu = gpu();

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

pub fn device() -> &'static wgpu::Device {
    &gpu().device
}

pub fn queue() -> &'static wgpu::Queue {
    &gpu().queue
}

pub fn adapter() -> &'static wgpu::Adapter {
    &gpu().adapter
}

pub fn instance() -> &'static wgpu::Instance {
    &gpu().instance
}
