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
#[path = "all/invoice_profiles.rs"]
mod invoice_profiles;
#[path = "all/invoice_summaries.rs"]
mod invoice_summaries;
#[path = "all/invoice_unit_lifecycle.rs"]
mod invoice_unit_lifecycle;
#[path = "all/procurement_portal_error_message_key.rs"]
mod procurement_portal_error_message_key;
#[path = "all/procurement_portal_preference_lifecycle.rs"]
mod procurement_portal_preference_lifecycle;
#[path = "all/procurement_portal_preference_reads.rs"]
mod procurement_portal_preference_reads;
#[path = "all/procurement_portal_preferences.rs"]
mod procurement_portal_preferences;
#[path = "all/retired_units_and_portal_validation.rs"]
mod retired_units_and_portal_validation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
#[path = "all/wire_envelope_and_error_detail.rs"]
mod wire_envelope_and_error_detail;
