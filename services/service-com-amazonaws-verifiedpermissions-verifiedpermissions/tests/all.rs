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
#[path = "all/batch_get_policy.rs"]
mod batch_get_policy;
#[path = "all/batch_is_authorized.rs"]
mod batch_is_authorized;
#[path = "all/date_time_wire_format.rs"]
mod date_time_wire_format;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identity_sources.rs"]
mod identity_sources;
#[path = "all/policy_store_alias.rs"]
mod policy_store_alias;
#[path = "all/policy_store_authorization.rs"]
mod policy_store_authorization;
#[path = "all/schema.rs"]
mod schema;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging.rs"]
mod tagging;
