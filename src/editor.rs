use egui::{
    Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextureId, Vec2,
};

use egui_wgpu::wgpu;

use crate::{render, simulation, wire_render, wires};

const TAG_WIDTH: f32 = 54.0;
const TAG_HEIGHT: f32 = 24.0;
const RESET_BUTTON_WIDTH: f32 = 64.0;
const RESET_BUTTON_HEIGHT: f32 = 24.0;
const PANEL_WIDTH: f32 = 260.0;
const PANEL_HEIGHT: f32 = 420.0;
const PANEL_MARGIN: f32 = 12.0;
const PANEL_INNER_WIDTH: f32 = PANEL_WIDTH - 24.0;
const TOOL_CARD_HEIGHT: f32 = 78.0;
const WIRE_COLOR_MENU_WIDTH: f32 = 152.0;

const WIRE_COLOR_OPTIONS: [([f32; 4], &str); 6] = [
    (wires::DEFAULT_WIRE_COLOR, "Blue"),
    ([0.87, 0.32, 0.28, 1.0], "Red"),
    ([0.27, 0.74, 0.43, 1.0], "Green"),
    ([0.93, 0.76, 0.25, 1.0], "Gold"),
    ([0.64, 0.42, 0.88, 1.0], "Purple"),
    ([0.93, 0.48, 0.76, 1.0], "Pink"),
];
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorTool {
    Wire,
    Source,
    Noop,
    Not,
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
}

impl EditorTool {
    pub const ALL: [Self; 10] = [
        Self::Wire,
        Self::Source,
        Self::Noop,
        Self::Not,
        Self::And,
        Self::Or,
        Self::Xor,
        Self::Nand,
        Self::Nor,
        Self::Xnor,
    ];

    pub const COUNT: usize = Self::ALL.len();

    pub fn is_placeable(self) -> bool {
        self != Self::Wire
    }

    pub fn preview_index(self) -> usize {
        match self {
            Self::Wire => 0,
            Self::Source => 1,
            Self::Noop => 2,
            Self::Not => 3,
            Self::And => 4,
            Self::Or => 5,
            Self::Xor => 6,
            Self::Nand => 7,
            Self::Nor => 8,
            Self::Xnor => 9,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Wire => "Wire",
            Self::Source => "Source",
            Self::Noop => "NO-OP",
            Self::Not => "NOT",
            Self::And => "AND",
            Self::Or => "OR",
            Self::Xor => "XOR",
            Self::Nand => "NAND",
            Self::Nor => "NOR",
            Self::Xnor => "XNOR",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Wire => "Connect components",
            Self::Source => "Constant",
            Self::Noop => "Pass-through",
            Self::Not => "Not Gate",
            Self::And => "And Gate",
            Self::Or => "Or Gate",
            Self::Xor => "Xor Gate (not equal)",
            Self::Nand => "Nand Gate",
            Self::Nor => "Nor Gate",
            Self::Xnor => "Xnor Gate (equal)",
        }
    }
}

const TOOL_OPTIONS: &[EditorTool] = &EditorTool::ALL;

pub struct EditorUi {
    expanded: bool,
    selected_tool: EditorTool,
    selected_wire_color: [f32; 4],
    tool_preview_textures: [TextureId; EditorTool::COUNT],
}

pub struct EditorHistory<T> {
    actions: Vec<T>,
    next_index: usize,
}

#[derive(Clone)]
pub enum EditorAction {
    UpdateDraft {
        before: Option<wires::DraftWire>,
        after: Option<wires::DraftWire>,
    },
    CommitWire {
        edge: wire_render::StoredWireEdge,
        previous_draft: Option<wires::DraftWire>,
    },
    DeleteWire(wire_render::StoredWireEdge),
    PlaceCell {
        grid_cell: wires::GridCell,
        layer: u32,
        previous_cell: simulation::CellSnapshot,
        previous_charge_values: Vec<u8>,
        new_cell: simulation::CellSnapshot,
        new_charge_values: Vec<u8>,
    },
    DeleteCell {
        grid_cell: wires::GridCell,
        layer: u32,
        cell: simulation::CellSnapshot,
        charge_values: Vec<u8>,
    },
}

pub struct EditorSession {
    ui: EditorUi,
    history: EditorHistory<EditorAction>,
}

impl<T> Default for EditorHistory<T> {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            next_index: 0,
        }
    }
}

impl<T> EditorHistory<T> {
    pub fn can_undo(&self) -> bool {
        self.next_index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.next_index < self.actions.len()
    }

    pub fn push(&mut self, action: T) {
        self.actions.truncate(self.next_index);
        self.actions.push(action);
        self.next_index = self.actions.len();
    }

    pub fn undo_action(&mut self) -> Option<&T> {
        if self.next_index == 0 {
            return None;
        }

        self.next_index -= 1;
        self.actions.get(self.next_index)
    }

    pub fn redo_action(&mut self) -> Option<&T> {
        if self.next_index >= self.actions.len() {
            return None;
        }

        let action = self.actions.get(self.next_index);
        self.next_index += 1;
        action
    }

    pub fn take_applied_suffix_while<P>(&mut self, mut predicate: P) -> Vec<T>
    where
        P: FnMut(&T) -> bool,
    {
        let mut start = self.next_index;
        while start > 0 && predicate(&self.actions[start - 1]) {
            start -= 1;
        }

        if start == self.next_index {
            return Vec::new();
        }

        self.next_index = start;
        self.actions.drain(start..).collect()
    }
}

impl EditorSession {
    pub fn new(tool_preview_textures: [TextureId; EditorTool::COUNT]) -> Self {
        Self {
            ui: EditorUi::new(tool_preview_textures),
            history: EditorHistory::default(),
        }
    }

    pub fn selected_tool(&self) -> EditorTool {
        self.ui.selected_tool()
    }

    pub fn selected_wire_color(&self) -> [f32; 4] {
        self.ui.selected_wire_color()
    }

    pub fn show(&mut self, ctx: &egui::Context, displayed_layer: u32) -> bool {
        self.ui.show(ctx, displayed_layer, &self.history)
    }

    pub fn cancel_draft_if_inactive(
        &mut self,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if self.selected_tool() != EditorTool::Wire {
            wire_overlay.restore_draft(device, queue, None);
        }
    }

    pub fn undo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(action) = self.history.undo_action() else {
            return false;
        };
        apply_inverse_action(action, simulation, component, wire_overlay, device, queue);
        true
    }

    pub fn redo(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(action) = self.history.redo_action() else {
            return false;
        };
        apply_action(action, simulation, component, wire_overlay, device, queue);
        true
    }

    pub fn finish_wire_attempt(
        &mut self,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let Some(draft) = wire_overlay.current_draft() else {
            return false;
        };
        let Some(edge) = wire_overlay.commit_draft(device, queue, self.ui.selected_wire_color())
        else {
            wire_overlay.restore_draft(device, queue, None);
            return false;
        };

        let previous_draft = self
            .history
            .take_applied_suffix_while(|action| matches!(action, EditorAction::UpdateDraft { .. }))
            .into_iter()
            .find_map(|action| match action {
                EditorAction::UpdateDraft { before, .. } => Some(before),
                _ => None,
            })
            .unwrap_or(Some(draft.clone()));

        component.add_wire_edge(edge.clone());
        sync_component_wires(wire_overlay, component, device, queue);
        self.history.push(EditorAction::CommitWire {
            edge,
            previous_draft,
        });
        true
    }

    pub fn cancel_wire_draft(
        &mut self,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        self.update_draft(wire_overlay, |overlay| {
            overlay.restore_draft(device, queue, None);
        })
    }

    pub fn pop_wire_point(
        &mut self,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        self.update_draft(wire_overlay, |overlay| {
            overlay.pop_point(device, queue);
        })
    }

    pub fn handle_left_click(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: render::CameraState,
        cursor: [f32; 2],
        displayed_layer: u32,
        extend_wire: bool,
    ) -> bool {
        let Some(source) = wires::snap_cursor_to_cell(
            camera,
            cursor,
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        ) else {
            return false;
        };

        match self.selected_tool() {
            EditorTool::Wire => {
                let Some(point) = wires::cursor_to_board_point(
                    camera,
                    cursor,
                    [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
                ) else {
                    return false;
                };

                let had_draft = wire_overlay.has_draft();
                self.update_draft(wire_overlay, |overlay| {
                    overlay.add_point(device, queue, displayed_layer, point, source);
                });

                if had_draft && !extend_wire {
                    self.finish_wire_attempt(component, wire_overlay, device, queue)
                } else {
                    true
                }
            }
            tool => {
                if let Some(action) = place_cell_at_cursor(
                    simulation,
                    device,
                    queue,
                    camera,
                    cursor,
                    displayed_layer,
                    tool,
                ) {
                    self.history.push(action);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn handle_right_click(
        &mut self,
        simulation: &simulation::Simulation,
        component: &mut wire_render::WireRenderInfo,
        wire_overlay: &mut wires::WireOverlay,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: render::CameraState,
        cursor_position: Option<[f32; 2]>,
        displayed_layer: u32,
        extend_wire: bool,
    ) -> bool {
        if self.selected_tool() != EditorTool::Wire {
            self.ui.reset_to_default_tool();
            wire_overlay.restore_draft(device, queue, None);
            return true;
        }

        if wire_overlay.has_draft() {
            return if extend_wire {
                self.pop_wire_point(wire_overlay, device, queue)
            } else {
                self.cancel_wire_draft(wire_overlay, device, queue)
            };
        }

        if let Some(action) = delete_at_cursor(
            simulation,
            component,
            wire_overlay,
            device,
            queue,
            camera,
            cursor_position,
            displayed_layer,
        ) {
            self.history.push(action);
            true
        } else {
            false
        }
    }

    fn update_draft<F>(&mut self, wire_overlay: &mut wires::WireOverlay, edit: F) -> bool
    where
        F: FnOnce(&mut wires::WireOverlay),
    {
        let before = wire_overlay.current_draft();
        edit(wire_overlay);
        let after = wire_overlay.current_draft();
        if before == after {
            return false;
        }

        self.history
            .push(EditorAction::UpdateDraft { before, after });
        true
    }
}

impl EditorUi {
    pub fn new(tool_preview_textures: [TextureId; EditorTool::COUNT]) -> Self {
        Self {
            expanded: false,
            selected_tool: EditorTool::Wire,
            selected_wire_color: wires::DEFAULT_WIRE_COLOR,
            tool_preview_textures,
        }
    }

    pub fn selected_tool(&self) -> EditorTool {
        self.selected_tool
    }

    pub fn selected_wire_color(&self) -> [f32; 4] {
        self.selected_wire_color
    }

    pub fn reset_to_default_tool(&mut self) {
        self.selected_tool = EditorTool::Wire;
    }

    pub fn show<T>(
        &mut self,
        ctx: &egui::Context,
        displayed_layer: u32,
        history: &EditorHistory<T>,
    ) -> bool {
        let screen_rect = ctx.content_rect();
        let panel_height = PANEL_HEIGHT.min((screen_rect.height() - PANEL_MARGIN * 2.0).max(160.0));
        let origin = Pos2::new(
            screen_rect.left() + PANEL_MARGIN,
            screen_rect.top() + PANEL_MARGIN,
        );
        let reset_origin = Pos2::new(
            screen_rect.right() - PANEL_MARGIN - RESET_BUTTON_WIDTH,
            screen_rect.top() + PANEL_MARGIN,
        );
        let tag_rect = Rect::from_min_size(origin, Vec2::new(TAG_WIDTH, TAG_HEIGHT));
        let mut reset_requested = false;

        let expanded_rect = Rect::from_min_size(
            origin,
            Vec2::new(PANEL_WIDTH + WIRE_COLOR_MENU_WIDTH + 8.0, panel_height),
        );
        let mut wire_menu_anchor = None;
        let pointer_pos = ctx.input(|input| input.pointer.hover_pos());
        let hovered_activation = pointer_pos.is_some_and(|pos| tag_rect.contains(pos));
        let hovered_panel =
            self.expanded && pointer_pos.is_some_and(|pos| expanded_rect.contains(pos));
        let visible_width = if self.expanded || hovered_activation {
            PANEL_WIDTH + WIRE_COLOR_MENU_WIDTH + 8.0
        } else {
            TAG_WIDTH
        };
        let visible_height = if self.expanded || hovered_activation {
            panel_height
        } else {
            TAG_HEIGHT
        };
        self.expanded = hovered_activation || hovered_panel;

        egui::Area::new(Id::new("editor_hover_menu"))
            .order(Order::Foreground)
            .fixed_pos(origin)
            .show(ctx, |ui| {
                ui.set_min_size(Vec2::new(visible_width, visible_height));

                if self.expanded {
                    let frame = egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(10, 12, 16, 236))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(58, 72, 90)))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(10.0);

                    frame.show(ui, |ui| {
                        ui.set_width(PANEL_INNER_WIDTH);
                        ui.horizontal(|ui| {
                            ui.heading("Editor");
                            ui.label(
                                RichText::new(format!("Layer {}", displayed_layer))
                                    .small()
                                    .color(Color32::from_rgb(170, 184, 198)),
                            );
                        });
                        ui.label(
                            RichText::new(format!("Active: {}", self.selected_tool.title()))
                                .small()
                                .strong()
                                .color(Color32::from_rgb(214, 224, 235)),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Undo {}  Redo {}",
                                if history.can_undo() { "Ctrl+Z" } else { "-" },
                                if history.can_redo() {
                                    "Ctrl+Shift+Z / Ctrl+Y"
                                } else {
                                    "-"
                                },
                            ))
                            .small()
                            .color(Color32::from_rgb(160, 174, 190)),
                        );
                        ui.add_space(8.0);

                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for tool in TOOL_OPTIONS {
                                    let response = draw_tool_card(
                                        ui,
                                        *tool,
                                        *tool == self.selected_tool,
                                        if *tool == EditorTool::Wire {
                                            Some(self.selected_wire_color)
                                        } else {
                                            None
                                        },
                                        self.tool_preview_textures[tool.preview_index()],
                                    );
                                    if *tool == EditorTool::Wire
                                        && (response.hovered()
                                            || self.selected_tool == EditorTool::Wire)
                                    {
                                        wire_menu_anchor = Some(response.rect);
                                    }
                                    if response.clicked() {
                                        self.selected_tool = *tool;
                                    }
                                    ui.add_space(6.0);
                                }
                            });
                    });
                } else {
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(TAG_WIDTH, TAG_HEIGHT), Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(
                        rect,
                        CornerRadius::same(8),
                        Color32::from_rgba_unmultiplied(12, 16, 22, 220),
                    );
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(8),
                        Stroke::new(1.0, Color32::from_rgb(58, 72, 90)),
                        StrokeKind::Outside,
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Tools",
                        FontId::proportional(12.0),
                        Color32::from_rgb(214, 224, 235),
                    );
                }
            });

        if self.expanded {
            if let Some(anchor) = wire_menu_anchor {
                show_wire_color_menu(
                    ctx,
                    anchor,
                    &mut self.selected_wire_color,
                    &mut self.selected_tool,
                );
            }
        }

        egui::Area::new(Id::new("editor_reset_view_button"))
            .order(Order::Foreground)
            .fixed_pos(reset_origin)
            .show(ctx, |ui| {
                let button = egui::Button::new(RichText::new("Reset").size(12.0))
                    .min_size(Vec2::new(RESET_BUTTON_WIDTH, RESET_BUTTON_HEIGHT))
                    .fill(Color32::from_rgba_unmultiplied(12, 16, 22, 220))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(58, 72, 90)))
                    .corner_radius(CornerRadius::same(8));
                if ui.add(button).clicked() {
                    reset_requested = true;
                }
            });

        reset_requested
    }
}

pub fn hover_preview_state_with_visibility(
    camera: render::CameraState,
    cursor_position: Option<[f32; 2]>,
    tool: EditorTool,
    visible: bool,
) -> Option<render::HoverPreviewState> {
    if !visible || !tool.is_placeable() {
        return None;
    }

    let cursor = cursor_position?;
    let grid_cell = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    )?;

    Some(render::HoverPreviewState {
        cell: [grid_cell.x, grid_cell.y],
        circuit: snapshot_for_tool(tool)?.bytes,
        charge: charge_values_for_tool(tool).into_iter().next().unwrap_or(0),
    })
}

fn sync_component_wires(
    wire_overlay: &mut wires::WireOverlay,
    component: &wire_render::WireRenderInfo,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    wire_overlay.replace_wires(device, queue, component.wire_edges().cloned().collect());
}

fn delete_at_cursor(
    simulation: &simulation::Simulation,
    component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: render::CameraState,
    cursor_position: Option<[f32; 2]>,
    displayed_layer: u32,
) -> Option<EditorAction> {
    let cursor = cursor_position?;
    let grid_cell = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    )?;
    let point = wires::cursor_to_board_point(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    )?;

    if let Some(edge) = component.remove_wire_at_point(displayed_layer, point) {
        sync_component_wires(wire_overlay, component, device, queue);
        Some(EditorAction::DeleteWire(edge))
    } else {
        let cell = simulation.read_cell(device, queue, grid_cell, displayed_layer);
        let charge_values = (0..simulation::CHARGE_BUFFER_COUNT)
            .map(|buffer_index| {
                pollster::block_on(simulation.read_charge_value(
                    device,
                    queue,
                    buffer_index,
                    grid_cell.x,
                    grid_cell.y,
                    displayed_layer,
                ))
            })
            .collect::<Vec<_>>();
        if cell.bytes == [0, 0, 0, 0] && charge_values.iter().all(|value| *value == 0) {
            return None;
        }
        simulation.clear_cell(queue, grid_cell, displayed_layer);
        simulation.clear_charge_at(device, queue, grid_cell, displayed_layer);
        Some(EditorAction::DeleteCell {
            grid_cell,
            layer: displayed_layer,
            cell,
            charge_values,
        })
    }
}

fn place_cell_at_cursor(
    simulation: &simulation::Simulation,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera: render::CameraState,
    cursor: [f32; 2],
    displayed_layer: u32,
    tool: EditorTool,
) -> Option<EditorAction> {
    let grid_cell = wires::snap_cursor_to_cell(
        camera,
        cursor,
        [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
    )?;
    let new_cell = snapshot_for_tool(tool)?;
    let previous_cell = simulation.read_cell(device, queue, grid_cell, displayed_layer);

    if previous_cell == new_cell {
        return None;
    }

    let previous_charge_values = (0..simulation::CHARGE_BUFFER_COUNT)
        .map(|buffer_index| {
            pollster::block_on(simulation.read_charge_value(
                device,
                queue,
                buffer_index,
                grid_cell.x,
                grid_cell.y,
                displayed_layer,
            ))
        })
        .collect::<Vec<_>>();

    let new_charge_values = charge_values_for_tool(tool);

    simulation.write_cell(queue, grid_cell, displayed_layer, new_cell);
    write_charge_values(
        simulation,
        device,
        queue,
        grid_cell,
        displayed_layer,
        &new_charge_values,
    );

    Some(EditorAction::PlaceCell {
        grid_cell,
        layer: displayed_layer,
        previous_cell,
        previous_charge_values,
        new_cell,
        new_charge_values,
    })
}

fn snapshot_for_tool(tool: EditorTool) -> Option<simulation::CellSnapshot> {
    match tool {
        EditorTool::Wire => None,
        EditorTool::Source => Some(simulation::CellSnapshot::source(0xff)),
        EditorTool::Noop => Some(simulation::CellSnapshot::noop()),
        EditorTool::Not => Some(simulation::CellSnapshot::gate(simulation::GateKind::Not)),
        EditorTool::And => Some(simulation::CellSnapshot::gate(simulation::GateKind::And)),
        EditorTool::Or => Some(simulation::CellSnapshot::gate(simulation::GateKind::Or)),
        EditorTool::Xor => Some(simulation::CellSnapshot::gate(simulation::GateKind::Xor)),
        EditorTool::Nand => Some(simulation::CellSnapshot::gate(simulation::GateKind::Nand)),
        EditorTool::Nor => Some(simulation::CellSnapshot::gate(simulation::GateKind::Nor)),
        EditorTool::Xnor => Some(simulation::CellSnapshot::gate(simulation::GateKind::Xnor)),
    }
}

fn charge_values_for_tool(tool: EditorTool) -> Vec<u8> {
    let value = match tool {
        EditorTool::Source => 0xff,
        _ => 0x00,
    };

    vec![value; simulation::CHARGE_BUFFER_COUNT as usize]
}

fn write_charge_values(
    simulation: &simulation::Simulation,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    grid_cell: wires::GridCell,
    layer: u32,
    charge_values: &[u8],
) {
    for (buffer_index, value) in charge_values.iter().copied().enumerate() {
        simulation.write_charge_value(device, queue, buffer_index as u32, grid_cell, layer, value);
    }
}

fn apply_action(
    action: &EditorAction,
    simulation: &simulation::Simulation,
    component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    match action {
        EditorAction::UpdateDraft { after, .. } => {
            wire_overlay.restore_draft(device, queue, after.as_ref());
        }
        EditorAction::CommitWire { edge, .. } => {
            component.add_wire_edge(edge.clone());
            sync_component_wires(wire_overlay, component, device, queue);
            wire_overlay.restore_draft(device, queue, None);
        }
        EditorAction::DeleteWire(edge) => {
            component.remove_matching_wire_edge(edge);
            sync_component_wires(wire_overlay, component, device, queue);
        }
        EditorAction::PlaceCell {
            grid_cell,
            layer,
            new_cell,
            new_charge_values,
            ..
        } => {
            simulation.write_cell(queue, *grid_cell, *layer, *new_cell);
            write_charge_values(
                simulation,
                device,
                queue,
                *grid_cell,
                *layer,
                new_charge_values,
            );
        }
        EditorAction::DeleteCell {
            grid_cell, layer, ..
        } => {
            simulation.clear_cell(queue, *grid_cell, *layer);
            simulation.clear_charge_at(device, queue, *grid_cell, *layer);
        }
    }
}

fn apply_inverse_action(
    action: &EditorAction,
    simulation: &simulation::Simulation,
    component: &mut wire_render::WireRenderInfo,
    wire_overlay: &mut wires::WireOverlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    match action {
        EditorAction::UpdateDraft { before, .. } => {
            wire_overlay.restore_draft(device, queue, before.as_ref());
        }
        EditorAction::CommitWire {
            edge,
            previous_draft,
        } => {
            component.remove_matching_wire_edge(edge);
            sync_component_wires(wire_overlay, component, device, queue);
            wire_overlay.restore_draft(device, queue, previous_draft.as_ref());
        }
        EditorAction::DeleteWire(edge) => {
            component.add_wire_edge(edge.clone());
            sync_component_wires(wire_overlay, component, device, queue);
        }
        EditorAction::PlaceCell {
            grid_cell,
            layer,
            previous_cell,
            previous_charge_values,
            ..
        } => {
            simulation.write_cell(queue, *grid_cell, *layer, *previous_cell);
            write_charge_values(
                simulation,
                device,
                queue,
                *grid_cell,
                *layer,
                previous_charge_values,
            );
        }
        EditorAction::DeleteCell {
            grid_cell,
            layer,
            cell,
            charge_values,
        } => {
            simulation.write_cell(queue, *grid_cell, *layer, *cell);
            for (buffer_index, value) in charge_values.iter().copied().enumerate() {
                simulation.write_charge_value(
                    device,
                    queue,
                    buffer_index as u32,
                    *grid_cell,
                    *layer,
                    value,
                );
            }
        }
    }
}

fn draw_tool_card(
    ui: &mut egui::Ui,
    tool: EditorTool,
    selected: bool,
    color_chip: Option<[f32; 4]>,
    preview_texture: TextureId,
) -> egui::Response {
    let size = Vec2::new(ui.available_width(), TOOL_CARD_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let hovered = response.hovered();
    let background = if selected {
        Color32::from_rgb(34, 44, 58)
    } else if hovered {
        Color32::from_rgb(24, 31, 42)
    } else {
        Color32::from_rgb(16, 20, 27)
    };
    let border = if selected {
        Color32::from_rgb(120, 156, 194)
    } else if hovered {
        Color32::from_rgb(80, 102, 128)
    } else {
        Color32::from_rgb(46, 58, 74)
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(10),
        background,
        Stroke::new(1.0, border),
        StrokeKind::Outside,
    );

    let preview_rect = Rect::from_min_size(rect.min + Vec2::new(10.0, 10.0), Vec2::new(62.0, 58.0));
    ui.painter().rect_filled(
        preview_rect,
        CornerRadius::same(8),
        Color32::from_rgb(10, 12, 16),
    );
    ui.painter().rect_stroke(
        preview_rect,
        CornerRadius::same(8),
        Stroke::new(1.0, Color32::from_rgb(40, 50, 64)),
        StrokeKind::Outside,
    );
    let _ = ui.put(
        preview_rect.shrink(6.0),
        egui::Image::new((preview_texture, preview_rect.shrink(6.0).size())),
    );

    let painter = ui.painter();
    let text_left = preview_rect.max.x + 10.0;
    let title_pos = Pos2::new(text_left, rect.min.y + 14.0);
    let description_pos = Pos2::new(text_left, rect.min.y + 38.0);
    painter.text(
        title_pos,
        egui::Align2::LEFT_TOP,
        tool.title(),
        FontId::proportional(17.0),
        Color32::from_rgb(230, 236, 242),
    );
    painter.text(
        description_pos,
        egui::Align2::LEFT_TOP,
        tool.description(),
        FontId::proportional(12.0),
        Color32::from_rgb(160, 174, 190),
    );

    if selected {
        let badge_rect = Rect::from_min_size(
            Pos2::new(rect.right() - 58.0, rect.min.y + 10.0),
            Vec2::new(48.0, 20.0),
        );
        painter.rect_filled(
            badge_rect,
            CornerRadius::same(255),
            Color32::from_rgb(50, 64, 82),
        );
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            "active",
            FontId::proportional(11.0),
            Color32::from_rgb(206, 220, 234),
        );
    }

    if let Some(color_chip) = color_chip {
        let swatch_rect = Rect::from_min_size(
            Pos2::new(rect.right() - 28.0, rect.bottom() - 28.0),
            Vec2::new(16.0, 16.0),
        );
        ui.painter().rect_filled(
            swatch_rect,
            CornerRadius::same(6),
            color32_from_wire(color_chip),
        );
        ui.painter().rect_stroke(
            swatch_rect,
            CornerRadius::same(6),
            Stroke::new(1.0, Color32::from_rgb(230, 236, 242)),
            StrokeKind::Outside,
        );
    }

    response
}

fn show_wire_color_menu(
    ctx: &egui::Context,
    anchor: Rect,
    selected_wire_color: &mut [f32; 4],
    selected_tool: &mut EditorTool,
) {
    egui::Area::new(Id::new("wire_color_submenu"))
        .order(Order::Foreground)
        .fixed_pos(Pos2::new(anchor.right() + 8.0, anchor.top()))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(10, 12, 16, 236))
                .stroke(Stroke::new(1.0, Color32::from_rgb(58, 72, 90)))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_width(WIRE_COLOR_MENU_WIDTH);
                    ui.label(
                        RichText::new("Wire Color")
                            .strong()
                            .color(Color32::from_rgb(230, 236, 242)),
                    );
                    ui.label(
                        RichText::new("Choose the color for new wires")
                            .small()
                            .color(Color32::from_rgb(160, 174, 190)),
                    );
                    ui.add_space(8.0);

                    for (color, label) in WIRE_COLOR_OPTIONS {
                        let selected = *selected_wire_color == color;
                        let response = ui.add(
                            egui::Button::new(
                                RichText::new(label).color(Color32::from_rgb(230, 236, 242)),
                            )
                            .min_size(Vec2::new(ui.available_width(), 28.0))
                            .fill(if selected {
                                Color32::from_rgb(34, 44, 58)
                            } else {
                                Color32::from_rgb(16, 20, 27)
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if selected {
                                    Color32::from_rgb(120, 156, 194)
                                } else {
                                    Color32::from_rgb(46, 58, 74)
                                },
                            )),
                        );

                        let swatch_rect = Rect::from_min_size(
                            Pos2::new(response.rect.left() + 8.0, response.rect.center().y - 7.0),
                            Vec2::new(14.0, 14.0),
                        );
                        ui.painter().rect_filled(
                            swatch_rect,
                            CornerRadius::same(5),
                            color32_from_wire(color),
                        );
                        ui.painter().rect_stroke(
                            swatch_rect,
                            CornerRadius::same(5),
                            Stroke::new(1.0, Color32::from_rgb(230, 236, 242)),
                            StrokeKind::Outside,
                        );

                        if response.clicked() {
                            *selected_wire_color = color;
                            *selected_tool = EditorTool::Wire;
                        }

                        ui.add_space(4.0);
                    }
                });
        });
}

fn color32_from_wire(color: [f32; 4]) -> Color32 {
    Color32::from_rgba_premultiplied(
        (color[0] * 255.0).round() as u8,
        (color[1] * 255.0).round() as u8,
        (color[2] * 255.0).round() as u8,
        (color[3] * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use egui_winit::winit::dpi::PhysicalSize;

    const TEST_SURFACE_SIZE: PhysicalSize<u32> = PhysicalSize::new(1600, 900);

    async fn create_headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;

        adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()
    }

    fn create_editor_test_context() -> Option<(
        wgpu::Device,
        wgpu::Queue,
        simulation::Simulation,
        wire_render::WireRenderInfo,
        wires::WireOverlay,
        render::CameraState,
        EditorSession,
    )> {
        let (device, queue) = pollster::block_on(create_headless_device())?;
        let simulation = simulation::Simulation::new(&device, &queue);
        let component = wire_render::WireRenderInfo::new(wire_render::WireBufferId {
            texture_index: 0,
            layer: 0,
        });
        let wire_overlay = wires::WireOverlay::new(
            &device,
            &queue,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            TEST_SURFACE_SIZE,
            [simulation::GRID_WIDTH, simulation::GRID_HEIGHT],
        );
        let camera = render::CameraState::new(TEST_SURFACE_SIZE);
        let session = EditorSession::new([TextureId::Managed(0); EditorTool::COUNT]);

        Some((
            device,
            queue,
            simulation,
            component,
            wire_overlay,
            camera,
            session,
        ))
    }

    fn cursor_for_cell(cell: wires::GridCell) -> [f32; 2] {
        [
            (cell.x as f32 + 0.5) / simulation::GRID_WIDTH as f32 * TEST_SURFACE_SIZE.width as f32,
            (cell.y as f32 + 0.5) / simulation::GRID_HEIGHT as f32
                * TEST_SURFACE_SIZE.height as f32,
        ]
    }

    fn read_charge_values(
        simulation: &simulation::Simulation,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cell: wires::GridCell,
        layer: u32,
    ) -> Vec<u8> {
        (0..simulation::CHARGE_BUFFER_COUNT)
            .map(|buffer_index| {
                pollster::block_on(simulation.read_charge_value(
                    device,
                    queue,
                    buffer_index,
                    cell.x,
                    cell.y,
                    layer,
                ))
            })
            .collect()
    }

    #[test]
    fn history_uses_single_cursor_and_truncates_redo_branch() {
        let mut history = EditorHistory::default();

        history.push("first");
        history.push("second");

        assert_eq!(history.actions, vec!["first", "second"]);
        assert_eq!(history.next_index, 2);
        assert!(history.can_undo());
        assert!(!history.can_redo());

        assert_eq!(history.undo_action(), Some(&"second"));
        assert_eq!(history.next_index, 1);
        assert!(history.can_undo());
        assert!(history.can_redo());

        history.push("replacement");

        assert_eq!(history.actions, vec!["first", "replacement"]);
        assert_eq!(history.next_index, 2);
        assert!(history.can_undo());
        assert!(!history.can_redo());
        assert_eq!(history.redo_action(), None);
    }

    #[test]
    fn history_redo_does_not_duplicate_actions_after_many_cycles() {
        let mut history = EditorHistory::default();

        history.push("a");
        history.push("b");
        history.push("c");

        for _ in 0..8 {
            assert_eq!(history.undo_action(), Some(&"c"));
            assert_eq!(history.redo_action(), Some(&"c"));
        }

        assert_eq!(history.actions, vec!["a", "b", "c"]);
        assert_eq!(history.next_index, 3);
        assert!(history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn placing_tools_writes_expected_cell_and_charge_values() {
        let Some((device, queue, simulation, _, _, camera, _)) = create_editor_test_context()
        else {
            return;
        };

        let cases = [
            (
                EditorTool::Source,
                wires::GridCell { x: 0, y: 7 },
                simulation::CellSnapshot::source(0xff),
                vec![0xff; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::Not,
                wires::GridCell { x: 1, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::Not),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::And,
                wires::GridCell { x: 2, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::And),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::Or,
                wires::GridCell { x: 3, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::Or),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::Xor,
                wires::GridCell { x: 4, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::Xor),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::Nand,
                wires::GridCell { x: 5, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::Nand),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::Nor,
                wires::GridCell { x: 6, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::Nor),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
            (
                EditorTool::Xnor,
                wires::GridCell { x: 7, y: 7 },
                simulation::CellSnapshot::gate(simulation::GateKind::Xnor),
                vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize],
            ),
        ];

        for (tool, grid_cell, expected_cell, expected_charge_values) in cases {
            let action = place_cell_at_cursor(
                &simulation,
                &device,
                &queue,
                camera,
                cursor_for_cell(grid_cell),
                0,
                tool,
            )
            .unwrap();

            match action {
                EditorAction::PlaceCell {
                    grid_cell: action_grid_cell,
                    layer,
                    previous_cell,
                    previous_charge_values,
                    new_cell,
                    new_charge_values,
                } => {
                    assert_eq!(action_grid_cell, grid_cell);
                    assert_eq!(layer, 0);
                    assert_eq!(previous_cell, simulation::CellSnapshot::empty());
                    assert_eq!(
                        previous_charge_values,
                        vec![0x00; simulation::CHARGE_BUFFER_COUNT as usize]
                    );
                    assert_eq!(new_cell, expected_cell);
                    assert_eq!(new_charge_values, expected_charge_values);
                }
                _ => panic!("expected place-cell action"),
            }

            assert_eq!(
                simulation.read_cell(&device, &queue, grid_cell, 0),
                expected_cell
            );
            assert_eq!(
                read_charge_values(&simulation, &device, &queue, grid_cell, 0),
                expected_charge_values
            );
        }
    }

    #[test]
    fn undo_and_redo_restore_previous_cell_and_charge_values() {
        let Some((device, queue, simulation, mut component, mut wire_overlay, camera, mut session)) =
            create_editor_test_context()
        else {
            return;
        };

        let grid_cell = wires::GridCell { x: 3, y: 4 };
        let layer = 0;
        let previous_cell = simulation::CellSnapshot::gate(simulation::GateKind::Not);
        let previous_charge_values = [0x12, 0x34];

        simulation.write_cell(&queue, grid_cell, layer, previous_cell);
        for (buffer_index, value) in previous_charge_values.into_iter().enumerate() {
            simulation.write_charge_value(
                &device,
                &queue,
                buffer_index as u32,
                grid_cell,
                layer,
                value,
            );
        }

        let action = place_cell_at_cursor(
            &simulation,
            &device,
            &queue,
            camera,
            cursor_for_cell(grid_cell),
            layer,
            EditorTool::Source,
        )
        .unwrap();
        session.history.push(action);

        assert_eq!(
            simulation.read_cell(&device, &queue, grid_cell, layer),
            simulation::CellSnapshot::source(0xff)
        );
        assert_eq!(
            read_charge_values(&simulation, &device, &queue, grid_cell, layer),
            vec![0xff; simulation::CHARGE_BUFFER_COUNT as usize]
        );

        assert!(session.undo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            simulation.read_cell(&device, &queue, grid_cell, layer),
            previous_cell
        );
        assert_eq!(
            read_charge_values(&simulation, &device, &queue, grid_cell, layer),
            previous_charge_values.to_vec()
        );

        assert!(session.redo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            simulation.read_cell(&device, &queue, grid_cell, layer),
            simulation::CellSnapshot::source(0xff)
        );
        assert_eq!(
            read_charge_values(&simulation, &device, &queue, grid_cell, layer),
            vec![0xff; simulation::CHARGE_BUFFER_COUNT as usize]
        );
    }

    #[test]
    fn wire_draft_point_edits_are_undoable() {
        let Some((device, queue, simulation, mut component, mut wire_overlay, camera, mut session)) =
            create_editor_test_context()
        else {
            return;
        };

        let first = wires::GridCell { x: 2, y: 2 };
        let second = wires::GridCell { x: 5, y: 2 };

        assert!(session.handle_left_click(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
            camera,
            cursor_for_cell(first),
            0,
            true,
        ));
        assert_eq!(
            wire_overlay
                .current_draft()
                .as_ref()
                .map(|draft| draft.points.len()),
            Some(1)
        );

        assert!(session.handle_left_click(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
            camera,
            cursor_for_cell(second),
            0,
            true,
        ));
        assert_eq!(
            wire_overlay
                .current_draft()
                .as_ref()
                .map(|draft| draft.points.len()),
            Some(2)
        );

        assert!(session.undo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            wire_overlay
                .current_draft()
                .as_ref()
                .map(|draft| draft.points.len()),
            Some(1)
        );

        assert!(session.redo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            wire_overlay
                .current_draft()
                .as_ref()
                .map(|draft| draft.points.len()),
            Some(2)
        );

        assert!(session.pop_wire_point(&mut wire_overlay, &device, &queue));
        assert_eq!(
            wire_overlay
                .current_draft()
                .as_ref()
                .map(|draft| draft.points.len()),
            Some(1)
        );

        assert!(session.undo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(
            wire_overlay
                .current_draft()
                .as_ref()
                .map(|draft| draft.points.len()),
            Some(2)
        );
    }

    #[test]
    fn undoing_a_committed_wire_clears_it_without_reviving_the_draft() {
        let Some((device, queue, simulation, mut component, mut wire_overlay, camera, mut session)) =
            create_editor_test_context()
        else {
            return;
        };

        let first = wires::GridCell { x: 2, y: 2 };
        let second = wires::GridCell { x: 5, y: 2 };

        assert!(session.handle_left_click(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
            camera,
            cursor_for_cell(first),
            0,
            true,
        ));
        assert!(session.handle_left_click(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
            camera,
            cursor_for_cell(second),
            0,
            false,
        ));

        assert!(wire_overlay.current_draft().is_none());
        assert_eq!(component.wire_edges().count(), 1);

        assert!(session.undo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(component.wire_edges().count(), 0);
        assert!(wire_overlay.current_draft().is_none());

        assert!(session.redo(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
        ));
        assert_eq!(component.wire_edges().count(), 1);
        assert!(wire_overlay.current_draft().is_none());
    }

    #[test]
    fn committed_wires_use_selected_wire_color() {
        let Some((device, queue, simulation, mut component, mut wire_overlay, camera, mut session)) =
            create_editor_test_context()
        else {
            return;
        };

        let selected_color = [0.87, 0.32, 0.28, 1.0];
        session.ui.selected_wire_color = selected_color;
        wire_overlay.set_draft_color(&device, &queue, selected_color);

        assert!(session.handle_left_click(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
            camera,
            cursor_for_cell(wires::GridCell { x: 1, y: 1 }),
            0,
            true,
        ));
        assert!(session.handle_left_click(
            &simulation,
            &mut component,
            &mut wire_overlay,
            &device,
            &queue,
            camera,
            cursor_for_cell(wires::GridCell { x: 4, y: 1 }),
            0,
            true,
        ));
        assert!(session.finish_wire_attempt(&mut component, &mut wire_overlay, &device, &queue));

        let wire = component.wire_edges().next().unwrap();
        assert_eq!(wire.color, selected_color);
    }
}
