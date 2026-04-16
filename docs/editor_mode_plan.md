## Editor Mode Plan

This repo already has the pieces needed for a solid editor foundation, but they are currently coupled around the compiled runtime path:

- `main.rs` drives a compiled `DemoRuntime` plus a `ViewerState`.
- `visual_ui.rs` builds a `FocusedScene` from `Component` + `ComponentPlans` + compiled `gate_store` data.
- `scene_render.rs` uploads GPU-side instance buffers from that `FocusedScene`.
- `scene_render.rs` already uses the shared gate shader header pattern with:
  `concat!(include_str!("gate_shared.wgsl"), "\n", include_str!("scene_render.wgsl"))`

The next step should be an explicit split between editable data, compiled runtime data, and render cache so UI editing does not accidentally depend on compile-time charge allocation.

The most important consequence of that split is:

- edit mode should render plan definitions, not the fully expanded runtime instance tree
- all instances of the same child plan share the exact same internal edit-mode visual state
- only run mode should expand per-instance runtime state and show different internal charge behavior per instance

That gives edit mode a much smaller object count and removes the need to pay compile cost just to draw the scene.

## Goals

- Add edit UI for creating plans, navigating plans, placing components, placing wires, deleting, and undo/redo.
- Make undoing edits trivial and obvious.
- Avoid recompiling runtime charge/gate-store data while the user is editing.
- Keep rendering live during editing, even when the scene is not compiled for simulation.
- Reuse `gate_shared.wgsl` for gate and ghost rendering styles.

## Recommended Architecture

Use three layers instead of one mixed state object.

### 1. Editable Model

This is the source of truth for editing. It should not contain compiled charge-buffer locations.

Suggested shape:

```rust
struct EditorDocument {
    plans: EditablePlans,
    root: EditableComponent,
    active_plan: PlanId,
    mode: EditorMode,
    history: EditHistory,
}

enum EditorMode {
    Edit,
    Run,
}
```

`EditablePlans` should stay very close to `ComponentPlans`, but with stable ids for things the editor needs to target directly.

The editable model should be plan-centric, not runtime-instance-centric.

That means:

- a plan's internal gates/wires/child placements exist once in edit mode
- multiple instances of that plan should all point at the same editable plan data
- editing a child plan updates the visual internals for every instance automatically, because they are the same definition

This is the key reason edit mode can stay small even when run mode would explode into many instantiated nodes/gates.

Suggested additions over current plan data:

- Stable `WireId`
- Stable `PlacedGateId` or stable gate slot identity
- Stable child instance identity
- Optional metadata for selection, names, and ghost placement

Important rule: ids referenced by undo actions must stay stable across normal edits.

### 2. Compiled Runtime Snapshot

This is only built in `Run` mode, or on explicit transitions into `Run` mode.

Suggested shape:

```rust
struct CompiledRuntime {
    root: Component,
    plans: ComponentPlans,
    gate_store: Arc<HashMap<(NodeId, GateId), GateStoreLocation>>,
    words_per_buffer: u32,
    gpu_plan: UploadedGpuPlan,
    charge_buffers: [wgpu::Buffer; 2],
}
```

This is basically the existing `DemoRuntime`, but treated as a cache derived from the editable document, not as the thing the UI edits directly.

### 3. Render Scene Cache

Rendering should work in both modes.

- In `Run` mode, render from compiled charge refs as today.
- In `Edit` mode, render from editable plan geometry with no requirement that compile has run.

That means the scene-building layer should split into two paths instead of trying to make one runtime scene type serve both jobs.

Recommended change:

- Keep the existing runtime-focused scene dependent on compiled charge data.
- Add a separate edit-focused scene that carries no compiled charge dependencies.
- In the edit-focused scene, gate/port/wire charge references should be `None` or an explicit zero-state ref.
- In the runtime-focused scene, charge references should still come from compiled `gate_store`.

This keeps the renderer working while making the simulation overlay optional.

## Why Edit Mode Must Be Plan-Centric

If edit mode reuses the fully compiled runtime scene shape, large circuits will still pay for:

- per-instance node expansion
- per-instance gate expansion
- per-instance charge/gate-store mapping
- full compile-time dependency chains before any visual update

That defeats the purpose of edit mode.

The correct edit-mode rendering target is the plan graph itself.

Example:

- if plan `AdderCell` is instantiated 500 times,
- edit mode should store and render the `AdderCell` internals once per focused view of that plan,
- not 500 separate copies of its gates and wires.

Run mode is allowed to expand those 500 instances because simulation state is per instance. Edit mode should not.

## Undo / Redo

Use a linear history plus a cursor, exactly like you described.

```rust
struct EditHistory {
    actions: Vec<EditAction>,
    cursor: usize,
}
```

- Applied range: `actions[..cursor]`
- Redo range: `actions[cursor..]`
- On new edit while `cursor < actions.len()`, truncate `actions` to `cursor` first
- Undo applies `action.inverse()`
- Redo reapplies `action`

### Key Rule

Every stored action must be self-inverting without requiring a diff pass.

Do not store vague actions like "rebuild plan" or "mutate gate list".

Store concrete reversible actions instead.

### Recommended Action Shape

Prefer actions that explicitly contain enough before/after data to apply in either direction.

```rust
enum EditAction {
    AddGate { plan: PlanId, gate_id: GateId, gate: EditableGate, placement: GatePlacement },
    RemoveGate { plan: PlanId, gate_id: GateId, gate: EditableGate, placement: GatePlacement },
    MoveGate { plan: PlanId, gate_id: GateId, from: GatePlacement, to: GatePlacement },
    AddWire { plan: PlanId, wire: EditableWire },
    RemoveWire { plan: PlanId, wire: EditableWire },
    UpdateWire { plan: PlanId, wire_id: WireId, from: EditableWire, to: EditableWire },
    AddChild { plan: PlanId, child: EditableChild },
    RemoveChild { plan: PlanId, child: EditableChild },
    MoveChild { plan: PlanId, child_id: ChildId, from: ChildPlacement, to: ChildPlacement },
    CreatePlan { plan_id: PlanId, plan: EditablePlan },
    DeletePlan { plan_id: PlanId, plan: EditablePlan },
    SetRootPlan { from: PlanId, to: PlanId },
}
```

For every variant, implement:

- `apply(&mut EditorDocument)`
- `inverse(&self) -> EditAction`

This makes undo logic mechanically simple.

### Composite Actions

Use transactions for multi-step UI actions.

Example:

- deleting a gate should also remove attached wires
- deleting a child instance may require removing child-input connections

Represent this as:

```rust
EditAction::Batch(Vec<EditAction>)
```

Undo then just walks the batch in reverse.

## Edit vs Run Mode

This should be a hard product distinction, not just a UI toggle.

### Edit Mode

- Place gates/components/wires
- Delete/select/navigate plans
- Build visual geometry inline from editable plans
- Do not call `compile_component_tree`
- Do not rebuild GPU charge plan
- Draw with optional or dummy charge refs
- Treat all charge state as zero by default

Because the runtime charge buffers are zeroed at init anyway, edit mode can simply assume wires and gates are visually in the zero/off state. There is no need to synthesize meaningful charge data before compile.

### Run Mode

- Freeze or snapshot current editable document
- Convert editable model into current `Component` + `ComponentPlans`
- Run `compile_component_tree`
- Upload kernel plan and charge buffers
- Render with real charge refs

Transition recommendation:

- `Edit -> Run`: compile a fresh runtime snapshot
- `Run -> Edit`: discard compiled cache or mark it stale
- Any edit while in `Run` should either:
  - force a return to `Edit`, or
  - mark runtime as dirty and prevent stepping until recompiled

The first option is simpler and safer.

## Rendering Changes

Current issue:

- the current focused scene path is built around compiled runtime data
- there is no separate edit-scene path yet

Recommended render-side change:

1. Introduce a lightweight render charge descriptor:

```rust
enum VisualChargeRef {
    None,
    Gate {
        buffer: u32,
        bit: u32,
        source_mode: u32,
        gate_tag: u32,
    },
}
```

2. Store `VisualChargeRef` directly in placed gates/ports/wires instead of resolving through `gate_store` during upload.

3. In edit mode, emit `VisualChargeRef::None`.

4. In run mode, emit real refs from `GateStoreLocation`.

This should be applied to the edit-scene path first.

For the runtime-scene path, keeping compiled charge references is correct because that scene exists specifically to show real simulated state.

The important split is not "runtime scene no longer needs charge data". The important split is "edit scene should never need runtime compile data".

For edit mode specifically, `VisualChargeRef::None` should be treated by the shader as inactive/off. That matches the desired "all zeros" behavior and avoids fake buffer plumbing.

### Ghost Rendering

Ghost gates and ghost wires should use the same geometry path as normal renderables with alternate style flags.

Recommended approach:

- Extend gate instance metadata with a `render_kind` field:
  - normal
  - ghost
  - selected
- Keep `render_gate()` in `gate_shared.wgsl` as the common gate body
- Add small style-selection helpers rather than forking gate drawing logic

Because `gate_shared.wgsl` already contains `render_gate()`, `scene_gate_style()`, and `ui_gate_style()`, it should stay the shared include for scene rendering and any future editor-preview shader.

## Buffer Management

Right now `upload_scene_tree()` rebuilds full instance buffers from the scene tree. That is acceptable for the first editor pass.

Do not start with a freelist allocator unless profiling proves it is needed.

The bigger optimization win is not freelists first. It is avoiding runtime instance expansion entirely in edit mode.

That should drastically reduce:

- total gate count
- total wire count
- total child scene count
- amount of CPU work per CRUD operation
- number of dependencies touched by a local edit

Recommended phases:

1. Rebuild whole edit-scene instance buffers on each committed edit.
2. If large edits become expensive, chunk by scene section:
   - gates
   - ports
   - child frames
   - wires
   - optionally per-child subtree
3. Only add CPU freelists if chunk rebuilds are still too slow.

This keeps the first implementation simple and correct.

## Suggested Implementation Order

### Phase 1

- Add editor document types
- Add `EditorMode`
- Add `EditHistory`
- Add reversible `EditAction` + `Batch`
- Add edit-mode scene builder that does not require compile output

### Phase 2

- Add plan list UI
- Add focus/navigation between plans
- Add selection state
- Add gate placement with ghost preview
- Add delete action

### Phase 3

- Add wire placement UI
- Add child placement UI
- Route all edits through history transactions
- Add undo/redo hotkeys and buttons

### Phase 4

- Add `Run` compile transition
- Add dirty-state handling between edit and run
- Re-enable stepping/simulation only for compiled snapshots

### Phase 5

- Profile large scenes
- If needed, chunk render uploads
- If still needed, introduce CPU-side freelist management for render instances

## Concrete Refactors To Start With

These are the first code changes I would make in a follow-up pass:

1. Extract the current demo-only runtime/viewer wiring in `main.rs` behind an app state that can hold either edit or run state.
2. Split scene building into:
   - edit-mode plan rendering
   - run-mode compiled instance rendering
3. Keep `build_focused_scene(...)` or its runtime replacement dependent on compiled `gate_store` data.
4. Add a separate edit-scene builder that does not require compiled `gate_store` data.
5. Move charge resolution earlier where useful so render upload can consume direct charge refs instead of repeatedly consulting a `(NodeId, GateId) -> GateStoreLocation` map.
6. Add `EditHistory` with cursor semantics and `Batch` undo.
7. Start by making plan creation, gate placement, delete, undo, and redo the first complete vertical slice.

## First Vertical Slice Target

The best small milestone is:

- switch between `Edit` and `Run`
- create a new empty plan
- navigate to that plan
- place a gate with ghost preview
- delete that gate
- undo/redo those edits

That gets the architecture right before wires and child composition make the state model more complicated.
