#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentSizing {
    pub small_internal_min: [u32; 2],
    pub large_internal_min: [u32; 2],
    pub max_internal: [u32; 2],
    pub texture_page: [u32; 2],
}

impl Default for ComponentSizing {
    fn default() -> Self {
        Self {
            small_internal_min: [8, 8],
            large_internal_min: [16, 16],
            max_internal: [1024, 1024],
            texture_page: [1024, 1024],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildIoSizing {
    pub words_per_page: u32,
}

impl Default for ChildIoSizing {
    fn default() -> Self {
        Self {
            words_per_page: 64 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GameConstants {
    pub component_sizing: ComponentSizing,
    pub child_io_sizing: ChildIoSizing,
}
