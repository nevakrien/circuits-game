# Circuits Game Document Format (`CGDF`)

This file format stores the authored circuit document, with no runtime state.

It is designed to be:
- simple to parse in low-level languages
- stable across saves
- explicit about errors
- easy to extend without changing the whole container layout

All integers are little-endian.

## File Layout

Every file starts with this header:

| Bytes | Type | Meaning |
| --- | --- | --- |
| 0..4 | ASCII | Magic: `CGDF` |
| 4..6 | `u16` | Format version. Current value: `1` |
| 6..8 | `u16` | Reserved. Must be `0` for version 1 |
| 8..12 | `u32` | Section count |

After the header come `section_count` sections.

Each section is:

| Bytes | Type | Meaning |
| --- | --- | --- |
| 0..4 | ASCII | Section tag |
| 4..8 | `u32` | Payload length in bytes |
| 8.. | bytes | Payload |

Known section tags:
- `PLNS`: component plans
- `CMPS`: component definitions
- `ROOT`: root component id

Unknown sections may be skipped.

## Plans Section: `PLNS`

Payload layout:

| Field | Type |
| --- | --- |
| plan count | `u32` |
| repeated plans | variable |

Each plan is:

| Field | Type |
| --- | --- |
| plan id | `u32` |
| grid width | `u32` |
| grid height | `u32` |
| gate count | `u32` |
| gates | variable |
| input port count + ports | variable |
| output port count + ports | variable |

## Gate Encoding

Each gate is:

| Field | Type |
| --- | --- |
| gate kind id | `u16` |
| payload length | `u16` |
| payload bytes | variable |

Current gate kind ids:

| Id | Gate | Payload |
| --- | --- | --- |
| 1 | `BitNAND` | 2 signal refs |
| 2 | `BitAND` | 2 signal refs |
| 3 | `BitOR` | 2 signal refs |
| 4 | `BitNOR` | 2 signal refs |
| 5 | `BitXOR` | 2 signal refs |
| 6 | `BitXNOR` | 2 signal refs |
| 7 | `BitNot` | 1 signal ref |
| 8 | `BitNop` | 1 signal ref |

Unknown gate ids are an error in the current game version.

The separate `payload length` field is there so newer tools can add new gate kinds without changing the outer container structure.

## Signal Reference Encoding

Every signal ref is exactly 12 bytes:

| Field | Type | Meaning |
| --- | --- | --- |
| kind | `u8` | signal kind id |
| aux | `u8` | currently `0` |
| reserved | `u16` | currently `0` |
| a | `u32` | first payload field |
| b | `u32` | second payload field |

Kinds:

| Kind | Variant | `a` | `b` |
| --- | --- | --- | --- |
| 1 | `ThisGate` | gate id | `0` |
| 2 | `InputPort` | port id | `0` |
| 3 | `ChildOutput` | child id | port id |
| 4 | `AncestorOutput` | ancestor depth | port id |

## Port Encoding

Ports are used for both inputs and outputs.

Port list layout:

| Field | Type |
| --- | --- |
| port count | `u32` |
| repeated ports | variable |

Each port is:

| Field | Type |
| --- | --- |
| port id | `u32` |
| source gate id | `u32` |
| x | `u16` |
| y | `u16` |
| label byte length | `u32` |
| label bytes | UTF-8 |

An empty label means "no label".

## Components Section: `CMPS`

Payload layout:

| Field | Type |
| --- | --- |
| component count | `u32` |
| repeated components | variable |

Components are stored by component definition id. The first component in the section is component `0`, the next is component `1`, and so on.

Each component is:

| Field | Type |
| --- | --- |
| plan id | `u32` |
| child count | `u32` |
| child component ids | `u32[]` |
| child input connection count | `u32` |
| child input connections | variable |
| child placement count | `u32` |
| child placements | variable |
| wire count | `u32` |
| wires | variable |

`child count` and `child placement count` are intentionally separate.

- `child count` is the number of child components in the component's actual circuit structure.
- `child placement count` is the number of saved visual positions for those children in the editor layout.

In a complete file, these counts are usually the same because each child normally has one placement entry at the same index.

They can still differ in valid files because child placements are visual-only data. A file may omit some placement entries at the end of the list, and the game will infer default positions for those missing children when loading the document.

The placement list is ordered to match the child list:

- placement `0` describes child `0`
- placement `1` describes child `1`
- and so on

Because of that ordering, omitted placements are only supported at the end of the placement list, not in the middle.

### Child Input Connections

Each child input connection is:

| Field | Type |
| --- | --- |
| child id | `u32` |
| child input port id | `u32` |
| source signal ref | 12 bytes |

### Child Placements

Each child placement is:

| Field | Type |
| --- | --- |
| min x | `u32` |
| min y | `u32` |

### Wires

Wires store visual routing only. They do not change circuit logic.

Each wire is:

| Field | Type |
| --- | --- |
| from endpoint | 12 bytes |
| to endpoint | 12 bytes |
| bend count | `u32` |
| bend points | variable |

The wire must connect one of the real logical connections in the component.

If `bend count` is `0`, the game generates a basic orthogonal route.

Each bend point is:

| Field | Type |
| --- | --- |
| x | `i32` |
| y | `i32` |

Wire bend coordinates are in local component grid space, using `256` units per grid cell.

Examples:
- `(256, 0)` means "1 cell right, same row"
- `(-128, 512)` means "half a cell left, 2 cells down"

## Wire Endpoint Encoding

Every wire endpoint is exactly 12 bytes:

| Field | Type | Meaning |
| --- | --- | --- |
| kind | `u8` | endpoint kind id |
| aux | `u8` | extra small value |
| reserved | `u16` | currently `0` |
| a | `u32` | first payload field |
| b | `u32` | second payload field |

Kinds:

| Kind | Variant | `aux` | `a` | `b` |
| --- | --- | --- | --- | --- |
| 1 | `GateOutput` | `0` | gate id | `0` |
| 2 | `GateInput` | input index | gate id | `0` |
| 3 | `ComponentInput` | `0` | port id | `0` |
| 4 | `ComponentOutput` | `0` | port id | `0` |
| 5 | `ChildOutput` | `0` | child id | port id |
| 6 | `ChildInput` | `0` | child id | port id |
| 7 | `AncestorOutput` | `0` | ancestor depth | port id |

## Root Section: `ROOT`

Payload:

| Field | Type |
| --- | --- |
| root component id | `u32` |

## Error Handling Rules

The current game version treats these as hard errors:
- wrong magic
- unsupported version
- truncated file
- invalid UTF-8 labels
- unknown gate kind ids
- invalid signal ref kinds
- invalid wire endpoint kinds
- bad component references after decoding
- wire records that do not match a real logical connection

The current game repairs a few visual-only omissions:
- missing child placements at the end of a placement list can be inferred
- missing wire records can be regenerated from circuit logic
- wires with zero bends use the default orthogonal path

## Backward Compatibility Strategy

Version 1 keeps the container stable with sections and per-gate payload lengths.

That means future versions can usually evolve by:
- adding new optional sections
- adding new gate ids with their own payload format
- teaching newer loaders how to understand them

Older versions of the game should fail clearly when they see a gate id they do not know.

## Reference Implementation

Rust reference code:
- `src/document_format.rs`

Python example:
- `examples/document_codec.py`
