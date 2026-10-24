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
#[path = "all/cache_policy_lifecycle.rs"]
mod cache_policy_lifecycle;
#[path = "all/cache_policy_update.rs"]
mod cache_policy_update;
#[path = "all/continuous_deployment_policies.rs"]
mod continuous_deployment_policies;
#[path = "all/distribution_config_defaults.rs"]
mod distribution_config_defaults;
#[path = "all/distribution_config_read.rs"]
mod distribution_config_read;
#[path = "all/distribution_copy.rs"]
mod distribution_copy;
#[path = "all/distribution_delete_gate.rs"]
mod distribution_delete_gate;
#[path = "all/distribution_lifecycle.rs"]
mod distribution_lifecycle;
#[path = "all/distribution_tags.rs"]
mod distribution_tags;
#[path = "all/distribution_tenant_lifecycle.rs"]
mod distribution_tenant_lifecycle;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/function_lifecycle.rs"]
mod function_lifecycle;
#[path = "all/invalidation_list.rs"]
mod invalidation_list;
#[path = "all/key_value_store_lifecycle.rs"]
mod key_value_store_lifecycle;
#[path = "all/key_value_store_update.rs"]
mod key_value_store_update;
#[path = "all/monitoring_subscription.rs"]
mod monitoring_subscription;
#[path = "all/origin_access_control_lifecycle.rs"]
mod origin_access_control_lifecycle;
#[path = "all/origin_access_identity_lifecycle.rs"]
mod origin_access_identity_lifecycle;
#[path = "all/origin_request_policy_lifecycle.rs"]
mod origin_request_policy_lifecycle;
#[path = "all/response_headers_policy_lifecycle.rs"]
mod response_headers_policy_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tenant_distribution_and_method_order.rs"]
mod tenant_distribution_and_method_order;
#[path = "all/web_acl_association.rs"]
mod web_acl_association;
