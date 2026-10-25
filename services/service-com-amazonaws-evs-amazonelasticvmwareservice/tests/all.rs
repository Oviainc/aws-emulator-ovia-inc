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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/connector_deletion.rs"]
mod connector_deletion;
#[path = "all/connector_secret_arn.rs"]
mod connector_secret_arn;
#[path = "all/connectors.rs"]
mod connectors;
// SUPERSEDED and empty: mainline's `connectors.rs` replaced its connector half and
// `hosts.rs` its host half. Declared only because the layout check requires every file
// under tests/all/ to be declared, and this agent's tools cannot delete a file.
#[path = "all/connectors_and_hosts.rs"]
mod connectors_and_hosts;
#[path = "all/create_environment_network_membership.rs"]
mod create_environment_network_membership;
#[path = "all/create_environment_validation.rs"]
mod create_environment_validation;
#[path = "all/created_environment_hosts.rs"]
mod created_environment_hosts;
#[path = "all/delete_entitlement.rs"]
mod delete_entitlement;
#[path = "all/depot_url.rs"]
mod depot_url;
#[path = "all/eip_allocation_existence.rs"]
mod eip_allocation_existence;
#[path = "all/eip_ec2_propagation.rs"]
mod eip_ec2_propagation;
#[path = "all/entitlement_connector.rs"]
mod entitlement_connector;
#[path = "all/entitlements.rs"]
mod entitlements;
#[path = "all/environment_deletion_hosts.rs"]
mod environment_deletion_hosts;
#[path = "all/environment_health_checks.rs"]
mod environment_health_checks;
#[path = "all/environment_lifecycle.rs"]
mod environment_lifecycle;
#[path = "all/error_wire_status.rs"]
mod error_wire_status;
#[path = "all/hcx_eip_associations.rs"]
mod hcx_eip_associations;
#[path = "all/host_ec2_instances.rs"]
mod host_ec2_instances;
#[path = "all/host_placement_references.rs"]
mod host_placement_references;
#[path = "all/hosts.rs"]
mod hosts;
#[path = "all/list_vm_entitlements_connector.rs"]
mod list_vm_entitlements_connector;
#[path = "all/list_while_initializing.rs"]
mod list_while_initializing;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_not_found.rs"]
mod tagging_not_found;
#[path = "all/tagging_resource_arn_resolution.rs"]
mod tagging_resource_arn_resolution;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/versions_catalog.rs"]
mod versions_catalog;
#[path = "all/vlan_subnet_propagation.rs"]
mod vlan_subnet_propagation;
