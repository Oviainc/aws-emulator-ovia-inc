// SPDX-License-Identifier: CC0-1.0

//! @op GetSuiteRunReport
//!
//! Reports the download URL of one suite run's QUALIFICATION REPORT: the report AWS
//! documents for "a successful Device Advisor qualifying test suite run".
//!
//! Whether a report exists at all is the store's one rule
//! ([`crate::state::ComAmazonawsIotdeviceadvisorIotsenateserviceState::qualification_report`]):
//! the run belongs to the definition the caller named, that definition is intended for
//! qualification, and the run SUCCEEDED. Anything else names no report (an id pair the
//! store does not hold, a run of a suite that is not a qualification suite, a run still in
//! progress, a run a caller stopped, or one that failed or errored) and is refused with the
//! modelled `ResourceNotFoundException`, worded with the one not-found template this
//! service answers a missing resource with ([`crate::state::not_found_message`]).
//!
//! No run of this emulator succeeds: nothing executes the suite against the device under
//! test, so a run ends in `ERROR` carrying [`crate::state::NO_TEST_RUNNER_REASON`] (see
//! `start_suite_run`), and this operation therefore refuses every run the emulator can
//! produce rather than reporting a report for an unsuccessful one. The rule above is the
//! SERVICE's, read off the store, so the report is announced the moment a run can succeed.
//!
//! A report, not a mutation: the only write it makes is materialising the status
//! transitions the CLOCK had already made due
//! ([`crate::state::SuiteRun::materialize_due`]), the same state the next caller would
//! have seen had nobody polled.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetSuiteRunReport::new(engine)` and drives it through the centralized request walk:
//! deserialize -> validate -> `handle` -> serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request, so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only:
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetSuiteRunReport`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped. For the FULL text of any one item, ask the model
//! rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json")
//! ```
//!
//! Gets a report download link for a successful Device Advisor qualifying test suite run.
//! Requires permission to access the GetSuiteRunReport action.
//!
//! ## Input
//!
//! * `suiteDefinitionId` (UUID, len 12..36) **required** - Suite definition ID of the test
//!   suite.
//! * `suiteRunId` (UUID, len 12..36) **required** - Suite run ID of the test suite run.
//!
//! ## Output
//!
//! * `qualificationReportDownloadUrl` (QualificationReportDownloadUrl) - Download URL of the
//!   qualification report.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) - Sends an Internal Failure exception.
//! * `ResourceNotFoundException` (HTTP 404) - Sends a Resource Not Found exception.
//! * `ValidationException` (HTTP 400) - Sends a validation exception.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetSuiteRunReportError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetSuiteRunReportError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iotdeviceadvisor#GetSuiteRunReport")]
#[derive(Debug)]
pub enum GetSuiteRunReportErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_suite_run_report::GetSuiteRunReportError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetSuiteRunReportError = crate::scaffolded::Scaffolded<GetSuiteRunReportErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetSuiteRunReportErrorModeled> for GetSuiteRunReportError {
    fn from(modeled: GetSuiteRunReportErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSuiteRunReport` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetSuiteRunReport<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSuiteRunReport<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSuiteRunReport<Eng>
{
    type Input = GetSuiteRunReportInput;
    type Output = GetSuiteRunReportOutput;
    type Error = GetSuiteRunReportError;

    const NAME: &'static str = "GetSuiteRunReport";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SUITE_RUN_REPORT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SUITE_RUN_REPORT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetSuiteRunReportErrorModeled::InternalServerException(InternalServerException {
                message: Some("the iotdeviceadvisor store is unavailable".to_string()),
            })
        })?;

        // The endpoint the caller reached this emulator on, because that is the endpoint
        // the URL announced below has to resolve back to.
        let host = crate::state::report_url_host(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // Materialise what the clock already owed the run the caller named, and nothing
        // else: a run whose status moved while nobody was polling is judged on the status
        // it would have carried had nobody called at all.
        let _ = store.run_mut(&input.suite_definition_id, &input.suite_run_id, now);

        let Some((definition, run)) =
            store.qualification_report(&input.suite_definition_id, &input.suite_run_id)
        else {
            return Err(GetSuiteRunReportErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::not_found_message(
                        "suiteRunId",
                        &input.suite_run_id,
                    )),
                },
            )
            .into());
        };

        Ok(GetSuiteRunReportOutput {
            qualification_report_download_url: Some(crate::state::qualification_report_url(
                &host, definition, run,
            )),
        })
    }
}
