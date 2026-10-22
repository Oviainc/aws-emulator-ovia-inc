// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/channel_lifecycle.rs`) — which the layout check refuses. The `#[path]` is
// what points it into `tests/all/`.
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;

#[path = "all/cloud_watch_alarm_template_groups.rs"]
mod cloud_watch_alarm_template_groups;

#[path = "all/event_bridge_rule_template_groups.rs"]
mod event_bridge_rule_template_groups;

#[path = "all/input_lifecycle.rs"]
mod input_lifecycle;

#[path = "all/input_security_group.rs"]
mod input_security_group;

#[path = "all/sdi_source_lifecycle.rs"]
mod sdi_source_lifecycle;

#[path = "all/signal_maps.rs"]
mod signal_maps;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

// Not a suite of its own: the cloudwatch alarm template group request helpers
// the group suites share, layered on `wire`. Declared because every file under
// `tests/all/` must be.
#[path = "all/support.rs"]
mod support;

/// The shared restJson1 harness the suites above drive the service through.
#[path = "all/wire.rs"]
mod wire;
