# Component Runtime Refactor Plan

This is the implementation plan for introducing a clean split between editable plans and runnable circuits with minimum rewrite.

## Scope and Constraints

- Keep current shader/render flow; do not edit or add shaders.
- Keep edits minimal and incremental.
- Add explicit plan/runtime/context boundaries.
- Add save/load for schematic files with extension `.circuit_schematic` from one code constant.
- Use binary serialization (serde + bincode), not JSON.
- Assume component dependency graph is a DAG; reject invalid/cyclic loaded data.
- Do not implement nested child execution semantics yet.
- Leave clear TODO markers where nested editing/connection/IO mapping will go.

## Target Architecture

### 1) ComponentPlan (editable + serializable)

- New module: `src/component_plan.rs`
- Key fields:
  - `ComponentId` (context-local id, no global type id)
  - `name`
  - `board_size: [u32; 2]`
  - `cells: Vec<[u32; 4]>` (flattened board, representing a per-component `[X, Y, 1]` circuit texture)
  - `wires: Vec<StoredWireEdge>`
  - `child_mentions`/placements placeholders for future nested support
  - `direct_dependencies: Vec<ComponentId>`
- Provide helpers for indexing and cell read/write that mirror existing board edit behavior.

### 2) LevelContext (owner + resolver + persistence)

- New module: `src/level_context.rs`
- Owns all `ComponentPlan` values for one level solution.
- Resolves `ComponentId -> ComponentPlan`.
- Stores root edited component id.
- Owns save/load and validation for plan data.
- Save/load should operate only on plan/context data, never runtime.

### 3) CircuitRuntime (initialized + linked runtime)

- New module: `src/circuit_runtime.rs`
- Built from `LevelContext + root ComponentId`.
- Contains runtime GPU resources and source-plan identity link.
- Contains recursively initialized child runtimes (structure now, partial semantics).
- Explicit phases:
  - `build`: allocate/upload runtime resources from plans
  - `link`: resolve runtime references/indices and patch links
- Add TODOs for parent-child IO/interconnect mapping.

## File Format Plan

- Add constant in one place: `SCHEMATIC_FILE_EXTENSION: &str = "circuit_schematic"`.
- Add serializable file envelope type in `level_context` module, example:
  - format version
  - root component id
  - all component plans
- Use `serde` + `bincode` (binary on disk).
- Load path:
  1. decode
  2. structural validation
  3. dependency DAG validation
  4. accept or reject

## Validation Plan

- Keep dependency graph data explicit via `direct_dependencies`.
- Validate in `LevelContext` at load and other plan-input boundaries.
- Validation checks:
  - referenced component ids exist
  - direct self-cycle
  - global cycle detection with concrete path output
- Error style should include readable paths, e.g.:
  - `Processor -> Accumulator -> Register -> Processor`

## UI and App-Flow Plan

- Introduce explicit app mode enum in `main.rs`:
  - `Edit { current_component_id }`
  - `Run { runtime }`
- Add buttons in editor panel:
  - `Start Running`
  - `Restart Running`
  - `Save Schematic`
  - `Load Schematic`
  - optional `Back to Edit`
- Behavior:
  - Edit mode: mutate `ComponentPlan`
  - Start Running: compile plan/context into fresh `CircuitRuntime` and enter run mode
  - Restart Running: rebuild runtime from current plan/context
  - Run mode no longer implicitly edits the same live runtime state

## Rendering and Resource Reuse Plan

- Keep `render.rs`, `hover_preview`, and `wires` shader pipeline unchanged.
- Keep runtime simulation path in `simulation.rs` mostly unchanged.
- Add small adapter path so both modes can provide the same render inputs:
  - edit mode: plan-backed board upload/preview resource
  - run mode: runtime `BoardTextures`

## Incremental Implementation Steps

1. Add dependencies in `Cargo.toml`:
   - `serde` with derive
   - `bincode`
   - optional `rfd` for native open/save dialogs
2. Add `component_plan`, `level_context`, and `circuit_runtime` modules and export from `lib.rs`.
3. Move demo-start content into an initial `LevelContext` + root `ComponentPlan` constructor.
4. Add plan serialization/deserialization and validation.
5. Add app mode split and runtime build/link entrypoint.
6. Rewire editor operations to target current `ComponentPlan` data.
7. Keep render pipeline shared; route render inputs based on app mode.
8. Add save/load UI actions and file picker fallback behavior.
9. Add targeted tests for:
   - round-trip save/load
   - cycle detection path reporting
   - runtime build/link invocation from valid plan graph

## Deferred TODOs (must remain explicit in code)

- Nested child component editing UX.
- Nested connection authoring semantics.
- Full parent-child IO routing/linking semantics.
- Optimized batched runtime packing beyond current per-component `[X, Y, 1]` baseline.

## Definition of Done for This Milestone

- Main editor owns an editable `ComponentPlan` in a `LevelContext`.
- Schematic save/load works via `.circuit_schematic` binary files.
- Loaded invalid/cyclic schematic data is rejected with clear errors.
- There is a clear `plan/context -> build -> link -> CircuitRuntime` path.
- UI has clear edit/run separation with Start/Restart controls.
- Existing shaders remain unchanged.
