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
#[path = "all/alias_viewer_certificate.rs"]
mod alias_viewer_certificate;
#[path = "all/cache_policy_lifecycle.rs"]
mod cache_policy_lifecycle;
#[path = "all/cache_policy_list.rs"]
mod cache_policy_list;
#[path = "all/cache_policy_update.rs"]
mod cache_policy_update;
#[path = "all/connection_function_lifecycle.rs"]
mod connection_function_lifecycle;
#[path = "all/connection_function_publish.rs"]
mod connection_function_publish;
#[path = "all/connection_group_tags.rs"]
mod connection_group_tags;
#[path = "all/connection_groups.rs"]
mod connection_groups;
#[path = "all/continuous_deployment_policies.rs"]
mod continuous_deployment_policies;
#[path = "all/create_location_header.rs"]
mod create_location_header;
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
#[path = "all/distribution_web_acl_association.rs"]
mod distribution_web_acl_association;
#[path = "all/distributions_by_cache_policy_id.rs"]
mod distributions_by_cache_policy_id;
#[path = "all/distributions_by_connection_mode.rs"]
mod distributions_by_connection_mode;
#[path = "all/distributions_by_origin_request_policy_id.rs"]
mod distributions_by_origin_request_policy_id;
#[path = "all/distributions_by_realtime_log_config.rs"]
mod distributions_by_realtime_log_config;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/function_lifecycle.rs"]
mod function_lifecycle;
#[path = "all/function_update.rs"]
mod function_update;
#[path = "all/invalidation_list.rs"]
mod invalidation_list;
#[path = "all/key_value_store_lifecycle.rs"]
mod key_value_store_lifecycle;
#[path = "all/key_value_store_update.rs"]
mod key_value_store_update;
#[path = "all/managed_cache_policy_catalog.rs"]
mod managed_cache_policy_catalog;
#[path = "all/managed_certificate_details.rs"]
mod managed_certificate_details;
#[path = "all/monitoring_subscription.rs"]
mod monitoring_subscription;
#[path = "all/origin_access_control_delete.rs"]
mod origin_access_control_delete;
#[path = "all/origin_access_control_lifecycle.rs"]
mod origin_access_control_lifecycle;
#[path = "all/origin_access_control_update.rs"]
mod origin_access_control_update;
#[path = "all/origin_access_identity_lifecycle.rs"]
mod origin_access_identity_lifecycle;
#[path = "all/origin_access_identity_list.rs"]
mod origin_access_identity_list;
#[path = "all/origin_access_identity_update.rs"]
mod origin_access_identity_update;
#[path = "all/origin_request_policy_config_read.rs"]
mod origin_request_policy_config_read;
#[path = "all/origin_request_policy_lifecycle.rs"]
mod origin_request_policy_lifecycle;
#[path = "all/origin_request_policy_list.rs"]
mod origin_request_policy_list;
#[path = "all/public_key_lifecycle.rs"]
mod public_key_lifecycle;
#[path = "all/realtime_log_config_lifecycle.rs"]
mod realtime_log_config_lifecycle;
#[path = "all/response_headers_policy_lifecycle.rs"]
mod response_headers_policy_lifecycle;
#[path = "all/staging_config_promotion.rs"]
mod staging_config_promotion;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/streaming_distribution_list.rs"]
mod streaming_distribution_list;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tenant_distribution_and_method_order.rs"]
mod tenant_distribution_and_method_order;
#[path = "all/tenant_invalidations.rs"]
mod tenant_invalidations;
#[path = "all/tenant_web_acl_association.rs"]
mod tenant_web_acl_association;
#[path = "all/verify_dns_configuration.rs"]
mod verify_dns_configuration;
#[path = "all/web_acl_association.rs"]
mod web_acl_association;
