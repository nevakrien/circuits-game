pub const GRID_WIDTH: u32 = 8;
pub const GRID_HEIGHT: u32 = 8;
pub const FRAME_HISTORY: u32 = 4;

pub struct Simulation {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    _frames: Vec<wgpu::Texture>,
    frame_views: Vec<wgpu::TextureView>,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let frames: Vec<_> = (0..FRAME_HISTORY)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("simulation-frame"),
                    size: wgpu::Extent3d {
                        width: GRID_WIDTH,
                        height: GRID_HEIGHT,
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
                })
            })
            .collect();

        let frame_views: Vec<_> = frames
            .iter()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();

        seed_first_frame(queue, &frames[0]);

        let down_wire_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("down-wire"),
            source: wgpu::ShaderSource::Wgsl(include_str!("down_wire.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("simulation-bind-group-layout"),
            entries: &[
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
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("simulation-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("down-wire-pipeline"),
            layout: Some(&pipeline_layout),
            module: &down_wire_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            _frames: frames,
            frame_views,
        }
    }

    pub fn frame_view(&self, frame_index: u32) -> &wgpu::TextureView {
        &self.frame_views[frame_index as usize]
    }

    pub fn step(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        current_frame: u32,
        next_frame: u32,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("simulation-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.frame_view(current_frame)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(self.frame_view(next_frame)),
                },
            ],
        });

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(GRID_WIDTH.div_ceil(8), GRID_HEIGHT.div_ceil(8), 1);
    }
}

fn seed_first_frame(queue: &wgpu::Queue, texture: &wgpu::Texture) {
    let mut initial_state = vec![0u32; (GRID_WIDTH * GRID_HEIGHT) as usize];
    for x in 0..GRID_WIDTH {
        initial_state[x as usize] = 1;
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&initial_state),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(GRID_WIDTH * 4),
            rows_per_image: Some(GRID_HEIGHT),
        },
        wgpu::Extent3d {
            width: GRID_WIDTH,
            height: GRID_HEIGHT,
            depth_or_array_layers: 1,
        },
    );
}
