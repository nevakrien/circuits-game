use egui::Color32;

pub const DEFAULT_TICK_RATE: u32 = 6;
pub const STEP_RATE_FACTOR: f32 = 0.35;
pub const FAST_RATE_FACTOR: f32 = 2.5;
pub const PAUSED_VISUAL_RATE_FACTOR: f32 = 0.35;
pub const MAX_FRAME_STEP_BUDGET: u32 = 12;

pub const PANEL_BG: Color32 = Color32::from_rgb(15, 18, 25);
pub const GRID_BG: Color32 = Color32::from_rgb(21, 26, 35);
pub const GRID_LINE: Color32 = Color32::from_rgb(53, 64, 83);
pub const GATE_OFF: Color32 = Color32::from_rgb(50, 59, 74);
pub const GATE_ON: Color32 = Color32::from_rgb(53, 179, 118);
pub const GATE_STROKE: Color32 = Color32::from_rgb(118, 132, 156);
pub const GATE_INPUT_FILL: Color32 = Color32::from_rgb(103, 163, 255);
pub const GATE_INPUT_STROKE: Color32 = Color32::from_rgb(207, 228, 255);
pub const GATE_OUTPUT_FILL: Color32 = Color32::from_rgb(121, 224, 143);
pub const GATE_OUTPUT_STROKE: Color32 = Color32::from_rgb(220, 255, 228);
pub const INPUT_PORT_COLOR: Color32 = Color32::from_rgb(115, 173, 255);
pub const OUTPUT_PORT_COLOR: Color32 = Color32::from_rgb(129, 227, 148);
pub const CHILD_INPUT_COLOR: Color32 = Color32::from_rgb(237, 169, 97);
pub const CHILD_OUTPUT_COLOR: Color32 = Color32::from_rgb(214, 136, 255);
pub const ANCESTOR_COLOR: Color32 = Color32::from_rgb(205, 138, 255);

pub const PAD: f32 = 24.0;
pub const CELL: f32 = 88.0;
pub const GRID_STROKE: f32 = 1.0;
pub const PORT_RADIUS: f32 = 5.0;
pub const CHILD_BG: Color32 = Color32::from_rgb(36, 43, 57);
pub const CHILD_ZOOM_PREVIEW: f32 = 1.6;
pub const CHILD_PORT_INSET: f32 = 10.0;

pub const PULSE_CYCLES_PER_TICK: f32 = 0.14;
pub const MIN_PULSE_CYCLES_PER_SECOND: f32 = 0.18;
pub const MAX_PULSE_CYCLES_PER_SECOND: f32 = 2.4;
