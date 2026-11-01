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
#[path = "all/client_token_replay_outlives_a_state_change.rs"]
mod client_token_replay_outlives_a_state_change;
#[path = "all/clock_driven_timeout_is_materialised.rs"]
mod clock_driven_timeout_is_materialised;
#[path = "all/command_execution_concurrency.rs"]
mod command_execution_concurrency;
#[path = "all/command_execution_not_found.rs"]
mod command_execution_not_found;
#[path = "all/command_execution_parameter_names.rs"]
mod command_execution_parameter_names;
#[path = "all/command_execution_refusal_errors.rs"]
mod command_execution_refusal_errors;
#[path = "all/command_execution_validation.rs"]
mod command_execution_validation;
#[path = "all/describe_job_execution_against_the_clock.rs"]
mod describe_job_execution_against_the_clock;
#[path = "all/describe_next_honours_the_execution_number.rs"]
mod describe_next_honours_the_execution_number;
#[path = "all/job_execution_lifecycle.rs"]
mod job_execution_lifecycle;
#[path = "all/job_execution_state_machine.rs"]
mod job_execution_state_machine;
#[path = "all/nested_document_completeness.rs"]
mod nested_document_completeness;
#[path = "all/refusals_report_the_execution_state.rs"]
mod refusals_report_the_execution_state;
#[path = "all/response_document_completeness.rs"]
mod response_document_completeness;
#[path = "all/start_next_against_a_passed_deadline.rs"]
mod start_next_against_a_passed_deadline;
#[path = "all/start_next_applies_the_members_it_carries.rs"]
mod start_next_applies_the_members_it_carries;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_details_modeled_constraints.rs"]
mod status_details_modeled_constraints;
#[path = "all/status_details_refusal_names_one_offender.rs"]
mod status_details_refusal_names_one_offender;
#[path = "all/step_timeout_bound_is_shared.rs"]
mod step_timeout_bound_is_shared;
// The harness the suites above share: the emulator, its HTTP calls and the
// stored-record helpers, declared last so the list of SUITES stays sorted.
#[path = "all/support.rs"]
mod support;
#[path = "all/update_after_a_passed_deadline.rs"]
mod update_after_a_passed_deadline;
