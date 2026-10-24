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
#[path = "all/auto_scaling_group_recommendations.rs"]
mod auto_scaling_group_recommendations;
#[path = "all/auto_scaling_group_recommendations_export.rs"]
mod auto_scaling_group_recommendations_export;
#[path = "all/cross_account_requests.rs"]
mod cross_account_requests;
#[path = "all/ebs_volume_recommendations.rs"]
mod ebs_volume_recommendations;
#[path = "all/ebs_volume_recommendations_export.rs"]
mod ebs_volume_recommendations_export;
#[path = "all/ecs_service_recommendations.rs"]
mod ecs_service_recommendations;
#[path = "all/enrollment_and_preferences.rs"]
mod enrollment_and_preferences;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/idle_recommendations.rs"]
mod idle_recommendations;
#[path = "all/lambda_function_recommendations.rs"]
mod lambda_function_recommendations;
#[path = "all/license_recommendations.rs"]
mod license_recommendations;
#[path = "all/organization_enrollment_statuses.rs"]
mod organization_enrollment_statuses;
#[path = "all/preference_resource_type_support.rs"]
mod preference_resource_type_support;
#[path = "all/rds_database_recommendation_options.rs"]
mod rds_database_recommendation_options;
#[path = "all/rds_database_recommendation_projected_metrics.rs"]
mod rds_database_recommendation_projected_metrics;
#[path = "all/rds_database_recommendations.rs"]
mod rds_database_recommendations;
#[path = "all/rds_database_recommendations_export.rs"]
mod rds_database_recommendations_export;
#[path = "all/recommendation_export_jobs.rs"]
mod recommendation_export_jobs;
#[path = "all/recommendation_summaries.rs"]
mod recommendation_summaries;
#[path = "all/reported_defaults_and_preference_validation.rs"]
mod reported_defaults_and_preference_validation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unrestricted_preference_resource_types.rs"]
mod unrestricted_preference_resource_types;
