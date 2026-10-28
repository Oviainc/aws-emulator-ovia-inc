// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// The ONE harness every suite below shares: the wire-answer reduction and the job
// directory setup, declared first so each suite reaches it as `crate::harness`.
#[path = "all/harness.rs"]
mod harness;

#[path = "all/job_arn_is_the_callers_job.rs"]
mod job_arn_is_the_callers_job;

#[path = "all/job_resolution.rs"]
mod job_resolution;

#[path = "all/modelled_rejections.rs"]
mod modelled_rejections;

#[path = "all/refused_sampling_leaves_no_trajectory.rs"]
mod refused_sampling_leaves_no_trajectory;

#[path = "all/rejected_value_echo.rs"]
mod rejected_value_echo;

#[path = "all/rollout_operations_resolve_the_job.rs"]
mod rollout_operations_resolve_the_job;

#[path = "all/sampling_path_is_shared.rs"]
mod sampling_path_is_shared;

#[path = "all/shared_harness_is_one_path.rs"]
mod shared_harness_is_one_path;

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/trajectory_lifecycle.rs"]
mod trajectory_lifecycle;
