// SPDX-License-Identifier: CC0-1.0

//! @op DeleteAssessmentTarget
//!
//! Removes a stored assessment target and CASCADES to everything beneath it in the Inspector
//! hierarchy — its templates and those templates' runs — so a later describe reports all of
//! those ARNs in `failedItems` rather than returning them.
//!
//! It refuses while any of those runs is still in progress, with the
//! `AssessmentRunInProgressException` this operation models: "You cannot perform a specified
//! action if an assessment run is currently in progress."
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteAssessmentTarget`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-inspector-inspectorservice/smithy-model/com-amazonaws-inspector-inspectorservice.json")
//! ```
//!
//! Deletes the assessment target that is specified by the ARN of the assessment target.
//!
//! ## Input
//!
//! * `assessmentTargetArn` (Arn, len 1..300) **required** — The ARN that specifies the
//!   assessment target that you want to delete.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have required permissions to access the
//!   requested resource.
//! * `AssessmentRunInProgressException` (HTTP 400) — You cannot perform a specified action if
//!   an assessment run is currently in progress.
//! * `InternalException` (HTTP 500) — Internal server error.
//! * `InvalidInputException` (HTTP 400) — The request was rejected because an invalid or
//!   out-of-range value was supplied for an input parameter.
//! * `NoSuchEntityException` (HTTP 400) — The request was rejected because it referenced an
//!   entity that does not exist. The error code describes the entity.
//! * `ServiceTemporarilyUnavailableException` (HTTP 503) — The serice is temporary
//!   unavailable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteAssessmentTargetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-inspector-inspectorservice.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteAssessmentTargetOutput;

impl DeleteAssessmentTargetOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static DELETE_ASSESSMENT_TARGET_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteAssessmentTargetOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteAssessmentTargetOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteAssessmentTargetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.inspector#DeleteAssessmentTarget")]
#[derive(Debug)]
pub enum DeleteAssessmentTargetErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AssessmentRunInProgressException(AssessmentRunInProgressException),
    InternalException(InternalException),
    InvalidInputException(InvalidInputException),
    NoSuchEntityException(NoSuchEntityException),
    ServiceTemporarilyUnavailableException(ServiceTemporarilyUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_assessment_target::DeleteAssessmentTargetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteAssessmentTargetError = crate::scaffolded::Scaffolded<DeleteAssessmentTargetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteAssessmentTargetErrorModeled> for DeleteAssessmentTargetError {
    fn from(modeled: DeleteAssessmentTargetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteAssessmentTarget` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteAssessmentTarget<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteAssessmentTarget<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// How many in-progress run ARNs `AssessmentRunInProgressException` can carry
/// (`AssessmentRunInProgressArnList`, len 1..10); past that the list is truncated and says so.
const MAX_REPORTED_RUNS_IN_PROGRESS: usize = 10;

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteAssessmentTarget<Eng>
{
    type Input = DeleteAssessmentTargetInput;
    type Output = DeleteAssessmentTargetOutput;
    type Error = DeleteAssessmentTargetError;

    const NAME: &'static str = "DeleteAssessmentTarget";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ASSESSMENT_TARGET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_ASSESSMENT_TARGET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteAssessmentTargetErrorModeled::InternalException(InternalException {
                message: "the Inspector state store is unavailable".to_string(),
                can_retry: true,
            })
            .into());
        };
        let _ = ctx;

        let mut store = state.lock();

        if !store
            .assessment_targets
            .contains_key(&input.assessment_target_arn)
        {
            return Err(DeleteAssessmentTargetErrorModeled::NoSuchEntityException(
                NoSuchEntityException {
                    message: format!(
                        "The assessment target {} does not exist",
                        input.assessment_target_arn
                    ),
                    error_code: "ASSESSMENT_TARGET_DOES_NOT_EXIST".to_string(),
                    can_retry: false,
                },
            )
            .into());
        }

        let template_arns = store.templates_of_target(&input.assessment_target_arn);
        // Settle the run state the clock already owes this target's runs before judging what
        // is in progress, so a run whose window closed does not block the delete.
        store.advance_runs_of_templates(&template_arns, ::std::time::SystemTime::now());
        let in_progress = store.runs_in_progress(&template_arns);
        if !in_progress.is_empty() {
            let truncated = in_progress.len() > MAX_REPORTED_RUNS_IN_PROGRESS;
            return Err(
                DeleteAssessmentTargetErrorModeled::AssessmentRunInProgressException(
                    AssessmentRunInProgressException {
                        message: format!(
                            "The assessment target {} cannot be deleted while {} of its assessment runs are in progress",
                            input.assessment_target_arn,
                            in_progress.len()
                        ),
                        assessment_run_arns: in_progress
                            .into_iter()
                            .take(MAX_REPORTED_RUNS_IN_PROGRESS)
                            .collect(),
                        assessment_run_arns_truncated: truncated,
                        can_retry: false,
                    },
                )
                .into(),
            );
        }

        // Cascade: the target's templates, and those templates' runs, go with it. One pass
        // over each map with a set lookup per record, so the cascade is linear in the stored
        // records (each capped in `crate::state`), not a scan per template.
        let doomed: ::std::collections::HashSet<&str> =
            template_arns.iter().map(String::as_str).collect();
        store
            .assessment_runs
            .retain(|_, record| !doomed.contains(record.run.assessment_template_arn.as_str()));
        store
            .assessment_templates
            .retain(|arn, _| !doomed.contains(arn.as_str()));
        store
            .assessment_targets
            .remove(&input.assessment_target_arn);

        Ok(DeleteAssessmentTargetOutput)
    }
}
