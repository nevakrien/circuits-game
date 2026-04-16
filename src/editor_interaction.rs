use egui::{Pos2, Rect, Vec2};

use crate::{
    gate_plans::{ChildId, NodeId},
    ui_config::CELL,
    visual_ui::{screen_to_world, FocusedScene, PlacedChild, SceneViewportOutput, ViewportState},
};

const CHILD_FOOTPRINT_FILL: f32 = 0.88;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditSceneAction {
    FocusChild(NodeId),
    SelectChild(ChildId),
    MoveChild {
        child: ChildId,
        delta_cells: [i32; 2],
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EditInteractionState {
    drag: Option<ChildDragState>,
}

#[derive(Debug, Clone, Copy)]
struct ChildDragState {
    child: ChildId,
    pointer_offset: Vec2,
    moved: bool,
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
        if let Some(child) = child_at_pointer(scene, pointer_world) {
            let footprint = child_footprint_rect(child.rect);
            let pointer_world =
                pointer_world.expect("pointer world should exist while starting drag");
            state.drag = Some(ChildDragState {
                child: child.id,
                pointer_offset: pointer_world - footprint.min,
                moved: false,
            });
        }
    }

    if viewport_output.primary_dragged {
        if let (Some(pointer_world), Some(mut drag)) = (pointer_world, state.drag) {
            let Some(child) = scene.children.iter().find(|child| child.id == drag.child) else {
                state.drag = None;
                return None;
            };
            let current_footprint = child_footprint_rect(child.rect);
            let desired_min = pointer_world - drag.pointer_offset;
            let delta_world = desired_min - current_footprint.min;
            let delta_cells = [
                quantize_drag_axis(delta_world.x),
                quantize_drag_axis(delta_world.y),
            ];
            drag.moved |= delta_cells != [0, 0];
            state.drag = Some(drag);
            if delta_cells != [0, 0] {
                return Some(EditSceneAction::MoveChild {
                    child: drag.child,
                    delta_cells,
                });
            }
        }
    }

    let drag_moved = state.drag.is_some_and(|drag| drag.moved);
    if viewport_output.primary_drag_stopped {
        state.drag = None;
    }
    if drag_moved {
        return None;
    }

    let hovered_child = child_at_pointer(scene, pointer_world);
    if viewport_output.primary_double_clicked {
        return hovered_child.map(|child| EditSceneAction::FocusChild(child.node));
    }
    if viewport_output.primary_clicked {
        return hovered_child.map(|child| EditSceneAction::SelectChild(child.id));
    }
    None
}

pub fn child_at_pointer(scene: &FocusedScene, pointer_world: Option<Pos2>) -> Option<&PlacedChild> {
    let pointer_world = pointer_world?;
    scene
        .children
        .iter()
        .find(|child| child_footprint_rect(child.rect).contains(pointer_world))
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
    fn drag_tracks_non_origin_child_across_scene_rebuilds() {
        let mut document = stress_drag_document();
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
        assert_eq!(
            first_move,
            Some(EditSceneAction::MoveChild {
                child: target_child,
                delta_cells: [-1, 0],
            })
        );
        document
            .move_child_by(focused, target_child, [-1, 0])
            .expect("move should succeed");

        let rebuilt_scene = document
            .build_edit_scene(focused, &viewport, available, None)
            .expect("rebuilt scene should build");
        let second_move = interact_edit_scene(
            &rebuilt_scene,
            &viewport,
            &drag_output(start + Vec2::new(-CELL * 2.2, 0.0), DragPhase::Dragging),
            &mut interaction,
        );
        assert_eq!(
            second_move,
            Some(EditSceneAction::MoveChild {
                child: target_child,
                delta_cells: [-1, 0],
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

    #[derive(Clone, Copy)]
    enum DragPhase {
        Started,
        Dragging,
    }

    fn drag_output(pointer_world: Pos2, phase: DragPhase) -> SceneViewportOutput {
        SceneViewportOutput {
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(8000.0, 8000.0)),
            pointer_screen: Some(pointer_world),
            primary_clicked: false,
            primary_double_clicked: false,
            primary_drag_started: matches!(phase, DragPhase::Started),
            primary_dragged: matches!(phase, DragPhase::Dragging),
            primary_drag_stopped: false,
            hover_world: Some(pointer_world),
            viewport_changed: false,
        }
    }
}
