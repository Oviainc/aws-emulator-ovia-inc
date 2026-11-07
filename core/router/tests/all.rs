// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for the router.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/boot_serve.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
//
// This first entry is out of alphabetical order on purpose: both the appended region
// at the end of this file and the span of the sorted list beside a suite's own name
// are where concurrently landing suites add their line, so a declaration placed there
// collides with every one of them. The head of the list is the one region nothing
// else appends to.
#[path = "all/cloudformation_fifo_resource_generated_names.rs"]
mod cloudformation_fifo_resource_generated_names;
// Declared here, in this same head region, for the reason the entry above records: the
// appended tail and the sorted `d…` span are both regions concurrently landing suites add
// their line to.
#[path = "all/directory_data_plane_authorization.rs"]
mod directory_data_plane_authorization;
// appended tail and the sorted `k…` span are both regions concurrently landing suites add
// their line to.
#[path = "all/kinesisvideo_discovered_data_plane.rs"]
mod kinesisvideo_discovered_data_plane;
// Declared here, in this same head region, for the reason the entry above records.
#[path = "all/cloudformation_ec2_cdk_vpc_estate.rs"]
mod cloudformation_ec2_cdk_vpc_estate;
#[path = "all/greengrass_deployment_iot_job.rs"]
mod greengrass_deployment_iot_job;
// Declared here, in this same head region, for the reason the entry above records: the
// appended tail and the sorted `c…` span (where every cloudformation suite lands) are both
// regions concurrently landing suites add their line to.
#[path = "all/cloudformation_sam_function_writes_its_table.rs"]
mod cloudformation_sam_function_writes_its_table;
// Declared here, in this same head region, for the reason the entry above records.
// What S3's two address forms — path-style and virtual-hosted — mean to the router, which is
// the rule every cross-service suite that stages an S3 object addresses its setup calls by.
#[path = "all/s3_address_forms.rs"]
mod s3_address_forms;
#[path = "all/cloudformation_refuses_s3vectors_vector_buckets.rs"]
mod cloudformation_refuses_s3vectors_vector_buckets;
// Declared here, in this same head region, for the reason the entry above records: the
// appended tail and the sorted `a…` span (where the other apigateway suites land) are both
// regions concurrently landing suites add their line to.
#[path = "all/apigateway_cognito_user_pools_authorizer.rs"]
mod apigateway_cognito_user_pools_authorizer;
#[path = "all/apigateway_iam_authorized_invoke.rs"]
mod apigateway_iam_authorized_invoke;
// appended tail and the sorted `e…` span are both regions concurrently landing suites add
// their line to.
#[path = "all/ebs_start_snapshot_resolves_its_kms_key.rs"]
mod ebs_start_snapshot_resolves_its_kms_key;
// appended tail and the sorted `s…` span (where every sqs suite lands) are both regions
// concurrently landing suites add their line to.
#[path = "all/sqs_queue_policy_governs_cross_service_delivery.rs"]
mod sqs_queue_policy_governs_cross_service_delivery;
// Declared here, at the head, for the reason the entry above records: the appended tail and
// the sorted `a…` span (beside the other application* suites) are both regions concurrently
// landing suites add their line to.
#[path = "all/applicationinsights_resource_group_admission.rs"]
mod applicationinsights_resource_group_admission;
// Declared at the head for the reason the entry above records: the appended tail and the
// sorted `s…` span (where the other sagemaker suites land) are both regions concurrently
#[path = "all/sagemaker_create_admission.rs"]
mod sagemaker_create_admission;
// Declared in this same head region, for the reason the entry above records: the appended
// tail and the sorted `s…` span (where the other sagemaker suites land) are both regions
#[path = "all/sagemaker_execution_role_admission.rs"]
mod sagemaker_execution_role_admission;
// the sorted `g…` span (beside the other greengrass/glue suites) are both regions
#[path = "all/greengrass_signing_name_routing.rs"]
mod greengrass_signing_name_routing;
// the sorted `c…` span (where every cloudformation suite lands) are both regions
#[path = "all/cloudformation_deployed_domain_index_lifecycle.rs"]
mod cloudformation_deployed_domain_index_lifecycle;
// the sorted `g…` span (beside the other glue suites) are both regions concurrently landing
// suites add their line to.
#[path = "all/glue_studio_notebook_catalog_through_the_router.rs"]
mod glue_studio_notebook_catalog_through_the_router;
// the sorted `e…` span (where every elbv2 suite lands) are both regions concurrently landing
#[path = "all/elbv2_dualstack_and_security_group_classes.rs"]
mod elbv2_dualstack_and_security_group_classes;
#[path = "all/cloudformation_provisions_codeartifact_resources.rs"]
mod cloudformation_provisions_codeartifact_resources;
// tail and the sorted `c…` span (where every cloudformation suite lands) are both regions
#[path = "all/cloudformation_provisions_glue_catalog.rs"]
mod cloudformation_provisions_glue_catalog;
// Declared in this same head region, for the reason the entries above record: the appended
// tail and the sorted `s…` span (where every ssm suite would land) are both regions
#[path = "all/ssm_securestring_ciphertext_decrypts_in_kms.rs"]
mod ssm_securestring_ciphertext_decrypts_in_kms;
// the sorted `s…` span (where every ses suite lands) are both regions concurrently landing
#[path = "all/ses_raw_send_under_a_shared_domain_identity.rs"]
mod ses_raw_send_under_a_shared_domain_identity;
// Declared here, at the head, for the reason the entries below record: the appended tail
// and the sorted `c…` span (where every codepipeline and cloudformation suite would land)
// are both regions concurrently landing suites add their line to.
#[path = "all/codepipeline_execution_controls.rs"]
mod codepipeline_execution_controls;
// tail and the sorted `c…` span (where every codepipeline suite would land) are both regions
#[path = "all/codepipeline_management_events.rs"]
mod codepipeline_management_events;
// and the sorted `i…` span are both regions concurrently landing suites add their line to.
#[path = "all/iam_management_events.rs"]
mod iam_management_events;
// tail and the sorted `s…` span are both regions concurrently landing suites add their line
// to. `sts_management_events` pins that an STS call — a read-only one included — is in
// CloudTrail event history, and that no credential material reaches a record.
#[path = "all/sts_management_events.rs"]
mod sts_management_events;
// and the sorted `a…` span are both regions concurrently landing suites add their line to.
#[path = "all/assembled_router_refusal_identities.rs"]
mod assembled_router_refusal_identities;
#[path = "all/cloudformation_apigateway_custom_domains.rs"]
mod cloudformation_apigateway_custom_domains;
#[path = "all/cloudformation_provisions_guardduty_ip_set.rs"]
mod cloudformation_provisions_guardduty_ip_set;
// Declared in this same head region, for the reason the entries above record.
#[path = "all/cloudformation_provisions_wafv2_web_acl_associations.rs"]
mod cloudformation_provisions_wafv2_web_acl_associations;
// Declared here, at the head, for the reason the entry below records: the appended tail and
// the sorted `k…` span are both regions concurrently landing suites add their line to.
#[path = "all/kinesisvideo_signing_name_routing.rs"]
mod kinesisvideo_signing_name_routing;
// the sorted `k…` span (beside the other kafka/kinesis suites) are both regions concurrently
#[path = "all/kafka_placement_subnets_resolve_in_ec2.rs"]
mod kafka_placement_subnets_resolve_in_ec2;
// the sorted `b…` span (where every batch suite would land) are both regions concurrently
#[path = "all/batch_job_state_change_is_scoped_to_its_own_job_queue.rs"]
mod batch_job_state_change_is_scoped_to_its_own_job_queue;
// the sorted `t…` span are both regions concurrently landing suites add their line to.
#[path = "all/tagged_hosted_zone_selects_a_sending_domain.rs"]
mod tagged_hosted_zone_selects_a_sending_domain;
// the sorted `c…` span (where every codeartifact and cloudformation suite would land) are
// both regions concurrently landing suites add their line to.
#[path = "all/codeartifact_package_version_events.rs"]
mod codeartifact_package_version_events;
#[path = "all/cloudformation_opensearch_domain_access_policies.rs"]
mod cloudformation_opensearch_domain_access_policies;
#[path = "all/cloudformation_opensearch_domain_deployments.rs"]
mod cloudformation_opensearch_domain_deployments;
#[path = "all/cloudformation_opensearch_vpc_endpoint_access.rs"]
mod cloudformation_opensearch_vpc_endpoint_access;
#[path = "all/cloudformation_search_indexing_pipeline_outputs.rs"]
mod cloudformation_search_indexing_pipeline_outputs;
#[path = "all/cloudformation_ec2_ebs_instance_estate.rs"]
mod cloudformation_ec2_ebs_instance_estate;
// the sorted `k…` span (beside the other kinesis suites) are both regions concurrently
#[path = "all/kinesisanalytics_signing_name_routing.rs"]
mod kinesisanalytics_signing_name_routing;
// the sorted `s…` span (where the other s3 suites land) are both regions concurrently
#[path = "all/s3control_tags_a_general_purpose_bucket.rs"]
mod s3control_tags_a_general_purpose_bucket;
#[path = "all/cloudformation_provisions_kinesis_streams.rs"]
mod cloudformation_provisions_kinesis_streams;
// the sorted `e…` span (where the other ecr/eks suites land) are both regions concurrently
#[path = "all/ecr_public_action_scoped_iam_identity.rs"]
mod ecr_public_action_scoped_iam_identity;
// the sorted `a…` span (where the other apigateway suites land) are both regions
#[path = "all/apigateway_private_vpce_invoke.rs"]
mod apigateway_private_vpce_invoke;
#[path = "all/apigateway_s3_service_integration.rs"]
mod apigateway_s3_service_integration;
// the sorted `b…` span (where the other bedrock suites land) are both regions concurrently
#[path = "all/bedrock_model_customization_pass_role.rs"]
mod bedrock_model_customization_pass_role;
#[path = "all/bedrock_model_invocation_logging_pass_role.rs"]
mod bedrock_model_invocation_logging_pass_role;
#[path = "all/cloudformation_firehose_stream_stabilisation.rs"]
mod cloudformation_firehose_stream_stabilisation;
// Declared beside the suite above rather than in the head region: both are regions
// concurrently landing suites add their line to, and this one pins the other half of that
// suite's rule — a stack operation is not reported complete while a resource it DELETED is
// still held by the service that owned it.
#[path = "all/cloudformation_resource_removal_stabilisation.rs"]
mod cloudformation_resource_removal_stabilisation;
#[path = "all/cloudformation_provisions_opensearch_domains.rs"]
mod cloudformation_provisions_opensearch_domains;
#[path = "all/cloudformation_provisions_glue_jobs.rs"]
mod cloudformation_provisions_glue_jobs;
#[path = "all/cloudformation_glue_job_redeploy_across_regions.rs"]
mod cloudformation_glue_job_redeploy_across_regions;
#[path = "all/cloudformation_provisions_backup_resources.rs"]
mod cloudformation_provisions_backup_resources;
#[path = "all/cloudformation_provisions_route53_health_checks.rs"]
mod cloudformation_provisions_route53_health_checks;
#[path = "all/cloudformation_provisions_kms_aliases.rs"]
mod cloudformation_provisions_kms_aliases;
// Declared beside the entry above, at the head, for the same reason: the appended tail and
// the sorted `c…` span (where the other cloudwatch/cloudformation suites land) are both
#[path = "all/cloudwatch_alarm_action_reaches_its_subscribed_queue.rs"]
mod cloudwatch_alarm_action_reaches_its_subscribed_queue;
#[path = "all/cloudwatch_breaching_alarm_fires_through_the_whole_router.rs"]
mod cloudwatch_breaching_alarm_fires_through_the_whole_router;
// Declared beside the entry above, at the head, for the same reason.
#[path = "all/cloudwatch_composite_alarm_rule_decides_its_sns_action.rs"]
mod cloudwatch_composite_alarm_rule_decides_its_sns_action;
// the sorted `c…` span (where the other cloudwatch suites land) are both regions concurrently
#[path = "all/contributor_insights_log_events.rs"]
mod contributor_insights_log_events;
// the sorted `c…` span (where every cloudformation suite lands) are both regions concurrently
#[path = "all/cloudformation_records_per_resource_stack_events.rs"]
mod cloudformation_records_per_resource_stack_events;
// the sorted `i…` span (where the other iam suites land) are both regions concurrently
#[path = "all/iam_holds_the_principal_sts_reports.rs"]
mod iam_holds_the_principal_sts_reports;
// the sorted `s…` span are both regions concurrently landing suites add their line to.
#[path = "all/sts_does_not_disclose_an_absent_role.rs"]
mod sts_does_not_disclose_an_absent_role;
// Declared here, at the head, for the reason the entry above records: the appended tail
// and the sorted `s…` span are both regions concurrent suites add their line to.
#[path = "all/signer_revocation_status_across_services.rs"]
mod signer_revocation_status_across_services;
// the sorted `e…` span (where the other ecs/elbv2 suites land) are both regions concurrently
#[path = "all/ecs_awsvpc_configuration_over_an_ec2_network.rs"]
mod ecs_awsvpc_configuration_over_an_ec2_network;
// the sorted `l…` span are both regions concurrently landing suites add their line to.
#[path = "all/log_alarm_scheduled_query.rs"]
mod log_alarm_scheduled_query;
// the sorted `c…` span (where every cloudformation suite lands) are both regions concurrent
#[path = "all/cloudformation_provisions_codebuild_report_groups.rs"]
mod cloudformation_provisions_codebuild_report_groups;
#[path = "all/cloudformation_provisions_codecommit_repositories.rs"]
mod cloudformation_provisions_codecommit_repositories;
// and the sorted `c…` span (where every cloudformation suite lands) are both regions
// concurrent suites add their line to.
#[path = "all/cloudformation_provisions_transfer_servers.rs"]
mod cloudformation_provisions_transfer_servers;
// Declared beside the entry above, at the head, for the same reason: the appended tail
#[path = "all/cloudformation_provisions_batch_resources.rs"]
mod cloudformation_provisions_batch_resources;
// and the sorted `s…` span are both regions concurrent suites land in.
#[path = "all/ses_family_signing_name_routing.rs"]
mod ses_family_signing_name_routing;
// Declared beside its sibling suite, at the head, for the reason the entries above record:
// the appended tail and the sorted `s…` span are both regions concurrently landing suites
// add their line to.
#[path = "all/ses_event_destination_setup_across_services.rs"]
mod ses_event_destination_setup_across_services;
// Declared beside its two sibling suites, at the head, for the reason the entries above
// record: the appended tail and the sorted `s…` span are both regions concurrently landing
#[path = "all/ses_firehose_event_destination_setup.rs"]
mod ses_firehose_event_destination_setup;
// Declared beside its sibling ses suites, at the head, for the reason the entries above
#[path = "all/ses_v1_receipt_rule_sets_through_the_router.rs"]
mod ses_v1_receipt_rule_sets_through_the_router;
#[path = "all/ses_identities_shared_across_v1_and_v2.rs"]
mod ses_identities_shared_across_v1_and_v2;
#[path = "all/ses_easy_dkim_shared_across_surfaces.rs"]
mod ses_easy_dkim_shared_across_surfaces;
#[path = "all/ses_identity_policies_shared_across_surfaces.rs"]
mod ses_identity_policies_shared_across_surfaces;
// Declared beside the suites above, at the head, for the same reason: the appended tail and
#[path = "all/cloudformation_provisions_ses_configuration_sets.rs"]
mod cloudformation_provisions_ses_configuration_sets;
// the appended tail and the sorted `c…` span (where every cloudformation suite lands) are
#[path = "all/cloudformation_provisions_ses_receipt_rule_sets.rs"]
mod cloudformation_provisions_ses_receipt_rule_sets;
// At the head for the same reason, and moved here out of the `c…` span of the sorted
// list: that span is where the cloudformation/cloudtrail suites land, so a line beside
// them lost repeated squash merges to branches that touched it in the same window.
#[path = "all/chime_signing_name_routing.rs"]
mod chime_signing_name_routing;
// Out of order for the same reason, and placed beside it rather than in the sorted span
// of its own name or in the appended tail, both of which concurrent suites land into.
#[path = "all/cognito_vended_session_reads_sse_kms_object.rs"]
mod cognito_vended_session_reads_sse_kms_object;
// Declared beside the suite above, at the head, for the same reason it is here.
#[path = "all/cognito_vended_session_is_audited.rs"]
mod cognito_vended_session_is_audited;
// Declared beside its two sibling cognito suites, at the head, for the same reason they are
// here: the appended tail and the sorted `c…` span are both regions concurrent suites land in.
#[path = "all/cognito_vended_session_is_an_assumed_role_caller.rs"]
mod cognito_vended_session_is_an_assumed_role_caller;
// Declared beside its sibling cognito suites, at the head, for the same reason they are
#[path = "all/cognito_post_confirmation_lambda_trigger.rs"]
mod cognito_post_confirmation_lambda_trigger;
#[path = "all/cognito_group_role_mapping.rs"]
mod cognito_group_role_mapping;
#[path = "all/pi_resource_metadata_reflects_rds_configuration.rs"]
mod pi_resource_metadata_reflects_rds_configuration;
// At the head for the reason the entries above record: the appended tail and the sorted
// `e…` span (where the other events suites land) are both regions concurrent suites add
#[path = "all/events_fan_out_to_each_matched_target.rs"]
mod events_fan_out_to_each_matched_target;
// Out of order for the same reason as the two entries above it.
#[path = "all/redshift_cluster_iam_role_poll_lifecycle.rs"]
mod redshift_cluster_iam_role_poll_lifecycle;
// At the head for the same reason: the `c…` span of the sorted list is where the
// cloudformation suites land, and the appended tail is where every concurrent suite does.
#[path = "all/cloudformation_updates_iam_roles.rs"]
mod cloudformation_updates_iam_roles;
// At the head for the same reason as the entry above: the `c…` span of the sorted list is
// where the cloudformation suites land, and the appended tail is where every concurrent
// suite does.
#[path = "all/cloudformation_apigatewayv2_api_children.rs"]
mod cloudformation_apigatewayv2_api_children;
#[path = "all/cloudformation_apigatewayv2_resources.rs"]
mod cloudformation_apigatewayv2_resources;
#[path = "all/amplifybackend_app_registration.rs"]
mod amplifybackend_app_registration;
#[path = "all/apigateway_signing_name_routing.rs"]
mod apigateway_signing_name_routing;
#[path = "all/appconfig_signing_name_routing.rs"]
mod appconfig_signing_name_routing;
#[path = "all/aws_marketplace_signing_name_routing.rs"]
mod aws_marketplace_signing_name_routing;
// These sit in the SORTED block rather than being appended below, deliberately: the
// appended region at the end of this file is where every concurrent suite lands, so a
// line added there conflicts with every other suite in the same window. Here the
// neighbouring lines are stable.
#[path = "all/bedrock_agentcore_execution_role.rs"]
mod bedrock_agentcore_execution_role;
#[path = "all/bedrock_agentcore_gateway_lambda_target.rs"]
mod bedrock_agentcore_gateway_lambda_target;
#[path = "all/bedrock_agentcore_gateway_mcp_endpoint.rs"]
mod bedrock_agentcore_gateway_mcp_endpoint;
#[path = "all/bedrock_agentcore_signing_name_routing.rs"]
mod bedrock_agentcore_signing_name_routing;
#[path = "all/bedrock_agent_runtime_session_authorization.rs"]
mod bedrock_agent_runtime_session_authorization;
#[path = "all/bedrock_signing_name_routing.rs"]
mod bedrock_signing_name_routing;
#[path = "all/billingconductor_payer_account.rs"]
mod billingconductor_payer_account;
#[path = "all/boot_serve.rs"]
mod boot_serve;
// Declared in the sorted list rather than appended at the end of this file: the tail is
// the contended region every other new router suite lands in, and a declaration here
// merges cleanly beside them.
#[path = "all/cfn_provisions_declared_resources.rs"]
mod cfn_provisions_declared_resources;
// Declared HERE, in the sorted list, for the same reason.
#[path = "all/chime_family_signing_name_routing.rs"]
mod chime_family_signing_name_routing;
// Declared in this quiet span of the sorted list, for the reason the notes below give.
#[path = "all/cloudformation_canceled_delete_keeps_exported_resources.rs"]
mod cloudformation_canceled_delete_keeps_exported_resources;
// Declared in this span of the sorted list rather than appended at the end of this file,
// for the reason the notes here give: the tail is the region every concurrently landing
// suite appends to.
#[path = "all/cloudformation_provisions_acm_acme_endpoints.rs"]
mod cloudformation_provisions_acm_acme_endpoints;
// Declared in this same quiet span, for the reason the note above gives.
#[path = "all/cloudformation_provisions_acm_certificates.rs"]
mod cloudformation_provisions_acm_certificates;
#[path = "all/cloudformation_activates_acmpca_certificate_authorities.rs"]
mod cloudformation_activates_acmpca_certificate_authorities;
#[path = "all/cloudformation_admits_acmpca_resource_types.rs"]
mod cloudformation_admits_acmpca_resource_types;
#[path = "all/cloudformation_provisions_acmpca_certificate_authorities.rs"]
mod cloudformation_provisions_acmpca_certificate_authorities;
#[path = "all/cloudformation_provisions_codedeploy_resources.rs"]
mod cloudformation_provisions_codedeploy_resources;
// Declared in this same quiet span of the sorted list, for the reason the note above
// gives: the tail of this file and the `c…` span beside the other cloudformation suites
// are both regions concurrently landing suites put their line in.
#[path = "all/cloudformation_provisions_eks_access_entries.rs"]
mod cloudformation_provisions_eks_access_entries;
// Declared in this same quiet span of the sorted list, for the reason the note above gives:
// a stack that declares an `AWS::EKS::Cluster` or an `AWS::EKS::PodIdentityAssociation`
// really creates it in Amazon EKS, holds the cluster's creation open until that service
// reports it ACTIVE, and reports what it minted in the stack's own outputs.
#[path = "all/cloudformation_provisions_eks_clusters.rs"]
mod cloudformation_provisions_eks_clusters;
#[path = "all/cloudformation_provisions_cognito_user_pool_groups.rs"]
mod cloudformation_provisions_cognito_user_pool_groups;
// the tail of this file is where every concurrently landing router suite appends.
#[path = "all/cloudformation_provisions_cloudwatch_dashboards.rs"]
mod cloudformation_provisions_cloudwatch_dashboards;
#[path = "all/cloudformation_provisions_sagemaker_model_package_group.rs"]
mod cloudformation_provisions_sagemaker_model_package_group;
// for the reason the notes above give: the tail is where every concurrently landing router
// suite appends, so a line added there conflicts with all of them.
#[path = "all/cloudformation_provisions_scheduler_schedules.rs"]
mod cloudformation_provisions_scheduler_schedules;
#[path = "all/cloudformation_provisions_stack_resources.rs"]
mod cloudformation_provisions_stack_resources;
#[path = "all/cloudfront_key_value_store_handshake.rs"]
mod cloudfront_key_value_store_handshake;
// Declared HERE, in the sorted list beside the other CloudFront suite, rather than appended
// at the end of this file: the tail is where every suite landing in the same window appends,
// so a line added there conflicts with all of them.
#[path = "all/cloudfront_oac_s3_origin_edge.rs"]
mod cloudfront_oac_s3_origin_edge;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every suite landing in the same window appends, so it is the contended
// region, while this span of the list is stable.
#[path = "all/cloudhsm_signing_name_routing.rs"]
mod cloudhsm_signing_name_routing;
// Declared here, in the SORTED block rather than appended at the end: the tail is where
// every suite landing in the same window appends, so it is the contended region, while
// this span of the list is stable.
#[path = "all/cloudtrail_channel_target_routing.rs"]
mod cloudtrail_channel_target_routing;
// In the SORTED block, not appended at the end, for the reason stated above: the end of
// this file is where every concurrent suite lands.
#[path = "all/cloudtrail_channel_routing.rs"]
mod cloudtrail_channel_routing;
// region, while this span of the sorted list is untouched by those appends.
#[path = "all/cloudtrail_import_failures_routing.rs"]
mod cloudtrail_import_failures_routing;
// tail is where every suite landing in the same window appends, so a line added there
// conflicts with every other one; this span of the sorted list is untouched by those
// appends.
#[path = "all/config_compliance_change_event.rs"]
mod config_compliance_change_event;
#[path = "all/connect_signing_name_routing.rs"]
mod connect_signing_name_routing;
// Declared HERE, in sorted position beside the other DynamoDB suite rather than appended
// at the end of this file: the tail is where every suite landing in the same window
// appends, so it is the contended region, while these neighbouring lines are stable.
#[path = "all/ddb_stream_lambda_esm_provisioning.rs"]
mod ddb_stream_lambda_esm_provisioning;
#[path = "all/dynamodb_data_plane_authorization.rs"]
mod dynamodb_data_plane_authorization;
#[path = "all/dynamodb_session_policy_authorization.rs"]
mod dynamodb_session_policy_authorization;
#[path = "all/dynamodb_streams_routing.rs"]
mod dynamodb_streams_routing;
// tail is where every landing in the same window appends, so it is the contended region,
// while this span of the sorted list is untouched by those appends.
#[path = "all/ecr_repository_policy_principal.rs"]
mod ecr_repository_policy_principal;
#[path = "all/ecs_service_target_group_association.rs"]
mod ecs_service_target_group_association;
// Declared HERE, in the sorted list, rather than appended at the end of this file like
// the most recent suites: the tail is where every landing in the same window appends, so
// it is the contended region now, while this span of the sorted list is untouched by
// those appends.
#[path = "all/elb_classic_and_v2_signing_name_routing.rs"]
mod elb_classic_and_v2_signing_name_routing;
// Declared in this span of the sorted list for the reason the note above gives: the tail
// of this file is where every concurrent router suite appends, so a line added there
// conflicts with every other landing in the same window.
#[path = "all/elbv2_network_resolution.rs"]
mod elbv2_network_resolution;
// Declared beside the suite it is the sibling of, in this span of the sorted list, for the
// same reason: the tail of this file is where every concurrently landing router suite
// appends its line.
#[path = "all/elbv2_ip_target_refusal_classes.rs"]
mod elbv2_ip_target_refusal_classes;
// Declared beside its sibling suites, in this span of the sorted list rather than appended
// at the tail of this file, for the reason the notes above give.
#[path = "all/elbv2_subnet_and_elastic_ip_resolution.rs"]
mod elbv2_subnet_and_elastic_ip_resolution;
// Declared beside the sibling suite above, in this span of the sorted list rather than
// appended at the tail of this file, for the reason the notes above give.
#[path = "all/elbv2_subnet_vpc_membership.rs"]
mod elbv2_subnet_vpc_membership;
// Declared beside its sibling ELBv2 suites, in this span of the sorted list rather than
#[path = "all/elbv2_certificate_in_use_by_acm.rs"]
mod elbv2_certificate_in_use_by_acm;
#[path = "all/elbv2_load_balancer_security_groups.rs"]
mod elbv2_load_balancer_security_groups;
// Declared beside the Classic/ELBv2 suites above, in this span of the sorted list rather
// than in the appended tail, for the same reason: the tail is where every concurrently
// landing router suite puts its line.
#[path = "all/elb_classic_zone_membership_follows_its_subnets.rs"]
mod elb_classic_zone_membership_follows_its_subnets;
// Declared beside its sibling Classic suite above, in this span of the sorted list rather
// than in the appended tail, for the same reason the notes above give.
#[path = "all/elb_classic_network_resources_resolve_in_ec2.rs"]
mod elb_classic_network_resources_resolve_in_ec2;
// Alphabetically between the `elbv2_` and `es_` suites, rather than appended at the end
// of this file, which is where every new suite lands and therefore where two changes
// collide.
#[path = "all/emr_job_flow_role_instance_profile.rs"]
mod emr_job_flow_role_instance_profile;
// Declared HERE too, in the sorted list rather than appended at the tail, for the reason
// the note above gives: the appended region at the end of this file is where every
// concurrent router suite lands, so a line added there conflicts with every other suite
// in the same window — this fix lost four serialized squash merges to exactly that. These
#[path = "all/es_opensearch_and_elasticsearch_signing_name_routing.rs"]
mod es_opensearch_and_elasticsearch_signing_name_routing;
#[path = "all/events_signing_name_routing.rs"]
mod events_signing_name_routing;
#[path = "all/events_store_ownership.rs"]
mod events_store_ownership;
#[path = "all/events_to_sfn_to_dynamodb_chain.rs"]
mod events_to_sfn_to_dynamodb_chain;
// In the sorted list rather than appended at the end of the file: every new suite appends
// there, so that is the contended region now and the sorted position this file's header
// asks for is the quiet one.
#[path = "all/firehose_destination_role.rs"]
mod firehose_destination_role;
// Declared HERE, beside the trust suite it is the sibling of, rather than appended at the
// end of this file: the tail is where every landing in the same window appends, so it is
// the contended region, while this span of the sorted list is untouched by those appends.
#[path = "all/fis_execution_role_permissions.rs"]
mod fis_execution_role_permissions;
#[path = "all/fis_execution_role_trust.rs"]
mod fis_execution_role_trust;
#[path = "all/forecast_signing_name_routing.rs"]
mod forecast_signing_name_routing;
#[path = "all/glue_jobs_are_regional_while_their_role_is_global.rs"]
mod glue_jobs_are_regional_while_their_role_is_global;
// Declared beside the other Glue suite rather than at the head or the tail of this file:
// both of those are where every concurrently landing suite puts its line.
#[path = "all/sfn_glue_run_a_job_sync.rs"]
mod sfn_glue_run_a_job_sync;
// Declared beside the other Step Functions suite, for the reason the lines below give:
// the head and the tail of this file are where every concurrently landing suite puts its
// line, so a declaration there conflicts with all of them.
#[path = "all/sfn_task_target_authorization.rs"]
mod sfn_task_target_authorization;
// Declared beside the other Step Functions suites, for the reason the lines above give.
#[path = "all/sfn_lambda_task_invocation.rs"]
mod sfn_lambda_task_invocation;
// Declared HERE, in the sorted list at its own name, rather than at the head or the tail
// of this file: both of those are where every concurrently landing suite puts its line, so
// a declaration there conflicts with all of them, while these neighbouring lines are
// stable.
#[path = "all/guardduty_runtime_coverage.rs"]
mod guardduty_runtime_coverage;
// tail is where every concurrently landing router suite appends, so a line added there
// conflicts with all of them, while these neighbouring lines are stable.
#[path = "all/sagemaker_feature_store_control_plane_handoff.rs"]
mod sagemaker_feature_store_control_plane_handoff;
// Declared beside the hand-off suite whose harness it drives, rather than appended at the
// end of this file, for the same reason the line above gives.
#[path = "all/sagemaker_feature_store_group_deletion.rs"]
mod sagemaker_feature_store_group_deletion;
// Declared beside the hand-off suite whose harness it drives, rather than appended at the
// end of this file, for the same reason the two lines above give.
#[path = "all/sagemaker_feature_store_default_ttl.rs"]
mod sagemaker_feature_store_default_ttl;
// Declared beside the feature-store hand-off suite it is the counterpart of, rather than
// appended at the end of this file, for the same reason the line above gives: the tail is
// where every concurrently landing router suite appends its line.
#[path = "all/sagemaker_agentic_rft_job_runtime_handoff.rs"]
mod sagemaker_agentic_rft_job_runtime_handoff;
#[path = "all/iam_and_signer_signing_name_routing.rs"]
mod iam_and_signer_signing_name_routing;
#[path = "all/iam_role_id_agrees_with_sts.rs"]
mod iam_role_id_agrees_with_sts;
// Declared beside the role-side suite it is the counterpart of, in the sorted list rather
// than appended at the end of this file: the tail is where every concurrently landing
// router suite puts its line, so a declaration there conflicts with all of them, while
// these neighbouring lines are stable.
#[path = "all/iam_access_key_signs_as_its_user.rs"]
mod iam_access_key_signs_as_its_user;
#[path = "all/iam_authorizes_an_assumed_role_session.rs"]
mod iam_authorizes_an_assumed_role_session;
#[path = "all/iam_session_refusal_audit_documents.rs"]
mod iam_session_refusal_audit_documents;
#[path = "all/iam_signing_name_routing.rs"]
mod iam_signing_name_routing;
#[path = "all/iot_command_deprecation_is_observable_on_the_data_plane.rs"]
mod iot_command_deprecation_is_observable_on_the_data_plane;
#[path = "all/iot_command_execution_lifecycle.rs"]
mod iot_command_execution_lifecycle;
// Declared HERE, beside the command-execution suite it is the sibling of, rather than
// appended at the end of this file: the tail is where every suite landing in the same
// window appends, so it is the contended region, while these neighbouring lines are
#[path = "all/iot_command_executions_are_listed_across_planes.rs"]
mod iot_command_executions_are_listed_across_planes;
#[path = "all/iot_job_execution_reaches_the_device.rs"]
mod iot_job_execution_reaches_the_device;
#[path = "all/iot_jobs_data_endpoint_discovery.rs"]
mod iot_jobs_data_endpoint_discovery;
#[path = "all/ivs_signing_name_routing.rs"]
mod ivs_signing_name_routing;
// Declared here, between two unrelated spans, rather than at the head of this file, in its
// appended tail or in the `a…` span of its own name: all three are where concurrently
// landing suites put their line, so a declaration in any of them loses a squash merge to
// every one of them, while this boundary's neighbouring lines are stable.
#[path = "all/apigatewayv2_cognito_jwt_authorizer.rs"]
mod apigatewayv2_cognito_jwt_authorizer;
#[path = "all/keyspaces_signing_name_routing.rs"]
mod keyspaces_signing_name_routing;
#[path = "all/keyspaces_streams_routing.rs"]
mod keyspaces_streams_routing;
#[path = "all/lambda_bundled_wire_operations.rs"]
mod lambda_bundled_wire_operations;
#[path = "all/lambda_signing_name_routing.rs"]
mod lambda_signing_name_routing;
// Declared in this span of the sorted list, beside the other `lambda…` suites, rather than
// at the head of this file or in its appended tail: both of those are where concurrently
// landing suites put their line, so a declaration there loses a squash merge to every one
// of them, while these neighbouring lines are stable.
#[path = "all/lambda_execution_role_trust.rs"]
mod lambda_execution_role_trust;
// Declared in this span of the sorted list, for the reason the line above states.
#[path = "all/lambda_capacity_provider_operator_role.rs"]
mod lambda_capacity_provider_operator_role;
// Declared in this span of the sorted list, beside the other `l…` suites, rather than at
// the head of this file or in its appended tail: both of those are where concurrently
#[path = "all/lakeformation_catalog_resource_resolution.rs"]
mod lakeformation_catalog_resource_resolution;
// tail is where every concurrently landing router suite appends its line, so a
// declaration placed there conflicts with every one of them. (The build script requires
// every suite under `tests/all/` to be declared in this file, so it cannot live anywhere
// else.) These neighbouring lines are stable.
#[path = "all/lex_runtime_v2_sessions_through_the_router.rs"]
mod lex_runtime_v2_sessions_through_the_router;
#[path = "all/marketplace_signing_name_routing.rs"]
mod marketplace_signing_name_routing;
// Out of alphabetical order on purpose: the `c` span of this list, its end, and the
// appended region at the bottom of the file are where concurrently landing suites insert
// their own line, and a line next to theirs cannot be merged with them. This `n`/`o` gap
// is the quietest span in the file.
#[path = "all/chime_app_instance_admin_authorization.rs"]
mod chime_app_instance_admin_authorization;
// Declared in this same quiet `n`/`o` gap, beside the other cross-service Chime suite and
// for the reason the entry above records.
#[path = "all/chime_identity_name_projection.rs"]
mod chime_identity_name_projection;
#[path = "all/mediaconvert_input_probe.rs"]
mod mediaconvert_input_probe;
// Declared HERE, in this same quiet `n`/`o` gap rather than in the appended tail, the head
// region or the `c…` span: all three are where concurrently landing suites put their line,
// and a line next to theirs cannot be merged with them.
#[path = "all/compute_optimizer_automation_denial_principal_agrees_with_sts.rs"]
mod compute_optimizer_automation_denial_principal_agrees_with_sts;
// Declared HERE, in this quiet span of the sorted list, rather than appended at the end of
// this file or beside the other `cloudformation_*` suites in the `c` span: both of those are
// where every concurrently landing suite puts its line, so a declaration there conflicts
// with all of them.
#[path = "all/cloudformation_ec2_network_identifiers.rs"]
mod cloudformation_ec2_network_identifiers;
// Declared HERE, beside the sibling suite above in the same quiet span, for the reason that
// entry records: the appended tail at the end of this file and the `c…` span beside the other
// `cloudformation_*` suites are both where every concurrently landing suite puts its line.
#[path = "all/cloudformation_ec2_network_stack_routes.rs"]
mod cloudformation_ec2_network_stack_routes;
// Declared HERE, in this same quiet `n`/`o` gap rather than in the appended tail or in the
// `c…` span beside the other `cloudformation_provisions_*` suites: both of those are where
// every concurrently landing suite puts its line.
#[path = "all/cloudformation_provisions_wafv2_entities.rs"]
mod cloudformation_provisions_wafv2_entities;
#[path = "all/cloudformation_provisions_codepipeline_pipelines.rs"]
mod cloudformation_provisions_codepipeline_pipelines;
// Declared HERE, in this quiet `n`/`o` gap rather than in the appended tail or beside the
// other `eks_*` suite: both of those are where concurrently landing suites put their line.
// `eks_wire` is this test crate's shared EKS request harness, not a suite of its own.
#[path = "all/eks_wire.rs"]
mod eks_wire;
#[path = "all/eks_cluster_security_group.rs"]
mod eks_cluster_security_group;
// Declared HERE, beside the sibling suite above in the same quiet `n`/`o` gap, for the
// reason that entry records: the appended tail at the end of this file and the `e…` span
// beside the other `eks_*` suites are both regions every concurrently landing router suite
// puts its line in.
#[path = "all/eks_wrapped_peer_refusals.rs"]
mod eks_wrapped_peer_refusals;
// Declared HERE, in this same quiet `n`/`o` gap rather than in the appended tail at the end
// of this file or beside the sibling `logs_emf_*` suite in it: both of those are where every
// concurrently landing router suite puts its line.
#[path = "all/logs_emf_disabled_errors.rs"]
mod logs_emf_disabled_errors;
// Declared HERE, beside the sibling suite above in the same quiet `n`/`o` gap, for the reason
// that entry records: the appended tail at the end of this file and the head of the list are
// both regions every concurrently landing router suite puts its line in.
#[path = "all/logs_emf_extraction_policy_scope.rs"]
mod logs_emf_extraction_policy_scope;
// Declared HERE, beside the two sibling suites above in the same quiet `n`/`o` gap, for the
// reason their entries record: the appended tail at the end of this file and the head of the
// list are both regions every concurrently landing router suite puts its line in.
#[path = "all/logs_emf_declared_projections.rs"]
mod logs_emf_declared_projections;
// Declared HERE, beside the sibling `logs_*` suites in the same quiet `n`/`o` gap, for the
// reason those entries record: the appended tail at the end of this file and the head of the
#[path = "all/logs_export_task_destination_bucket.rs"]
mod logs_export_task_destination_bucket;
#[path = "all/partnercentral_channel_target_routing.rs"]
mod partnercentral_channel_target_routing;
#[path = "all/personalize_signing_name_routing.rs"]
mod personalize_signing_name_routing;
// Declared HERE, beside the sibling Personalize suite, for the reason the entries around it
// record: the appended tail at the end of this file and the head of the list are both regions
// every concurrently landing router suite puts its line in.
#[path = "all/personalize_ingestion_targets.rs"]
mod personalize_ingestion_targets;
// Declared HERE, in this quiet span of the sorted list, for the reason the entries around
// it record: the appended tail at the end of this file and the head of the list are both
// regions every concurrently landing router suite puts its line in.
#[path = "all/payment_cryptography_signing_name_routing.rs"]
mod payment_cryptography_signing_name_routing;
// Declared HERE, in this same quiet span of the sorted list, for the reason the entries
// around it record: the appended tail at the end of this file and the HEAD of the list are
// both regions every concurrently landing router suite puts its line in, so a declaration in
// either collides with all of them.
#[path = "all/pinpoint_sms_voice_signing_name_routing.rs"]
mod pinpoint_sms_voice_signing_name_routing;
// this file or beside the other `cloudformation_provisions_*` suites in the `c` span: both
// of those are where every concurrently landing suite puts its line, so a declaration there
// conflicts with all of them.
#[path = "all/cloudformation_provisions_redshift_clusters.rs"]
mod cloudformation_provisions_redshift_clusters;
#[path = "all/cloudformation_provisions_route53resolver_rules.rs"]
mod cloudformation_provisions_route53resolver_rules;
// Declared HERE, beside the sibling Route 53 Resolver suite in the same quiet span, for the
// reason that entry records: the appended tail at the end of this file and the `c` span are
#[path = "all/cloudformation_provisions_route53resolver_firewall.rs"]
mod cloudformation_provisions_route53resolver_firewall;
// Declared HERE, beside the sibling CloudFormation provisioning suites in the same quiet
// span, for the reason those entries record: the appended tail at the end of this file and
// the `c` span are both regions every concurrently landing router suite puts its line in.
#[path = "all/cloudformation_provisions_xray_sampling_rules.rs"]
mod cloudformation_provisions_xray_sampling_rules;
// `query_wire` and `rds_query_front_door` are declared HERE, in the sorted list,
// rather than appended at the end: the end of this file is where every new router
// suite lands, so a line added there collides with every other suite landing in the
// same window. Their own files are ordinary `tests/all/<name>.rs` suites.
#[path = "all/query_wire.rs"]
mod query_wire;
#[path = "all/rds_docdb_neptune_signing_name_routing.rs"]
mod rds_docdb_neptune_signing_name_routing;
// Declared beside its `rds_*` siblings, in sorted position: this is the quiet span the
// comment above names, and not the tail every concurrently landing suite appends to.
#[path = "all/rds_management_events.rs"]
mod rds_management_events;
// Declared beside its sibling management-event suite, in the same quiet span and for the
// same reason: the head of this file and its appended tail are both where concurrently
// landing suites add their line. This one pins that a CloudFormation call — accepted and
// refused alike — is in CloudTrail event history.
#[path = "all/cloudformation_management_events.rs"]
mod cloudformation_management_events;
#[path = "all/rds_query_front_door.rs"]
mod rds_query_front_door;
#[path = "all/rds_shared_query_front_door_routing.rs"]
mod rds_shared_query_front_door_routing;
#[path = "all/rds_shared_query_surface_routing.rs"]
mod rds_shared_query_surface_routing;
#[path = "all/rds_shared_signing_name_routing.rs"]
mod rds_shared_signing_name_routing;
#[path = "all/rds_signing_name_routing.rs"]
mod rds_signing_name_routing;
#[path = "all/resiliencehub_signing_name_routing.rs"]
mod resiliencehub_signing_name_routing;
// Declared HERE, in the sorted list at its own name, rather than appended at the end of
// this file: the tail is where every concurrently landing router suite appends its line, so
// a declaration there conflicts with every one of them, while these neighbouring lines are
#[path = "all/route53_health_check_status_metric.rs"]
mod route53_health_check_status_metric;
// Declared beside the suite above for the same reason its own comment gives: this is a
// quiet span of the sorted list, while the tail of this file and the `c` span are where
#[path = "all/cloudformation_provisions_route53_hosted_zones.rs"]
mod cloudformation_provisions_route53_hosted_zones;
#[path = "all/s3files_create_file_system_bucket.rs"]
mod s3files_create_file_system_bucket;
#[path = "all/scheduler_execution_role.rs"]
mod scheduler_execution_role;
#[path = "all/servicecatalog_signing_name_routing.rs"]
mod servicecatalog_signing_name_routing;
// Declared HERE, in sorted position in this quiet span, rather than beside the sibling SES
// suites at the head of this file or in its appended tail: both of those are where every
// concurrently landing suite puts its line, so a declaration there loses the squash merge
// to all of them, while these neighbouring lines are stable.
#[path = "all/ses_family_pre_dispatch_refusals.rs"]
mod ses_family_pre_dispatch_refusals;
// Declared HERE, beside the sibling SES suite above in this quiet span, rather than with the
// other SES suites at the head of this file or in its appended tail: both of those are where
#[path = "all/sesv2_sns_event_destination_topic_policy.rs"]
mod sesv2_sns_event_destination_topic_policy;
// Declared HERE, beside the sibling SESv2 suite above in this quiet span, for the same
// reason: the head of this file and its appended tail are where every concurrently landing
// suite puts its line, while these neighbouring lines are stable.
#[path = "all/ses_send_events_reach_a_subscribed_queue.rs"]
mod ses_send_events_reach_a_subscribed_queue;
#[path = "all/ses_bulk_send_entry_headers_override_the_template.rs"]
mod ses_bulk_send_entry_headers_override_the_template;
#[path = "all/shared_signing_name_routing.rs"]
mod shared_signing_name_routing;
#[path = "all/ssm_contacts_replication_set.rs"]
mod ssm_contacts_replication_set;
#[path = "all/sts_source_identity_is_audited.rs"]
mod sts_source_identity_is_audited;
#[path = "all/sts_web_identity_federation.rs"]
mod sts_web_identity_federation;
#[path = "all/transcribe_signing_name_routing.rs"]
mod transcribe_signing_name_routing;
#[path = "all/transfer_user_with_iam_role.rs"]
mod transfer_user_with_iam_role;
// Declared HERE, in this quiet span rather than at the head of this file, in its appended
// tail or in the `b…` span of its own name: all three are where concurrently landing
// suites put their line, so a declaration in any of them loses a squash merge to every
// one of them, while these neighbouring lines are stable.
#[path = "all/batch_job_state_change_reaches_matching_rules.rs"]
mod batch_job_state_change_reaches_matching_rules;
// Declared HERE, in sorted position in this quiet span, rather than at the head of this
// file or in its appended tail: both of those are where every concurrently landing suite
// puts its line, so a declaration there conflicts with all of them.
#[path = "all/unsigned_entry_points_reach_their_service.rs"]
mod unsigned_entry_points_reach_their_service;
#[path = "all/wisdom_qconnect_signing_name_routing.rs"]
mod wisdom_qconnect_signing_name_routing;
// Declared last, out of the sorted order above, on purpose: every new router suite adds
// a line to this file, and a line inserted in the middle of the list conflicts with
// every other suite landing in the same window. Appending keeps this change out of that
// contended region. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/sagemaker_signing_name_routing.rs"]
mod sagemaker_signing_name_routing;
#[path = "all/billing_access_denied_principal_agrees_with_sts.rs"]
mod billing_access_denied_principal_agrees_with_sts;
#[path = "all/bedrock_modelled_route_claims.rs"]
mod bedrock_modelled_route_claims;
#[path = "all/connectcampaigns_signing_name_routing.rs"]
mod connectcampaigns_signing_name_routing;
#[path = "all/pi_data_source_ownership.rs"]
mod pi_data_source_ownership;
#[path = "all/lex_signing_name_routing.rs"]
mod lex_signing_name_routing;
#[path = "all/sagemaker_trial_component_metric_suggestions.rs"]
mod sagemaker_trial_component_metric_suggestions;
#[path = "all/lex_models_v2_runtime_v2_cross_service.rs"]
mod lex_models_v2_runtime_v2_cross_service;
#[path = "all/wisdom_signing_name_routing.rs"]
mod wisdom_signing_name_routing;
#[path = "all/cloudtrail_sdk_target_routing.rs"]
mod cloudtrail_sdk_target_routing;
#[path = "all/eks_cluster_inputs_resolve.rs"]
mod eks_cluster_inputs_resolve;
#[path = "all/cloudformation_provisions_ecr_public_repository.rs"]
mod cloudformation_provisions_ecr_public_repository;
#[path = "all/cloudformation_provisions_elastic_beanstalk_resources.rs"]
mod cloudformation_provisions_elastic_beanstalk_resources;
#[path = "all/cloudformation_provisions_sqs_queue_policies.rs"]
mod cloudformation_provisions_sqs_queue_policies;
#[path = "all/cloudformation_provisions_sqs_queues.rs"]
mod cloudformation_provisions_sqs_queues;
#[path = "all/opensearchserverless_cloudformation_resources.rs"]
mod opensearchserverless_cloudformation_resources;
#[path = "all/cloudformation_provisions_container_platform_resources.rs"]
mod cloudformation_provisions_container_platform_resources;
#[path = "all/kinesis_video_signing_name_routing.rs"]
mod kinesis_video_signing_name_routing;
#[path = "all/apigatewayv2_websocket_deploy_then_connections.rs"]
mod apigatewayv2_websocket_deploy_then_connections;
#[path = "all/cloudformation_provisions_fis_experiment_templates.rs"]
mod cloudformation_provisions_fis_experiment_templates;
#[path = "all/cloudformation_provisions_guardduty_detector.rs"]
mod cloudformation_provisions_guardduty_detector;
#[path = "all/cloudformation_provisions_sns_topics.rs"]
mod cloudformation_provisions_sns_topics;
#[path = "all/cloudformation_provisions_sns_subscriptions.rs"]
mod cloudformation_provisions_sns_subscriptions;
#[path = "all/cloudformation_sns_topic_policy_admits_s3_notifications.rs"]
mod cloudformation_sns_topic_policy_admits_s3_notifications;
#[path = "all/cloudformation_provisions_waf_regional_ip_sets.rs"]
mod cloudformation_provisions_waf_regional_ip_sets;
#[path = "all/cloudformation_provisions_grafana_workspaces.rs"]
mod cloudformation_provisions_grafana_workspaces;
#[path = "all/mediapackagev2_published_data_plane_urls.rs"]
mod mediapackagev2_published_data_plane_urls;
#[path = "all/pi_resource_features.rs"]
mod pi_resource_features;
#[path = "all/cloudformation_provisions_cloudwatch_metric_streams.rs"]
mod cloudformation_provisions_cloudwatch_metric_streams;
#[path = "all/cloudformation_provisions_event_rules_and_ssm_parameters.rs"]
mod cloudformation_provisions_event_rules_and_ssm_parameters;
#[path = "all/cloudformation_provisions_sfn_activities.rs"]
mod cloudformation_provisions_sfn_activities;
#[path = "all/cloudformation_provisions_sfn_state_machines.rs"]
mod cloudformation_provisions_sfn_state_machines;
#[path = "all/cloudformation_provisions_s3vectors_resources.rs"]
mod cloudformation_provisions_s3vectors_resources;
#[path = "all/cloudformation_provisions_msk_clusters.rs"]
mod cloudformation_provisions_msk_clusters;
#[path = "all/logs_emf_metric_extraction.rs"]
mod logs_emf_metric_extraction;
#[path = "all/logs_metric_filter_extraction.rs"]
mod logs_metric_filter_extraction;
#[path = "all/cloudformation_provisions_kms_keys.rs"]
mod cloudformation_provisions_kms_keys;
#[path = "all/cloudformation_provisions_mq_brokers.rs"]
mod cloudformation_provisions_mq_brokers;
#[path = "all/cloudformation_refuses_mq_brokers.rs"]
mod cloudformation_refuses_mq_brokers;
#[path = "all/cloudformation_provisions_agentcore_workload_identities.rs"]
mod cloudformation_provisions_agentcore_workload_identities;
#[path = "all/events_lambda_target_resource_policy.rs"]
mod events_lambda_target_resource_policy;
#[path = "all/events_lambda_target_runs_the_handler.rs"]
mod events_lambda_target_runs_the_handler;
#[path = "all/cloudformation_provisions_lambda_functions.rs"]
mod cloudformation_provisions_lambda_functions;
#[path = "all/cloudformation_updates_lambda_functions.rs"]
mod cloudformation_updates_lambda_functions;
#[path = "all/cloudformation_provisions_lambda_event_source_mappings.rs"]
mod cloudformation_provisions_lambda_event_source_mappings;
// Declared here, in the sorted block, for the reason the entries above give: the appended
// tail of this file is the contended region every other new suite lands in.
#[path = "all/cloudformation_sam_http_api_definitions.rs"]
mod cloudformation_sam_http_api_definitions;
#[path = "all/opensearch_vpc_endpoint_vpc_membership.rs"]
mod opensearch_vpc_endpoint_vpc_membership;
// Declared here, in this quiet span rather than in the appended tail every concurrently
// landing router suite adds its line to.
#[path = "all/apigatewayv2_vpc_link_subnets_resolve_in_ec2.rs"]
mod apigatewayv2_vpc_link_subnets_resolve_in_ec2;
// Declared here, in this quiet span, for the reason the entry above gives: the IAM role an
// API Gateway account may write its logs under.
#[path = "all/apigateway_account_cloudwatch_role.rs"]
mod apigateway_account_cloudwatch_role;
#[path = "all/zz_probe_whole_emulator.rs"]
mod zz_probe_whole_emulator;
#[path = "all/cloudformation_provisions_cognito_identity_pools.rs"]
mod cloudformation_provisions_cognito_identity_pools;
