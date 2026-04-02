use crate::windowing::{self, GpuState};
use std::sync::OnceLock;

static SHARED_TEST_GPU: OnceLock<Result<GpuState, String>> = OnceLock::new();

pub fn shared_test_gpu() -> Option<&'static GpuState> {
    SHARED_TEST_GPU
        .get_or_init(|| pollster::block_on(windowing::prepare_gpu(None)))
        .as_ref()
        .ok()
}
