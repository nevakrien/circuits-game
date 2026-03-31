use egui::{
    Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextureId, Vec2,
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
    tool_preview_textures: [TextureId; EditorTool::COUNT],
}

pub struct EditorHistory<T> {
    undo_stack: Vec<T>,
    redo_stack: Vec<T>,
}

impl<T> Default for EditorHistory<T> {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}

impl<T> EditorHistory<T> {
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn push(&mut self, action: T) {
        self.undo_stack.push(action);
        self.redo_stack.clear();
    }

    pub fn pop_undo(&mut self) -> Option<T> {
        self.undo_stack.pop()
    }

    pub fn push_redo(&mut self, action: T) {
        self.redo_stack.push(action);
    }

    pub fn pop_redo(&mut self) -> Option<T> {
        self.redo_stack.pop()
    }

    pub fn push_undo(&mut self, action: T) {
        self.undo_stack.push(action);
    }
}

impl EditorUi {
    pub fn new(tool_preview_textures: [TextureId; EditorTool::COUNT]) -> Self {
        Self {
            expanded: false,
            selected_tool: EditorTool::Wire,
            tool_preview_textures,
        }
    }

    pub fn selected_tool(&self) -> EditorTool {
        self.selected_tool
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
                                        self.tool_preview_textures[tool.preview_index()],
                                    );
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

fn draw_tool_card(
    ui: &mut egui::Ui,
    tool: EditorTool,
    selected: bool,
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

    response
}
