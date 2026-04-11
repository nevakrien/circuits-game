use std::{
    env,
    sync::mpsc,
    time::{Duration, Instant},
};

use circuits_game::{
    gate_plans::{
        compile_component_tree, ChildId, Component, ComponentPlan, ComponentPlans, Gate, GateId,
        PortId, SignalRef,
    },
    kernel::{GateKernel, UploadedGpuPlan},
    setup,
};
use egui_wgpu::wgpu;
use wgpu::util::DeviceExt;

const STRESS_GATES_PER_COMPONENT: u32 = 8_192;
const STRESS_BRANCH_FACTOR: usize = 4;
const STRESS_DEPTH: u32 = 5;
const DEFAULT_TICKS: u32 = 8;

fn main() {
    let args = Args::parse();
    let started_at = Instant::now();

    let gpu_started_at = Instant::now();
    let gpu = setup::gpu();
    let gpu_init = gpu_started_at.elapsed();
    let device = &gpu.device;
    let queue = &gpu.queue;
    let adapter_info = gpu.adapter.get_info();

    let scene_started_at = Instant::now();
    let mut scene = build_stress_demo_circuit();
    let scene_build = scene_started_at.elapsed();

    let bits_per_buffer = runtime_bits_per_buffer(device);
    let compile_started_at = Instant::now();
    let compiled = compile_component_tree(&mut scene.root, &scene.plans, bits_per_buffer)
        .expect("stress benchmark circuit should compile");
    let compile_time = compile_started_at.elapsed();

    let buffer_count = compiled
        .gate_store
        .values()
        .map(|store| store.buffer.0)
        .max()
        .unwrap_or(0)
        + 1;
    let storage_words = buffer_count * compiled.gpu_plan.words_per_buffer;
    let initial_words = seed_demo_words(
        &compiled.gate_store,
        compiled.gpu_plan.words_per_buffer,
        storage_words,
    );

    let kernel_started_at = Instant::now();
    let kernel = GateKernel::new(device);
    let kernel_init = kernel_started_at.elapsed();

    let upload_started_at = Instant::now();
    let uploaded = GateKernel::upload_plan(device, &compiled.gpu_plan);
    let read_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("stress-bench-read-buffer-0"),
        contents: bytemuck::cast_slice(&initial_words),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
    });
    let write_buffer =
        GateKernel::create_io_buffer(device, storage_words, "stress-bench-read-buffer-1");
    let output_buffer = GateKernel::create_io_buffer(
        device,
        compiled.gpu_plan.output_words,
        "stress-bench-output",
    );
    queue.write_buffer(&write_buffer, 0, bytemuck::cast_slice(&initial_words));
    wait_for_gpu(device);
    let upload_time = upload_started_at.elapsed();

    let mut runtime = BenchRuntime {
        kernel,
        uploaded,
        charge_buffers: [read_buffer, write_buffer],
        output_buffer,
        current_read: 0,
    };

    let first_tick_started_at = Instant::now();
    runtime.step(device, queue);
    let first_tick = first_tick_started_at.elapsed();
    let time_to_first_tick = started_at.elapsed();

    let mut steady_tick_durations = Vec::with_capacity(args.ticks.saturating_sub(1) as usize);
    for _ in 1..args.ticks {
        let tick_started_at = Instant::now();
        runtime.step(device, queue);
        steady_tick_durations.push(tick_started_at.elapsed());
    }

    let upload_probe = measure_upload_bandwidth(device, queue, storage_words);
    let charge_readback_probe =
        measure_readback_bandwidth(device, queue, &runtime.charge_buffers[runtime.current_read]);
    let output_readback_probe = measure_readback_bandwidth(device, queue, &runtime.output_buffer);

    let total = started_at.elapsed();
    let plan_bytes = uploaded_plan_bytes(&runtime.uploaded);
    let state_bytes = storage_words as u64 * std::mem::size_of::<u32>() as u64;
    let output_bytes = runtime.output_buffer.size();
    let steady_total = steady_tick_durations
        .iter()
        .copied()
        .fold(Duration::ZERO, |sum, value| sum + value);
    let steady_avg = if steady_tick_durations.is_empty() {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(steady_total.as_secs_f64() / steady_tick_durations.len() as f64)
    };

    println!("stress benchmark");
    println!(
        "  adapter: {} ({:?})",
        adapter_info.name, adapter_info.backend
    );
    println!("  ticks: {}", args.ticks);
    println!(
        "  scene: {} components, {} gates, depth {}",
        scene.component_count, scene.gate_count, scene.nesting_depth
    );
    println!(
        "  buffers: {} logical, {} words/buffer, {:.2} MiB state, {:.2} MiB output",
        buffer_count,
        compiled.gpu_plan.words_per_buffer,
        mib(state_bytes),
        mib(output_bytes)
    );
    println!(
        "  uploaded plan: {:.2} MiB total | basic {} workers | cross {} workers | output {} workers",
        mib(plan_bytes),
        runtime.uploaded.basic_gates.worker_count,
        runtime.uploaded.cross_writes.worker_count,
        runtime.uploaded.output_writes.worker_count
    );
    println!("timings");
    println!("  gpu init:            {}", fmt_duration(gpu_init));
    println!("  build stress scene:  {}", fmt_duration(scene_build));
    println!("  compile scene:       {}", fmt_duration(compile_time));
    println!("  create kernel:       {}", fmt_duration(kernel_init));
    println!("  upload/init buffers: {}", fmt_duration(upload_time));
    println!(
        "  time to first tick:  {}",
        fmt_duration(time_to_first_tick)
    );
    println!("  first tick only:     {}", fmt_duration(first_tick));
    println!("  steady avg tick:     {}", fmt_duration(steady_avg));
    if !steady_tick_durations.is_empty() {
        println!(
            "  steady throughput:   {:.1} ticks/s",
            1.0 / steady_avg.as_secs_f64()
        );
    }
    println!("  full run:            {}", fmt_duration(total));
    println!("transfer health");
    println!(
        "  init CPU->GPU writes: {:.2} MiB state mirrored twice + {:.2} MiB plan upload",
        mib(state_bytes * 2),
        mib(plan_bytes)
    );
    println!(
        "  per tick GPU copy:    {:.2} MiB scratch copy{}",
        mib(if runtime.uploaded.cross_writes.worker_count > 0 {
            state_bytes
        } else {
            0
        }),
        if runtime.uploaded.cross_writes.worker_count > 0 {
            ""
        } else {
            " (cross-write pass disabled)"
        }
    );
    println!(
        "  upload probe:         {} for {:.2} MiB ({:.2} GiB/s)",
        fmt_duration(upload_probe.elapsed),
        mib(upload_probe.bytes),
        gib_per_sec(upload_probe.bytes, upload_probe.elapsed)
    );
    println!(
        "  charge readback:      {} for {:.2} MiB ({:.2} GiB/s)",
        fmt_duration(charge_readback_probe.elapsed),
        mib(charge_readback_probe.bytes),
        gib_per_sec(charge_readback_probe.bytes, charge_readback_probe.elapsed)
    );
    println!(
        "  output readback:      {} for {:.2} MiB ({:.2} GiB/s)",
        fmt_duration(output_readback_probe.elapsed),
        mib(output_readback_probe.bytes),
        gib_per_sec(output_readback_probe.bytes, output_readback_probe.elapsed)
    );
}

struct Args {
    ticks: u32,
}

impl Args {
    fn parse() -> Self {
        let mut ticks = DEFAULT_TICKS;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--ticks" {
                if let Some(value) = args.next() {
                    ticks = value.parse().expect("--ticks must be a positive integer");
                }
            }
        }
        Self {
            ticks: ticks.max(1),
        }
    }
}

struct DemoSceneSpec {
    component_count: u64,
    gate_count: u64,
    nesting_depth: u32,
    root: Component,
    plans: ComponentPlans,
}

struct BenchRuntime {
    kernel: GateKernel,
    uploaded: UploadedGpuPlan,
    charge_buffers: [wgpu::Buffer; 2],
    output_buffer: wgpu::Buffer,
    current_read: usize,
}

impl BenchRuntime {
    fn step(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let write_index = (self.current_read + 1) % self.charge_buffers.len();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("stress-bench-step"),
        });
        self.kernel.encode(
            device,
            &mut encoder,
            &self.uploaded,
            &self.charge_buffers[self.current_read],
            &self.charge_buffers[write_index],
            &self.output_buffer,
        );
        queue.submit(Some(encoder.finish()));
        wait_for_gpu(device);
        self.current_read = write_index;
    }
}

struct TransferProbe {
    bytes: u64,
    elapsed: Duration,
}

fn measure_upload_bandwidth(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    storage_words: u32,
) -> TransferProbe {
    let bytes = storage_words.max(1) as u64 * std::mem::size_of::<u32>() as u64;
    let buffer =
        GateKernel::create_io_buffer(device, storage_words.max(1), "stress-bench-upload-probe");
    let words = vec![0xA5A5_5A5Au32; storage_words.max(1) as usize];
    let started_at = Instant::now();
    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&words));
    wait_for_gpu(device);
    TransferProbe {
        bytes,
        elapsed: started_at.elapsed(),
    }
}

fn measure_readback_bandwidth(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
) -> TransferProbe {
    let bytes = source.size().max(4);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stress-bench-readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let started_at = Instant::now();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("stress-bench-readback-copy"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, bytes);
    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    wait_for_gpu(device);
    receiver
        .recv()
        .expect("readback channel should receive a result")
        .expect("readback map should succeed");
    let mapped = slice.get_mapped_range();
    let checksum = mapped.iter().fold(0u8, |acc, byte| acc ^ byte);
    std::hint::black_box(checksum);
    drop(mapped);
    readback.unmap();

    TransferProbe {
        bytes,
        elapsed: started_at.elapsed(),
    }
}

fn wait_for_gpu(device: &wgpu::Device) {
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
}

fn uploaded_plan_bytes(plan: &UploadedGpuPlan) -> u64 {
    plan.basic_gates.worker_buffer.size()
        + plan.basic_gates.instruction_buffer.size()
        + plan.cross_writes.worker_buffer.size()
        + plan.cross_writes.instruction_buffer.size()
        + plan.output_writes.worker_buffer.size()
        + plan.output_writes.instruction_buffer.size()
}

fn runtime_bits_per_buffer(device: &wgpu::Device) -> u32 {
    let max_storage_bytes = device.limits().max_storage_buffer_binding_size;
    let max_storage_bits = max_storage_bytes.saturating_mul(8);
    (max_storage_bits & !31).max(32)
}

fn seed_demo_words(
    gate_store: &foldhash::HashMap<
        (circuits_game::gate_plans::NodeId, GateId),
        circuits_game::gate_plans::GateStoreLocation,
    >,
    words_per_buffer: u32,
    storage_words: u32,
) -> Vec<u32> {
    let mut words = vec![0u32; storage_words as usize];
    set_gate_seed(gate_store, &mut words, words_per_buffer, GateId(0), true);
    words
}

fn set_gate_seed(
    gate_store: &foldhash::HashMap<
        (circuits_game::gate_plans::NodeId, GateId),
        circuits_game::gate_plans::GateStoreLocation,
    >,
    words: &mut [u32],
    words_per_buffer: u32,
    gate: GateId,
    value: bool,
) {
    let Some((&(_, _), store)) = gate_store
        .iter()
        .find(|((node, candidate), _)| node.0 == 0 && *candidate == gate)
    else {
        return;
    };
    let word_index = store.buffer.0 * words_per_buffer + (store.bit.0 / 32);
    if let Some(word) = words.get_mut(word_index as usize) {
        if value {
            *word |= 1u32 << store.bit_in_word;
        } else {
            *word &= !(1u32 << store.bit_in_word);
        }
    }
}

fn build_stress_demo_circuit() -> DemoSceneSpec {
    let mut plans = ComponentPlans::new();
    let leaf_plan = plans.insert(
        ComponentPlan::new(build_stress_gates(STRESS_GATES_PER_COMPONENT))
            .with_grid_size([128, 64]),
    );
    let branch_plan = plans.insert(
        ComponentPlan::new(build_stress_gates(STRESS_GATES_PER_COMPONENT))
            .with_grid_size([256, 160])
            .with_child_placements(vec![
                circuits_game::gate_plans::ChildPlacement::at([0, 0]),
                circuits_game::gate_plans::ChildPlacement::at([128, 0]),
                circuits_game::gate_plans::ChildPlacement::at([0, 80]),
                circuits_game::gate_plans::ChildPlacement::at([128, 80]),
            ]),
    );
    let root = build_stress_component_tree(branch_plan, leaf_plan, STRESS_DEPTH);

    DemoSceneSpec {
        component_count: geometric_series_total(STRESS_BRANCH_FACTOR as u64, STRESS_DEPTH),
        gate_count: geometric_series_total(STRESS_BRANCH_FACTOR as u64, STRESS_DEPTH)
            * STRESS_GATES_PER_COMPONENT as u64,
        nesting_depth: STRESS_DEPTH + 1,
        root,
        plans,
    }
}

fn build_stress_component_tree(
    branch_plan: circuits_game::gate_plans::PlanId,
    leaf_plan: circuits_game::gate_plans::PlanId,
    depth: u32,
) -> Component {
    if depth == 0 {
        return Component::new(leaf_plan, Vec::new());
    }

    let children = (0..STRESS_BRANCH_FACTOR)
        .map(|_| build_stress_component_tree(branch_plan, leaf_plan, depth - 1))
        .collect();
    Component::new(branch_plan, children)
}

fn build_stress_gates(gate_count: u32) -> Vec<Gate> {
    let mut gates = Vec::with_capacity(gate_count as usize);
    gates.push(Gate::BitNot { src: this_ref(0) });
    for gate in 1..gate_count {
        let prev = gate - 1;
        let tap = gate.saturating_sub(37);
        let diag = gate.saturating_sub(113);
        gates.push(match gate % 6 {
            0 => Gate::BitNop {
                src: this_ref(prev),
            },
            1 => Gate::BitNot {
                src: this_ref(prev),
            },
            2 => Gate::BitXOR {
                a: this_ref(prev),
                b: this_ref(tap),
            },
            3 => Gate::BitAND {
                a: this_ref(prev),
                b: this_ref(diag),
            },
            4 => Gate::BitOR {
                a: this_ref(prev),
                b: this_ref(tap),
            },
            _ => Gate::BitXNOR {
                a: this_ref(prev),
                b: this_ref(diag),
            },
        });
    }
    gates
}

fn this_ref(gate: u32) -> SignalRef {
    SignalRef::ThisGate(GateId(gate))
}

fn geometric_series_total(branch_factor: u64, depth: u32) -> u64 {
    let mut total = 0u64;
    let mut layer = 1u64;
    for _ in 0..=depth {
        total += layer;
        layer *= branch_factor;
    }
    total
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn gib_per_sec(bytes: u64, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        return 0.0;
    }
    bytes as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0 * 1024.0)
}

fn fmt_duration(duration: Duration) -> String {
    format!("{:>8.3} ms", duration.as_secs_f64() * 1_000.0)
}

#[allow(dead_code)]
fn _unused_port_id(_: PortId, _: ChildId) {}
