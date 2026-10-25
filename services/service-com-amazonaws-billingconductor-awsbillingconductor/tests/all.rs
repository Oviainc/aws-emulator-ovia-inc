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
#[path = "all/account_association_billing_periods.rs"]
mod account_association_billing_periods;
#[path = "all/account_exclusivity_at_list_bounds.rs"]
mod account_exclusivity_at_list_bounds;
#[path = "all/account_exclusivity_refusal.rs"]
mod account_exclusivity_refusal;
#[path = "all/associate_pricing_rules.rs"]
mod associate_pricing_rules;
#[path = "all/auto_associate_billing_group_cap.rs"]
mod auto_associate_billing_group_cap;
#[path = "all/batch_disassociate_resources_from_custom_line_item.rs"]
mod batch_disassociate_resources_from_custom_line_item;
#[path = "all/billing_group_core_loop.rs"]
mod billing_group_core_loop;
#[path = "all/billing_group_cost_report.rs"]
mod billing_group_cost_report;
#[path = "all/billing_transfer_preference.rs"]
mod billing_transfer_preference;
#[path = "all/custom_line_item_and_create_name_refusals.rs"]
mod custom_line_item_and_create_name_refusals;
#[path = "all/custom_line_item_billing_group_scope.rs"]
mod custom_line_item_billing_group_scope;
#[path = "all/custom_line_item_tag_lifecycle.rs"]
mod custom_line_item_tag_lifecycle;
#[path = "all/delete_custom_line_item.rs"]
mod delete_custom_line_item;
#[path = "all/disassociate_accounts.rs"]
mod disassociate_accounts;
#[path = "all/list_custom_line_item_versions.rs"]
mod list_custom_line_item_versions;
#[path = "all/list_custom_line_items.rs"]
mod list_custom_line_items;
#[path = "all/payer_account_authorization.rs"]
mod payer_account_authorization;
#[path = "all/payer_account_gate.rs"]
mod payer_account_gate;
#[path = "all/payer_gate_precedes_idempotency_existence_and_assembly.rs"]
mod payer_gate_precedes_idempotency_existence_and_assembly;
#[path = "all/percentage_custom_line_item_resource_associations.rs"]
mod percentage_custom_line_item_resource_associations;
#[path = "all/pricing_plan_delete.rs"]
mod pricing_plan_delete;
#[path = "all/pricing_plan_list.rs"]
mod pricing_plan_list;
#[path = "all/pricing_plan_update.rs"]
mod pricing_plan_update;
#[path = "all/pricing_plans_associated_with_pricing_rule.rs"]
mod pricing_plans_associated_with_pricing_rule;
#[path = "all/pricing_rule_association_billing_periods.rs"]
mod pricing_rule_association_billing_periods;
#[path = "all/pricing_rule_association_period_subsets.rs"]
mod pricing_rule_association_period_subsets;
#[path = "all/pricing_rule_association_periods.rs"]
mod pricing_rule_association_periods;
#[path = "all/pricing_rule_billing_periods.rs"]
mod pricing_rule_billing_periods;
#[path = "all/pricing_rule_core_loop.rs"]
mod pricing_rule_core_loop;
#[path = "all/pricing_rule_lifecycle.rs"]
mod pricing_rule_lifecycle;
#[path = "all/pricing_rule_rate_exclusivity.rs"]
mod pricing_rule_rate_exclusivity;
#[path = "all/pricing_rule_update.rs"]
mod pricing_rule_update;
#[path = "all/pricing_rules_associated_to_pricing_plan.rs"]
mod pricing_rules_associated_to_pricing_plan;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_surface.rs"]
mod tagging_surface;
#[path = "all/untag_created_tags.rs"]
mod untag_created_tags;
#[path = "all/update_billing_group.rs"]
mod update_billing_group;
#[path = "all/update_custom_line_item.rs"]
mod update_custom_line_item;
#[path = "all/validation_exception_message.rs"]
mod validation_exception_message;
#[path = "all/validation_messages.rs"]
mod validation_messages;
