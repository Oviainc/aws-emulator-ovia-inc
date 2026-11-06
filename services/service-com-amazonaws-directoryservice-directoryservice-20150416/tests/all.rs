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
#[path = "all/ad_assessment.rs"]
mod ad_assessment;
#[path = "all/ad_connector.rs"]
mod ad_connector;
#[path = "all/certificate_deregistration.rs"]
mod certificate_deregistration;
#[path = "all/certificate_listing.rs"]
mod certificate_listing;
#[path = "all/certificates.rs"]
mod certificates;
#[path = "all/computer_attributes.rs"]
mod computer_attributes;
#[path = "all/computers.rs"]
mod computers;
#[path = "all/conditional_forwarder_deletion.rs"]
mod conditional_forwarder_deletion;
#[path = "all/conditional_forwarder_update.rs"]
mod conditional_forwarder_update;
#[path = "all/conditional_forwarders.rs"]
mod conditional_forwarders;
#[path = "all/directory_data_access.rs"]
mod directory_data_access;
#[path = "all/directory_lifecycle.rs"]
mod directory_lifecycle;
#[path = "all/directory_limits.rs"]
mod directory_limits;
#[path = "all/directory_networking_and_settings.rs"]
mod directory_networking_and_settings;
#[path = "all/directory_settings.rs"]
mod directory_settings;
#[path = "all/directory_unsharing.rs"]
mod directory_unsharing;
#[path = "all/domain_controller_resize.rs"]
mod domain_controller_resize;
#[path = "all/domain_controllers.rs"]
mod domain_controllers;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/event_topic_deregistration.rs"]
mod event_topic_deregistration;
#[path = "all/event_topics.rs"]
mod event_topics;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/ip_route_removal.rs"]
mod ip_route_removal;
#[path = "all/ldaps.rs"]
mod ldaps;
#[path = "all/log_subscriptions.rs"]
mod log_subscriptions;
#[path = "all/microsoft_ad.rs"]
mod microsoft_ad;
#[path = "all/pagination_token_refusal.rs"]
mod pagination_token_refusal;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/region_replication.rs"]
mod region_replication;
#[path = "all/request_id_header.rs"]
mod request_id_header;
#[path = "all/schema_extension_cancellation.rs"]
mod schema_extension_cancellation;
#[path = "all/schema_extensions.rs"]
mod schema_extensions;
#[path = "all/shared_directories.rs"]
mod shared_directories;
#[path = "all/single_sign_on.rs"]
mod single_sign_on;
#[path = "all/snapshot_deletion.rs"]
mod snapshot_deletion;
#[path = "all/snapshots.rs"]
mod snapshots;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trust_conflict_message.rs"]
mod trust_conflict_message;
#[path = "all/trusts.rs"]
mod trusts;
#[path = "all/user_passwords.rs"]
mod user_passwords;
