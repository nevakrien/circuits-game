use crate::{
    simulation::{CellSnapshot, GateKind},
    wire_render::{StoredWireEdge, WireEndpointId},
    wires::{GridCell, WirePoint, DEFAULT_WIRE_COLOR},
};

pub struct DemoComponent {
    pub arena_z: u32,
    pub cells: Vec<PlacedCell>,
    pub wires: Vec<StoredWireEdge>,
}

pub struct PlacedCell {
    pub grid_cell: GridCell,
    pub snapshot: CellSnapshot,
}

impl DemoComponent {
    pub fn cell_at(&self, x: u32, y: u32, z: u32) -> CellSnapshot {
        if z != self.arena_z {
            return CellSnapshot::empty();
        }

        self.cells
            .iter()
            .find(|cell| cell.grid_cell.x == x && cell.grid_cell.y == y)
            .map(|cell| cell.snapshot)
            .unwrap_or_else(CellSnapshot::empty)
    }
}

pub fn starter_component() -> DemoComponent {
    let arena_z = 0;

    DemoComponent {
        arena_z,
        cells: vec![
            placed_cell(0, 0, CellSnapshot::source(0xff)),
            placed_cell(0, 2, CellSnapshot::source(0x00)),
            placed_cell(1, 1, CellSnapshot::gate(GateKind::And)),
            placed_cell(2, 1, CellSnapshot::noop()),
            placed_cell(2, 3, CellSnapshot::source(0xff)),
            placed_cell(3, 2, CellSnapshot::gate(GateKind::Or)),
            placed_cell(4, 2, CellSnapshot::noop()),
            placed_cell(4, 4, CellSnapshot::source(0xff)),
            placed_cell(5, 0, CellSnapshot::source(0xff)),
            placed_cell(5, 2, CellSnapshot::gate(GateKind::Not)),
            placed_cell(5, 3, CellSnapshot::gate(GateKind::Nand)),
            placed_cell(6, 1, CellSnapshot::gate(GateKind::And)),
            placed_cell(6, 3, CellSnapshot::noop()),
            placed_cell(7, 2, CellSnapshot::gate(GateKind::Xnor)),
        ],
        wires: vec![
            wire(
                arena_z,
                (0, 0),
                (1, 1),
                &[(0.73, 0.5), (1.11, 0.5), (1.11, 1.24)],
            ),
            wire(
                arena_z,
                (0, 2),
                (1, 1),
                &[(0.73, 2.5), (1.11, 2.5), (1.11, 1.76)],
            ),
            wire(arena_z, (1, 1), (2, 1), &[(1.885, 1.5), (2.11, 1.5)]),
            wire(
                arena_z,
                (2, 1),
                (3, 2),
                &[(2.885, 1.5), (3.11, 1.5), (3.11, 2.24)],
            ),
            wire(
                arena_z,
                (2, 3),
                (3, 2),
                &[(2.73, 3.5), (3.11, 3.5), (3.11, 2.76)],
            ),
            wire(arena_z, (3, 2), (4, 2), &[(3.885, 2.5), (4.11, 2.5)]),
            wire(arena_z, (4, 2), (5, 2), &[(4.885, 2.5), (5.11, 2.5)]),
            wire(
                arena_z,
                (4, 2),
                (5, 3),
                &[(4.885, 2.5), (5.11, 2.5), (5.11, 3.24)],
            ),
            wire(
                arena_z,
                (4, 4),
                (5, 3),
                &[(4.73, 4.5), (5.11, 4.5), (5.11, 3.76)],
            ),
            wire(
                arena_z,
                (5, 0),
                (6, 1),
                &[(5.73, 0.5), (6.11, 0.5), (6.11, 1.24)],
            ),
            wire(
                arena_z,
                (5, 2),
                (6, 1),
                &[(5.885, 2.5), (6.11, 2.5), (6.11, 1.76)],
            ),
            wire(
                arena_z,
                (6, 1),
                (7, 2),
                &[(6.885, 1.5), (7.11, 1.5), (7.11, 2.24)],
            ),
            wire(arena_z, (5, 3), (6, 3), &[(5.885, 3.5), (6.11, 3.5)]),
            wire(
                arena_z,
                (6, 3),
                (7, 2),
                &[(6.885, 3.5), (7.11, 3.5), (7.11, 2.76)],
            ),
        ],
    }
}

fn placed_cell(x: u32, y: u32, snapshot: CellSnapshot) -> PlacedCell {
    PlacedCell {
        grid_cell: GridCell { x, y },
        snapshot,
    }
}

fn wire(
    arena_z: u32,
    source: (u32, u32),
    destination: (u32, u32),
    points: &[(f32, f32)],
) -> StoredWireEdge {
    StoredWireEdge {
        source_id: endpoint(source.0, source.1, arena_z),
        destination_id: endpoint(destination.0, destination.1, arena_z),
        points: points
            .iter()
            .map(|(x, y)| WirePoint { x: *x, y: *y })
            .collect(),
        color: DEFAULT_WIRE_COLOR,
    }
}

fn endpoint(x: u32, y: u32, arena_z: u32) -> WireEndpointId {
    WireEndpointId { x, y, arena_z }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_component_groups_cells_and_wires() {
        let component = starter_component();

        assert_eq!(component.arena_z, 0);
        assert_eq!(
            component.cell_at(1, 1, 0),
            CellSnapshot::gate(GateKind::And)
        );
        assert_eq!(
            component.cell_at(7, 2, 0),
            CellSnapshot::gate(GateKind::Xnor)
        );
        assert_eq!(
            component.cell_at(5, 2, 0),
            CellSnapshot::gate(GateKind::Not)
        );
        assert_eq!(
            component.cell_at(6, 1, 0),
            CellSnapshot::gate(GateKind::And)
        );
        assert_eq!(component.cell_at(7, 2, 1), CellSnapshot::empty());
        assert_eq!(component.wires.len(), 14);
        assert!(component
            .wires
            .iter()
            .all(|wire| wire.source_id.arena_z == 0));
    }
}
