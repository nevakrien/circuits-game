#!/usr/bin/env python3

import struct
import sys
from dataclasses import dataclass


MAGIC = b"CGDF"
VERSION = 3


class DecodeError(Exception):
    pass


class EncodeError(Exception):
    pass


class Reader:
    def __init__(self, data: bytes, base: int = 0):
        self.data = data
        self.pos = 0
        self.base = base

    def offset(self) -> int:
        return self.base + self.pos

    def take(self, size: int, context: str) -> bytes:
        end = self.pos + size
        if end > len(self.data):
            raise DecodeError(f"unexpected end of file at byte {self.offset()} while reading {context}")
        out = self.data[self.pos:end]
        self.pos = end
        return out

    def u8(self, context: str) -> int:
        return self.take(1, context)[0]

    def u16(self, context: str) -> int:
        return struct.unpack("<H", self.take(2, context))[0]

    def u32(self, context: str) -> int:
        return struct.unpack("<I", self.take(4, context))[0]

    def i32(self, context: str) -> int:
        return struct.unpack("<i", self.take(4, context))[0]

    def text(self, context: str) -> str:
        size = self.u32(f"{context} length")
        raw = self.take(size, context)
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise DecodeError(f"invalid utf-8 at byte {self.offset() - size} while reading {context}") from exc


class Writer:
    def __init__(self):
        self.data = bytearray()

    def put(self, chunk: bytes) -> None:
        self.data.extend(chunk)

    def u8(self, value: int) -> None:
        self.put(struct.pack("<B", value))

    def u16(self, value: int) -> None:
        self.put(struct.pack("<H", value))

    def u32(self, value: int) -> None:
        self.put(struct.pack("<I", value))

    def i32(self, value: int) -> None:
        self.put(struct.pack("<i", value))

    def text(self, value: str, context: str) -> None:
        raw = value.encode("utf-8")
        self.u32(len(raw))
        self.put(raw)

    def bytes(self) -> bytes:
        return bytes(self.data)


@dataclass
class SignalRef:
    kind: str
    a: int
    b: int


def read_signal_ref(reader: Reader) -> SignalRef:
    offset = reader.offset()
    kind = reader.u8("signal ref kind")
    reader.u8("signal ref aux")
    reader.u16("signal ref reserved")
    a = reader.u32("signal ref field a")
    b = reader.u32("signal ref field b")
    names = {
        1: "ThisGate",
        2: "InputPort",
        3: "ChildOutput",
        4: "AncestorOutput",
    }
    if kind not in names:
        raise DecodeError(f"unknown signal ref kind {kind} at byte {offset}")
    return SignalRef(names[kind], a, b)


def write_signal_ref(writer: Writer, signal: SignalRef) -> None:
    kinds = {
        "ThisGate": (1, 0, signal.a, 0),
        "InputPort": (2, 0, signal.a, 0),
        "ChildOutput": (3, 0, signal.a, signal.b),
        "AncestorOutput": (4, 0, signal.a, signal.b),
    }
    if signal.kind not in kinds:
        raise EncodeError(f"unknown signal ref kind {signal.kind!r}")
    kind, aux, a, b = kinds[signal.kind]
    writer.u8(kind)
    writer.u8(aux)
    writer.u16(0)
    writer.u32(a)
    writer.u32(b)


def read_wire_endpoint(reader: Reader) -> dict:
    offset = reader.offset()
    kind = reader.u8("wire endpoint kind")
    aux = reader.u8("wire endpoint aux")
    reader.u16("wire endpoint reserved")
    a = reader.u32("wire endpoint field a")
    b = reader.u32("wire endpoint field b")
    names = {
        1: "GateOutput",
        2: "GateInput",
        3: "ComponentInput",
        4: "ComponentOutput",
        5: "ChildOutput",
        6: "ChildInput",
        7: "AncestorOutput",
    }
    if kind not in names:
        raise DecodeError(f"unknown wire endpoint kind {kind} at byte {offset}")
    return {"kind": names[kind], "aux": aux, "a": a, "b": b}


def write_wire_endpoint(writer: Writer, endpoint: dict) -> None:
    kinds = {
        "GateOutput": (1, endpoint.get("aux", 0), endpoint["a"], 0),
        "GateInput": (2, endpoint["aux"], endpoint["a"], 0),
        "ComponentInput": (3, endpoint.get("aux", 0), endpoint["a"], 0),
        "ComponentOutput": (4, endpoint.get("aux", 0), endpoint["a"], 0),
        "ChildOutput": (5, endpoint.get("aux", 0), endpoint["a"], endpoint["b"]),
        "ChildInput": (6, endpoint.get("aux", 0), endpoint["a"], endpoint["b"]),
        "AncestorOutput": (7, endpoint.get("aux", 0), endpoint["a"], endpoint["b"]),
    }
    kind_name = endpoint["kind"]
    if kind_name not in kinds:
        raise EncodeError(f"unknown wire endpoint kind {kind_name!r}")
    kind, aux, a, b = kinds[kind_name]
    writer.u8(kind)
    writer.u8(aux)
    writer.u16(0)
    writer.u32(a)
    writer.u32(b)


def read_gate(reader: Reader) -> dict:
    offset = reader.offset()
    kind = reader.u16("gate kind")
    payload_len = reader.u16("gate payload length")
    payload = Reader(reader.take(payload_len, "gate payload"), offset + 4)
    gate_names = {
        1: ("BitNAND", 2),
        2: ("BitAND", 2),
        3: ("BitOR", 2),
        4: ("BitNOR", 2),
        5: ("BitXOR", 2),
        6: ("BitXNOR", 2),
        7: ("BitNot", 1),
        8: ("BitNop", 1),
    }
    if kind not in gate_names:
        raise DecodeError(f"unknown gate kind id {kind} at byte {offset}")
    name, signal_count = gate_names[kind]
    refs = [read_signal_ref(payload) for _ in range(signal_count)]
    return {"kind": name, "refs": refs}


def write_gate(writer: Writer, gate: dict) -> None:
    gate_kinds = {
        "BitNAND": 1,
        "BitAND": 2,
        "BitOR": 3,
        "BitNOR": 4,
        "BitXOR": 5,
        "BitXNOR": 6,
        "BitNot": 7,
        "BitNop": 8,
    }
    kind_name = gate["kind"]
    if kind_name not in gate_kinds:
        raise EncodeError(f"unknown gate kind {kind_name!r}")
    refs = gate["refs"]
    payload = Writer()
    for signal in refs:
        write_signal_ref(payload, signal)
    writer.u16(gate_kinds[kind_name])
    writer.u16(len(payload.bytes()))
    writer.put(payload.bytes())


def read_ports(reader: Reader) -> list[dict]:
    count = reader.u32("port count")
    ports = []
    for _ in range(count):
        ports.append(
            {
                "id": reader.u32("port id"),
                "gate": reader.u32("port gate id"),
                "x": reader.u16("port x"),
                "y": reader.u16("port y"),
                "label": reader.text("port label"),
            }
        )
    return ports


def write_ports(writer: Writer, ports: list[dict]) -> None:
    writer.u32(len(ports))
    for port in ports:
        writer.u32(port["id"])
        writer.u32(port["gate"])
        writer.u16(port["x"])
        writer.u16(port["y"])
        writer.text(port.get("label", ""), "port label")


def write_section(writer: Writer, tag: bytes, payload: bytes) -> None:
    if len(tag) != 4:
        raise EncodeError(f"section tag must be 4 bytes, got {tag!r}")
    writer.put(tag)
    writer.u32(len(payload))
    writer.put(payload)


def default_gate_placement(gate_id: int, grid: list[int]) -> dict:
    width = max(grid[0], 1)
    height = max(grid[1], 1)
    return {"gate": gate_id, "min": [gate_id % width, min(gate_id // width, height - 1)]}


def decode_document(path: str) -> dict:
    with open(path, "rb") as f:
        data = f.read()
    reader = Reader(data)

    if reader.take(4, "magic") != MAGIC:
        raise DecodeError("not a CGDF file")
    version = reader.u16("version")
    if version < 1 or version > VERSION:
        raise DecodeError(f"unsupported version {version}")
    reader.u16("reserved")
    section_count = reader.u32("section count")

    sections = {}
    for _ in range(section_count):
        tag = reader.take(4, "section tag").decode("ascii")
        size = reader.u32("section size")
        sections[tag] = Reader(reader.take(size, f"section {tag}"), reader.offset() - size)

    plans_reader = sections["PLNS"]
    plan_count = plans_reader.u32("plan count")
    plans = []
    for _ in range(plan_count):
        plans.append(
            {
                "id": plans_reader.u32("plan id"),
                "grid": [plans_reader.u32("grid width"), plans_reader.u32("grid height")],
                "gates": [
                    {
                        "id": (plans_reader.u32("gate id") if version >= 2 else gate_index),
                        **read_gate(plans_reader),
                    }
                    for gate_index in range(plans_reader.u32("gate count"))
                ],
                "inputs": read_ports(plans_reader),
                "outputs": read_ports(plans_reader),
            }
        )

    components_reader = sections["CMPS"]
    component_count = components_reader.u32("component count")
    components = []
    for _ in range(component_count):
        component = {
            "plan": components_reader.u32("component plan id"),
            "children": [],
            "child_input_connections": [],
            "gate_placements": [],
            "child_placements": [],
            "wires": [],
        }
        child_count = components_reader.u32("child count")
        component["children"] = [components_reader.u32("child id") for _ in range(child_count)]
        connection_count = components_reader.u32("child input connection count")
        for _ in range(connection_count):
            component["child_input_connections"].append(
                {
                    "child": components_reader.u32("connection child id"),
                    "input": components_reader.u32("connection input port id"),
                    "src": read_signal_ref(components_reader),
                }
            )
        if version >= 3:
            placement_count = components_reader.u32("gate placement count")
            for _ in range(placement_count):
                component["gate_placements"].append(
                    {
                        "gate": components_reader.u32("gate placement gate id"),
                        "min": [
                            components_reader.u32("gate placement x"),
                            components_reader.u32("gate placement y"),
                        ],
                    }
                )
        placement_count = components_reader.u32("child placement count")
        for _ in range(placement_count):
            component["child_placements"].append(
                [components_reader.u32("child placement x"), components_reader.u32("child placement y")]
            )
        wire_count = components_reader.u32("wire count")
        for _ in range(wire_count):
            wire = {
                "from": read_wire_endpoint(components_reader),
                "to": read_wire_endpoint(components_reader),
                "bends": [],
            }
            bend_count = components_reader.u32("wire bend count")
            for _ in range(bend_count):
                wire["bends"].append(
                    [components_reader.i32("wire bend x"), components_reader.i32("wire bend y")]
                )
            component["wires"].append(wire)
        components.append(component)

    plans_by_id = {plan["id"]: plan for plan in plans}
    for component in components:
        if component["gate_placements"]:
            continue
        plan = plans_by_id[component["plan"]]
        component["gate_placements"] = [
            default_gate_placement(gate["id"], plan["grid"])
            for gate in sorted(plan["gates"], key=lambda gate: gate["id"])
        ]

    root_reader = sections["ROOT"]
    root = root_reader.u32("root component id")

    return {"plans": plans, "components": components, "root": root}


def encode_document(document: dict) -> bytes:
    out = Writer()
    out.put(MAGIC)
    out.u16(VERSION)
    out.u16(0)
    out.u32(3)

    plans = Writer()
    plans_list = sorted(document["plans"], key=lambda plan: plan["id"])
    plans.u32(len(plans_list))
    for plan in plans_list:
        plans.u32(plan["id"])
        plans.u32(plan["grid"][0])
        plans.u32(plan["grid"][1])
        sorted_gates = sorted(plan["gates"], key=lambda gate: gate["id"])
        plans.u32(len(sorted_gates))
        for gate in sorted_gates:
            plans.u32(gate["id"])
            write_gate(plans, gate)
        write_ports(plans, plan["inputs"])
        write_ports(plans, plan["outputs"])
    write_section(out, b"PLNS", plans.bytes())

    components = Writer()
    components_list = document["components"]
    components.u32(len(components_list))
    for component in components_list:
        components.u32(component["plan"])
        components.u32(len(component["children"]))
        for child_id in component["children"]:
            components.u32(child_id)

        components.u32(len(component["child_input_connections"]))
        for connection in component["child_input_connections"]:
            components.u32(connection["child"])
            components.u32(connection["input"])
            write_signal_ref(components, connection["src"])

        components.u32(len(component.get("gate_placements", [])))
        for placement in component.get("gate_placements", []):
            components.u32(placement["gate"])
            components.u32(placement["min"][0])
            components.u32(placement["min"][1])

        components.u32(len(component["child_placements"]))
        for x, y in component["child_placements"]:
            components.u32(x)
            components.u32(y)

        components.u32(len(component["wires"]))
        for wire in component["wires"]:
            write_wire_endpoint(components, wire["from"])
            write_wire_endpoint(components, wire["to"])
            components.u32(len(wire["bends"]))
            for x, y in wire["bends"]:
                components.i32(x)
                components.i32(y)
    write_section(out, b"CMPS", components.bytes())

    root = Writer()
    root.u32(document["root"])
    write_section(out, b"ROOT", root.bytes())

    return out.bytes()


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print("usage: document_codec.py path/to/file.cgdf [roundtrip-output.cgdf]")
        return 1
    document = decode_document(sys.argv[1])
    print(f"root component: {document['root']}")
    print(f"plans: {len(document['plans'])}")
    print(f"components: {len(document['components'])}")
    for index, component in enumerate(document["components"]):
        print(
            f"component {index}: plan={component['plan']} children={len(component['children'])} wires={len(component['wires'])}"
        )
    if len(sys.argv) == 3:
        encoded = encode_document(document)
        with open(sys.argv[2], "wb") as f:
            f.write(encoded)
        print(f"wrote {len(encoded)} bytes to {sys.argv[2]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
