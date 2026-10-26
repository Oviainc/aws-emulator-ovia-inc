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
#[path = "all/identity_notification_topics.rs"]
mod identity_notification_topics;
#[path = "all/ses_account_sending_and_legacy_addresses.rs"]
mod ses_account_sending_and_legacy_addresses;
#[path = "all/ses_configuration_set_listing.rs"]
mod ses_configuration_set_listing;
#[path = "all/ses_identity_policies.rs"]
mod ses_identity_policies;
#[path = "all/ses_identity_send_loop.rs"]
mod ses_identity_send_loop;
#[path = "all/ses_receipt_filters.rs"]
mod ses_receipt_filters;
#[path = "all/ses_receipt_rule_set_listing.rs"]
mod ses_receipt_rule_set_listing;
#[path = "all/ses_receipt_rule_sets.rs"]
mod ses_receipt_rule_sets;
#[path = "all/ses_store_bounds.rs"]
mod ses_store_bounds;
#[path = "all/ses_template_creation.rs"]
mod ses_template_creation;
#[path = "all/ses_template_deletion.rs"]
mod ses_template_deletion;
#[path = "all/ses_template_listing.rs"]
mod ses_template_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The `z`-prefixed service-wide suites sort after the per-operation ones;
// `tests/all.rs` lists suites in file order.
#[path = "all/zprobe.rs"]
mod zprobe;
#[path = "all/zz_dump.rs"]
mod zz_dump;
