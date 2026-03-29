pub const GRID_WIDTH: u32 = 8;
pub const GRID_HEIGHT: u32 = 8;
pub const BOARD_LAYERS: u32 = 4;
pub const CHARGE_BUFFER_COUNT: u32 = 2;

const CHARGE_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Uint;
const CIRCUIT_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Uint;

#[repr(u8)]
#[derive(Clone, Copy)]
enum CircuitTag {
    Noop = 0,
    Source = 1,
    Wire = 2,
}

#[derive(Clone, Copy)]
struct CircuitCell {
    tag: CircuitTag,
    data: [u8; 3],
}

pub struct Simulation {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    _charge_textures: Vec<wgpu::Texture>,
    charge_views: Vec<wgpu::TextureView>,
    _circuit_texture: wgpu::Texture,
    circuit_view: wgpu::TextureView,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let charge_textures: Vec<_> = (0..CHARGE_BUFFER_COUNT)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("simulation-charge"),
                    size: wgpu::Extent3d {
                        width: GRID_WIDTH,
                        height: GRID_HEIGHT,
                        depth_or_array_layers: BOARD_LAYERS,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: CHARGE_TEXTURE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();

        let charge_views: Vec<_> = charge_textures
            .iter()
            .map(|texture| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D3),
                    ..Default::default()
                })
            })
            .collect();

        let circuit_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("simulation-circuits"),
            size: wgpu::Extent3d {
                width: GRID_WIDTH,
                height: GRID_HEIGHT,
                depth_or_array_layers: BOARD_LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: CIRCUIT_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let circuit_view = circuit_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });

        seed_circuits(queue, &circuit_texture);
        seed_initial_charge(queue, &charge_textures[0]);

        let down_wire_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire"),
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
                        view_dimension: wgpu::TextureViewDimension::D3,
                        sample_type: wgpu::TextureSampleType::Uint,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        sample_type: wgpu::TextureSampleType::Uint,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: CHARGE_TEXTURE_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D3,
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
            label: Some("wire-pipeline"),
            layout: Some(&pipeline_layout),
            module: &down_wire_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            _charge_textures: charge_textures,
            charge_views,
            _circuit_texture: circuit_texture,
            circuit_view,
        }
    }

    pub fn charge_view(&self, buffer_index: u32) -> &wgpu::TextureView {
        &self.charge_views[buffer_index as usize]
    }

    pub fn circuit_view(&self) -> &wgpu::TextureView {
        &self.circuit_view
    }

    pub fn step(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        current_buffer: u32,
        next_buffer: u32,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("simulation-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.charge_view(current_buffer)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.circuit_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(self.charge_view(next_buffer)),
                },
            ],
        });

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(
            GRID_WIDTH.div_ceil(8),
            GRID_HEIGHT.div_ceil(8),
            BOARD_LAYERS,
        );
    }
}

fn seed_initial_charge(queue: &wgpu::Queue, texture: &wgpu::Texture) {
    let initial_state = vec![[0u8; 4]; (GRID_WIDTH * GRID_HEIGHT * BOARD_LAYERS) as usize];

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
            depth_or_array_layers: BOARD_LAYERS,
        },
    );
}

fn seed_circuits(queue: &wgpu::Queue, texture: &wgpu::Texture) {
    let mut circuits = vec![[0u8; 4]; (GRID_WIDTH * GRID_HEIGHT * BOARD_LAYERS) as usize];

    for z in 0..BOARD_LAYERS {
        for y in 0..GRID_HEIGHT {
            for x in 0..GRID_WIDTH {
                let ix = linear_index(x, y, z);
                let cell = circuit_cell_for_coord(x, y, z);
                circuits[ix] = [cell.tag as u8, cell.data[0], cell.data[1], cell.data[2]];
            }
        }
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&circuits),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(GRID_WIDTH * 4),
            rows_per_image: Some(GRID_HEIGHT),
        },
        wgpu::Extent3d {
            width: GRID_WIDTH,
            height: GRID_HEIGHT,
            depth_or_array_layers: BOARD_LAYERS,
        },
    );
}

fn linear_index(x: u32, y: u32, z: u32) -> usize {
    (z * GRID_WIDTH * GRID_HEIGHT + y * GRID_WIDTH + x) as usize
}

fn circuit_cell_for_coord(x: u32, y: u32, z: u32) -> CircuitCell {
    if is_noop_cell(x, y, z) {
        CircuitCell {
            tag: CircuitTag::Noop,
            data: [0, 0, 0],
        }
    } else if is_source_cell(x, y, z) {
        CircuitCell {
            tag: CircuitTag::Source,
            data: [source_constant_for_layer(z), 0, 0],
        }
    } else {
        CircuitCell {
            tag: CircuitTag::Wire,
            data: encode_spatial_id(copy_source_for_cell(x, y, z)),
        }
    }
}

fn encode_spatial_id(coord: (u32, u32, u32)) -> [u8; 3] {
    [coord.0 as u8, coord.1 as u8, coord.2 as u8]
}

fn source_constant_for_layer(z: u32) -> u8 {
    match z {
        0 => 0xff,
        1 => 0xc0,
        2 => 0x80,
        3 => 0x40,
        _ => 0xff,
    }
}

fn copy_source_for_cell(x: u32, y: u32, z: u32) -> (u32, u32, u32) {
    if y == 0 {
        (x, y, z)
    } else {
        (x, y - 1, z)
    }
}

fn is_noop_cell(x: u32, y: u32, z: u32) -> bool {
    match z {
        1 => x == GRID_WIDTH / 2,
        2 => y == GRID_HEIGHT / 2,
        3 => x == y || x + y + 1 == GRID_WIDTH,
        _ => false,
    }
}

fn is_source_cell(x: u32, y: u32, z: u32) -> bool {
    if y != 0 {
        return false;
    }

    match z {
        0 => true,
        1 => x % 2 == 0,
        2 => x >= GRID_WIDTH / 2,
        3 => x == GRID_WIDTH / 2,
        _ => false,
    }
}
