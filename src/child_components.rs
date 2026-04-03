use serde::{Deserialize, Serialize};

use crate::{
    allocator::{AllocHandle, AllocRange, TextureAllocator},
    buffer_allocator::{BufferAllocHandle, BufferAllocRange, BufferAllocator},
    component_plan::ComponentId,
    game_constants::GameConstants,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentFootprint {
    pub width: u32,
    pub height: u32,
}

impl ComponentFootprint {
    pub fn as_array(self) -> [u32; 2] {
        [self.width, self.height]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildInstancePlan {
    pub component_id: ComponentId,
    pub origin: [u32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildPortLayout {
    pub input_words: u32,
    pub output_words: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildResourcePlanNode {
    pub grid_size: [u16; 2],
    pub port_layout: ChildPortLayout,
    pub children: Vec<ChildResourcePlanNode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocatedChildGrid {
    pub range: AllocRange,
    pub handle: AllocHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocatedChildIo {
    pub input: BufferAllocRange,
    pub output: BufferAllocRange,
    pub input_handle: BufferAllocHandle,
    pub output_handle: BufferAllocHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildRuntimePlan {
    pub root_children: Vec<PlannedChildInstance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedChildInstance {
    pub grid: AllocatedChildGrid,
    pub io: AllocatedChildIo,
    pub children: Vec<PlannedChildInstance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentShapeError {
    EmptyOutsideShape,
    EmptyInternalShape,
    OutsideExceedsInternal {
        outside: [u32; 2],
        internal: [u32; 2],
    },
    InternalExceedsHardMax {
        internal: [u32; 2],
        max: [u32; 2],
    },
}

pub fn validate_component_shapes(
    outside: [u32; 2],
    internal: [u32; 2],
    constants: &GameConstants,
) -> Result<(), ComponentShapeError> {
    if outside[0] == 0 || outside[1] == 0 {
        return Err(ComponentShapeError::EmptyOutsideShape);
    }
    if internal[0] == 0 || internal[1] == 0 {
        return Err(ComponentShapeError::EmptyInternalShape);
    }
    if outside[0] > internal[0] || outside[1] > internal[1] {
        return Err(ComponentShapeError::OutsideExceedsInternal { outside, internal });
    }
    if internal[0] > constants.component_sizing.max_internal[0]
        || internal[1] > constants.component_sizing.max_internal[1]
    {
        return Err(ComponentShapeError::InternalExceedsHardMax {
            internal,
            max: constants.component_sizing.max_internal,
        });
    }
    Ok(())
}

pub fn plan_child_runtime(
    root_children: &[ChildResourcePlanNode],
    constants: &GameConstants,
) -> ChildRuntimePlan {
    let page = constants.component_sizing.texture_page;
    let classes = collect_grid_classes(root_children);
    let mut texture_alloc = TextureAllocator::with_page_size(page[0], page[1], classes);
    let mut input_alloc = BufferAllocator::new(constants.child_io_sizing.words_per_page);
    let mut output_alloc = BufferAllocator::new(constants.child_io_sizing.words_per_page);

    let mut planned_children = Vec::with_capacity(root_children.len());

    for child in root_children {
        planned_children.push(plan_child_instance(
            child,
            &mut texture_alloc,
            &mut input_alloc,
            &mut output_alloc,
        ));
    }

    for child in &planned_children {
        input_alloc.free(child.io.input_handle);
        output_alloc.free(child.io.output_handle);
    }

    ChildRuntimePlan {
        root_children: planned_children,
    }
}

fn collect_grid_classes(nodes: &[ChildResourcePlanNode]) -> Vec<(u16, u16)> {
    let mut classes = Vec::new();
    collect_grid_classes_into(nodes, &mut classes);
    classes.sort_unstable();
    classes.dedup();
    classes
}

fn collect_grid_classes_into(nodes: &[ChildResourcePlanNode], classes: &mut Vec<(u16, u16)>) {
    for node in nodes {
        classes.push((node.grid_size[0], node.grid_size[1]));
        collect_grid_classes_into(&node.children, classes);
    }
}

fn plan_child_instance(
    node: &ChildResourcePlanNode,
    texture_alloc: &mut TextureAllocator,
    input_alloc: &mut BufferAllocator,
    output_alloc: &mut BufferAllocator,
) -> PlannedChildInstance {
    let grid_alloc = texture_alloc
        .alloc_exact(node.grid_size[0], node.grid_size[1])
        .expect("child grid size should be supported by texture allocator");
    let input_alloc_result = input_alloc
        .alloc(node.port_layout.input_words.max(1))
        .expect("child input allocation should fit in configured page size");
    let output_alloc_result = output_alloc
        .alloc(node.port_layout.output_words.max(1))
        .expect("child output allocation should fit in configured page size");

    let mut planned_children = Vec::with_capacity(node.children.len());
    for child in &node.children {
        planned_children.push(plan_child_instance(
            child,
            texture_alloc,
            input_alloc,
            output_alloc,
        ));
    }

    for child in &planned_children {
        input_alloc.free(child.io.input_handle);
        output_alloc.free(child.io.output_handle);
    }

    PlannedChildInstance {
        grid: AllocatedChildGrid {
            range: grid_alloc.range,
            handle: grid_alloc.handle,
        },
        io: AllocatedChildIo {
            input: input_alloc_result.range,
            output: output_alloc_result.range,
            input_handle: input_alloc_result.handle,
            output_handle: output_alloc_result.handle,
        },
        children: planned_children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn constants_with_limits(texture_page: [u32; 2], words_per_page: u32) -> GameConstants {
        let mut constants = GameConstants::default();
        constants.component_sizing.texture_page = texture_page;
        constants.child_io_sizing.words_per_page = words_per_page;
        constants
    }

    fn leaf(grid: [u16; 2], input_words: u32, output_words: u32) -> ChildResourcePlanNode {
        ChildResourcePlanNode {
            grid_size: grid,
            port_layout: ChildPortLayout {
                input_words,
                output_words,
            },
            children: Vec::new(),
        }
    }

    fn collect_resource_usage(
        children: &[PlannedChildInstance],
        texture_pages: &mut BTreeSet<u32>,
        input_pages: &mut BTreeSet<u32>,
        output_pages: &mut BTreeSet<u32>,
    ) {
        for child in children {
            texture_pages.insert(child.grid.range.z);
            input_pages.insert(child.io.input.page);
            output_pages.insert(child.io.output.page);
            collect_resource_usage(&child.children, texture_pages, input_pages, output_pages);
        }
    }

    fn textures_needed_for_pages(page_count: usize) -> u32 {
        let max_depth = crate::simulation::guaranteed_wgpu_limits().max_texture_dimension_3d;
        (page_count as u32).div_ceil(max_depth)
    }

    #[test]
    fn validates_outside_shape_against_internal_shape_and_hard_limit() {
        let constants = GameConstants::default();
        assert_eq!(
            validate_component_shapes([2, 1], [8, 8], &constants),
            Ok(())
        );
        assert_eq!(
            validate_component_shapes([3, 2], [2, 2], &constants),
            Err(ComponentShapeError::OutsideExceedsInternal {
                outside: [3, 2],
                internal: [2, 2],
            })
        );
        assert_eq!(
            validate_component_shapes([2, 1], [2048, 8], &constants),
            Err(ComponentShapeError::InternalExceedsHardMax {
                internal: [2048, 8],
                max: [1024, 1024],
            })
        );
    }

    #[test]
    fn massive_child_runtime_plan_can_be_checked_with_tiny_pages() {
        // We cannot efficiently run a truly massive runtime build in tests just to prove the
        // allocator math scales. Instead, this test forces one child per texture page and one
        // child per input/output buffer page by using tiny limits, then plans more pages than the
        // guaranteed wgpu 3D texture depth. That gives us a cheap planner-side stress test for a
        // massive build without constructing the actual GPU resources.
        //
        // What is still missing: runtime construction does not consume `plan_child_runtime`, so we
        // do not yet have a path that turns these planned z pages into multiple real textures once
        // they exceed a single texture's guaranteed depth. The correct behavior is to split across
        // textures, not reject the plan.
        let child_count = crate::simulation::guaranteed_wgpu_limits().max_texture_dimension_3d + 1;
        let constants = constants_with_limits([8, 8], 4);
        let root_children = vec![leaf([8, 8], 4, 4); child_count as usize];

        let planned = plan_child_runtime(&root_children, &constants);

        let mut texture_pages = BTreeSet::new();
        let mut input_pages = BTreeSet::new();
        let mut output_pages = BTreeSet::new();
        collect_resource_usage(
            &planned.root_children,
            &mut texture_pages,
            &mut input_pages,
            &mut output_pages,
        );

        assert_eq!(texture_pages.len(), child_count as usize);
        assert_eq!(input_pages.len(), child_count as usize);
        assert_eq!(output_pages.len(), child_count as usize);
        assert!(
            texture_pages.len() as u32
                > crate::simulation::guaranteed_wgpu_limits().max_texture_dimension_3d
        );
        assert_eq!(textures_needed_for_pages(texture_pages.len()), 2);
    }

    #[test]
    fn small_hierarchy_reuses_single_child_io_resource_pair() {
        let constants = constants_with_limits([16, 16], 64);
        let grandchild = leaf([8, 8], 2, 2);
        let child = ChildResourcePlanNode {
            grid_size: [8, 8],
            port_layout: ChildPortLayout {
                input_words: 2,
                output_words: 2,
            },
            children: vec![
                grandchild.clone(),
                grandchild.clone(),
                grandchild.clone(),
                grandchild,
            ],
        };
        let root_children = vec![child.clone(), child.clone(), child.clone(), child];

        let planned = plan_child_runtime(&root_children, &constants);

        let mut texture_pages = BTreeSet::new();
        let mut input_pages = BTreeSet::new();
        let mut output_pages = BTreeSet::new();
        collect_resource_usage(
            &planned.root_children,
            &mut texture_pages,
            &mut input_pages,
            &mut output_pages,
        );

        assert_eq!(input_pages.len(), 1);
        assert_eq!(output_pages.len(), 1);
        assert_eq!(texture_pages.len(), 5);
    }

    #[test]
    fn configurable_limits_can_force_exactly_two_child_io_resources() {
        let constants = constants_with_limits([16, 16], 8);
        let root_children = vec![
            leaf([8, 8], 2, 2),
            leaf([8, 8], 2, 2),
            leaf([8, 8], 2, 2),
            leaf([8, 8], 2, 2),
            leaf([8, 8], 2, 2),
        ];

        let planned = plan_child_runtime(&root_children, &constants);

        let mut texture_pages = BTreeSet::new();
        let mut input_pages = BTreeSet::new();
        let mut output_pages = BTreeSet::new();
        collect_resource_usage(
            &planned.root_children,
            &mut texture_pages,
            &mut input_pages,
            &mut output_pages,
        );

        assert_eq!(input_pages.len(), 2);
        assert_eq!(output_pages.len(), 2);
    }

    #[test]
    fn oversized_child_io_request_panics_before_it_can_overallocate_a_page() {
        let constants = constants_with_limits([16, 16], 8);
        let root_children = vec![leaf([8, 8], 9, 2)];

        let panic = std::panic::catch_unwind(|| {
            let _ = plan_child_runtime(&root_children, &constants);
        });

        assert!(panic.is_err());
    }
}
