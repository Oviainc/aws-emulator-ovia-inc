// SPDX-License-Identifier: CC0-1.0

//! Boot check: the production router assembles.
//!
//! Keep this as a suite of the router's integration-test target, declared in
//! `router/tests/all.rs`. Removing it, or moving it into `src/`, stops `cargo test` from
//! emitting the `aws-emulator` binary.
//!
//! # Assemble on an explicitly sized thread
//!
//! Assembling the router costs stack in proportion to the largest `mount_chunk_NN` plus
//! the largest single service's `ops::register_all`, which is more than a cargo test
//! thread's default stack. Call `build_router` on a thread created with
//! [`BOOT_STACK_SIZE`]; running it directly on the test thread overflows the stack and
//! aborts once the workspace carries one large service.
use std::sync::Arc;
use aws_emulator_router::{build_router, InMemoryEngine};

/// Stack for assembling the router here. Keep this at or above the size
/// `router/src/main.rs` uses, so a workspace that boots in production also passes here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

#[test]
fn the_production_router_assembles() {
    std::thread::Builder::new()
        .name("boot-check".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let _router = build_router(engine);
        })
        .expect("spawn the boot-check thread")
        .join()
        .expect("the production router must assemble");
}
