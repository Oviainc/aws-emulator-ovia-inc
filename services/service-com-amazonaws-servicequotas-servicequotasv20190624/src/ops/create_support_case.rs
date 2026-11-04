// SPDX-License-Identifier: CC0-1.0

//! @op CreateSupportCase
//!
//! Opens a support case for a stored quota increase request that is still awaiting a
//! decision, and writes it onto that request: the record keeps its `Id`,
//! `DesiredValue` and `Created`, and gains the `CaseId` of the case plus the
//! `CASE_OPENED` status the model's `RequestStatus` documents for a request a support
//! case is processing. `GetRequestedServiceQuotaChange` and the history listings read
//! the same record back.
//!
//! The case is the review the decision waits on, so the decision becomes due
//! [`crate::state::REQUEST_DECISION_DELAY`] after the case was opened and the increase
//! is then `APPROVED` with its `DesiredValue` in force — including for a request whose
//! submitter had opted out of the support case Service Quotas would have opened
//! itself, because a case now reviews it. The response body is empty: the model's
//! `CreateSupportCaseResponse` declares no members.
//!
//! This is how a case is opened on a request BEFORE Service Quotas takes it up: a
//! request that allows one has its case opened by the service itself once it is taken
//! up, and from then on it already carries a case.
//!
//! Refusals:
//!
//! * a `RequestId` no request this endpoint resolves carries ->
//!   `NoSuchResourceException` (404): a request is reached on the plane it was made on,
//!   so a request for a quota the catalog counts account-wide is reachable at every
//!   regional endpoint and one for a quota counted per region at the endpoint it was
//!   made at;
//! * a request that is no longer awaiting a decision ->
//!   `InvalidResourceStateException` (405): the operation "only creates a Support case
//!   if the request has a Pending status", and a decided request has nothing left for a
//!   case to process — whether or not a case was ever opened for it;
//! * a request still awaiting a decision that already has a support case ->
//!   `ResourceAlreadyExistsException` (400): one request has at most one case.
//!
//! Every step the clock has already made due is materialised before the request is
//! read, so a request whose decision fell due before this call is refused on the status
//! it had earned rather than having a case opened for it, and one Service Quotas has
//! already opened a case for is refused for carrying that case.
//!
//! ---
//!
//! # Modeled contract for `CreateSupportCase`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Creates a Support case for an existing quota increase request. This call only creates a
//! Support case if the request has a Pending status.
//!
//! ## Input
//!
//! * `RequestId` (RequestId, len 1..128, pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$)
//!   **required** — The ID of the pending quota increase request for which you want to open a
//!   Support case.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `DependencyAccessDeniedException` (HTTP 403) — You can't perform this action because a
//!   dependency does not have access.
//! * `IllegalArgumentException` (HTTP 400) — Invalid input was provided.
//! * `InvalidResourceStateException` (HTTP 405) — The resource is in an invalid state.
//! * `NoSuchResourceException` (HTTP 404) — The specified resource does not exist.
//! * `ResourceAlreadyExistsException` (HTTP 400) — The specified resource already exists.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateSupportCaseError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#CreateSupportCase")]
#[derive(Debug)]
pub enum CreateSupportCaseErrorModeled {
    AccessDeniedException(AccessDeniedException),
    DependencyAccessDeniedException(DependencyAccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidResourceStateException(InvalidResourceStateException),
    NoSuchResourceException(NoSuchResourceException),
    ResourceAlreadyExistsException(ResourceAlreadyExistsException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_support_case::CreateSupportCaseError`).
pub type CreateSupportCaseError = crate::scaffolded::Scaffolded<CreateSupportCaseErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<CreateSupportCaseErrorModeled> for CreateSupportCaseError {
    fn from(modeled: CreateSupportCaseErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateSupportCase` operation.
pub struct CreateSupportCase<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateSupportCase<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateSupportCase<Eng>
{
    type Input = CreateSupportCaseInput;
    type Output = CreateSupportCaseOutput;
    type Error = CreateSupportCaseError;

    const NAME: &'static str = "CreateSupportCase";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_SUPPORT_CASE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_SUPPORT_CASE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateSupportCaseErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let mut quotas_state = state.lock();
        // The decisions the clock already owes are materialised first: a request whose
        // decision fell due before this call is no longer awaiting one, so no case is
        // opened for it.
        quotas_state.settle_due_requests(now);

        match quotas_state.open_support_case(&region, &input.request_id, now) {
            Ok(_) => Ok(CreateSupportCaseOutput {}),
            Err(crate::state::SupportCaseRefusal::NoSuchRequest) => Err(
                CreateSupportCaseErrorModeled::NoSuchResourceException(NoSuchResourceException {
                    message: Some(
                        crate::service::refusals::NO_SUCH_QUOTA_REQUEST_MESSAGE.to_string(),
                    ),
                })
                .into(),
            ),
            Err(crate::state::SupportCaseRefusal::CaseAlreadyOpen { case_id }) => Err(
                CreateSupportCaseErrorModeled::ResourceAlreadyExistsException(
                    ResourceAlreadyExistsException {
                        message: Some(format!(
                            "request {} already has the support case {case_id}",
                            input.request_id
                        )),
                    },
                )
                .into(),
            ),
            Err(crate::state::SupportCaseRefusal::NotPending { status }) => Err(
                CreateSupportCaseErrorModeled::InvalidResourceStateException(
                    InvalidResourceStateException {
                        message: Some(format!(
                            "request {} is {status}, so it is not awaiting a decision a support \
                             case could process",
                            input.request_id
                        )),
                    },
                )
                .into(),
            ),
        }
    }
}
