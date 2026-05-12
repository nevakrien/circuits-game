use egui::{Color32, Pos2, Rect, Vec2};

use crate::{
    editor::{local_wire_point_to_pos, pos_to_wire_point},
    gate_plans::{ChildId, GateId, NodeId, WireEndpoint, WirePoint},
    ui_config::CELL,
    visual_ui::{
        FocusedScene, PlacedChild, PlacedGate, SceneViewportOutput, ViewportState, VisualWire,
        screen_to_world,
    },
};

const CHILD_FOOTPRINT_FILL: f32 = 0.88;
const WIRE_ENDPOINT_RADIUS: f32 = CELL * 0.35;
const WIRE_BEND_RADIUS: f32 = CELL * 0.35;
const WIRE_SEGMENT_RADIUS: f32 = CELL * 0.25;
const GATE_HIT_PADDING: f32 = CELL * 0.18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditSceneAction {
    ClearSelection,
    FocusChild(NodeId),
    SelectChild(ChildId),
    SelectGate(GateId),
    SelectWire {
        from: WireEndpoint,
        to: WireEndpoint,
    },
    MoveChild {
        child: ChildId,
        delta_cells: [i32; 2],
    },
    MoveGate {
        gate: GateId,
        delta_cells: [i32; 2],
    },
    MoveWireBend {
        from: WireEndpoint,
        to: WireEndpoint,
        bend_index: usize,
        point: WirePoint,
    },
    InsertWireBend {
        from: WireEndpoint,
        to: WireEndpoint,
        bend_index: usize,
        point: WirePoint,
    },
    RewireWireSource {
        from: WireEndpoint,
        to: WireEndpoint,
        new_from: WireEndpoint,
    },
    RewireWireTarget {
        from: WireEndpoint,
        to: WireEndpoint,
        new_to: WireEndpoint,
    },
    ConnectWire {
        from: WireEndpoint,
        to: WireEndpoint,
    },
    CreateDanglingWire {
        from: WireEndpoint,
        point: WirePoint,
    },
    MoveDanglingWire {
        from: WireEndpoint,
        from_point: WirePoint,
        to_point: WirePoint,
    },
    ConnectDanglingWire {
        from: WireEndpoint,
        point: WirePoint,
        to: WireEndpoint,
    },
    DetachWire {
        from: WireEndpoint,
        to: WireEndpoint,
        point: WirePoint,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EditInteractionState {
    drag: Option<DragState>,
}

#[derive(Debug, Clone, Copy)]
enum DragState {
    Child(ChildDragState),
    Gate(GateDragState),
    WireBend(WireBendDragState),
    WireEndpoint(WireEndpointDragState),
    NewWire(NewWireDragState),
    DanglingWire(DanglingWireDragState),
}

#[derive(Debug, Clone, Copy)]
struct ChildDragState {
    child: ChildId,
    origin_rect_min: Pos2,
    origin_footprint_min: Pos2,
    preview_footprint_min: Pos2,
    pointer_offset: Vec2,
    moved: bool,
}

#[derive(Debug, Clone, Copy)]
struct GateDragState {
    gate: GateId,
    origin_rect_min: Pos2,
    preview_rect_min: Pos2,
    pointer_offset: Vec2,
    moved: bool,
}

#[derive(Debug, Clone, Copy)]
struct WireBendDragState {
    from: WireEndpoint,
    to: WireEndpoint,
    bend_index: usize,
    pending_insert: bool,
    preview_point: Option<WirePoint>,
    moved: bool,
}

#[derive(Debug, Clone, Copy)]
struct WireEndpointDragState {
    original_from: WireEndpoint,
    original_to: WireEndpoint,
    preview_from: Option<WireEndpoint>,
    preview_to: Option<WireEndpoint>,
    side: WireEndpointSide,
    moved: bool,
}

#[derive(Debug, Clone, Copy)]
struct NewWireDragState {
    from: WireEndpoint,
    preview_to: Option<WireEndpoint>,
    preview_point: Option<WirePoint>,
    moved: bool,
}

#[derive(Debug, Clone, Copy)]
struct DanglingWireDragState {
    from: WireEndpoint,
    origin_point: WirePoint,
    preview_to: Option<WireEndpoint>,
    preview_point: WirePoint,
    moved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireEndpointSide {
    Source,
    Target,
}

impl EditInteractionState {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn clear(&mut self) {
        self.drag = None;
    }
}

pub fn interact_edit_scene(
    scene: &FocusedScene,
    viewport: &ViewportState,
    viewport_output: &SceneViewportOutput,
    state: &mut EditInteractionState,
) -> Option<EditSceneAction> {
    let pointer_world = viewport_output
        .pointer_screen
        .map(|pointer| screen_to_world(pointer, viewport_output.rect, viewport));

    if viewport_output.primary_drag_started {
        state.drag = pointer_world.and_then(|pointer| drag_target_at_pointer(scene, pointer));
    }

    if viewport_output.primary_dragged {
        if let (Some(pointer_world), Some(drag)) = (pointer_world, state.drag) {
            match drag {
                DragState::Child(mut drag) => {
                    if !scene.children.iter().any(|child| child.id == drag.child) {
                        state.drag = None;
                        return None;
                    }
                    let desired_min = pointer_world - drag.pointer_offset;
                    drag.preview_footprint_min = desired_min;
                    let delta_world = desired_min - drag.origin_footprint_min;
                    let delta_cells = [
                        quantize_drag_axis(delta_world.x),
                        quantize_drag_axis(delta_world.y),
                    ];
                    drag.moved |= delta_cells != [0, 0];
                    state.drag = Some(DragState::Child(drag));
                }
                DragState::Gate(mut drag) => {
                    if !scene.gates.iter().any(|gate| gate.id == drag.gate) {
                        state.drag = None;
                        return None;
                    }
                    let desired_min = pointer_world - drag.pointer_offset;
                    drag.preview_rect_min = desired_min;
                    let delta_world = desired_min - drag.origin_rect_min;
                    let delta_cells = [
                        quantize_drag_axis(delta_world.x),
                        quantize_drag_axis(delta_world.y),
                    ];
                    drag.moved |= delta_cells != [0, 0];
                    state.drag = Some(DragState::Gate(drag));
                }
                DragState::WireBend(mut drag) => {
                    let point = pos_to_wire_point(pointer_world);
                    drag.preview_point = Some(point);
                    drag.moved = true;
                    state.drag = Some(DragState::WireBend(drag));
                }
                DragState::WireEndpoint(mut drag) => {
                    match drag.side {
                        WireEndpointSide::Source => {
                            drag.preview_from =
                                source_endpoint_at_pointer(scene, pointer_world, drag.original_to);
                            drag.moved = true;
                        }
                        WireEndpointSide::Target => {
                            drag.preview_to = target_endpoint_at_pointer(
                                scene,
                                pointer_world,
                                drag.original_from,
                            );
                            drag.moved = true;
                        }
                    }
                    state.drag = Some(DragState::WireEndpoint(drag));
                }
                DragState::NewWire(mut drag) => {
                    drag.preview_to = target_endpoint_at_pointer(scene, pointer_world, drag.from);
                    drag.preview_point = Some(pos_to_wire_point(pointer_world));
                    drag.moved = true;
                    state.drag = Some(DragState::NewWire(drag));
                }
                DragState::DanglingWire(mut drag) => {
                    drag.preview_to = target_endpoint_at_pointer(scene, pointer_world, drag.from);
                    drag.preview_point = pos_to_wire_point(pointer_world);
                    drag.moved = true;
                    state.drag = Some(DragState::DanglingWire(drag));
                }
            }
        }
    }

    if viewport_output.primary_drag_stopped {
        if let (Some(pointer_world), Some(drag)) = (pointer_world, state.drag) {
            let action = match drag {
                DragState::Child(mut drag) => {
                    drag.preview_footprint_min = pointer_world - drag.pointer_offset;
                    finish_child_drag(drag)
                }
                DragState::Gate(mut drag) => {
                    drag.preview_rect_min = pointer_world - drag.pointer_offset;
                    finish_gate_drag(drag)
                }
                DragState::WireBend(mut drag) => {
                    drag.preview_point = Some(pos_to_wire_point(pointer_world));
                    finish_wire_bend_drag(drag)
                }
                DragState::WireEndpoint(drag) => finish_wire_endpoint_drag(drag, pointer_world),
                DragState::NewWire(mut drag) => {
                    drag.preview_point = Some(pos_to_wire_point(pointer_world));
                    finish_new_wire_drag(drag)
                }
                DragState::DanglingWire(mut drag) => {
                    drag.preview_point = pos_to_wire_point(pointer_world);
                    finish_dangling_wire_drag(drag)
                }
            };
            state.drag = None;
            if action.is_some() {
                return action;
            }
        }
        state.drag = None;
    }
    let drag_moved = state.drag.is_some_and(drag_moved);
    if drag_moved {
        return None;
    }

    let hovered_child = child_at_pointer(scene, pointer_world);
    if viewport_output.primary_double_clicked {
        return hovered_child.map(|child| EditSceneAction::FocusChild(child.node));
    }
    if viewport_output.primary_clicked {
        return Some(selection_action_at_pointer(scene, pointer_world));
    }
    None
}

fn selection_action_at_pointer(
    scene: &FocusedScene,
    pointer_world: Option<Pos2>,
) -> EditSceneAction {
    let Some(pointer_world) = pointer_world else {
        return EditSceneAction::ClearSelection;
    };
    if let Some((from, to, _)) = wire_endpoint_hit(scene, pointer_world) {
        return EditSceneAction::SelectWire { from, to };
    }
    if let Some(gate) = gate_at_pointer(scene, pointer_world) {
        return EditSceneAction::SelectGate(gate.id);
    }
    if let Some((from, to, _)) = wire_bend_hit(scene, pointer_world) {
        return EditSceneAction::SelectWire { from, to };
    }
    if let Some((from, to, _)) = wire_segment_hit(scene, pointer_world) {
        return EditSceneAction::SelectWire { from, to };
    }
    child_at_pointer(scene, Some(pointer_world))
        .map(|child| EditSceneAction::SelectChild(child.id))
        .unwrap_or(EditSceneAction::ClearSelection)
}

fn drag_target_at_pointer(scene: &FocusedScene, pointer_world: Pos2) -> Option<DragState> {
    if let Some((from, to, side)) = wire_endpoint_hit(scene, pointer_world) {
        return Some(DragState::WireEndpoint(WireEndpointDragState {
            original_from: from,
            original_to: to,
            preview_from: Some(from),
            preview_to: Some(to),
            side,
            moved: false,
        }));
    }
    if let Some((from, point)) = dangling_wire_endpoint_hit(scene, pointer_world) {
        return Some(DragState::DanglingWire(DanglingWireDragState {
            from,
            origin_point: point,
            preview_to: None,
            preview_point: point,
            moved: false,
        }));
    }
    if let Some((from, to, bend_index)) = wire_bend_hit(scene, pointer_world) {
        return Some(DragState::WireBend(WireBendDragState {
            from,
            to,
            bend_index,
            pending_insert: false,
            preview_point: None,
            moved: false,
        }));
    }
    if let Some(gate) = gate_at_pointer(scene, pointer_world) {
        return Some(DragState::Gate(GateDragState {
            gate: gate.id,
            origin_rect_min: gate.rect.min,
            preview_rect_min: gate.rect.min,
            pointer_offset: pointer_world - gate.rect.min,
            moved: false,
        }));
    }
    if let Some((from, to, bend_index)) = wire_segment_hit(scene, pointer_world) {
        return Some(DragState::WireBend(WireBendDragState {
            from,
            to,
            bend_index,
            pending_insert: true,
            preview_point: None,
            moved: false,
        }));
    }
    if let Some(from) = source_endpoint_hit(scene, pointer_world) {
        return Some(DragState::NewWire(NewWireDragState {
            from,
            preview_to: None,
            preview_point: None,
            moved: false,
        }));
    }
    child_at_pointer(scene, Some(pointer_world)).map(|child| {
        let footprint = child_footprint_rect(child.rect);
        DragState::Child(ChildDragState {
            child: child.id,
            origin_rect_min: child.rect.min,
            origin_footprint_min: footprint.min,
            preview_footprint_min: footprint.min,
            pointer_offset: pointer_world - footprint.min,
            moved: false,
        })
    })
}

fn drag_moved(drag: DragState) -> bool {
    match drag {
        DragState::Child(drag) => drag.moved,
        DragState::Gate(drag) => drag.moved,
        DragState::WireBend(drag) => drag.moved,
        DragState::WireEndpoint(drag) => drag.moved,
        DragState::NewWire(drag) => drag.moved,
        DragState::DanglingWire(drag) => drag.moved,
    }
}

fn finish_child_drag(drag: ChildDragState) -> Option<EditSceneAction> {
    let delta_world = drag.preview_footprint_min - drag.origin_footprint_min;
    let delta_cells = [
        quantize_drag_axis(delta_world.x),
        quantize_drag_axis(delta_world.y),
    ];
    (delta_cells != [0, 0]).then_some(EditSceneAction::MoveChild {
        child: drag.child,
        delta_cells,
    })
}

fn finish_gate_drag(drag: GateDragState) -> Option<EditSceneAction> {
    let delta_world = drag.preview_rect_min - drag.origin_rect_min;
    let delta_cells = [
        quantize_drag_axis(delta_world.x),
        quantize_drag_axis(delta_world.y),
    ];
    (delta_cells != [0, 0]).then_some(EditSceneAction::MoveGate {
        gate: drag.gate,
        delta_cells,
    })
}

fn finish_wire_bend_drag(drag: WireBendDragState) -> Option<EditSceneAction> {
    let point = drag.preview_point?;
    if !drag.moved {
        return None;
    }
    if drag.pending_insert {
        Some(EditSceneAction::InsertWireBend {
            from: drag.from,
            to: drag.to,
            bend_index: drag.bend_index,
            point,
        })
    } else {
        Some(EditSceneAction::MoveWireBend {
            from: drag.from,
            to: drag.to,
            bend_index: drag.bend_index,
            point,
        })
    }
}

fn finish_wire_endpoint_drag(
    drag: WireEndpointDragState,
    pointer_world: Pos2,
) -> Option<EditSceneAction> {
    if !drag.moved {
        return None;
    }
    match drag.side {
        WireEndpointSide::Source if drag.preview_from != Some(drag.original_from) => {
            Some(EditSceneAction::RewireWireSource {
                from: drag.original_from,
                to: drag.original_to,
                new_from: drag.preview_from?,
            })
        }
        WireEndpointSide::Target if drag.preview_to != Some(drag.original_to) => {
            if let Some(new_to) = drag.preview_to {
                Some(EditSceneAction::RewireWireTarget {
                    from: drag.original_from,
                    to: drag.original_to,
                    new_to,
                })
            } else {
                Some(EditSceneAction::DetachWire {
                    from: drag.original_from,
                    to: drag.original_to,
                    point: pos_to_wire_point(pointer_world),
                })
            }
        }
        _ => None,
    }
}

fn finish_new_wire_drag(drag: NewWireDragState) -> Option<EditSceneAction> {
    if !drag.moved {
        return None;
    }
    if let Some(to) = drag.preview_to {
        Some(EditSceneAction::ConnectWire {
            from: drag.from,
            to,
        })
    } else {
        Some(EditSceneAction::CreateDanglingWire {
            from: drag.from,
            point: drag.preview_point?,
        })
    }
}

fn finish_dangling_wire_drag(drag: DanglingWireDragState) -> Option<EditSceneAction> {
    if !drag.moved {
        return None;
    }
    if let Some(to) = drag.preview_to {
        Some(EditSceneAction::ConnectDanglingWire {
            from: drag.from,
            point: drag.origin_point,
            to,
        })
    } else {
        Some(EditSceneAction::MoveDanglingWire {
            from: drag.from,
            from_point: drag.origin_point,
            to_point: drag.preview_point,
        })
    }
}

pub fn child_at_pointer(scene: &FocusedScene, pointer_world: Option<Pos2>) -> Option<&PlacedChild> {
    let pointer_world = pointer_world?;
    scene
        .children
        .iter()
        .find(|child| child_footprint_rect(child.rect).contains(pointer_world))
}

fn gate_at_pointer(scene: &FocusedScene, pointer_world: Pos2) -> Option<&PlacedGate> {
    scene
        .gates
        .iter()
        .find(|gate| gate.rect.expand(GATE_HIT_PADDING).contains(pointer_world))
}

pub fn apply_edit_scene_drag_preview(
    scene: &mut FocusedScene,
    state: &EditInteractionState,
    pointer_world: Option<Pos2>,
) -> bool {
    match state.drag {
        Some(DragState::Child(drag)) => preview_child_drag(scene, drag),
        Some(DragState::Gate(drag)) => preview_gate_drag(scene, drag),
        Some(DragState::WireEndpoint(drag)) => {
            let Some(pointer_world) = pointer_world else {
                return false;
            };
            preview_wire_endpoint_drag(scene, drag, pointer_world)
        }
        Some(DragState::NewWire(drag)) => {
            let Some(pointer_world) = pointer_world else {
                return false;
            };
            preview_new_wire_drag(scene, drag, pointer_world)
        }
        Some(DragState::DanglingWire(drag)) => preview_dangling_wire_drag(scene, drag),
        Some(DragState::WireBend(drag)) => preview_wire_bend_drag(scene, drag),
        None => false,
    }
}

fn preview_child_drag(scene: &mut FocusedScene, drag: ChildDragState) -> bool {
    let Some(child) = scene
        .children
        .iter_mut()
        .find(|child| child.id == drag.child)
    else {
        return false;
    };
    let footprint_offset = drag.origin_footprint_min - drag.origin_rect_min;
    let target_min = drag.preview_footprint_min - footprint_offset;
    let delta = target_min - child.rect.min;
    if delta == Vec2::ZERO {
        return false;
    }
    child.rect = child.rect.translate(delta);
    for port in &mut child.inputs {
        port.anchor += delta;
    }
    for port in &mut child.outputs {
        port.anchor += delta;
    }
    refresh_wire_points(scene);
    true
}

fn preview_gate_drag(scene: &mut FocusedScene, drag: GateDragState) -> bool {
    let Some(gate) = scene.gates.iter_mut().find(|gate| gate.id == drag.gate) else {
        return false;
    };
    let delta = drag.preview_rect_min - gate.rect.min;
    if delta == Vec2::ZERO {
        return false;
    }
    gate.rect = gate.rect.translate(delta);
    refresh_wire_points(scene);
    true
}

fn preview_wire_endpoint_drag(
    scene: &mut FocusedScene,
    drag: WireEndpointDragState,
    pointer_world: Pos2,
) -> bool {
    let source_anchor = endpoint_anchor(scene, drag.original_from);
    let target_anchor = endpoint_anchor(scene, drag.original_to);
    let (start, end) = match drag.side {
        WireEndpointSide::Source => {
            let preview_start = drag
                .preview_from
                .and_then(|endpoint| endpoint_anchor(scene, endpoint))
                .unwrap_or(pointer_world);
            let Some(end) = target_anchor else {
                return false;
            };
            (preview_start, end)
        }
        WireEndpointSide::Target => {
            let Some(start) = source_anchor else {
                return false;
            };
            let preview_end = drag
                .preview_to
                .and_then(|endpoint| endpoint_anchor(scene, endpoint))
                .unwrap_or(pointer_world);
            (start, preview_end)
        }
    };

    let Some(wire) = scene
        .wires
        .iter_mut()
        .find(|wire| wire.from == Some(drag.original_from) && wire.to == Some(drag.original_to))
    else {
        return false;
    };
    wire.points = wire_points_from_layout(start, end, &wire.bends);
    true
}

fn preview_wire_bend_drag(scene: &mut FocusedScene, drag: WireBendDragState) -> bool {
    let Some(point) = drag.preview_point else {
        return false;
    };
    let Some(wire) = scene
        .wires
        .iter_mut()
        .find(|wire| wire.from == Some(drag.from) && wire.to == Some(drag.to))
    else {
        return false;
    };

    let mut bends = wire.bends.clone();
    if drag.pending_insert {
        let index = drag.bend_index.min(bends.len());
        bends.insert(index, point);
    } else {
        let Some(bend) = bends.get_mut(drag.bend_index) else {
            return false;
        };
        *bend = point;
    }

    let Some(start) = wire.points.first().copied() else {
        return false;
    };
    let Some(end) = wire.points.last().copied() else {
        return false;
    };
    wire.points = wire_points_from_layout(start, end, &bends);
    true
}

fn preview_new_wire_drag(
    scene: &mut FocusedScene,
    drag: NewWireDragState,
    pointer_world: Pos2,
) -> bool {
    let Some(start) = endpoint_anchor(scene, drag.from) else {
        return false;
    };
    let end = drag
        .preview_to
        .and_then(|endpoint| endpoint_anchor(scene, endpoint))
        .unwrap_or(pointer_world);
    let points = wire_points_from_layout(start, end, &[]);
    if let Some(wire) = scene
        .wires
        .iter_mut()
        .find(|wire| wire.from == Some(drag.from) && wire.to.is_none())
    {
        wire.points = points;
    } else {
        scene.wires.push(VisualWire {
            source_gate: None,
            color: Color32::from_rgb(120, 180, 255),
            points,
            from: Some(drag.from),
            to: None,
            bends: Vec::new(),
        });
    }
    true
}

fn preview_dangling_wire_drag(scene: &mut FocusedScene, drag: DanglingWireDragState) -> bool {
    let Some(start) = endpoint_anchor(scene, drag.from) else {
        return false;
    };
    let end = drag
        .preview_to
        .and_then(|endpoint| endpoint_anchor(scene, endpoint))
        .unwrap_or_else(|| local_wire_point_to_pos(&drag.preview_point));
    let Some(wire) = scene.wires.iter_mut().find(|wire| {
        wire.from == Some(drag.from)
            && wire.to.is_none()
            && wire
                .points
                .last()
                .is_some_and(|point| *point == local_wire_point_to_pos(&drag.origin_point))
    }) else {
        return false;
    };
    wire.points = wire_points_from_layout(start, end, &[]);
    true
}

fn refresh_wire_points(scene: &mut FocusedScene) {
    for index in 0..scene.wires.len() {
        let (from, to, bends) = {
            let wire = &scene.wires[index];
            (wire.from, wire.to, wire.bends.clone())
        };
        if let Some((from, to)) = from.zip(to) {
            let Some(start) = endpoint_anchor(scene, from) else {
                continue;
            };
            let Some(end) = endpoint_anchor(scene, to) else {
                continue;
            };
            scene.wires[index].points = wire_points_from_layout(start, end, &bends);
        } else if let Some(from) = from {
            let Some(start) = endpoint_anchor(scene, from) else {
                continue;
            };
            let Some(end) = scene.wires[index].points.last().copied() else {
                continue;
            };
            scene.wires[index].points = wire_points_from_layout(start, end, &[]);
        }
    }
}

fn wire_points_from_layout(start: Pos2, end: Pos2, bends: &[WirePoint]) -> Vec<Pos2> {
    if bends.is_empty() {
        return preview_orth_wire_points(start, end);
    }
    let mut points = Vec::with_capacity(bends.len() + 2);
    points.push(start);
    points.extend(bends.iter().map(local_wire_point_to_pos));
    points.push(end);
    points
}

fn source_endpoint_at_pointer(
    scene: &FocusedScene,
    pointer_world: Pos2,
    current_to: WireEndpoint,
) -> Option<WireEndpoint> {
    endpoint_candidates(scene, WireEndpointSide::Source)
        .into_iter()
        .filter(|(endpoint, _)| source_endpoint_allowed(*endpoint, current_to))
        .filter_map(|(endpoint, pos)| {
            let distance = pos.distance(pointer_world);
            (distance <= WIRE_ENDPOINT_RADIUS).then_some((distance, endpoint))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, endpoint)| endpoint)
}

fn source_endpoint_hit(scene: &FocusedScene, pointer_world: Pos2) -> Option<WireEndpoint> {
    endpoint_candidates(scene, WireEndpointSide::Source)
        .into_iter()
        .filter_map(|(endpoint, pos)| {
            let distance = pos.distance(pointer_world);
            (distance <= WIRE_ENDPOINT_RADIUS).then_some((distance, endpoint))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, endpoint)| endpoint)
}

fn target_endpoint_at_pointer(
    scene: &FocusedScene,
    pointer_world: Pos2,
    current_from: WireEndpoint,
) -> Option<WireEndpoint> {
    endpoint_candidates(scene, WireEndpointSide::Target)
        .into_iter()
        .filter(|(endpoint, _)| target_endpoint_allowed(scene, *endpoint, current_from))
        .filter_map(|(endpoint, pos)| {
            let distance = pos.distance(pointer_world);
            (distance <= WIRE_ENDPOINT_RADIUS).then_some((distance, endpoint))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, endpoint)| endpoint)
}

fn wire_endpoint_hit(
    scene: &FocusedScene,
    pointer_world: Pos2,
) -> Option<(WireEndpoint, WireEndpoint, WireEndpointSide)> {
    scene
        .wires
        .iter()
        .filter_map(connected_wire)
        .filter_map(|(wire, from, to)| {
            let start = wire.points.first().copied()?;
            let end = wire.points.last().copied()?;
            let start_distance = start.distance(pointer_world);
            let end_distance = end.distance(pointer_world);
            let mut best = None;
            if start_distance <= WIRE_ENDPOINT_RADIUS {
                best = Some((start_distance, from, to, WireEndpointSide::Source));
            }
            if end_distance <= WIRE_ENDPOINT_RADIUS {
                let candidate = (end_distance, from, to, WireEndpointSide::Target);
                best = match best {
                    Some(current) if current.0 <= candidate.0 => Some(current),
                    _ => Some(candidate),
                };
            }
            best
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, from, to, side)| (from, to, side))
}

fn dangling_wire_endpoint_hit(
    scene: &FocusedScene,
    pointer_world: Pos2,
) -> Option<(WireEndpoint, WirePoint)> {
    scene
        .wires
        .iter()
        .filter_map(|wire| Some((wire, wire.from?)).filter(|(wire, _)| wire.to.is_none()))
        .filter_map(|(wire, from)| {
            let end = wire.points.last().copied()?;
            let distance = end.distance(pointer_world);
            (distance <= WIRE_ENDPOINT_RADIUS).then_some((distance, from, pos_to_wire_point(end)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, from, point)| (from, point))
}

fn wire_bend_hit(
    scene: &FocusedScene,
    pointer_world: Pos2,
) -> Option<(WireEndpoint, WireEndpoint, usize)> {
    scene
        .wires
        .iter()
        .filter_map(connected_wire)
        .flat_map(|(_, from, to)| {
            let wire = scene
                .wires
                .iter()
                .find(|wire| wire.from == Some(from) && wire.to == Some(to))
                .into_iter();
            wire.flat_map(move |wire| {
                wire.bends.iter().enumerate().map(move |(index, bend)| {
                    (
                        local_wire_point_to_pos(bend).distance(pointer_world),
                        from,
                        to,
                        index,
                    )
                })
            })
        })
        .filter(|(distance, _, _, _)| *distance <= WIRE_BEND_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, from, to, index)| (from, to, index))
}

fn wire_segment_hit(
    scene: &FocusedScene,
    pointer_world: Pos2,
) -> Option<(WireEndpoint, WireEndpoint, usize)> {
    scene
        .wires
        .iter()
        .filter_map(connected_wire)
        .filter_map(|(wire, from, to)| {
            wire.points
                .windows(2)
                .enumerate()
                .map(|(index, segment)| {
                    (
                        distance_to_segment(pointer_world, segment[0], segment[1]),
                        from,
                        to,
                        index,
                    )
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        })
        .filter(|(distance, _, _, _)| *distance <= WIRE_SEGMENT_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, from, to, index)| (from, to, index))
}

fn connected_wire(wire: &VisualWire) -> Option<(&VisualWire, WireEndpoint, WireEndpoint)> {
    Some((wire, wire.from?, wire.to?))
}

fn endpoint_candidates(scene: &FocusedScene, side: WireEndpointSide) -> Vec<(WireEndpoint, Pos2)> {
    let mut endpoints = Vec::new();
    match side {
        WireEndpointSide::Source => {
            for port in &scene.input_ports {
                endpoints.push((WireEndpoint::ComponentInput(port.id), port.anchor));
            }
            for gate in &scene.gates {
                endpoints.push((WireEndpoint::GateOutput(gate.id), gate_output_anchor(gate)));
            }
            for child in &scene.children {
                for port in &child.outputs {
                    endpoints.push((
                        WireEndpoint::ChildOutput {
                            child: child.id,
                            port: port.id,
                        },
                        port.anchor,
                    ));
                }
            }
            for port in &scene.ancestor_ports {
                endpoints.push((
                    WireEndpoint::AncestorOutput {
                        depth: crate::gate_plans::AncestorDepth(1),
                        port: port.port,
                    },
                    port.anchor,
                ));
            }
        }
        WireEndpointSide::Target => {
            for gate in &scene.gates {
                for input in 0..gate_input_count(gate) {
                    endpoints.push((
                        WireEndpoint::GateInput {
                            gate: gate.id,
                            input: input as u8,
                        },
                        gate_input_anchor(gate, input),
                    ));
                }
            }
            for child in &scene.children {
                for port in &child.inputs {
                    endpoints.push((
                        WireEndpoint::ChildInput {
                            child: child.id,
                            port: port.id,
                        },
                        port.anchor,
                    ));
                }
            }
            for port in &scene.output_ports {
                endpoints.push((WireEndpoint::ComponentOutput(port.id), port.anchor));
            }
        }
    }
    endpoints
}

fn source_endpoint_allowed(candidate: WireEndpoint, current_to: WireEndpoint) -> bool {
    !matches!(current_to, WireEndpoint::ComponentOutput(_))
        || matches!(candidate, WireEndpoint::GateOutput(_))
}

fn target_endpoint_allowed(
    _scene: &FocusedScene,
    candidate: WireEndpoint,
    current_from: WireEndpoint,
) -> bool {
    if matches!(candidate, WireEndpoint::ComponentOutput(_))
        && !matches!(current_from, WireEndpoint::GateOutput(_))
    {
        return false;
    }
    if matches!(
        (current_from, candidate),
        (
            WireEndpoint::ChildOutput { child: source_child, .. },
            WireEndpoint::ChildInput { child: target_child, .. },
        ) if source_child == target_child
    ) {
        return false;
    }
    matches!(candidate, WireEndpoint::GateInput { .. })
        || matches!(candidate, WireEndpoint::ChildInput { .. })
        || matches!(candidate, WireEndpoint::ComponentOutput(_))
}

fn endpoint_anchor(scene: &FocusedScene, endpoint: WireEndpoint) -> Option<Pos2> {
    match endpoint {
        WireEndpoint::ComponentInput(port) => scene
            .input_ports
            .iter()
            .find(|candidate| candidate.id == port)
            .map(|port| port.anchor),
        WireEndpoint::ComponentOutput(port) => scene
            .output_ports
            .iter()
            .find(|candidate| candidate.id == port)
            .map(|port| port.anchor),
        WireEndpoint::GateOutput(gate) => scene
            .gates
            .iter()
            .find(|candidate| candidate.id == gate)
            .map(gate_output_anchor),
        WireEndpoint::GateInput { gate, input } => scene
            .gates
            .iter()
            .find(|candidate| candidate.id == gate)
            .map(|gate| gate_input_anchor(gate, input as usize)),
        WireEndpoint::ChildOutput { child, port } => scene
            .children
            .iter()
            .find(|candidate| candidate.id == child)
            .and_then(|child| child.outputs.iter().find(|candidate| candidate.id == port))
            .map(|port| port.anchor),
        WireEndpoint::ChildInput { child, port } => scene
            .children
            .iter()
            .find(|candidate| candidate.id == child)
            .and_then(|child| child.inputs.iter().find(|candidate| candidate.id == port))
            .map(|port| port.anchor),
        WireEndpoint::AncestorOutput { port, .. } => scene
            .ancestor_ports
            .iter()
            .find(|candidate| candidate.port == port)
            .map(|port| port.anchor),
    }
}

fn preview_orth_wire_points(start: Pos2, end: Pos2) -> Vec<Pos2> {
    let mid_x = (start.x + end.x) * 0.5;
    vec![
        start,
        Pos2::new(mid_x, start.y),
        Pos2::new(mid_x, end.y),
        end,
    ]
}

fn gate_output_anchor(gate: &PlacedGate) -> Pos2 {
    Pos2::new(
        gate.rect.right() - gate.rect.width() * 0.08,
        gate.rect.center().y,
    )
}

fn gate_input_anchor(gate: &PlacedGate, input: usize) -> Pos2 {
    let y = match input {
        0 if gate_input_count(gate) == 1 => 0.5,
        0 => 0.3,
        _ => 0.7,
    };
    Pos2::new(
        gate.rect.left() + gate.rect.width() * 0.08,
        gate.rect.top() + gate.rect.height() * y,
    )
}

fn gate_input_count(gate: &PlacedGate) -> usize {
    match gate.gate {
        crate::gate_plans::Gate::BitNot { .. } | crate::gate_plans::Gate::BitNop { .. } => 1,
        _ => 2,
    }
}

fn child_footprint_rect(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() / CHILD_FOOTPRINT_FILL)
}

fn quantize_drag_axis(delta: f32) -> i32 {
    if delta >= 0.0 {
        (delta / CELL).floor() as i32
    } else {
        (delta / CELL).ceil() as i32
    }
}

fn distance_to_segment(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let delta = end - start;
    let length_sq = delta.length_sq();
    if length_sq <= f32::EPSILON {
        return point.distance(start);
    }
    let t = ((point - start).dot(delta) / length_sq).clamp(0.0, 1.0);
    let projection = start + delta * t;
    point.distance(projection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        editor::{ComponentDefId, EditableComponentDef, EditorDocument},
        gate_plans::{
            ChildPlacement, ComponentLayout, ComponentPlan, Gate, GateId, PlanId, SignalRef,
        },
    };
    use egui::{Rect, Vec2};
    use foldhash::HashMap;

    #[test]
    fn child_drag_commits_single_snapped_move_on_release() {
        let document = stress_drag_document();
        let viewport = ViewportState::default();
        let available = Vec2::new(6000.0, 4000.0);
        let focused = ComponentDefId(1);
        let target_child = ChildId(3);
        let mut interaction = EditInteractionState::default();

        let scene = document
            .build_edit_scene(focused, &viewport, available, None)
            .expect("scene should build");
        let child = scene
            .children
            .iter()
            .find(|child| child.id == target_child)
            .expect("target child should exist");
        let footprint = child_footprint_rect(child.rect);
        let start = footprint.min + Vec2::new(CELL * 0.5, CELL * 0.5);

        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(start, DragPhase::Started),
                &mut interaction,
            ),
            None
        );

        let first_move = interact_edit_scene(
            &scene,
            &viewport,
            &drag_output(start + Vec2::new(-CELL * 1.2, 0.0), DragPhase::Dragging),
            &mut interaction,
        );
        assert_eq!(first_move, None);

        let second_move = interact_edit_scene(
            &scene,
            &viewport,
            &drag_output(start + Vec2::new(-CELL * 2.2, 0.0), DragPhase::Dragging),
            &mut interaction,
        );
        assert_eq!(second_move, None);

        let commit_move = interact_edit_scene(
            &scene,
            &viewport,
            &drag_output(start + Vec2::new(-CELL * 2.2, 0.0), DragPhase::Stopped),
            &mut interaction,
        );
        assert_eq!(
            commit_move,
            Some(EditSceneAction::MoveChild {
                child: target_child,
                delta_cells: [-2, 0],
            })
        );
    }

    #[test]
    fn child_drag_preview_moves_rect_without_scene_rebuild() {
        let document = stress_drag_document();
        let viewport = ViewportState::default();
        let available = Vec2::new(6000.0, 4000.0);
        let focused = ComponentDefId(1);
        let target_child = ChildId(3);
        let mut interaction = EditInteractionState::default();

        let scene = document
            .build_edit_scene(focused, &viewport, available, None)
            .expect("scene should build");
        let child = scene
            .children
            .iter()
            .find(|child| child.id == target_child)
            .expect("target child should exist");
        let footprint = child_footprint_rect(child.rect);
        let start = footprint.min + Vec2::new(CELL * 0.5, CELL * 0.5);

        interact_edit_scene(
            &scene,
            &viewport,
            &drag_output(start, DragPhase::Started),
            &mut interaction,
        );
        interact_edit_scene(
            &scene,
            &viewport,
            &drag_output(start + Vec2::new(-CELL * 1.2, 0.0), DragPhase::Dragging),
            &mut interaction,
        );

        let mut preview_scene = scene.clone();
        assert!(apply_edit_scene_drag_preview(
            &mut preview_scene,
            &interaction,
            Some(start + Vec2::new(-CELL * 1.2, 0.0)),
        ));

        let preview_child = preview_scene
            .children
            .iter()
            .find(|child| child.id == target_child)
            .expect("preview child should exist");
        assert!(preview_child.rect.min.x < child.rect.min.x);
    }

    #[test]
    fn gate_hit_beats_wire_segment_hit_inside_gate_body() {
        let gate = PlacedGate {
            id: GateId(4),
            gate: crate::gate_plans::Gate::BitAND {
                a: SignalRef::InputPort(crate::gate_plans::PortId(0)),
                b: SignalRef::InputPort(crate::gate_plans::PortId(1)),
            },
            input_sources: [None, None],
            rect: Rect::from_min_size(Pos2::new(32.0, 32.0), Vec2::splat(CELL)),
        };
        let scene = FocusedScene {
            node: NodeId(0),
            title: String::new(),
            bounds: Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0)),
            words_per_buffer: 0,
            gate_store: std::sync::Arc::new(HashMap::default()),
            grid_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0)),
            grid_dims: [4, 4],
            input_ports: Vec::new(),
            output_ports: Vec::new(),
            gates: vec![gate],
            children: Vec::new(),
            drill_child: None,
            ancestor_ports: Vec::new(),
            wires: vec![VisualWire {
                source_gate: None,
                color: egui::Color32::WHITE,
                points: vec![
                    Pos2::new(20.0, 76.0),
                    Pos2::new(52.0, 76.0),
                    Pos2::new(100.0, 76.0),
                    Pos2::new(140.0, 76.0),
                ],
                from: Some(WireEndpoint::ComponentInput(crate::gate_plans::PortId(0))),
                to: Some(WireEndpoint::GateInput {
                    gate: GateId(4),
                    input: 0,
                }),
                bends: Vec::new(),
            }],
        };

        let drag = drag_target_at_pointer(&scene, Pos2::new(76.0, 76.0));

        assert!(matches!(
            drag,
            Some(DragState::Gate(GateDragState {
                gate: GateId(4),
                ..
            }))
        ));
    }

    #[test]
    fn wire_bend_drag_commits_once_on_release() {
        let viewport = ViewportState::default();
        let mut interaction = EditInteractionState::default();
        let scene = simple_wire_scene();
        let bend = local_wire_point_to_pos(&WirePoint { x: 256, y: 256 });
        let moved = bend + Vec2::new(CELL, 0.0);

        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(bend, DragPhase::Started),
                &mut interaction,
            ),
            None
        );
        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(moved, DragPhase::Dragging),
                &mut interaction,
            ),
            None
        );
        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(moved, DragPhase::Stopped),
                &mut interaction,
            ),
            Some(EditSceneAction::MoveWireBend {
                from: WireEndpoint::GateOutput(GateId(0)),
                to: WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 0,
                },
                bend_index: 0,
                point: pos_to_wire_point(moved),
            })
        );
    }

    #[test]
    fn wire_endpoint_drag_commits_once_on_release() {
        let viewport = ViewportState::default();
        let mut interaction = EditInteractionState::default();
        let scene = simple_wire_scene();
        let start = gate_output_anchor(
            scene
                .gates
                .iter()
                .find(|gate| gate.id == GateId(0))
                .unwrap(),
        );
        let new_start = gate_output_anchor(
            scene
                .gates
                .iter()
                .find(|gate| gate.id == GateId(1))
                .unwrap(),
        );

        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(start, DragPhase::Started),
                &mut interaction,
            ),
            None
        );
        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(new_start, DragPhase::Dragging),
                &mut interaction,
            ),
            None
        );
        assert_eq!(
            interact_edit_scene(
                &scene,
                &viewport,
                &drag_output(new_start, DragPhase::Stopped),
                &mut interaction,
            ),
            Some(EditSceneAction::RewireWireSource {
                from: WireEndpoint::GateOutput(GateId(0)),
                to: WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 0,
                },
                new_from: WireEndpoint::GateOutput(GateId(1)),
            })
        );
    }

    fn stress_drag_document() -> EditorDocument {
        let plan = PlanId(0);
        let mut plans = HashMap::default();
        plans.insert(
            plan,
            ComponentPlan::new(vec![Gate::BitNot {
                src: SignalRef::ThisGate(GateId(0)),
            }])
            .with_grid_size([256, 160]),
        );
        EditorDocument::new(
            plans,
            vec![
                EditableComponentDef {
                    plan,
                    children: Vec::new(),
                    child_input_connections: Vec::new(),
                    dangling_wires: Vec::new(),
                    layout: ComponentLayout::default(),
                },
                EditableComponentDef {
                    plan,
                    children: vec![ComponentDefId(0); 4],
                    child_input_connections: Vec::new(),
                    dangling_wires: Vec::new(),
                    layout: ComponentLayout::default().with_child_placements(vec![
                        ChildPlacement::at([0, 0]),
                        ChildPlacement::at([128, 0]),
                        ChildPlacement::at([0, 80]),
                        ChildPlacement::at([128, 80]),
                    ]),
                },
            ],
            ComponentDefId(1),
        )
        .expect("stress drag document should build")
    }

    fn simple_wire_scene() -> FocusedScene {
        let gate0 = PlacedGate {
            id: GateId(0),
            gate: Gate::BitNot {
                src: SignalRef::ThisGate(GateId(0)),
            },
            input_sources: [None, None],
            rect: Rect::from_min_size(Pos2::new(32.0, 32.0), Vec2::splat(CELL)),
        };
        let gate1 = PlacedGate {
            id: GateId(1),
            gate: Gate::BitNot {
                src: SignalRef::ThisGate(GateId(1)),
            },
            input_sources: [None, None],
            rect: Rect::from_min_size(Pos2::new(32.0, 128.0), Vec2::splat(CELL)),
        };
        let gate2 = PlacedGate {
            id: GateId(2),
            gate: Gate::BitAND {
                a: SignalRef::ThisGate(GateId(0)),
                b: SignalRef::ThisGate(GateId(1)),
            },
            input_sources: [Some((NodeId(0), GateId(0))), Some((NodeId(0), GateId(1)))],
            rect: Rect::from_min_size(Pos2::new(224.0, 64.0), Vec2::splat(CELL)),
        };
        let start = gate_output_anchor(&gate0);
        let end = gate_input_anchor(&gate2, 0);
        let bend = WirePoint { x: 256, y: 256 };
        FocusedScene {
            node: NodeId(0),
            title: String::new(),
            bounds: Rect::from_min_max(Pos2::ZERO, Pos2::new(400.0, 240.0)),
            words_per_buffer: 0,
            gate_store: std::sync::Arc::new(HashMap::default()),
            grid_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(400.0, 240.0)),
            grid_dims: [8, 5],
            input_ports: Vec::new(),
            output_ports: Vec::new(),
            gates: vec![gate0, gate1, gate2],
            children: Vec::new(),
            drill_child: None,
            ancestor_ports: Vec::new(),
            wires: vec![VisualWire {
                source_gate: Some((NodeId(0), GateId(0))),
                color: egui::Color32::WHITE,
                points: wire_points_from_layout(start, end, &[bend]),
                from: Some(WireEndpoint::GateOutput(GateId(0))),
                to: Some(WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 0,
                }),
                bends: vec![bend],
            }],
        }
    }

    #[derive(Clone, Copy)]
    enum DragPhase {
        Started,
        Dragging,
        Stopped,
    }

    fn drag_output(pointer_world: Pos2, phase: DragPhase) -> SceneViewportOutput {
        SceneViewportOutput {
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(8000.0, 8000.0)),
            pointer_screen: Some(pointer_world),
            primary_clicked: false,
            primary_double_clicked: false,
            primary_drag_started: matches!(phase, DragPhase::Started),
            primary_dragged: matches!(phase, DragPhase::Dragging),
            primary_drag_stopped: matches!(phase, DragPhase::Stopped),
            hover_world: Some(pointer_world),
            viewport_changed: false,
        }
    }
}
