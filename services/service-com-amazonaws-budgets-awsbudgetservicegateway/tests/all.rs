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
#[path = "all/account_notification_listing.rs"]
mod account_notification_listing;
#[path = "all/account_scoping.rs"]
mod account_scoping;
#[path = "all/budget_action_execution.rs"]
mod budget_action_execution;
#[path = "all/budget_action_history.rs"]
mod budget_action_history;
#[path = "all/budget_action_lifecycle.rs"]
mod budget_action_lifecycle;
#[path = "all/budget_action_lock.rs"]
mod budget_action_lock;
#[path = "all/budget_action_read_purity.rs"]
mod budget_action_read_purity;
#[path = "all/budget_action_settle_history.rs"]
mod budget_action_settle_history;
#[path = "all/budget_action_subscriber_limits.rs"]
mod budget_action_subscriber_limits;
#[path = "all/budget_actions_for_budget.rs"]
mod budget_actions_for_budget;
#[path = "all/budget_creation_limits.rs"]
mod budget_creation_limits;
#[path = "all/budget_end_date_expiry.rs"]
mod budget_end_date_expiry;
#[path = "all/budget_lifecycle.rs"]
mod budget_lifecycle;
#[path = "all/budget_read_projection.rs"]
mod budget_read_projection;
#[path = "all/budget_validation_rules.rs"]
mod budget_validation_rules;
#[path = "all/calculated_spend.rs"]
mod calculated_spend;
#[path = "all/client_error_envelopes.rs"]
mod client_error_envelopes;
#[path = "all/notification_deletion.rs"]
mod notification_deletion;
#[path = "all/notification_lifecycle.rs"]
mod notification_lifecycle;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_owned_budget_members.rs"]
mod service_owned_budget_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subscriber_creation.rs"]
mod subscriber_creation;
#[path = "all/subscriber_notification_reads.rs"]
mod subscriber_notification_reads;
