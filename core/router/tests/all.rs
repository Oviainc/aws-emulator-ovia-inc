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
#[path = "all/apigateway_signing_name_routing.rs"]
mod apigateway_signing_name_routing;
#[path = "all/appconfig_signing_name_routing.rs"]
mod appconfig_signing_name_routing;
#[path = "all/aws_marketplace_signing_name_routing.rs"]
mod aws_marketplace_signing_name_routing;
// These two sit in the SORTED block rather than being appended below, deliberately: the
// appended region at the end of this file is where every concurrent suite lands, so a
// line added there conflicts with every other suite in the same window. Here the
// neighbouring lines are stable.
#[path = "all/bedrock_agentcore_gateway_lambda_target.rs"]
mod bedrock_agentcore_gateway_lambda_target;
#[path = "all/bedrock_agentcore_signing_name_routing.rs"]
mod bedrock_agentcore_signing_name_routing;
#[path = "all/boot_serve.rs"]
mod boot_serve;
#[path = "all/cloudfront_key_value_store_handshake.rs"]
mod cloudfront_key_value_store_handshake;
#[path = "all/connect_signing_name_routing.rs"]
mod connect_signing_name_routing;
#[path = "all/dynamodb_streams_routing.rs"]
mod dynamodb_streams_routing;
#[path = "all/events_signing_name_routing.rs"]
mod events_signing_name_routing;
#[path = "all/events_store_ownership.rs"]
mod events_store_ownership;
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
#[path = "all/rds_docdb_neptune_signing_name_routing.rs"]
mod rds_docdb_neptune_signing_name_routing;
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

// Declared last, out of the sorted order above, on purpose: every new router suite adds
// a line to this file, and a line inserted in the middle of the list conflicts with
// every other suite landing in the same window. Appending keeps this change out of that
// contended region. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/sagemaker_signing_name_routing.rs"]
mod sagemaker_signing_name_routing;
#[path = "all/bedrock_signing_name_routing.rs"]
mod bedrock_signing_name_routing;
#[path = "all/billing_access_denied_principal_agrees_with_sts.rs"]
mod billing_access_denied_principal_agrees_with_sts;
#[path = "all/connectcampaigns_signing_name_routing.rs"]
mod connectcampaigns_signing_name_routing;
#[path = "all/pi_data_source_ownership.rs"]
mod pi_data_source_ownership;
#[path = "all/lex_signing_name_routing.rs"]
mod lex_signing_name_routing;
