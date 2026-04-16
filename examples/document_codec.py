#!/usr/bin/env python3

import struct
import sys
from dataclasses import dataclass


MAGIC = b"CGDF"
VERSION = 1


class DecodeError(Exception):
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


def decode_document(path: str) -> dict:
    data = open(path, "rb").read()
    reader = Reader(data)

    if reader.take(4, "magic") != MAGIC:
        raise DecodeError("not a CGDF file")
    version = reader.u16("version")
    if version != VERSION:
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
                "gates": [read_gate(plans_reader) for _ in range(plans_reader.u32("gate count"))],
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

    root_reader = sections["ROOT"]
    root = root_reader.u32("root component id")

    return {"plans": plans, "components": components, "root": root}


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: document_codec.py path/to/file.cgdf")
        return 1
    document = decode_document(sys.argv[1])
    print(f"root component: {document['root']}")
    print(f"plans: {len(document['plans'])}")
    print(f"components: {len(document['components'])}")
    for index, component in enumerate(document["components"]):
        print(
            f"component {index}: plan={component['plan']} children={len(component['children'])} wires={len(component['wires'])}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
