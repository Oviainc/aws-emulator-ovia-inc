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
#[path = "all/abac_attribute_configuration.rs"]
mod abac_attribute_configuration;
#[path = "all/account_assignment_creation_status.rs"]
mod account_assignment_creation_status;
#[path = "all/account_assignment_delete.rs"]
mod account_assignment_delete;
#[path = "all/account_instance_restriction.rs"]
mod account_instance_restriction;
#[path = "all/application_access_scope_limits.rs"]
mod application_access_scope_limits;
#[path = "all/application_access_scopes.rs"]
mod application_access_scopes;
#[path = "all/application_assignment_configuration.rs"]
mod application_assignment_configuration;
#[path = "all/application_assignments.rs"]
mod application_assignments;
#[path = "all/application_authentication_methods.rs"]
mod application_authentication_methods;
#[path = "all/application_create.rs"]
mod application_create;
#[path = "all/application_create_providers.rs"]
mod application_create_providers;
#[path = "all/application_grants.rs"]
mod application_grants;
#[path = "all/application_provider_describe.rs"]
mod application_provider_describe;
#[path = "all/application_providers.rs"]
mod application_providers;
#[path = "all/application_update.rs"]
mod application_update;
#[path = "all/customer_managed_policy_references.rs"]
mod customer_managed_policy_references;
#[path = "all/inline_policy.rs"]
mod inline_policy;
#[path = "all/instance_create.rs"]
mod instance_create;
#[path = "all/instance_delete.rs"]
mod instance_delete;
#[path = "all/instance_describe.rs"]
mod instance_describe;
#[path = "all/instance_region_removal.rs"]
mod instance_region_removal;
#[path = "all/instance_region_status.rs"]
mod instance_region_status;
#[path = "all/instance_regions.rs"]
mod instance_regions;
#[path = "all/instance_update.rs"]
mod instance_update;
#[path = "all/list_instances_pagination.rs"]
mod list_instances_pagination;
#[path = "all/managed_policies.rs"]
mod managed_policies;
#[path = "all/managed_policies_in_permission_set.rs"]
mod managed_policies_in_permission_set;
#[path = "all/permission_set_update.rs"]
mod permission_set_update;
#[path = "all/permissions_boundary.rs"]
mod permissions_boundary;
#[path = "all/permissions_boundary_delete.rs"]
mod permissions_boundary_delete;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/ssoadmin_permission_sets.rs"]
mod ssoadmin_permission_sets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trusted_token_issuers.rs"]
mod trusted_token_issuers;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
