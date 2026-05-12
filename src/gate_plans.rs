use crate::charge_buffer::{
    BitCrossWorker, Bits, BitsIndex, BufferId, ChargeAlloc, PreparedBitCross, WorkingMem,
};
use foldhash::{HashMap, HashSet};
use rayon::prelude::*;
use slab::Slab;

const MAX_KERNEL_INSTRUCTIONS: usize = 1 << 8;
const MAX_KERNEL_WORKERS: usize = 1 << 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GateId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlanId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChildId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AncestorDepth(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortLocation {
    pub x: u16,
    pub y: u16,
}

impl PortLocation {
    pub const BOTTOM_RIGHT: Self = Self {
        x: u16::MAX,
        y: u16::MAX,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComponentPort {
    pub id: PortId,
    pub gate: GateId,
    pub location: PortLocation,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildPlacement {
    pub min: [u32; 2],
}

impl ChildPlacement {
    pub const ONE_CELL: Self = Self { min: [0, 0] };

    pub const fn at(min: [u32; 2]) -> Self {
        Self { min }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatePlacement {
    pub gate: GateId,
    pub min: [u32; 2],
}

impl GatePlacement {
    pub const fn at(gate: GateId, min: [u32; 2]) -> Self {
        Self { gate, min }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireEndpoint {
    GateOutput(GateId),
    GateInput { gate: GateId, input: u8 },
    ComponentInput(PortId),
    ComponentOutput(PortId),
    ChildOutput { child: ChildId, port: PortId },
    ChildInput { child: ChildId, port: PortId },
    AncestorOutput { depth: AncestorDepth, port: PortId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WirePoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WireLayout {
    pub from: WireEndpoint,
    pub to: WireEndpoint,
    pub bends: Vec<WirePoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentLayout {
    pub gate_placements: Vec<GatePlacement>,
    pub child_placements: Vec<ChildPlacement>,
    pub wires: Vec<WireLayout>,
}

impl ComponentLayout {
    pub fn with_child_placements(mut self, child_placements: Vec<ChildPlacement>) -> Self {
        self.child_placements = child_placements;
        self
    }

    pub fn with_gate_placements(mut self, gate_placements: Vec<GatePlacement>) -> Self {
        self.gate_placements = gate_placements;
        self
    }

    pub fn with_wires(mut self, wires: Vec<WireLayout>) -> Self {
        self.wires = wires;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalRef {
    Disconnected,
    ThisGate(GateId),
    InputPort(PortId),
    ChildOutput { child: ChildId, port: PortId },
    AncestorOutput { depth: AncestorDepth, port: PortId },
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gate {
    BitNAND { a: SignalRef, b: SignalRef } = 1,
    BitAND { a: SignalRef, b: SignalRef },
    BitOR { a: SignalRef, b: SignalRef },
    BitNOR { a: SignalRef, b: SignalRef },
    BitXOR { a: SignalRef, b: SignalRef },
    BitXNOR { a: SignalRef, b: SignalRef },
    BitNot { src: SignalRef },
    BitNop { src: SignalRef },
}

impl Gate {
    pub fn label(self) -> &'static str {
        match self {
            Gate::BitNAND { .. } => "NAND",
            Gate::BitAND { .. } => "AND",
            Gate::BitOR { .. } => "OR",
            Gate::BitNOR { .. } => "NOR",
            Gate::BitXOR { .. } => "XOR",
            Gate::BitXNOR { .. } => "XNOR",
            Gate::BitNot { .. } => "NOT",
            Gate::BitNop { .. } => "NOP",
        }
    }

    pub fn input_refs(self) -> [Option<SignalRef>; 2] {
        match self {
            Gate::BitNAND { a, b }
            | Gate::BitAND { a, b }
            | Gate::BitOR { a, b }
            | Gate::BitNOR { a, b }
            | Gate::BitXOR { a, b }
            | Gate::BitXNOR { a, b } => [Some(a), Some(b)],
            Gate::BitNot { src } | Gate::BitNop { src } => [Some(src), None],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComponentPlan {
    pub grid_size: [u32; 2],
    pub gates: HashMap<GateId, Gate>,
    pub inputs: Vec<ComponentPort>,
    pub outputs: Vec<ComponentPort>,
}

impl ComponentPlan {
    pub fn new(gates: Vec<Gate>) -> Self {
        Self {
            grid_size: default_grid_size(gates.len() as u32),
            gates: gates
                .into_iter()
                .enumerate()
                .map(|(index, gate)| (GateId(index as u32), gate))
                .collect(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn with_ports(
        gates: Vec<Gate>,
        inputs: Vec<ComponentPort>,
        outputs: Vec<ComponentPort>,
    ) -> Self {
        Self {
            grid_size: default_grid_size(gates.len() as u32),
            gates: gates
                .into_iter()
                .enumerate()
                .map(|(index, gate)| (GateId(index as u32), gate))
                .collect(),
            inputs,
            outputs,
        }
    }

    pub fn with_gate_map(mut self, gates: HashMap<GateId, Gate>) -> Self {
        self.gates = gates;
        self
    }

    pub fn with_grid_size(mut self, grid_size: [u32; 2]) -> Self {
        self.grid_size = [grid_size[0].max(1), grid_size[1].max(1)];
        self
    }

    pub fn gate_count(&self) -> u32 {
        self.gates.len() as u32
    }

    pub fn gate(&self, id: GateId) -> Option<Gate> {
        self.gates.get(&id).copied()
    }

    pub fn gate_mut(&mut self, id: GateId) -> Option<&mut Gate> {
        self.gates.get_mut(&id)
    }

    pub fn ordered_gates(&self) -> Vec<(GateId, Gate)> {
        let mut gates = self
            .gates
            .iter()
            .map(|(&gate_id, &gate)| (gate_id, gate))
            .collect::<Vec<_>>();
        gates.sort_by_key(|(gate_id, _)| *gate_id);
        gates
    }

    pub fn ordered_gate_ids(&self) -> Vec<GateId> {
        let mut gate_ids = self.gates.keys().copied().collect::<Vec<_>>();
        gate_ids.sort();
        gate_ids
    }

    fn input_port(&self, id: PortId) -> Option<&ComponentPort> {
        self.inputs.iter().find(|port| port.id == id)
    }

    fn output_port(&self, id: PortId) -> Option<&ComponentPort> {
        self.outputs.iter().find(|port| port.id == id)
    }
}

fn default_grid_size(count: u32) -> [u32; 2] {
    let count = count.max(1);
    let mut candidates = Vec::new();

    for w_pow in 0..=8u32 {
        for h_pow in 0..=8u32 {
            let w = 1u32 << w_pow;
            let h = 1u32 << h_pow;
            if w.max(h) > 4 * w.min(h) || w * h < count {
                continue;
            }
            candidates.push((w * h, w.abs_diff(h), w, h));
        }
    }

    let (_, _, w, h) = candidates
        .into_iter()
        .min()
        .expect("grid candidates should exist");
    [w, h]
}

#[derive(Debug, Default, Clone)]
pub struct ComponentPlans {
    plans: Slab<ComponentPlan>,
}

impl ComponentPlans {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, plan: ComponentPlan) -> PlanId {
        PlanId(self.plans.insert(plan))
    }

    pub fn get(&self, id: PlanId) -> Option<&ComponentPlan> {
        self.plans.get(id.0)
    }
}

#[derive(Debug, Clone)]
pub struct Component {
    pub id: NodeId,
    pub plan: PlanId,
    pub children: Vec<Component>,
    pub child_input_connections: Vec<ChildInputConnection>,
    pub layout: ComponentLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildInputConnection {
    pub child: ChildId,
    pub input: PortId,
    pub src: SignalRef,
}

impl Component {
    pub fn new(plan: PlanId, children: Vec<Component>) -> Self {
        Self {
            id: INVALID_NODE_ID,
            plan,
            children,
            child_input_connections: Vec::new(),
            layout: ComponentLayout::default(),
        }
    }

    pub fn with_child_input_connections(
        plan: PlanId,
        children: Vec<Component>,
        child_input_connections: Vec<ChildInputConnection>,
    ) -> Self {
        Self {
            id: INVALID_NODE_ID,
            plan,
            children,
            child_input_connections,
            layout: ComponentLayout::default(),
        }
    }

    pub fn with_layout_and_child_input_connections(
        plan: PlanId,
        children: Vec<Component>,
        child_input_connections: Vec<ChildInputConnection>,
        layout: ComponentLayout,
    ) -> Self {
        Self {
            id: INVALID_NODE_ID,
            plan,
            children,
            child_input_connections,
            layout,
        }
    }

    pub fn from_gates(
        plans: &mut ComponentPlans,
        gates: Vec<Gate>,
        children: Vec<Component>,
    ) -> Self {
        let plan = plans.insert(ComponentPlan::new(gates));
        Self::new(plan, children)
    }

    pub fn from_plan(
        plans: &mut ComponentPlans,
        plan: ComponentPlan,
        children: Vec<Component>,
    ) -> Self {
        let plan = plans.insert(plan);
        Self::new(plan, children)
    }
}

pub const INVALID_NODE_ID: NodeId = NodeId(u32::MAX);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileLayout {
    Inline,
    Outline,
}

#[derive(Debug, Clone)]
pub struct OutlinePlan {
    pub input_count: u32,
    pub output_count: u32,
    pub extra_bits_needed: u32,
    pub layout: CompileLayout,
}

#[derive(Debug, Clone)]
pub struct CompiledComponentInfo {
    pub node: NodeId,
    pub self_bits: Vec<BitsIndex>,
    pub child_ids: Vec<NodeId>,
    pub outline: OutlinePlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GateStoreLocation {
    pub buffer: crate::charge_buffer::BufferId,
    pub bit: Bits,
    pub word_byte_offset: u32,
    pub bit_in_word: u8,
}

#[derive(Debug, Clone)]
pub struct CompiledTree {
    pub components: HashMap<NodeId, CompiledComponentInfo>,
    pub gate_store: HashMap<(NodeId, GateId), GateStoreLocation>,
    pub gpu_plan: GpuPlan,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicGateOp {
    BitNAND = 1,
    BitAND,
    BitOR,
    BitNOR,
    BitXOR,
    BitXNOR,
    BitNot,
    BitNop,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicGateInstruction {
    pub op: BasicGateOp,
    pub dst_bit_in_word: Bits,
    pub src_a: BitsIndex,
    pub src_b: BitsIndex,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicGateWorker {
    pub tgt_word_byte_offset: u32,
    pub instruction_start: u32,
    pub instruction_len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBasicGates {
    pub tgt_buffer: crate::charge_buffer::BufferId,
    pub workers: Vec<BasicGateWorker>,
    pub instructions: Vec<BasicGateInstruction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedOutputWrites {
    pub workers: Vec<BitCrossWorker>,
    pub instructions: Vec<OutputWriteInstruction>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputWriteInstruction {
    pub src: BitsIndex,
    pub dst_bit_in_word: Bits,
}

#[derive(Debug, Clone)]
pub struct GpuPlan {
    pub bits_per_buffer: u32,
    pub words_per_buffer: u32,
    pub output_words: u32,
    pub basic_gates: Vec<PreparedBasicGates>,
    pub cross_writes: Vec<PreparedBitCross>,
    pub output_writes: Vec<PreparedOutputWrites>,
}

pub struct GateCompiler {
    pub bits: HashMap<(NodeId, GateId), BitsIndex>,
    pub zero_bit: BitsIndex,
    pub alloc: ChargeAlloc,
    pub mem: WorkingMem,
    pub components: HashMap<NodeId, CompiledComponentInfo>,
}

#[derive(Debug, Clone)]
pub struct RefUsage {
    pub input_ports_read: HashSet<PortId>,
    pub output_ports_read_by_parent: HashSet<PortId>,
}

impl Default for RefUsage {
    fn default() -> Self {
        Self {
            input_ports_read: HashSet::default(),
            output_ports_read_by_parent: HashSet::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompileError {
    MissingPlan(PlanId),
    MissingNode(NodeId),
    MissingCompiledInfo(NodeId),
    InvalidGateRef {
        from_node: NodeId,
        from_gate: GateId,
        bad_ref: SignalRef,
        reason: &'static str,
    },
    InvalidChildInputConnection {
        node: NodeId,
        child: ChildId,
        input: PortId,
        reason: &'static str,
    },
    TargetGateOutOfRange {
        from_node: NodeId,
        from_gate: GateId,
        target_node: NodeId,
        target_gate: GateId,
        target_gate_count: u32,
    },
    MissingBitsForGate {
        node: NodeId,
        gate: GateId,
    },
    MissingInputPort {
        node: NodeId,
        port: PortId,
    },
    MissingOutputPort {
        node: NodeId,
        port: PortId,
    },
    DuplicatePortId {
        node: NodeId,
        port: PortId,
        kind: &'static str,
    },
    DuplicateChildInputConnection {
        node: NodeId,
        child: ChildId,
        input: PortId,
    },
}

fn component_plan<'a>(
    node: &Component,
    plans: &'a ComponentPlans,
) -> Result<&'a ComponentPlan, CompileError> {
    plans
        .get(node.plan)
        .ok_or(CompileError::MissingPlan(node.plan))
}

fn validate_plan_ports(node: NodeId, plan: &ComponentPlan) -> Result<(), CompileError> {
    let mut seen_inputs = HashSet::default();
    for port in &plan.inputs {
        if !seen_inputs.insert(port.id) {
            return Err(CompileError::DuplicatePortId {
                node,
                port: port.id,
                kind: "input",
            });
        }
    }

    let mut seen_outputs = HashSet::default();
    for port in &plan.outputs {
        if !seen_outputs.insert(port.id) {
            return Err(CompileError::DuplicatePortId {
                node,
                port: port.id,
                kind: "output",
            });
        }
    }

    Ok(())
}

fn child_ctx<'a>(
    parent_stack: &'a [NodeId],
    parent: NodeId,
    child_ids: &'a [NodeId],
    child_index_in_parent: Option<ChildId>,
) -> CompileCtx<'a> {
    CompileCtx {
        current: parent,
        parent_stack,
        child_ids,
        child_index_in_parent,
    }
}

fn child_input_connection<'a>(
    node: &'a Component,
    child: ChildId,
    input: PortId,
) -> Option<&'a ChildInputConnection> {
    node.child_input_connections
        .iter()
        .find(|connection| connection.child == child && connection.input == input)
}

fn child_index_in_parent(
    node_id: NodeId,
    parent_stack: &[NodeId],
    by_id: &HashMap<NodeId, &Component>,
) -> Result<Option<ChildId>, CompileError> {
    if parent_stack.len() < 2 {
        return Ok(None);
    }

    let parent_id = *parent_stack.last().expect("len checked above");
    let grandparent_id = parent_stack[parent_stack.len() - 2];

    debug_assert_eq!(parent_id, node_id);
    let grandparent = by_id
        .get(&grandparent_id)
        .copied()
        .ok_or(CompileError::MissingNode(grandparent_id))?;
    Ok(grandparent
        .children
        .iter()
        .position(|child| child.id == node_id)
        .map(|i| ChildId(i as u32)))
}

#[derive(Debug, Clone, Copy)]
struct CompileCtx<'a> {
    current: NodeId,
    parent_stack: &'a [NodeId],
    child_ids: &'a [NodeId],
    child_index_in_parent: Option<ChildId>,
}

pub fn assign_node_ids(root: &mut Component) {
    fn rec(node: &mut Component, next: &mut u32) {
        node.id = NodeId(*next);
        *next += 1;
        for child in &mut node.children {
            rec(child, next);
        }
    }

    let mut next = 0;
    rec(root, &mut next);
}

pub fn validate_component_tree(
    root: &Component,
    plans: &ComponentPlans,
) -> Result<(), CompileError> {
    let by_id = collect_components(root);
    validate_component_tree_with_index(root, plans, &by_id)
}

pub fn compile_component_tree(
    root: &mut Component,
    plans: &ComponentPlans,
    total_bits_per_buffer: u32,
) -> Result<CompiledTree, CompileError> {
    if root.id == INVALID_NODE_ID {
        assign_node_ids(root);
    }

    let by_id = collect_components(root);
    validate_component_tree_with_index(root, plans, &by_id)?;

    let usage = collect_ref_usage(root, plans, &by_id)?;
    let alloc = ChargeAlloc::new(total_bits_per_buffer);
    let zero_bit = BitsIndex(BufferId(0), Bits(0));
    let mut compiler = GateCompiler {
        bits: HashMap::default(),
        zero_bit,
        alloc,
        mem: WorkingMem {
            mem: Vec::new(),
            bit_cross: HashMap::default(),
        },
        components: HashMap::default(),
    };

    compile_component_rec(root, plans, &[], &usage, &mut compiler)?;
    lower_cross_component_edges(root, plans, &[], &by_id, &mut compiler)?;
    let gpu_plan = lower_gpu_plan(root, plans, &by_id, &compiler)?;
    let gate_store = gate_store_map(&compiler);

    Ok(CompiledTree {
        components: compiler.components,
        gate_store,
        gpu_plan,
    })
}

fn gate_store_map(compiler: &GateCompiler) -> HashMap<(NodeId, GateId), GateStoreLocation> {
    compiler
        .bits
        .iter()
        .map(|(&(node, gate), &bit_index)| {
            let bit_in_word = (bit_index.1.0 % 32) as u8;
            (
                (node, gate),
                GateStoreLocation {
                    buffer: bit_index.0,
                    bit: bit_index.1,
                    word_byte_offset: (bit_index.1.0 >> 5) << 2,
                    bit_in_word,
                },
            )
        })
        .collect()
}

fn lower_gpu_plan(
    root: &Component,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<GpuPlan, CompileError> {
    let root_plan = component_plan(root, plans)?;
    Ok(GpuPlan {
        bits_per_buffer: compiler.alloc.total_bits,
        words_per_buffer: compiler.alloc.total_bits.div_ceil(32),
        output_words: root_plan.gate_count().div_ceil(32),
        basic_gates: lower_basic_gate_groups(root, plans, &[], by_id, compiler)?,
        cross_writes: compiler.mem.make_bit_cross(),
        output_writes: lower_output_write_groups(root, plans, compiler)?,
    })
}

fn lower_basic_gate_groups(
    root: &Component,
    plans: &ComponentPlans,
    parent_stack: &[NodeId],
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<Vec<PreparedBasicGates>, CompileError> {
    #[derive(Clone)]
    struct LoweringNode<'a> {
        node: &'a Component,
        parent_stack: Vec<NodeId>,
        child_index_in_parent: Option<ChildId>,
    }

    type BasicGateGroups =
        HashMap<crate::charge_buffer::BufferId, HashMap<u32, Vec<BasicGateInstruction>>>;

    fn collect_lowering_nodes<'a>(
        node: &'a Component,
        parent_stack: &[NodeId],
        child_index_in_parent: Option<ChildId>,
        out: &mut Vec<LoweringNode<'a>>,
    ) {
        out.push(LoweringNode {
            node,
            parent_stack: parent_stack.to_vec(),
            child_index_in_parent,
        });

        let mut next_stack = Vec::with_capacity(parent_stack.len() + 1);
        next_stack.extend_from_slice(parent_stack);
        next_stack.push(node.id);

        for (child_i, child) in node.children.iter().enumerate() {
            collect_lowering_nodes(child, &next_stack, Some(ChildId(child_i as u32)), out);
        }
    }

    fn lower_basic_gate_node(
        node: &Component,
        plans: &ComponentPlans,
        parent_stack: &[NodeId],
        by_id: &HashMap<NodeId, &Component>,
        compiler: &GateCompiler,
        child_index_in_parent: Option<ChildId>,
    ) -> Result<BasicGateGroups, CompileError> {
        let mut grouped: BasicGateGroups = HashMap::default();
        let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
        let ctx = child_ctx(parent_stack, node.id, &child_ids, child_index_in_parent);
        let plan = component_plan(node, plans)?;

        for (gate_id, gate) in plan.ordered_gates() {
            let dst = compiler.bits.get(&(node.id, gate_id)).copied().ok_or(
                CompileError::MissingBitsForGate {
                    node: node.id,
                    gate: gate_id,
                },
            )?;

            let mut inputs = gate_inputs(gate).into_iter();
            let src_a_ref = inputs
                .next()
                .expect("basic gates always have at least one input");
            let src_a = resolve_gate_bits(node, gate_id, src_a_ref, &ctx, plans, by_id, compiler)?;
            let src_b = match inputs.next() {
                Some(src_b_ref) => {
                    resolve_gate_bits(node, gate_id, src_b_ref, &ctx, plans, by_id, compiler)?
                }
                None => src_a,
            };

            let tgt_word_byte_offset = (dst.1.0 >> 5) << 2;
            grouped
                .entry(dst.0)
                .or_default()
                .entry(tgt_word_byte_offset)
                .or_default()
                .push(BasicGateInstruction {
                    op: BasicGateOp::from_gate(gate),
                    dst_bit_in_word: Bits(dst.1.0 % 32),
                    src_a,
                    src_b,
                });
        }

        Ok(grouped)
    }

    fn append_basic_gate_groups(into: &mut BasicGateGroups, from: BasicGateGroups) {
        for (buffer_id, by_word) in from {
            let buffer_groups = into.entry(buffer_id).or_default();
            for (tgt_word_byte_offset, mut instructions) in by_word {
                buffer_groups
                    .entry(tgt_word_byte_offset)
                    .or_default()
                    .append(&mut instructions);
            }
        }
    }

    fn pack_basic_gate_buffer(
        buffer_id: crate::charge_buffer::BufferId,
        mut by_word: Vec<(u32, Vec<BasicGateInstruction>)>,
    ) -> Vec<PreparedBasicGates> {
        by_word.sort_by_key(|(tgt_word_byte_offset, _)| *tgt_word_byte_offset);

        let word_count = by_word.len();
        let mut out = Vec::new();
        let mut cur = PreparedBasicGates {
            tgt_buffer: buffer_id,
            workers: Vec::with_capacity(MAX_KERNEL_WORKERS.min(word_count)),
            instructions: Vec::with_capacity(MAX_KERNEL_INSTRUCTIONS),
        };

        for (tgt_word_byte_offset, mut local) in by_word {
            local.sort_by_key(|task| task.dst_bit_in_word.0);

            let mut local_i = 0usize;
            while local_i < local.len() {
                if cur.workers.len() == MAX_KERNEL_WORKERS
                    || cur.instructions.len() == MAX_KERNEL_INSTRUCTIONS
                {
                    if !cur.workers.is_empty() || !cur.instructions.is_empty() {
                        out.push(cur);
                    }
                    cur = PreparedBasicGates {
                        tgt_buffer: buffer_id,
                        workers: Vec::with_capacity(MAX_KERNEL_WORKERS.min(word_count)),
                        instructions: Vec::with_capacity(MAX_KERNEL_INSTRUCTIONS),
                    };
                }

                let remaining_instruction_slots = MAX_KERNEL_INSTRUCTIONS - cur.instructions.len();
                let remaining_worker_slots = MAX_KERNEL_WORKERS - cur.workers.len();

                if remaining_instruction_slots == 0 || remaining_worker_slots == 0 {
                    out.push(cur);
                    cur = PreparedBasicGates {
                        tgt_buffer: buffer_id,
                        workers: Vec::with_capacity(MAX_KERNEL_WORKERS.min(word_count)),
                        instructions: Vec::with_capacity(MAX_KERNEL_INSTRUCTIONS),
                    };
                    continue;
                }

                let take = remaining_instruction_slots.min(local.len() - local_i);
                let instruction_start = cur.instructions.len() as u32;

                cur.instructions
                    .extend_from_slice(&local[local_i..local_i + take]);
                cur.workers.push(BasicGateWorker {
                    tgt_word_byte_offset,
                    instruction_start,
                    instruction_len: take as u32,
                });

                local_i += take;
            }
        }

        if !cur.workers.is_empty() || !cur.instructions.is_empty() {
            out.push(cur);
        }

        out
    }

    let mut lowering_nodes = Vec::new();
    collect_lowering_nodes(root, parent_stack, None, &mut lowering_nodes);

    let local_groups: Vec<_> = lowering_nodes
        .par_iter()
        .map(|entry| {
            lower_basic_gate_node(
                entry.node,
                plans,
                &entry.parent_stack,
                by_id,
                compiler,
                entry.child_index_in_parent,
            )
        })
        .collect();

    let mut grouped: BasicGateGroups = HashMap::default();
    for local in local_groups {
        append_basic_gate_groups(&mut grouped, local?);
    }

    let mut buffer_ids: Vec<_> = grouped.keys().copied().collect();
    buffer_ids.sort();

    let ordered_work: Vec<_> = buffer_ids
        .into_iter()
        .map(|buffer_id| {
            let by_word = grouped
                .remove(&buffer_id)
                .expect("buffer id collected from map keys")
                .into_iter()
                .collect();
            (buffer_id, by_word)
        })
        .collect();

    let ordered_chunks: Vec<_> = ordered_work
        .into_par_iter()
        .map(|(buffer_id, by_word)| pack_basic_gate_buffer(buffer_id, by_word))
        .collect();

    let out = ordered_chunks.into_iter().flatten().collect();

    Ok(out)
}

fn lower_output_write_groups(
    root: &Component,
    plans: &ComponentPlans,
    compiler: &GateCompiler,
) -> Result<Vec<PreparedOutputWrites>, CompileError> {
    let mut by_word: HashMap<u32, Vec<OutputWriteInstruction>> = HashMap::default();
    let root_plan = component_plan(root, plans)?;

    for (dense_index, gate_id) in root_plan.ordered_gate_ids().into_iter().enumerate() {
        let src = compiler.bits.get(&(root.id, gate_id)).copied().ok_or(
            CompileError::MissingBitsForGate {
                node: root.id,
                gate: gate_id,
            },
        )?;
        let dense_index = dense_index as u32;
        let output_word_index = dense_index / 32;
        by_word
            .entry(output_word_index)
            .or_default()
            .push(OutputWriteInstruction {
                src,
                dst_bit_in_word: Bits(dense_index % 32),
            });
    }

    let mut ordered_words: Vec<_> = by_word.into_iter().collect();
    ordered_words.sort_by_key(|(output_word_index, _)| *output_word_index);

    let mut out = Vec::new();
    let word_count = ordered_words.len();
    let mut cur = PreparedOutputWrites {
        workers: Vec::with_capacity(MAX_KERNEL_WORKERS.min(word_count)),
        instructions: Vec::with_capacity(MAX_KERNEL_INSTRUCTIONS),
    };

    for (output_word_index, mut local) in ordered_words {
        local.sort_by_key(|task| task.dst_bit_in_word.0);

        let mut local_i = 0usize;
        while local_i < local.len() {
            if cur.workers.len() == MAX_KERNEL_WORKERS
                || cur.instructions.len() == MAX_KERNEL_INSTRUCTIONS
            {
                if !cur.workers.is_empty() || !cur.instructions.is_empty() {
                    out.push(cur);
                }
                cur = PreparedOutputWrites {
                    workers: Vec::with_capacity(MAX_KERNEL_WORKERS.min(word_count)),
                    instructions: Vec::with_capacity(MAX_KERNEL_INSTRUCTIONS),
                };
            }

            let remaining_instruction_slots = MAX_KERNEL_INSTRUCTIONS - cur.instructions.len();
            let remaining_worker_slots = MAX_KERNEL_WORKERS - cur.workers.len();

            if remaining_instruction_slots == 0 || remaining_worker_slots == 0 {
                out.push(cur);
                cur = PreparedOutputWrites {
                    workers: Vec::with_capacity(MAX_KERNEL_WORKERS.min(word_count)),
                    instructions: Vec::with_capacity(MAX_KERNEL_INSTRUCTIONS),
                };
                continue;
            }

            let take = remaining_instruction_slots.min(local.len() - local_i);
            let instruction_start = cur.instructions.len() as u32;

            cur.instructions
                .extend_from_slice(&local[local_i..local_i + take]);
            cur.workers.push(BitCrossWorker {
                tgt_word_byte_offset: output_word_index * 4,
                instruction_start,
                instruction_len: take as u32,
            });

            local_i += take;
        }
    }

    if !cur.workers.is_empty() || !cur.instructions.is_empty() {
        out.push(cur);
    }

    Ok(out)
}

fn resolve_gate_bits(
    node: &Component,
    from_gate: GateId,
    r: SignalRef,
    ctx: &CompileCtx<'_>,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<BitsIndex, CompileError> {
    match r {
        SignalRef::Disconnected => Ok(compiler.zero_bit),
        SignalRef::ThisGate(gate) => Ok(resolved_gate_or_zero(compiler, node.id, gate)),
        SignalRef::InputPort(port) => {
            resolve_input_port_bits(node, from_gate, port, ctx, plans, by_id, compiler)
        }
        SignalRef::ChildOutput { child, port } => {
            let target_node = ctx.child_ids.get(child.0 as usize).copied().ok_or(
                CompileError::InvalidGateRef {
                    from_node: node.id,
                    from_gate,
                    bad_ref: r,
                    reason: "child does not exist from this location",
                },
            )?;
            let target = by_id
                .get(&target_node)
                .copied()
                .ok_or(CompileError::MissingNode(target_node))?;
            let target_plan = component_plan(target, plans)?;
            let output = target_plan
                .output_port(port)
                .ok_or(CompileError::MissingOutputPort {
                    node: target_node,
                    port,
                })?;
            Ok(resolved_gate_or_zero(compiler, target_node, output.gate))
        }
        SignalRef::AncestorOutput { depth, port } => {
            let depth = depth.0 as usize;
            if depth == 0 || depth > ctx.parent_stack.len() {
                return Err(CompileError::InvalidGateRef {
                    from_node: node.id,
                    from_gate,
                    bad_ref: r,
                    reason: "ancestor does not exist from this location",
                });
            }
            let target_node = ctx.parent_stack[ctx.parent_stack.len() - depth];
            let target = by_id
                .get(&target_node)
                .copied()
                .ok_or(CompileError::MissingNode(target_node))?;
            let target_plan = component_plan(target, plans)?;
            let output = target_plan
                .output_port(port)
                .ok_or(CompileError::MissingOutputPort {
                    node: target_node,
                    port,
                })?;
            Ok(resolved_gate_or_zero(compiler, target_node, output.gate))
        }
    }
}

fn resolved_gate_or_zero(compiler: &GateCompiler, node: NodeId, gate: GateId) -> BitsIndex {
    compiler
        .bits
        .get(&(node, gate))
        .copied()
        .unwrap_or(compiler.zero_bit)
}

fn resolve_input_port_bits(
    node: &Component,
    from_gate: GateId,
    port: PortId,
    ctx: &CompileCtx<'_>,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<BitsIndex, CompileError> {
    let parent_id = match ctx.parent_stack.last().copied() {
        Some(parent_id) => parent_id,
        None => {
            let plan = component_plan(node, plans)?;
            let input = plan
                .input_port(port)
                .ok_or(CompileError::MissingInputPort {
                    node: node.id,
                    port,
                })?;
            return Ok(resolved_gate_or_zero(compiler, node.id, input.gate));
        }
    };

    let child_index = ctx
        .child_index_in_parent
        .ok_or(CompileError::InvalidGateRef {
            from_node: node.id,
            from_gate,
            bad_ref: SignalRef::InputPort(port),
            reason: "component input ports require a parent child slot",
        })?;

    let parent = by_id
        .get(&parent_id)
        .copied()
        .ok_or(CompileError::MissingNode(parent_id))?;

    let Some(connection) = child_input_connection(parent, child_index, port) else {
        let plan = component_plan(node, plans)?;
        let input = plan
            .input_port(port)
            .ok_or(CompileError::MissingInputPort {
                node: node.id,
                port,
            })?;
        // TODO(UI): surface disconnected component inputs so the player can fix them.
        return Ok(resolved_gate_or_zero(compiler, node.id, input.gate));
    };

    let parent_child_ids: Vec<NodeId> = parent.children.iter().map(|c| c.id).collect();
    let parent_ctx = child_ctx(
        &ctx.parent_stack[..ctx.parent_stack.len() - 1],
        parent.id,
        &parent_child_ids,
        child_index_in_parent(
            parent.id,
            &ctx.parent_stack[..ctx.parent_stack.len() - 1],
            by_id,
        )?,
    );
    resolve_gate_bits(
        parent,
        from_gate,
        connection.src,
        &parent_ctx,
        plans,
        by_id,
        compiler,
    )
}

impl BasicGateOp {
    fn from_gate(gate: Gate) -> Self {
        match gate {
            Gate::BitNAND { .. } => Self::BitNAND,
            Gate::BitAND { .. } => Self::BitAND,
            Gate::BitOR { .. } => Self::BitOR,
            Gate::BitNOR { .. } => Self::BitNOR,
            Gate::BitXOR { .. } => Self::BitXOR,
            Gate::BitXNOR { .. } => Self::BitXNOR,
            Gate::BitNot { .. } => Self::BitNot,
            Gate::BitNop { .. } => Self::BitNop,
        }
    }
}

fn collect_components<'a>(root: &'a Component) -> HashMap<NodeId, &'a Component> {
    fn rec<'a>(node: &'a Component, out: &mut HashMap<NodeId, &'a Component>) {
        out.insert(node.id, node);
        for child in &node.children {
            rec(child, out);
        }
    }

    let mut out = HashMap::default();
    rec(root, &mut out);
    out
}

fn validate_component_tree_with_index(
    root: &Component,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(), CompileError> {
    fn rec(
        node: &Component,
        plans: &ComponentPlans,
        parent_stack: &mut Vec<NodeId>,
        by_id: &HashMap<NodeId, &Component>,
    ) -> Result<(), CompileError> {
        rec_with_child_index(node, plans, parent_stack, by_id, None)
    }

    fn rec_with_child_index(
        node: &Component,
        plans: &ComponentPlans,
        parent_stack: &mut Vec<NodeId>,
        by_id: &HashMap<NodeId, &Component>,
        child_index: Option<ChildId>,
    ) -> Result<(), CompileError> {
        let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
        let ctx = child_ctx(parent_stack, node.id, &child_ids, child_index);
        let plan = component_plan(node, plans)?;
        validate_plan_ports(node.id, plan)?;

        let mut seen_connections = HashSet::default();
        for connection in &node.child_input_connections {
            if !seen_connections.insert((connection.child, connection.input)) {
                return Err(CompileError::DuplicateChildInputConnection {
                    node: node.id,
                    child: connection.child,
                    input: connection.input,
                });
            }

            let child_node = child_ids.get(connection.child.0 as usize).copied().ok_or(
                CompileError::InvalidChildInputConnection {
                    node: node.id,
                    child: connection.child,
                    input: connection.input,
                    reason: "child does not exist",
                },
            )?;
            let child = by_id
                .get(&child_node)
                .copied()
                .ok_or(CompileError::MissingNode(child_node))?;
            let child_plan = component_plan(child, plans)?;
            child_plan.input_port(connection.input).ok_or(
                CompileError::InvalidChildInputConnection {
                    node: node.id,
                    child: connection.child,
                    input: connection.input,
                    reason: "child input port does not exist",
                },
            )?;
            validate_gate_ref(
                node.id,
                GateId(u32::MAX),
                connection.src,
                &ctx,
                plans,
                by_id,
            )?;
        }

        for (gate_id, gate) in plan.ordered_gates() {
            validate_gate(node.id, gate_id, gate, &ctx, plans, by_id)?;
        }

        parent_stack.push(node.id);
        for (child_i, child) in node.children.iter().enumerate() {
            rec_with_child_index(
                child,
                plans,
                parent_stack,
                by_id,
                Some(ChildId(child_i as u32)),
            )?;
        }
        parent_stack.pop();

        Ok(())
    }

    let mut stack = Vec::new();
    rec(root, plans, &mut stack, by_id)
}

fn validate_gate(
    node_id: NodeId,
    gate_id: GateId,
    gate: Gate,
    ctx: &CompileCtx<'_>,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(), CompileError> {
    match gate {
        Gate::BitNAND { a, b }
        | Gate::BitAND { a, b }
        | Gate::BitOR { a, b }
        | Gate::BitNOR { a, b }
        | Gate::BitXOR { a, b }
        | Gate::BitXNOR { a, b } => {
            validate_gate_ref(node_id, gate_id, a, ctx, plans, by_id)?;
            validate_gate_ref(node_id, gate_id, b, ctx, plans, by_id)?;
        }
        Gate::BitNot { src } | Gate::BitNop { src } => {
            validate_gate_ref(node_id, gate_id, src, ctx, plans, by_id)?;
        }
    }
    Ok(())
}

fn validate_gate_ref(
    from_node: NodeId,
    from_gate: GateId,
    r: SignalRef,
    ctx: &CompileCtx<'_>,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(), CompileError> {
    match r {
        SignalRef::Disconnected => Ok(()),
        SignalRef::ThisGate(_) => Ok(()),
        SignalRef::InputPort(port) => {
            let node = by_id
                .get(&ctx.current)
                .copied()
                .ok_or(CompileError::MissingNode(ctx.current))?;
            let plan = component_plan(node, plans)?;
            plan.input_port(port)
                .ok_or(CompileError::MissingInputPort {
                    node: ctx.current,
                    port,
                })?;
            Ok(())
        }
        SignalRef::ChildOutput { child, port } => {
            let target_node = ctx.child_ids.get(child.0 as usize).copied().ok_or(
                CompileError::InvalidGateRef {
                    from_node,
                    from_gate,
                    bad_ref: r,
                    reason: "child does not exist from this location",
                },
            )?;
            let target = by_id
                .get(&target_node)
                .copied()
                .ok_or(CompileError::MissingNode(target_node))?;
            let target_plan = component_plan(target, plans)?;
            target_plan
                .output_port(port)
                .ok_or(CompileError::MissingOutputPort {
                    node: target_node,
                    port,
                })?;
            Ok(())
        }
        SignalRef::AncestorOutput { depth, port } => {
            let depth = depth.0 as usize;
            if depth == 0 || depth > ctx.parent_stack.len() {
                Err(CompileError::InvalidGateRef {
                    from_node,
                    from_gate,
                    bad_ref: r,
                    reason: "ancestor does not exist from this location",
                })
            } else {
                let target_node = ctx.parent_stack[ctx.parent_stack.len() - depth];
                let target = by_id
                    .get(&target_node)
                    .copied()
                    .ok_or(CompileError::MissingNode(target_node))?;
                let target_plan = component_plan(target, plans)?;
                target_plan
                    .output_port(port)
                    .ok_or(CompileError::MissingOutputPort {
                        node: target_node,
                        port,
                    })?;
                Ok(())
            }
        }
    }
}

fn collect_ref_usage(
    root: &Component,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<HashMap<NodeId, RefUsage>, CompileError> {
    fn rec(
        node: &Component,
        plans: &ComponentPlans,
        parent_stack: &mut Vec<NodeId>,
        by_id: &HashMap<NodeId, &Component>,
        usage: &mut HashMap<NodeId, RefUsage>,
    ) -> Result<(), CompileError> {
        rec_with_child_index(node, plans, parent_stack, by_id, usage, None)
    }

    fn rec_with_child_index(
        node: &Component,
        plans: &ComponentPlans,
        parent_stack: &mut Vec<NodeId>,
        by_id: &HashMap<NodeId, &Component>,
        usage: &mut HashMap<NodeId, RefUsage>,
        child_index: Option<ChildId>,
    ) -> Result<(), CompileError> {
        let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
        let ctx = child_ctx(parent_stack, node.id, &child_ids, child_index);
        let plan = component_plan(node, plans)?;

        for (from_gate, gate) in plan.ordered_gates() {
            for r in gate_inputs(gate) {
                validate_gate_ref(node.id, from_gate, r, &ctx, plans, by_id)?;

                match r {
                    SignalRef::Disconnected => {}
                    SignalRef::ThisGate(_) => {}
                    SignalRef::InputPort(port) => {
                        usage
                            .entry(node.id)
                            .or_default()
                            .input_ports_read
                            .insert(port);
                    }
                    SignalRef::ChildOutput { child, port } => {
                        let target_node = child_ids[child.0 as usize];
                        usage
                            .entry(target_node)
                            .or_default()
                            .output_ports_read_by_parent
                            .insert(port);
                    }
                    SignalRef::AncestorOutput { depth, port } => {
                        let depth = depth.0 as usize;
                        let target_node = parent_stack[parent_stack.len() - depth];
                        usage
                            .entry(target_node)
                            .or_default()
                            .output_ports_read_by_parent
                            .insert(port);
                    }
                }
            }
        }

        for connection in &node.child_input_connections {
            validate_gate_ref(
                node.id,
                GateId(u32::MAX),
                connection.src,
                &ctx,
                plans,
                by_id,
            )?;
            match connection.src {
                SignalRef::Disconnected => {}
                SignalRef::ThisGate(_) => {}
                SignalRef::InputPort(port) => {
                    usage
                        .entry(node.id)
                        .or_default()
                        .input_ports_read
                        .insert(port);
                }
                SignalRef::ChildOutput { child, port } => {
                    let target_node = child_ids[child.0 as usize];
                    usage
                        .entry(target_node)
                        .or_default()
                        .output_ports_read_by_parent
                        .insert(port);
                }
                SignalRef::AncestorOutput { depth, port } => {
                    let depth = depth.0 as usize;
                    let target_node = parent_stack[parent_stack.len() - depth];
                    usage
                        .entry(target_node)
                        .or_default()
                        .output_ports_read_by_parent
                        .insert(port);
                }
            }
        }

        parent_stack.push(node.id);
        for (child_i, child) in node.children.iter().enumerate() {
            rec_with_child_index(
                child,
                plans,
                parent_stack,
                by_id,
                usage,
                Some(ChildId(child_i as u32)),
            )?;
        }
        parent_stack.pop();

        Ok(())
    }

    let mut usage: HashMap<NodeId, RefUsage> = HashMap::default();
    let mut stack = Vec::new();
    rec(root, plans, &mut stack, by_id, &mut usage)?;
    Ok(usage)
}

fn compile_component_rec(
    node: &Component,
    plans: &ComponentPlans,
    parent_stack: &[NodeId],
    usage: &HashMap<NodeId, RefUsage>,
    compiler: &mut GateCompiler,
) -> Result<(), CompileError> {
    let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
    let _ctx = child_ctx(parent_stack, node.id, &child_ids, None);
    let plan = component_plan(node, plans)?;

    let outline = decide_outline_plan(plan.gate_count(), usage.get(&node.id));

    let ordered_gate_ids = plan.ordered_gate_ids();
    let mut self_bits = Vec::with_capacity(ordered_gate_ids.len());

    match outline.layout {
        CompileLayout::Inline => {
            for gate_id in ordered_gate_ids.iter().copied() {
                let bit = compiler.alloc.alloc_bit();
                compiler.bits.insert((node.id, gate_id), bit);
                self_bits.push(bit);
            }
        }
        CompileLayout::Outline => {
            let total_slots = plan.gate_count() + outline.input_count + outline.output_count;
            for gate_id in ordered_gate_ids.iter().copied() {
                let word = compiler.alloc.alloc_word();
                let bit = BitsIndex(word.0, Bits(word.1 * 8));
                compiler.bits.insert((node.id, gate_id), bit);
                self_bits.push(bit);
            }
            for _ in ordered_gate_ids.len() as u32..total_slots {
                let _ = compiler.alloc.alloc_word();
            }
        }
    }

    compiler.components.insert(
        node.id,
        CompiledComponentInfo {
            node: node.id,
            self_bits,
            child_ids: child_ids.clone(),
            outline,
        },
    );

    let mut next_stack = Vec::with_capacity(parent_stack.len() + 1);
    next_stack.extend_from_slice(parent_stack);
    next_stack.push(node.id);

    for child in &node.children {
        compile_component_rec(child, plans, &next_stack, usage, compiler)?;
    }

    Ok(())
}

fn lower_cross_component_edges(
    node: &Component,
    plans: &ComponentPlans,
    parent_stack: &[NodeId],
    by_id: &HashMap<NodeId, &Component>,
    compiler: &mut GateCompiler,
) -> Result<(), CompileError> {
    let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
    let ctx = child_ctx(parent_stack, node.id, &child_ids, None);

    for connection in &node.child_input_connections {
        let child_node = child_ids.get(connection.child.0 as usize).copied().ok_or(
            CompileError::InvalidChildInputConnection {
                node: node.id,
                child: connection.child,
                input: connection.input,
                reason: "child does not exist",
            },
        )?;
        let child = by_id
            .get(&child_node)
            .copied()
            .ok_or(CompileError::MissingNode(child_node))?;
        let child_plan = component_plan(child, plans)?;
        let input =
            child_plan
                .input_port(connection.input)
                .ok_or(CompileError::MissingInputPort {
                    node: child_node,
                    port: connection.input,
                })?;

        let src = resolve_gate_bits(
            node,
            input.gate,
            connection.src,
            &ctx,
            plans,
            by_id,
            compiler,
        )?;
        let dst = resolved_gate_or_zero(compiler, child_node, input.gate);

        if dst != compiler.zero_bit && (src.0 != dst.0 || src.1 != dst.1) {
            let _ = compiler.mem.queue_bit_write(src, dst);
        }
    }

    let mut next_stack = Vec::with_capacity(parent_stack.len() + 1);
    next_stack.extend_from_slice(parent_stack);
    next_stack.push(node.id);

    for child in &node.children {
        lower_cross_component_edges(child, plans, &next_stack, by_id, compiler)?;
    }

    Ok(())
}

fn decide_outline_plan(gate_count: u32, usage: Option<&RefUsage>) -> OutlinePlan {
    let usage = usage.cloned().unwrap_or_default();

    let input_count = usage.input_ports_read.len() as u32;
    let output_count = usage.output_ports_read_by_parent.len() as u32;

    let inline_cost_bits = gate_count;
    let outlined_cost_bits_worst = gate_count.saturating_mul(32)
        + input_count.saturating_mul(32)
        + output_count.saturating_mul(32);

    let layout = if outlined_cost_bits_worst <= inline_cost_bits {
        CompileLayout::Outline
    } else {
        CompileLayout::Inline
    };

    OutlinePlan {
        input_count,
        output_count,
        extra_bits_needed: input_count + output_count,
        layout,
    }
}

fn gate_inputs(gate: Gate) -> SmallGateInputs {
    match gate {
        Gate::BitNAND { a, b }
        | Gate::BitAND { a, b }
        | Gate::BitOR { a, b }
        | Gate::BitNOR { a, b }
        | Gate::BitXOR { a, b }
        | Gate::BitXNOR { a, b } => SmallGateInputs {
            refs: [Some(a), Some(b)],
        },
        Gate::BitNot { src } | Gate::BitNop { src } => SmallGateInputs {
            refs: [Some(src), None],
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct SmallGateInputs {
    refs: [Option<SignalRef>; 2],
}

impl IntoIterator for SmallGateInputs {
    type Item = SignalRef;
    type IntoIter = SmallGateInputsIter;

    fn into_iter(self) -> Self::IntoIter {
        SmallGateInputsIter {
            refs: self.refs,
            i: 0,
        }
    }
}

struct SmallGateInputsIter {
    refs: [Option<SignalRef>; 2],
    i: usize,
}

impl Iterator for SmallGateInputsIter {
    type Item = SignalRef;

    fn next(&mut self) -> Option<Self::Item> {
        while self.i < self.refs.len() {
            let out = self.refs[self.i];
            self.i += 1;
            if let Some(r) = out {
                return Some(r);
            }
        }
        None
    }
}

pub fn compile_gates(
    root: &mut Component,
    plans: &ComponentPlans,
    total_bits_per_buffer: u32,
) -> Result<CompiledTree, CompileError> {
    compile_component_tree(root, plans, total_bits_per_buffer)
}

// TODO:
// - assign explicit outlined input/output slots instead of only counting them
// - make outlining cost depend on actual cross-buffer traffic, not just worst-case space
// - lower gate execution into real shader-side instruction streams
// - make parent/child interfaces explicit instead of inferring only from raw refs
// - add byte/word level refs later instead of forcing everything through bits

#[cfg(test)]
mod tests {
    use super::*;
    use crate::charge_buffer::{BufferId, WordIndex};

    const INPUT_A: PortId = PortId(10);
    const OUTPUT_Z: PortId = PortId(20);

    fn this_ref(gate: u32) -> SignalRef {
        SignalRef::ThisGate(GateId(gate))
    }

    fn input_ref(port: PortId) -> SignalRef {
        SignalRef::InputPort(port)
    }

    fn child_output_ref(child: u32, port: PortId) -> SignalRef {
        SignalRef::ChildOutput {
            child: ChildId(child),
            port,
        }
    }

    fn port(id: PortId, gate: u32, x: u16, y: u16) -> ComponentPort {
        ComponentPort {
            id,
            gate: GateId(gate),
            location: PortLocation { x, y },
            label: None,
        }
    }

    fn compile_with_state(
        root: &mut Component,
        plans: &ComponentPlans,
        total_bits_per_buffer: u32,
    ) -> Result<GateCompiler, CompileError> {
        if root.id == INVALID_NODE_ID {
            assign_node_ids(root);
        }

        let by_id = collect_components(root);
        validate_component_tree_with_index(root, plans, &by_id)?;

        let usage = collect_ref_usage(root, plans, &by_id)?;
        let mut compiler = GateCompiler {
            bits: HashMap::default(),
            zero_bit: BitsIndex(BufferId(0), Bits(0)),
            alloc: ChargeAlloc::new(total_bits_per_buffer),
            mem: WorkingMem {
                mem: Vec::new(),
                bit_cross: HashMap::default(),
            },
            components: HashMap::default(),
        };

        compile_component_rec(root, plans, &[], &usage, &mut compiler)?;
        lower_cross_component_edges(root, plans, &[], &by_id, &mut compiler)?;
        Ok(compiler)
    }

    fn make_large_inline_component(plans: &mut ComponentPlans, gate_count: u32) -> Component {
        let gates = (0..gate_count)
            .map(|gate| Gate::BitNop {
                src: this_ref(gate),
            })
            .collect();
        Component::from_gates(plans, gates, vec![])
    }

    fn make_port_child(plans: &mut ComponentPlans) -> Component {
        let plan = ComponentPlan::with_ports(
            vec![
                Gate::BitNop {
                    src: input_ref(INPUT_A),
                },
                Gate::BitNot { src: this_ref(0) },
            ],
            vec![port(INPUT_A, 0, 0, u16::MAX)],
            vec![port(OUTPUT_Z, 1, u16::MAX, u16::MAX)],
        );
        Component::from_plan(plans, plan, vec![])
    }

    #[test]
    fn oversized_inline_component_spills_across_buffers_without_out_of_bounds_bits() {
        let mut plans = ComponentPlans::new();
        let mut root = make_large_inline_component(&mut plans, 40);

        let compiled =
            compile_component_tree(&mut root, &plans, 32).expect("component should compile");
        let info = compiled
            .components
            .get(&root.id)
            .expect("root should have compiled info");

        assert_eq!(info.outline.layout, CompileLayout::Inline);
        assert_eq!(info.self_bits.len(), 40);
        assert!(info.self_bits.iter().all(|bit| bit.1.0 < 32));
        assert!(
            info.self_bits
                .iter()
                .map(|bit| bit.0)
                .collect::<HashSet<_>>()
                .len()
                > 1
        );
    }

    #[test]
    fn child_input_connections_queue_cross_writes_into_existing_input_gate_bits() {
        let mut plans = ComponentPlans::new();
        let child = make_port_child(&mut plans);
        let root_plan = ComponentPlan::with_ports(
            (0..32)
                .map(|gate| Gate::BitNop {
                    src: this_ref(gate),
                })
                .collect(),
            vec![],
            vec![port(OUTPUT_Z, 5, 10, 10)],
        );
        let mut root = Component::with_child_input_connections(
            plans.insert(root_plan),
            vec![child],
            vec![ChildInputConnection {
                child: ChildId(0),
                input: INPUT_A,
                src: this_ref(5),
            }],
        );

        let compiler = compile_with_state(&mut root, &plans, 32).expect("tree should compile");
        let root_id = root.id;
        let child_id = root.children[0].id;
        let src = compiler.bits[&(root_id, GateId(5))];
        let dst = compiler.bits[&(child_id, GateId(0))];

        assert_eq!(src, BitsIndex(BufferId(1), Bits(5)));
        assert_eq!(dst, BitsIndex(BufferId(2), Bits(0)));

        let queued = compiler
            .mem
            .bit_cross
            .get(&(BufferId(1), WordIndex(BufferId(2), 0)))
            .expect("expected cross-buffer child input write");
        assert!(queued.contains(&(Bits(5), 0)));
    }

    #[test]
    fn missing_child_input_connection_is_left_detached() {
        let mut plans = ComponentPlans::new();
        let child = make_port_child(&mut plans);
        let mut root = Component::from_gates(
            &mut plans,
            (0..32)
                .map(|gate| Gate::BitNop {
                    src: this_ref(gate),
                })
                .collect(),
            vec![child],
        );

        let compiler = compile_with_state(&mut root, &plans, 32).expect("tree should compile");
        assert!(compiler.mem.bit_cross.is_empty());
    }

    #[test]
    fn child_output_refs_use_stable_port_ids_not_output_order() {
        let mut plans = ComponentPlans::new();
        let child = Component::from_plan(
            &mut plans,
            ComponentPlan::with_ports(
                vec![
                    Gate::BitNop { src: this_ref(0) },
                    Gate::BitNot { src: this_ref(0) },
                ],
                vec![],
                vec![
                    port(PortId(2), 0, 1, 1),
                    port(OUTPUT_Z, 1, u16::MAX, u16::MAX),
                ],
            ),
            vec![],
        );
        let root_plan = ComponentPlan::new(vec![Gate::BitNop {
            src: child_output_ref(0, OUTPUT_Z),
        }]);
        let mut root = Component::from_plan(&mut plans, root_plan, vec![child]);

        let compiled = compile_component_tree(&mut root, &plans, 64).expect("tree should compile");
        let instruction = &compiled.gpu_plan.basic_gates[0].instructions[0];
        let child_output_bit = compiled.components[&root.children[0].id].self_bits[1];
        assert_eq!(instruction.src_a, child_output_bit);
    }

    #[test]
    fn basic_gate_plan_groups_tasks_by_target_word() {
        let gates = vec![
            Gate::BitNop { src: this_ref(0) },
            Gate::BitNot { src: this_ref(0) },
            Gate::BitAND {
                a: this_ref(0),
                b: this_ref(1),
            },
            Gate::BitOR {
                a: this_ref(0),
                b: this_ref(1),
            },
            Gate::BitNAND {
                a: this_ref(0),
                b: this_ref(1),
            },
            Gate::BitNOR {
                a: this_ref(0),
                b: this_ref(1),
            },
            Gate::BitXOR {
                a: this_ref(0),
                b: this_ref(1),
            },
            Gate::BitXNOR {
                a: this_ref(0),
                b: this_ref(1),
            },
        ];

        let mut plans = ComponentPlans::new();
        let mut root = Component::from_gates(&mut plans, gates, vec![]);
        let compiled =
            compile_component_tree(&mut root, &plans, 64).expect("component should compile");

        assert_eq!(compiled.gpu_plan.basic_gates.len(), 1);
        assert_eq!(compiled.gpu_plan.output_writes[0].instructions.len(), 8);
    }
}
