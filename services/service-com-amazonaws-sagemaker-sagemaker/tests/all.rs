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
#[path = "all/ai_workload_config_lifecycle.rs"]
mod ai_workload_config_lifecycle;
#[path = "all/app_image_config_registry.rs"]
mod app_image_config_registry;
#[path = "all/artifact_lineage.rs"]
mod artifact_lineage;
#[path = "all/batch_transform_jobs.rs"]
mod batch_transform_jobs;
#[path = "all/code_repositories.rs"]
mod code_repositories;
#[path = "all/custom_images.rs"]
mod custom_images;
#[path = "all/endpoint_config_deletion.rs"]
mod endpoint_config_deletion;
#[path = "all/hub_contents_listing.rs"]
mod hub_contents_listing;
#[path = "all/inference_deployment_lifecycle.rs"]
mod inference_deployment_lifecycle;
#[path = "all/lineage_action_lifecycle.rs"]
mod lineage_action_lifecycle;
#[path = "all/list_endpoints.rs"]
mod list_endpoints;
#[path = "all/mlflow_app_lifecycle.rs"]
mod mlflow_app_lifecycle;
#[path = "all/model_card_governance.rs"]
mod model_card_governance;
#[path = "all/model_package_group_lifecycle.rs"]
mod model_package_group_lifecycle;
#[path = "all/model_package_registry.rs"]
mod model_package_registry;
#[path = "all/notebook_instance_lifecycle.rs"]
mod notebook_instance_lifecycle;
#[path = "all/notebook_lifecycle_config.rs"]
mod notebook_lifecycle_config;
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;
#[path = "all/processing_job_lifecycle.rs"]
mod processing_job_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/studio_app_lifecycle.rs"]
mod studio_app_lifecycle;
#[path = "all/trial_component_tracking.rs"]
mod trial_component_tracking;
