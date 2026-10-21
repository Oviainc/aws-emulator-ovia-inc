// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
#![doc(hidden)]
//! Router — assembles all services into the multi-service emulator.
//!
//! # Writing integration tests
//!
//! Tests live in `router/tests/all/<name>.rs`, each declared in `router/tests/all.rs`.
//! Every router test target links all services, so the router has exactly one; a file
//! directly under `router/tests/` fails the build. `build_router(engine)` returns the
//! `MultiServiceRouter` the production entrypoint serves; drive requests through
//! `router.dispatch(req)` and assert on the response. The framework handles
//! deserialization, routing, and engine injection.
//!
//! ## Pattern
//!
//! ```rust,ignore
//! use std::sync::Arc;
//!
//! #[tokio::test]
//! async fn test_create_thing() {
//!     let engine = Arc::new(engine::InMemoryEngine::new());
//!     let router = build_router(engine);
//!     let req = http::Request::builder()
//!         .method("POST")
//!         .uri("http://localhost/2021-01-01/things")
//!         .header("authorization", /* a SigV4 header naming the service */ "...")
//!         .body(bytes::Bytes::from_static(br#"{"Name":"my-thing"}"#))
//!         .unwrap();
//!     let resp = router.dispatch(req).await;
//!     // 501 until you implement the operation — a FRESHLY SCAFFOLDED operation
//!     // answers `NotImplemented`, so asserting 200 here fails until the `handle`
//!     // body is written. Change this to 200 as part of implementing it.
//!     assert_eq!(resp.status().as_u16(), 501);
//! }
//! ```
//!
//! ## Key points
//!
//! - **Direct construction**: each service is built from the shared engine via
//!   `<service>::EmulatedService::new(engine.clone())`, which constructs each
//!   operation's authored `impl Operation` struct through the engine-as-constructor
//!   seam. There is no builder and no per-operation registration step.
//! - **Routing**: `MultiServiceRouter` picks the service whose `can_handle` claims
//!   the request (seeded from the model's SigV4 signing name), then the service's
//!   protocol names the operation.
//! - **Deps**: a router integration test needs `service-foo`, `engine`, `http`,
//!   `bytes` and `tokio`. Put the test in the SERVICE crate rather than here:
//!   `router/Cargo.toml` is re-rendered on every service add (see the banner at the
//!   top of this file), so a dev-dependency added there is overwritten.

// @generated — RE-RENDERED ON EVERY SERVICE ADD. Edits below are OVERWRITTEN.
//
// Put hand-written code in a SERVICE crate, or in `router/src/main.rs`. Those are
// yours; this file, `router/Cargo.toml` and the workspace `Cargo.toml` are replaced
// whenever a service is added, so an edit here is lost.

use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
pub use engine::InMemoryEngine;

pub fn new_engine() -> Arc<InMemoryEngine> {
    Arc::new(InMemoryEngine::new())
}

/// Assemble the multi-service router the production entrypoint serves.
///
/// Registrations are DERIVED from the scaffolded service set, not hand-maintained:
/// adding a service re-renders this whole file. `router/tests/all/boot_serve.rs` exercises
/// this function.
///
/// Keep this a plain `fn` whose registrations stay in the `#[inline(never)]`
/// `mount_chunk_NN` helpers below, one frame at a time. Collapsing them into a single
/// chained expression holds every service's `new` frame at once and overflows
/// the stack here, aborting before either listener binds.
pub fn build_router(engine: Arc<InMemoryEngine>) -> MultiServiceRouter {
    let router = MultiServiceRouter::new();
    let router = mount_chunk_00(router, &engine);
    let router = mount_chunk_01(router, &engine);
    let router = mount_chunk_02(router, &engine);
    let router = mount_chunk_03(router, &engine);
    let router = mount_chunk_04(router, &engine);
    let router = mount_chunk_05(router, &engine);
    let router = mount_chunk_06(router, &engine);
    let router = mount_chunk_07(router, &engine);
    let router = mount_chunk_08(router, &engine);
    let router = mount_chunk_09(router, &engine);
    let router = mount_chunk_10(router, &engine);
    let router = mount_chunk_11(router, &engine);
    let router = mount_chunk_12(router, &engine);
    let router = mount_chunk_13(router, &engine);
    let router = mount_chunk_14(router, &engine);
    let router = mount_chunk_15(router, &engine);
    let router = mount_chunk_16(router, &engine);
    let router = mount_chunk_17(router, &engine);
    let router = mount_chunk_18(router, &engine);
    let router = mount_chunk_19(router, &engine);
    mount_chunk_20(router, &engine)
}

/// Register services 00 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_00(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_account_account::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_accountaccess_awsaccountaccess::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_acm_certificatemanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentregistry_agentregistry::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amp_amazonprometheusservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplify_amplify::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService::new(engine.clone()));
    router
}

/// Register services 01 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_01(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appfabric_fabricfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appintegrations_amazonappintegrationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationcostprofiler_awsapplicationcostprofiler::EmulatedService::new(engine.clone()));
    router
}

/// Register services 02 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_02(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_applicationdiscoveryservice_awsposeidonservice_v2015_11_01::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appmesh_appmesh::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apprunner_apprunner::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appstream_photonadminproxyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_arczonalshift_percdataplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_artifact_artifact::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_athena_amazonathena::EmulatedService::new(engine.clone()));
    router
}

/// Register services 03 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_03(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_auditmanager_bedrockassessmentmanagerlambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_b2bi_b2bi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_backupgateway_backuponpremises_v20210101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_backupsearch_cryobackupsearchservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::EmulatedService::new(engine.clone()));
    router
}

/// Register services 04 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_04(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_billing_awsbilling::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService::new(engine.clone()));
    router
}

/// Register services 05 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_05(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chatbot_wheatleyorchestration_20171011::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chime_ucbuzzconsoleservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 06 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_06(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_cloud9_awscloud9workspacemanagementservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudcontrol_cloudapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_clouddirectory_amazonclouddirectory_20170111::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudfrontkeyvaluestore_cloudfrontkeyvaluestore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudhsm_cloudhsmfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudhsmv2_baldrapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudsearch_a9searchcloudconfigservice2013::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudsearchdomain_amazoncloudsearch2013::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine.clone()));
    router
}

/// Register services 07 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_07(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_cloudtraildata_cloudtraildataservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatchevents_awsevents::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codecatalyst_codecatalyst::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codeconnections_codeconnections_20231201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(engine.clone()));
    router
}

/// Register services 08 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_08(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codegurureviewer_awsgurufrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codegurusecurity_awscodegurusecurity::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codestarconnections_codestar_connections_20191201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codestarnotifications_codestarnotifications_20191015::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cognitosync_awscognitosyncservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_comprehend_comprehend_20171127::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(engine.clone()));
    router
}

/// Register services 09 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_09(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_computeoptimizer_computeoptimizerservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connect_amazonconnectservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcases_amazonconnectcases::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcontactlens_amazonconnectcontactlens::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connecthealth_connecthealth::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectparticipant_amazonconnectparticipantservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_controlcatalog_controlcatalog::EmulatedService::new(engine.clone()));
    router
}

/// Register services 10 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_10(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_controltower_awscontroltowerapis::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_costexplorer_awsinsightsindexservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_costoptimizationhub_costoptimizationhubservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_customerprofiles_customerprofiles_20200815::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_databasemigrationservice_amazondmsv20160101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_databrew_awsgluedatabrew::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dataexchange_dataexchange::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_datapipeline_datapipeline::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_datazone_datazone::EmulatedService::new(engine.clone()));
    router
}

/// Register services 11 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_11(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_dax_amazondaxv3::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_deadline_deadline::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_detective_amazondetective::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_devicefarm_devicefarm_20150623::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_devopsguru_capstonecontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_directconnect_overtureservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dlm_dlm_20180112::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_docdb_amazonrdsv19::EmulatedService::new(engine.clone()));
    router
}

/// Register services 12 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_12(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_drs_elasticdisasterrecoveryservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dsql_dsql::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ebs_ebs::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine.clone()));
    router
}

/// Register services 13 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_13(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_efs_magnolioapiservice_v20150201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eksauth_eksauthfrontend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticache_amazonelasticachev9::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elementalinference_elementalinference::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_emrcontainers_awschicagowebservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 14 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_14(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_endusermessaging_endusermessaging::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_entityresolution_awsveniceservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eventbridgev2_awseventsv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_fms_awsfms_20180101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_forecast_amazonforecast::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_forecastquery_amazonforecastruntime::EmulatedService::new(engine.clone()));
    router
}

/// Register services 15 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_15(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_frauddetector_awshawksnestservicefacade::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_freetier_awsfreetierservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_fsx_awssimbaapiservice_v20180301::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_gamelift_gamelift::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_gameliftstreams_gameliftstreams::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_geomaps_mapsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_geoplaces_placesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_georoutes_routesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_glacier_glacier::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_globalaccelerator_globalaccelerator_v20180706::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(engine.clone()));
    router
}

/// Register services 16 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_16(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_greengrass_greengrass::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_groundstation_groundstation::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_guardduty_guarddutyapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_health_awshealth_20160804::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_healthlake_healthlake::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iamtoolbox_authrequestservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_identitystore_awsidentitystore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine.clone()));
    router
}

/// Register services 17 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_17(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_inspector_inspectorservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_inspector2_inspector2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_inspectorscan_inspectorscan::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_interconnect_interconnect::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_internetmonitor_internetmonitor20210603::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iot_awsiotservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotdeviceadvisor_iotsenateservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotfleetwise_iotautobahncontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 18 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_18(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotthingsgraph_iotthingsgraphfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotwireless_iotwireless::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService::new(engine.clone()));
    router
}

/// Register services 19 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_19(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_kendra_awskendrafrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisanalytics_kinesisanalytics_20150814::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 20 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_20(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_kinesisvideowebrtcstorage_awsacuityroutingservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lakeformation_awslakeformation::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService::new(engine.clone()));
    router
}

