use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Vec2};
use foldhash::HashMap;

use crate::gate_plans::{
    AncestorDepth, ChildId, ChildInputConnection, ChildPlacement, Component, ComponentPlan,
    ComponentPlans, Gate, GateId, NodeId, PlanId, PortId, PortLocation, SignalRef,
};
use crate::ui_config::{CELL, CHILD_PORT_INSET, PAD};
use crate::visual_ui::{
    ExternalPort, FocusedScene, PlacedChild, PlacedGate, PlacedPort, VisualWire,
};

pub const EDIT_PREVIEW_DEPTH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentDefId(pub usize);

#[derive(Debug, Clone)]
pub struct EditableComponentDef {
    pub plan: PlanId,
    pub children: Vec<ComponentDefId>,
    pub child_input_connections: Vec<ChildInputConnection>,
}

#[derive(Debug, Clone, Default)]
pub struct EditHistory {
    actions: Vec<EditAction>,
    cursor: usize,
}

#[derive(Debug, Clone)]
pub enum EditAction {
    MoveChild {
        component: ComponentDefId,
        child: ChildId,
        from: ChildPlacement,
        to: ChildPlacement,
    },
    Batch(Vec<EditAction>),
}

#[derive(Debug, Clone)]
pub struct EditorDocument {
    pub plans: HashMap<PlanId, ComponentPlan>,
    pub components: Vec<EditableComponentDef>,
    pub root: ComponentDefId,
    history: EditHistory,
}

impl EditAction {
    pub fn inverse(&self) -> Self {
        match self {
            Self::MoveChild {
                component,
                child,
                from,
                to,
            } => Self::MoveChild {
                component: *component,
                child: *child,
                from: *to,
                to: *from,
            },
            Self::Batch(actions) => Self::Batch(actions.iter().rev().map(Self::inverse).collect()),
        }
    }

    fn apply(&self, doc: &mut EditorDocument) -> Result<(), String> {
        match self {
            Self::MoveChild {
                component,
                child,
                to,
                ..
            } => {
                let component = doc
                    .components
                    .get(component.0)
                    .ok_or_else(|| format!("missing component {:?}", component))?
                    .clone();
                let plan = doc
                    .plans
                    .get_mut(&component.plan)
                    .ok_or_else(|| format!("missing plan {:?}", component.plan))?;
                let slot = plan
                    .child_placements
                    .get_mut(child.0 as usize)
                    .ok_or_else(|| format!("missing child placement {:?}", child))?;
                *slot = *to;
                Ok(())
            }
            Self::Batch(actions) => {
                for action in actions {
                    action.apply(doc)?;
                }
                Ok(())
            }
        }
    }
}

impl EditHistory {
    pub fn applied_len(&self) -> usize {
        self.cursor
    }

    pub fn redo_len(&self) -> usize {
        self.actions.len().saturating_sub(self.cursor)
    }

    fn push_and_apply(
        &mut self,
        doc: &mut EditorDocument,
        action: EditAction,
    ) -> Result<(), String> {
        if self.cursor < self.actions.len() {
            self.actions.truncate(self.cursor);
        }
        action.apply(doc)?;
        self.actions.push(action);
        self.cursor = self.actions.len();
        Ok(())
    }

    fn undo(&mut self, doc: &mut EditorDocument) -> Result<bool, String> {
        if self.cursor == 0 {
            return Ok(false);
        }
        let action = self.actions[self.cursor - 1].inverse();
        action.apply(doc)?;
        self.cursor -= 1;
        Ok(true)
    }

    fn redo(&mut self, doc: &mut EditorDocument) -> Result<bool, String> {
        let Some(action) = self.actions.get(self.cursor).cloned() else {
            return Ok(false);
        };
        action.apply(doc)?;
        self.cursor += 1;
        Ok(true)
    }
}

impl EditorDocument {
    pub fn new(
        plans: HashMap<PlanId, ComponentPlan>,
        components: Vec<EditableComponentDef>,
        root: ComponentDefId,
    ) -> Result<Self, String> {
        let doc = Self {
            plans,
            components,
            root,
            history: EditHistory::default(),
        };
        doc.validate()?;
        Ok(doc)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (component_id, component) in self.components.iter().enumerate() {
            let plan = self.plans.get(&component.plan).ok_or_else(|| {
                format!(
                    "component {} references missing plan {:?}",
                    component_id, component.plan
                )
            })?;
            if component.children.len() != plan.child_placements.len() {
                return Err(format!(
                    "component {} has {} children but plan {:?} has {} child placements",
                    component_id,
                    component.children.len(),
                    component.plan,
                    plan.child_placements.len()
                ));
            }
        }
        self.components
            .get(self.root.0)
            .ok_or_else(|| format!("missing root component {:?}", self.root))?;
        Ok(())
    }

    pub fn history(&self) -> &EditHistory {
        &self.history
    }

    pub fn component(&self, id: ComponentDefId) -> Option<&EditableComponentDef> {
        self.components.get(id.0)
    }

    pub fn plan(&self, id: PlanId) -> Option<&ComponentPlan> {
        self.plans.get(&id)
    }

    pub fn child_component(
        &self,
        component: ComponentDefId,
        child: ChildId,
    ) -> Option<ComponentDefId> {
        self.component(component)
            .and_then(|component| component.children.get(child.0 as usize).copied())
    }

    pub fn build_edit_scene(
        &self,
        focused: ComponentDefId,
        preview_depth: usize,
    ) -> Result<FocusedScene, String> {
        self.build_edit_scene_with_stack(&[], focused, preview_depth)
    }

    pub fn apply_action(&mut self, action: EditAction) -> Result<(), String> {
        let mut history = std::mem::take(&mut self.history);
        let result = history.push_and_apply(self, action);
        self.history = history;
        result
    }

    pub fn move_child_by(
        &mut self,
        component: ComponentDefId,
        child: ChildId,
        delta: [i32; 2],
    ) -> Result<bool, String> {
        let editable = self
            .component(component)
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let plan = self
            .plan(editable.plan)
            .ok_or_else(|| format!("missing plan {:?}", editable.plan))?;
        let child_component = editable
            .children
            .get(child.0 as usize)
            .copied()
            .ok_or_else(|| format!("missing child {:?}", child))?;
        let child_plan = self
            .component(child_component)
            .and_then(|child| self.plan(child.plan))
            .ok_or_else(|| format!("missing child plan for {:?}", child_component))?;
        let from = *plan
            .child_placements
            .get(child.0 as usize)
            .ok_or_else(|| format!("missing child placement {:?}", child))?;
        let width = child_plan.grid_size[0].max(1).min(plan.grid_size[0].max(1));
        let height = child_plan.grid_size[1].max(1).min(plan.grid_size[1].max(1));
        let max_x = plan.grid_size[0].saturating_sub(width) as i32;
        let max_y = plan.grid_size[1].saturating_sub(height) as i32;
        let to = ChildPlacement::at([
            (from.min[0] as i32 + delta[0]).clamp(0, max_x) as u32,
            (from.min[1] as i32 + delta[1]).clamp(0, max_y) as u32,
        ]);
        if to == from {
            return Ok(false);
        }
        self.apply_action(EditAction::MoveChild {
            component,
            child,
            from,
            to,
        })?;
        Ok(true)
    }

    pub fn undo(&mut self) -> Result<bool, String> {
        let mut history = std::mem::take(&mut self.history);
        let result = history.undo(self);
        self.history = history;
        result
    }

    pub fn redo(&mut self) -> Result<bool, String> {
        let mut history = std::mem::take(&mut self.history);
        let result = history.redo(self);
        self.history = history;
        result
    }

    pub fn build_runtime_root_and_plans(&self) -> Result<(Component, ComponentPlans), String> {
        let mut sorted_plans: Vec<_> = self
            .plans
            .iter()
            .map(|(id, plan)| (*id, plan.clone()))
            .collect();
        sorted_plans.sort_by_key(|(id, _)| id.0);
        let mut remap = HashMap::default();
        let mut plans = ComponentPlans::new();
        for (old_id, plan) in sorted_plans {
            remap.insert(old_id, plans.insert(plan));
        }
        let root = build_runtime_component(self, self.root, &remap)?;
        Ok((root, plans))
    }

    fn build_edit_scene_with_stack(
        &self,
        parent_stack: &[ComponentDefId],
        focused: ComponentDefId,
        preview_depth: usize,
    ) -> Result<FocusedScene, String> {
        let component = self
            .component(focused)
            .ok_or_else(|| format!("missing component {:?}", focused))?;
        let plan = self
            .plan(component.plan)
            .ok_or_else(|| format!("missing plan {:?}", component.plan))?;
        let grid_dims = plan.grid_size;
        let grid_size = Vec2::new(grid_dims[0] as f32 * CELL, grid_dims[1] as f32 * CELL);
        let grid_rect = Rect::from_min_size(Pos2::new(PAD, PAD + 36.0), grid_size);

        let input_ports: Vec<_> = plan
            .inputs
            .iter()
            .map(|port| PlacedPort {
                id: port.id,
                source_gate: (component_node_id(focused), port.gate),
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
                source_gate: (component_node_id(focused), port.gate),
                anchor: grid_anchor_for_port(grid_rect, grid_dims, port.location),
                location: port.location,
                label: port.label.unwrap_or_default().to_owned(),
            })
            .collect();

        let child_defs = component.children.clone();
        let ctx = VisualCtx {
            parent_stack,
            child_defs: &child_defs,
        };

        let gates = plan
            .gates
            .iter()
            .copied()
            .enumerate()
            .map(|(index, gate)| {
                let index = index as u32;
                let gx = index % grid_dims[0].max(1);
                let gy = index / grid_dims[0].max(1);
                let min = grid_rect.min + Vec2::new(gx as f32 * CELL, gy as f32 * CELL);
                let input_sources = gate.input_refs().map(|source| {
                    source.and_then(|signal| source_gate_of_ref(focused, signal, &ctx, self).ok())
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
            let mut next_parent_stack = Vec::with_capacity(parent_stack.len() + 1);
            next_parent_stack.extend_from_slice(parent_stack);
            next_parent_stack.push(focused);
            component
                .children
                .iter()
                .enumerate()
                .map(|(child_i, child_component)| {
                    let child_id = ChildId(child_i as u32);
                    let child_component_data = self
                        .component(*child_component)
                        .ok_or_else(|| format!("missing child component {:?}", child_component))?;
                    let child_plan = self.plan(child_component_data.plan).ok_or_else(|| {
                        format!("missing child plan {:?}", child_component_data.plan)
                    })?;
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
                            source_gate: (component_node_id(*child_component), port.gate),
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
                            source_gate: (component_node_id(*child_component), port.gate),
                            anchor: child_port_anchor(rect, child_plan.grid_size, port.location),
                            location: port.location,
                            label: port.label.unwrap_or_default().to_owned(),
                        })
                        .collect();
                    let scene = self.build_edit_scene_with_stack(
                        &next_parent_stack,
                        *child_component,
                        preview_depth.saturating_sub(1),
                    )?;
                    Ok(PlacedChild {
                        id: child_id,
                        node: component_node_id(*child_component),
                        rect,
                        inputs,
                        outputs,
                        scene: Box::new(scene),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        };

        let ancestor_ports = parent_stack
            .last()
            .and_then(|ancestor| {
                self.component(*ancestor)
                    .map(|component| (*ancestor, component))
            })
            .and_then(|(ancestor, component)| {
                self.plan(component.plan).map(|plan| (ancestor, plan))
            })
            .map(|(ancestor, parent_plan)| {
                parent_plan
                    .outputs
                    .iter()
                    .enumerate()
                    .map(|(i, port)| ExternalPort {
                        child: None,
                        node: Some(component_node_id(ancestor)),
                        port: port.id,
                        source_gate: (component_node_id(ancestor), port.gate),
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
                let src_endpoint = resolve_endpoint(focused, source_ref, &ctx, self)?;
                let src_gate = source_gate_of_ref(focused, source_ref, &ctx, self).ok();
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

        for connection in &component.child_input_connections {
            let src_endpoint = resolve_endpoint(focused, connection.src, &ctx, self)?;
            let src_gate = source_gate_of_ref(focused, connection.src, &ctx, self).ok();
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
                    source_gate: Some((component_node_id(focused), output.gate)),
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
            node: component_node_id(focused),
            title: format!("Component Def {}", focused.0),
            bounds,
            words_per_buffer: 0,
            gate_store: Arc::new(HashMap::default()),
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
}

fn build_runtime_component(
    doc: &EditorDocument,
    component_id: ComponentDefId,
    remap: &HashMap<PlanId, PlanId>,
) -> Result<Component, String> {
    let component = doc
        .component(component_id)
        .ok_or_else(|| format!("missing component {:?}", component_id))?;
    let children = component
        .children
        .iter()
        .copied()
        .map(|child| build_runtime_component(doc, child, remap))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Component::with_child_input_connections(
        *remap
            .get(&component.plan)
            .ok_or_else(|| format!("missing remapped plan {:?}", component.plan))?,
        children,
        component.child_input_connections.clone(),
    ))
}

fn component_node_id(component: ComponentDefId) -> NodeId {
    NodeId(component.0 as u32)
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

struct VisualCtx<'a> {
    parent_stack: &'a [ComponentDefId],
    child_defs: &'a [ComponentDefId],
}

fn resolve_endpoint(
    component_id: ComponentDefId,
    signal: SignalRef,
    ctx: &VisualCtx<'_>,
    doc: &EditorDocument,
) -> Result<Endpoint, String> {
    match signal {
        SignalRef::ThisGate(gate) => Ok(Endpoint::GateOutput(gate)),
        SignalRef::InputPort(port) => Ok(Endpoint::ComponentInput(port)),
        SignalRef::ChildOutput { child, port } => Ok(Endpoint::ChildOutput(child, port)),
        SignalRef::AncestorOutput {
            depth: AncestorDepth(depth),
            port,
        } => {
            let depth = depth as usize;
            if depth == 0 || depth > ctx.parent_stack.len() {
                return Err(format!(
                    "ancestor output {:?} is invalid for component {:?}",
                    port, component_id
                ));
            }
            let ancestor = ctx.parent_stack[ctx.parent_stack.len() - depth];
            let ancestor_component = doc
                .component(ancestor)
                .ok_or_else(|| format!("missing ancestor component {:?}", ancestor))?;
            let ancestor_plan = doc
                .plan(ancestor_component.plan)
                .ok_or_else(|| format!("missing ancestor plan {:?}", ancestor_component.plan))?;
            ancestor_plan
                .outputs
                .iter()
                .find(|output| output.id == port)
                .ok_or_else(|| format!("missing ancestor output port {:?}", port))?;
            Ok(Endpoint::AncestorOutput(port))
        }
    }
}

fn source_gate_of_ref(
    component_id: ComponentDefId,
    signal: SignalRef,
    ctx: &VisualCtx<'_>,
    doc: &EditorDocument,
) -> Result<(NodeId, GateId), String> {
    let component = doc
        .component(component_id)
        .ok_or_else(|| format!("missing component {:?}", component_id))?;
    match signal {
        SignalRef::ThisGate(gate) => Ok((component_node_id(component_id), gate)),
        SignalRef::InputPort(port) => {
            let plan = doc
                .plan(component.plan)
                .ok_or_else(|| format!("missing plan {:?}", component.plan))?;
            let gate = plan
                .inputs
                .iter()
                .find(|input| input.id == port)
                .ok_or_else(|| format!("missing input port {:?}", port))?
                .gate;
            Ok((component_node_id(component_id), gate))
        }
        SignalRef::ChildOutput { child, port } => {
            let child_component = ctx
                .child_defs
                .get(child.0 as usize)
                .copied()
                .ok_or_else(|| format!("missing child {:?}", child))?;
            let child = doc
                .component(child_component)
                .ok_or_else(|| format!("missing child component {:?}", child_component))?;
            let plan = doc
                .plan(child.plan)
                .ok_or_else(|| format!("missing child plan {:?}", child.plan))?;
            let gate = plan
                .outputs
                .iter()
                .find(|output| output.id == port)
                .ok_or_else(|| format!("missing child output port {:?}", port))?
                .gate;
            Ok((component_node_id(child_component), gate))
        }
        SignalRef::AncestorOutput {
            depth: AncestorDepth(depth),
            port,
        } => {
            let depth = depth as usize;
            let ancestor_id = ctx
                .parent_stack
                .get(ctx.parent_stack.len().saturating_sub(depth))
                .copied()
                .ok_or_else(|| format!("missing ancestor for depth {}", depth))?;
            let ancestor = doc
                .component(ancestor_id)
                .ok_or_else(|| format!("missing ancestor component {:?}", ancestor_id))?;
            let plan = doc
                .plan(ancestor.plan)
                .ok_or_else(|| format!("missing ancestor plan {:?}", ancestor.plan))?;
            let gate = plan
                .outputs
                .iter()
                .find(|output| output.id == port)
                .ok_or_else(|| format!("missing ancestor output port {:?}", port))?
                .gate;
            Ok((component_node_id(ancestor_id), gate))
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
