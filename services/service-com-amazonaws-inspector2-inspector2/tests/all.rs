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
#[path = "all/account_disable.rs"]
mod account_disable;
#[path = "all/cis_scan_configurations.rs"]
mod cis_scan_configurations;
#[path = "all/ec2_deep_inspection.rs"]
mod ec2_deep_inspection;
#[path = "all/encryption_key.rs"]
mod encryption_key;
#[path = "all/filters_and_enablement.rs"]
mod filters_and_enablement;
#[path = "all/finding_aggregations.rs"]
mod finding_aggregations;
#[path = "all/pattern_refusal_wording.rs"]
mod pattern_refusal_wording;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/usage_totals.rs"]
mod usage_totals;
