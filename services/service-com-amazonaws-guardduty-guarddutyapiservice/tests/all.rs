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
#[path = "all/account_id_well_formedness.rs"]
mod account_id_well_formedness;
#[path = "all/administrator_disassociation.rs"]
mod administrator_disassociation;
#[path = "all/client_rejections.rs"]
mod client_rejections;
#[path = "all/client_token_store.rs"]
mod client_token_store;
#[path = "all/coverage_listing.rs"]
mod coverage_listing;
#[path = "all/coverage_statistics.rs"]
mod coverage_statistics;
#[path = "all/custom_detection_rule_association_listing.rs"]
mod custom_detection_rule_association_listing;
#[path = "all/custom_detection_rule_association_update.rs"]
mod custom_detection_rule_association_update;
#[path = "all/custom_detection_rule_org_configuration_delete.rs"]
mod custom_detection_rule_org_configuration_delete;
#[path = "all/custom_detection_rule_org_configuration_listing.rs"]
mod custom_detection_rule_org_configuration_listing;
#[path = "all/custom_detection_rule_org_configuration_update.rs"]
mod custom_detection_rule_org_configuration_update;
#[path = "all/custom_detection_rules.rs"]
mod custom_detection_rules;
#[path = "all/detector_data_sources_projection.rs"]
mod detector_data_sources_projection;
#[path = "all/detector_lifecycle.rs"]
mod detector_lifecycle;
#[path = "all/detector_sub_resource_creates.rs"]
mod detector_sub_resource_creates;
#[path = "all/filter_delete.rs"]
mod filter_delete;
#[path = "all/filter_lifecycle.rs"]
mod filter_lifecycle;
#[path = "all/filter_listing.rs"]
mod filter_listing;
#[path = "all/filter_update_isolation.rs"]
mod filter_update_isolation;
#[path = "all/finding_field_catalog.rs"]
mod finding_field_catalog;
#[path = "all/findings_archive_shared_rule.rs"]
mod findings_archive_shared_rule;
#[path = "all/findings_archive_state.rs"]
mod findings_archive_state;
#[path = "all/findings_feedback.rs"]
mod findings_feedback;
#[path = "all/findings_statistics.rs"]
mod findings_statistics;
#[path = "all/investigation_lifecycle.rs"]
mod investigation_lifecycle;
#[path = "all/investigation_listing.rs"]
mod investigation_listing;
#[path = "all/invitation_count.rs"]
mod invitation_count;
#[path = "all/invitation_decline.rs"]
mod invitation_decline;
#[path = "all/invitation_delete.rs"]
mod invitation_delete;
#[path = "all/invitation_listing.rs"]
mod invitation_listing;
#[path = "all/ip_set_lifecycle.rs"]
mod ip_set_lifecycle;
#[path = "all/ip_set_listing.rs"]
mod ip_set_listing;
#[path = "all/ip_set_name_alphabet.rs"]
mod ip_set_name_alphabet;
#[path = "all/ip_set_update_and_delete.rs"]
mod ip_set_update_and_delete;
#[path = "all/malware_protection_plan.rs"]
mod malware_protection_plan;
#[path = "all/malware_protection_plan_delete.rs"]
mod malware_protection_plan_delete;
#[path = "all/malware_scan.rs"]
mod malware_scan;
#[path = "all/malware_scan_description.rs"]
mod malware_scan_description;
#[path = "all/malware_scan_lifecycle.rs"]
mod malware_scan_lifecycle;
#[path = "all/malware_scan_settings.rs"]
mod malware_scan_settings;
#[path = "all/master_account_disassociation.rs"]
mod master_account_disassociation;
#[path = "all/member_accounts.rs"]
mod member_accounts;
#[path = "all/member_association.rs"]
mod member_association;
#[path = "all/member_creation.rs"]
mod member_creation;
#[path = "all/member_detector_configuration.rs"]
mod member_detector_configuration;
#[path = "all/member_invitation.rs"]
mod member_invitation;
#[path = "all/member_monitoring.rs"]
mod member_monitoring;
#[path = "all/member_monitoring_restart.rs"]
mod member_monitoring_restart;
#[path = "all/modeled_constraint_refusals.rs"]
mod modeled_constraint_refusals;
#[path = "all/negative_class_rejections.rs"]
mod negative_class_rejections;
#[path = "all/object_malware_scan.rs"]
mod object_malware_scan;
#[path = "all/organization_admin_accounts.rs"]
mod organization_admin_accounts;
#[path = "all/organization_admin_removal.rs"]
mod organization_admin_removal;
#[path = "all/organization_configuration.rs"]
mod organization_configuration;
#[path = "all/organization_managed_members.rs"]
mod organization_managed_members;
#[path = "all/organization_statistics.rs"]
mod organization_statistics;
#[path = "all/publishing_destination_lifecycle.rs"]
mod publishing_destination_lifecycle;
#[path = "all/publishing_destination_listing.rs"]
mod publishing_destination_listing;
#[path = "all/publishing_destination_update.rs"]
mod publishing_destination_update;
#[path = "all/request_region_partition_bounds.rs"]
mod request_region_partition_bounds;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/sample_finding_catalog.rs"]
mod sample_finding_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/threat_entity_set_delete.rs"]
mod threat_entity_set_delete;
#[path = "all/threat_entity_set_lifecycle.rs"]
mod threat_entity_set_lifecycle;
#[path = "all/threat_entity_set_listing.rs"]
mod threat_entity_set_listing;
#[path = "all/threat_intel_set_delete.rs"]
mod threat_intel_set_delete;
#[path = "all/threat_intel_set_listing.rs"]
mod threat_intel_set_listing;
#[path = "all/threat_intel_set_update.rs"]
mod threat_intel_set_update;
#[path = "all/threat_intel_sets.rs"]
mod threat_intel_sets;
#[path = "all/trusted_entity_set_delete.rs"]
mod trusted_entity_set_delete;
#[path = "all/trusted_entity_set_lifecycle.rs"]
mod trusted_entity_set_lifecycle;
#[path = "all/trusted_entity_set_listing.rs"]
mod trusted_entity_set_listing;
#[path = "all/trusted_entity_set_update.rs"]
mod trusted_entity_set_update;
#[path = "all/usage_statistics.rs"]
mod usage_statistics;
