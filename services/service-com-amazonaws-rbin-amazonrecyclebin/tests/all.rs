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
#[path = "all/error_reason_fault_classes.rs"]
mod error_reason_fault_classes;
#[path = "all/error_response_bodies.rs"]
mod error_response_bodies;
#[path = "all/list_rules_minted_page_positions.rs"]
mod list_rules_minted_page_positions;
#[path = "all/list_rules_page_order.rs"]
mod list_rules_page_order;
#[path = "all/list_rules_page_token_authenticity.rs"]
mod list_rules_page_token_authenticity;
#[path = "all/list_rules_settled_lock_state.rs"]
mod list_rules_settled_lock_state;
#[path = "all/lock_rule_idempotence.rs"]
mod lock_rule_idempotence;
#[path = "all/lockable_rule_invariant.rs"]
mod lockable_rule_invariant;
#[path = "all/minted_arn_region_segment.rs"]
mod minted_arn_region_segment;
#[path = "all/not_found_error_body_members.rs"]
mod not_found_error_body_members;
#[path = "all/pattern_requirement_quoting.rs"]
mod pattern_requirement_quoting;
#[path = "all/quota_refusal_bodies.rs"]
mod quota_refusal_bodies;
#[path = "all/regional_rule_isolation.rs"]
mod regional_rule_isolation;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/reserved_tag_values.rs"]
mod reserved_tag_values;
#[path = "all/resource_tag_filter_pair_identity.rs"]
mod resource_tag_filter_pair_identity;
#[path = "all/resource_tag_pair_quota.rs"]
mod resource_tag_pair_quota;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/retention_period_resource_type_bounds.rs"]
mod retention_period_resource_type_bounds;
#[path = "all/retention_rule_fidelity.rs"]
mod retention_rule_fidelity;
#[path = "all/retention_rule_lifecycle.rs"]
mod retention_rule_lifecycle;
#[path = "all/rule_lock_transitions.rs"]
mod rule_lock_transitions;
#[path = "all/rule_not_found_identifier.rs"]
mod rule_not_found_identifier;
#[path = "all/rule_status_transitions.rs"]
mod rule_status_transitions;
#[path = "all/selection_combination_invariant.rs"]
mod selection_combination_invariant;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_arn_label_encoding.rs"]
mod tag_arn_label_encoding;
#[path = "all/unlock_delay_boundedness.rs"]
mod unlock_delay_boundedness;
#[path = "all/update_rule_lock_invariant.rs"]
mod update_rule_lock_invariant;
#[path = "all/update_rule_refusal_atomicity.rs"]
mod update_rule_refusal_atomicity;
