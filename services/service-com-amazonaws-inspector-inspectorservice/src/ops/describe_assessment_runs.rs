// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAssessmentRuns
//!
//! Reports the stored runs the requested ARNs name, and puts every ARN the store does not
//! hold into `failedItems` under `ITEM_DOES_NOT_EXIST`. Both members are always present, and
//! each run is reported exactly as the store holds it — nothing is echoed from the request.
//!
//! A reporting operation that still WRITES one thing: it first materialises the run state
//! changes the clock has already made due (see [`crate::state::AssessmentRunRecord::advance`]),
//! which were owed at this instant whether or not anybody called.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeAssessmentRuns`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-inspector-inspectorservice/smithy-model/com-amazonaws-inspector-inspectorservice.json")
//! ```
//!
//! Describes the assessment runs that are specified by the ARNs of the assessment runs.
//!
//! ## Input
//!
//! * `assessmentRunArns` (BatchDescribeArnList, len 1..10) **required** — The ARN that
//!   specifies the assessment run that you want to describe.
//!
//! ## Output
//!
//! * `assessmentRuns` (AssessmentRunList, len 0..10) **required** — Information about the
//!   assessment run.
//! * `failedItems` (FailedItems) **required** — Assessment run details that cannot be
//!   described. An error code is provided for each failed item.
//!
//! ## Errors
//!
//! * `InternalException` (HTTP 500) — Internal server error.
//! * `InvalidInputException` (HTTP 400) — The request was rejected because an invalid or
//!   out-of-range value was supplied for an input parameter.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAssessmentRunsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-inspector-inspectorservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeAssessmentRunsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.inspector#DescribeAssessmentRuns")]
#[derive(Debug)]
pub enum DescribeAssessmentRunsErrorModeled {
    InternalException(InternalException),
    InvalidInputException(InvalidInputException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_assessment_runs::DescribeAssessmentRunsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeAssessmentRunsError = crate::scaffolded::Scaffolded<DescribeAssessmentRunsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeAssessmentRunsErrorModeled> for DescribeAssessmentRunsError {
    fn from(modeled: DescribeAssessmentRunsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeAssessmentRuns` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeAssessmentRuns<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAssessmentRuns<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAssessmentRuns<Eng>
{
    type Input = DescribeAssessmentRunsInput;
    type Output = DescribeAssessmentRunsOutput;
    type Error = DescribeAssessmentRunsError;

    const NAME: &'static str = "DescribeAssessmentRuns";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ASSESSMENT_RUNS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ASSESSMENT_RUNS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribeAssessmentRunsErrorModeled::InternalException(InternalException {
                message: "the Inspector state store is unavailable".to_string(),
                can_retry: true,
            })
            .into());
        };
        let _ = ctx;

        let mut store = state.lock();
        store.advance_runs(&input.assessment_run_arns, ::std::time::SystemTime::now());

        let mut assessment_runs: Vec<AssessmentRun> = Vec::new();
        let mut failed_items: ::std::collections::HashMap<String, FailedItemDetails> =
            ::std::collections::HashMap::new();
        for arn in &input.assessment_run_arns {
            // A repeated ARN is described once: `failedItems` is keyed by ARN and the
            // response list is bounded by the request's own 1..10 bound.
            if assessment_runs.iter().any(|run| &run.arn == arn) {
                continue;
            }
            match store.assessment_runs.get(arn) {
                Some(record) => assessment_runs.push(record.run.clone()),
                None => {
                    failed_items.insert(
                        arn.clone(),
                        FailedItemDetails {
                            failure_code: crate::state::FAILED_ITEM_DOES_NOT_EXIST.to_string(),
                            retryable: false,
                        },
                    );
                }
            }
        }

        Ok(DescribeAssessmentRunsOutput {
            assessment_runs,
            failed_items,
        })
    }
}
