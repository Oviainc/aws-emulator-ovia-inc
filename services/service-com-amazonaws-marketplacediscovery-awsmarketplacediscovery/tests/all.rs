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
#[path = "all/catalog_breadth_and_hierarchy.rs"]
mod catalog_breadth_and_hierarchy;
#[path = "all/catalog_drilldown.rs"]
mod catalog_drilldown;
#[path = "all/catalog_record_breadth.rs"]
mod catalog_record_breadth;
#[path = "all/discovery_ordering_and_refusals.rs"]
mod discovery_ordering_and_refusals;
#[path = "all/fulfillment_options.rs"]
mod fulfillment_options;
#[path = "all/keyword_search_and_paging.rs"]
mod keyword_search_and_paging;
#[path = "all/purchase_option_badges_and_offer_sets.rs"]
mod purchase_option_badges_and_offer_sets;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/search_facets.rs"]
mod search_facets;
#[path = "all/signing_name_claims.rs"]
mod signing_name_claims;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_message_rules.rs"]
mod validation_message_rules;
#[path = "all/whole_value_patterns.rs"]
mod whole_value_patterns;
