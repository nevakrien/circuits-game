# Component Model Notes

## Direction

The long-term model is **not** "the editor activates a layer".

- The thing being edited is a **component plan**.
- A plan can have child component plans and multiple instances.
- Runtime state lives on **component instances**, not on an abstract layer concept.
- The packed 3D textures are an execution/storage detail for batching many component instances in one pass.

## Current Terminology Rule

When code refers to the `z` coordinate inside the packed textures, call it something like:

- `arena_z`
- `component_z`
- `arena_index`

Do **not** call that a `layer` in new UI or new editor-facing APIs. `Layer` is too overloaded and suggests the user is selecting an editing layer, which is the wrong model.

## Editing Model

- Editing targets a **component plan**.
- In the current mental model, the edited component is effectively the root plan.
- Child components only appear as **instances of existing plans**; they are not the thing the user is directly editing in the same sense.
- That means there is a strong argument that active editing happens at `arena_z = 0` for the edited plan, while non-zero packed `z` ranges are runtime placement details for instances.

## GPU / CPU Direction

- Gate/circuit data and charge data should eventually be packed into batched 3D textures.
- Each component instance should own an allocated range in those textures.
- Simulation should advance across the packed batch in one forward pass rather than one kernel per 2D slice.
- Rendering should use views into the packed textures for the relevant component instance.
- Automatic simulation stepping is still expected to be CPU-driven at the orchestration level.

## Refactor Guidance

- Safe short-term work: rename misleading `layer` names that actually mean packed texture `z` or arena placement.
- Do not introduce more editor concepts that imply the user is selecting or editing a layer.
- The bigger follow-up refactor is to replace remaining fake-layer assumptions with explicit component-plan/component-instance terminology.
