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
#[path = "all/domain_repository_lifecycle.rs"]
mod domain_repository_lifecycle;
#[path = "all/external_connection.rs"]
mod external_connection;
#[path = "all/package_group_hierarchy.rs"]
mod package_group_hierarchy;
#[path = "all/package_listing.rs"]
mod package_listing;
#[path = "all/repository_endpoint.rs"]
mod repository_endpoint;
#[path = "all/repository_permissions_policy.rs"]
mod repository_permissions_policy;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

// Declared last rather than in sorted position, deliberately: every concurrent change to
// this crate adds its suite in sorted order, so an entry appended here is the one that
// never collides with theirs when two of them land together. The build only requires that
// a suite under `tests/all/` be declared (see `vendor/build-support`); ordering is a
// readability convention, and the sorted list above is intact.
#[path = "all/external_connection_disassociation.rs"]
mod external_connection_disassociation;
