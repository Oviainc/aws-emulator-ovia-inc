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
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_policy_and_tagging.rs"]
mod bucket_policy_and_tagging;
#[path = "all/bucket_tagging.rs"]
mod bucket_tagging;
#[path = "all/head_object_read_back.rs"]
mod head_object_read_back;
#[path = "all/object_and_policy.rs"]
mod object_and_policy;
#[path = "all/object_and_tagging_roundtrip.rs"]
mod object_and_tagging_roundtrip;
#[path = "all/object_write_and_read_back.rs"]
mod object_write_and_read_back;
#[path = "all/objects.rs"]
mod objects;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
