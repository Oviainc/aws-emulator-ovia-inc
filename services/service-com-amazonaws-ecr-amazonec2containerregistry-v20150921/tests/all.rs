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
#[path = "all/layer_availability.rs"]
mod layer_availability;
#[path = "all/pull_time_update_exclusions.rs"]
mod pull_time_update_exclusions;
#[path = "all/registry_policy.rs"]
mod registry_policy;
#[path = "all/repository_image_lifecycle.rs"]
mod repository_image_lifecycle;
#[path = "all/resource_tag_mutations.rs"]
mod resource_tag_mutations;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scanning_configuration.rs"]
mod scanning_configuration;
#[path = "all/signing_configuration.rs"]
mod signing_configuration;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire.rs"]
mod wire;
