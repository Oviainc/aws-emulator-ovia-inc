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
#[path = "all/arn_parameter_refusals.rs"]
mod arn_parameter_refusals;
#[path = "all/component_image_recipe.rs"]
mod component_image_recipe;
#[path = "all/component_policy.rs"]
mod component_policy;
#[path = "all/image_packages.rs"]
mod image_packages;
#[path = "all/image_pipeline_execution.rs"]
mod image_pipeline_execution;
#[path = "all/infrastructure_configuration.rs"]
mod infrastructure_configuration;
#[path = "all/resource_not_found.rs"]
mod resource_not_found;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
