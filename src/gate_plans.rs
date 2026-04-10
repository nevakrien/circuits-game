use crate::charge_buffer::{
    BitCrossWorker, Bits, BitsIndex, ChargeAlloc, PreparedBitCross, WorkingMem,
};
use foldhash::{HashMap, HashSet};

const MAX_KERNEL_INSTRUCTIONS: usize = 1 << 8;
const MAX_KERNEL_WORKERS: usize = 1 << 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GateId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChildId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AncestorDepth(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopeRef {
    This,
    Parent,
    Child(ChildId),
    Ancestor(AncestorDepth),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GateRef {
    pub scope: ScopeRef,
    pub gate: GateId,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gate {
    BitNAND { a: GateRef, b: GateRef } = 1,
    BitAND { a: GateRef, b: GateRef },
    BitOR { a: GateRef, b: GateRef },
    BitNOR { a: GateRef, b: GateRef },
    BitXOR { a: GateRef, b: GateRef },
    BitXNOR { a: GateRef, b: GateRef },
    BitNot { src: GateRef },
    BitNop { src: GateRef },
}

#[derive(Debug, Clone)]
pub struct Component {
    pub id: NodeId,
    pub gates: Vec<Gate>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(gates: Vec<Gate>, children: Vec<Component>) -> Self {
        Self {
            id: INVALID_NODE_ID,
            gates,
            children,
        }
    }

    pub fn gate_count(&self) -> u32 {
        self.gates.len() as u32
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

#[derive(Debug, Clone)]
pub struct CompiledTree {
    pub components: HashMap<NodeId, CompiledComponentInfo>,
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
    pub alloc: ChargeAlloc,
    pub mem: WorkingMem,
    pub components: HashMap<NodeId, CompiledComponentInfo>,
}

#[derive(Debug, Clone)]
pub struct RefUsage {
    pub read_by_parent: HashSet<GateId>,
    pub written_by_parent: HashSet<GateId>,
    pub read_by_children: HashMap<ChildId, HashSet<GateId>>,
    pub written_by_children: HashMap<ChildId, HashSet<GateId>>,
}

impl Default for RefUsage {
    fn default() -> Self {
        Self {
            read_by_parent: HashSet::default(),
            written_by_parent: HashSet::default(),
            read_by_children: HashMap::default(),
            written_by_children: HashMap::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompileError {
    MissingNode(NodeId),
    MissingCompiledInfo(NodeId),
    InvalidGateRef {
        from_node: NodeId,
        from_gate: GateId,
        bad_ref: GateRef,
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
}

#[derive(Debug, Clone, Copy)]
struct CompileCtx<'a> {
    current: NodeId,
    parent_stack: &'a [NodeId],
    child_ids: &'a [NodeId],
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

pub fn validate_component_tree(root: &Component) -> Result<(), CompileError> {
    let by_id = collect_components(root);
    validate_component_tree_with_index(root, &by_id)
}

pub fn compile_component_tree(
    root: &mut Component,
    total_bits_per_buffer: u32,
) -> Result<CompiledTree, CompileError> {
    if root.id == INVALID_NODE_ID {
        assign_node_ids(root);
    }

    let by_id = collect_components(root);
    validate_component_tree_with_index(root, &by_id)?;

    let usage = collect_ref_usage(root, &by_id)?;
    let mut compiler = GateCompiler {
        bits: HashMap::default(),
        alloc: ChargeAlloc::new(total_bits_per_buffer),
        mem: WorkingMem {
            mem: Vec::new(),
            bit_cross: HashMap::default(),
        },
        components: HashMap::default(),
    };

    compile_component_rec(root, &[], &usage, &mut compiler)?;
    lower_cross_component_edges(root, &[], &by_id, &mut compiler)?;
    let gpu_plan = lower_gpu_plan(root, &by_id, &compiler)?;

    Ok(CompiledTree {
        components: compiler.components,
        gpu_plan,
    })
}

fn lower_gpu_plan(
    root: &Component,
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<GpuPlan, CompileError> {
    Ok(GpuPlan {
        bits_per_buffer: compiler.alloc.total_bits,
        words_per_buffer: compiler.alloc.total_bits.div_ceil(32),
        output_words: root.gate_count().div_ceil(32),
        basic_gates: lower_basic_gate_groups(root, &[], by_id, compiler)?,
        cross_writes: compiler.mem.make_bit_cross(),
        output_writes: lower_output_write_groups(root, compiler)?,
    })
}

fn lower_basic_gate_groups(
    root: &Component,
    parent_stack: &[NodeId],
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<Vec<PreparedBasicGates>, CompileError> {
    fn rec(
        node: &Component,
        parent_stack: &[NodeId],
        by_id: &HashMap<NodeId, &Component>,
        compiler: &GateCompiler,
        grouped: &mut HashMap<
            crate::charge_buffer::BufferId,
            HashMap<u32, Vec<BasicGateInstruction>>,
        >,
    ) -> Result<(), CompileError> {
        let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
        let ctx = CompileCtx {
            current: node.id,
            parent_stack,
            child_ids: &child_ids,
        };

        for (gate_i, gate) in node.gates.iter().copied().enumerate() {
            let gate_id = GateId(gate_i as u32);
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
            let src_a = resolve_gate_bits(node.id, gate_id, src_a_ref, &ctx, by_id, compiler)?;
            let src_b = match inputs.next() {
                Some(src_b_ref) => {
                    resolve_gate_bits(node.id, gate_id, src_b_ref, &ctx, by_id, compiler)?
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

        let mut next_stack = Vec::with_capacity(parent_stack.len() + 1);
        next_stack.extend_from_slice(parent_stack);
        next_stack.push(node.id);

        for child in &node.children {
            rec(child, &next_stack, by_id, compiler, grouped)?;
        }

        Ok(())
    }

    let mut grouped: HashMap<
        crate::charge_buffer::BufferId,
        HashMap<u32, Vec<BasicGateInstruction>>,
    > = HashMap::default();
    rec(root, parent_stack, by_id, compiler, &mut grouped)?;

    let mut buffer_ids: Vec<_> = grouped.keys().copied().collect();
    buffer_ids.sort();

    let mut out = Vec::new();

    for buffer_id in buffer_ids {
        let mut by_word: Vec<(u32, Vec<BasicGateInstruction>)> = grouped
            .remove(&buffer_id)
            .expect("buffer id collected from map keys")
            .into_iter()
            .collect();
        by_word.sort_by_key(|(tgt_word_byte_offset, _)| *tgt_word_byte_offset);

        let word_count = by_word.len();
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
    }

    Ok(out)
}

fn lower_output_write_groups(
    root: &Component,
    compiler: &GateCompiler,
) -> Result<Vec<PreparedOutputWrites>, CompileError> {
    let mut by_word: HashMap<u32, Vec<OutputWriteInstruction>> = HashMap::default();

    for (gate_i, _) in root.gates.iter().enumerate() {
        let gate_id = GateId(gate_i as u32);
        let src = compiler.bits.get(&(root.id, gate_id)).copied().ok_or(
            CompileError::MissingBitsForGate {
                node: root.id,
                gate: gate_id,
            },
        )?;
        let output_word_index = gate_id.0 / 32;
        by_word
            .entry(output_word_index)
            .or_default()
            .push(OutputWriteInstruction {
                src,
                dst_bit_in_word: Bits(gate_id.0 % 32),
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
    from_node: NodeId,
    from_gate: GateId,
    r: GateRef,
    ctx: &CompileCtx<'_>,
    by_id: &HashMap<NodeId, &Component>,
    compiler: &GateCompiler,
) -> Result<BitsIndex, CompileError> {
    let target_node = resolve_scope(ctx, r.scope).ok_or(CompileError::InvalidGateRef {
        from_node,
        from_gate,
        bad_ref: r,
        reason: "scope does not exist from this location",
    })?;

    let target = by_id
        .get(&target_node)
        .copied()
        .ok_or(CompileError::MissingNode(target_node))?;

    if r.gate.0 >= target.gates.len() as u32 {
        return Err(CompileError::TargetGateOutOfRange {
            from_node,
            from_gate,
            target_node,
            target_gate: r.gate,
            target_gate_count: target.gates.len() as u32,
        });
    }

    compiler
        .bits
        .get(&(target_node, r.gate))
        .copied()
        .ok_or(CompileError::MissingBitsForGate {
            node: target_node,
            gate: r.gate,
        })
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
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(), CompileError> {
    fn rec(
        node: &Component,
        parent_stack: &mut Vec<NodeId>,
        by_id: &HashMap<NodeId, &Component>,
    ) -> Result<(), CompileError> {
        let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
        let ctx = CompileCtx {
            current: node.id,
            parent_stack,
            child_ids: &child_ids,
        };

        for (gate_i, gate) in node.gates.iter().copied().enumerate() {
            let gate_id = GateId(gate_i as u32);
            validate_gate(node.id, gate_id, gate, &ctx, by_id)?;
        }

        parent_stack.push(node.id);
        for child in &node.children {
            rec(child, parent_stack, by_id)?;
        }
        parent_stack.pop();

        Ok(())
    }

    let mut stack = Vec::new();
    rec(root, &mut stack, by_id)
}

fn validate_gate(
    node_id: NodeId,
    gate_id: GateId,
    gate: Gate,
    ctx: &CompileCtx<'_>,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(), CompileError> {
    match gate {
        Gate::BitNAND { a, b }
        | Gate::BitAND { a, b }
        | Gate::BitOR { a, b }
        | Gate::BitNOR { a, b }
        | Gate::BitXOR { a, b }
        | Gate::BitXNOR { a, b } => {
            validate_gate_ref(node_id, gate_id, a, ctx, by_id)?;
            validate_gate_ref(node_id, gate_id, b, ctx, by_id)?;
        }
        Gate::BitNot { src } | Gate::BitNop { src } => {
            validate_gate_ref(node_id, gate_id, src, ctx, by_id)?;
        }
    }
    Ok(())
}

fn validate_gate_ref(
    from_node: NodeId,
    from_gate: GateId,
    r: GateRef,
    ctx: &CompileCtx<'_>,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(), CompileError> {
    let target_node = resolve_scope(ctx, r.scope).ok_or(CompileError::InvalidGateRef {
        from_node,
        from_gate,
        bad_ref: r,
        reason: "scope does not exist from this location",
    })?;

    let target = by_id
        .get(&target_node)
        .copied()
        .ok_or(CompileError::MissingNode(target_node))?;

    if r.gate.0 >= target.gates.len() as u32 {
        return Err(CompileError::TargetGateOutOfRange {
            from_node,
            from_gate,
            target_node,
            target_gate: r.gate,
            target_gate_count: target.gates.len() as u32,
        });
    }

    Ok(())
}

fn resolve_scope(ctx: &CompileCtx<'_>, scope: ScopeRef) -> Option<NodeId> {
    match scope {
        ScopeRef::This => Some(ctx.current),
        ScopeRef::Parent => ctx.parent_stack.last().copied(),
        ScopeRef::Child(child_id) => ctx.child_ids.get(child_id.0 as usize).copied(),
        ScopeRef::Ancestor(depth) => {
            let depth = depth.0 as usize;
            if depth == 0 || depth > ctx.parent_stack.len() {
                None
            } else {
                Some(ctx.parent_stack[ctx.parent_stack.len() - depth])
            }
        }
    }
}

fn collect_ref_usage(
    root: &Component,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<HashMap<NodeId, RefUsage>, CompileError> {
    fn rec(
        node: &Component,
        parent_stack: &mut Vec<NodeId>,
        by_id: &HashMap<NodeId, &Component>,
        usage: &mut HashMap<NodeId, RefUsage>,
    ) -> Result<(), CompileError> {
        let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
        let ctx = CompileCtx {
            current: node.id,
            parent_stack,
            child_ids: &child_ids,
        };

        for (gate_i, gate) in node.gates.iter().copied().enumerate() {
            let from_gate = GateId(gate_i as u32);

            for r in gate_inputs(gate) {
                let target_node =
                    resolve_scope(&ctx, r.scope).ok_or(CompileError::InvalidGateRef {
                        from_node: node.id,
                        from_gate,
                        bad_ref: r,
                        reason: "scope does not exist from this location",
                    })?;

                let target = by_id
                    .get(&target_node)
                    .copied()
                    .ok_or(CompileError::MissingNode(target_node))?;

                if r.gate.0 >= target.gates.len() as u32 {
                    return Err(CompileError::TargetGateOutOfRange {
                        from_node: node.id,
                        from_gate,
                        target_node,
                        target_gate: r.gate,
                        target_gate_count: target.gates.len() as u32,
                    });
                }

                match r.scope {
                    ScopeRef::This => {}
                    ScopeRef::Parent | ScopeRef::Ancestor(_) => {
                        usage
                            .entry(node.id)
                            .or_default()
                            .read_by_parent
                            .insert(from_gate);
                        usage
                            .entry(target_node)
                            .or_default()
                            .written_by_children
                            .entry(ChildId(0))
                            .or_default()
                            .insert(r.gate);
                    }
                    ScopeRef::Child(child_id) => {
                        usage
                            .entry(node.id)
                            .or_default()
                            .written_by_children
                            .entry(child_id)
                            .or_default()
                            .insert(r.gate);
                        usage
                            .entry(target_node)
                            .or_default()
                            .read_by_parent
                            .insert(r.gate);
                    }
                }
            }
        }

        parent_stack.push(node.id);
        for child in &node.children {
            rec(child, parent_stack, by_id, usage)?;
        }
        parent_stack.pop();

        Ok(())
    }

    let mut usage: HashMap<NodeId, RefUsage> = HashMap::default();
    let mut stack = Vec::new();
    rec(root, &mut stack, by_id, &mut usage)?;
    Ok(usage)
}

fn compile_component_rec(
    node: &Component,
    parent_stack: &[NodeId],
    usage: &HashMap<NodeId, RefUsage>,
    compiler: &mut GateCompiler,
) -> Result<(), CompileError> {
    let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
    let _ctx = CompileCtx {
        current: node.id,
        parent_stack,
        child_ids: &child_ids,
    };

    let outline = decide_outline_plan(node, usage.get(&node.id));

    let mut self_bits = Vec::with_capacity(node.gates.len());

    match outline.layout {
        CompileLayout::Inline => {
            for gate_i in 0..node.gates.len() {
                let gate_id = GateId(gate_i as u32);
                let bit = compiler.alloc.alloc_bit();
                compiler.bits.insert((node.id, gate_id), bit);
                self_bits.push(bit);
            }
        }
        CompileLayout::Outline => {
            let total_slots = node.gate_count() + outline.input_count + outline.output_count;
            for slot_i in 0..total_slots {
                let word = compiler.alloc.alloc_word();
                let bit = BitsIndex(word.0, Bits(word.1 * 8));
                if slot_i < node.gates.len() as u32 {
                    let gate_id = GateId(slot_i);
                    compiler.bits.insert((node.id, gate_id), bit);
                    self_bits.push(bit);
                }
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
        compile_component_rec(child, &next_stack, usage, compiler)?;
    }

    Ok(())
}

fn lower_cross_component_edges(
    node: &Component,
    parent_stack: &[NodeId],
    by_id: &HashMap<NodeId, &Component>,
    compiler: &mut GateCompiler,
) -> Result<(), CompileError> {
    let child_ids: Vec<NodeId> = node.children.iter().map(|c| c.id).collect();
    let ctx = CompileCtx {
        current: node.id,
        parent_stack,
        child_ids: &child_ids,
    };

    for (gate_i, gate) in node.gates.iter().copied().enumerate() {
        let dst_gate = GateId(gate_i as u32);

        let dst = compiler.bits.get(&(node.id, dst_gate)).copied().ok_or(
            CompileError::MissingBitsForGate {
                node: node.id,
                gate: dst_gate,
            },
        )?;

        for src_ref in gate_inputs(gate) {
            let target_node =
                resolve_scope(&ctx, src_ref.scope).ok_or(CompileError::InvalidGateRef {
                    from_node: node.id,
                    from_gate: dst_gate,
                    bad_ref: src_ref,
                    reason: "scope does not exist from this location",
                })?;

            let _target_component = by_id
                .get(&target_node)
                .copied()
                .ok_or(CompileError::MissingNode(target_node))?;

            let src = compiler
                .bits
                .get(&(target_node, src_ref.gate))
                .copied()
                .ok_or(CompileError::MissingBitsForGate {
                    node: target_node,
                    gate: src_ref.gate,
                })?;

            if src.0 != dst.0 {
                let _ = compiler.mem.queue_bit_write(src, dst);
            }
        }
    }

    let mut next_stack = Vec::with_capacity(parent_stack.len() + 1);
    next_stack.extend_from_slice(parent_stack);
    next_stack.push(node.id);

    for child in &node.children {
        lower_cross_component_edges(child, &next_stack, by_id, compiler)?;
    }

    Ok(())
}

fn decide_outline_plan(node: &Component, usage: Option<&RefUsage>) -> OutlinePlan {
    let usage = usage.cloned().unwrap_or_default();

    let input_count = usage.read_by_parent.len() as u32;
    let output_count = usage.written_by_parent.len() as u32;

    let inline_cost_bits = node.gate_count();
    let outlined_cost_bits_worst = node.gate_count().saturating_mul(32)
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
    refs: [Option<GateRef>; 2],
}

impl IntoIterator for SmallGateInputs {
    type Item = GateRef;
    type IntoIter = SmallGateInputsIter;

    fn into_iter(self) -> Self::IntoIter {
        SmallGateInputsIter {
            refs: self.refs,
            i: 0,
        }
    }
}

struct SmallGateInputsIter {
    refs: [Option<GateRef>; 2],
    i: usize,
}

impl Iterator for SmallGateInputsIter {
    type Item = GateRef;

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
    total_bits_per_buffer: u32,
) -> Result<CompiledTree, CompileError> {
    compile_component_tree(root, total_bits_per_buffer)
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

    fn this_ref(gate: u32) -> GateRef {
        GateRef {
            scope: ScopeRef::This,
            gate: GateId(gate),
        }
    }

    fn parent_ref(gate: u32) -> GateRef {
        GateRef {
            scope: ScopeRef::Parent,
            gate: GateId(gate),
        }
    }

    fn child_ref(child: u32, gate: u32) -> GateRef {
        GateRef {
            scope: ScopeRef::Child(ChildId(child)),
            gate: GateId(gate),
        }
    }

    fn ancestor_ref(depth: u32, gate: u32) -> GateRef {
        GateRef {
            scope: ScopeRef::Ancestor(AncestorDepth(depth)),
            gate: GateId(gate),
        }
    }

    fn compile_with_state(
        root: &mut Component,
        total_bits_per_buffer: u32,
    ) -> Result<GateCompiler, CompileError> {
        if root.id == INVALID_NODE_ID {
            assign_node_ids(root);
        }

        let by_id = collect_components(root);
        validate_component_tree_with_index(root, &by_id)?;

        let usage = collect_ref_usage(root, &by_id)?;
        let mut compiler = GateCompiler {
            bits: HashMap::default(),
            alloc: ChargeAlloc::new(total_bits_per_buffer),
            mem: WorkingMem {
                mem: Vec::new(),
                bit_cross: HashMap::default(),
            },
            components: HashMap::default(),
        };

        compile_component_rec(root, &[], &usage, &mut compiler)?;
        lower_cross_component_edges(root, &[], &by_id, &mut compiler)?;
        Ok(compiler)
    }

    fn make_large_inline_component(gate_count: u32) -> Component {
        let gates = (0..gate_count)
            .map(|gate| Gate::BitNop {
                src: this_ref(gate),
            })
            .collect();
        Component::new(gates, vec![])
    }

    fn make_deep_inline_tree(depth: u32) -> Component {
        if depth == 0 {
            return Component::new(vec![Gate::BitNop { src: this_ref(0) }], vec![]);
        }

        Component::new(
            vec![Gate::BitNop { src: this_ref(0) }],
            vec![make_deep_inline_tree_with_parent_refs(depth - 1)],
        )
    }

    fn make_deep_inline_tree_with_parent_refs(depth: u32) -> Component {
        if depth == 0 {
            return Component::new(vec![Gate::BitNop { src: parent_ref(0) }], vec![]);
        }

        Component::new(
            vec![Gate::BitNop { src: parent_ref(0) }],
            vec![make_deep_inline_tree_with_parent_refs(depth - 1)],
        )
    }

    fn make_deep_cyclic_tree() -> Component {
        let root_gates = (0..32)
            .map(|gate| {
                if gate == 31 {
                    Gate::BitNop {
                        src: child_ref(0, 0),
                    }
                } else {
                    Gate::BitNop {
                        src: this_ref(gate),
                    }
                }
            })
            .collect();

        let child_gates = (0..32)
            .map(|gate| {
                if gate == 13 {
                    Gate::BitNop {
                        src: child_ref(0, 0),
                    }
                } else {
                    Gate::BitNop {
                        src: this_ref(gate),
                    }
                }
            })
            .collect();

        let grandchild_gates = (0..14)
            .map(|gate| {
                if gate == 13 {
                    Gate::BitNop {
                        src: ancestor_ref(2, 5),
                    }
                } else {
                    Gate::BitNop {
                        src: this_ref(gate),
                    }
                }
            })
            .collect();

        Component::new(
            root_gates,
            vec![Component::new(
                child_gates,
                vec![Component::new(grandchild_gates, vec![])],
            )],
        )
    }

    #[test]
    fn oversized_inline_component_spills_across_buffers_without_out_of_bounds_bits() {
        let mut root = make_large_inline_component(40);

        let compiled = compile_component_tree(&mut root, 32).expect("component should compile");
        let info = compiled
            .components
            .get(&root.id)
            .expect("root should have compiled info");

        assert_eq!(info.outline.layout, CompileLayout::Inline);
        assert_eq!(info.self_bits.len(), 40);
        assert!(info.self_bits.iter().all(|bit| bit.1.0 < 32));

        let used_buffers: HashSet<BufferId> = info.self_bits.iter().map(|bit| bit.0).collect();
        assert!(used_buffers.len() > 1);

        let unique_bits: HashSet<BitsIndex> = info.self_bits.iter().copied().collect();
        assert_eq!(unique_bits.len(), info.self_bits.len());
    }

    #[test]
    fn deep_tree_can_inline_everything_without_cross_buffer_stitching() {
        let mut root = make_deep_inline_tree(20);

        let compiler = compile_with_state(&mut root, 64).expect("deep tree should compile");

        assert_eq!(compiler.components.len(), 21);
        assert!(
            compiler
                .components
                .values()
                .all(|info| info.outline.layout == CompileLayout::Inline)
        );
        assert!(compiler.mem.bit_cross.is_empty());
        assert!(compiler.bits.values().all(|bit| bit.0 == BufferId(0)));
        assert!(compiler.bits.values().all(|bit| bit.1.0 < 64));
    }

    #[test]
    fn cross_buffer_parent_child_edges_link_source_bits_to_expected_targets() {
        let root_gates = (0..32)
            .map(|gate| Gate::BitNop {
                src: this_ref(gate),
            })
            .collect();

        let child_gates = (0..14)
            .map(|gate| {
                if gate == 13 {
                    Gate::BitNop { src: parent_ref(5) }
                } else {
                    Gate::BitNop {
                        src: this_ref(gate),
                    }
                }
            })
            .collect();

        let mut root = Component::new(root_gates, vec![Component::new(child_gates, vec![])]);
        let compiler = compile_with_state(&mut root, 32).expect("tree should compile");

        let root_id = root.id;
        let child_id = root.children[0].id;
        let src = compiler
            .bits
            .get(&(root_id, GateId(5)))
            .copied()
            .expect("source gate should have bits");
        let dst = compiler
            .bits
            .get(&(child_id, GateId(13)))
            .copied()
            .expect("target gate should have bits");

        assert_eq!(src, BitsIndex(BufferId(0), Bits(5)));
        assert_eq!(dst, BitsIndex(BufferId(1), Bits(13)));

        let queued = compiler
            .mem
            .bit_cross
            .get(&(BufferId(0), WordIndex(BufferId(1), 0)))
            .expect("expected cross-buffer edge to be queued on the target word");

        assert!(queued.contains(&(Bits(5), 13)));
    }

    #[test]
    fn deep_cyclic_child_parent_edges_queue_expected_cross_buffer_links() {
        let mut root = make_deep_cyclic_tree();
        let compiler = compile_with_state(&mut root, 32).expect("cyclic tree should compile");

        let root_id = root.id;
        let child_id = root.children[0].id;
        let grandchild_id = root.children[0].children[0].id;

        assert_eq!(
            compiler.bits.get(&(root_id, GateId(31))).copied(),
            Some(BitsIndex(BufferId(0), Bits(31)))
        );
        assert_eq!(
            compiler.bits.get(&(child_id, GateId(0))).copied(),
            Some(BitsIndex(BufferId(1), Bits(0)))
        );
        assert_eq!(
            compiler.bits.get(&(child_id, GateId(13))).copied(),
            Some(BitsIndex(BufferId(1), Bits(13)))
        );
        assert_eq!(
            compiler.bits.get(&(grandchild_id, GateId(0))).copied(),
            Some(BitsIndex(BufferId(2), Bits(0)))
        );
        assert_eq!(
            compiler.bits.get(&(grandchild_id, GateId(13))).copied(),
            Some(BitsIndex(BufferId(2), Bits(13)))
        );

        let child_to_root = compiler
            .mem
            .bit_cross
            .get(&(BufferId(1), WordIndex(BufferId(0), 0)))
            .expect("child output should stitch into the root target word");
        assert!(child_to_root.contains(&(Bits(0), 31)));

        let grandchild_to_child = compiler
            .mem
            .bit_cross
            .get(&(BufferId(2), WordIndex(BufferId(1), 0)))
            .expect("grandchild output should stitch into the child target word");
        assert!(grandchild_to_child.contains(&(Bits(0), 13)));

        let root_to_grandchild = compiler
            .mem
            .bit_cross
            .get(&(BufferId(0), WordIndex(BufferId(2), 0)))
            .expect("root output should stitch into the grandchild target word");
        assert!(root_to_grandchild.contains(&(Bits(5), 13)));
    }

    #[test]
    fn gpu_plan_includes_basic_cross_and_output_write_passes() {
        let root_gates = (0..32)
            .map(|gate| Gate::BitNop {
                src: this_ref(gate),
            })
            .collect();

        let child_gates = (0..14)
            .map(|gate| {
                if gate == 13 {
                    Gate::BitNop { src: parent_ref(5) }
                } else {
                    Gate::BitNop {
                        src: this_ref(gate),
                    }
                }
            })
            .collect();

        let mut root = Component::new(root_gates, vec![Component::new(child_gates, vec![])]);
        let compiled = compile_component_tree(&mut root, 32).expect("tree should compile");

        assert!(!compiled.gpu_plan.basic_gates.is_empty());
        assert!(!compiled.gpu_plan.cross_writes.is_empty());
        assert!(!compiled.gpu_plan.output_writes.is_empty());
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

        let mut root = Component::new(gates, vec![]);
        let compiled = compile_component_tree(&mut root, 64).expect("component should compile");

        assert_eq!(compiled.gpu_plan.basic_gates.len(), 1);

        let main = &compiled.gpu_plan.basic_gates[0];
        assert_eq!(main.tgt_buffer, BufferId(0));
        assert_eq!(
            main.workers,
            vec![BasicGateWorker {
                tgt_word_byte_offset: 0,
                instruction_start: 0,
                instruction_len: 8,
            }]
        );
        assert_eq!(main.instructions.len(), 8);
        assert_eq!(main.instructions[0].op, BasicGateOp::BitNop);
        assert_eq!(main.instructions[1].op, BasicGateOp::BitNot);
        assert_eq!(main.instructions[2].op, BasicGateOp::BitAND);
        assert_eq!(main.instructions[3].op, BasicGateOp::BitOR);
        assert_eq!(main.instructions[4].op, BasicGateOp::BitNAND);
        assert_eq!(main.instructions[5].op, BasicGateOp::BitNOR);
        assert_eq!(main.instructions[6].op, BasicGateOp::BitXOR);
        assert_eq!(main.instructions[7].op, BasicGateOp::BitXNOR);

        assert_eq!(compiled.gpu_plan.output_writes.len(), 1);
        assert_eq!(
            compiled.gpu_plan.output_writes[0].workers,
            vec![BitCrossWorker {
                tgt_word_byte_offset: 0,
                instruction_start: 0,
                instruction_len: 8,
            }]
        );
    }

    #[test]
    fn output_writes_split_root_outputs_by_word() {
        let mut root = make_large_inline_component(35);
        let compiled = compile_component_tree(&mut root, 32).expect("component should compile");

        assert_eq!(compiled.gpu_plan.output_writes.len(), 1);
        assert_eq!(compiled.gpu_plan.output_writes[0].workers.len(), 2);
        assert_eq!(
            compiled.gpu_plan.output_writes[0].workers,
            vec![
                BitCrossWorker {
                    tgt_word_byte_offset: 0,
                    instruction_start: 0,
                    instruction_len: 32,
                },
                BitCrossWorker {
                    tgt_word_byte_offset: 4,
                    instruction_start: 32,
                    instruction_len: 3,
                },
            ]
        );
        assert_eq!(compiled.gpu_plan.output_writes[0].instructions.len(), 35);
    }
}
