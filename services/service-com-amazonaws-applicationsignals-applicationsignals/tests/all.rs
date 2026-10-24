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
#[path = "all/error_identity.rs"]
mod error_identity;
#[path = "all/get_instrumentation_configuration.rs"]
mod get_instrumentation_configuration;
#[path = "all/get_service.rs"]
mod get_service;
#[path = "all/grouping_configuration.rs"]
mod grouping_configuration;
#[path = "all/instrumentation_configurations.rs"]
mod instrumentation_configurations;
#[path = "all/list_services.rs"]
mod list_services;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/sli_metric_type.rs"]
mod sli_metric_type;
#[path = "all/slo_lifecycle.rs"]
mod slo_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/time_ranged_reads.rs"]
mod time_ranged_reads;
