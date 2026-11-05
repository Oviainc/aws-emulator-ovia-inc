// SPDX-License-Identifier: CC0-1.0

//! @op GetCredentialReport
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming one of the conditions
//! this operation models. The input, output, error enum and schemas a generated stub would
//! have carried are declared in `src/ops/types/get_credential_report_types.rs` (the layout
//! every other operation of this service follows) and reached through `super::types::*`;
//! `build.rs` picks the operation up from the `@op` line above and wires it into the
//! service's routing table, exactly as it does for the other half of this feature,
//! `src/ops/generate_credential_report.rs`.
//!
//! ## What this operation answers on the wire
//!
//! A READ: it "retrieves a credential report for the Amazon Web Services account" — the
//! report `GenerateCredentialReport` starts the generation of. An account has ONE
//! credential report, so what a call answers is decided by the generation the account
//! holds ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::credential_report`])
//! and by the clock, and nothing is written: the artifact was rendered once, by the call
//! that started the generation, so two reads of one report answer the same bytes and the
//! same `GeneratedTime`.
//!
//! * no generation on record: the account has never had a report, which is the modelled
//!   `CredentialReportNotPresentException` (HTTP 410, awsQuery code `ReportNotPresent`) —
//!   "to generate a credential report, use GenerateCredentialReport";
//! * a generation that is not due yet: the report "is still being generated", the modelled
//!   `CredentialReportNotReadyException` (HTTP 404, awsQuery code `ReportInProgress`), so
//!   a caller that reads straight after starting a generation is told to poll rather than
//!   handed a half-made report;
//! * a report the clock has made due and that is still inside its validity window
//!   ([`crate::state::CREDENTIAL_REPORT_VALIDITY_SECONDS`]): served, `200`, with the one
//!   `ReportFormatType` value ([`REPORT_FORMAT`]), the instant the report was generated,
//!   and the `text/csv` artifact itself;
//! * a report past that window: "the most recent credential report has expired", the
//!   modelled `CredentialReportExpiredException` (HTTP 410, awsQuery code `ReportExpired`)
//!   — which is exactly when `GenerateCredentialReport` starts a new one again, so the
//!   refusal names a condition the account can act on.
//!
//! A caller whose identity-based policies do not permit the call is refused before anything
//! is reported, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetCredentialReport`
//!
//! Retrieves a credential report for the Amazon Web Services account. For more information
//! about the credential report, see Getting credential reports in the IAM User Guide.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `Content` (ReportContentType, blob) — Contains the credential report. The report is
//!   Base64-encoded.
//! * `ReportFormat` (ReportFormatType, one of: text/csv) — The format (MIME type) of the
//!   credential report.
//! * `GeneratedTime` (dateType) — The date and time when the credential report was created,
//!   in ISO 8601 date-time format.
//!
//! ## Errors
//!
//! * `CredentialReportNotReadyException` (HTTP 404) — The request was rejected because the
//!   credential report is still being generated.
//! * `CredentialReportNotPresentException` (HTTP 410) — The request was rejected because the
//!   credential report does not exist. To generate a credential report, use
//!   GenerateCredentialReport.
//! * `CredentialReportExpiredException` (HTTP 410) — The request was rejected because the
//!   most recent credential report has expired. To generate a new credential report, use
//!   GenerateCredentialReport.
//! * `ServiceFailureException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

/// The `ReportFormat` every served report carries: the ONE value `ReportFormatType`
/// declares ([`super::types::REPORT_FORMAT_TYPE_VALUES`]), which is also the MIME type of
/// the artifact this emulator renders.
const REPORT_FORMAT: &str = "text/csv";

/// The message each refusal of this operation carries: NONE.
///
/// The one refusal of this family that was measured — the not-ready refusal a read made
/// while a generation is still running — carries no message at all: the `<Message>` element
/// is absent, so a client reading one is left with the empty default its own parser fills
/// in, and the condition is named by the error code alone. The other two conditions are
/// raised by the same service component about the same single report, and are worded the
/// same way rather than one of them inventing a sentence of its own.
const UNWORDED: Option<String> = None;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetCredentialReportError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#GetCredentialReport")]
#[derive(Debug)]
pub enum GetCredentialReportErrorModeled {
    CredentialReportExpiredException(CredentialReportExpiredException),
    CredentialReportNotPresentException(CredentialReportNotPresentException),
    CredentialReportNotReadyException(CredentialReportNotReadyException),
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_credential_report::GetCredentialReportError`).
pub type GetCredentialReportError = crate::scaffolded::Scaffolded<GetCredentialReportErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetCredentialReportErrorModeled> for GetCredentialReportError {
    fn from(modeled: GetCredentialReportErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetCredentialReport` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetCredentialReport<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetCredentialReport<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetCredentialReport<Eng>
{
    type Input = GetCredentialReportInput;
    type Output = GetCredentialReportOutput;
    type Error = GetCredentialReportError;

    const NAME: &'static str = "GetCredentialReport";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_CREDENTIAL_REPORT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_CREDENTIAL_REPORT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it answers comes from the account's own
        // record of its credential report and from the clock.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetCredentialReportErrorModeled::ServiceFailureException(
                ServiceFailureException {
                    message: Some("the IAM store is unavailable".to_string()),
                },
            )
            .into());
        };

        // A READ: the report was rendered by the call that started its generation, so this
        // operation only consults the record and writes nothing — asking for the report
        // changes nothing a later caller sees, including what the next
        // `GenerateCredentialReport` decides.
        let store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is reported. The report is the ACCOUNT's, not one
        // entity's, and the Service Authorization Reference lists no resource type for the
        // action, so the decision is made against the account-wide resource — exactly as
        // its sibling `GenerateCredentialReport` and the other account-wide operations of
        // this service decide.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::GET_CREDENTIAL_REPORT,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        // An account that has never asked for a report has none to read.
        let Some(held) = store.credential_report.as_ref() else {
            return Err(
                GetCredentialReportErrorModeled::CredentialReportNotPresentException(
                    CredentialReportNotPresentException { message: UNWORDED },
                )
                .into(),
            );
        };

        // The generation's own due instant, stored rather than slept for, is what makes a
        // read taken straight after the generate see the report still being made.
        let now = crate::state::now();
        if now.as_secs_f64() < held.completes.as_secs_f64() {
            return Err(
                GetCredentialReportErrorModeled::CredentialReportNotReadyException(
                    CredentialReportNotReadyException { message: UNWORDED },
                )
                .into(),
            );
        }

        // A report is downloadable while it is inside the window it was generated for;
        // from the end of that window the account needs a fresh generation, which is the
        // condition this refusal names.
        if now.secs() >= held.requested.secs() + crate::state::CREDENTIAL_REPORT_VALIDITY_SECONDS {
            return Err(
                GetCredentialReportErrorModeled::CredentialReportExpiredException(
                    CredentialReportExpiredException { message: UNWORDED },
                )
                .into(),
            );
        }

        Ok(GetCredentialReportOutput {
            content: Some(held.content.clone().into_bytes()),
            report_format: Some(REPORT_FORMAT.to_string()),
            generated_time: Some(held.requested),
        })
    }
}
