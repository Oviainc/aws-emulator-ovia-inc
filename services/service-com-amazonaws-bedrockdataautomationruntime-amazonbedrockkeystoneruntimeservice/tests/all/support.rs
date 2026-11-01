// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! A submission names WHAT to extract by reference, and the referenced data-automation
//! project belongs to the build-time service rather than to this one, so a suite whose
//! submissions name a project has to ARRANGE that project in the owning service's store
//! before the submission can be accepted.
//!
//! [`seed_project`] is that arrangement, for a suite that is about something else and
//! names its project by a FIXED ARN (the ARN a create mints is the store's to choose, so
//! a suite that wants a particular one writes the record). The suite that is about the
//! resolution rule itself (`control_plane_reference_resolution`) does not use this: it
//! creates every project and blueprint it references through the build-time service's own
//! `CreateDataAutomationProject` / `CreateBlueprint` over the wire.

use std::time::SystemTime;

use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state as build_time;

/// The `invocationId` inside a minted invocation ARN: the segment after the resource
/// prefix the ARN is built around (`state::invocation_arn`).
///
/// A job's output lives under a segment of its OWN id below the submitted output prefix,
/// so the id is what a reader's reported output location is checked against
/// ([`job_metadata_uri`]).
pub fn invocation_id(invocation_arn: &str) -> &str {
    invocation_arn.split('/').next_back().unwrap_or_default()
}

/// Where `GetDataAutomationStatus` reports the output of the job `invocation_arn` names,
/// for a submission whose `outputConfiguration.s3Uri` was `output_prefix`: that job's own
/// `job_metadata.json`, under its id below the prefix.
///
/// For the suites whose subject is something else and which only have to know that the
/// reader answers for the record their submission minted; the rule itself — including how
/// a prefix with and without a trailing separator are joined — is asserted row by row in
/// `job_output_location_and_outcome`.
pub fn job_metadata_uri(output_prefix: &str, invocation_arn: &str) -> String {
    format!(
        "{}/{}/job_metadata.json",
        output_prefix.trim_end_matches('/'),
        invocation_id(invocation_arn)
    )
}

/// Register one data-automation project, under `project_arn` and named `project_name`, in
/// the build-time service's store on `engine` — a project in its settled stage, as a
/// create that has already completed leaves it.
pub fn seed_project(engine: &engine::InMemoryEngine, project_arn: &str, project_name: &str) {
    let state = build_time::service_state(engine).expect("the build-time store on this engine");
    let created = build_time::now();
    let mut store = state.lock();
    store.projects.insert(
        project_arn.to_string(),
        build_time::DataAutomationProjectRecord {
            project_arn: project_arn.to_string(),
            project_name: project_name.to_string(),
            project_stage: build_time::DEFAULT_PROJECT_STAGE.to_string(),
            project_type: "ASYNC".to_string(),
            project_description: None,
            standard_output_configuration: None,
            custom_output_configuration: None,
            override_configuration: None,
            data_automation_library_configuration: None,
            creation_time: created,
            last_modified_time: created,
            kms_key_id: None,
            kms_encryption_context: None,
            completes_at: SystemTime::now(),
            tags: Vec::new(),
        },
    );
    store
        .project_names
        .insert(project_name.to_string(), project_arn.to_string());
}
