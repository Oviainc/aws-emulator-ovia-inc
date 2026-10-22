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
//
// This crate's model is the LEGACY CloudWatch Events name for the API the EventBridge
// model serves, so it no longer claims the `events` key and answers under its own
// sdkId-derived one. Both suites here mount THIS service alone: `rule_lifecycle.rs`
// exercises its handlers under its own key, and `superseded_by_eventbridge.rs` pins that
// its own key is served out of its own store while the `events` key is claimed by
// nothing here. The cross-model claim — that each key is served by exactly one of the
// models publishing this wire identity — needs every claimant mounted and lives in
// `core/router/tests/all/events_store_ownership.rs`.
#[path = "all/rule_lifecycle.rs"]
mod rule_lifecycle;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/superseded_by_eventbridge.rs"]
mod superseded_by_eventbridge;

#[path = "all/support.rs"]
mod support;
