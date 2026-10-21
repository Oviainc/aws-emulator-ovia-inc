// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
#![doc(hidden)]
//! Router — assembles all services into the multi-service emulator.
//!
//! # Writing integration tests
//!
//! Tests live in `router/tests/all/<name>.rs`, each declared in `router/tests/all.rs`.
//! Every router test target links all services, so the router has exactly one; a file
//! directly under `router/tests/` fails the build. `build_router(engine)` returns the
//! `MultiServiceRouter` the production entrypoint serves; drive requests through
//! `router.dispatch(req)` and assert on the response. The framework handles
//! deserialization, routing, and engine injection.
//!
//! ## Pattern
//!
//! ```rust,ignore
//! use std::sync::Arc;
//!
//! #[tokio::test]
//! async fn test_create_thing() {
//!     let engine = Arc::new(engine::InMemoryEngine::new());
//!     let router = build_router(engine);
//!     let req = http::Request::builder()
//!         .method("POST")
//!         .uri("http://localhost/2021-01-01/things")
//!         .header("authorization", /* a SigV4 header naming the service */ "...")
//!         .body(bytes::Bytes::from_static(br#"{"Name":"my-thing"}"#))
//!         .unwrap();
//!     let resp = router.dispatch(req).await;
//!     // 501 until you implement the operation — a FRESHLY SCAFFOLDED operation
//!     // answers `NotImplemented`, so asserting 200 here fails until the `handle`
//!     // body is written. Change this to 200 as part of implementing it.
//!     assert_eq!(resp.status().as_u16(), 501);
//! }
//! ```
//!
//! ## Key points
//!
//! - **Direct construction**: each service is built from the shared engine via
//!   `<service>::EmulatedService::new(engine.clone())`, which constructs each
//!   operation's authored `impl Operation` struct through the engine-as-constructor
//!   seam. There is no builder and no per-operation registration step.
//! - **Routing**: `MultiServiceRouter` picks the service whose `can_handle` claims
//!   the request (seeded from the model's SigV4 signing name), then the service's
//!   protocol names the operation.
//! - **Deps**: a router integration test needs `service-foo`, `engine`, `http`,
//!   `bytes` and `tokio`. Put the test in the SERVICE crate rather than here:
//!   `router/Cargo.toml` is re-rendered on every service add (see the banner at the
//!   top of this file), so a dev-dependency added there is overwritten.

// @generated — RE-RENDERED ON EVERY SERVICE ADD. Edits below are OVERWRITTEN.
//
// Put hand-written code in a SERVICE crate, or in `router/src/main.rs`. Those are
// yours; this file, `router/Cargo.toml` and the workspace `Cargo.toml` are replaced
// whenever a service is added, so an edit here is lost.

use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
pub use engine::InMemoryEngine;

pub fn new_engine() -> Arc<InMemoryEngine> {
    Arc::new(InMemoryEngine::new())
}

/// Assemble the multi-service router the production entrypoint serves.
///
/// Registrations are DERIVED from the scaffolded service set, not hand-maintained:
/// adding a service re-renders this whole file. `router/tests/all/boot_serve.rs` exercises
/// this function.
///
/// Keep this a plain `fn` whose registrations stay in the `#[inline(never)]`
/// `mount_chunk_NN` helpers below, one frame at a time. Collapsing them into a single
/// chained expression holds every service's `new` frame at once and overflows
/// the stack here, aborting before either listener binds.
pub fn build_router(_engine: Arc<InMemoryEngine>) -> MultiServiceRouter {
    MultiServiceRouter::new()
}

