use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use wgpu::util::DeviceExt;

use crate::{
    charge_buffer::{BitsIndex, BufferId, PreparedBitCross},
    gate_plans::{
        BasicGateInstruction, GpuPlan, OutputWriteInstruction, PreparedBasicGates,
        PreparedOutputWrites,
    },
};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct BasicGateGpuWorker {
    pub tgt_word_index: u32,
    pub instruction_start: u32,
    pub instruction_len: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct BasicGateGpuInstruction {
    pub op: u32,
    pub dst_bit_in_word: u32,
    pub src_a_bit_index: u32,
    pub src_b_bit_index: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct CrossWriteGpuWorker {
    pub tgt_word_index: u32,
    pub instruction_start: u32,
    pub instruction_len: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct CrossWriteGpuInstruction {
    pub src_bit_index: u32,
    pub tgt_bit_in_word: u32,
}

const _: [(); 12] = [(); std::mem::size_of::<BasicGateGpuWorker>()];
const _: [(); 4] = [(); std::mem::align_of::<BasicGateGpuWorker>()];
const _: [(); 16] = [(); std::mem::size_of::<BasicGateGpuInstruction>()];
const _: [(); 4] = [(); std::mem::align_of::<BasicGateGpuInstruction>()];
const _: [(); 12] = [(); std::mem::size_of::<CrossWriteGpuWorker>()];
const _: [(); 4] = [(); std::mem::align_of::<CrossWriteGpuWorker>()];
const _: [(); 8] = [(); std::mem::size_of::<CrossWriteGpuInstruction>()];
const _: [(); 4] = [(); std::mem::align_of::<CrossWriteGpuInstruction>()];

pub struct UploadedPass {
    pub worker_count: u32,
    pub worker_buffer: wgpu::Buffer,
    pub instruction_buffer: wgpu::Buffer,
}

pub struct UploadedGpuPlan {
    pub bits_per_buffer: u32,
    pub words_per_buffer: u32,
    pub output_words: u32,
    pub basic_gates: UploadedPass,
    pub cross_writes: UploadedPass,
    pub output_writes: UploadedPass,
}

pub struct GateKernel {
    basic_gates_layout: wgpu::BindGroupLayout,
    cross_write_layout: wgpu::BindGroupLayout,
    basic_gates_pipeline: wgpu::ComputePipeline,
    cross_write_pipeline: wgpu::ComputePipeline,
}

impl GateKernel {
    pub fn new(device: &wgpu::Device) -> Self {
        let basic_gates_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("basic-gates"),
            source: wgpu::ShaderSource::Wgsl(include_str!("basic_gates.wgsl").into()),
        });
        let cross_write_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cross-write"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cross_write.wgsl").into()),
        });

        let basic_gates_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("basic-gates-layout"),
                entries: &[
                    storage_layout_entry(0, true),
                    storage_layout_entry(1, false),
                    storage_layout_entry(2, true),
                    storage_layout_entry(3, true),
                ],
            });
        let cross_write_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cross-write-layout"),
                entries: &[
                    storage_layout_entry(0, true),
                    storage_layout_entry(1, false),
                    storage_layout_entry(2, true),
                    storage_layout_entry(3, true),
                ],
            });

        let basic_gates_pipeline = create_pipeline(
            device,
            &basic_gates_shader,
            &basic_gates_layout,
            "main",
            "basic-gates-pipeline",
        );
        let cross_write_pipeline = create_pipeline(
            device,
            &cross_write_shader,
            &cross_write_layout,
            "main",
            "cross-write-pipeline",
        );

        Self {
            basic_gates_layout,
            cross_write_layout,
            basic_gates_pipeline,
            cross_write_pipeline,
        }
    }

    pub fn upload_plan(device: &wgpu::Device, plan: &GpuPlan) -> UploadedGpuPlan {
        let (basic_gate_workers, basic_gate_instructions) = flatten_basic_gates(plan);
        let (cross_write_workers, cross_write_instructions) = flatten_cross_writes(plan);
        let (output_write_workers, output_write_instructions) = flatten_output_writes(plan);

        UploadedGpuPlan {
            bits_per_buffer: plan.bits_per_buffer,
            words_per_buffer: plan.words_per_buffer,
            output_words: plan.output_words,
            basic_gates: UploadedPass {
                worker_count: basic_gate_workers.len() as u32,
                worker_buffer: create_storage_buffer(
                    device,
                    "basic-gate-workers",
                    &basic_gate_workers,
                ),
                instruction_buffer: create_storage_buffer(
                    device,
                    "basic-gate-instructions",
                    &basic_gate_instructions,
                ),
            },
            cross_writes: UploadedPass {
                worker_count: cross_write_workers.len() as u32,
                worker_buffer: create_storage_buffer(
                    device,
                    "cross-write-workers",
                    &cross_write_workers,
                ),
                instruction_buffer: create_storage_buffer(
                    device,
                    "cross-write-instructions",
                    &cross_write_instructions,
                ),
            },
            output_writes: UploadedPass {
                worker_count: output_write_workers.len() as u32,
                worker_buffer: create_storage_buffer(
                    device,
                    "output-write-workers",
                    &output_write_workers,
                ),
                instruction_buffer: create_storage_buffer(
                    device,
                    "output-write-instructions",
                    &output_write_instructions,
                ),
            },
        }
    }

    pub fn create_io_buffer(device: &wgpu::Device, words: u32, label: &str) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: words.max(1) as u64 * std::mem::size_of::<u32>() as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        plan: &UploadedGpuPlan,
        read_buffer: &wgpu::Buffer,
        write_buffer: &wgpu::Buffer,
        output_buffer: &wgpu::Buffer,
    ) {
        encoder.clear_buffer(write_buffer, 0, None);
        encoder.clear_buffer(output_buffer, 0, None);

        if plan.basic_gates.worker_count > 0 {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("basic-gates-bind-group"),
                layout: &self.basic_gates_layout,
                entries: &[
                    bind_buffer(0, read_buffer),
                    bind_buffer(1, write_buffer),
                    bind_buffer(2, &plan.basic_gates.worker_buffer),
                    bind_buffer(3, &plan.basic_gates.instruction_buffer),
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.basic_gates_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(plan.basic_gates.worker_count, 1, 1);
        }

        if plan.cross_writes.worker_count > 0 {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cross-write-bind-group"),
                layout: &self.cross_write_layout,
                entries: &[
                    bind_buffer(0, write_buffer),
                    bind_buffer(1, write_buffer),
                    bind_buffer(2, &plan.cross_writes.worker_buffer),
                    bind_buffer(3, &plan.cross_writes.instruction_buffer),
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.cross_write_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(plan.cross_writes.worker_count, 1, 1);
        }

        if plan.output_writes.worker_count > 0 {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("output-write-bind-group"),
                layout: &self.cross_write_layout,
                entries: &[
                    bind_buffer(0, write_buffer),
                    bind_buffer(1, output_buffer),
                    bind_buffer(2, &plan.output_writes.worker_buffer),
                    bind_buffer(3, &plan.output_writes.instruction_buffer),
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.cross_write_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(plan.output_writes.worker_count, 1, 1);
        }
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    entry_point: &str,
    label: &str,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        module: shader,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn storage_layout_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bind_buffer(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn create_storage_buffer<T: Pod + Zeroable>(
    device: &wgpu::Device,
    label: &str,
    values: &[T],
) -> wgpu::Buffer {
    if values.is_empty() {
        return device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(&T::zeroed()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
    }

    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(values),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

fn flatten_basic_gates(plan: &GpuPlan) -> (Vec<BasicGateGpuWorker>, Vec<BasicGateGpuInstruction>) {
    let mut workers = Vec::new();
    let mut instructions = Vec::new();

    for batch in &plan.basic_gates {
        append_basic_gate_batch(
            plan.bits_per_buffer,
            plan.words_per_buffer,
            batch,
            &mut workers,
            &mut instructions,
        );
    }

    (workers, instructions)
}

fn append_basic_gate_batch(
    bits_per_buffer: u32,
    words_per_buffer: u32,
    batch: &PreparedBasicGates,
    workers: &mut Vec<BasicGateGpuWorker>,
    instructions: &mut Vec<BasicGateGpuInstruction>,
) {
    for worker in &batch.workers {
        let instruction_start = instructions.len() as u32;
        let begin = worker.instruction_start as usize;
        let end = begin + worker.instruction_len as usize;

        for instruction in &batch.instructions[begin..end] {
            instructions.push(flatten_basic_gate_instruction(bits_per_buffer, instruction));
        }

        workers.push(BasicGateGpuWorker {
            tgt_word_index: absolute_word_index(
                words_per_buffer,
                batch.tgt_buffer,
                worker.tgt_word_byte_offset / 4,
            ),
            instruction_start,
            instruction_len: worker.instruction_len,
        });
    }
}

fn flatten_basic_gate_instruction(
    bits_per_buffer: u32,
    instruction: &BasicGateInstruction,
) -> BasicGateGpuInstruction {
    BasicGateGpuInstruction {
        op: instruction.op as u32,
        dst_bit_in_word: instruction.dst_bit_in_word.0,
        src_a_bit_index: absolute_bit_index(bits_per_buffer, instruction.src_a),
        src_b_bit_index: absolute_bit_index(bits_per_buffer, instruction.src_b),
    }
}

fn flatten_cross_writes(
    plan: &GpuPlan,
) -> (Vec<CrossWriteGpuWorker>, Vec<CrossWriteGpuInstruction>) {
    let mut workers = Vec::new();
    let mut instructions = Vec::new();

    for batch in &plan.cross_writes {
        append_cross_write_batch(
            plan.bits_per_buffer,
            plan.words_per_buffer,
            batch,
            &mut workers,
            &mut instructions,
        );
    }

    (workers, instructions)
}

fn append_cross_write_batch(
    bits_per_buffer: u32,
    words_per_buffer: u32,
    batch: &PreparedBitCross,
    workers: &mut Vec<CrossWriteGpuWorker>,
    instructions: &mut Vec<CrossWriteGpuInstruction>,
) {
    for worker in &batch.workers {
        let instruction_start = instructions.len() as u32;
        let begin = worker.instruction_start as usize;
        let end = begin + worker.instruction_len as usize;

        for instruction in &batch.instructions[begin..end] {
            instructions.push(CrossWriteGpuInstruction {
                src_bit_index: absolute_buffer_bit(
                    bits_per_buffer,
                    batch.src_buffer,
                    instruction.src_bit.0,
                ),
                tgt_bit_in_word: instruction.tgt_bit_in_word.0,
            });
        }

        workers.push(CrossWriteGpuWorker {
            tgt_word_index: absolute_word_index(
                words_per_buffer,
                batch.tgt_buffer,
                worker.tgt_word_byte_offset / 4,
            ),
            instruction_start,
            instruction_len: worker.instruction_len,
        });
    }
}

fn flatten_output_writes(
    plan: &GpuPlan,
) -> (Vec<CrossWriteGpuWorker>, Vec<CrossWriteGpuInstruction>) {
    let mut workers = Vec::new();
    let mut instructions = Vec::new();

    for batch in &plan.output_writes {
        append_output_write_batch(plan.bits_per_buffer, batch, &mut workers, &mut instructions);
    }

    (workers, instructions)
}

fn append_output_write_batch(
    bits_per_buffer: u32,
    batch: &PreparedOutputWrites,
    workers: &mut Vec<CrossWriteGpuWorker>,
    instructions: &mut Vec<CrossWriteGpuInstruction>,
) {
    for worker in &batch.workers {
        let instruction_start = instructions.len() as u32;
        let begin = worker.instruction_start as usize;
        let end = begin + worker.instruction_len as usize;

        for instruction in &batch.instructions[begin..end] {
            instructions.push(flatten_output_write_instruction(
                bits_per_buffer,
                instruction,
            ));
        }

        workers.push(CrossWriteGpuWorker {
            tgt_word_index: worker.tgt_word_byte_offset / 4,
            instruction_start,
            instruction_len: worker.instruction_len,
        });
    }
}

fn flatten_output_write_instruction(
    bits_per_buffer: u32,
    instruction: &OutputWriteInstruction,
) -> CrossWriteGpuInstruction {
    CrossWriteGpuInstruction {
        src_bit_index: absolute_bit_index(bits_per_buffer, instruction.src),
        tgt_bit_in_word: instruction.dst_bit_in_word.0,
    }
}

fn absolute_bit_index(bits_per_buffer: u32, bit: BitsIndex) -> u32 {
    absolute_buffer_bit(bits_per_buffer, bit.0, bit.1.0)
}

fn absolute_buffer_bit(bits_per_buffer: u32, buffer: BufferId, bit_in_buffer: u32) -> u32 {
    buffer.0 * bits_per_buffer + bit_in_buffer
}

fn absolute_word_index(words_per_buffer: u32, buffer: BufferId, word_in_buffer: u32) -> u32 {
    buffer.0 * words_per_buffer + word_in_buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate_plans::{Component, Gate, GateId, GateRef, ScopeRef, compile_component_tree};

    fn this_ref(gate: u32) -> GateRef {
        GateRef {
            scope: ScopeRef::This,
            gate: GateId(gate),
        }
    }

    #[test]
    fn flattening_converts_plan_to_absolute_word_and_bit_indices() {
        let root_gates = (0..32)
            .map(|gate| Gate::BitNop {
                src: this_ref(gate),
            })
            .collect();
        let child_gates = (0..14)
            .map(|gate| {
                if gate == 13 {
                    Gate::BitNop {
                        src: GateRef {
                            scope: ScopeRef::Parent,
                            gate: GateId(5),
                        },
                    }
                } else {
                    Gate::BitNop {
                        src: this_ref(gate),
                    }
                }
            })
            .collect();

        let mut root = Component::new(root_gates, vec![Component::new(child_gates, vec![])]);
        let compiled = compile_component_tree(&mut root, 32).expect("tree should compile");

        let (basic_gate_workers, basic_gate_instructions) = flatten_basic_gates(&compiled.gpu_plan);
        let (cross_write_workers, cross_write_instructions) =
            flatten_cross_writes(&compiled.gpu_plan);
        let (output_write_workers, output_write_instructions) =
            flatten_output_writes(&compiled.gpu_plan);

        assert!(!basic_gate_workers.is_empty());
        assert!(!basic_gate_instructions.is_empty());
        assert_eq!(cross_write_workers[0].tgt_word_index, 1);
        assert_eq!(cross_write_instructions[0].src_bit_index, 5);
        assert_eq!(cross_write_instructions[0].tgt_bit_in_word, 13);
        assert_eq!(output_write_workers[0].tgt_word_index, 0);
        assert_eq!(output_write_instructions[31].tgt_bit_in_word, 31);
    }

    #[test]
    fn basic_gate_op_values_match_shader_encoding() {
        assert_eq!(crate::gate_plans::BasicGateOp::BitNAND as u32, 1);
        assert_eq!(crate::gate_plans::BasicGateOp::BitAND as u32, 2);
        assert_eq!(crate::gate_plans::BasicGateOp::BitOR as u32, 3);
        assert_eq!(crate::gate_plans::BasicGateOp::BitNOR as u32, 4);
        assert_eq!(crate::gate_plans::BasicGateOp::BitXOR as u32, 5);
        assert_eq!(crate::gate_plans::BasicGateOp::BitXNOR as u32, 6);
        assert_eq!(crate::gate_plans::BasicGateOp::BitNot as u32, 7);
        assert_eq!(crate::gate_plans::BasicGateOp::BitNop as u32, 8);
    }
}
