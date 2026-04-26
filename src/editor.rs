use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Vec2};
use foldhash::{HashMap, HashSet};

use crate::gate_plans::{
    AncestorDepth, ChildId, ChildInputConnection, ChildPlacement, Component, ComponentLayout,
    ComponentPlan, ComponentPlans, Gate, GateId, NodeId, PlanId, PortId, PortLocation, SignalRef,
    WireEndpoint, WireLayout, WirePoint,
};
use crate::ui_config::{CELL, CHILD_PORT_INSET, PAD};
use crate::visual_ui::{
    ExternalPort, FocusedScene, PlacedChild, PlacedGate, PlacedPort, VisualWire,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentDefId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableComponentDef {
    pub plan: PlanId,
    pub children: Vec<ComponentDefId>,
    pub child_input_connections: Vec<ChildInputConnection>,
    pub dangling_wires: Vec<DanglingWire>,
    pub layout: ComponentLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DanglingWire {
    pub src: SignalRef,
    pub target: WirePoint,
}

#[derive(Debug, Clone, Default)]
pub struct EditHistory {
    actions: Vec<EditorCommand>,
    cursor: usize,
}

#[derive(Debug, Clone)]
pub struct MoveChildCommand {
    pub component: ComponentDefId,
    pub child: ChildId,
    pub from: ChildPlacement,
    pub to: ChildPlacement,
    before_component: EditableComponentDef,
    after_component: EditableComponentDef,
}

#[derive(Debug, Clone)]
pub struct PlanEditCommand {
    changes: Vec<PlanStateChange>,
}

#[derive(Debug, Clone)]
pub struct ComponentEditCommand {
    changes: Vec<ComponentStateChange>,
}

#[derive(Debug, Clone)]
struct PlanStateChange {
    plan: PlanId,
    before: ComponentPlan,
    after: ComponentPlan,
}

#[derive(Debug, Clone)]
struct ComponentStateChange {
    component: ComponentDefId,
    before: EditableComponentDef,
    after: EditableComponentDef,
}

#[derive(Debug, Clone)]
pub struct BatchCommand {
    pub commands: Vec<EditorCommand>,
}

#[derive(Debug, Clone)]
pub enum EditorCommand {
    MoveChild(MoveChildCommand),
    EditPlan(PlanEditCommand),
    EditComponent(ComponentEditCommand),
    Batch(BatchCommand),
}

#[derive(Debug, Clone)]
pub struct EditorDocument {
    pub plans: HashMap<PlanId, ComponentPlan>,
    pub components: Vec<EditableComponentDef>,
    pub root: ComponentDefId,
    history: EditHistory,
}

impl EditorCommand {
    pub fn inverse(&self) -> Self {
        match self {
            Self::MoveChild(command) => Self::MoveChild(MoveChildCommand {
                component: command.component,
                child: command.child,
                from: command.to,
                to: command.from,
                before_component: command.after_component.clone(),
                after_component: command.before_component.clone(),
            }),
            Self::EditPlan(command) => Self::EditPlan(PlanEditCommand {
                changes: command
                    .changes
                    .iter()
                    .map(|change| PlanStateChange {
                        plan: change.plan,
                        before: change.after.clone(),
                        after: change.before.clone(),
                    })
                    .collect(),
            }),
            Self::EditComponent(command) => Self::EditComponent(ComponentEditCommand {
                changes: command
                    .changes
                    .iter()
                    .map(|change| ComponentStateChange {
                        component: change.component,
                        before: change.after.clone(),
                        after: change.before.clone(),
                    })
                    .collect(),
            }),
            Self::Batch(command) => Self::Batch(BatchCommand {
                commands: command.commands.iter().rev().map(Self::inverse).collect(),
            }),
        }
    }

    fn apply(&self, doc: &mut EditorDocument) -> Result<(), String> {
        match self {
            Self::MoveChild(command) => {
                let component = doc
                    .components
                    .get_mut(command.component.0)
                    .ok_or_else(|| format!("missing component {:?}", command.component))?;
                *component = command.after_component.clone();
                Ok(())
            }
            Self::EditPlan(command) => {
                for change in &command.changes {
                    let plan = doc
                        .plans
                        .get_mut(&change.plan)
                        .ok_or_else(|| format!("missing plan {:?}", change.plan))?;
                    *plan = change.after.clone();
                }
                Ok(())
            }
            Self::EditComponent(command) => {
                for change in &command.changes {
                    let component = doc
                        .components
                        .get_mut(change.component.0)
                        .ok_or_else(|| format!("missing component {:?}", change.component))?;
                    *component = change.after.clone();
                }
                Ok(())
            }
            Self::Batch(command) => {
                for action in &command.commands {
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
        action: EditorCommand,
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
        let mut doc = Self {
            plans,
            components,
            root,
            history: EditHistory::default(),
        };
        doc.repair_visual_layouts()?;
        doc.validate()?;
        Ok(doc)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (component_id, component) in self.components.iter().enumerate() {
            self.plans.get(&component.plan).ok_or_else(|| {
                format!(
                    "component {} references missing plan {:?}",
                    component_id, component.plan
                )
            })?;
            if component.children.len() != component.layout.child_placements.len() {
                return Err(format!(
                    "component {} has {} children but layout has {} child placements",
                    component_id,
                    component.children.len(),
                    component.layout.child_placements.len()
                ));
            }
            self.validate_component_wires(ComponentDefId(component_id))?;
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
        viewport: &crate::visual_ui::ViewportState,
        available: Vec2,
        hover_world: Option<Pos2>,
    ) -> Result<FocusedScene, String> {
        if available.x <= 0.0 || available.y <= 0.0 {
            return self.build_edit_probe_scene(focused);
        }
        self.build_edit_scene_with_stack(
            &[],
            focused,
            Some(EditViewportHint::root(viewport, available, hover_world)),
            EditSceneDetail::Full,
        )
    }

    fn build_edit_probe_scene(&self, focused: ComponentDefId) -> Result<FocusedScene, String> {
        self.build_edit_scene_with_stack(&[], focused, None, EditSceneDetail::Probe)
    }

    pub fn refresh_edit_scene_drill_path(
        &self,
        focused: ComponentDefId,
        scene: &mut FocusedScene,
        viewport: &crate::visual_ui::ViewportState,
        available: Vec2,
        hover_world: Option<Pos2>,
    ) -> Result<bool, String> {
        if available.x <= 0.0 || available.y <= 0.0 {
            return Ok(false);
        }
        self.refresh_edit_scene_drill_path_with_stack(
            &[],
            focused,
            scene,
            EditViewportHint::root(viewport, available, hover_world),
        )
    }

    pub fn apply_command(&mut self, action: EditorCommand) -> Result<(), String> {
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
        let from = *editable
            .layout
            .child_placements
            .get(child.0 as usize)
            .ok_or_else(|| format!("missing child placement {:?}", child))?;
        let [width, height] = effective_child_grid_size(plan.grid_size, child_plan.grid_size);
        let max_x = plan.grid_size[0].saturating_sub(width) as i32;
        let max_y = plan.grid_size[1].saturating_sub(height) as i32;
        let to = ChildPlacement::at([
            (from.min[0] as i32 + delta[0]).clamp(0, max_x) as u32,
            (from.min[1] as i32 + delta[1]).clamp(0, max_y) as u32,
        ]);
        if to == from {
            return Ok(false);
        }
        let command = self.move_child_command(component, child, to)?;
        self.apply_command(EditorCommand::MoveChild(command))?;
        Ok(true)
    }

    pub fn move_gate_by(
        &mut self,
        component: ComponentDefId,
        gate: GateId,
        delta: [i32; 2],
    ) -> Result<bool, String> {
        let editable = self
            .component(component)
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let plan = self
            .plan(editable.plan)
            .ok_or_else(|| format!("missing plan {:?}", editable.plan))?;
        if !plan.gates.contains_key(&gate) {
            return Err(format!("missing gate {:?}", gate));
        }
        let width = plan.grid_size[0].max(1) as i32;
        let height = plan.grid_size[1].max(1) as i32;
        let from_x = gate.0 as i32 % width;
        let from_y = gate.0 as i32 / width;
        let target_x = (from_x + delta[0]).clamp(0, width.saturating_sub(1));
        let target_y = (from_y + delta[1]).clamp(0, height.saturating_sub(1));
        let target_index = (target_y.saturating_mul(width) + target_x) as u32;
        if target_index == gate.0 {
            return Ok(false);
        }
        let command = self.move_gate_command(editable.plan, gate, GateId(target_index))?;
        self.apply_command(EditorCommand::Batch(BatchCommand { commands: command }))?;
        Ok(true)
    }

    pub fn move_wire_bend_to(
        &mut self,
        component: ComponentDefId,
        from: WireEndpoint,
        to: WireEndpoint,
        bend_index: usize,
        point: WirePoint,
    ) -> Result<bool, String> {
        let before_component = self
            .component(component)
            .cloned()
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let mut after_component = before_component.clone();
        let Some(layout) = after_component
            .layout
            .wires
            .iter_mut()
            .find(|wire| wire.from == from && wire.to == to)
        else {
            return Ok(false);
        };
        if bend_index >= layout.bends.len() {
            return Ok(false);
        }
        if layout.bends[bend_index] == point {
            return Ok(false);
        }
        layout.bends[bend_index] = point;
        self.apply_component_edit(component, before_component, after_component)?;
        Ok(true)
    }

    pub fn insert_wire_bend(
        &mut self,
        component: ComponentDefId,
        from: WireEndpoint,
        to: WireEndpoint,
        bend_index: usize,
        point: WirePoint,
    ) -> Result<Option<usize>, String> {
        let before_component = self
            .component(component)
            .cloned()
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let mut after_component = before_component.clone();
        let Some(layout) = after_component
            .layout
            .wires
            .iter_mut()
            .find(|wire| wire.from == from && wire.to == to)
        else {
            return Ok(None);
        };
        let index = bend_index.min(layout.bends.len());
        layout.bends.insert(index, point);
        self.apply_component_edit(component, before_component, after_component)?;
        Ok(Some(index))
    }

    pub fn rewire_source_endpoint(
        &mut self,
        component: ComponentDefId,
        from: WireEndpoint,
        to: WireEndpoint,
        new_from: WireEndpoint,
    ) -> Result<Option<(WireEndpoint, WireEndpoint)>, String> {
        if from == new_from {
            return Ok(None);
        }
        let Some(signal) = signal_ref_from_wire_endpoint(new_from) else {
            return Ok(None);
        };
        let (plan_id, mut after_plan) = self.plan_for_component(component)?;
        let before_plan = after_plan.clone();
        let before_component = self
            .component(component)
            .cloned()
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let mut after_component = before_component.clone();
        if assign_target_source(&mut after_plan, &mut after_component, to, signal).is_err() {
            return Ok(None);
        }
        if !move_wire_layout(&mut after_component, from, to, new_from, to) {
            return Ok(None);
        }
        self.apply_plan_component_edits(
            vec![(plan_id, before_plan, after_plan)],
            vec![(component, before_component, after_component)],
        )?;
        Ok(Some((new_from, to)))
    }

    pub fn rewire_target_endpoint(
        &mut self,
        component: ComponentDefId,
        from: WireEndpoint,
        to: WireEndpoint,
        new_to: WireEndpoint,
    ) -> Result<Option<(WireEndpoint, WireEndpoint)>, String> {
        if to == new_to {
            return Ok(None);
        }
        let (plan_id, mut after_plan) = self.plan_for_component(component)?;
        let before_plan = after_plan.clone();
        let before_component = self
            .component(component)
            .cloned()
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let mut after_component = before_component.clone();
        let Ok(old_signal) = target_signal_ref(&after_plan, &after_component, to) else {
            return Ok(None);
        };
        let displaced_signal = optional_target_signal_ref(&after_plan, &after_component, new_to)?;
        if assign_target_source(&mut after_plan, &mut after_component, new_to, old_signal).is_err()
        {
            return Ok(None);
        }
        let new_from = signal_to_wire_endpoint(old_signal);
        if let Some(displaced_signal) = displaced_signal {
            if assign_target_source(&mut after_plan, &mut after_component, to, displaced_signal)
                .is_err()
            {
                return Ok(None);
            }
            let displaced_from = signal_to_wire_endpoint(displaced_signal);
            if !swap_wire_layout_targets(
                &mut after_component,
                from,
                to,
                displaced_from,
                new_to,
                new_from,
            ) {
                return Ok(None);
            }
        } else {
            if clear_target_source(&mut after_plan, &mut after_component, to).is_err() {
                return Ok(None);
            }
            if !move_wire_layout(&mut after_component, from, to, from, new_to) {
                return Ok(None);
            }
        }
        self.apply_plan_component_edits(
            vec![(plan_id, before_plan, after_plan)],
            vec![(component, before_component, after_component)],
        )?;
        Ok(Some((new_from, new_to)))
    }

    fn move_child_command(
        &self,
        component: ComponentDefId,
        child: ChildId,
        to: ChildPlacement,
    ) -> Result<MoveChildCommand, String> {
        let before_component = self
            .component(component)
            .cloned()
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let from = *before_component
            .layout
            .child_placements
            .get(child.0 as usize)
            .ok_or_else(|| format!("missing child placement {:?}", child))?;
        let mut after_component = before_component.clone();
        let detached_wires = self.detached_child_dangling_wires(component, child, from)?;
        let slot = after_component
            .layout
            .child_placements
            .get_mut(child.0 as usize)
            .ok_or_else(|| format!("missing child placement {:?}", child))?;
        *slot = to;
        after_component
            .child_input_connections
            .retain(|connection| connection.child != child);
        after_component.dangling_wires.extend(detached_wires);
        let (reattached, retained_dangling) =
            self.auto_attach_child_inputs(component, child, to, &after_component.dangling_wires)?;
        after_component.dangling_wires = retained_dangling;
        after_component.child_input_connections.extend(reattached);
        after_component.layout.wires =
            self.refreshed_component_wires(component, &after_component)?;
        Ok(MoveChildCommand {
            component,
            child,
            from,
            to,
            before_component,
            after_component,
        })
    }

    fn move_gate_command(
        &self,
        plan_id: PlanId,
        from: GateId,
        to: GateId,
    ) -> Result<Vec<EditorCommand>, String> {
        let before_plan = self
            .plan(plan_id)
            .cloned()
            .ok_or_else(|| format!("missing plan {:?}", plan_id))?;
        let width = before_plan.grid_size[0].max(1);
        let height = before_plan.grid_size[1].max(1);
        let max_gate_id = width.saturating_mul(height);
        if from.0 >= max_gate_id || to.0 >= max_gate_id {
            return Err(format!("gate move {:?} -> {:?} is out of range", from, to));
        }
        if !before_plan.gates.contains_key(&from) {
            return Err(format!("missing gate {:?}", from));
        }
        let mut after_plan = before_plan.clone();
        let moved_gate = after_plan
            .gates
            .remove(&from)
            .ok_or_else(|| format!("missing gate {:?}", from))?;
        let displaced_gate = after_plan.gates.remove(&to);
        after_plan.gates.insert(to, moved_gate);
        if let Some(displaced_gate) = displaced_gate {
            after_plan.gates.insert(from, displaced_gate);
        }
        Ok(vec![EditorCommand::EditPlan(PlanEditCommand {
            changes: vec![PlanStateChange {
                plan: plan_id,
                before: before_plan,
                after: after_plan,
            }],
        })])
    }

    fn plan_for_component(
        &self,
        component: ComponentDefId,
    ) -> Result<(PlanId, ComponentPlan), String> {
        let editable = self
            .component(component)
            .ok_or_else(|| format!("missing component {:?}", component))?;
        let plan = self
            .plan(editable.plan)
            .cloned()
            .ok_or_else(|| format!("missing plan {:?}", editable.plan))?;
        Ok((editable.plan, plan))
    }

    fn apply_component_edit(
        &mut self,
        component: ComponentDefId,
        before: EditableComponentDef,
        after: EditableComponentDef,
    ) -> Result<(), String> {
        if before == after {
            return Ok(());
        }
        self.apply_command(EditorCommand::EditComponent(ComponentEditCommand {
            changes: vec![ComponentStateChange {
                component,
                before,
                after,
            }],
        }))
    }

    fn apply_plan_component_edits(
        &mut self,
        plan_changes: Vec<(PlanId, ComponentPlan, ComponentPlan)>,
        component_changes: Vec<(ComponentDefId, EditableComponentDef, EditableComponentDef)>,
    ) -> Result<(), String> {
        let mut commands = Vec::new();
        if !plan_changes.is_empty() {
            commands.push(EditorCommand::EditPlan(PlanEditCommand {
                changes: plan_changes
                    .into_iter()
                    .map(|(plan, before, after)| PlanStateChange {
                        plan,
                        before,
                        after,
                    })
                    .collect(),
            }));
        }
        let component_changes = component_changes
            .into_iter()
            .filter(|(_, before, after)| before != after)
            .map(|(component, before, after)| ComponentStateChange {
                component,
                before,
                after,
            })
            .collect::<Vec<_>>();
        if !component_changes.is_empty() {
            commands.push(EditorCommand::EditComponent(ComponentEditCommand {
                changes: component_changes,
            }));
        }
        if commands.is_empty() {
            return Ok(());
        }
        self.apply_command(EditorCommand::Batch(BatchCommand { commands }))
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
        viewport_hint: Option<EditViewportHint>,
        detail: EditSceneDetail,
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
        let bounds = Rect::from_min_max(
            Pos2::ZERO,
            Pos2::new(grid_rect.right() + PAD, grid_rect.bottom() + PAD),
        );

        if matches!(detail, EditSceneDetail::Placeholder) {
            return Ok(placeholder_edit_scene(
                focused, bounds, grid_rect, grid_dims,
            ));
        }

        let input_ports: Vec<_> = plan
            .inputs
            .iter()
            .map(|port| PlacedPort {
                id: port.id,
                source_gate: (component_node_id(focused), port.gate),
                anchor: grid_anchor_for_port(grid_rect, grid_dims, port.location),
                location: port.location,
                label: port.label.clone().unwrap_or_default(),
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
                label: port.label.clone().unwrap_or_default(),
            })
            .collect();

        let child_defs = component.children.clone();
        let ctx = VisualCtx {
            parent_stack,
            child_defs: &child_defs,
        };

        let gates = plan
            .ordered_gates()
            .into_iter()
            .map(|(gate_id, gate)| {
                let gx = gate_id.0 % grid_dims[0].max(1);
                let gy = gate_id.0 / grid_dims[0].max(1);
                let min = grid_rect.min + Vec2::new(gx as f32 * CELL, gy as f32 * CELL);
                let input_sources = gate.input_refs().map(|source| {
                    source.and_then(|signal| source_gate_of_ref(focused, signal, &ctx, self).ok())
                });
                PlacedGate {
                    id: gate_id,
                    gate,
                    input_sources,
                    rect: Rect::from_min_size(min, Vec2::splat(CELL)),
                }
            })
            .collect::<Vec<_>>();

        let focused_child_index = viewport_hint.and_then(|hint| {
            select_preview_child_index(
                component,
                self,
                grid_rect,
                grid_dims,
                hint.visible_world_rect,
                hint.hover_world,
            )
        });

        let child_detail = match detail {
            EditSceneDetail::Full => ChildSceneDetail::SelectedPathOnly,
            EditSceneDetail::Probe => ChildSceneDetail::PlaceholderChildren,
            EditSceneDetail::Placeholder => ChildSceneDetail::PlaceholderOnly,
        };

        let children = if component.children.is_empty() {
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
                    let placement = component
                        .layout
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
                            label: port.label.clone().unwrap_or_default(),
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
                            label: port.label.clone().unwrap_or_default(),
                        })
                        .collect();
                    let (next_detail, next_viewport_hint) = match child_detail {
                        ChildSceneDetail::SelectedPathOnly
                            if focused_child_index == Some(child_i) =>
                        {
                            (
                                EditSceneDetail::Full,
                                viewport_hint
                                    .map(|hint| hint.for_child(child_plan.grid_size, rect)),
                            )
                        }
                        ChildSceneDetail::SelectedPathOnly => (EditSceneDetail::Placeholder, None),
                        ChildSceneDetail::PlaceholderChildren => {
                            (EditSceneDetail::Placeholder, None)
                        }
                        ChildSceneDetail::PlaceholderOnly => (EditSceneDetail::Placeholder, None),
                    };
                    let scene = self.build_edit_scene_with_stack(
                        &next_parent_stack,
                        *child_component,
                        next_viewport_hint,
                        next_detail,
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
                        label: port.label.clone().unwrap_or_default(),
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

        let wires = build_component_wires(component, focused, &ctx, self, &lookup)?;

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
            drill_child: focused_child_index.map(|index| ChildId(index as u32)),
            ancestor_ports,
            wires,
        })
    }

    fn refresh_edit_scene_drill_path_with_stack(
        &self,
        parent_stack: &[ComponentDefId],
        focused: ComponentDefId,
        scene: &mut FocusedScene,
        viewport_hint: EditViewportHint,
    ) -> Result<bool, String> {
        let component = self
            .component(focused)
            .ok_or_else(|| format!("missing component {:?}", focused))?;
        let desired_child_index = select_preview_child_index(
            component,
            self,
            scene.grid_rect,
            scene.grid_dims,
            viewport_hint.visible_world_rect,
            viewport_hint.hover_world,
        );
        let desired_drill_child = desired_child_index.map(|index| ChildId(index as u32));
        let mut changed = scene.drill_child != desired_drill_child;
        scene.drill_child = desired_drill_child;

        let Some(child_index) = desired_child_index else {
            return Ok(changed);
        };
        let Some(child_component) = component.children.get(child_index).copied() else {
            return Ok(changed);
        };
        let child_component_data = self
            .component(child_component)
            .ok_or_else(|| format!("missing child component {:?}", child_component))?;
        let child_plan = self
            .plan(child_component_data.plan)
            .ok_or_else(|| format!("missing child plan {:?}", child_component_data.plan))?;
        let Some(child) = scene.children.get_mut(child_index) else {
            return Ok(changed);
        };
        let child_hint = viewport_hint.for_child(child_plan.grid_size, child.rect);

        if is_placeholder_scene(&child.scene) {
            let mut next_parent_stack = Vec::with_capacity(parent_stack.len() + 1);
            next_parent_stack.extend_from_slice(parent_stack);
            next_parent_stack.push(focused);
            child.scene = Box::new(self.build_edit_scene_with_stack(
                &next_parent_stack,
                child_component,
                Some(child_hint),
                EditSceneDetail::Full,
            )?);
            changed = true;
        } else {
            let mut next_parent_stack = Vec::with_capacity(parent_stack.len() + 1);
            next_parent_stack.extend_from_slice(parent_stack);
            next_parent_stack.push(focused);
            changed |= self.refresh_edit_scene_drill_path_with_stack(
                &next_parent_stack,
                child_component,
                &mut child.scene,
                child_hint,
            )?;
        }

        Ok(changed)
    }

    fn refreshed_component_wires(
        &self,
        component_id: ComponentDefId,
        next_component: &EditableComponentDef,
    ) -> Result<Vec<WireLayout>, String> {
        let mut temp = self.clone();
        let previous_component = temp
            .components
            .get_mut(component_id.0)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        *previous_component = next_component.clone();
        let ctx = VisualCtx {
            parent_stack: &[],
            child_defs: &next_component.children,
        };
        let expected = temp.expected_component_wires(component_id, &ctx)?;
        let preserved: HashMap<_, _> = next_component
            .layout
            .wires
            .iter()
            .cloned()
            .map(|wire| ((wire.from, wire.to), wire))
            .collect();
        Ok(expected
            .into_iter()
            .map(|wire| {
                preserved
                    .get(&(wire.layout.from, wire.layout.to))
                    .cloned()
                    .unwrap_or(wire.layout)
            })
            .collect())
    }

    fn auto_attach_child_inputs(
        &self,
        component_id: ComponentDefId,
        child_id: ChildId,
        placement: ChildPlacement,
        dangling_wires: &[DanglingWire],
    ) -> Result<(Vec<ChildInputConnection>, Vec<DanglingWire>), String> {
        let mut temp = self.clone();
        let component = temp
            .components
            .get_mut(component_id.0)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        let slot = component
            .layout
            .child_placements
            .get_mut(child_id.0 as usize)
            .ok_or_else(|| format!("missing child placement {:?}", child_id))?;
        *slot = placement;
        component
            .child_input_connections
            .retain(|connection| connection.child != child_id);
        component.dangling_wires = dangling_wires.to_vec();

        let scene = temp.build_edit_probe_scene(component_id)?;
        let target_child = scene
            .children
            .get(child_id.0 as usize)
            .ok_or_else(|| format!("missing child scene {:?}", child_id))?;
        let mut connections = Vec::new();
        let mut remaining_dangling = dangling_wires.to_vec();
        let mut used_wire_indices = HashSet::default();
        for port in &target_child.inputs {
            let best_match = remaining_dangling
                .iter()
                .enumerate()
                .filter_map(|(index, wire)| {
                    (!used_wire_indices.contains(&index)).then_some((
                        index,
                        wire.src,
                        port.anchor.distance(local_wire_point_to_pos(&wire.target)),
                    ))
                })
                .filter(|(_, src, _)| {
                    !matches!(
                        src,
                        SignalRef::ChildOutput {
                            child: source_child,
                            ..
                        } if *source_child == child_id
                    )
                })
                .filter(|(_, _, distance)| *distance <= CELL * 0.35)
                .min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some((wire_index, src, _)) = best_match {
                used_wire_indices.insert(wire_index);
                connections.push(ChildInputConnection {
                    child: child_id,
                    input: port.id,
                    src,
                });
            }
        }
        remaining_dangling = remaining_dangling
            .into_iter()
            .enumerate()
            .filter_map(|(index, wire)| (!used_wire_indices.contains(&index)).then_some(wire))
            .collect();
        connections.sort_by_key(|connection| connection.input.0);
        Ok((connections, remaining_dangling))
    }

    fn detached_child_dangling_wires(
        &self,
        component_id: ComponentDefId,
        child_id: ChildId,
        placement: ChildPlacement,
    ) -> Result<Vec<DanglingWire>, String> {
        let mut temp = self.clone();
        let component = temp
            .components
            .get_mut(component_id.0)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        let slot = component
            .layout
            .child_placements
            .get_mut(child_id.0 as usize)
            .ok_or_else(|| format!("missing child placement {:?}", child_id))?;
        *slot = placement;
        let scene = temp.build_edit_probe_scene(component_id)?;
        let target_child = scene
            .children
            .get(child_id.0 as usize)
            .ok_or_else(|| format!("missing child scene {:?}", child_id))?;
        let editable = self
            .component(component_id)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        Ok(editable
            .child_input_connections
            .iter()
            .filter(|connection| connection.child == child_id)
            .filter_map(|connection| {
                target_child
                    .inputs
                    .iter()
                    .find(|port| port.id == connection.input)
                    .map(|port| DanglingWire {
                        src: connection.src,
                        target: pos_to_wire_point(port.anchor),
                    })
            })
            .collect())
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
    Ok(Component::with_layout_and_child_input_connections(
        *remap
            .get(&component.plan)
            .ok_or_else(|| format!("missing remapped plan {:?}", component.plan))?,
        children,
        component.child_input_connections.clone(),
        component.layout.clone(),
    ))
}

fn component_node_id(component: ComponentDefId) -> NodeId {
    NodeId(component.0 as u32)
}

fn placeholder_edit_scene(
    focused: ComponentDefId,
    bounds: Rect,
    grid_rect: Rect,
    grid_dims: [u32; 2],
) -> FocusedScene {
    FocusedScene {
        node: component_node_id(focused),
        title: format!("Component Def {}", focused.0),
        bounds,
        words_per_buffer: 0,
        gate_store: Arc::new(HashMap::default()),
        grid_rect,
        grid_dims,
        input_ports: Vec::new(),
        output_ports: Vec::new(),
        gates: Vec::new(),
        children: Vec::new(),
        drill_child: None,
        ancestor_ports: Vec::new(),
        wires: Vec::new(),
    }
}

fn is_placeholder_scene(scene: &FocusedScene) -> bool {
    scene.input_ports.is_empty()
        && scene.output_ports.is_empty()
        && scene.gates.is_empty()
        && scene.children.is_empty()
        && scene.ancestor_ports.is_empty()
        && scene.wires.is_empty()
}

#[derive(Debug, Clone, Copy)]
struct VisualCtx<'a> {
    parent_stack: &'a [ComponentDefId],
    child_defs: &'a [ComponentDefId],
}

#[derive(Debug, Clone, Copy)]
struct EditViewportHint {
    visible_world_rect: Rect,
    hover_world: Option<Pos2>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditSceneDetail {
    Full,
    Probe,
    Placeholder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildSceneDetail {
    SelectedPathOnly,
    PlaceholderChildren,
    PlaceholderOnly,
}

impl EditViewportHint {
    fn root(
        viewport: &crate::visual_ui::ViewportState,
        available: Vec2,
        hover_world: Option<Pos2>,
    ) -> Self {
        Self {
            visible_world_rect: visible_world_rect(viewport, available),
            hover_world,
        }
    }

    fn for_child(self, child_grid_size: [u32; 2], child_rect: Rect) -> Self {
        let source = child_scene_grid_rect(child_grid_size);
        let scale = (child_rect.width() / source.width().max(f32::EPSILON))
            .min(child_rect.height() / source.height().max(f32::EPSILON));
        let fitted_size = source.size() * scale;
        let target_min = child_rect.center() - fitted_size * 0.5;
        Self {
            visible_world_rect: inverse_transform_rect(
                self.visible_world_rect,
                source.min,
                target_min,
                scale,
            ),
            hover_world: self
                .hover_world
                .filter(|hover| child_rect.contains(*hover))
                .map(|hover| inverse_transform_pos(hover, source.min, target_min, scale)),
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
    fn anchor(&self, endpoint: WireEndpoint) -> Option<Pos2> {
        match endpoint {
            WireEndpoint::GateOutput(gate) => {
                let (rect, gate_kind) = self.gates.get(&gate)?;
                Some(gate_anchor(*rect, *gate_kind, None))
            }
            WireEndpoint::GateInput { gate, input } => {
                let (rect, gate_kind) = self.gates.get(&gate)?;
                Some(gate_anchor(*rect, *gate_kind, Some(input as usize)))
            }
            WireEndpoint::ComponentInput(port) => self.input_ports.get(&port).copied(),
            WireEndpoint::ComponentOutput(port) => self.output_ports.get(&port).copied(),
            WireEndpoint::ChildOutput { child, port } => {
                self.child_outputs.get(&(child, port)).copied()
            }
            WireEndpoint::ChildInput { child, port } => {
                self.child_inputs.get(&(child, port)).copied()
            }
            WireEndpoint::AncestorOutput { port, .. } => self.ancestor_outputs.get(&port).copied(),
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

    let [width, height] = effective_child_grid_size(grid_dims, child_dims);
    let max_x = grid_dims[0].saturating_sub(width);
    let max_y = grid_dims[1].saturating_sub(height);
    let min_x = placement.min[0].min(max_x);
    let min_y = placement.min[1].min(max_y);
    let min = grid_rect.min + Vec2::new(min_x as f32 * CELL, min_y as f32 * CELL);
    let footprint = Rect::from_min_size(min, Vec2::new(width as f32 * CELL, height as f32 * CELL));
    let scaled_size = footprint.size() * CHILD_FOOTPRINT_FILL;
    Rect::from_center_size(footprint.center(), scaled_size)
}

fn effective_child_grid_size(grid_dims: [u32; 2], child_dims: [u32; 2]) -> [u32; 2] {
    if child_dims[0] >= grid_dims[0] || child_dims[1] >= grid_dims[1] {
        [(grid_dims[0] / 2).max(1), (grid_dims[1] / 2).max(1)]
    } else {
        [
            child_dims[0].max(1).min(grid_dims[0].max(1)),
            child_dims[1].max(1).min(grid_dims[1].max(1)),
        ]
    }
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

pub(crate) const WIRE_POINT_UNITS_PER_CELL: f32 = 256.0;

#[derive(Debug, Clone)]
struct ExpectedWire {
    layout: WireLayout,
    source_gate: Option<(NodeId, GateId)>,
}

impl EditorDocument {
    fn repair_visual_layouts(&mut self) -> Result<(), String> {
        let component_ids: Vec<_> = (0..self.components.len()).map(ComponentDefId).collect();

        for component_id in component_ids.iter().copied() {
            let inferred = self.infer_missing_child_placements(component_id)?;
            let component = self
                .components
                .get_mut(component_id.0)
                .ok_or_else(|| format!("missing component {:?}", component_id))?;
            component.layout.child_placements.extend(inferred);
        }

        for component_id in component_ids {
            let default_wires = self
                .expected_component_wires(
                    component_id,
                    &VisualCtx {
                        parent_stack: &[],
                        child_defs: self
                            .component(component_id)
                            .ok_or_else(|| format!("missing component {:?}", component_id))?
                            .children
                            .as_slice(),
                    },
                )?
                .into_iter()
                .map(|wire| wire.layout)
                .collect::<Vec<_>>();
            let component = self
                .components
                .get_mut(component_id.0)
                .ok_or_else(|| format!("missing component {:?}", component_id))?;
            if component.layout.wires.is_empty() {
                component.layout.wires = default_wires;
            }
        }

        Ok(())
    }

    fn infer_missing_child_placements(
        &self,
        component_id: ComponentDefId,
    ) -> Result<Vec<ChildPlacement>, String> {
        let component = self
            .component(component_id)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        if component.layout.child_placements.len() > component.children.len() {
            return Err(format!(
                "component {:?} has {} child placements for {} children",
                component_id,
                component.layout.child_placements.len(),
                component.children.len()
            ));
        }
        let plan = self
            .plan(component.plan)
            .ok_or_else(|| format!("missing plan {:?}", component.plan))?;
        let mut used = component
            .layout
            .child_placements
            .iter()
            .enumerate()
            .map(|(index, placement)| {
                let child_component = component.children[index];
                let child_plan = self
                    .component(child_component)
                    .and_then(|child| self.plan(child.plan))
                    .ok_or_else(|| format!("missing child plan for {:?}", child_component))?;
                let dims = child_footprint_dims(plan.grid_size, child_plan.grid_size);
                validate_child_placement_bounds(
                    component_id,
                    ChildId(index as u32),
                    *placement,
                    dims,
                    plan.grid_size,
                )?;
                Ok((*placement, dims))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut inferred = Vec::new();
        for child_component in component
            .children
            .iter()
            .skip(component.layout.child_placements.len())
            .copied()
        {
            let child_plan = self
                .component(child_component)
                .and_then(|child| self.plan(child.plan))
                .ok_or_else(|| format!("missing child plan for {:?}", child_component))?;
            let dims = child_footprint_dims(plan.grid_size, child_plan.grid_size);
            let placement = infer_child_placement(plan.grid_size, dims, &used);
            used.push((placement, dims));
            inferred.push(placement);
        }
        Ok(inferred)
    }

    fn validate_component_wires(&self, component_id: ComponentDefId) -> Result<(), String> {
        let component = self
            .component(component_id)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        let ctx = VisualCtx {
            parent_stack: &[],
            child_defs: &component.children,
        };
        let expected = self.expected_component_wires(component_id, &ctx)?;
        let mut expected_pairs = HashSet::default();
        for wire in expected {
            expected_pairs.insert((wire.layout.from, wire.layout.to));
        }

        let mut seen = HashSet::default();
        for (index, wire) in component.layout.wires.iter().enumerate() {
            let key = (wire.from, wire.to);
            if !expected_pairs.contains(&key) {
                return Err(format!(
                    "component {:?} wire {} references unexpected endpoints {:?} -> {:?}",
                    component_id, index, wire.from, wire.to
                ));
            }
            if !seen.insert(key) {
                return Err(format!(
                    "component {:?} contains duplicate wire {:?} -> {:?}",
                    component_id, wire.from, wire.to
                ));
            }
        }
        Ok(())
    }

    fn expected_component_wires(
        &self,
        component_id: ComponentDefId,
        ctx: &VisualCtx<'_>,
    ) -> Result<Vec<ExpectedWire>, String> {
        let component = self
            .component(component_id)
            .ok_or_else(|| format!("missing component {:?}", component_id))?;
        let plan = self
            .plan(component.plan)
            .ok_or_else(|| format!("missing plan {:?}", component.plan))?;
        let mut wires = Vec::new();

        for (target_gate, gate) in plan.ordered_gates() {
            for (input_i, source_ref) in gate.input_refs().into_iter().flatten().enumerate() {
                wires.push(ExpectedWire {
                    layout: WireLayout {
                        from: signal_to_wire_endpoint(source_ref),
                        to: WireEndpoint::GateInput {
                            gate: target_gate,
                            input: input_i as u8,
                        },
                        bends: Vec::new(),
                    },
                    source_gate: source_gate_of_ref(component_id, source_ref, ctx, self).ok(),
                });
            }
        }

        for connection in &component.child_input_connections {
            wires.push(ExpectedWire {
                layout: WireLayout {
                    from: signal_to_wire_endpoint(connection.src),
                    to: WireEndpoint::ChildInput {
                        child: connection.child,
                        port: connection.input,
                    },
                    bends: Vec::new(),
                },
                source_gate: source_gate_of_ref(component_id, connection.src, ctx, self).ok(),
            });
        }

        for output in &plan.outputs {
            wires.push(ExpectedWire {
                layout: WireLayout {
                    from: WireEndpoint::GateOutput(output.gate),
                    to: WireEndpoint::ComponentOutput(output.id),
                    bends: Vec::new(),
                },
                source_gate: Some((component_node_id(component_id), output.gate)),
            });
        }

        Ok(wires)
    }
}

fn build_component_wires(
    component: &EditableComponentDef,
    component_id: ComponentDefId,
    ctx: &VisualCtx<'_>,
    doc: &EditorDocument,
    lookup: &Lookup,
) -> Result<Vec<VisualWire>, String> {
    let expected = doc.expected_component_wires(component_id, ctx)?;
    let mut overrides = HashMap::default();
    for wire in &component.layout.wires {
        overrides.entry((wire.from, wire.to)).or_insert(wire);
    }

    let mut wires = Vec::with_capacity(expected.len() + component.dangling_wires.len());
    for expected_wire in expected {
        let layout = overrides
            .get(&(expected_wire.layout.from, expected_wire.layout.to))
            .copied()
            .unwrap_or(&expected_wire.layout);
        if let Some(points) = wire_points(layout, lookup) {
            wires.push(VisualWire {
                source_gate: expected_wire.source_gate,
                color: palette_color(wires.len()),
                points,
                from: Some(layout.from),
                to: Some(layout.to),
                bends: layout.bends.clone(),
            });
        }
    }
    for dangling in &component.dangling_wires {
        if let Some(start) = lookup.anchor(signal_to_wire_endpoint(dangling.src)) {
            let end = local_wire_point_to_pos(&dangling.target);
            if let Some(points) = orth_wire_points(Some(start), Some(end)) {
                wires.push(VisualWire {
                    source_gate: source_gate_of_ref(component_id, dangling.src, ctx, doc).ok(),
                    color: palette_color(wires.len()),
                    points,
                    from: None,
                    to: None,
                    bends: Vec::new(),
                });
            }
        }
    }
    Ok(wires)
}

fn signal_to_wire_endpoint(signal: SignalRef) -> WireEndpoint {
    match signal {
        SignalRef::ThisGate(gate) => WireEndpoint::GateOutput(gate),
        SignalRef::InputPort(port) => WireEndpoint::ComponentInput(port),
        SignalRef::ChildOutput { child, port } => WireEndpoint::ChildOutput { child, port },
        SignalRef::AncestorOutput { depth, port } => WireEndpoint::AncestorOutput { depth, port },
    }
}

fn signal_ref_from_wire_endpoint(endpoint: WireEndpoint) -> Option<SignalRef> {
    match endpoint {
        WireEndpoint::GateOutput(gate) => Some(SignalRef::ThisGate(gate)),
        WireEndpoint::ComponentInput(port) => Some(SignalRef::InputPort(port)),
        WireEndpoint::ChildOutput { child, port } => Some(SignalRef::ChildOutput { child, port }),
        WireEndpoint::AncestorOutput { depth, port } => {
            Some(SignalRef::AncestorOutput { depth, port })
        }
        WireEndpoint::GateInput { .. }
        | WireEndpoint::ComponentOutput(_)
        | WireEndpoint::ChildInput { .. } => None,
    }
}

fn target_signal_ref(
    plan: &ComponentPlan,
    component: &EditableComponentDef,
    target: WireEndpoint,
) -> Result<SignalRef, String> {
    match target {
        WireEndpoint::GateInput { gate, input } => plan
            .gates
            .get(&gate)
            .and_then(|gate| gate.input_refs().get(input as usize).copied().flatten())
            .ok_or_else(|| format!("missing gate input {:?}:{input}", gate)),
        WireEndpoint::ChildInput { child, port } => component
            .child_input_connections
            .iter()
            .find(|connection| connection.child == child && connection.input == port)
            .map(|connection| connection.src)
            .ok_or_else(|| format!("missing child input {:?}:{:?}", child, port)),
        WireEndpoint::ComponentOutput(port) => plan
            .outputs
            .iter()
            .find(|output| output.id == port)
            .map(|output| SignalRef::ThisGate(output.gate))
            .ok_or_else(|| format!("missing component output {:?}", port)),
        _ => Err(format!("invalid wire target {:?}", target)),
    }
}

fn optional_target_signal_ref(
    plan: &ComponentPlan,
    component: &EditableComponentDef,
    target: WireEndpoint,
) -> Result<Option<SignalRef>, String> {
    match target {
        WireEndpoint::GateInput { gate, input } => plan
            .gates
            .get(&gate)
            .and_then(|gate| gate.input_refs().get(input as usize).copied().flatten())
            .ok_or_else(|| format!("missing gate input {:?}:{input}", gate))
            .map(Some),
        WireEndpoint::ChildInput { child, port } => Ok(component
            .child_input_connections
            .iter()
            .find(|connection| connection.child == child && connection.input == port)
            .map(|connection| connection.src)),
        WireEndpoint::ComponentOutput(port) => plan
            .outputs
            .iter()
            .find(|output| output.id == port)
            .map(|output| Some(SignalRef::ThisGate(output.gate)))
            .ok_or_else(|| format!("missing component output {:?}", port)),
        _ => Err(format!("invalid wire target {:?}", target)),
    }
}

fn assign_target_source(
    plan: &mut ComponentPlan,
    component: &mut EditableComponentDef,
    target: WireEndpoint,
    source: SignalRef,
) -> Result<(), String> {
    match target {
        WireEndpoint::GateInput { gate, input } => {
            let gate_ref = plan
                .gates
                .get_mut(&gate)
                .ok_or_else(|| format!("missing gate {:?}", gate))?;
            *gate_ref = set_gate_input_ref(*gate_ref, input as usize, source)?;
            Ok(())
        }
        WireEndpoint::ChildInput { child, port } => {
            let connection = component
                .child_input_connections
                .iter_mut()
                .find(|connection| connection.child == child && connection.input == port)
                .ok_or_else(|| format!("missing child input {:?}:{:?}", child, port))?;
            connection.src = source;
            Ok(())
        }
        WireEndpoint::ComponentOutput(port) => {
            let WireEndpoint::GateOutput(gate) = signal_to_wire_endpoint(source) else {
                return Err(format!("component output {:?} must come from a gate", port));
            };
            let output = plan
                .outputs
                .iter_mut()
                .find(|output| output.id == port)
                .ok_or_else(|| format!("missing component output {:?}", port))?;
            output.gate = gate;
            Ok(())
        }
        _ => Err(format!("invalid wire target {:?}", target)),
    }
}

fn clear_target_source(
    _plan: &mut ComponentPlan,
    component: &mut EditableComponentDef,
    target: WireEndpoint,
) -> Result<(), String> {
    match target {
        WireEndpoint::ChildInput { child, port } => {
            let before_len = component.child_input_connections.len();
            component
                .child_input_connections
                .retain(|connection| !(connection.child == child && connection.input == port));
            if component.child_input_connections.len() == before_len {
                return Err(format!("missing child input {:?}:{:?}", child, port));
            }
            Ok(())
        }
        WireEndpoint::GateInput { .. } | WireEndpoint::ComponentOutput(_) => {
            Err(format!("target {:?} cannot be disconnected", target))
        }
        _ => Err(format!("invalid wire target {:?}", target)),
    }
}

fn set_gate_input_ref(gate: Gate, input: usize, source: SignalRef) -> Result<Gate, String> {
    match (gate, input) {
        (Gate::BitNAND { b, .. }, 0) => Ok(Gate::BitNAND { a: source, b }),
        (Gate::BitNAND { a, .. }, 1) => Ok(Gate::BitNAND { a, b: source }),
        (Gate::BitAND { b, .. }, 0) => Ok(Gate::BitAND { a: source, b }),
        (Gate::BitAND { a, .. }, 1) => Ok(Gate::BitAND { a, b: source }),
        (Gate::BitOR { b, .. }, 0) => Ok(Gate::BitOR { a: source, b }),
        (Gate::BitOR { a, .. }, 1) => Ok(Gate::BitOR { a, b: source }),
        (Gate::BitNOR { b, .. }, 0) => Ok(Gate::BitNOR { a: source, b }),
        (Gate::BitNOR { a, .. }, 1) => Ok(Gate::BitNOR { a, b: source }),
        (Gate::BitXOR { b, .. }, 0) => Ok(Gate::BitXOR { a: source, b }),
        (Gate::BitXOR { a, .. }, 1) => Ok(Gate::BitXOR { a, b: source }),
        (Gate::BitXNOR { b, .. }, 0) => Ok(Gate::BitXNOR { a: source, b }),
        (Gate::BitXNOR { a, .. }, 1) => Ok(Gate::BitXNOR { a, b: source }),
        (Gate::BitNot { .. }, 0) => Ok(Gate::BitNot { src: source }),
        (Gate::BitNop { .. }, 0) => Ok(Gate::BitNop { src: source }),
        _ => Err(format!("gate input index {input} is out of range")),
    }
}

fn move_wire_layout(
    component: &mut EditableComponentDef,
    from: WireEndpoint,
    to: WireEndpoint,
    new_from: WireEndpoint,
    new_to: WireEndpoint,
) -> bool {
    let Some(layout) = component
        .layout
        .wires
        .iter_mut()
        .find(|wire| wire.from == from && wire.to == to)
    else {
        return false;
    };
    layout.from = new_from;
    layout.to = new_to;
    true
}

fn swap_wire_layout_targets(
    component: &mut EditableComponentDef,
    from: WireEndpoint,
    to: WireEndpoint,
    displaced_from: WireEndpoint,
    new_to: WireEndpoint,
    new_from: WireEndpoint,
) -> bool {
    let Some(first_index) = component
        .layout
        .wires
        .iter()
        .position(|wire| wire.from == from && wire.to == to)
    else {
        return false;
    };
    let Some(second_index) = component
        .layout
        .wires
        .iter()
        .position(|wire| wire.from == displaced_from && wire.to == new_to)
    else {
        return false;
    };
    if first_index == second_index {
        component.layout.wires[first_index].from = new_from;
        component.layout.wires[first_index].to = new_to;
        return true;
    }
    if first_index < second_index {
        let (left, right) = component.layout.wires.split_at_mut(second_index);
        let first = &mut left[first_index];
        let second = &mut right[0];
        first.from = new_from;
        first.to = new_to;
        second.from = displaced_from;
        second.to = to;
    } else {
        let (left, right) = component.layout.wires.split_at_mut(first_index);
        let second = &mut left[second_index];
        let first = &mut right[0];
        first.from = new_from;
        first.to = new_to;
        second.from = displaced_from;
        second.to = to;
    }
    true
}

fn infer_child_placement(
    parent_grid: [u32; 2],
    child_dims: [u32; 2],
    used: &[(ChildPlacement, [u32; 2])],
) -> ChildPlacement {
    let [width, height] = child_dims;
    let max_x = parent_grid[0].saturating_sub(width);
    let max_y = parent_grid[1].saturating_sub(height);
    for y in 0..=max_y {
        for x in 0..=max_x {
            let candidate = ChildPlacement::at([x, y]);
            if used.iter().all(|(placement, dims)| {
                !placements_overlap(candidate, [width, height], *placement, *dims)
            }) {
                return candidate;
            }
        }
    }
    ChildPlacement::ONE_CELL
}

fn child_footprint_dims(parent_grid: [u32; 2], child_grid: [u32; 2]) -> [u32; 2] {
    if child_grid[0] >= parent_grid[0] || child_grid[1] >= parent_grid[1] {
        [(parent_grid[0] / 2).max(1), (parent_grid[1] / 2).max(1)]
    } else {
        [
            child_grid[0].max(1).min(parent_grid[0].max(1)),
            child_grid[1].max(1).min(parent_grid[1].max(1)),
        ]
    }
}

fn placements_overlap(
    a: ChildPlacement,
    a_dims: [u32; 2],
    b: ChildPlacement,
    b_dims: [u32; 2],
) -> bool {
    let Some(a_max) = placement_extent(a, a_dims) else {
        return true;
    };
    let Some(b_max) = placement_extent(b, b_dims) else {
        return true;
    };
    a.min[0] < b_max[0] && b.min[0] < a_max[0] && a.min[1] < b_max[1] && b.min[1] < a_max[1]
}

fn validate_child_placement_bounds(
    component_id: ComponentDefId,
    child_id: ChildId,
    placement: ChildPlacement,
    dims: [u32; 2],
    parent_grid: [u32; 2],
) -> Result<(), String> {
    let Some(max) = placement_extent(placement, dims) else {
        return Err(format!(
            "component {:?} child {:?} placement {:?} overflows its footprint {:?}",
            component_id, child_id, placement.min, dims
        ));
    };
    if max[0] > parent_grid[0] || max[1] > parent_grid[1] {
        return Err(format!(
            "component {:?} child {:?} placement {:?} with footprint {:?} exceeds parent grid {:?}",
            component_id, child_id, placement.min, dims, parent_grid
        ));
    }
    Ok(())
}

fn placement_extent(placement: ChildPlacement, dims: [u32; 2]) -> Option<[u32; 2]> {
    Some([
        placement.min[0].checked_add(dims[0])?,
        placement.min[1].checked_add(dims[1])?,
    ])
}

fn wire_points(layout: &WireLayout, lookup: &Lookup) -> Option<Vec<Pos2>> {
    let start = lookup.anchor(layout.from)?;
    let end = lookup.anchor(layout.to)?;
    if layout.bends.is_empty() {
        return orth_wire_points(Some(start), Some(end));
    }

    let mut points = Vec::with_capacity(layout.bends.len() + 2);
    points.push(start);
    points.extend(layout.bends.iter().map(local_wire_point_to_pos));
    points.push(end);
    Some(points)
}

pub(crate) fn local_wire_point_to_pos(point: &WirePoint) -> Pos2 {
    Pos2::new(
        PAD + (point.x as f32 / WIRE_POINT_UNITS_PER_CELL) * CELL,
        PAD + 36.0 + (point.y as f32 / WIRE_POINT_UNITS_PER_CELL) * CELL,
    )
}

pub(crate) fn pos_to_wire_point(pos: Pos2) -> WirePoint {
    WirePoint {
        x: (((pos.x - PAD) / CELL) * WIRE_POINT_UNITS_PER_CELL).round() as i32,
        y: (((pos.y - PAD - 36.0) / CELL) * WIRE_POINT_UNITS_PER_CELL).round() as i32,
    }
}

fn visible_world_rect(viewport: &crate::visual_ui::ViewportState, available: Vec2) -> Rect {
    Rect::from_min_max(
        Pos2::new(
            (-viewport.pan.x) / viewport.zoom.max(f32::EPSILON),
            (-viewport.pan.y) / viewport.zoom.max(f32::EPSILON),
        ),
        Pos2::new(
            (available.x - viewport.pan.x) / viewport.zoom.max(f32::EPSILON),
            (available.y - viewport.pan.y) / viewport.zoom.max(f32::EPSILON),
        ),
    )
}

fn select_preview_child_index(
    component: &EditableComponentDef,
    doc: &EditorDocument,
    grid_rect: Rect,
    grid_dims: [u32; 2],
    visible_world_rect: Rect,
    hover_world: Option<Pos2>,
) -> Option<usize> {
    if let Some(hover_world) = hover_world {
        if let Some((child_i, _)) =
            component
                .children
                .iter()
                .enumerate()
                .find(|(child_i, child_component)| {
                    let Some(child_component_data) = doc.component(**child_component) else {
                        return false;
                    };
                    let Some(child_plan) = doc.plan(child_component_data.plan) else {
                        return false;
                    };
                    let placement = component
                        .layout
                        .child_placements
                        .get(*child_i)
                        .copied()
                        .unwrap_or(ChildPlacement::ONE_CELL);
                    child_rect_from_placement(grid_rect, grid_dims, child_plan.grid_size, placement)
                        .contains(hover_world)
                })
        {
            return Some(child_i);
        }
    }

    let mut best = None;
    let mut second_best = None;
    for (child_i, child_component) in component.children.iter().enumerate() {
        let Some(child_component_data) = doc.component(*child_component) else {
            continue;
        };
        let Some(child_plan) = doc.plan(child_component_data.plan) else {
            continue;
        };
        let placement = component
            .layout
            .child_placements
            .get(child_i)
            .copied()
            .unwrap_or(ChildPlacement::ONE_CELL);
        let rect = child_rect_from_placement(grid_rect, grid_dims, child_plan.grid_size, placement);
        let Some(coverage) = focused_child_visible_coverage(rect, visible_world_rect) else {
            continue;
        };
        match best {
            None => best = Some((coverage, child_i)),
            Some((best_coverage, best_child_i)) if coverage > best_coverage => {
                second_best = Some((best_coverage, best_child_i));
                best = Some((coverage, child_i));
            }
            _ => {
                second_best = Some(match second_best {
                    Some((second_coverage, second_child_i)) if second_coverage >= coverage => {
                        (second_coverage, second_child_i)
                    }
                    _ => (coverage, child_i),
                });
            }
        }
    }
    match (best, second_best) {
        (Some((best_coverage, child_i)), Some((next_coverage, _)))
            if best_coverage >= AUTO_DRILL_DOMINANT_VISIBLE_COVERAGE
                && best_coverage - next_coverage >= AUTO_DRILL_MIN_MARGIN =>
        {
            Some(child_i)
        }
        (Some((best_coverage, child_i)), None)
            if best_coverage >= AUTO_DRILL_DOMINANT_VISIBLE_COVERAGE =>
        {
            Some(child_i)
        }
        _ => None,
    }
}

fn focused_child_visible_coverage(rect: Rect, visible_world_rect: Rect) -> Option<f32> {
    let viewport_area = rect_area(visible_world_rect).max(f32::EPSILON);
    let viewport_center = visible_world_rect.center();
    child_focus_rect(rect)
        .contains(viewport_center)
        .then(|| rect_area(rect.intersect(visible_world_rect)) / viewport_area)
        .filter(|coverage| *coverage >= DRILL_IN_VISIBLE_COVERAGE_THRESHOLD)
}

fn child_scene_grid_rect(grid_dims: [u32; 2]) -> Rect {
    Rect::from_min_size(
        Pos2::new(PAD, PAD + 36.0),
        Vec2::new(grid_dims[0] as f32 * CELL, grid_dims[1] as f32 * CELL),
    )
}

fn inverse_transform_rect(rect: Rect, source_min: Pos2, target_min: Pos2, scale: f32) -> Rect {
    Rect::from_min_max(
        inverse_transform_pos(rect.min, source_min, target_min, scale),
        inverse_transform_pos(rect.max, source_min, target_min, scale),
    )
}

fn inverse_transform_pos(pos: Pos2, source_min: Pos2, target_min: Pos2, scale: f32) -> Pos2 {
    source_min + (pos - target_min) / scale.max(f32::EPSILON)
}

fn child_focus_rect(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * DRILL_IN_FOCUS_REGION)
}

fn rect_area(rect: Rect) -> f32 {
    rect.width().max(0.0) * rect.height().max(0.0)
}

const DRILL_IN_VISIBLE_COVERAGE_THRESHOLD: f32 = 0.20;
const DRILL_IN_FOCUS_REGION: f32 = 0.55;
const AUTO_DRILL_DOMINANT_VISIBLE_COVERAGE: f32 = 0.35;
const AUTO_DRILL_MIN_MARGIN: f32 = 0.05;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate_plans::ComponentPort;

    const INPUT_A: PortId = PortId(10);
    const INPUT_B: PortId = PortId(11);
    const OUTPUT_Y: PortId = PortId(20);
    const OUTPUT_Z: PortId = PortId(21);

    #[test]
    fn auto_attach_does_not_snap_to_the_childs_own_outputs() {
        let document = sample_document();
        let component = document.component(ComponentDefId(1)).unwrap();
        let placement = component.layout.child_placements[0];

        let inferred = document
            .auto_attach_child_inputs(
                ComponentDefId(1),
                ChildId(0),
                placement,
                &document
                    .component(ComponentDefId(1))
                    .unwrap()
                    .dangling_wires,
            )
            .expect("current placement should infer child attachments");

        assert!(inferred.0.iter().all(|connection| {
            !matches!(
                connection.src,
                SignalRef::ChildOutput {
                    child: ChildId(0),
                    ..
                }
            )
        }));
    }

    #[test]
    fn move_child_commands_round_trip_through_history() {
        let mut document = sample_document();
        let before = document.component(ComponentDefId(1)).unwrap().clone();

        assert!(
            document
                .move_child_by(ComponentDefId(1), ChildId(0), [-2, -2])
                .expect("move should succeed")
        );
        assert_eq!(document.history().applied_len(), 1);
        assert_eq!(document.history().redo_len(), 0);
        assert_ne!(
            document.component(ComponentDefId(1)).unwrap().layout,
            before.layout
        );

        assert!(document.undo().expect("undo should succeed"));
        assert_eq!(
            document.component(ComponentDefId(1)).unwrap().layout,
            before.layout
        );
        assert_eq!(
            document
                .component(ComponentDefId(1))
                .unwrap()
                .child_input_connections,
            before.child_input_connections
        );

        assert!(document.redo().expect("redo should succeed"));
        assert_eq!(document.history().applied_len(), 1);
        assert_eq!(document.history().redo_len(), 0);
    }

    #[test]
    fn move_child_detaches_into_dangling_wires_instead_of_deleting_connections() {
        let mut document = sample_document();

        assert!(
            document
                .move_child_by(ComponentDefId(1), ChildId(0), [-2, -2])
                .expect("move should succeed")
        );

        let component = document.component(ComponentDefId(1)).unwrap();
        assert!(component.child_input_connections.is_empty());
        assert_eq!(component.dangling_wires.len(), 2);
        assert_eq!(
            component
                .dangling_wires
                .iter()
                .map(|wire| wire.src)
                .collect::<Vec<_>>(),
            vec![
                SignalRef::ThisGate(GateId(0)),
                SignalRef::ThisGate(GateId(1)),
            ]
        );
    }

    #[test]
    fn move_child_back_onto_dangling_wires_reattaches_connections() {
        let mut document = sample_document();
        let original = document.component(ComponentDefId(1)).unwrap().clone();

        assert!(
            document
                .move_child_by(ComponentDefId(1), ChildId(0), [-2, -2])
                .expect("move away should succeed")
        );
        assert!(
            document
                .move_child_by(ComponentDefId(1), ChildId(0), [2, 2])
                .expect("move back should succeed")
        );

        let component = document.component(ComponentDefId(1)).unwrap();
        assert_eq!(
            component.child_input_connections,
            original.child_input_connections
        );
        assert!(component.dangling_wires.is_empty());
    }

    #[test]
    fn move_gate_moves_slot_contents_without_remapping_refs() {
        let mut document = sample_document();

        assert!(
            document
                .move_gate_by(ComponentDefId(1), GateId(0), [1, 0])
                .expect("move should succeed")
        );

        let plan = document.plan(PlanId(1)).unwrap();
        assert!(matches!(
            plan.gate(GateId(0)).unwrap(),
            Gate::BitNop {
                src: SignalRef::ThisGate(GateId(0))
            }
        ));
        assert!(matches!(
            plan.gate(GateId(1)).unwrap(),
            Gate::BitNot {
                src: SignalRef::ThisGate(GateId(0))
            }
        ));
        let component = document.component(ComponentDefId(1)).unwrap();
        assert_eq!(
            component
                .child_input_connections
                .iter()
                .map(|connection| connection.src)
                .collect::<Vec<_>>(),
            vec![
                SignalRef::ThisGate(GateId(0)),
                SignalRef::ThisGate(GateId(1))
            ]
        );
    }

    #[test]
    fn rewiring_source_endpoint_updates_gate_input_signal() {
        let mut document = sample_document();

        let rewired = document
            .rewire_source_endpoint(
                ComponentDefId(1),
                WireEndpoint::GateOutput(GateId(1)),
                WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 1,
                },
                WireEndpoint::GateOutput(GateId(0)),
            )
            .expect("rewire should succeed");

        assert_eq!(
            rewired,
            Some((
                WireEndpoint::GateOutput(GateId(0)),
                WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 1,
                },
            ))
        );
        let plan = document.plan(PlanId(1)).unwrap();
        assert!(matches!(
            plan.gate(GateId(2)).unwrap(),
            Gate::BitXOR {
                a: SignalRef::ThisGate(GateId(0)),
                b: SignalRef::ThisGate(GateId(0))
            }
        ));
    }

    #[test]
    fn rewiring_target_endpoint_swaps_gate_input_sources() {
        let mut document = sample_document();

        let rewired = document
            .rewire_target_endpoint(
                ComponentDefId(1),
                WireEndpoint::GateOutput(GateId(0)),
                WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 0,
                },
                WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 1,
                },
            )
            .expect("rewire should succeed");

        assert_eq!(
            rewired,
            Some((
                WireEndpoint::GateOutput(GateId(0)),
                WireEndpoint::GateInput {
                    gate: GateId(2),
                    input: 1,
                },
            ))
        );
        let plan = document.plan(PlanId(1)).unwrap();
        assert!(matches!(
            plan.gate(GateId(2)).unwrap(),
            Gate::BitXOR {
                a: SignalRef::ThisGate(GateId(1)),
                b: SignalRef::ThisGate(GateId(0))
            }
        ));
    }

    #[test]
    fn wire_bends_can_be_inserted_and_moved() {
        let mut document = sample_document();
        let point = WirePoint { x: 128, y: 256 };

        assert_eq!(
            document
                .insert_wire_bend(
                    ComponentDefId(1),
                    WireEndpoint::GateOutput(GateId(0)),
                    WireEndpoint::GateInput {
                        gate: GateId(2),
                        input: 0,
                    },
                    0,
                    point,
                )
                .expect("bend insert should succeed"),
            Some(0)
        );
        assert!(
            document
                .move_wire_bend_to(
                    ComponentDefId(1),
                    WireEndpoint::GateOutput(GateId(0)),
                    WireEndpoint::GateInput {
                        gate: GateId(2),
                        input: 0,
                    },
                    0,
                    WirePoint { x: 192, y: 320 },
                )
                .expect("bend move should succeed")
        );

        let component = document.component(ComponentDefId(1)).unwrap();
        let wire = component
            .layout
            .wires
            .iter()
            .find(|wire| {
                wire.from == WireEndpoint::GateOutput(GateId(0))
                    && wire.to
                        == (WireEndpoint::GateInput {
                            gate: GateId(2),
                            input: 0,
                        })
            })
            .expect("wire should exist");
        assert_eq!(wire.bends, vec![WirePoint { x: 192, y: 320 }]);
    }

    fn sample_document() -> EditorDocument {
        let child_plan = PlanId(0);
        let root_plan = PlanId(1);
        let mut plans = HashMap::default();
        plans.insert(
            child_plan,
            ComponentPlan::with_ports(
                vec![
                    Gate::BitNop {
                        src: SignalRef::InputPort(INPUT_A),
                    },
                    Gate::BitNop {
                        src: SignalRef::InputPort(INPUT_B),
                    },
                    Gate::BitXOR {
                        a: SignalRef::ThisGate(GateId(0)),
                        b: SignalRef::ThisGate(GateId(1)),
                    },
                    Gate::BitAND {
                        a: SignalRef::ThisGate(GateId(0)),
                        b: SignalRef::ThisGate(GateId(1)),
                    },
                    Gate::BitNot {
                        src: SignalRef::ThisGate(GateId(0)),
                    },
                ],
                vec![port(INPUT_A, 0, 0, 1, "A"), port(INPUT_B, 1, 0, 2, "B")],
                vec![
                    port(OUTPUT_Y, 2, u16::MAX, 1, "sum"),
                    port(OUTPUT_Z, 4, u16::MAX, 2, "carry"),
                ],
            )
            .with_grid_size([3, 2]),
        );
        plans.insert(
            root_plan,
            ComponentPlan::with_ports(
                vec![
                    Gate::BitNot {
                        src: SignalRef::ThisGate(GateId(0)),
                    },
                    Gate::BitNop {
                        src: SignalRef::ThisGate(GateId(0)),
                    },
                    Gate::BitXOR {
                        a: SignalRef::ThisGate(GateId(0)),
                        b: SignalRef::ThisGate(GateId(1)),
                    },
                    Gate::BitNop {
                        src: SignalRef::ChildOutput {
                            child: ChildId(0),
                            port: OUTPUT_Y,
                        },
                    },
                    Gate::BitNot {
                        src: SignalRef::ChildOutput {
                            child: ChildId(0),
                            port: OUTPUT_Y,
                        },
                    },
                    Gate::BitNop {
                        src: SignalRef::ChildOutput {
                            child: ChildId(0),
                            port: OUTPUT_Z,
                        },
                    },
                    Gate::BitOR {
                        a: SignalRef::ThisGate(GateId(2)),
                        b: SignalRef::ThisGate(GateId(5)),
                    },
                ],
                vec![],
                vec![
                    port(OUTPUT_Y, 3, u16::MAX, 1, "sum"),
                    port(OUTPUT_Z, 6, u16::MAX, 3, "carry"),
                ],
            )
            .with_grid_size([5, 5]),
        );

        EditorDocument::new(
            plans,
            vec![
                EditableComponentDef {
                    plan: child_plan,
                    children: Vec::new(),
                    child_input_connections: Vec::new(),
                    dangling_wires: Vec::new(),
                    layout: ComponentLayout::default(),
                },
                EditableComponentDef {
                    plan: root_plan,
                    children: vec![ComponentDefId(0)],
                    child_input_connections: vec![
                        ChildInputConnection {
                            child: ChildId(0),
                            input: INPUT_A,
                            src: SignalRef::ThisGate(GateId(0)),
                        },
                        ChildInputConnection {
                            child: ChildId(0),
                            input: INPUT_B,
                            src: SignalRef::ThisGate(GateId(1)),
                        },
                    ],
                    dangling_wires: Vec::new(),
                    layout: ComponentLayout::default()
                        .with_child_placements(vec![ChildPlacement::at([2, 2])]),
                },
            ],
            ComponentDefId(1),
        )
        .expect("sample document should build")
    }

    fn port(id: PortId, gate: u32, x: u16, y: u16, label: &str) -> ComponentPort {
        ComponentPort {
            id,
            gate: GateId(gate),
            location: PortLocation { x, y },
            label: Some(label.to_owned()),
        }
    }
}
