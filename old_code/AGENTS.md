# AGENTS.md

## Project Overview

This is a Rust + `wgpu` + `winit` + `egui` project for a small circuit-board simulation/editor.

- The app has two main modes:
- Interactive windowed mode via `cargo run --`
- Headless image rendering via `cargo run -- --render-scene ...` or `./render-scene.sh`

The core idea is:

- circuit definitions live in a 3D circuit texture
- charge/state lives in double-buffered 3D charge textures
- simulation advances with a compute shader
- rendering samples those textures directly on the GPU
- editor UI and wire overlay sit on top of the base board render

## Main File Placement

Most of the project lives under `src/`.

- `src/main.rs`: app entrypoint, CLI parsing, window event loop, frame orchestration, undo/redo plumbing
- `src/lib.rs`: module exports
- `src/windowing.rs`: `winit`/surface/device setup helpers
- `src/simulation.rs`: simulation data model, GPU texture setup, compute pipeline, CPU read/write helpers for cells and charge
- `src/render.rs`: main board renderer, hover preview renderer, editor tool preview renderer, camera math
- `src/wires.rs`: wire overlay rendering and in-progress wire drafting behavior
- `src/wire_render.rs`: persistent wire graph/storage structures keyed by endpoints
- `src/editor.rs`: egui editor panel and generic undo/redo stack
- `src/allocator.rs`: standalone texture/page allocator work; important infrastructure, but not currently central to the main frame flow

## Shader Placement

WGSL files also live in `src/` and are embedded with `include_str!`.

- `src/basic_cell.wgsl`: compute shader for simulation stepping
- `src/render.wgsl`: main board fragment/vertex shader
- `src/wires.wgsl`: wire overlay shader
- `src/hover_preview.wgsl`: translucent cell hover preview shader
- `src/editor_tool_preview.wgsl`: tiny preview cards used in the editor UI
- `src/gates_render_header.wgsl`: shared render-only WGSL helpers for drawing gates, labels, wires, and cell visuals

## WGSL Header Setup

This project has one important shader composition pattern:

- `src/render.rs` defines `shader_with_gate_header(source: &str) -> String`
- that function prepends `include_str!("gates_render_header.wgsl")` to another WGSL source string
- the combined WGSL source is then passed to `device.create_shader_module(...)`

Today that shared header is used by these render shaders:

- `render.wgsl`
- `hover_preview.wgsl`
- `editor_tool_preview.wgsl`

It is **not** used by:

- `basic_cell.wgsl` because that is simulation/compute logic
- `wires.wgsl` because the wire overlay has its own dedicated shader path

What lives in `gates_render_header.wgsl`:

- shared shape helpers like segment and rounded-box masks
- small glyph/font data for gate labels
- `render_noop`, `render_source`, `render_wire`, `render_gate`
- `render_cell_color(...)`, which is the main shared entry used by the other render shaders

Practical rule: if a new WGSL file needs to draw board cells/gates with the same visual language, it should usually call through this header and use `render_cell_color(...)` instead of reimplementing that logic.

## Runtime Flow

Interactive mode in `src/main.rs` generally runs in this order:

- create window/device/surface
- create `Simulation`
- create `Renderer`, `HoverPreviewRenderer`, and `WireOverlay`
- create egui state and editor previews
- on each redraw:
- optionally step the simulation compute pipeline
- draw the main board from the active charge buffer + circuit texture
- draw the wire overlay
- draw the hover preview
- draw egui last

Headless render mode is simpler:

- create device
- create `Simulation`
- optionally run `N` simulation steps
- render one frame to an offscreen texture
- copy texture back to CPU and write PNG

## Data Conventions

- Board dimensions and layer counts are currently defined in `src/simulation.rs`
- Circuit cells are encoded as packed `u8` data inside RGBA texels
- Charge is also packed into RGBA texels, with one texel storing a 2x2 block of charge values
- The simulation uses double buffering for charge: read from one buffer, write the next buffer

## Generated / Output Areas

- `target/`: cargo build artifacts and generated docs/images
- `target/render/`: rendered PNG outputs used by the CLI/script flow
- `render-scene.sh`: convenience wrapper around the headless render CLI

## Working Notes For Future Agents

- Prefer reading `src/main.rs` first if you need to understand frame flow or user input behavior
- Prefer reading `src/simulation.rs` first if the change touches cell encoding, stepping, buffers, or board dimensions
- Prefer reading `src/render.rs` plus `src/gates_render_header.wgsl` first for anything about board visuals
- Prefer reading `src/wires.rs` and `src/wire_render.rs` for wire behavior, storage, or overlay rendering
- If you add another board-cell render shader, consider whether it should use the shared WGSL header path rather than duplicating gate drawing code
