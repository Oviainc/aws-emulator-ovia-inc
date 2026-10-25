// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for the router.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
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
#[path = "all/kinesisvideo_signing_name_routing.rs"]
mod kinesisvideo_signing_name_routing;
// At the head for the same reason, and moved here out of the `c…` span of the sorted
// list: that span is where the cloudformation/cloudtrail suites land, so a line beside
// them lost repeated squash merges to branches that touched it in the same window.
#[path = "all/chime_signing_name_routing.rs"]
mod chime_signing_name_routing;
// Out of order for the same reason, and placed beside it rather than in the sorted span
// of its own name or in the appended tail, both of which concurrent suites land into.
#[path = "all/cognito_vended_session_reads_sse_kms_object.rs"]
mod cognito_vended_session_reads_sse_kms_object;
#[path = "all/pi_resource_metadata_reflects_rds_configuration.rs"]
mod pi_resource_metadata_reflects_rds_configuration;
// Out of order for the same reason as the two entries above it.
#[path = "all/redshift_cluster_iam_role_poll_lifecycle.rs"]
mod redshift_cluster_iam_role_poll_lifecycle;
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
// Declared in this span of the sorted list rather than appended at the end of this file,
// for the reason the notes here give: the tail is the region every concurrently landing
// suite appends to.
#[path = "all/cloudformation_provisions_acm_acme_endpoints.rs"]
mod cloudformation_provisions_acm_acme_endpoints;
#[path = "all/cloudformation_provisions_cognito_user_pool_groups.rs"]
mod cloudformation_provisions_cognito_user_pool_groups;
#[path = "all/cloudformation_provisions_sagemaker_model_package_group.rs"]
mod cloudformation_provisions_sagemaker_model_package_group;
// Declared in this span of the sorted list rather than appended at the end of this file,
// for the reason the notes above give: the tail is where every concurrently landing router
// suite appends, so a line added there conflicts with all of them.
#[path = "all/cloudformation_provisions_scheduler_schedules.rs"]
mod cloudformation_provisions_scheduler_schedules;
#[path = "all/cloudformation_provisions_stack_resources.rs"]
mod cloudformation_provisions_stack_resources;
#[path = "all/cloudfront_key_value_store_handshake.rs"]
mod cloudfront_key_value_store_handshake;
// In the SORTED block, not appended at the end, for the reason stated above: the end of
// this file is where every concurrent suite lands.
#[path = "all/cloudtrail_channel_routing.rs"]
mod cloudtrail_channel_routing;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every suite landing in the same window appends, so it is the contended
// region, while this span of the sorted list is untouched by those appends.
#[path = "all/cloudtrail_import_failures_routing.rs"]
mod cloudtrail_import_failures_routing;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every suite landing in the same window appends, so a line added there
// conflicts with every other one; this span of the sorted list is untouched by those
// appends.
#[path = "all/config_compliance_change_event.rs"]
mod config_compliance_change_event;
#[path = "all/connect_signing_name_routing.rs"]
mod connect_signing_name_routing;
#[path = "all/dynamodb_streams_routing.rs"]
mod dynamodb_streams_routing;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every landing in the same window appends, so it is the contended region,
// while this span of the sorted list is untouched by those appends.
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
// Alphabetically between the `elbv2_` and `es_` suites, rather than appended at the end
// of this file, which is where every new suite lands and therefore where two changes
// collide.
#[path = "all/emr_job_flow_role_instance_profile.rs"]
mod emr_job_flow_role_instance_profile;
// Declared HERE too, in the sorted list rather than appended at the tail, for the reason
// the note above gives: the appended region at the end of this file is where every
// concurrent router suite lands, so a line added there conflicts with every other suite
// in the same window — this fix lost four serialized squash merges to exactly that. These
// neighbouring lines are stable.
#[path = "all/es_opensearch_and_elasticsearch_signing_name_routing.rs"]
mod es_opensearch_and_elasticsearch_signing_name_routing;
#[path = "all/events_signing_name_routing.rs"]
mod events_signing_name_routing;
#[path = "all/events_store_ownership.rs"]
mod events_store_ownership;
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
// Declared HERE, in the sorted list at its own name, rather than at the head or the tail
// of this file: both of those are where every concurrently landing suite puts its line, so
// a declaration there conflicts with all of them, while these neighbouring lines are
// stable.
#[path = "all/guardduty_runtime_coverage.rs"]
mod guardduty_runtime_coverage;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every concurrently landing router suite appends, so a line added there
// conflicts with all of them, while these neighbouring lines are stable.
#[path = "all/sagemaker_feature_store_control_plane_handoff.rs"]
mod sagemaker_feature_store_control_plane_handoff;
#[path = "all/iam_and_signer_signing_name_routing.rs"]
mod iam_and_signer_signing_name_routing;
#[path = "all/iam_role_id_agrees_with_sts.rs"]
mod iam_role_id_agrees_with_sts;
#[path = "all/iam_signing_name_routing.rs"]
mod iam_signing_name_routing;
#[path = "all/iot_command_execution_lifecycle.rs"]
mod iot_command_execution_lifecycle;
// Declared HERE, beside the command-execution suite it is the sibling of, rather than
// appended at the end of this file: the tail is where every suite landing in the same
// window appends, so it is the contended region, while these neighbouring lines are
// stable.
#[path = "all/iot_command_executions_are_listed_across_planes.rs"]
mod iot_command_executions_are_listed_across_planes;
#[path = "all/iot_job_execution_reaches_the_device.rs"]
mod iot_job_execution_reaches_the_device;
#[path = "all/iot_jobs_data_endpoint_discovery.rs"]
mod iot_jobs_data_endpoint_discovery;
#[path = "all/ivs_signing_name_routing.rs"]
mod ivs_signing_name_routing;
#[path = "all/keyspaces_signing_name_routing.rs"]
mod keyspaces_signing_name_routing;
#[path = "all/keyspaces_streams_routing.rs"]
mod keyspaces_streams_routing;
#[path = "all/lambda_bundled_wire_operations.rs"]
mod lambda_bundled_wire_operations;
#[path = "all/lambda_signing_name_routing.rs"]
mod lambda_signing_name_routing;
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
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
// Declared HERE, in the sorted list, rather than appended at the end of this file: the
// tail is where every landing in the same window appends, so it is the contended region,
// while this span of the sorted list is untouched by those appends.
#[path = "all/mediaconvert_input_probe.rs"]
mod mediaconvert_input_probe;
#[path = "all/partnercentral_channel_target_routing.rs"]
mod partnercentral_channel_target_routing;
#[path = "all/personalize_signing_name_routing.rs"]
mod personalize_signing_name_routing;
// `query_wire` and `rds_query_front_door` are declared HERE, in the sorted list,
// rather than appended at the end: the end of this file is where every new router
// suite lands, so a line added there collides with every other suite landing in the
// same window. Their own files are ordinary `tests/all/<name>.rs` suites.
#[path = "all/query_wire.rs"]
mod query_wire;
#[path = "all/rds_docdb_neptune_signing_name_routing.rs"]
mod rds_docdb_neptune_signing_name_routing;
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
#[path = "all/s3files_create_file_system_bucket.rs"]
mod s3files_create_file_system_bucket;
#[path = "all/scheduler_execution_role.rs"]
mod scheduler_execution_role;
#[path = "all/servicecatalog_signing_name_routing.rs"]
mod servicecatalog_signing_name_routing;
#[path = "all/shared_signing_name_routing.rs"]
mod shared_signing_name_routing;
#[path = "all/ssm_contacts_replication_set.rs"]
mod ssm_contacts_replication_set;
#[path = "all/transcribe_signing_name_routing.rs"]
mod transcribe_signing_name_routing;
#[path = "all/transfer_user_with_iam_role.rs"]
mod transfer_user_with_iam_role;
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
#[path = "all/cloudformation_provisions_s3vectors_resources.rs"]
mod cloudformation_provisions_s3vectors_resources;
