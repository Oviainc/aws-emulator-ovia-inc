// SPDX-License-Identifier: CC0-1.0

//! @op RequestServiceQuotaIncrease
//!
//! Mints a quota increase request and stores it, so
//! `GetRequestedServiceQuotaChange` and
//! `ListRequestedServiceQuotaChangeHistoryByQuota` read back the same record.
//!
//! The minted request carries a derived `Id`, `Status = PENDING`, and `Created` /
//! `LastUpdated` timestamps. A request this emulator accepts is decided
//! ([`crate::state::REQUEST_DECISION_DELAY`] after the call) by what its submitter
//! asked for in `SupportCaseAllowed`:
//!
//! * allowed (the model's default) — the request is `APPROVED` and its `DesiredValue`
//!   is put in force as the quota's applied value; the AWS DEFAULT value is untouched,
//!   which is why `GetAWSDefaultServiceQuota` keeps answering the same number;
//! * `SupportCaseAllowed = false` — the submitter opted out of the support case that
//!   reviews an increase, so AWS opens none and the request ends `NOT_APPROVED`: no
//!   `CaseId`, and the quota keeps the value it had.
//!
//! `CaseId` stays absent either way: this emulator opens no support case.
//!
//! Refusals:
//!
//! * an unknown `(ServiceCode, QuotaCode)` pair -> `NoSuchResourceException` (404);
//! * a quota AWS publishes as not adjustable -> `IllegalArgumentException` (400);
//! * a `DesiredValue` that is not above the value in force -> `IllegalArgumentException`
//!   (400): this operation requests an INCREASE;
//! * a `ContextId` for a quota that is adjustable at the account level only ->
//!   `IllegalArgumentException` (400);
//! * a second open request for the same quota -> `ResourceAlreadyExistsException`
//!   (400): one quota has at most one request awaiting a decision;
//! * a region already holding [`crate::state::MAX_REQUESTS_PER_REGION`] stored
//!   requests -> `QuotaExceededException` (409): the request store is append-only, so
//!   it has a ceiling.
//!
//! ---
//!
//! # Modeled contract for `RequestServiceQuotaIncrease`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! ## Input
//!
//! * `ServiceCode` (required, len 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$)
//! * `QuotaCode` (required, len 1..128, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$)
//! * `DesiredValue` (required, QuotaValue, range 0..10000000000)
//! * `ContextId` (QuotaContextId), `SupportCaseAllowed` (boolean)
//!
//! ## Output
//!
//! * `RequestedQuota` (RequestedServiceQuotaChange)
//!
//! ## Errors
//!
//! * `AccessDeniedException` (403), `DependencyAccessDeniedException` (403),
//!   `IllegalArgumentException` (400), `InvalidResourceStateException` (405),
//!   `NoSuchResourceException` (404), `QuotaExceededException` (409),
//!   `ResourceAlreadyExistsException` (400), `ServiceException` (500),
//!   `TooManyRequestsException` (429).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#RequestServiceQuotaIncrease")]
#[derive(Debug)]
pub enum RequestServiceQuotaIncreaseErrorModeled {
    AccessDeniedException(AccessDeniedException),
    DependencyAccessDeniedException(DependencyAccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    InvalidResourceStateException(InvalidResourceStateException),
    NoSuchResourceException(NoSuchResourceException),
    QuotaExceededException(QuotaExceededException),
    ResourceAlreadyExistsException(ResourceAlreadyExistsException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type RequestServiceQuotaIncreaseError =
    crate::scaffolded::Scaffolded<RequestServiceQuotaIncreaseErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<RequestServiceQuotaIncreaseErrorModeled> for RequestServiceQuotaIncreaseError {
    fn from(modeled: RequestServiceQuotaIncreaseErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RequestServiceQuotaIncrease` operation.
pub struct RequestServiceQuotaIncrease<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RequestServiceQuotaIncrease<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RequestServiceQuotaIncrease<Eng>
{
    type Input = RequestServiceQuotaIncreaseInput;
    type Output = RequestServiceQuotaIncreaseOutput;
    type Error = RequestServiceQuotaIncreaseError;

    const NAME: &'static str = "RequestServiceQuotaIncrease";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REQUEST_SERVICE_QUOTA_INCREASE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REQUEST_SERVICE_QUOTA_INCREASE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(&ctx.parts().headers);
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            RequestServiceQuotaIncreaseErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let mut quotas_state = state.lock();
        // Decide every request already due before comparing against the value in
        // force: an increase is measured against the current value, not the default.
        quotas_state.settle_due_requests(&region, now);

        let Some(default) = quotas_state.default_quota(&input.service_code, &input.quota_code)
        else {
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(format!(
                            "no quota with ServiceCode '{}' and QuotaCode '{}' exists",
                            input.service_code, input.quota_code
                        )),
                    },
                )
                .into(),
            );
        };
        let adjustable = default.adjustable;
        let account_level_only = default.adjustable_at_level == crate::state::LEVEL_ACCOUNT;

        if let Some(context_id) = input.context_id.as_deref() {
            if account_level_only {
                return Err(
                    RequestServiceQuotaIncreaseErrorModeled::IllegalArgumentException(
                        IllegalArgumentException {
                            message: Some(format!(
                                "ContextId '{context_id}' was provided for QuotaCode '{}', which \
                                 is adjustable at the ACCOUNT level only",
                                input.quota_code
                            )),
                        },
                    )
                    .into(),
                );
            }
        }

        if !adjustable {
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(format!(
                            "QuotaCode '{}' of ServiceCode '{}' is not adjustable",
                            input.quota_code, input.service_code
                        )),
                    },
                )
                .into(),
            );
        }

        let in_force = quotas_state
            .applied_value(&region, &input.service_code, &input.quota_code)
            .unwrap_or(default.value);
        if input.desired_value <= in_force {
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(format!(
                            "DesiredValue {} is not an increase on the current value {in_force} \
                             of QuotaCode '{}'",
                            input.desired_value, input.quota_code
                        )),
                    },
                )
                .into(),
            );
        }

        let open = quotas_state
            .requests_for_quota(&region, &input.service_code, &input.quota_code)
            .into_iter()
            .find(|request| request.status == crate::state::STATUS_PENDING);
        if let Some(open) = open {
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::ResourceAlreadyExistsException(
                    ResourceAlreadyExistsException {
                        message: Some(format!(
                            "request {} for QuotaCode '{}' is still awaiting a decision",
                            open.id, input.quota_code
                        )),
                    },
                )
                .into(),
            );
        }

        let request = quotas_state.record_request(
            &region,
            &input.service_code,
            &input.quota_code,
            input.desired_value,
            // Absent means allowed: the model documents "By default, this flag is set
            // to True".
            input
                .support_case_allowed
                .unwrap_or(crate::state::SUPPORT_CASE_ALLOWED_BY_DEFAULT),
            crate::state::requester_identity(),
            now,
        );
        let Some(request) = request else {
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::QuotaExceededException(
                    QuotaExceededException {
                        message: Some(format!(
                            "this Region already holds the maximum of {} quota increase requests",
                            crate::state::MAX_REQUESTS_PER_REGION
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(RequestServiceQuotaIncreaseOutput {
            requested_quota: Some(quotas_state.request_view(&request)),
        })
    }
}
