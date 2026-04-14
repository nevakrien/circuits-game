use egui::{Color32, PointerButton, Pos2, Rect, Sense, Ui, Vec2};
use foldhash::HashMap;
use std::sync::Arc;

use crate::gate_plans::{
    ChildId, ChildPlacement, CompileError, Component, ComponentPlans, Gate, GateId,
    GateStoreLocation, NodeId, PortId, PortLocation, SignalRef,
};
use crate::ui_config::{CELL, CHILD_PORT_INSET, PAD};

#[derive(Debug, Clone, Copy)]
pub struct ViewportState {
    pub zoom: f32,
    pub pan: Vec2,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FocusedScene {
    pub node: NodeId,
    pub title: String,
    pub bounds: Rect,
    pub words_per_buffer: u32,
    pub gate_store: Arc<HashMap<(NodeId, GateId), GateStoreLocation>>,
    pub grid_rect: Rect,
    pub grid_dims: [u32; 2],
    pub input_ports: Vec<PlacedPort>,
    pub output_ports: Vec<PlacedPort>,
    pub gates: Vec<PlacedGate>,
    pub children: Vec<PlacedChild>,
    pub ancestor_ports: Vec<ExternalPort>,
    pub wires: Vec<VisualWire>,
}

#[derive(Debug, Clone)]
pub struct PlacedPort {
    pub id: PortId,
    pub source_gate: (NodeId, GateId),
    pub anchor: Pos2,
    pub location: PortLocation,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct PlacedGate {
    pub id: GateId,
    pub gate: Gate,
    pub input_sources: [Option<(NodeId, GateId)>; 2],
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub struct PlacedChild {
    pub id: ChildId,
    pub node: NodeId,
    pub rect: Rect,
    pub inputs: Vec<PlacedPort>,
    pub outputs: Vec<PlacedPort>,
    pub scene: Box<FocusedScene>,
}

#[derive(Debug, Clone)]
pub struct ExternalPort {
    pub child: Option<ChildId>,
    pub node: Option<NodeId>,
    pub port: PortId,
    pub source_gate: (NodeId, GateId),
    pub anchor: Pos2,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct VisualWire {
    pub source_gate: Option<(NodeId, GateId)>,
    pub color: Color32,
    pub points: Vec<Pos2>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneAction {
    FocusChild(NodeId),
}

#[derive(Debug, Clone, Copy)]
pub struct SceneViewportOutput {
    pub rect: Rect,
    pub action: Option<SceneAction>,
}

#[derive(Debug, Clone, Copy)]
enum Endpoint {
    GateOutput(GateId),
    GateInput(GateId, usize),
    ComponentInput(PortId),
    ComponentOutput(PortId),
    ChildOutput(ChildId, PortId),
    ChildInput(ChildId, PortId),
    AncestorOutput(PortId),
}

pub fn build_focused_scene(
    root: &Component,
    plans: &ComponentPlans,
    focused: NodeId,
    gate_store: Arc<HashMap<(NodeId, GateId), GateStoreLocation>>,
    words_per_buffer: u32,
) -> Result<FocusedScene, CompileError> {
    let by_id = collect_components(root);
    build_focused_scene_with_index(
        root,
        plans,
        focused,
        gate_store,
        words_per_buffer,
        &by_id,
        usize::MAX,
    )
}

fn build_focused_scene_with_index(
    root: &Component,
    plans: &ComponentPlans,
    focused: NodeId,
    gate_store: Arc<HashMap<(NodeId, GateId), GateStoreLocation>>,
    words_per_buffer: u32,
    by_id: &HashMap<NodeId, &Component>,
    preview_depth: usize,
) -> Result<FocusedScene, CompileError> {
    let focus = by_id
        .get(&focused)
        .copied()
        .ok_or(CompileError::MissingNode(focused))?;
    let plan = plans
        .get(focus.plan)
        .ok_or(CompileError::MissingPlan(focus.plan))?;

    let grid_dims = plan.grid_size;
    let grid_size = Vec2::new(grid_dims[0] as f32 * CELL, grid_dims[1] as f32 * CELL);
    let grid_rect = Rect::from_min_size(Pos2::new(PAD, PAD + 36.0), grid_size);
    let parent_stack = parent_stack_to(root, focused).unwrap_or_default();
    let child_ids: Vec<_> = focus.children.iter().map(|child| child.id).collect();
    let ctx = VisualCtx {
        parent_stack: &parent_stack,
        child_ids: &child_ids,
    };

    let input_ports: Vec<_> = plan
        .inputs
        .iter()
        .map(|port| PlacedPort {
            id: port.id,
            source_gate: (focused, port.gate),
            anchor: grid_anchor_for_port(grid_rect, grid_dims, port.location),
            location: port.location,
            label: port.label.unwrap_or_default().to_owned(),
        })
        .collect();
    let output_ports: Vec<_> = plan
        .outputs
        .iter()
        .map(|port| PlacedPort {
            id: port.id,
            source_gate: (focused, port.gate),
            anchor: grid_anchor_for_port(grid_rect, grid_dims, port.location),
            location: port.location,
            label: port.label.unwrap_or_default().to_owned(),
        })
        .collect();

    let gates = plan
        .gates
        .iter()
        .copied()
        .enumerate()
        .map(|(index, gate)| {
            let index = index as u32;
            let gx = index % grid_dims[0];
            let gy = index / grid_dims[0];
            let min = grid_rect.min + Vec2::new(gx as f32 * CELL, gy as f32 * CELL);
            let input_sources = gate.input_refs().map(|source| {
                source
                    .and_then(|signal| source_gate_of_ref(focus, signal, &ctx, plans, &by_id).ok())
            });
            PlacedGate {
                id: GateId(index),
                gate,
                input_sources,
                rect: Rect::from_min_size(min, Vec2::splat(CELL)),
            }
        })
        .collect::<Vec<_>>();

    let children = if preview_depth == 0 {
        Vec::new()
    } else {
        focus
            .children
            .iter()
            .enumerate()
            .map(|(child_i, child)| {
                let child_id = ChildId(child_i as u32);
                let child_plan = plans
                    .get(child.plan)
                    .ok_or(CompileError::MissingPlan(child.plan))?;
                let placement = plan
                    .child_placements
                    .get(child_i)
                    .copied()
                    .unwrap_or(ChildPlacement::ONE_CELL);
                let rect = child_rect_from_placement(
                    grid_rect,
                    grid_dims,
                    child_plan.grid_size,
                    placement,
                );
                let inputs = child_plan
                    .inputs
                    .iter()
                    .map(|port| PlacedPort {
                        id: port.id,
                        source_gate: (child.id, port.gate),
                        anchor: child_port_anchor(rect, child_plan.grid_size, port.location),
                        location: port.location,
                        label: port.label.unwrap_or_default().to_owned(),
                    })
                    .collect();
                let outputs = child_plan
                    .outputs
                    .iter()
                    .map(|port| PlacedPort {
                        id: port.id,
                        source_gate: (child.id, port.gate),
                        anchor: child_port_anchor(rect, child_plan.grid_size, port.location),
                        location: port.location,
                        label: port.label.unwrap_or_default().to_owned(),
                    })
                    .collect();
                let scene = build_focused_scene_with_index(
                    root,
                    plans,
                    child.id,
                    gate_store.clone(),
                    words_per_buffer,
                    by_id,
                    preview_depth.saturating_sub(1),
                )?;
                Ok(PlacedChild {
                    id: child_id,
                    node: child.id,
                    rect,
                    inputs,
                    outputs,
                    scene: Box::new(scene),
                })
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    let ancestor_ports = parent_stack
        .last()
        .and_then(|parent_id| by_id.get(parent_id).copied())
        .and_then(|parent| plans.get(parent.plan).map(|plan| (parent, plan)))
        .map(|(parent, parent_plan)| {
            parent_plan
                .outputs
                .iter()
                .enumerate()
                .map(|(i, port)| ExternalPort {
                    child: None,
                    node: Some(parent.id),
                    port: port.id,
                    source_gate: (parent.id, port.gate),
                    anchor: Pos2::new(
                        grid_rect.left() - 28.0,
                        grid_rect.top() + 20.0 + i as f32 * 32.0,
                    ),
                    label: port.label.unwrap_or_default().to_owned(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let lookup = Lookup {
        input_ports: input_ports
            .iter()
            .map(|port| (port.id, port.anchor))
            .collect(),
        output_ports: output_ports
            .iter()
            .map(|port| (port.id, port.anchor))
            .collect(),
        child_outputs: children
            .iter()
            .flat_map(|child| {
                child
                    .outputs
                    .iter()
                    .map(move |port| ((child.id, port.id), port.anchor))
            })
            .collect(),
        child_inputs: children
            .iter()
            .flat_map(|child| {
                child
                    .inputs
                    .iter()
                    .map(move |port| ((child.id, port.id), port.anchor))
            })
            .collect(),
        ancestor_outputs: ancestor_ports
            .iter()
            .map(|port| (port.port, port.anchor))
            .collect(),
        gates: gates
            .iter()
            .map(|gate| (gate.id, (gate.rect, gate.gate)))
            .collect(),
    };

    let mut wires = Vec::new();
    for (gate_i, gate) in plan.gates.iter().copied().enumerate() {
        let target_gate = GateId(gate_i as u32);
        for (input_i, source_ref) in gate.input_refs().into_iter().flatten().enumerate() {
            let src_endpoint = resolve_endpoint(focus, source_ref, &ctx, plans, &by_id)?;
            let src_gate = source_gate_of_ref(focus, source_ref, &ctx, plans, &by_id).ok();
            if let Some(points) = orth_wire_points(
                lookup.anchor(src_endpoint),
                lookup.anchor(Endpoint::GateInput(target_gate, input_i)),
            ) {
                wires.push(VisualWire {
                    source_gate: src_gate,
                    color: palette_color(wires.len()),
                    points,
                });
            }
        }
    }

    for connection in &focus.child_input_connections {
        let src_endpoint = resolve_endpoint(focus, connection.src, &ctx, plans, &by_id)?;
        let src_gate = source_gate_of_ref(focus, connection.src, &ctx, plans, &by_id).ok();
        if let Some(points) = orth_wire_points(
            lookup.anchor(src_endpoint),
            lookup.anchor(Endpoint::ChildInput(connection.child, connection.input)),
        ) {
            wires.push(VisualWire {
                source_gate: src_gate,
                color: palette_color(wires.len()),
                points,
            });
        }
    }

    for output in &plan.outputs {
        if let Some(points) = orth_wire_points(
            lookup.anchor(Endpoint::GateOutput(output.gate)),
            lookup.anchor(Endpoint::ComponentOutput(output.id)),
        ) {
            wires.push(VisualWire {
                source_gate: Some((focused, output.gate)),
                color: palette_color(wires.len()),
                points,
            });
        }
    }

    let bounds = Rect::from_min_max(
        Pos2::ZERO,
        Pos2::new(grid_rect.right() + PAD, grid_rect.bottom() + PAD),
    );

    Ok(FocusedScene {
        node: focused,
        title: format!("Component {}", focused.0),
        bounds,
        words_per_buffer,
        gate_store,
        grid_rect,
        grid_dims,
        input_ports,
        output_ports,
        gates,
        children,
        ancestor_ports,
        wires,
    })
}

pub fn parent_stack_to(root: &Component, target: NodeId) -> Option<Vec<NodeId>> {
    fn rec(node: &Component, target: NodeId, stack: &mut Vec<NodeId>) -> bool {
        if node.id == target {
            return true;
        }
        stack.push(node.id);
        for child in &node.children {
            if rec(child, target, stack) {
                return true;
            }
        }
        stack.pop();
        false
    }

    let mut stack = Vec::new();
    rec(root, target, &mut stack).then_some(stack)
}

pub fn child_ids_of(root: &Component, node_id: NodeId) -> Vec<NodeId> {
    fn rec(node: &Component, node_id: NodeId) -> Option<Vec<NodeId>> {
        if node.id == node_id {
            return Some(node.children.iter().map(|child| child.id).collect());
        }
        for child in &node.children {
            if let Some(ids) = rec(child, node_id) {
                return Some(ids);
            }
        }
        None
    }

    rec(root, node_id).unwrap_or_default()
}

pub fn interact_focused_scene(
    ui: &mut Ui,
    scene: &FocusedScene,
    viewport: &mut ViewportState,
) -> SceneViewportOutput {
    let available = ui.available_size_before_wrap();
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());

    if response.dragged_by(PointerButton::Middle) || response.dragged_by(PointerButton::Primary) {
        viewport.pan += ui.ctx().input(|input| input.pointer.delta());
    }
    if response.hovered() {
        let zoom_delta = ui.ctx().input(|input| input.zoom_delta());
        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            let old_zoom = viewport.zoom;
            let new_zoom = (old_zoom * zoom_delta).clamp(0.35, 3.0);
            if (new_zoom - old_zoom).abs() > f32::EPSILON {
                if let Some(pointer) = ui.ctx().input(|input| input.pointer.hover_pos()) {
                    let local = pointer - rect.min;
                    let world = (local - viewport.pan) / old_zoom.max(f32::EPSILON);
                    viewport.pan = local - world * new_zoom;
                }
                viewport.zoom = new_zoom;
            }
        }
    }

    let camera = Camera::new(Pos2::ZERO, rect.min + viewport.pan, viewport.zoom);
    let mut action = None;
    for child in &scene.children {
        let child_hit = camera.rect(child.rect);
        let response = ui.interact(
            child_hit,
            ui.make_persistent_id((scene.node.0, child.node.0, "child-rect")),
            Sense::click(),
        );
        if response.clicked() {
            action = Some(SceneAction::FocusChild(child.node));
        }
    }

    SceneViewportOutput { rect, action }
}

#[derive(Clone, Copy)]
struct Camera {
    source_min: Pos2,
    screen_min: Pos2,
    scale: f32,
}

impl Camera {
    fn new(source_min: Pos2, screen_min: Pos2, scale: f32) -> Self {
        Self {
            source_min,
            screen_min,
            scale,
        }
    }

    fn pos(&self, pos: Pos2) -> Pos2 {
        self.screen_min + (pos - self.source_min) * self.scale
    }

    fn rect(&self, rect: Rect) -> Rect {
        Rect::from_min_max(self.pos(rect.min), self.pos(rect.max))
    }
}

fn grid_anchor_for_port(grid_rect: Rect, grid_dims: [u32; 2], location: PortLocation) -> Pos2 {
    let fx = port_axis_fraction(location.x, grid_dims[0]);
    let fy = port_axis_fraction(location.y, grid_dims[1]);
    Pos2::new(
        grid_rect.left() + grid_rect.width() * fx,
        grid_rect.top() + grid_rect.height() * fy,
    )
}

fn child_port_anchor(child_rect: Rect, grid_dims: [u32; 2], location: PortLocation) -> Pos2 {
    let mut anchor = grid_anchor_for_port(child_rect, grid_dims, location);
    if location.x == 0 {
        anchor.x += CHILD_PORT_INSET;
    } else if location.x == u16::MAX {
        anchor.x -= CHILD_PORT_INSET;
    }
    if location.y == 0 {
        anchor.y += CHILD_PORT_INSET;
    } else if location.y == u16::MAX {
        anchor.y -= CHILD_PORT_INSET;
    }
    anchor
}

fn port_axis_fraction(value: u16, dim: u32) -> f32 {
    if value == u16::MAX {
        return 1.0;
    }

    let dim = dim.max(1);
    if value as u32 <= dim {
        return value as f32 / dim as f32;
    }

    value as f32 / u16::MAX as f32
}

fn child_rect_from_placement(
    grid_rect: Rect,
    grid_dims: [u32; 2],
    child_dims: [u32; 2],
    placement: ChildPlacement,
) -> Rect {
    const CHILD_FOOTPRINT_FILL: f32 = 0.88;

    let [width, height] = if child_dims[0] >= grid_dims[0] || child_dims[1] >= grid_dims[1] {
        [(grid_dims[0] / 2).max(1), (grid_dims[1] / 2).max(1)]
    } else {
        [
            child_dims[0].max(1).min(grid_dims[0].max(1)),
            child_dims[1].max(1).min(grid_dims[1].max(1)),
        ]
    };
    let max_x = grid_dims[0].saturating_sub(width);
    let max_y = grid_dims[1].saturating_sub(height);
    let min_x = placement.min[0].min(max_x);
    let min_y = placement.min[1].min(max_y);
    let min = grid_rect.min + Vec2::new(min_x as f32 * CELL, min_y as f32 * CELL);
    let footprint = Rect::from_min_size(min, Vec2::new(width as f32 * CELL, height as f32 * CELL));
    let scaled_size = footprint.size() * CHILD_FOOTPRINT_FILL;
    Rect::from_center_size(footprint.center(), scaled_size)
}

fn gate_anchor(rect: Rect, gate: Gate, input: Option<usize>) -> Pos2 {
    let local = match (gate, input) {
        (Gate::BitNot { .. } | Gate::BitNop { .. }, Some(0)) => [0.08, 0.5],
        (
            Gate::BitAND { .. }
            | Gate::BitOR { .. }
            | Gate::BitXOR { .. }
            | Gate::BitNAND { .. }
            | Gate::BitNOR { .. }
            | Gate::BitXNOR { .. },
            Some(0),
        ) => [0.08, 0.3],
        (
            Gate::BitAND { .. }
            | Gate::BitOR { .. }
            | Gate::BitXOR { .. }
            | Gate::BitNAND { .. }
            | Gate::BitNOR { .. }
            | Gate::BitXNOR { .. },
            Some(1),
        ) => [0.08, 0.7],
        _ => [0.92, 0.5],
    };
    Pos2::new(
        rect.left() + rect.width() * local[0],
        rect.top() + rect.height() * local[1],
    )
}

fn resolve_endpoint(
    node: &Component,
    signal: SignalRef,
    ctx: &VisualCtx<'_>,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<Endpoint, CompileError> {
    match signal {
        SignalRef::ThisGate(gate) => Ok(Endpoint::GateOutput(gate)),
        SignalRef::InputPort(port) => Ok(Endpoint::ComponentInput(port)),
        SignalRef::ChildOutput { child, port } => Ok(Endpoint::ChildOutput(child, port)),
        SignalRef::AncestorOutput { depth, port } => {
            let depth = depth.0 as usize;
            if depth == 0 || depth > ctx.parent_stack.len() {
                return Err(CompileError::InvalidGateRef {
                    from_node: node.id,
                    from_gate: GateId(u32::MAX),
                    bad_ref: signal,
                    reason: "ancestor does not exist from this location",
                });
            }
            let ancestor = ctx.parent_stack[ctx.parent_stack.len() - depth];
            let ancestor_node = by_id
                .get(&ancestor)
                .copied()
                .ok_or(CompileError::MissingNode(ancestor))?;
            let ancestor_plan = plans
                .get(ancestor_node.plan)
                .ok_or(CompileError::MissingPlan(ancestor_node.plan))?;
            ancestor_plan
                .outputs
                .iter()
                .find(|output| output.id == port)
                .ok_or(CompileError::MissingOutputPort {
                    node: ancestor,
                    port,
                })?;
            Ok(Endpoint::AncestorOutput(port))
        }
    }
}

fn source_gate_of_ref(
    node: &Component,
    signal: SignalRef,
    ctx: &VisualCtx<'_>,
    plans: &ComponentPlans,
    by_id: &HashMap<NodeId, &Component>,
) -> Result<(NodeId, GateId), CompileError> {
    match signal {
        SignalRef::ThisGate(gate) => Ok((node.id, gate)),
        SignalRef::InputPort(port) => {
            let plan = plans
                .get(node.plan)
                .ok_or(CompileError::MissingPlan(node.plan))?;
            let gate = plan
                .inputs
                .iter()
                .find(|input| input.id == port)
                .ok_or(CompileError::MissingInputPort {
                    node: node.id,
                    port,
                })?
                .gate;
            Ok((node.id, gate))
        }
        SignalRef::ChildOutput { child, port } => {
            let child_id = ctx.child_ids.get(child.0 as usize).copied().ok_or(
                CompileError::InvalidGateRef {
                    from_node: node.id,
                    from_gate: GateId(u32::MAX),
                    bad_ref: signal,
                    reason: "child does not exist from this location",
                },
            )?;
            let child = by_id
                .get(&child_id)
                .copied()
                .ok_or(CompileError::MissingNode(child_id))?;
            let plan = plans
                .get(child.plan)
                .ok_or(CompileError::MissingPlan(child.plan))?;
            let gate = plan
                .outputs
                .iter()
                .find(|output| output.id == port)
                .ok_or(CompileError::MissingOutputPort {
                    node: child_id,
                    port,
                })?
                .gate;
            Ok((child_id, gate))
        }
        SignalRef::AncestorOutput { depth, port } => {
            let depth = depth.0 as usize;
            let ancestor_id = ctx
                .parent_stack
                .get(ctx.parent_stack.len() - depth)
                .copied()
                .ok_or(CompileError::InvalidGateRef {
                    from_node: node.id,
                    from_gate: GateId(u32::MAX),
                    bad_ref: signal,
                    reason: "ancestor does not exist from this location",
                })?;
            let ancestor = by_id
                .get(&ancestor_id)
                .copied()
                .ok_or(CompileError::MissingNode(ancestor_id))?;
            let plan = plans
                .get(ancestor.plan)
                .ok_or(CompileError::MissingPlan(ancestor.plan))?;
            let gate = plan
                .outputs
                .iter()
                .find(|output| output.id == port)
                .ok_or(CompileError::MissingOutputPort {
                    node: ancestor_id,
                    port,
                })?
                .gate;
            Ok((ancestor_id, gate))
        }
    }
}

struct Lookup {
    input_ports: HashMap<PortId, Pos2>,
    output_ports: HashMap<PortId, Pos2>,
    child_outputs: HashMap<(ChildId, PortId), Pos2>,
    child_inputs: HashMap<(ChildId, PortId), Pos2>,
    ancestor_outputs: HashMap<PortId, Pos2>,
    gates: HashMap<GateId, (Rect, Gate)>,
}

impl Lookup {
    fn anchor(&self, endpoint: Endpoint) -> Option<Pos2> {
        match endpoint {
            Endpoint::GateOutput(gate) => {
                let (rect, gate_kind) = self.gates.get(&gate)?;
                Some(gate_anchor(*rect, *gate_kind, None))
            }
            Endpoint::GateInput(gate, input) => {
                let (rect, gate_kind) = self.gates.get(&gate)?;
                Some(gate_anchor(*rect, *gate_kind, Some(input)))
            }
            Endpoint::ComponentInput(port) => self.input_ports.get(&port).copied(),
            Endpoint::ComponentOutput(port) => self.output_ports.get(&port).copied(),
            Endpoint::ChildOutput(child, port) => self.child_outputs.get(&(child, port)).copied(),
            Endpoint::ChildInput(child, port) => self.child_inputs.get(&(child, port)).copied(),
            Endpoint::AncestorOutput(port) => self.ancestor_outputs.get(&port).copied(),
        }
    }
}

struct VisualCtx<'a> {
    parent_stack: &'a [NodeId],
    child_ids: &'a [NodeId],
}

fn orth_wire_points(start: Option<Pos2>, end: Option<Pos2>) -> Option<Vec<Pos2>> {
    let (start, end) = (start?, end?);
    let mid_x = (start.x + end.x) * 0.5;
    Some(vec![
        start,
        Pos2::new(mid_x, start.y),
        Pos2::new(mid_x, end.y),
        end,
    ])
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

fn palette_color(index: usize) -> Color32 {
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(96, 214, 184),
        Color32::from_rgb(250, 176, 91),
        Color32::from_rgb(100, 186, 255),
        Color32::from_rgb(255, 122, 162),
        Color32::from_rgb(196, 154, 255),
        Color32::from_rgb(246, 226, 92),
        Color32::from_rgb(105, 234, 108),
        Color32::from_rgb(255, 146, 103),
    ];
    PALETTE[index % PALETTE.len()]
}
