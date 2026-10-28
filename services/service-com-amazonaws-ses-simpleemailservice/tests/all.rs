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
#[path = "all/configuration_set_delete.rs"]
mod configuration_set_delete;
#[path = "all/configuration_set_tracking_options.rs"]
mod configuration_set_tracking_options;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identity_mail_from_domain.rs"]
mod identity_mail_from_domain;
#[path = "all/identity_notification_topics.rs"]
mod identity_notification_topics;
#[path = "all/ses_account_sending_and_legacy_addresses.rs"]
mod ses_account_sending_and_legacy_addresses;
#[path = "all/ses_active_receipt_rule_set.rs"]
mod ses_active_receipt_rule_set;
#[path = "all/ses_configuration_set_creation.rs"]
mod ses_configuration_set_creation;
#[path = "all/ses_configuration_set_delivery_options.rs"]
mod ses_configuration_set_delivery_options;
#[path = "all/ses_configuration_set_event_destinations.rs"]
mod ses_configuration_set_event_destinations;
#[path = "all/ses_configuration_set_listing.rs"]
mod ses_configuration_set_listing;
#[path = "all/ses_configuration_set_reputation_metrics.rs"]
mod ses_configuration_set_reputation_metrics;
#[path = "all/ses_configuration_set_sending_state.rs"]
mod ses_configuration_set_sending_state;
#[path = "all/ses_create_receipt_rule.rs"]
mod ses_create_receipt_rule;
#[path = "all/ses_custom_verification_email_send.rs"]
mod ses_custom_verification_email_send;
#[path = "all/ses_identity_policies.rs"]
mod ses_identity_policies;
#[path = "all/ses_identity_policy_delete.rs"]
mod ses_identity_policy_delete;
#[path = "all/ses_identity_send_loop.rs"]
mod ses_identity_send_loop;
#[path = "all/ses_receipt_filters.rs"]
mod ses_receipt_filters;
#[path = "all/ses_receipt_rule_set_creation.rs"]
mod ses_receipt_rule_set_creation;
#[path = "all/ses_receipt_rule_set_deletion.rs"]
mod ses_receipt_rule_set_deletion;
#[path = "all/ses_receipt_rule_set_listing.rs"]
mod ses_receipt_rule_set_listing;
#[path = "all/ses_receipt_rule_sets.rs"]
mod ses_receipt_rule_sets;
#[path = "all/ses_send_raw_email.rs"]
mod ses_send_raw_email;
#[path = "all/ses_store_bounds.rs"]
mod ses_store_bounds;
#[path = "all/ses_template_creation.rs"]
mod ses_template_creation;
#[path = "all/ses_template_delete.rs"]
mod ses_template_delete;
#[path = "all/ses_template_deletion.rs"]
mod ses_template_deletion;
#[path = "all/ses_template_listing.rs"]
mod ses_template_listing;
#[path = "all/ses_template_update.rs"]
mod ses_template_update;
#[path = "all/ses_unverified_sender_refusal.rs"]
mod ses_unverified_sender_refusal;
#[path = "all/ses_unverified_sender_region.rs"]
mod ses_unverified_sender_region;
#[path = "all/ses_update_receipt_rule.rs"]
mod ses_update_receipt_rule;
#[path = "all/ses_validation_refusal_wording.rs"]
mod ses_validation_refusal_wording;
#[path = "all/ses_verify_email_address.rs"]
mod ses_verify_email_address;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The `z`-prefixed service-wide suites sort after the per-operation ones;
// `tests/all.rs` lists suites in file order.
#[path = "all/zprobe.rs"]
mod zprobe;
#[path = "all/zz_dump.rs"]
mod zz_dump;
