use egui::{
    Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Vec2,
};

const TAG_WIDTH: f32 = 54.0;
const TAG_HEIGHT: f32 = 24.0;
const RESET_BUTTON_WIDTH: f32 = 64.0;
const RESET_BUTTON_HEIGHT: f32 = 24.0;
const PANEL_WIDTH: f32 = 260.0;
const PANEL_HEIGHT: f32 = 420.0;
const PANEL_MARGIN: f32 = 12.0;
const PANEL_INNER_WIDTH: f32 = PANEL_WIDTH - 24.0;
const TOOL_CARD_HEIGHT: f32 = 78.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorTool {
    Wire,
    RemoveWire,
    Source,
    RemoveGate,
    Not,
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
}

impl EditorTool {
    fn title(self) -> &'static str {
        match self {
            Self::Wire => "Wire",
            Self::RemoveWire => "Remove Wire",
            Self::Source => "Source",
            Self::RemoveGate => "Remove Gate",
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
            Self::Wire => "Route a new wire path",
            Self::RemoveWire => "Delete an existing wire",
            Self::Source => "Place a constant signal source",
            Self::RemoveGate => "Delete a placed gate",
            Self::Not => "Invert the incoming signal",
            Self::And => "Output high when both inputs are high",
            Self::Or => "Output high when either input is high",
            Self::Xor => "Output high when inputs differ",
            Self::Nand => "Output low only when both inputs are high",
            Self::Nor => "Output high only when both inputs are low",
            Self::Xnor => "Output high when inputs match",
        }
    }
}

const TOOL_OPTIONS: &[EditorTool] = &[
    EditorTool::Wire,
    EditorTool::RemoveWire,
    EditorTool::Source,
    EditorTool::RemoveGate,
    EditorTool::Not,
    EditorTool::And,
    EditorTool::Or,
    EditorTool::Xor,
    EditorTool::Nand,
    EditorTool::Nor,
    EditorTool::Xnor,
];

pub struct EditorUi {
    expanded: bool,
    selected_tool: EditorTool,
}

impl EditorUi {
    pub fn new() -> Self {
        Self {
            expanded: false,
            selected_tool: EditorTool::Wire,
        }
    }

    pub fn selected_tool(&self) -> EditorTool {
        self.selected_tool
    }

    pub fn show(&mut self, ctx: &egui::Context, displayed_layer: u32) -> bool {
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

        let expanded_rect = Rect::from_min_size(origin, Vec2::new(PANEL_WIDTH, panel_height));
        let pointer_pos = ctx.input(|input| input.pointer.hover_pos());
        let hovered_activation = pointer_pos.is_some_and(|pos| tag_rect.contains(pos));
        let hovered_panel =
            self.expanded && pointer_pos.is_some_and(|pos| expanded_rect.contains(pos));
        let visible_width = if self.expanded || hovered_activation {
            PANEL_WIDTH
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
                        ui.add_space(8.0);

                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for tool in TOOL_OPTIONS {
                                    let response =
                                        draw_tool_card(ui, *tool, *tool == self.selected_tool);
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

fn draw_tool_card(ui: &mut egui::Ui, tool: EditorTool, selected: bool) -> egui::Response {
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

    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        background,
        Stroke::new(1.0, border),
        StrokeKind::Outside,
    );

    let preview_rect = Rect::from_min_size(rect.min + Vec2::new(10.0, 10.0), Vec2::new(62.0, 58.0));
    painter.rect_filled(
        preview_rect,
        CornerRadius::same(8),
        Color32::from_rgb(10, 12, 16),
    );
    painter.rect_stroke(
        preview_rect,
        CornerRadius::same(8),
        Stroke::new(1.0, Color32::from_rgb(40, 50, 64)),
        StrokeKind::Outside,
    );
    draw_tool_preview(painter, preview_rect.shrink(6.0), tool, selected || hovered);

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

    response
}

fn draw_tool_preview(painter: &egui::Painter, rect: Rect, tool: EditorTool, active: bool) {
    match tool {
        EditorTool::Wire => draw_wire_preview(painter, rect, false, active),
        EditorTool::RemoveWire => draw_wire_preview(painter, rect, true, active),
        EditorTool::Source => draw_source_preview(painter, rect, active),
        EditorTool::RemoveGate => draw_gate_preview(painter, rect, "DEL", true, active),
        EditorTool::Not => draw_gate_preview(painter, rect, "NOT", false, active),
        EditorTool::And => draw_gate_preview(painter, rect, "AND", false, active),
        EditorTool::Or => draw_gate_preview(painter, rect, "OR", false, active),
        EditorTool::Xor => draw_gate_preview(painter, rect, "XOR", false, active),
        EditorTool::Nand => draw_gate_preview(painter, rect, "NAND", false, active),
        EditorTool::Nor => draw_gate_preview(painter, rect, "NOR", false, active),
        EditorTool::Xnor => draw_gate_preview(painter, rect, "XNOR", false, active),
    }
}

fn draw_source_preview(painter: &egui::Painter, rect: Rect, active: bool) {
    let glow = if active {
        Color32::from_rgba_unmultiplied(244, 206, 168, 70)
    } else {
        Color32::from_rgba_unmultiplied(188, 138, 110, 36)
    };
    painter.circle_filled(rect.center(), rect.width() * 0.34, glow);
    painter.circle_filled(
        rect.center(),
        rect.width() * 0.22,
        Color32::from_rgb(114, 62, 38),
    );
    painter.circle_stroke(
        rect.center(),
        rect.width() * 0.22,
        Stroke::new(1.5, Color32::from_rgb(238, 208, 176)),
    );
}

fn draw_wire_preview(painter: &egui::Painter, rect: Rect, removing: bool, active: bool) {
    let y = rect.center().y;
    let left = Pos2::new(rect.left() + 6.0, y);
    let mid = Pos2::new(rect.center().x, y);
    let right = Pos2::new(rect.right() - 8.0, y);
    let wire = if active {
        Color32::from_rgb(160, 220, 255)
    } else {
        Color32::from_rgb(72, 134, 176)
    };
    painter.line_segment([left, right], Stroke::new(4.0, wire));
    painter.circle_filled(mid, 9.0, Color32::from_rgb(32, 84, 122));
    painter.circle_stroke(mid, 9.0, Stroke::new(1.5, Color32::from_rgb(214, 236, 252)));

    let arrow_color = Color32::from_rgb(214, 48, 34);
    painter.line_segment(
        [Pos2::new(mid.x - 4.0, y), Pos2::new(mid.x + 10.0, y)],
        Stroke::new(2.0, arrow_color),
    );
    painter.line_segment(
        [Pos2::new(mid.x + 10.0, y), Pos2::new(mid.x + 5.0, y - 4.0)],
        Stroke::new(2.0, arrow_color),
    );
    painter.line_segment(
        [Pos2::new(mid.x + 10.0, y), Pos2::new(mid.x + 5.0, y + 4.0)],
        Stroke::new(2.0, arrow_color),
    );

    if removing {
        draw_delete_mark(painter, rect);
    }
}

fn draw_gate_preview(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    removing: bool,
    active: bool,
) {
    let gate_rect = rect.shrink2(Vec2::new(3.0, 10.0));
    let fill = if active {
        Color32::from_rgb(190, 202, 214)
    } else {
        Color32::from_rgb(68, 76, 86)
    };
    painter.rect_filled(gate_rect, CornerRadius::same(8), fill);
    painter.rect_stroke(
        gate_rect,
        CornerRadius::same(8),
        Stroke::new(1.5, Color32::from_rgb(224, 232, 240)),
        StrokeKind::Outside,
    );
    painter.text(
        gate_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(15.0),
        Color32::from_rgb(142, 26, 28),
    );

    if removing {
        draw_delete_mark(painter, rect);
    }
}

fn draw_delete_mark(painter: &egui::Painter, rect: Rect) {
    let center = Pos2::new(rect.right() - 7.0, rect.top() + 7.0);
    painter.circle_filled(center, 8.0, Color32::from_rgb(154, 28, 30));
    painter.line_segment(
        [center + Vec2::new(-3.0, -3.0), center + Vec2::new(3.0, 3.0)],
        Stroke::new(1.6, Color32::WHITE),
    );
    painter.line_segment(
        [center + Vec2::new(3.0, -3.0), center + Vec2::new(-3.0, 3.0)],
        Stroke::new(1.6, Color32::WHITE),
    );
}
