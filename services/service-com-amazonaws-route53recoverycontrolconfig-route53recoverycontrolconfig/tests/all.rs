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
#[path = "all/body_enum_refusals.rs"]
mod body_enum_refusals;
#[path = "all/common.rs"]
mod common;
#[path = "all/cross_account_authorization.rs"]
mod cross_account_authorization;
#[path = "all/list_clusters.rs"]
mod list_clusters;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/recovery_control_lifecycle.rs"]
mod recovery_control_lifecycle;
#[path = "all/rejection_error_classes.rs"]
mod rejection_error_classes;
#[path = "all/reported_regex.rs"]
mod reported_regex;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/safety_rules.rs"]
mod safety_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_cluster.rs"]
mod update_cluster;
#[path = "all/update_control_panel.rs"]
mod update_control_panel;
