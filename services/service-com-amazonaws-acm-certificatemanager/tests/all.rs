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
#[path = "all/account_configuration.rs"]
mod account_configuration;
#[path = "all/certificate_domain_validations.rs"]
mod certificate_domain_validations;
#[path = "all/certificate_lifecycle.rs"]
mod certificate_lifecycle;
#[path = "all/certificate_list_filters.rs"]
mod certificate_list_filters;
#[path = "all/certificate_not_found_message.rs"]
mod certificate_not_found_message;
#[path = "all/certificate_revocation.rs"]
mod certificate_revocation;
#[path = "all/certificate_search.rs"]
mod certificate_search;
#[path = "all/certificate_tag_removal.rs"]
mod certificate_tag_removal;
#[path = "all/certificate_validation_state_machine.rs"]
mod certificate_validation_state_machine;
#[path = "all/resend_validation_email.rs"]
mod resend_validation_email;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_domain_superdomain.rs"]
mod validation_domain_superdomain;
