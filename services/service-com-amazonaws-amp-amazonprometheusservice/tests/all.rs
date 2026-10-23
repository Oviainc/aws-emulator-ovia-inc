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
#[path = "all/anomaly_detector_lifecycle.rs"]
mod anomaly_detector_lifecycle;
#[path = "all/default_scraper_configuration.rs"]
mod default_scraper_configuration;
#[path = "all/logging_configuration.rs"]
mod logging_configuration;
#[path = "all/query_logging_configuration.rs"]
mod query_logging_configuration;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/rule_groups_namespace_lifecycle.rs"]
mod rule_groups_namespace_lifecycle;
#[path = "all/scraper_listing.rs"]
mod scraper_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire_fidelity.rs"]
mod wire_fidelity;
#[path = "all/workspace_configuration.rs"]
mod workspace_configuration;
#[path = "all/workspace_lifecycle.rs"]
mod workspace_lifecycle;
