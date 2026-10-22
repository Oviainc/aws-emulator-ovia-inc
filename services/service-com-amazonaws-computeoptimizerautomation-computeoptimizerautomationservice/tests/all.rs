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
#[path = "all/automation_event_and_tagging.rs"]
mod automation_event_and_tagging;
#[path = "all/automation_events.rs"]
mod automation_events;
#[path = "all/automation_rule_lifecycle.rs"]
mod automation_rule_lifecycle;
#[path = "all/automation_rule_preview.rs"]
mod automation_rule_preview;
#[path = "all/enrollment_gate.rs"]
mod enrollment_gate;
#[path = "all/error_payloads.rs"]
mod error_payloads;
#[path = "all/recommended_actions.rs"]
mod recommended_actions;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rule_projection.rs"]
mod rule_projection;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
