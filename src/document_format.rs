use std::fmt;

use foldhash::HashMap;

use crate::{
    editor::{ComponentDefId, EditableComponentDef, EditorDocument},
    gate_plans::{
        AncestorDepth, ChildId, ChildInputConnection, ChildPlacement, ComponentLayout,
        ComponentPlan, ComponentPort, Gate, GateId, GatePlacement, PlanId, PortId, PortLocation,
        SignalRef, WireEndpoint, WireLayout, WirePoint,
    },
};

const MAGIC: [u8; 4] = *b"CGDF";
const VERSION: u16 = 4;
const SECTION_PLANS: [u8; 4] = *b"PLNS";
const SECTION_COMPONENTS: [u8; 4] = *b"CMPS";
const SECTION_ROOT: [u8; 4] = *b"ROOT";

const SIGNAL_REF_SIZE: usize = 12;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentFormatError {
    InvalidMagic {
        found: [u8; 4],
    },
    UnsupportedVersion {
        version: u16,
    },
    UnexpectedEof {
        offset: usize,
        context: &'static str,
    },
    InvalidUtf8 {
        offset: usize,
        context: &'static str,
    },
    MissingSection {
        tag: [u8; 4],
    },
    DuplicateSection {
        tag: [u8; 4],
    },
    UnknownGateKind {
        offset: usize,
        kind: u16,
    },
    InvalidGatePayload {
        offset: usize,
        kind: u16,
        payload_len: u16,
    },
    InvalidSignalRefKind {
        offset: usize,
        kind: u8,
    },
    InvalidWireEndpointKind {
        offset: usize,
        kind: u8,
    },
    IntegerOutOfRange {
        context: &'static str,
        value: u64,
    },
    InvalidDocument(String),
}

impl fmt::Display for DocumentFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic { found } => write!(f, "invalid file magic: {found:?}"),
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported document format version {version}")
            }
            Self::UnexpectedEof { offset, context } => {
                write!(
                    f,
                    "unexpected end of file at byte {offset} while reading {context}"
                )
            }
            Self::InvalidUtf8 { offset, context } => {
                write!(f, "invalid UTF-8 at byte {offset} while reading {context}")
            }
            Self::MissingSection { tag } => {
                write!(
                    f,
                    "missing required section {:?}",
                    std::str::from_utf8(tag).unwrap_or("????")
                )
            }
            Self::DuplicateSection { tag } => {
                write!(
                    f,
                    "duplicate section {:?}",
                    std::str::from_utf8(tag).unwrap_or("????")
                )
            }
            Self::UnknownGateKind { offset, kind } => {
                write!(f, "unknown gate kind id {kind} at byte {offset}")
            }
            Self::InvalidGatePayload {
                offset,
                kind,
                payload_len,
            } => write!(
                f,
                "invalid gate payload length {payload_len} for gate kind {kind} at byte {offset}"
            ),
            Self::InvalidSignalRefKind { offset, kind } => {
                write!(f, "invalid signal reference kind {kind} at byte {offset}")
            }
            Self::InvalidWireEndpointKind { offset, kind } => {
                write!(f, "invalid wire endpoint kind {kind} at byte {offset}")
            }
            Self::IntegerOutOfRange { context, value } => {
                write!(f, "value {value} is out of range for {context}")
            }
            Self::InvalidDocument(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DocumentFormatError {}

pub fn encode_document(document: &EditorDocument) -> Result<Vec<u8>, DocumentFormatError> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    put_u16(&mut out, VERSION);
    put_u16(&mut out, 0);
    put_u32(&mut out, 3);

    write_section(&mut out, SECTION_PLANS, |section| {
        encode_plans(section, document)
    })?;
    write_section(&mut out, SECTION_COMPONENTS, |section| {
        encode_components(section, document)
    })?;
    write_section(&mut out, SECTION_ROOT, |section| {
        put_u32(section, as_u32(document.root.0, "root component id")?);
        Ok(())
    })?;

    Ok(out)
}

pub fn decode_document(bytes: &[u8]) -> Result<EditorDocument, DocumentFormatError> {
    let mut reader = Reader::new(bytes, 0);
    let magic = reader.read_array::<4>("file magic")?;
    if magic != MAGIC {
        return Err(DocumentFormatError::InvalidMagic { found: magic });
    }

    let version = reader.read_u16("file version")?;
    if !(1..=VERSION).contains(&version) {
        return Err(DocumentFormatError::UnsupportedVersion { version });
    }
    let _reserved = reader.read_u16("reserved header field")?;
    let section_count = reader.read_u32("section count")?;

    let mut plans_section = None;
    let mut components_section = None;
    let mut root_section = None;

    for _ in 0..section_count {
        let tag = reader.read_array::<4>("section tag")?;
        let section_len = reader.read_u32("section length")? as usize;
        let section_offset = reader.pos;
        let section_bytes = reader.read_bytes(section_len, "section payload")?;
        let section_reader = Reader::new(section_bytes, section_offset);
        match tag {
            SECTION_PLANS => set_section(&mut plans_section, tag, section_reader)?,
            SECTION_COMPONENTS => set_section(&mut components_section, tag, section_reader)?,
            SECTION_ROOT => set_section(&mut root_section, tag, section_reader)?,
            _ => {}
        }
    }

    let plans = decode_plans(
        plans_section.ok_or(DocumentFormatError::MissingSection { tag: SECTION_PLANS })?,
        version,
    )?;
    let components = decode_components(
        components_section.ok_or(DocumentFormatError::MissingSection {
            tag: SECTION_COMPONENTS,
        })?,
        version,
    )?;
    let root = decode_root(
        root_section.ok_or(DocumentFormatError::MissingSection { tag: SECTION_ROOT })?,
    )?;

    EditorDocument::new(plans, components, root).map_err(DocumentFormatError::InvalidDocument)
}

fn encode_plans(out: &mut Vec<u8>, document: &EditorDocument) -> Result<(), DocumentFormatError> {
    let mut plans: Vec<_> = document.plans.iter().collect();
    plans.sort_by_key(|(id, _)| id.0);
    put_u32(out, as_u32(plans.len(), "plan count")?);
    for (id, plan) in plans {
        put_u32(out, as_u32(id.0, "plan id")?);
        put_u32(out, plan.grid_size[0]);
        put_u32(out, plan.grid_size[1]);
        let gates = plan.ordered_gates();
        put_u32(out, as_u32(gates.len(), "gate count")?);
        for (gate_id, gate) in gates {
            put_u32(out, gate_id.0);
            encode_gate(out, gate);
        }
        encode_ports(out, &plan.inputs)?;
        encode_ports(out, &plan.outputs)?;
    }
    Ok(())
}

fn encode_components(
    out: &mut Vec<u8>,
    document: &EditorDocument,
) -> Result<(), DocumentFormatError> {
    put_u32(out, as_u32(document.components.len(), "component count")?);
    for component in &document.components {
        put_u32(out, as_u32(component.plan.0, "component plan id")?);
        put_u32(
            out,
            as_u32(component.children.len(), "component child count")?,
        );
        for child in &component.children {
            put_u32(out, as_u32(child.0, "child component id")?);
        }

        put_u32(
            out,
            as_u32(
                component.child_input_connections.len(),
                "child input connection count",
            )?,
        );
        for connection in &component.child_input_connections {
            put_u32(out, connection.child.0);
            put_u32(out, connection.input.0);
            encode_signal_ref(out, connection.src);
        }

        put_u32(
            out,
            as_u32(
                component.layout.gate_placements.len(),
                "gate placement count",
            )?,
        );
        for placement in &component.layout.gate_placements {
            put_u32(out, placement.gate.0);
            put_u32(out, placement.min[0]);
            put_u32(out, placement.min[1]);
        }

        put_u32(
            out,
            as_u32(
                component.layout.child_placements.len(),
                "child placement count",
            )?,
        );
        for placement in &component.layout.child_placements {
            put_u32(out, placement.min[0]);
            put_u32(out, placement.min[1]);
        }

        put_u32(out, as_u32(component.layout.wires.len(), "wire count")?);
        for wire in &component.layout.wires {
            encode_wire_endpoint(out, wire.from);
            encode_wire_endpoint(out, wire.to);
            put_u32(out, as_u32(wire.bends.len(), "wire bend count")?);
            for bend in &wire.bends {
                put_i32(out, bend.x);
                put_i32(out, bend.y);
            }
        }
    }
    Ok(())
}

fn decode_plans(
    mut reader: Reader<'_>,
    version: u16,
) -> Result<HashMap<PlanId, ComponentPlan>, DocumentFormatError> {
    let plan_count = reader.read_u32("plan count")?;
    let mut plans = HashMap::default();
    for _ in 0..plan_count {
        let plan_id = PlanId(reader.read_u32("plan id")? as usize);
        let grid_size = [
            reader.read_u32("plan grid width")?,
            reader.read_u32("plan grid height")?,
        ];
        let gate_count = reader.read_u32("gate count")?;
        let mut gates = HashMap::default();
        for gate_index in 0..gate_count {
            let gate_id = if version >= 2 {
                GateId(reader.read_u32("gate id")?)
            } else {
                GateId(gate_index)
            };
            gates.insert(gate_id, decode_gate(&mut reader)?);
        }
        let inputs = decode_ports(&mut reader)?;
        let outputs = decode_ports(&mut reader)?;
        plans.insert(
            plan_id,
            ComponentPlan::with_ports(Vec::new(), inputs, outputs)
                .with_gate_map(gates)
                .with_grid_size(grid_size),
        );
    }
    Ok(plans)
}

fn decode_components(
    mut reader: Reader<'_>,
    version: u16,
) -> Result<Vec<EditableComponentDef>, DocumentFormatError> {
    let component_count = reader.read_u32("component count")?;
    let mut components = Vec::with_capacity(component_count as usize);
    for _ in 0..component_count {
        let plan = PlanId(reader.read_u32("component plan id")? as usize);

        let child_count = reader.read_u32("component child count")?;
        let mut children = Vec::with_capacity(child_count as usize);
        for _ in 0..child_count {
            children.push(ComponentDefId(
                reader.read_u32("child component id")? as usize
            ));
        }

        let connection_count = reader.read_u32("child input connection count")?;
        let mut child_input_connections = Vec::with_capacity(connection_count as usize);
        for _ in 0..connection_count {
            child_input_connections.push(ChildInputConnection {
                child: ChildId(reader.read_u32("child connection child id")?),
                input: PortId(reader.read_u32("child connection input port id")?),
                src: decode_signal_ref(&mut reader)?,
            });
        }

        let mut gate_placements = Vec::new();
        if version >= 3 {
            let placement_count = reader.read_u32("gate placement count")?;
            gate_placements = Vec::with_capacity(placement_count as usize);
            for _ in 0..placement_count {
                gate_placements.push(GatePlacement::at(
                    GateId(reader.read_u32("gate placement gate id")?),
                    [
                        reader.read_u32("gate placement x")?,
                        reader.read_u32("gate placement y")?,
                    ],
                ));
            }
        }

        let placement_count = reader.read_u32("child placement count")?;
        let mut child_placements = Vec::with_capacity(placement_count as usize);
        for _ in 0..placement_count {
            child_placements.push(ChildPlacement::at([
                reader.read_u32("child placement x")?,
                reader.read_u32("child placement y")?,
            ]));
        }

        let wire_count = reader.read_u32("wire count")?;
        let mut wires = Vec::with_capacity(wire_count as usize);
        for _ in 0..wire_count {
            let from = decode_wire_endpoint(&mut reader)?;
            let to = decode_wire_endpoint(&mut reader)?;
            let bend_count = reader.read_u32("wire bend count")?;
            let mut bends = Vec::with_capacity(bend_count as usize);
            for _ in 0..bend_count {
                bends.push(WirePoint {
                    x: reader.read_i32("wire bend x")?,
                    y: reader.read_i32("wire bend y")?,
                });
            }
            wires.push(WireLayout { from, to, bends });
        }

        components.push(EditableComponentDef {
            plan,
            children,
            child_input_connections,
            dangling_wires: Vec::new(),
            layout: ComponentLayout {
                gate_placements,
                child_placements,
                wires,
            },
        });
    }
    Ok(components)
}

fn decode_root(mut reader: Reader<'_>) -> Result<ComponentDefId, DocumentFormatError> {
    Ok(ComponentDefId(
        reader.read_u32("root component id")? as usize
    ))
}

fn encode_gate(out: &mut Vec<u8>, gate: Gate) {
    let (kind, refs) = match gate {
        Gate::BitNAND { a, b } => (1u16, vec![a, b]),
        Gate::BitAND { a, b } => (2u16, vec![a, b]),
        Gate::BitOR { a, b } => (3u16, vec![a, b]),
        Gate::BitNOR { a, b } => (4u16, vec![a, b]),
        Gate::BitXOR { a, b } => (5u16, vec![a, b]),
        Gate::BitXNOR { a, b } => (6u16, vec![a, b]),
        Gate::BitNot { src } => (7u16, vec![src]),
        Gate::BitNop { src } => (8u16, vec![src]),
    };
    put_u16(out, kind);
    put_u16(out, (refs.len() * SIGNAL_REF_SIZE) as u16);
    for signal in refs {
        encode_signal_ref(out, signal);
    }
}

fn decode_gate(reader: &mut Reader<'_>) -> Result<Gate, DocumentFormatError> {
    let gate_offset = reader.absolute_offset();
    let kind = reader.read_u16("gate kind")?;
    let payload_len = reader.read_u16("gate payload length")?;
    let payload = reader.read_subreader(payload_len as usize, "gate payload")?;
    let mut payload = payload;
    match kind {
        1..=6 if payload_len != (SIGNAL_REF_SIZE as u16 * 2) => {
            Err(DocumentFormatError::InvalidGatePayload {
                offset: gate_offset,
                kind,
                payload_len,
            })
        }
        7 | 8 if payload_len != SIGNAL_REF_SIZE as u16 => {
            Err(DocumentFormatError::InvalidGatePayload {
                offset: gate_offset,
                kind,
                payload_len,
            })
        }
        1 => Ok(Gate::BitNAND {
            a: decode_signal_ref(&mut payload)?,
            b: decode_signal_ref(&mut payload)?,
        }),
        2 => Ok(Gate::BitAND {
            a: decode_signal_ref(&mut payload)?,
            b: decode_signal_ref(&mut payload)?,
        }),
        3 => Ok(Gate::BitOR {
            a: decode_signal_ref(&mut payload)?,
            b: decode_signal_ref(&mut payload)?,
        }),
        4 => Ok(Gate::BitNOR {
            a: decode_signal_ref(&mut payload)?,
            b: decode_signal_ref(&mut payload)?,
        }),
        5 => Ok(Gate::BitXOR {
            a: decode_signal_ref(&mut payload)?,
            b: decode_signal_ref(&mut payload)?,
        }),
        6 => Ok(Gate::BitXNOR {
            a: decode_signal_ref(&mut payload)?,
            b: decode_signal_ref(&mut payload)?,
        }),
        7 => Ok(Gate::BitNot {
            src: decode_signal_ref(&mut payload)?,
        }),
        8 => Ok(Gate::BitNop {
            src: decode_signal_ref(&mut payload)?,
        }),
        _ => Err(DocumentFormatError::UnknownGateKind {
            offset: gate_offset,
            kind,
        }),
    }
}

fn encode_ports(out: &mut Vec<u8>, ports: &[ComponentPort]) -> Result<(), DocumentFormatError> {
    put_u32(out, as_u32(ports.len(), "port count")?);
    for port in ports {
        put_u32(out, port.id.0);
        put_u32(out, port.gate.0);
        put_u16(out, port.location.x);
        put_u16(out, port.location.y);
        put_string(out, port.label.as_deref().unwrap_or(""))?;
    }
    Ok(())
}

fn decode_ports(reader: &mut Reader<'_>) -> Result<Vec<ComponentPort>, DocumentFormatError> {
    let count = reader.read_u32("port count")?;
    let mut ports = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = PortId(reader.read_u32("port id")?);
        let gate = GateId(reader.read_u32("port gate id")?);
        let x = reader.read_u16("port x")?;
        let y = reader.read_u16("port y")?;
        let label = reader.read_string("port label")?;
        ports.push(ComponentPort {
            id,
            gate,
            location: PortLocation { x, y },
            label: (!label.is_empty()).then_some(label),
        });
    }
    Ok(ports)
}

fn encode_signal_ref(out: &mut Vec<u8>, signal: SignalRef) {
    match signal {
        SignalRef::Disconnected => {
            put_u8(out, 0);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, 0);
            put_u32(out, 0);
        }
        SignalRef::ThisGate(gate) => {
            put_u8(out, 1);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, gate.0);
            put_u32(out, 0);
        }
        SignalRef::InputPort(port) => {
            put_u8(out, 2);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, port.0);
            put_u32(out, 0);
        }
        SignalRef::ChildOutput { child, port } => {
            put_u8(out, 3);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, child.0);
            put_u32(out, port.0);
        }
        SignalRef::AncestorOutput { depth, port } => {
            put_u8(out, 4);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, depth.0);
            put_u32(out, port.0);
        }
    }
}

fn decode_signal_ref(reader: &mut Reader<'_>) -> Result<SignalRef, DocumentFormatError> {
    let offset = reader.absolute_offset();
    let kind = reader.read_u8("signal reference kind")?;
    let _reserved = reader.read_u8("signal reference reserved byte")?;
    let _reserved = reader.read_u16("signal reference reserved word")?;
    let a = reader.read_u32("signal reference field a")?;
    let b = reader.read_u32("signal reference field b")?;
    match kind {
        0 => Ok(SignalRef::Disconnected),
        1 => Ok(SignalRef::ThisGate(GateId(a))),
        2 => Ok(SignalRef::InputPort(PortId(a))),
        3 => Ok(SignalRef::ChildOutput {
            child: ChildId(a),
            port: PortId(b),
        }),
        4 => Ok(SignalRef::AncestorOutput {
            depth: AncestorDepth(a),
            port: PortId(b),
        }),
        _ => Err(DocumentFormatError::InvalidSignalRefKind { offset, kind }),
    }
}

fn encode_wire_endpoint(out: &mut Vec<u8>, endpoint: WireEndpoint) {
    match endpoint {
        WireEndpoint::GateOutput(gate) => {
            put_u8(out, 1);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, gate.0);
            put_u32(out, 0);
        }
        WireEndpoint::GateInput { gate, input } => {
            put_u8(out, 2);
            put_u8(out, input);
            put_u16(out, 0);
            put_u32(out, gate.0);
            put_u32(out, 0);
        }
        WireEndpoint::ComponentInput(port) => {
            put_u8(out, 3);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, port.0);
            put_u32(out, 0);
        }
        WireEndpoint::ComponentOutput(port) => {
            put_u8(out, 4);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, port.0);
            put_u32(out, 0);
        }
        WireEndpoint::ChildOutput { child, port } => {
            put_u8(out, 5);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, child.0);
            put_u32(out, port.0);
        }
        WireEndpoint::ChildInput { child, port } => {
            put_u8(out, 6);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, child.0);
            put_u32(out, port.0);
        }
        WireEndpoint::AncestorOutput { depth, port } => {
            put_u8(out, 7);
            put_u8(out, 0);
            put_u16(out, 0);
            put_u32(out, depth.0);
            put_u32(out, port.0);
        }
    }
}

fn decode_wire_endpoint(reader: &mut Reader<'_>) -> Result<WireEndpoint, DocumentFormatError> {
    let offset = reader.absolute_offset();
    let kind = reader.read_u8("wire endpoint kind")?;
    let aux = reader.read_u8("wire endpoint aux")?;
    let _reserved = reader.read_u16("wire endpoint reserved word")?;
    let a = reader.read_u32("wire endpoint field a")?;
    let b = reader.read_u32("wire endpoint field b")?;
    match kind {
        1 => Ok(WireEndpoint::GateOutput(GateId(a))),
        2 => Ok(WireEndpoint::GateInput {
            gate: GateId(a),
            input: aux,
        }),
        3 => Ok(WireEndpoint::ComponentInput(PortId(a))),
        4 => Ok(WireEndpoint::ComponentOutput(PortId(a))),
        5 => Ok(WireEndpoint::ChildOutput {
            child: ChildId(a),
            port: PortId(b),
        }),
        6 => Ok(WireEndpoint::ChildInput {
            child: ChildId(a),
            port: PortId(b),
        }),
        7 => Ok(WireEndpoint::AncestorOutput {
            depth: AncestorDepth(a),
            port: PortId(b),
        }),
        _ => Err(DocumentFormatError::InvalidWireEndpointKind { offset, kind }),
    }
}

fn write_section(
    out: &mut Vec<u8>,
    tag: [u8; 4],
    write: impl FnOnce(&mut Vec<u8>) -> Result<(), DocumentFormatError>,
) -> Result<(), DocumentFormatError> {
    let mut section = Vec::new();
    write(&mut section)?;
    out.extend_from_slice(&tag);
    put_u32(out, as_u32(section.len(), "section length")?);
    out.extend_from_slice(&section);
    Ok(())
}

fn set_section<'a>(
    slot: &mut Option<Reader<'a>>,
    tag: [u8; 4],
    reader: Reader<'a>,
) -> Result<(), DocumentFormatError> {
    if slot.is_some() {
        return Err(DocumentFormatError::DuplicateSection { tag });
    }
    *slot = Some(reader);
    Ok(())
}

fn put_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), DocumentFormatError> {
    put_u32(out, as_u32(value.len(), "string length")?);
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn as_u32(value: usize, context: &'static str) -> Result<u32, DocumentFormatError> {
    u32::try_from(value).map_err(|_| DocumentFormatError::IntegerOutOfRange {
        context,
        value: value as u64,
    })
}

#[derive(Clone, Copy)]
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    base: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], base: usize) -> Self {
        Self {
            bytes,
            pos: 0,
            base,
        }
    }

    fn read_array<const N: usize>(
        &mut self,
        context: &'static str,
    ) -> Result<[u8; N], DocumentFormatError> {
        let bytes = self.read_bytes(N, context)?;
        let mut out = [0; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn read_u8(&mut self, context: &'static str) -> Result<u8, DocumentFormatError> {
        Ok(self.read_bytes(1, context)?[0])
    }

    fn read_u16(&mut self, context: &'static str) -> Result<u16, DocumentFormatError> {
        let bytes = self.read_array::<2>(context)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u32(&mut self, context: &'static str) -> Result<u32, DocumentFormatError> {
        let bytes = self.read_array::<4>(context)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_i32(&mut self, context: &'static str) -> Result<i32, DocumentFormatError> {
        let bytes = self.read_array::<4>(context)?;
        Ok(i32::from_le_bytes(bytes))
    }

    fn read_string(&mut self, context: &'static str) -> Result<String, DocumentFormatError> {
        let len = self.read_u32(context)? as usize;
        let offset = self.absolute_offset();
        let bytes = self.read_bytes(len, context)?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_owned())
            .map_err(|_| DocumentFormatError::InvalidUtf8 { offset, context })
    }

    fn read_subreader(
        &mut self,
        len: usize,
        context: &'static str,
    ) -> Result<Reader<'a>, DocumentFormatError> {
        let offset = self.absolute_offset();
        let bytes = self.read_bytes(len, context)?;
        Ok(Reader::new(bytes, offset))
    }

    fn read_bytes(
        &mut self,
        len: usize,
        context: &'static str,
    ) -> Result<&'a [u8], DocumentFormatError> {
        let end = self.pos.saturating_add(len);
        if end > self.bytes.len() {
            return Err(DocumentFormatError::UnexpectedEof {
                offset: self.absolute_offset(),
                context,
            });
        }
        let bytes = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn absolute_offset(&self) -> usize {
        self.base + self.pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    const INPUT_A: PortId = PortId(10);
    const INPUT_B: PortId = PortId(11);
    const OUTPUT_Y: PortId = PortId(20);

    fn this_ref(gate: u32) -> SignalRef {
        SignalRef::ThisGate(GateId(gate))
    }

    fn child_output_ref(child: u32, port: PortId) -> SignalRef {
        SignalRef::ChildOutput {
            child: ChildId(child),
            port,
        }
    }

    fn input_ref(port: PortId) -> SignalRef {
        SignalRef::InputPort(port)
    }

    fn port(id: PortId, gate: u32, x: u16, y: u16, label: &str) -> ComponentPort {
        ComponentPort {
            id,
            gate: GateId(gate),
            location: PortLocation { x, y },
            label: Some(label.to_owned()),
        }
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
                        src: input_ref(INPUT_A),
                    },
                    Gate::BitNot {
                        src: input_ref(INPUT_B),
                    },
                ],
                vec![port(INPUT_A, 0, 0, 1, "a"), port(INPUT_B, 1, 0, 2, "b")],
                vec![port(OUTPUT_Y, 1, u16::MAX, 1, "y")],
            )
            .with_grid_size([2, 2]),
        );
        plans.insert(
            root_plan,
            ComponentPlan::with_ports(
                vec![
                    Gate::BitNop { src: this_ref(0) },
                    Gate::BitXOR {
                        a: this_ref(0),
                        b: child_output_ref(0, OUTPUT_Y),
                    },
                ],
                vec![],
                vec![port(OUTPUT_Y, 1, u16::MAX, 1, "sum")],
            )
            .with_grid_size([4, 3]),
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
                            src: this_ref(0),
                        },
                        ChildInputConnection {
                            child: ChildId(0),
                            input: INPUT_B,
                            src: this_ref(1),
                        },
                    ],
                    dangling_wires: Vec::new(),
                    layout: ComponentLayout::default()
                        .with_child_placements(vec![ChildPlacement::at([1, 1])]),
                },
            ],
            ComponentDefId(1),
        )
        .expect("sample document should build")
    }

    #[test]
    fn document_roundtrip_preserves_binary_encoding() {
        let document = sample_document();
        let encoded = encode_document(&document).expect("document should encode");
        let decoded = decode_document(&encoded).expect("document should decode");
        let reencoded = encode_document(&decoded).expect("decoded document should re-encode");

        assert_eq!(encoded, reencoded);
        assert_eq!(document.root, decoded.root);
        assert_eq!(document.components.len(), decoded.components.len());
        assert_eq!(document.plans.len(), decoded.plans.len());
    }

    #[test]
    fn legacy_v1_fixture_decodes_and_reencodes_as_v2() {
        let fixture = include_bytes!("../fixtures/sample_document_v1.cgdf");

        let decoded = decode_document(fixture).expect("fixture should decode");
        let reencoded = encode_document(&decoded).expect("fixture should re-encode");

        assert_eq!(&reencoded[0..4], &MAGIC);
        assert_eq!(u16::from_le_bytes([reencoded[4], reencoded[5]]), VERSION);
        assert_eq!(decoded.root, ComponentDefId(1));
        assert_eq!(decoded.components.len(), 2);
        assert_eq!(decoded.plans.len(), 2);
    }

    #[test]
    fn python_codec_reads_legacy_v1_fixture_and_writes_v2() {
        let fixture = include_bytes!("../fixtures/sample_document_v1.cgdf");
        let temp = TestPaths::new("fixture_roundtrip");
        fs::write(&temp.input, fixture).expect("fixture input should be written");

        run_python_codec(&temp.input, &temp.output);

        let python_encoded = fs::read(&temp.output).expect("python output should exist");
        assert_eq!(&python_encoded[0..4], &MAGIC);
        assert_eq!(
            u16::from_le_bytes([python_encoded[4], python_encoded[5]]),
            VERSION
        );

        let rust_decoded =
            decode_document(&python_encoded).expect("rust decoder should accept python output");
        let rust_reencoded =
            encode_document(&rust_decoded).expect("rust encoder should re-encode python output");
        assert_eq!(rust_reencoded, python_encoded);
    }

    #[test]
    fn python_codec_roundtrips_rust_encoded_document() {
        let rust_encoded = encode_document(&sample_document()).expect("document should encode");
        let temp = TestPaths::new("rust_roundtrip");
        fs::write(&temp.input, &rust_encoded).expect("rust input should be written");

        run_python_codec(&temp.input, &temp.output);

        let python_encoded = fs::read(&temp.output).expect("python output should exist");
        let python_decoded =
            decode_document(&python_encoded).expect("rust decoder should accept python output");
        let python_reencoded =
            encode_document(&python_decoded).expect("rust encoder should re-encode python output");

        assert_eq!(python_encoded, rust_encoded);
        assert_eq!(python_reencoded, rust_encoded);
        assert_eq!(python_decoded.root, ComponentDefId(1));
        assert_eq!(python_decoded.components.len(), 2);
        assert_eq!(python_decoded.plans.len(), 2);
    }

    #[test]
    fn unknown_gate_kind_reports_exact_offset() {
        let mut encoded = encode_document(&sample_document()).expect("document should encode");
        let plans_offset = encoded
            .windows(4)
            .position(|window| window == SECTION_PLANS)
            .expect("plans section should exist");
        let gate_kind_offset = plans_offset + 8 + 4 + 4 + 4 + 4 + 4 + 4;
        encoded[gate_kind_offset] = 99;
        encoded[gate_kind_offset + 1] = 0;

        let error = decode_document(&encoded).expect_err("decode should fail for unknown gate id");
        assert_eq!(
            error,
            DocumentFormatError::UnknownGateKind {
                offset: gate_kind_offset,
                kind: 99,
            }
        );
    }

    #[test]
    fn invalid_child_placement_reports_invalid_document_instead_of_panicking() {
        let mut encoded = encode_document(&sample_document()).expect("document should encode");
        let placement_x_offset = second_component_first_child_placement_x_offset(&encoded);
        encoded[placement_x_offset..placement_x_offset + 4]
            .copy_from_slice(&u32::MAX.to_le_bytes());

        let error =
            decode_document(&encoded).expect_err("decode should fail for invalid placement");
        assert!(matches!(error, DocumentFormatError::InvalidDocument(_)));
    }

    fn second_component_first_child_placement_x_offset(encoded: &[u8]) -> usize {
        let components_offset = encoded
            .windows(4)
            .position(|window| window == SECTION_COMPONENTS)
            .expect("components section should exist");
        let section_len = u32::from_le_bytes(
            encoded[components_offset + 4..components_offset + 8]
                .try_into()
                .expect("section length bytes should exist"),
        ) as usize;
        let section_offset = components_offset + 8;
        let section_reader = Reader::new(
            &encoded[section_offset..section_offset + section_len],
            section_offset,
        );
        let mut reader = section_reader;
        let component_count = reader
            .read_u32("component count")
            .expect("component count should decode");
        assert!(
            component_count >= 2,
            "sample document should have two components"
        );

        skip_component(&mut reader);

        reader
            .read_u32("component plan id")
            .expect("component plan id should decode");
        let child_count = reader
            .read_u32("component child count")
            .expect("child count should decode");
        for _ in 0..child_count {
            reader
                .read_u32("child component id")
                .expect("child component id should decode");
        }
        let connection_count = reader
            .read_u32("child input connection count")
            .expect("connection count should decode");
        for _ in 0..connection_count {
            reader
                .read_u32("child connection child id")
                .expect("child connection child id should decode");
            reader
                .read_u32("child connection input port id")
                .expect("child connection input port id should decode");
            reader
                .read_bytes(SIGNAL_REF_SIZE, "child connection signal ref")
                .expect("child connection signal ref should decode");
        }
        skip_gate_placements(&mut reader);
        let placement_count = reader
            .read_u32("child placement count")
            .expect("placement count should decode");
        assert!(
            placement_count >= 1,
            "sample document should have a child placement"
        );
        let offset = reader.absolute_offset();
        offset
    }

    fn skip_component(reader: &mut Reader<'_>) {
        reader
            .read_u32("component plan id")
            .expect("component plan id should decode");
        let child_count = reader
            .read_u32("component child count")
            .expect("child count should decode");
        for _ in 0..child_count {
            reader
                .read_u32("child component id")
                .expect("child component id should decode");
        }
        let connection_count = reader
            .read_u32("child input connection count")
            .expect("connection count should decode");
        for _ in 0..connection_count {
            reader
                .read_u32("child connection child id")
                .expect("child connection child id should decode");
            reader
                .read_u32("child connection input port id")
                .expect("child connection input port id should decode");
            reader
                .read_bytes(SIGNAL_REF_SIZE, "child connection signal ref")
                .expect("child connection signal ref should decode");
        }
        skip_gate_placements(reader);
        let placement_count = reader
            .read_u32("child placement count")
            .expect("placement count should decode");
        for _ in 0..placement_count {
            reader
                .read_u32("child placement x")
                .expect("child placement x should decode");
            reader
                .read_u32("child placement y")
                .expect("child placement y should decode");
        }
        let wire_count = reader
            .read_u32("wire count")
            .expect("wire count should decode");
        for _ in 0..wire_count {
            skip_wire_endpoint(reader);
            skip_wire_endpoint(reader);
            let bend_count = reader
                .read_u32("wire bend count")
                .expect("wire bend count should decode");
            for _ in 0..bend_count {
                reader
                    .read_i32("wire bend x")
                    .expect("wire bend x should decode");
                reader
                    .read_i32("wire bend y")
                    .expect("wire bend y should decode");
            }
        }
    }

    fn skip_gate_placements(reader: &mut Reader<'_>) {
        let placement_count = reader
            .read_u32("gate placement count")
            .expect("gate placement count should decode");
        for _ in 0..placement_count {
            reader
                .read_u32("gate placement gate id")
                .expect("gate placement gate id should decode");
            reader
                .read_u32("gate placement x")
                .expect("gate placement x should decode");
            reader
                .read_u32("gate placement y")
                .expect("gate placement y should decode");
        }
    }

    fn skip_wire_endpoint(reader: &mut Reader<'_>) {
        reader
            .read_u8("wire endpoint kind")
            .expect("wire endpoint kind should decode");
        reader
            .read_u8("wire endpoint aux")
            .expect("wire endpoint aux should decode");
        reader
            .read_u16("wire endpoint reserved word")
            .expect("wire endpoint reserved word should decode");
        reader
            .read_u32("wire endpoint field a")
            .expect("wire endpoint field a should decode");
        reader
            .read_u32("wire endpoint field b")
            .expect("wire endpoint field b should decode");
    }

    struct TestPaths {
        input: PathBuf,
        output: PathBuf,
    }

    impl TestPaths {
        fn new(label: &str) -> Self {
            let unique = format!(
                "{}_{}_{}",
                label,
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time should be after epoch")
                    .as_nanos()
            );
            let base = env::temp_dir().join(format!("circuits_game_{unique}"));
            fs::create_dir_all(&base).expect("test temp dir should be created");
            Self {
                input: base.join("input.cgdf"),
                output: base.join("output.cgdf"),
            }
        }
    }

    impl Drop for TestPaths {
        fn drop(&mut self) {
            if let Some(dir) = self.input.parent() {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    fn run_python_codec(input: &Path, output: &Path) {
        let output_result = Command::new("python3")
            .arg("examples/document_codec.py")
            .arg(input)
            .arg(output)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("python3 should be available to run the codec example");
        assert!(
            output_result.status.success(),
            "python codec command should succeed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output_result.stdout),
            String::from_utf8_lossy(&output_result.stderr),
        );
    }
}
