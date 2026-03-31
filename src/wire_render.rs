use std::collections::HashMap;

use crate::wires::{GridCell, WirePoint};

const WIRE_DELETE_DISTANCE: f32 = 0.35;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WireBufferId {
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

pub struct WireRenderInfo {
    pub buffer_id: WireBufferId,
    wire_edges: HashMap<WireEdgeKey, Vec<StoredWireEdge>>,
}

impl WireRenderInfo {
    pub fn new(buffer_id: WireBufferId) -> Self {
        Self {
            buffer_id,
            wire_edges: HashMap::new(),
        }
    }

    pub fn set_buffer_id(&mut self, buffer_id: WireBufferId) {
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

    pub fn remove_matching_wire_edge(&mut self, edge: &StoredWireEdge) -> Option<StoredWireEdge> {
        let key = edge.key();
        let edges = self.wire_edges.get_mut(&key)?;
        let index = edges.iter().position(|candidate| candidate == edge)?;
        let removed = edges.remove(index);
        if edges.is_empty() {
            self.wire_edges.remove(&key);
        }
        Some(removed)
    }

    pub fn remove_wire_at_point(&mut self, layer: u32, point: WirePoint) -> Option<StoredWireEdge> {
        let (key, index) = self
            .wire_edges
            .iter()
            .filter(|(_, edges)| {
                edges
                    .first()
                    .is_some_and(|edge| edge.source_id.layer == layer)
            })
            .find_map(|(key, edges)| {
                edges
                    .iter()
                    .position(|edge| wire_contains_point(&edge.points, point, WIRE_DELETE_DISTANCE))
                    .map(|index| (*key, index))
            })?;

        let edges = self.wire_edges.get_mut(&key)?;
        let removed = edges.remove(index);
        if edges.is_empty() {
            self.wire_edges.remove(&key);
        }
        Some(removed)
    }

    pub fn wire_edges(&self) -> impl Iterator<Item = &StoredWireEdge> {
        self.wire_edges.values().flatten()
    }
}

fn wire_contains_point(points: &[WirePoint], point: WirePoint, max_distance: f32) -> bool {
    points
        .windows(2)
        .any(|segment| point_to_segment_distance(point, segment[0], segment[1]) <= max_distance)
}

fn point_to_segment_distance(point: WirePoint, start: WirePoint, end: WirePoint) -> f32 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= f32::EPSILON {
        return point_distance(point, start);
    }

    let t = ((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared;
    let t = t.clamp(0.0, 1.0);
    let projection = WirePoint {
        x: start.x + dx * t,
        y: start.y + dy * t,
    };
    point_distance(point, projection)
}

fn point_distance(first: WirePoint, second: WirePoint) -> f32 {
    let dx = second.x - first.x;
    let dy = second.y - first.y;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_edges_by_source_and_destination() {
        let mut component = WireRenderInfo::new(WireBufferId {
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
        let mut component = WireRenderInfo::new(WireBufferId {
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
        let mut component = WireRenderInfo::new(WireBufferId {
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

    #[test]
    fn removes_exact_matching_duplicate_edge() {
        let mut component = WireRenderInfo::new(WireBufferId {
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
        let first = StoredWireEdge {
            source_id,
            destination_id,
            points: vec![WirePoint { x: 2.0, y: 1.0 }, WirePoint { x: 3.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        };
        let second = StoredWireEdge {
            source_id,
            destination_id,
            points: vec![WirePoint { x: 2.0, y: 1.0 }, WirePoint { x: 4.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        };

        component.add_wire_edge(first.clone());
        component.add_wire_edge(second.clone());

        let removed = component.remove_matching_wire_edge(&first);

        assert_eq!(removed, Some(first));
        assert_eq!(component.wire_edges().next(), Some(&second));
    }

    #[test]
    fn removes_wire_when_click_hits_segment_on_same_layer() {
        let mut component = WireRenderInfo::new(WireBufferId {
            texture_index: 0,
            layer: 1,
        });
        component.add_wire_edge(StoredWireEdge {
            source_id: WireEndpointId {
                x: 1,
                y: 1,
                layer: 1,
            },
            destination_id: WireEndpointId {
                x: 4,
                y: 1,
                layer: 1,
            },
            points: vec![WirePoint { x: 1.0, y: 1.0 }, WirePoint { x: 4.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        });

        let removed = component.remove_wire_at_point(1, WirePoint { x: 2.5, y: 1.2 });

        assert!(removed.is_some());
        assert_eq!(component.wire_edges().count(), 0);
    }

    #[test]
    fn keeps_wire_when_click_hits_different_layer() {
        let mut component = WireRenderInfo::new(WireBufferId {
            texture_index: 0,
            layer: 1,
        });
        component.add_wire_edge(StoredWireEdge {
            source_id: WireEndpointId {
                x: 1,
                y: 1,
                layer: 1,
            },
            destination_id: WireEndpointId {
                x: 4,
                y: 1,
                layer: 1,
            },
            points: vec![WirePoint { x: 1.0, y: 1.0 }, WirePoint { x: 4.0, y: 1.0 }],
            color: [1.0, 1.0, 1.0, 1.0],
        });

        let removed = component.remove_wire_at_point(2, WirePoint { x: 2.5, y: 1.0 });

        assert!(removed.is_none());
        assert_eq!(component.wire_edges().count(), 1);
    }
}
