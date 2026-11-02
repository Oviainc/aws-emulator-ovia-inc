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
#[path = "all/arn_identifier_lifting.rs"]
mod arn_identifier_lifting;
#[path = "all/clock_read_inside_the_store_lock.rs"]
mod clock_read_inside_the_store_lock;
#[path = "all/close_tunnel_idempotence.rs"]
mod close_tunnel_idempotence;
#[path = "all/closed_tunnel_shape.rs"]
mod closed_tunnel_shape;
#[path = "all/creation_order_agrees_with_created_at.rs"]
mod creation_order_agrees_with_created_at;
#[path = "all/destination_service_limit.rs"]
mod destination_service_limit;
#[path = "all/limit_refusals.rs"]
mod limit_refusals;
#[path = "all/list_tunnels_next_token_form.rs"]
mod list_tunnels_next_token_form;
#[path = "all/list_tunnels_page_token_scope.rs"]
mod list_tunnels_page_token_scope;
#[path = "all/member_update_stamp.rs"]
mod member_update_stamp;
#[path = "all/minted_tunnel_id_form.rs"]
mod minted_tunnel_id_form;
#[path = "all/owed_close_settled_by_every_entry_point.rs"]
mod owed_close_settled_by_every_entry_point;
#[path = "all/record_eviction_order.rs"]
mod record_eviction_order;
#[path = "all/repeated_tag_key_in_one_write.rs"]
mod repeated_tag_key_in_one_write;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/rotate_refusal_error_contract.rs"]
mod rotate_refusal_error_contract;
#[path = "all/shared_tag_limit_rule.rs"]
mod shared_tag_limit_rule;
#[path = "all/stamp_never_precedes_the_owed_close.rs"]
mod stamp_never_precedes_the_owed_close;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_count.rs"]
mod tag_key_count;
#[path = "all/tag_write_stamps.rs"]
mod tag_write_stamps;
#[path = "all/tunnel_concurrency_quota.rs"]
mod tunnel_concurrency_quota;
#[path = "all/tunnel_lifecycle.rs"]
mod tunnel_lifecycle;
#[path = "all/tunnel_refusal_rules.rs"]
mod tunnel_refusal_rules;
#[path = "all/tunnel_region_isolation.rs"]
mod tunnel_region_isolation;
#[path = "all/tunnel_tag_projection.rs"]
mod tunnel_tag_projection;
