# Component Model Notes

## Current State

The editor now works with an explicit plan/runtime split.

- `ComponentPlan` in `src/component_plan.rs` is the editable and serializable board definition.
- `LevelContext` in `src/level_context.rs` owns all component plans for one schematic, tracks the root component, and handles validation plus save/load.
- `CircuitRuntime` in `src/circuit_runtime.rs` builds GPU runtime state from a validated `LevelContext`.

## Terminology

Use these terms consistently in code and docs:

- **component plan**: editable board definition
- **level context**: collection of plans plus root-component metadata
- **circuit runtime**: GPU-backed runnable state built from plans
- **arena_z**: packed texture slice index used by rendering/simulation internals

Avoid using `layer` for user-facing editing concepts. In the current codebase, `arena_z` is still a valid low-level rendering/simulation term, but it should not replace the higher-level plan/runtime model.

## Editing and Run Flow

`src/main.rs` currently has two app modes:

- `Edit`: the board shown in the editor is backed by the root `ComponentPlan`
- `Run`: the app builds a fresh `CircuitRuntime` from the current `LevelContext` and simulates that runtime

The transition works like this:

1. Edit mode uploads the root plan into board textures for interactive editing.
2. `Start Running` or `Restart Running` validates the context and builds a new runtime.
3. `Back to Edit` copies the edited board state back into the root plan.

This keeps editable plan data separate from live simulation state.

## Schematic Files

Schematics are saved through `LevelContext` as binary `bincode` data.

- file extension: `.circuit_schematic`
- save/load entrypoints: `LevelContext::save_to_path` and `LevelContext::load_from_path`
- validation rejects missing roots, missing dependencies, direct self-cycles, and dependency cycles

## Current Limits

The data model already includes placeholders for nested component work, but nested runtime behavior is not implemented yet.

- `ComponentPlan` stores `child_mentions`
- `CircuitRuntime::build_and_link` still has TODOs for child runtime construction and parent/child IO linking
- the current editor experience is centered on the root component plan

## Guidance For Future Changes

- Prefer documenting user-visible behavior in terms of plans, schematics, and runtimes.
- Reserve `arena_z` for low-level packed-texture behavior.
- If nested components become runnable, update this file to describe the actual execution/linking model instead of adding another forward-looking plan doc.
