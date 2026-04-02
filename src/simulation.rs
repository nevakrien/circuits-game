use egui_wgpu::wgpu;

use crate::{demo_scene, wires::GridCell};

pub const GRID_WIDTH: u32 = 8;
pub const GRID_HEIGHT: u32 = 8;
pub const BOARD_LAYERS: u32 = 8;
pub const CHARGE_BUFFER_COUNT: u32 = 2;
// Input/output state stays in host-shareable `u32` storage buffers.
//
// One board slot maps to one 32-bit lane:
//   index = z * (GRID_WIDTH * GRID_HEIGHT) + y * GRID_WIDTH + x
//
// Today we store charge-like values (0x00/0xff) in the low byte and mask writes/reads
// with `& 0xff` in both CPU and WGSL paths. Keeping full-width `u32` lanes gives us
// room to widen semantics later without changing buffer indexing/plumbing.
pub const OUTPUT_BUFFER_LEN: u32 = GRID_WIDTH * GRID_HEIGHT * BOARD_LAYERS;
pub const INPUT_BUFFER_LEN: u32 = OUTPUT_BUFFER_LEN;

const CHARGE_GRID_WIDTH: u32 = GRID_WIDTH.div_ceil(2);
const CHARGE_GRID_HEIGHT: u32 = GRID_HEIGHT.div_ceil(2);
const CHARGE_TEXEL_SIZE: u32 = 4;

const CHARGE_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Uint;
const CIRCUIT_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
const CIRCUIT_WORDS_PER_CELL: u32 = 4;
pub const INVALID_INPUT_REF: u32 = u32::MAX;

#[repr(u8)]
#[derive(Clone, Copy)]
enum CircuitTag {
    Empty = 0,
    Source = 1,
    Noop = 2,
    Not = 3,
    And = 4,
    Or = 5,
    Xor = 6,
    Nand = 7,
    Nor = 8,
    Xnor = 9,
    Output = 10,
    Input = 11,
}

#[derive(Clone, Copy)]
struct CircuitCell {
    tag: CircuitTag,
    data: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellSnapshot {
    // One circuit cell is encoded into one RGBA texel of u32 words.
    pub words: [u32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateKind {
    Not,
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellKind {
    Empty,
    Source,
    Noop,
    Not,
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
    Output,
    Input,
}

pub struct BoardTextures {
    charge_buffers: Vec<(wgpu::Texture, wgpu::TextureView)>,
    circuit: (wgpu::Texture, wgpu::TextureView),
    input_write_buffer: wgpu::Buffer,
    input_read_buffer: wgpu::Buffer,
    output_buffer: wgpu::Buffer,
}

pub struct Simulation {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

// Charge is packed 2x2 cells per RGBA texel, so one `[u8; 4]` stores four cell charges.
pub type PackedChargeTexels = Vec<[u8; 4]>;

pub fn board_cell_count() -> usize {
    (GRID_WIDTH * GRID_HEIGHT * BOARD_LAYERS) as usize
}

pub fn packed_charge_texel_count() -> usize {
    (CHARGE_GRID_WIDTH * CHARGE_GRID_HEIGHT * BOARD_LAYERS) as usize
}

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

fn output_buffer_size() -> u64 {
    u64::from(OUTPUT_BUFFER_LEN) * std::mem::size_of::<u32>() as u64
}

fn input_buffer_size() -> u64 {
    u64::from(INPUT_BUFFER_LEN) * std::mem::size_of::<u32>() as u64
}

pub fn output_slot_index(x: u32, y: u32, z: u32) -> usize {
    linear_index(x, y, z)
}

pub fn input_slot_index(x: u32, y: u32, z: u32) -> usize {
    linear_index(x, y, z)
}

pub fn device_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    adapter.features() & wgpu::Features::MAPPABLE_PRIMARY_BUFFERS
}

pub fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor {
        label: Some("circuits-game-device"),
        required_features: device_features(adapter),
        ..Default::default()
    }
}

impl BoardTextures {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let charge_buffers: Vec<_> = (0..CHARGE_BUFFER_COUNT)
            .map(|_| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("board-charge"),
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
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D3),
                    ..Default::default()
                });

                (texture, view)
            })
            .collect();

        let circuit_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("board-circuits"),
            size: wgpu::Extent3d {
                width: GRID_WIDTH,
                height: GRID_HEIGHT,
                depth_or_array_layers: BOARD_LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: CIRCUIT_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let circuit_view = circuit_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });

        let input_write_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("input-buffer-write"),
            size: input_buffer_size(),
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let input_read_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("input-buffer-read"),
            size: input_buffer_size(),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output-buffer"),
            size: output_buffer_size(),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        seed_circuits(queue, &circuit_texture);
        seed_initial_charge(queue, &charge_buffers[0].0);
        queue.write_buffer(
            &input_write_buffer,
            0,
            bytemuck::cast_slice(&vec![0u32; INPUT_BUFFER_LEN as usize]),
        );
        queue.write_buffer(
            &input_read_buffer,
            0,
            bytemuck::cast_slice(&vec![0u32; INPUT_BUFFER_LEN as usize]),
        );
        queue.write_buffer(
            &output_buffer,
            0,
            bytemuck::cast_slice(&vec![0u32; OUTPUT_BUFFER_LEN as usize]),
        );

        Self {
            charge_buffers,
            circuit: (circuit_texture, circuit_view),
            input_write_buffer,
            input_read_buffer,
            output_buffer,
        }
    }

    pub fn charge_view(&self, buffer_index: u32) -> &wgpu::TextureView {
        &self.charge_buffers[buffer_index as usize].1
    }

    pub fn circuit_view(&self) -> &wgpu::TextureView {
        &self.circuit.1
    }

    pub fn output_buffer(&self) -> &wgpu::Buffer {
        &self.output_buffer
    }

    pub fn input_write_buffer(&self) -> &wgpu::Buffer {
        &self.input_write_buffer
    }

    pub fn input_read_buffer(&self) -> &wgpu::Buffer {
        &self.input_read_buffer
    }

    pub fn write_input_value(&self, queue: &wgpu::Queue, x: u32, y: u32, z: u32, value: u32) {
        let offset = (output_slot_index(x, y, z) * std::mem::size_of::<u32>()) as u64;
        queue.write_buffer(
            &self.input_write_buffer,
            offset,
            bytemuck::cast_slice(&[value & 0xff]),
        );
    }

    pub fn write_input_buffer(&self, queue: &wgpu::Queue, values: &[u32]) {
        let mut packed = vec![0u32; INPUT_BUFFER_LEN as usize];
        for (ix, value) in values
            .iter()
            .copied()
            .take(INPUT_BUFFER_LEN as usize)
            .enumerate()
        {
            packed[ix] = value & 0xff;
        }
        queue.write_buffer(&self.input_write_buffer, 0, bytemuck::cast_slice(&packed));
    }

    pub fn write_input_range(&self, queue: &wgpu::Queue, start_index: u32, values: &[u32]) {
        if values.is_empty() {
            return;
        }

        let start = start_index as usize;
        let end = start + values.len();
        assert!(
            end <= INPUT_BUFFER_LEN as usize,
            "input range out of bounds"
        );

        let packed: Vec<u32> = values.iter().map(|value| value & 0xff).collect();
        queue.write_buffer(
            &self.input_write_buffer,
            (start * std::mem::size_of::<u32>()) as u64,
            bytemuck::cast_slice(&packed),
        );
    }

    pub fn write_all_circuit_cells(&self, queue: &wgpu::Queue, cells: &[[u32; 4]]) {
        assert_eq!(
            cells.len(),
            board_cell_count(),
            "circuit cell payload size mismatch"
        );

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.circuit.0,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(cells),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(
                    GRID_WIDTH * CIRCUIT_WORDS_PER_CELL * std::mem::size_of::<u32>() as u32,
                ),
                rows_per_image: Some(GRID_HEIGHT),
            },
            wgpu::Extent3d {
                width: GRID_WIDTH,
                height: GRID_HEIGHT,
                depth_or_array_layers: BOARD_LAYERS,
            },
        );
    }

    pub fn write_all_charge_texels(
        &self,
        queue: &wgpu::Queue,
        buffer_index: u32,
        texels: &[[u8; 4]],
    ) {
        assert!(
            buffer_index < CHARGE_BUFFER_COUNT,
            "charge buffer index out of bounds"
        );
        assert_eq!(
            texels.len(),
            packed_charge_texel_count(),
            "charge texel payload size mismatch"
        );

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.charge_buffers[buffer_index as usize].0,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CHARGE_GRID_WIDTH * CHARGE_TEXEL_SIZE),
                rows_per_image: Some(CHARGE_GRID_HEIGHT),
            },
            wgpu::Extent3d {
                width: CHARGE_GRID_WIDTH,
                height: CHARGE_GRID_HEIGHT,
                depth_or_array_layers: BOARD_LAYERS,
            },
        );
    }

    pub fn clear_cell(&self, queue: &wgpu::Queue, grid_cell: GridCell, arena_z: u32) {
        self.write_cell(
            queue,
            grid_cell,
            arena_z,
            CellSnapshot::from_cell(empty_cell()),
        );
    }

    pub fn write_cell(
        &self,
        queue: &wgpu::Queue,
        grid_cell: GridCell,
        arena_z: u32,
        cell: CellSnapshot,
    ) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.circuit.0,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: grid_cell.x,
                    y: grid_cell.y,
                    z: arena_z,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&[cell.words]),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(CIRCUIT_WORDS_PER_CELL * std::mem::size_of::<u32>() as u32),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn read_cell(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        grid_cell: GridCell,
        arena_z: u32,
    ) -> CellSnapshot {
        let bytes_per_row = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("board-circuit-readback"),
            size: u64::from(bytes_per_row),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("board-circuit-readback"),
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.circuit.0,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: grid_cell.x,
                    y: grid_cell.y,
                    z: arena_z,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
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
        let words = bytemuck::cast_slice::<u8, u32>(&mapped[..16]);
        let words = [words[0], words[1], words[2], words[3]];
        drop(mapped);
        readback.unmap();

        CellSnapshot { words }
    }

    pub fn clear_charge_at(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        grid_cell: GridCell,
        arena_z: u32,
    ) {
        for buffer_index in 0..CHARGE_BUFFER_COUNT {
            self.write_charge_value(device, queue, buffer_index, grid_cell, arena_z, 0);
        }
    }

    pub fn write_charge_value(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer_index: u32,
        grid_cell: GridCell,
        arena_z: u32,
        value: u8,
    ) {
        let (packed_x, packed_y, packed_z) =
            packed_charge_texel_coord(grid_cell.x, grid_cell.y, arena_z);
        let channel = packed_charge_channel(grid_cell.x, grid_cell.y);

        let mut texel = [
            pollster::block_on(self.read_charge_buffer(device, queue, buffer_index))
                [packed_charge_texel_index(grid_cell.x, grid_cell.y, arena_z)],
        ];
        texel[0][channel] = value;

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.charge_buffers[buffer_index as usize].0,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: packed_x,
                    y: packed_y,
                    z: packed_z,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texel),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
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
            label: Some("board-charge-readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("board-charge-readback"),
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.charge_buffers[buffer_index as usize].0,
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

    pub async fn read_input_range(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        start_index: u32,
        len: u32,
    ) -> Vec<u32> {
        if len == 0 {
            return Vec::new();
        }

        let start = start_index as usize;
        let len = len as usize;
        let end = start + len;
        assert!(
            end <= INPUT_BUFFER_LEN as usize,
            "input range out of bounds"
        );

        let byte_offset = (start * std::mem::size_of::<u32>()) as u64;
        let byte_len = (len * std::mem::size_of::<u32>()) as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("input-buffer-range-readback"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("input-buffer-range-readback"),
        });
        encoder.copy_buffer_to_buffer(&self.input_read_buffer, byte_offset, &readback, 0, byte_len);
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
        let values = bytemuck::cast_slice::<u8, u32>(&mapped).to_vec();
        drop(mapped);
        readback.unmap();
        values
    }

    pub async fn read_input_buffer(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u32> {
        self.read_input_range(device, queue, 0, INPUT_BUFFER_LEN)
            .await
    }

    pub async fn read_input_value(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        z: u32,
    ) -> u32 {
        self.read_input_range(device, queue, input_slot_index(x, y, z) as u32, 1)
            .await[0]
    }

    pub async fn read_output_range(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        start_index: u32,
        len: u32,
    ) -> Vec<u32> {
        if len == 0 {
            return Vec::new();
        }

        let start = start_index as usize;
        let len = len as usize;
        let end = start + len;
        assert!(
            end <= OUTPUT_BUFFER_LEN as usize,
            "output range out of bounds"
        );

        let byte_offset = (start * std::mem::size_of::<u32>()) as u64;
        let byte_len = (len * std::mem::size_of::<u32>()) as u64;

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output-buffer-range-readback"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("output-buffer-range-readback"),
        });
        encoder.copy_buffer_to_buffer(&self.output_buffer, byte_offset, &readback, 0, byte_len);
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
        let values = bytemuck::cast_slice::<u8, u32>(&mapped).to_vec();
        drop(mapped);
        readback.unmap();
        values
    }

    pub async fn read_output_buffer(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u32> {
        self.read_output_range(device, queue, 0, OUTPUT_BUFFER_LEN)
            .await
    }

    pub async fn read_output_value(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        z: u32,
    ) -> u32 {
        self.read_output_range(device, queue, output_slot_index(x, y, z) as u32, 1)
            .await[0]
    }
}

impl Simulation {
    pub fn new(device: &wgpu::Device) -> Self {
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
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadOnly,
                        format: CHARGE_TEXTURE_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D3,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("simulation-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
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
        }
    }

    pub fn step(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        board: &BoardTextures,
        current_buffer: u32,
        next_buffer: u32,
    ) {
        encoder.copy_buffer_to_buffer(
            board.input_write_buffer(),
            0,
            board.input_read_buffer(),
            0,
            input_buffer_size(),
        );
        encoder.clear_buffer(board.output_buffer(), 0, None);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("simulation-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(board.charge_view(current_buffer)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(board.circuit_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(board.charge_view(next_buffer)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: board.input_read_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: board.output_buffer().as_entire_binding(),
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
    let mut circuits =
        vec![CellSnapshot::empty().words; (GRID_WIDTH * GRID_HEIGHT * BOARD_LAYERS) as usize];
    let component = demo_scene::starter_component();

    for placed_cell in component.cells {
        let ix = linear_index(
            placed_cell.grid_cell.x,
            placed_cell.grid_cell.y,
            component.arena_z,
        );
        let cell = circuit_cell_from_snapshot(placed_cell.snapshot);
        circuits[ix] = [cell.tag as u32, cell.data[0], cell.data[1], cell.data[2]];
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
            bytes_per_row: Some(
                GRID_WIDTH * CIRCUIT_WORDS_PER_CELL * std::mem::size_of::<u32>() as u32,
            ),
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

pub fn pack_input_ref(grid_cell: GridCell) -> u32 {
    ((grid_cell.y & 0xffff) << 16) | (grid_cell.x & 0xffff)
}

pub fn unpack_input_ref(input_ref: u32) -> Option<GridCell> {
    if input_ref == INVALID_INPUT_REF {
        return None;
    }

    Some(GridCell {
        x: input_ref & 0xffff,
        y: (input_ref >> 16) & 0xffff,
    })
}

fn circuit_cell_from_snapshot(snapshot: CellSnapshot) -> CircuitCell {
    CircuitCell {
        tag: match snapshot.words[0] {
            0 => CircuitTag::Empty,
            1 => CircuitTag::Source,
            2 => CircuitTag::Noop,
            3 => CircuitTag::Not,
            4 => CircuitTag::And,
            5 => CircuitTag::Or,
            6 => CircuitTag::Xor,
            7 => CircuitTag::Nand,
            8 => CircuitTag::Nor,
            9 => CircuitTag::Xnor,
            10 => CircuitTag::Output,
            11 => CircuitTag::Input,
            _ => CircuitTag::Empty,
        },
        data: [snapshot.words[1], snapshot.words[2], snapshot.words[3]],
    }
}

impl CellSnapshot {
    fn from_cell(cell: CircuitCell) -> Self {
        Self {
            words: [cell.tag as u32, cell.data[0], cell.data[1], cell.data[2]],
        }
    }

    pub fn empty() -> Self {
        Self::from_cell(empty_cell())
    }

    pub fn source(value: u8) -> Self {
        Self::from_cell(source_cell(value))
    }

    pub fn noop() -> Self {
        Self::from_cell(noop_cell())
    }

    pub fn output() -> Self {
        Self::from_cell(output_cell())
    }

    pub fn input() -> Self {
        Self::from_cell(input_cell())
    }

    pub fn with_primary_input(self, source: GridCell) -> Self {
        let mut snapshot = self;
        snapshot.words[1] = pack_input_ref(source);
        snapshot
    }

    pub fn with_secondary_input(self, source: GridCell) -> Self {
        let mut snapshot = self;
        snapshot.words[2] = pack_input_ref(source);
        snapshot
    }

    pub fn primary_input(self) -> Option<GridCell> {
        unpack_input_ref(self.words[1])
    }

    pub fn secondary_input(self) -> Option<GridCell> {
        unpack_input_ref(self.words[2])
    }

    pub fn source_value(self) -> u8 {
        self.words[1] as u8
    }

    pub fn gate(kind: GateKind) -> Self {
        let tag = match kind {
            GateKind::Not => CircuitTag::Not,
            GateKind::And => CircuitTag::And,
            GateKind::Or => CircuitTag::Or,
            GateKind::Xor => CircuitTag::Xor,
            GateKind::Nand => CircuitTag::Nand,
            GateKind::Nor => CircuitTag::Nor,
            GateKind::Xnor => CircuitTag::Xnor,
        };
        Self::from_cell(gate_cell(tag))
    }

    pub fn kind(self) -> CellKind {
        match self.words[0] {
            1 => CellKind::Source,
            2 => CellKind::Noop,
            3 => CellKind::Not,
            4 => CellKind::And,
            5 => CellKind::Or,
            6 => CellKind::Xor,
            7 => CellKind::Nand,
            8 => CellKind::Nor,
            9 => CellKind::Xnor,
            10 => CellKind::Output,
            11 => CellKind::Input,
            _ => CellKind::Empty,
        }
    }

    pub fn is_empty(self) -> bool {
        self.kind() == CellKind::Empty
    }
}

fn empty_cell() -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Empty,
        data: [INVALID_INPUT_REF, INVALID_INPUT_REF, 0],
    }
}

fn source_cell(value: u8) -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Source,
        data: [u32::from(value), INVALID_INPUT_REF, 0],
    }
}

fn noop_cell() -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Noop,
        data: [INVALID_INPUT_REF, INVALID_INPUT_REF, 0],
    }
}

fn gate_cell(tag: CircuitTag) -> CircuitCell {
    CircuitCell {
        tag,
        data: [INVALID_INPUT_REF, INVALID_INPUT_REF, 0],
    }
}

fn output_cell() -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Output,
        data: [INVALID_INPUT_REF, INVALID_INPUT_REF, 0],
    }
}

fn input_cell() -> CircuitCell {
    CircuitCell {
        tag: CircuitTag::Input,
        data: [INVALID_INPUT_REF, INVALID_INPUT_REF, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn cpu_cell_next(
        component: &demo_scene::DemoComponent,
        texels: &PackedChargeTexels,
        x: u32,
        y: u32,
        z: u32,
    ) -> u8 {
        let cell = circuit_cell_from_snapshot(component.cell_at(x, y, z));

        let read_input = |input_ref: u32| {
            unpack_input_ref(input_ref)
                .map(|grid_cell| read_packed_charge(texels, grid_cell.x, grid_cell.y, z))
                .unwrap_or(0)
        };

        match cell.tag {
            CircuitTag::Empty => 0,
            CircuitTag::Source => cell.data[0] as u8,
            CircuitTag::Noop => read_input(cell.data[0]),
            CircuitTag::Not => {
                let input = read_input(cell.data[0]);
                gate_output(cell.tag, 0, input)
            }
            CircuitTag::And
            | CircuitTag::Or
            | CircuitTag::Xor
            | CircuitTag::Nand
            | CircuitTag::Nor
            | CircuitTag::Xnor => {
                let lhs = read_input(cell.data[0]);
                let rhs = read_input(cell.data[1]);
                gate_output(cell.tag, lhs, rhs)
            }
            CircuitTag::Output => read_input(cell.data[0]),
            CircuitTag::Input => 0,
        }
    }

    fn expected_charge_texels_after_steps(steps: u32) -> PackedChargeTexels {
        let component = demo_scene::starter_component();
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
                            cpu_cell_next(&component, &current, col, row, layer),
                        );
                    }
                }
            }
            current = next;
        }

        current
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
    fn output_cells_write_to_output_buffer() {
        let Some(gpu) = crate::test_gpu::shared_test_gpu() else {
            return;
        };

        let device = &gpu.device;
        let queue = &gpu.queue;

        let board = BoardTextures::new(device, queue);
        board.write_cell(
            queue,
            GridCell { x: 1, y: 0 },
            0,
            CellSnapshot::output().with_primary_input(GridCell { x: 0, y: 0 }),
        );
        board.clear_charge_at(device, queue, GridCell { x: 1, y: 0 }, 0);

        let simulation = Simulation::new(device);
        for (current_buffer, next_buffer) in [(0, 1), (1, 0)] {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("output-buffer-test-step"),
            });
            simulation.step(device, &mut encoder, &board, current_buffer, next_buffer);
            queue.submit(Some(encoder.finish()));
        }

        let output_value = pollster::block_on(board.read_output_value(device, queue, 1, 0, 0));
        assert_eq!(output_value, 0xff);

        let output_values = pollster::block_on(board.read_output_buffer(device, queue));
        assert_eq!(output_values[output_slot_index(1, 0, 0)], 0xff);
        assert!(
            output_values
                .iter()
                .enumerate()
                .all(|(ix, value)| ix == output_slot_index(1, 0, 0) || *value == 0)
        );
    }

    #[test]
    fn input_cells_read_from_input_buffer() {
        let Some(gpu) = crate::test_gpu::shared_test_gpu() else {
            return;
        };

        let device = &gpu.device;
        let queue = &gpu.queue;

        let board = BoardTextures::new(device, queue);
        board.write_cell(queue, GridCell { x: 0, y: 0 }, 0, CellSnapshot::input());
        board.write_cell(
            queue,
            GridCell { x: 1, y: 0 },
            0,
            CellSnapshot::output().with_primary_input(GridCell { x: 0, y: 0 }),
        );
        board.clear_charge_at(device, queue, GridCell { x: 0, y: 0 }, 0);
        board.clear_charge_at(device, queue, GridCell { x: 1, y: 0 }, 0);
        board.write_input_value(queue, 0, 0, 0, 0xff);

        let simulation = Simulation::new(device);
        for (current_buffer, next_buffer) in [(0, 1), (1, 0)] {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("input-buffer-test-step"),
            });
            simulation.step(device, &mut encoder, &board, current_buffer, next_buffer);
            queue.submit(Some(encoder.finish()));
        }

        let output_value = pollster::block_on(board.read_output_value(device, queue, 1, 0, 0));
        assert_eq!(output_value, 0xff);
    }

    #[test]
    fn input_output_range_helpers_round_trip_contiguous_slots() {
        let Some(gpu) = crate::test_gpu::shared_test_gpu() else {
            return;
        };

        let device = &gpu.device;
        let queue = &gpu.queue;

        let board = BoardTextures::new(device, queue);
        board.write_cell(queue, GridCell { x: 0, y: 0 }, 0, CellSnapshot::input());
        board.write_cell(queue, GridCell { x: 1, y: 0 }, 0, CellSnapshot::input());
        board.write_cell(
            queue,
            GridCell { x: 2, y: 0 },
            0,
            CellSnapshot::output().with_primary_input(GridCell { x: 0, y: 0 }),
        );
        board.write_cell(
            queue,
            GridCell { x: 3, y: 0 },
            0,
            CellSnapshot::output().with_primary_input(GridCell { x: 1, y: 0 }),
        );
        for grid_cell in [
            GridCell { x: 0, y: 0 },
            GridCell { x: 1, y: 0 },
            GridCell { x: 2, y: 0 },
            GridCell { x: 3, y: 0 },
        ] {
            board.clear_charge_at(device, queue, grid_cell, 0);
        }

        let input_start = input_slot_index(0, 0, 0) as u32;
        board.write_input_range(queue, input_start, &[0xaa, 0x55]);

        let simulation = Simulation::new(device);
        for (current_buffer, next_buffer) in [(0, 1), (1, 0)] {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("input-output-range-helpers-test-step"),
            });
            simulation.step(device, &mut encoder, &board, current_buffer, next_buffer);
            queue.submit(Some(encoder.finish()));
        }

        let read_inputs = pollster::block_on(board.read_input_range(device, queue, input_start, 2));
        assert_eq!(read_inputs, vec![0xaa, 0x55]);

        let output_start = output_slot_index(2, 0, 0) as u32;
        let read_outputs =
            pollster::block_on(board.read_output_range(device, queue, output_start, 2));
        assert_eq!(read_outputs, vec![0xaa, 0x55]);
    }

    #[test]
    fn simulation_step_can_be_read_back_without_window() {
        let Some(gpu) = crate::test_gpu::shared_test_gpu() else {
            return;
        };

        let device = &gpu.device;
        let queue = &gpu.queue;

        let board = BoardTextures::new(device, queue);
        let simulation = Simulation::new(device);
        let mut current_buffer = 0;

        for step in 1..=GRID_HEIGHT {
            let next_buffer = (current_buffer + 1) % CHARGE_BUFFER_COUNT;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("simulation-test-step"),
            });
            simulation.step(device, &mut encoder, &board, current_buffer, next_buffer);
            queue.submit(Some(encoder.finish()));

            let texels = pollster::block_on(board.read_charge_buffer(device, queue, next_buffer));
            let expected = expected_charge_texels_after_steps(step);

            assert_eq!(
                pollster::block_on(board.read_charge_value(device, queue, next_buffer, 0, 0, 0)),
                read_packed_charge(&expected, 0, 0, 0)
            );

            for z in 0..BOARD_LAYERS {
                for y in 0..GRID_HEIGHT {
                    for x in 0..GRID_WIDTH {
                        assert_eq!(
                            read_packed_charge(&texels, x, y, z),
                            read_packed_charge(&expected, x, y, z),
                            "mismatch at step={step}, coord=({x}, {y}, {z})"
                        );
                    }
                }
            }

            current_buffer = next_buffer;
        }
    }
}
