use crate::simulation::{CellSnapshot, GateKind};

pub fn cell_at(x: u32, y: u32, z: u32) -> CellSnapshot {
    if z != 0 {
        return CellSnapshot::empty();
    }

    match (x, y) {
        (0, 0) => CellSnapshot::source(0xff),
        (0, 2) => CellSnapshot::source(0x00),
        (1, 1) => CellSnapshot::gate(GateKind::And),
        (2, 1) => CellSnapshot::wire((1, 1, 0)),
        (2, 3) => CellSnapshot::source(0xff),
        (3, 2) => CellSnapshot::gate(GateKind::Or),
        (4, 1) => CellSnapshot::wire((3, 2, 0)),
        (4, 2) => CellSnapshot::wire((3, 2, 0)),
        (4, 4) => CellSnapshot::source(0xff),
        (5, 1) => CellSnapshot::gate(GateKind::Not),
        (5, 3) => CellSnapshot::gate(GateKind::Nand),
        (6, 1) => CellSnapshot::wire((5, 1, 0)),
        (6, 3) => CellSnapshot::wire((5, 3, 0)),
        (7, 2) => CellSnapshot::gate(GateKind::Xnor),
        _ => CellSnapshot::empty(),
    }
}
