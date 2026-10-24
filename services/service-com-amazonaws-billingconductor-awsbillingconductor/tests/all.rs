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
#[path = "all/account_exclusivity_refusal.rs"]
mod account_exclusivity_refusal;
#[path = "all/billing_group_core_loop.rs"]
mod billing_group_core_loop;
#[path = "all/custom_line_item_and_create_name_refusals.rs"]
mod custom_line_item_and_create_name_refusals;
#[path = "all/payer_account_authorization.rs"]
mod payer_account_authorization;
#[path = "all/payer_account_gate.rs"]
mod payer_account_gate;
#[path = "all/payer_gate_precedes_idempotency_existence_and_assembly.rs"]
mod payer_gate_precedes_idempotency_existence_and_assembly;
#[path = "all/percentage_custom_line_item_resource_associations.rs"]
mod percentage_custom_line_item_resource_associations;
#[path = "all/pricing_plan_update.rs"]
mod pricing_plan_update;
#[path = "all/pricing_rule_core_loop.rs"]
mod pricing_rule_core_loop;
#[path = "all/pricing_rule_lifecycle.rs"]
mod pricing_rule_lifecycle;
#[path = "all/pricing_rule_update.rs"]
mod pricing_rule_update;
#[path = "all/pricing_rules_associated_to_pricing_plan.rs"]
mod pricing_rules_associated_to_pricing_plan;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_surface.rs"]
mod tagging_surface;
#[path = "all/validation_exception_message.rs"]
mod validation_exception_message;
#[path = "all/validation_messages.rs"]
mod validation_messages;
