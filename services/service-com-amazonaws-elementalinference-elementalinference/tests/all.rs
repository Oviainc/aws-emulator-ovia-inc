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
#[path = "all/dictionary_lifecycle.rs"]
mod dictionary_lifecycle;
#[path = "all/dictionary_listing.rs"]
mod dictionary_listing;
#[path = "all/dictionary_mutation.rs"]
mod dictionary_mutation;
#[path = "all/error_contract.rs"]
mod error_contract;
#[path = "all/error_message_rule.rs"]
mod error_message_rule;
#[path = "all/feed_association.rs"]
mod feed_association;
#[path = "all/feed_lifecycle.rs"]
mod feed_lifecycle;
#[path = "all/fixture_catalog.rs"]
mod fixture_catalog;
#[path = "all/fixture_search.rs"]
mod fixture_search;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/response_members.rs"]
mod response_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
