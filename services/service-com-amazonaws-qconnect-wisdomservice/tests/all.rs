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
#[path = "all/assistant_association.rs"]
mod assistant_association;
#[path = "all/assistant_configuration.rs"]
mod assistant_configuration;
#[path = "all/assistant_lifecycle.rs"]
mod assistant_lifecycle;
#[path = "all/authorization_precedence.rs"]
mod authorization_precedence;
#[path = "all/content_upload.rs"]
mod content_upload;
#[path = "all/enum_value_refusal.rs"]
mod enum_value_refusal;
#[path = "all/knowledge_base_lifecycle.rs"]
mod knowledge_base_lifecycle;
#[path = "all/modelled_refusals.rs"]
mod modelled_refusals;
#[path = "all/session_recommendations.rs"]
mod session_recommendations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
