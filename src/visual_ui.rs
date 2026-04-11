use egui::{
    Align2, Color32, FontId, Painter, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind,
    Ui, Vec2,
};
use foldhash::HashMap;

use crate::gate_plans::{
    ChildId, ChildPlacement, CompileError, Component, ComponentPlans, Gate, GateId,
    GateStoreLocation, NodeId, PortId, PortLocation, SignalRef,
};
use crate::ui_config::{
    ANCESTOR_COLOR, CELL, CHILD_BG, CHILD_INPUT_COLOR, CHILD_OUTPUT_COLOR, CHILD_PORT_INSET,
    CHILD_ZOOM_PREVIEW, GATE_INPUT_FILL, GATE_INPUT_STROKE, GATE_OFF, GATE_ON, GATE_OUTPUT_FILL,
    GATE_OUTPUT_STROKE, GATE_STROKE, GRID_BG, GRID_LINE, GRID_STROKE, INPUT_PORT_COLOR,
    MAX_PULSE_CYCLES_PER_SECOND, MIN_PULSE_CYCLES_PER_SECOND, OUTPUT_PORT_COLOR, PAD, PANEL_BG,
    PORT_RADIUS, PULSE_CYCLES_PER_TICK,
};

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
    pub gate_store: HashMap<(NodeId, GateId), GateStoreLocation>,
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
    pub anchor: Pos2,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct PlacedGate {
    pub id: GateId,
    pub gate: Gate,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub struct PlacedChild {
    pub id: ChildId,
    pub node: NodeId,
    pub rect: Rect,
    pub inputs: Vec<PlacedPort>,
    pub outputs: Vec<PlacedPort>,
    pub preview_gates: Vec<PlacedGate>,
}

#[derive(Debug, Clone)]
pub struct ExternalPort {
    pub child: Option<ChildId>,
    pub node: Option<NodeId>,
    pub port: PortId,
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
    gate_store: &HashMap<(NodeId, GateId), GateStoreLocation>,
    words_per_buffer: u32,
) -> Result<FocusedScene, CompileError> {
    let by_id = collect_components(root);
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

    let input_ports: Vec<_> = plan
        .inputs
        .iter()
        .map(|port| PlacedPort {
            id: port.id,
            anchor: grid_anchor_for_port(grid_rect, port.location),
            label: format!("in {}", port.id.0),
        })
        .collect();
    let output_ports: Vec<_> = plan
        .outputs
        .iter()
        .map(|port| PlacedPort {
            id: port.id,
            anchor: grid_anchor_for_port(grid_rect, port.location),
            label: format!("out {}", port.id.0),
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
            PlacedGate {
                id: GateId(index),
                gate,
                rect: Rect::from_min_size(min, Vec2::splat(CELL)),
            }
        })
        .collect::<Vec<_>>();

    let children = focus
        .children
        .iter()
        .enumerate()
        .filter_map(|(child_i, child)| {
            let child_id = ChildId(child_i as u32);
            let child_plan = plans.get(child.plan)?;
            let placement = plan
                .child_placements
                .get(child_i)
                .copied()
                .unwrap_or(ChildPlacement::ONE_CELL);
            let rect = child_rect_from_placement(grid_rect, placement);
            let inputs = child_plan
                .inputs
                .iter()
                .map(|port| PlacedPort {
                    id: port.id,
                    anchor: child_port_anchor(rect, port.location),
                    label: format!("in {}", port.id.0),
                })
                .collect();
            let outputs = child_plan
                .outputs
                .iter()
                .map(|port| PlacedPort {
                    id: port.id,
                    anchor: child_port_anchor(rect, port.location),
                    label: format!("out {}", port.id.0),
                })
                .collect();
            let preview_gates = child_preview_gates(rect, child_plan);
            Some(PlacedChild {
                id: child_id,
                node: child.id,
                rect,
                inputs,
                outputs,
                preview_gates,
            })
        })
        .collect::<Vec<_>>();

    let parent_stack = parent_stack_to(root, focused).unwrap_or_default();
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
                    anchor: Pos2::new(
                        grid_rect.left() - 28.0,
                        grid_rect.top() + 20.0 + i as f32 * 32.0,
                    ),
                    label: format!("ancestor {} out {}", parent.id.0, port.id.0),
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

    let child_ids: Vec<_> = focus.children.iter().map(|child| child.id).collect();
    let ctx = VisualCtx {
        parent_stack: &parent_stack,
        child_ids: &child_ids,
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
        gate_store: gate_store.clone(),
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

pub fn show_focused_scene(
    ui: &mut Ui,
    scene: &FocusedScene,
    read_words: &[u32],
    time: f64,
    pulse_rate_hz: f32,
    viewport: &mut ViewportState,
) -> Option<SceneAction> {
    let available = ui.available_size_before_wrap();
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    if response.dragged_by(PointerButton::Middle) || response.dragged_by(PointerButton::Primary) {
        viewport.pan += ui.ctx().input(|input| input.pointer.delta());
    }
    if response.hovered() {
        let zoom_delta = ui.ctx().input(|input| input.zoom_delta());
        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            viewport.zoom = (viewport.zoom * zoom_delta).clamp(0.35, 3.0);
        }
    }

    painter.rect(
        rect,
        12.0,
        PANEL_BG,
        Stroke::new(1.0, Color32::from_rgb(38, 46, 59)),
        StrokeKind::Outside,
    );

    let screen = |p: Pos2| rect.min + viewport.pan + p.to_vec2() * viewport.zoom;
    let screen_rect = |r: Rect| Rect::from_min_max(screen(r.min), screen(r.max));

    paint_grid(
        &painter,
        screen_rect(scene.grid_rect),
        scene.grid_dims,
        viewport.zoom,
    );
    painter.text(
        screen(Pos2::new(PAD, 8.0)),
        Align2::LEFT_TOP,
        &scene.title,
        FontId::proportional(20.0 * viewport.zoom.clamp(0.8, 1.2)),
        Color32::WHITE,
    );

    for gate in &scene.gates {
        paint_gate(
            &painter,
            gate,
            scene,
            read_words,
            &screen,
            &screen_rect,
            viewport.zoom,
        );
    }
    let mut action = None;
    for child in &scene.children {
        paint_child(&painter, child, &screen_rect, viewport.zoom);
        let child_hit = screen_rect(child.rect);
        let response = ui.interact(
            child_hit,
            ui.make_persistent_id((scene.node.0, child.node.0, "child-rect")),
            Sense::click(),
        );
        if response.clicked() {
            action = Some(SceneAction::FocusChild(child.node));
        }
    }
    for wire in &scene.wires {
        paint_wire(
            &painter,
            wire,
            scene,
            read_words,
            time,
            pulse_rate_hz,
            &screen,
            viewport.zoom,
        );
    }
    for port in &scene.input_ports {
        paint_port(&painter, port, INPUT_PORT_COLOR, &screen, viewport.zoom);
    }
    for port in &scene.output_ports {
        paint_port(&painter, port, OUTPUT_PORT_COLOR, &screen, viewport.zoom);
    }
    for port in &scene.ancestor_ports {
        paint_external_port(&painter, port, ANCESTOR_COLOR, &screen, viewport.zoom);
    }

    for child in &scene.children {
        for port in &child.inputs {
            paint_port(&painter, port, CHILD_INPUT_COLOR, &screen, viewport.zoom);
        }
        for port in &child.outputs {
            paint_port(&painter, port, CHILD_OUTPUT_COLOR, &screen, viewport.zoom);
        }
    }

    action
}

fn paint_grid(painter: &Painter, rect: Rect, dims: [u32; 2], zoom: f32) {
    painter.rect(
        rect,
        8.0,
        GRID_BG,
        Stroke::new(1.2, GRID_LINE),
        StrokeKind::Outside,
    );
    for x in 1..dims[0] {
        let x = rect.left() + x as f32 * CELL * zoom;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(GRID_STROKE, GRID_LINE),
        );
    }
    for y in 1..dims[1] {
        let y = rect.top() + y as f32 * CELL * zoom;
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(GRID_STROKE, GRID_LINE),
        );
    }
}

fn paint_gate<F: Fn(Rect) -> Rect>(
    painter: &Painter,
    gate: &PlacedGate,
    scene: &FocusedScene,
    read_words: &[u32],
    screen: &impl Fn(Pos2) -> Pos2,
    screen_rect: &F,
    zoom: f32,
) {
    let rect = screen_rect(gate.rect.shrink(10.0));
    let is_on = scene
        .gate_store
        .get(&(scene.node, gate.id))
        .is_some_and(|store| gate_value(read_words, scene.words_per_buffer, *store));
    painter.rect(
        rect,
        10.0,
        if is_on { GATE_ON } else { GATE_OFF },
        Stroke::new(1.5, GATE_STROKE),
        StrokeKind::Outside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        gate.gate.label(),
        FontId::proportional(15.0 * zoom.clamp(0.8, 1.1)),
        Color32::WHITE,
    );

    for input in gate.gate.input_refs().into_iter().flatten().enumerate() {
        let anchor = screen(gate_anchor(gate.rect, gate.gate, Some(input.0)));
        paint_gate_input_marker(painter, anchor, zoom);
    }
    let output_anchor = screen(gate_anchor(gate.rect, gate.gate, None));
    paint_gate_output_marker(painter, output_anchor, zoom);
}

fn paint_child<F: Fn(Rect) -> Rect>(
    painter: &Painter,
    child: &PlacedChild,
    screen_rect: &F,
    zoom: f32,
) {
    let rect = screen_rect(child.rect.shrink(6.0));
    painter.rect(
        rect,
        10.0,
        CHILD_BG,
        Stroke::new(1.5, CHILD_INPUT_COLOR),
        StrokeKind::Outside,
    );
    painter.text(
        rect.min + Vec2::new(8.0, 8.0),
        Align2::LEFT_TOP,
        format!("child {}", child.node.0),
        FontId::proportional(14.0 * zoom.clamp(0.8, 1.1)),
        Color32::WHITE,
    );
    if zoom >= CHILD_ZOOM_PREVIEW {
        for gate in &child.preview_gates {
            let gate_rect = screen_rect(gate.rect.shrink(6.0));
            painter.rect(
                gate_rect,
                6.0,
                Color32::from_rgb(61, 70, 88),
                Stroke::new(1.0, Color32::from_rgb(130, 143, 164)),
                StrokeKind::Outside,
            );
            painter.text(
                gate_rect.center(),
                Align2::CENTER_CENTER,
                gate.gate.label(),
                FontId::proportional(10.0 * zoom.clamp(0.7, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

fn paint_port<F: Fn(Pos2) -> Pos2>(
    painter: &Painter,
    port: &PlacedPort,
    color: Color32,
    screen: &F,
    zoom: f32,
) {
    let anchor = screen(port.anchor);
    painter.circle_filled(anchor, PORT_RADIUS * zoom.clamp(0.7, 1.4), color);
    painter.text(
        anchor + Vec2::new(8.0 * zoom, -8.0 * zoom),
        Align2::LEFT_BOTTOM,
        &port.label,
        FontId::monospace(12.0 * zoom.clamp(0.8, 1.0)),
        Color32::from_rgb(220, 227, 238),
    );
}

fn paint_gate_input_marker(painter: &Painter, anchor: Pos2, zoom: f32) {
    let radius = 5.5 * zoom.clamp(0.75, 1.35);
    painter.circle_filled(anchor, radius, GATE_INPUT_FILL);
    painter.circle_stroke(anchor, radius, Stroke::new(1.4, GATE_INPUT_STROKE));
}

fn paint_gate_output_marker(painter: &Painter, anchor: Pos2, zoom: f32) {
    let radius = 5.5 * zoom.clamp(0.75, 1.35);
    painter.circle_filled(anchor, radius, GATE_OUTPUT_FILL);
    painter.circle_stroke(anchor, radius, Stroke::new(1.4, GATE_OUTPUT_STROKE));
}

fn paint_external_port<F: Fn(Pos2) -> Pos2>(
    painter: &Painter,
    port: &ExternalPort,
    color: Color32,
    screen: &F,
    zoom: f32,
) {
    let anchor = screen(port.anchor);
    painter.circle_filled(anchor, PORT_RADIUS * zoom.clamp(0.7, 1.4), color);
    painter.text(
        anchor + Vec2::new(10.0 * zoom, 0.0),
        Align2::LEFT_CENTER,
        &port.label,
        FontId::monospace(12.0 * zoom.clamp(0.8, 1.0)),
        Color32::from_rgb(216, 224, 235),
    );
}

fn paint_wire<F: Fn(Pos2) -> Pos2>(
    painter: &Painter,
    wire: &VisualWire,
    scene: &FocusedScene,
    read_words: &[u32],
    time: f64,
    pulse_rate_hz: f32,
    screen: &F,
    zoom: f32,
) {
    let active = wire
        .source_gate
        .and_then(|key| scene.gate_store.get(&key).copied())
        .is_some_and(|store| gate_value(read_words, scene.words_per_buffer, store));
    let color = if active {
        wire.color
    } else {
        Color32::from_rgba_unmultiplied(wire.color.r(), wire.color.g(), wire.color.b(), 70)
    };
    let points: Vec<_> = wire.points.iter().map(|point| screen(*point)).collect();
    painter.add(Shape::line(
        points.clone(),
        Stroke::new(2.0 * zoom.clamp(0.7, 1.3), color),
    ));
    if active {
        let count = ((polyline_length(&points) / 90.0).ceil() as usize).max(2);
        let pulse_cycles = pulse_cycles_per_second(pulse_rate_hz);
        for i in 0..count {
            let phase = ((time as f32 * pulse_cycles) + i as f32 / count as f32).fract();
            painter.circle_filled(
                point_along_polyline(&points, phase),
                4.0 * zoom.clamp(0.7, 1.3),
                color,
            );
        }
    }
}

fn pulse_cycles_per_second(pulse_rate_hz: f32) -> f32 {
    (pulse_rate_hz * PULSE_CYCLES_PER_TICK)
        .clamp(MIN_PULSE_CYCLES_PER_SECOND, MAX_PULSE_CYCLES_PER_SECOND)
}

fn grid_anchor_for_port(grid_rect: Rect, location: PortLocation) -> Pos2 {
    let fx = if location.x == u16::MAX {
        1.0
    } else {
        location.x as f32 / u16::MAX as f32
    };
    let fy = if location.y == u16::MAX {
        1.0
    } else {
        location.y as f32 / u16::MAX as f32
    };
    Pos2::new(
        grid_rect.left() + grid_rect.width() * fx,
        grid_rect.top() + grid_rect.height() * fy,
    )
}

fn child_port_anchor(child_rect: Rect, location: PortLocation) -> Pos2 {
    let mut anchor = grid_anchor_for_port(child_rect, location);
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

fn child_rect_from_placement(grid_rect: Rect, placement: ChildPlacement) -> Rect {
    let min = Pos2::new(
        grid_rect.left() + grid_rect.width() * placement.min[0].clamp(0.0, 1.0),
        grid_rect.top() + grid_rect.height() * placement.min[1].clamp(0.0, 1.0),
    );
    let max = Pos2::new(
        grid_rect.left() + grid_rect.width() * placement.max[0].clamp(0.0, 1.0),
        grid_rect.top() + grid_rect.height() * placement.max[1].clamp(0.0, 1.0),
    );
    Rect::from_min_max(min, max)
}

fn child_preview_gates(rect: Rect, plan: &crate::gate_plans::ComponentPlan) -> Vec<PlacedGate> {
    let grid = plan.grid_size;
    let child_cell_w = rect.width() / grid[0].max(1) as f32;
    let child_cell_h = rect.height() / grid[1].max(1) as f32;
    plan.gates
        .iter()
        .copied()
        .enumerate()
        .map(|(index, gate)| {
            let index = index as u32;
            let gx = index % grid[0];
            let gy = index / grid[0];
            let min = rect.min + Vec2::new(gx as f32 * child_cell_w, gy as f32 * child_cell_h);
            PlacedGate {
                id: GateId(index),
                gate,
                rect: Rect::from_min_size(min, Vec2::new(child_cell_w, child_cell_h)),
            }
        })
        .collect()
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

fn gate_value(read_words: &[u32], words_per_buffer: u32, store: GateStoreLocation) -> bool {
    let absolute_word = store.buffer.0.saturating_mul(words_per_buffer) + (store.bit.0 / 32);
    read_words
        .get(absolute_word as usize)
        .is_some_and(|word| ((word >> store.bit_in_word) & 1) != 0)
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

fn polyline_length(points: &[Pos2]) -> f32 {
    points
        .windows(2)
        .map(|segment| segment[0].distance(segment[1]))
        .sum()
}

fn point_along_polyline(points: &[Pos2], t: f32) -> Pos2 {
    if points.len() < 2 {
        return points.first().copied().unwrap_or_default();
    }
    let total = polyline_length(points);
    if total <= f32::EPSILON {
        return points[0];
    }
    let mut remaining = total * t.clamp(0.0, 1.0);
    for segment in points.windows(2) {
        let length = segment[0].distance(segment[1]);
        if remaining <= length {
            return segment[0].lerp(segment[1], remaining / length.max(f32::EPSILON));
        }
        remaining -= length;
    }
    *points.last().expect("len checked above")
}
