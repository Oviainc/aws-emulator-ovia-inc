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
#[path = "all/account_meta_data.rs"]
mod account_meta_data;
#[path = "all/batch_tax_registration.rs"]
mod batch_tax_registration;
#[path = "all/country_requirements.rs"]
mod country_requirements;
#[path = "all/country_rules.rs"]
mod country_rules;
#[path = "all/derived_registration_members.rs"]
mod derived_registration_members;
#[path = "all/fidelity_rules.rs"]
mod fidelity_rules;
#[path = "all/listing_page_bounds.rs"]
mod listing_page_bounds;
#[path = "all/missing_resource_errors.rs"]
mod missing_resource_errors;
#[path = "all/non_api_delete_jurisdictions.rs"]
mod non_api_delete_jurisdictions;
#[path = "all/pending_trn_conflict.rs"]
mod pending_trn_conflict;
#[path = "all/put_tax_exemption_state.rs"]
mod put_tax_exemption_state;
#[path = "all/repeated_account_ids.rs"]
mod repeated_account_ids;
#[path = "all/shared_page_builder.rs"]
mod shared_page_builder;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/supplemental_tax_registrations.rs"]
mod supplemental_tax_registrations;
#[path = "all/tax_exemption_types.rs"]
mod tax_exemption_types;
#[path = "all/tax_exemptions.rs"]
mod tax_exemptions;
#[path = "all/tax_registration_lifecycle.rs"]
mod tax_registration_lifecycle;
