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
#[path = "all/account.rs"]
mod account;
#[path = "all/account_suppression_attributes.rs"]
mod account_suppression_attributes;
#[path = "all/configuration_set_event_destinations.rs"]
mod configuration_set_event_destinations;
#[path = "all/configuration_set_event_publication_surface.rs"]
mod configuration_set_event_publication_surface;
#[path = "all/configuration_sets.rs"]
mod configuration_sets;
#[path = "all/contact_lists.rs"]
mod contact_lists;
#[path = "all/email_identity_certificates.rs"]
mod email_identity_certificates;
#[path = "all/email_identity_configuration_set_association.rs"]
mod email_identity_configuration_set_association;
#[path = "all/email_identity_feedback_attributes.rs"]
mod email_identity_feedback_attributes;
#[path = "all/email_identity_lifecycle.rs"]
mod email_identity_lifecycle;
#[path = "all/multi_region_endpoints.rs"]
mod multi_region_endpoints;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/suppressed_destinations.rs"]
mod suppressed_destinations;
#[path = "all/tenant_resource_associations.rs"]
mod tenant_resource_associations;
#[path = "all/tenants.rs"]
mod tenants;
