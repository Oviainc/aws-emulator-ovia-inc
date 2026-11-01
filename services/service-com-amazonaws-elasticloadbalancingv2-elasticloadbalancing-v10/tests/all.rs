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
#[path = "all/alb_data_plane.rs"]
mod alb_data_plane;
#[path = "all/capacity_reservation.rs"]
mod capacity_reservation;
#[path = "all/catalog_reads.rs"]
mod catalog_reads;
#[path = "all/certificate_resolution_and_redirect_protocol.rs"]
mod certificate_resolution_and_redirect_protocol;
#[path = "all/declared_action_set.rs"]
mod declared_action_set;
#[path = "all/describe_input_bounds.rs"]
mod describe_input_bounds;
#[path = "all/input_rejection_classes.rs"]
mod input_rejection_classes;
#[path = "all/ip_targets.rs"]
mod ip_targets;
#[path = "all/listener_lifecycle_certificates_and_rules.rs"]
mod listener_lifecycle_certificates_and_rules;
#[path = "all/listener_rules.rs"]
mod listener_rules;
#[path = "all/listener_tls_certificates.rs"]
mod listener_tls_certificates;
#[path = "all/load_balancer_attributes.rs"]
mod load_balancer_attributes;
#[path = "all/load_balancer_input_rules.rs"]
mod load_balancer_input_rules;
#[path = "all/load_balancer_network_capacity_and_policy.rs"]
mod load_balancer_network_capacity_and_policy;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/provisioning_loop.rs"]
mod provisioning_loop;
#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rule_deletion_and_condition_value_spellings.rs"]
mod rule_deletion_and_condition_value_spellings;
#[path = "all/rule_modification.rs"]
mod rule_modification;
#[path = "all/rule_priorities.rs"]
mod rule_priorities;
#[path = "all/rule_transforms_and_fixed_response_content_type.rs"]
mod rule_transforms_and_fixed_response_content_type;
#[path = "all/shared_endpoint_routing.rs"]
mod shared_endpoint_routing;
#[path = "all/ssl_policies.rs"]
mod ssl_policies;
#[path = "all/ssl_policy_families.rs"]
mod ssl_policy_families;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subnet_vpc_membership.rs"]
mod subnet_vpc_membership;
#[path = "all/tag_not_found_wording.rs"]
mod tag_not_found_wording;
#[path = "all/tag_not_found_wording_by_kind.rs"]
mod tag_not_found_wording_by_kind;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/tag_writes.rs"]
mod tag_writes;
#[path = "all/target_group_attribute_writes.rs"]
mod target_group_attribute_writes;
#[path = "all/target_group_attributes_and_modification.rs"]
mod target_group_attributes_and_modification;
#[path = "all/target_group_health_check_port.rs"]
mod target_group_health_check_port;
#[path = "all/target_group_health_check_port_refusal.rs"]
mod target_group_health_check_port_refusal;
#[path = "all/target_group_health_checks.rs"]
mod target_group_health_checks;
#[path = "all/target_group_in_use_refusal.rs"]
mod target_group_in_use_refusal;
#[path = "all/target_group_layer_7_health_check.rs"]
mod target_group_layer_7_health_check;
#[path = "all/target_validation_and_quotas.rs"]
mod target_validation_and_quotas;
#[path = "all/trust_stores.rs"]
mod trust_stores;
#[path = "all/wire.rs"]
mod wire;
