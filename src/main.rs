use bytemuck::{Pod, Zeroable};
use winit::{
    event::*,
    event_loop::EventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::Window,
};

const WIDTH: u32 = 8;
const HEIGHT: u32 = 8;
const F: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    _pad: [u32; 4],
}

fn main() {
    pollster::block_on(run());
}

async fn run() {
    // ---------- window ----------
    let event_loop = EventLoop::new().unwrap();
    let window = Window::new(&event_loop).unwrap();

    // ---------- wgpu ----------
    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(&window).unwrap();

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .unwrap();

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .unwrap();

    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats[0];

    let size = window.inner_size();

    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };

    surface.configure(&device, &config);

    // ---------- textures ----------
    let textures: Vec<_> = (0..F)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                size: wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
                label: None,
            })
        })
        .collect();

    let views: Vec<_> = textures
        .iter()
        .map(|t| t.create_view(&Default::default()))
        .collect();

    // seed first texture
    let mut init = vec![0u32; (WIDTH * HEIGHT) as usize];
    for x in 0..WIDTH {
        init[(0 * WIDTH + x) as usize] = 1;
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &textures[0],
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&init),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 4),
            rows_per_image: Some(HEIGHT),
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );

    // ---------- compute ----------
    let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        label: None,
    });

    let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        entries: &[
            // read
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Uint,
                },
                count: None,
            },
            // write
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::R32Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
        ],
        label: None,
    });

    let compute_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[Some(&compute_bgl)],
            immediate_size: 0,
            label: None,
        });

    let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        layout: Some(&compute_pipeline_layout),
        module: &compute_shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
        label: None,
    });

    // ---------- render ----------
    let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        source: wgpu::ShaderSource::Wgsl(include_str!("render.wgsl").into()),
        label: None,
    });

    let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Uint,
            },
            count: None,
        }],
        label: None,
    });

    let render_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[Some(&render_bgl)],
            immediate_size: 0,
            label: None,
        });

    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        layout: Some(&render_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &render_shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &render_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
        label: None,
    });

    let mut frame_count = 0;
    let mut step_requested = false;

    event_loop.run(|event, target| {
        match event {
            Event::AboutToWait => window.request_redraw(),

            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
                let curr = frame_count % F;
                let next = (frame_count + 1) % F;

                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,

                    wgpu::CurrentSurfaceTexture::Outdated
                    | wgpu::CurrentSurfaceTexture::Lost => {
                        surface.configure(&device, &config);
                        return; // skip this frame
                    }

                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => {
                        return; // nothing to render this frame
                    }

                    wgpu::CurrentSurfaceTexture::Validation => {
                        panic!("surface validation error");
                    }
                };

                let view = frame
                    .texture.create_view(&wgpu::TextureViewDescriptor::default());

                let display_idx = if step_requested { next } else { curr };

                let render_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &render_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &views[display_idx as usize],
                        ),
                    }],
                    label: None,
                });

                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                // compute
                if step_requested {
                    let compute_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &compute_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(
                                    &views[curr as usize],
                                ),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(
                                    &views[next as usize],
                                ),
                            },
                        ],
                        label: None,
                    });

                    let mut pass =
                        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                    pass.set_pipeline(&compute_pipeline);
                    pass.set_bind_group(0, &compute_bg, &[]);
                    pass.dispatch_workgroups(WIDTH.div_ceil(8), HEIGHT.div_ceil(8), 1);
                }

                // render
                {
                    let mut rpass =
                        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(
                                wgpu::RenderPassColorAttachment {
                                    view: &view,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(
                                            wgpu::Color::BLACK,
                                        ),
                                        store: wgpu::StoreOp::Store,
                                    },
                                    depth_slice: None,
                                },
                            )],
                            ..Default::default()
                        });

                    rpass.set_pipeline(&render_pipeline);
                    rpass.set_bind_group(0, &render_bg, &[]);
                    rpass.draw(0..3, 0..1);
                }

                queue.submit(Some(encoder.finish()));
                frame.present();

                if step_requested {
                    frame_count = next;
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
                    config.width = size.width;
                    config.height = size.height;
                    surface.configure(&device, &config);
                }
            }

            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => target.exit(),

            _ => {}
        }
    }).unwrap();
}
