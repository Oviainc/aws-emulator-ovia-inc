// SPDX-License-Identifier: CC0-1.0

//! @op StartAssessmentRun
//!
//! Starts a run from a stored template: the run's duration, rules packages and user
//! attributes are COPIED from that template (never from the request), the template's
//! `assessmentRunCount` and `lastAssessmentRunArn` are updated, and the run is stored under
//! `<template arn>/run/0-xxxxxxxx`.
//!
//! The run is returned in the non-terminal `CREATED` state and owes two transitions the
//! clock materialises later (see [`crate::state::AssessmentRunRecord`]): data collection
//! starts after [`crate::state::DATA_COLLECTION_START_DELAY`], and the run completes once
//! the template's `durationInSeconds` collection window has elapsed.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartAssessmentRun`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-inspector-inspectorservice/smithy-model/com-amazonaws-inspector-inspectorservice.json")
//! ```
//!
//! Starts the assessment run specified by the ARN of the assessment template. For this API
//! to function properly, you must not exceed the limit of running up to 500 concurrent
//! agents per AWS account.
//!
//! ## Input
//!
//! * `assessmentRunName` (AssessmentRunName, len 1..140) — You can specify the name for the
//!   assessment run. The name must be unique for the assessment template whose ARN is used to
//!   start the assessment run.
//! * `assessmentTemplateArn` (Arn, len 1..300) **required** — The ARN of the assessment
//!   template of the assessment run that you want to start.
//!
//! ## Output
//!
//! * `assessmentRunArn` (Arn, len 1..300) **required** — The ARN of the assessment run that
//!   has been started.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have required permissions to access the
//!   requested resource.
//! * `AgentsAlreadyRunningAssessmentException` (HTTP 400) — You started an assessment run,
//!   but one of the instances is already participating in another assessment run.
//! * `InternalException` (HTTP 500) — Internal server error.
//! * `InvalidCrossAccountRoleException` (HTTP 400) — Amazon Inspector cannot assume the
//!   cross-account role that it needs to list your EC2 instances during the assessment run.
//! * `InvalidInputException` (HTTP 400) — The request was rejected because an invalid or
//!   out-of-range value was supplied for an input parameter.
//! * `LimitExceededException` (HTTP 400) — The request was rejected because it attempted to
//!   create resources beyond the current AWS account limits. The error code describes the
//!   limit exceeded.
//! * `NoSuchEntityException` (HTTP 400) — The request was rejected because it referenced an
//!   entity that does not exist. The error code describes the entity.
//! * `ServiceTemporarilyUnavailableException` (HTTP 503) — The serice is temporary
//!   unavailable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartAssessmentRunError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-inspector-inspectorservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartAssessmentRunError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.inspector#StartAssessmentRun")]
#[derive(Debug)]
pub enum StartAssessmentRunErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AgentsAlreadyRunningAssessmentException(AgentsAlreadyRunningAssessmentException),
    InternalException(InternalException),
    InvalidCrossAccountRoleException(InvalidCrossAccountRoleException),
    InvalidInputException(InvalidInputException),
    LimitExceededException(LimitExceededException),
    NoSuchEntityException(NoSuchEntityException),
    ServiceTemporarilyUnavailableException(ServiceTemporarilyUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_assessment_run::StartAssessmentRunError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartAssessmentRunError = crate::scaffolded::Scaffolded<StartAssessmentRunErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartAssessmentRunErrorModeled> for StartAssessmentRunError {
    fn from(modeled: StartAssessmentRunErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartAssessmentRun` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartAssessmentRun<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartAssessmentRun<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// `AssessmentRunName`'s modelled maximum length, which a derived name must also respect.
const MAX_RUN_NAME_LEN: usize = 140;

/// The name the service gives a run the caller did not name: the template's name with the
/// run's own id appended, clipped to the modelled name length.
fn derived_run_name(template_name: &str, run_id: &str) -> String {
    let suffix = format!("-{run_id}");
    let head = crate::state::truncate_chars(
        template_name,
        MAX_RUN_NAME_LEN.saturating_sub(suffix.chars().count()),
    );
    format!("{head}{suffix}")
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartAssessmentRun<Eng>
{
    type Input = StartAssessmentRunInput;
    type Output = StartAssessmentRunOutput;
    type Error = StartAssessmentRunError;

    const NAME: &'static str = "StartAssessmentRun";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_ASSESSMENT_RUN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_ASSESSMENT_RUN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(StartAssessmentRunErrorModeled::InternalException(InternalException {
                message: "the Inspector state store is unavailable".to_string(),
                can_retry: true,
            })
            .into());
        };
        let now = ::std::time::SystemTime::now();
        let _ = ctx;

        let mut store = state.lock();

        let Some(template) = store
            .assessment_templates
            .get(&input.assessment_template_arn)
            .cloned()
        else {
            return Err(StartAssessmentRunErrorModeled::NoSuchEntityException(
                NoSuchEntityException {
                    message: format!(
                        "The assessment template {} does not exist",
                        input.assessment_template_arn
                    ),
                    error_code: "ASSESSMENT_TEMPLATE_DOES_NOT_EXIST".to_string(),
                    can_retry: false,
                },
            )
            .into());
        };

        // An assessment run reports 1..50 rules packages (`AssessmentRun$rulesPackageArns`),
        // so a template that attached none has nothing to evaluate and cannot start one.
        if template.rules_package_arns.is_empty() {
            return Err(StartAssessmentRunErrorModeled::InvalidInputException(
                InvalidInputException {
                    message: format!(
                        "The assessment template {} has no rules packages attached",
                        template.arn
                    ),
                    error_code: "INVALID_NUMBER_OF_RULES_PACKAGE_ARNS".to_string(),
                    can_retry: false,
                },
            )
            .into());
        }

        if store.assessment_runs.len() >= crate::state::MAX_ASSESSMENT_RUNS
            || store.run_count_of_template(&template.arn)
                >= crate::state::MAX_ASSESSMENT_RUNS_PER_TEMPLATE
        {
            return Err(StartAssessmentRunErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: format!(
                        "The account already holds the maximum of {} assessment runs ({} per assessment template)",
                        crate::state::MAX_ASSESSMENT_RUNS,
                        crate::state::MAX_ASSESSMENT_RUNS_PER_TEMPLATE
                    ),
                    error_code: "ASSESSMENT_RUN_LIMIT_EXCEEDED".to_string(),
                    can_retry: false,
                },
            )
            .into());
        }

        let id = store.mint_id();
        let arn = format!("{}/run/{id}", template.arn);
        let name = input
            .assessment_run_name
            .unwrap_or_else(|| derived_run_name(&template.name, &id));
        let started_at = crate::state::date_time(now);

        let run = AssessmentRun {
            arn: arn.clone(),
            name,
            assessment_template_arn: template.arn.clone(),
            state: crate::state::RUN_STATE_CREATED.to_string(),
            // Copied from the template: the run reports what it was configured with, not
            // what this request carried.
            duration_in_seconds: template.duration_in_seconds,
            rules_package_arns: template.rules_package_arns.clone(),
            user_attributes_for_findings: template.user_attributes_for_findings.clone(),
            created_at: started_at,
            started_at: Some(started_at),
            completed_at: None,
            state_changed_at: started_at,
            data_collected: false,
            state_changes: vec![AssessmentRunStateChange {
                state_changed_at: started_at,
                state: crate::state::RUN_STATE_CREATED.to_string(),
            }],
            // Notifications arrive from event subscriptions, of which this store holds none.
            notifications: Vec::new(),
            // No finding has been reported yet, so there is no count to report.
            finding_counts: ::std::collections::HashMap::new(),
        };

        let collecting_data_due_at = now + crate::state::DATA_COLLECTION_START_DELAY;
        let seconds = u64::try_from(template.duration_in_seconds).unwrap_or_default();
        let window = ::std::time::Duration::from_secs(seconds);
        store.assessment_runs.insert(
            arn.clone(),
            crate::state::AssessmentRunRecord {
                run,
                collecting_data_due_at,
                completion_due_at: collecting_data_due_at + window,
            },
        );

        if let Some(stored) = store.assessment_templates.get_mut(&template.arn) {
            stored.assessment_run_count = stored.assessment_run_count.saturating_add(1);
            stored.last_assessment_run_arn = Some(arn.clone());
        }

        Ok(StartAssessmentRunOutput {
            assessment_run_arn: arn,
        })
    }
}
