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
#[path = "all/ai_benchmark_job_lifecycle.rs"]
mod ai_benchmark_job_lifecycle;
#[path = "all/ai_workload_config_lifecycle.rs"]
mod ai_workload_config_lifecycle;
#[path = "all/algorithm_lifecycle.rs"]
mod algorithm_lifecycle;
#[path = "all/app_image_config_registry.rs"]
mod app_image_config_registry;
#[path = "all/artifact_deletion.rs"]
mod artifact_deletion;
#[path = "all/artifact_lineage.rs"]
mod artifact_lineage;
#[path = "all/automl_job_v2_lifecycle.rs"]
mod automl_job_v2_lifecycle;
#[path = "all/batch_describe_model_packages.rs"]
mod batch_describe_model_packages;
#[path = "all/batch_transform_jobs.rs"]
mod batch_transform_jobs;
#[path = "all/cluster_scheduler_config_lifecycle.rs"]
mod cluster_scheduler_config_lifecycle;
#[path = "all/code_repositories.rs"]
mod code_repositories;
#[path = "all/code_repository_deletion.rs"]
mod code_repository_deletion;
#[path = "all/compute_quota_lifecycle.rs"]
mod compute_quota_lifecycle;
#[path = "all/control_plane_preconditions.rs"]
mod control_plane_preconditions;
#[path = "all/custom_images.rs"]
mod custom_images;
#[path = "all/endpoint_config_deletion.rs"]
mod endpoint_config_deletion;
#[path = "all/experiment_lifecycle.rs"]
mod experiment_lifecycle;
#[path = "all/feature_group_lifecycle.rs"]
mod feature_group_lifecycle;
#[path = "all/hub_contents_listing.rs"]
mod hub_contents_listing;
#[path = "all/hub_registry.rs"]
mod hub_registry;
#[path = "all/hyperparameter_tuning_job.rs"]
mod hyperparameter_tuning_job;
#[path = "all/hyperpod_cluster_audit_record.rs"]
mod hyperpod_cluster_audit_record;
#[path = "all/hyperpod_cluster_listing.rs"]
mod hyperpod_cluster_listing;
#[path = "all/inference_deployment_lifecycle.rs"]
mod inference_deployment_lifecycle;
#[path = "all/inference_recommendations_jobs.rs"]
mod inference_recommendations_jobs;
#[path = "all/lineage_action_lifecycle.rs"]
mod lineage_action_lifecycle;
#[path = "all/lineage_associations.rs"]
mod lineage_associations;
#[path = "all/lineage_context_lifecycle.rs"]
mod lineage_context_lifecycle;
#[path = "all/lineage_group_reporting.rs"]
mod lineage_group_reporting;
#[path = "all/list_artifacts.rs"]
mod list_artifacts;
#[path = "all/list_code_repositories.rs"]
mod list_code_repositories;
#[path = "all/list_endpoints.rs"]
mod list_endpoints;
#[path = "all/list_model_packages.rs"]
mod list_model_packages;
#[path = "all/list_pipeline_execution_steps.rs"]
mod list_pipeline_execution_steps;
#[path = "all/list_training_jobs.rs"]
mod list_training_jobs;
#[path = "all/mlflow_app_lifecycle.rs"]
mod mlflow_app_lifecycle;
#[path = "all/mlflow_tracking_server.rs"]
mod mlflow_tracking_server;
#[path = "all/model_card_deletion.rs"]
mod model_card_deletion;
#[path = "all/model_card_export_jobs.rs"]
mod model_card_export_jobs;
#[path = "all/model_card_governance.rs"]
mod model_card_governance;
#[path = "all/model_card_updates.rs"]
mod model_card_updates;
#[path = "all/model_package_group_lifecycle.rs"]
mod model_package_group_lifecycle;
#[path = "all/model_package_group_policy.rs"]
mod model_package_group_policy;
#[path = "all/model_package_registry.rs"]
mod model_package_registry;
#[path = "all/model_package_updates.rs"]
mod model_package_updates;
#[path = "all/notebook_instance_lifecycle.rs"]
mod notebook_instance_lifecycle;
#[path = "all/notebook_instance_stop.rs"]
mod notebook_instance_stop;
#[path = "all/notebook_lifecycle_config.rs"]
mod notebook_lifecycle_config;
#[path = "all/partner_apps.rs"]
mod partner_apps;
#[path = "all/pipeline_execution.rs"]
mod pipeline_execution;
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;
#[path = "all/presigned_notebook_instance_url.rs"]
mod presigned_notebook_instance_url;
#[path = "all/processing_job_lifecycle.rs"]
mod processing_job_lifecycle;
#[path = "all/rejection_fidelity.rs"]
mod rejection_fidelity;
#[path = "all/servicecatalog_portfolio_status.rs"]
mod servicecatalog_portfolio_status;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/studio_app_lifecycle.rs"]
mod studio_app_lifecycle;
#[path = "all/trial_component_tracking.rs"]
mod trial_component_tracking;
// Declared after the sorted list rather than among the other `hyperpod_*` suites: the
// cluster operations are being implemented one issue at a time, so that run of lines is
// where every concurrent cluster change inserts its own suite.
#[path = "all/hyperpod_cluster_lifecycle.rs"]
mod hyperpod_cluster_lifecycle;
#[path = "all/create_cluster_admission.rs"]
mod create_cluster_admission;
