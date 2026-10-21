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
pub fn build_router(engine: Arc<InMemoryEngine>) -> MultiServiceRouter {
    let router = MultiServiceRouter::new();
    let router = mount_chunk_00(router, &engine);
    let router = mount_chunk_01(router, &engine);
    mount_chunk_02(router, &engine)
}

/// Register services 00 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_00(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_account_account::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_accountaccess_awsaccountaccess::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_acm_certificatemanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentregistry_agentregistry::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amp_amazonprometheusservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplify_amplify::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService::new(engine.clone()));
    router
}

/// Register services 01 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_01(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appfabric_fabricfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appintegrations_amazonappintegrationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationcostprofiler_awsapplicationcostprofiler::EmulatedService::new(engine.clone()));
    router
}

/// Register services 02 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_02(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_applicationdiscoveryservice_awsposeidonservice_v2015_11_01::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine.clone()));
    router
}

