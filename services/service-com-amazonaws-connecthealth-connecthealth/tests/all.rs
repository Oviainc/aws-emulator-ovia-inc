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
#[path = "all/arn_resolution_during_domain_deletion.rs"]
mod arn_resolution_during_domain_deletion;
#[path = "all/domain_account_cap_and_lookup_semantics.rs"]
mod domain_account_cap_and_lookup_semantics;
#[path = "all/domain_response_projection.rs"]
mod domain_response_projection;
#[path = "all/domain_subscription_lifecycle.rs"]
mod domain_subscription_lifecycle;
#[path = "all/error_detail_member.rs"]
mod error_detail_member;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/pagination_token_issuance.rs"]
mod pagination_token_issuance;
#[path = "all/patient_insights_job_lifecycle.rs"]
mod patient_insights_job_lifecycle;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/reserved_tag_key_prefix.rs"]
mod reserved_tag_key_prefix;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/start_patient_insights_job_write_discipline.rs"]
mod start_patient_insights_job_write_discipline;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_map_bounds.rs"]
mod tag_map_bounds;
