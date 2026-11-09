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
#[path = "all/bucket_policy.rs"]
mod bucket_policy;
#[path = "all/bucket_policy_enforcement.rs"]
mod bucket_policy_enforcement;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/index_mode.rs"]
mod index_mode;
#[path = "all/metadata_budget.rs"]
mod metadata_budget;
#[path = "all/modelled_input_validation.rs"]
mod modelled_input_validation;
#[path = "all/policy_document_bytes.rs"]
mod policy_document_bytes;
#[path = "all/policy_not_found_message.rs"]
mod policy_not_found_message;
#[path = "all/query_candidate_slots.rs"]
mod query_candidate_slots;
#[path = "all/query_pagination.rs"]
mod query_pagination;
#[path = "all/refusal_sentence_wording.rs"]
mod refusal_sentence_wording;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/selector_validation.rs"]
mod selector_validation;
#[path = "all/service_set_tag_keys.rs"]
mod service_set_tag_keys;
#[path = "all/session_policy_scoping.rs"]
mod session_policy_scoping;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_write_cancellation.rs"]
mod tag_write_cancellation;
#[path = "all/tag_write_conflict.rs"]
mod tag_write_conflict;
#[path = "all/vector_data_plane_not_found.rs"]
mod vector_data_plane_not_found;
#[path = "all/vector_lifecycle.rs"]
mod vector_lifecycle;
