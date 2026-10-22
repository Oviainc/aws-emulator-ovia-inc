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
#[path = "all/certificate_authority_listing.rs"]
mod certificate_authority_listing;
#[path = "all/certificate_authority_tags.rs"]
mod certificate_authority_tags;
#[path = "all/delete_certificate_authority.rs"]
mod delete_certificate_authority;
#[path = "all/permissions.rs"]
mod permissions;
#[path = "all/policy_documents.rs"]
mod policy_documents;
#[path = "all/private_ca_lifecycle.rs"]
mod private_ca_lifecycle;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_policy_read.rs"]
mod resource_policy_read;
#[path = "all/restore_certificate_authority.rs"]
mod restore_certificate_authority;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_listing.rs"]
mod tag_listing;
