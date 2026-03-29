use std::collections::HashMap;

use crate::wires::{GridCell, WirePoint};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ComponentBufferId {
    pub texture_index: u32,
    pub layer: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WireEndpointId {
    pub x: u32,
    pub y: u32,
    pub layer: u32,
}

impl WireEndpointId {
    pub fn from_grid_cell(cell: GridCell, layer: u32) -> Self {
        Self {
            x: cell.x,
            y: cell.y,
            layer,
        }
    }

    pub fn as_grid_cell(self) -> GridCell {
        GridCell {
            x: self.x,
            y: self.y,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WireEdgeKey {
    pub source_id: WireEndpointId,
    pub destination_id: WireEndpointId,
}

impl WireEdgeKey {
    pub fn new(source_id: WireEndpointId, destination_id: WireEndpointId) -> Self {
        Self {
            source_id,
            destination_id,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct StoredWireEdge {
    pub source_id: WireEndpointId,
    pub destination_id: WireEndpointId,
    pub points: Vec<WirePoint>,
    pub color: [f32; 4],
}

impl StoredWireEdge {
    pub fn key(&self) -> WireEdgeKey {
        WireEdgeKey::new(self.source_id, self.destination_id)
    }
}

pub struct ComponentInfo {
    pub buffer_id: ComponentBufferId,
    wire_edges: HashMap<WireEdgeKey, Vec<StoredWireEdge>>,
}

impl ComponentInfo {
    pub fn new(buffer_id: ComponentBufferId) -> Self {
        Self {
            buffer_id,
            wire_edges: HashMap::new(),
        }
    }

    pub fn set_buffer_id(&mut self, buffer_id: ComponentBufferId) {
        self.buffer_id = buffer_id;
    }

    pub fn add_wire_edge(&mut self, edge: StoredWireEdge) {
        self.wire_edges.entry(edge.key()).or_default().push(edge);
    }

    pub fn remove_wire_edge(
        &mut self,
        source_id: WireEndpointId,
        destination_id: WireEndpointId,
    ) -> Option<StoredWireEdge> {
        let key = WireEdgeKey::new(source_id, destination_id);
        let edges = self.wire_edges.get_mut(&key)?;
        let removed = edges.pop();
        if edges.is_empty() {
            self.wire_edges.remove(&key);
        }
        removed
    }

    pub fn remove_wire_edge_between(
        &mut self,
        first_id: WireEndpointId,
        second_id: WireEndpointId,
    ) -> Option<StoredWireEdge> {
        self.remove_wire_edge(first_id, second_id)
            .or_else(|| self.remove_wire_edge(second_id, first_id))
    }

    pub fn wire_edges(&self) -> impl Iterator<Item = &StoredWireEdge> {
        self.wire_edges.values().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_edges_by_source_and_destination() {
        let mut component = ComponentInfo::new(ComponentBufferId {
            texture_index: 0,
            layer: 1,
        });
        let edge = StoredWireEdge {
            source_id: WireEndpointId {
                x: 1,
                y: 2,
                layer: 1,
            },
            destination_id: WireEndpointId {
                x: 3,
                y: 4,
                layer: 1,
            },
            points: vec![WirePoint { x: 1.0, y: 2.0 }, WirePoint { x: 3.0, y: 4.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        };

        component.add_wire_edge(edge.clone());

        assert_eq!(component.wire_edges().count(), 1);
        assert_eq!(component.wire_edges().next(), Some(&edge));
    }

    #[test]
    fn removes_edges_in_either_direction() {
        let mut component = ComponentInfo::new(ComponentBufferId {
            texture_index: 0,
            layer: 2,
        });
        let source_id = WireEndpointId {
            x: 2,
            y: 1,
            layer: 2,
        };
        let destination_id = WireEndpointId {
            x: 4,
            y: 1,
            layer: 2,
        };
        component.add_wire_edge(StoredWireEdge {
            source_id,
            destination_id,
            points: vec![WirePoint { x: 2.0, y: 1.0 }, WirePoint { x: 4.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        });

        let removed = component.remove_wire_edge_between(destination_id, source_id);

        assert!(removed.is_some());
        assert_eq!(component.wire_edges().count(), 0);
    }

    #[test]
    fn keeps_duplicate_edges_with_same_endpoints() {
        let mut component = ComponentInfo::new(ComponentBufferId {
            texture_index: 0,
            layer: 2,
        });
        let source_id = WireEndpointId {
            x: 2,
            y: 1,
            layer: 2,
        };
        let destination_id = WireEndpointId {
            x: 4,
            y: 1,
            layer: 2,
        };

        component.add_wire_edge(StoredWireEdge {
            source_id,
            destination_id,
            points: vec![WirePoint { x: 2.0, y: 1.0 }, WirePoint { x: 3.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        });
        component.add_wire_edge(StoredWireEdge {
            source_id,
            destination_id,
            points: vec![WirePoint { x: 2.0, y: 1.0 }, WirePoint { x: 4.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        });

        assert_eq!(component.wire_edges().count(), 2);

        let removed = component.remove_wire_edge(source_id, destination_id);

        assert!(removed.is_some());
        assert_eq!(component.wire_edges().count(), 1);
    }
}
