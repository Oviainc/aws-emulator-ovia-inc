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
#[path = "all/amplifybackend_app_registration.rs"]
mod amplifybackend_app_registration;
#[path = "all/apigateway_signing_name_routing.rs"]
mod apigateway_signing_name_routing;
#[path = "all/appconfig_signing_name_routing.rs"]
mod appconfig_signing_name_routing;
#[path = "all/aws_marketplace_signing_name_routing.rs"]
mod aws_marketplace_signing_name_routing;
// These three sit in the SORTED block rather than being appended below, deliberately: the
// appended region at the end of this file is where every concurrent suite lands, so a
// line added there conflicts with every other suite in the same window. Here the
// neighbouring lines are stable.
#[path = "all/bedrock_agentcore_gateway_lambda_target.rs"]
mod bedrock_agentcore_gateway_lambda_target;
#[path = "all/bedrock_agentcore_signing_name_routing.rs"]
mod bedrock_agentcore_signing_name_routing;
#[path = "all/bedrock_agent_runtime_session_authorization.rs"]
mod bedrock_agent_runtime_session_authorization;
#[path = "all/bedrock_signing_name_routing.rs"]
mod bedrock_signing_name_routing;
#[path = "all/boot_serve.rs"]
mod boot_serve;
#[path = "all/cloudformation_provisions_stack_resources.rs"]
mod cloudformation_provisions_stack_resources;
#[path = "all/cloudfront_key_value_store_handshake.rs"]
mod cloudfront_key_value_store_handshake;
#[path = "all/connect_signing_name_routing.rs"]
mod connect_signing_name_routing;
#[path = "all/dynamodb_streams_routing.rs"]
mod dynamodb_streams_routing;
// Declared HERE, in the sorted list, rather than appended at the end of this file like
// the most recent suites: the tail is where every landing in the same window appends, so
// it is the contended region now, while this span of the sorted list is untouched by
// those appends.
#[path = "all/elb_classic_and_v2_signing_name_routing.rs"]
mod elb_classic_and_v2_signing_name_routing;
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
#[path = "all/iam_and_signer_signing_name_routing.rs"]
mod iam_and_signer_signing_name_routing;
#[path = "all/iam_role_id_agrees_with_sts.rs"]
mod iam_role_id_agrees_with_sts;
#[path = "all/iam_signing_name_routing.rs"]
mod iam_signing_name_routing;
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
#[path = "all/lambda_signing_name_routing.rs"]
mod lambda_signing_name_routing;
#[path = "all/marketplace_signing_name_routing.rs"]
mod marketplace_signing_name_routing;
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
#[path = "all/cloudformation_provisions_container_platform_resources.rs"]
mod cloudformation_provisions_container_platform_resources;
#[path = "all/kinesis_video_signing_name_routing.rs"]
mod kinesis_video_signing_name_routing;
