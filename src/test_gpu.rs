use crate::windowing::{self, GpuState};
use std::sync::OnceLock;

static SHARED_TEST_GPU: OnceLock<Result<GpuState, String>> = OnceLock::new();

pub fn shared_test_gpu() -> &'static GpuState {
    SHARED_TEST_GPU
        .get_or_init(|| pollster::block_on(windowing::prepare_gpu(None)))
        .as_ref()
        .unwrap_or_else(|error| panic!("Failed to initialize shared test GPU: {error}"))
}
