pub const GRID_WIDTH: u32 = 8;
pub const GRID_HEIGHT: u32 = 8;
pub const BOARD_LAYERS: u32 = 8;
pub const CHARGE_BUFFER_COUNT: u32 = 2;

const CHARGE_GRID_WIDTH: u32 = GRID_WIDTH.div_ceil(2);
const CHARGE_GRID_HEIGHT: u32 = GRID_HEIGHT.div_ceil(2);
const CHARGE_TEXEL_SIZE: u32 = 4;

const CHARGE_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Uint;
const CIRCUIT_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Uint;

#[repr(u8)]
#[derive(Clone, Copy)]
enum CircuitTag {
    Noop = 0,
    Source = 1,
    Wire = 2,
    Not = 3,
    And = 4,
    Or = 5,
    Xor = 6,
    Nand = 7,
    Nor = 8,
    Xnor = 9,
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

pub type PackedChargeTexels = Vec<[u8; 4]>;

pub fn packed_charge_texel_coord(x: u32, y: u32, z: u32) -> (u32, u32, u32) {
    (x / 2, y / 2, z)
}

pub fn packed_charge_channel(x: u32, y: u32) -> usize {
    ((y & 1) * 2 + (x & 1)) as usize
}

pub fn packed_charge_texel_index(x: u32, y: u32, z: u32) -> usize {
    let (packed_x, packed_y, packed_z) = packed_charge_texel_coord(x, y, z);
    (packed_z * CHARGE_GRID_WIDTH * CHARGE_GRID_HEIGHT + packed_y * CHARGE_GRID_WIDTH + packed_x)
        as usize
}

pub fn read_packed_charge(texels: &[[u8; 4]], x: u32, y: u32, z: u32) -> u8 {
    texels[packed_charge_texel_index(x, y, z)][packed_charge_channel(x, y)]
}

pub fn write_packed_charge(texels: &mut [[u8; 4]], x: u32, y: u32, z: u32, value: u8) {
    texels[packed_charge_texel_index(x, y, z)][packed_charge_channel(x, y)] = value;
}

fn charge_readback_bytes_per_row() -> u32 {
    let unpadded = CHARGE_GRID_WIDTH * CHARGE_TEXEL_SIZE;
    unpadded.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

impl Simulation {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let charge_textures: Vec<_> = (0..CHARGE_BUFFER_COUNT)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("simulation-charge"),
                    size: wgpu::Extent3d {
                        width: CHARGE_GRID_WIDTH,
                        height: CHARGE_GRID_HEIGHT,
                        depth_or_array_layers: BOARD_LAYERS,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: CHARGE_TEXTURE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
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

        let basic_cell_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire"),
            source: wgpu::ShaderSource::Wgsl(include_str!("basic_cell.wgsl").into()),
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
            module: &basic_cell_shader,
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

    pub async fn read_charge_buffer(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer_index: u32,
    ) -> PackedChargeTexels {
        let bytes_per_row = charge_readback_bytes_per_row();
        let size =
            u64::from(bytes_per_row) * u64::from(CHARGE_GRID_HEIGHT) * u64::from(BOARD_LAYERS);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("simulation-charge-readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("simulation-charge-readback"),
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self._charge_textures[buffer_index as usize],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(CHARGE_GRID_HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: CHARGE_GRID_WIDTH,
                height: CHARGE_GRID_HEIGHT,
                depth_or_array_layers: BOARD_LAYERS,
            },
        );

        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        receiver.recv().unwrap().unwrap();

        let mapped = slice.get_mapped_range();
        let mut texels =
            vec![[0u8; 4]; (CHARGE_GRID_WIDTH * CHARGE_GRID_HEIGHT * BOARD_LAYERS) as usize];
        for z in 0..BOARD_LAYERS as usize {
            for y in 0..CHARGE_GRID_HEIGHT as usize {
                let src_row_offset = z * CHARGE_GRID_HEIGHT as usize * bytes_per_row as usize
                    + y * bytes_per_row as usize;
                let dst_row_offset = z * CHARGE_GRID_HEIGHT as usize * CHARGE_GRID_WIDTH as usize
                    + y * CHARGE_GRID_WIDTH as usize;
                let src_row = &mapped[src_row_offset
                    ..src_row_offset + CHARGE_GRID_WIDTH as usize * CHARGE_TEXEL_SIZE as usize];
                for x in 0..CHARGE_GRID_WIDTH as usize {
                    let src = x * CHARGE_TEXEL_SIZE as usize;
                    texels[dst_row_offset + x]
                        .copy_from_slice(&src_row[src..src + CHARGE_TEXEL_SIZE as usize]);
                }
            }
        }
        drop(mapped);
        readback.unmap();

        texels
    }

    pub async fn read_charge_value(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer_index: u32,
        x: u32,
        y: u32,
        z: u32,
    ) -> u8 {
        let texels = self.read_charge_buffer(device, queue, buffer_index).await;
        read_packed_charge(&texels, x, y, z)
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
            CHARGE_GRID_WIDTH.div_ceil(8),
            CHARGE_GRID_HEIGHT.div_ceil(8),
            BOARD_LAYERS,
        );
    }
}

fn seed_initial_charge(queue: &wgpu::Queue, texture: &wgpu::Texture) {
    let initial_state =
        vec![[0u8; 4]; (CHARGE_GRID_WIDTH * CHARGE_GRID_HEIGHT * BOARD_LAYERS) as usize];

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
            bytes_per_row: Some(CHARGE_GRID_WIDTH * 4),
            rows_per_image: Some(CHARGE_GRID_HEIGHT),
        },
        wgpu::Extent3d {
            width: CHARGE_GRID_WIDTH,
            height: CHARGE_GRID_HEIGHT,
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
    if z == 0 {
        return primary_board_cell(x, y);
    }

    noop_cell()
}

fn encode_spatial_id(coord: (u32, u32, u32)) -> [u8; 3] {
    [coord.0 as u8, coord.1 as u8, coord.2 as u8]
}

fn noop_cell() -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Noop,
        data: [0, 0, 0],
    }
}

fn source_cell(value: u8) -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Source,
        data: [value, 0, 0],
    }
}

fn wire_cell(coord: (u32, u32, u32)) -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Wire,
        data: encode_spatial_id(coord),
    }
}

fn gate_cell(tag: CircuitTag) -> CircuitCell {
    CircuitCell {
        tag,
        data: [0, 0, 0],
    }
}

fn primary_board_cell(x: u32, y: u32) -> CircuitCell {
    match (x, y) {
        (1, 1) => source_cell(0xff),
        (3, 1) => source_cell(0x00),
        (5, 1) => source_cell(0xff),
        (6, 1) => source_cell(0xff),
        (1, 2) => wire_cell((1, 1, 0)),
        (3, 2) => wire_cell((3, 1, 0)),
        (5, 2) => wire_cell((5, 1, 0)),
        (6, 2) => wire_cell((6, 1, 0)),
        (2, 3) => gate_cell(CircuitTag::And),
        (4, 3) => gate_cell(CircuitTag::Or),
        (6, 3) => gate_cell(CircuitTag::Not),
        (2, 4) => wire_cell((2, 3, 0)),
        (4, 4) => wire_cell((4, 3, 0)),
        (6, 4) => wire_cell((6, 3, 0)),
        (3, 5) => gate_cell(CircuitTag::Xor),
        (5, 5) => gate_cell(CircuitTag::Nand),
        (3, 6) => wire_cell((3, 5, 0)),
        (4, 6) => wire_cell((3, 6, 0)),
        (5, 6) => wire_cell((5, 5, 0)),
        (6, 6) => wire_cell((5, 6, 0)),
        _ => noop_cell(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;

        adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()
    }

    fn bool_to_charge(value: bool) -> u8 {
        if value { 0xff } else { 0x00 }
    }

    fn gate_output(tag: CircuitTag, lhs: u8, rhs: u8) -> u8 {
        let lhs = lhs != 0;
        let rhs = rhs != 0;

        bool_to_charge(match tag {
            CircuitTag::Not => !rhs,
            CircuitTag::And => lhs && rhs,
            CircuitTag::Or => lhs || rhs,
            CircuitTag::Xor => lhs != rhs,
            CircuitTag::Nand => !(lhs && rhs),
            CircuitTag::Nor => !(lhs || rhs),
            CircuitTag::Xnor => lhs == rhs,
            _ => false,
        })
    }

    fn cpu_cell_next(texels: &PackedChargeTexels, x: u32, y: u32, z: u32) -> u8 {
        let cell = circuit_cell_for_coord(x, y, z);

        match cell.tag {
            CircuitTag::Noop => 0,
            CircuitTag::Source => cell.data[0],
            CircuitTag::Wire => {
                let [src_x, src_y, src_z] = cell.data;
                read_packed_charge(texels, src_x as u32, src_y as u32, src_z as u32)
            }
            CircuitTag::Not => {
                let input = read_packed_charge(texels, x, y - 1, z);
                gate_output(cell.tag, 0, input)
            }
            CircuitTag::And
            | CircuitTag::Or
            | CircuitTag::Xor
            | CircuitTag::Nand
            | CircuitTag::Nor
            | CircuitTag::Xnor => {
                let lhs = read_packed_charge(texels, x - 1, y - 1, z);
                let rhs = read_packed_charge(texels, x + 1, y - 1, z);
                gate_output(cell.tag, lhs, rhs)
            }
        }
    }

    fn expected_charge_after_steps(steps: u32, x: u32, y: u32, z: u32) -> u8 {
        let mut current =
            vec![[0u8; 4]; (CHARGE_GRID_WIDTH * CHARGE_GRID_HEIGHT * BOARD_LAYERS) as usize];

        for _ in 0..steps {
            let mut next = vec![[0u8; 4]; current.len()];
            for layer in 0..BOARD_LAYERS {
                for row in 0..GRID_HEIGHT {
                    for col in 0..GRID_WIDTH {
                        write_packed_charge(
                            &mut next,
                            col,
                            row,
                            layer,
                            cpu_cell_next(&current, col, row, layer),
                        );
                    }
                }
            }
            current = next;
        }

        read_packed_charge(&current, x, y, z)
    }

    #[test]
    fn gate_truth_tables_match_expected_logic() {
        let cases = [
            (CircuitTag::And, [false, false, false, true]),
            (CircuitTag::Or, [false, true, true, true]),
            (CircuitTag::Xor, [false, true, true, false]),
            (CircuitTag::Nand, [true, true, true, false]),
            (CircuitTag::Nor, [true, false, false, false]),
            (CircuitTag::Xnor, [true, false, false, true]),
        ];

        for (tag, expected) in cases {
            for (ix, (lhs, rhs)) in [(false, false), (false, true), (true, false), (true, true)]
                .into_iter()
                .enumerate()
            {
                assert_eq!(
                    gate_output(tag, bool_to_charge(lhs), bool_to_charge(rhs)) != 0,
                    expected[ix]
                );
            }
        }

        assert_eq!(gate_output(CircuitTag::Not, 0, 0) != 0, true);
        assert_eq!(gate_output(CircuitTag::Not, 0, 0xff) != 0, false);
    }

    #[test]
    fn packed_charge_cpu_helpers_match_layout() {
        let mut texels =
            vec![[0u8; 4]; (CHARGE_GRID_WIDTH * CHARGE_GRID_HEIGHT * BOARD_LAYERS) as usize];

        write_packed_charge(&mut texels, 0, 0, 0, 0x11);
        write_packed_charge(&mut texels, 1, 0, 0, 0x22);
        write_packed_charge(&mut texels, 0, 1, 0, 0x33);
        write_packed_charge(&mut texels, 1, 1, 0, 0x44);

        assert_eq!(
            texels[packed_charge_texel_index(0, 0, 0)],
            [0x11, 0x22, 0x33, 0x44]
        );
        assert_eq!(read_packed_charge(&texels, 0, 0, 0), 0x11);
        assert_eq!(read_packed_charge(&texels, 1, 0, 0), 0x22);
        assert_eq!(read_packed_charge(&texels, 0, 1, 0), 0x33);
        assert_eq!(read_packed_charge(&texels, 1, 1, 0), 0x44);
    }

    #[test]
    fn simulation_step_can_be_read_back_without_window() {
        let Some((device, queue)) = pollster::block_on(create_headless_device()) else {
            return;
        };

        let simulation = Simulation::new(&device, &queue);
        let mut current_buffer = 0;

        for step in 1..=GRID_HEIGHT {
            let next_buffer = (current_buffer + 1) % CHARGE_BUFFER_COUNT;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("simulation-test-step"),
            });
            simulation.step(&device, &mut encoder, current_buffer, next_buffer);
            queue.submit(Some(encoder.finish()));

            let texels =
                pollster::block_on(simulation.read_charge_buffer(&device, &queue, next_buffer));

            assert_eq!(
                pollster::block_on(simulation.read_charge_value(
                    &device,
                    &queue,
                    next_buffer,
                    0,
                    0,
                    0
                )),
                expected_charge_after_steps(step, 0, 0, 0)
            );

            for z in 0..BOARD_LAYERS {
                for y in 0..GRID_HEIGHT {
                    for x in 0..GRID_WIDTH {
                        assert_eq!(
                            read_packed_charge(&texels, x, y, z),
                            expected_charge_after_steps(step, x, y, z),
                            "mismatch at step={step}, coord=({x}, {y}, {z})"
                        );
                    }
                }
            }

            current_buffer = next_buffer;
        }
    }
}
