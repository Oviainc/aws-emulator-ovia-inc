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
#[path = "all/application_metadata.rs"]
mod application_metadata;
#[path = "all/application_mutations.rs"]
mod application_mutations;
#[path = "all/application_policies.rs"]
mod application_policies;
#[path = "all/applications.rs"]
mod applications;
#[path = "all/artifact_document_plane.rs"]
mod artifact_document_plane;
#[path = "all/change_sets.rs"]
mod change_sets;
#[path = "all/cloudformation_templates.rs"]
mod cloudformation_templates;
#[path = "all/deploy_refusal_messages.rs"]
mod deploy_refusal_messages;
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/presigned_locators.rs"]
mod presigned_locators;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/version_publish_surface.rs"]
mod version_publish_surface;
#[path = "all/version_templates.rs"]
mod version_templates;
#[path = "all/wire_probe.rs"]
mod wire_probe;
#[path = "all/yaml_templates.rs"]
mod yaml_templates;
