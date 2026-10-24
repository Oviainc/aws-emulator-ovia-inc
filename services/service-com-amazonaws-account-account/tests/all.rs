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
#[path = "all/account_information_wire_fidelity.rs"]
mod account_information_wire_fidelity;
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/alternate_contact_delete_precedence.rs"]
mod alternate_contact_delete_precedence;
#[path = "all/contact_information.rs"]
mod contact_information;
#[path = "all/enum_input_validation.rs"]
mod enum_input_validation;
#[path = "all/pattern_validation_messages.rs"]
mod pattern_validation_messages;
#[path = "all/phone_number_verification.rs"]
mod phone_number_verification;
#[path = "all/primary_email_update.rs"]
mod primary_email_update;
#[path = "all/primary_email_update_acceptance.rs"]
mod primary_email_update_acceptance;
#[path = "all/region_opt_in.rs"]
mod region_opt_in;
#[path = "all/region_opt_in_listing.rs"]
mod region_opt_in_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_account_refusal.rs"]
mod unknown_account_refusal;
