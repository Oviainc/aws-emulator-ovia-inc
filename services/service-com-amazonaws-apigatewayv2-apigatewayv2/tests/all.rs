// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/api_endpoint_scheme.rs"]
mod api_endpoint_scheme;
#[path = "all/api_lifecycle.rs"]
mod api_lifecycle;
#[path = "all/authorizer_lifecycle.rs"]
mod authorizer_lifecycle;
#[path = "all/cors_configuration.rs"]
mod cors_configuration;
#[path = "all/deployment_lifecycle.rs"]
mod deployment_lifecycle;
#[path = "all/integration_lifecycle.rs"]
mod integration_lifecycle;
#[path = "all/integration_pagination.rs"]
mod integration_pagination;
#[path = "all/model_lifecycle.rs"]
mod model_lifecycle;
#[path = "all/protocol_type_gating.rs"]
mod protocol_type_gating;
#[path = "all/route_request_parameter.rs"]
mod route_request_parameter;
#[path = "all/route_response_lifecycle.rs"]
mod route_response_lifecycle;
#[path = "all/route_update.rs"]
mod route_update;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/stage_lifecycle.rs"]
mod stage_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
