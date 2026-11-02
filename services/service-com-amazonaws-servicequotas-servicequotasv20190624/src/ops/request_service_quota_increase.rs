// SPDX-License-Identifier: CC0-1.0

//! @op RequestServiceQuotaIncrease
//!
//! Mints a quota increase request and stores it, so
//! `GetRequestedServiceQuotaChange` and
//! `ListRequestedServiceQuotaChangeHistoryByQuota` read back the same record.
//!
//! The minted request carries a derived `Id`, `Status = PENDING`, and `Created` /
//! `LastUpdated` timestamps. A request this emulator accepts then walks the workflow
//! its submitter asked for in `SupportCaseAllowed`, one clock-owed step at a time —
//! nothing sleeps, each step's due instant is stored and materialised by the next read
//! past it:
//!
//! * allowed (the model's default) — Service Quotas opens the support case that
//!   processes the request ([`crate::state::SUPPORT_CASE_OPEN_DELAY`] after the call):
//!   it moves to `CASE_OPENED` carrying that case's `CaseId`, which is the case a
//!   client follows up on. The review that case waits on then decides it
//!   ([`crate::state::REQUEST_DECISION_DELAY`] later): the increase is `APPROVED` and
//!   its `DesiredValue` is put in force as the quota's applied value, while the case
//!   id it was processed under is kept. The AWS DEFAULT value is untouched, which is
//!   why `GetAWSDefaultServiceQuota` keeps answering the same number;
//! * `SupportCaseAllowed = false` — the submitter opted out of the support case that
//!   reviews an increase, so AWS opens none and the request goes straight from
//!   `PENDING` to `NOT_APPROVED`: no `CaseId`, and the quota keeps the value it had.
//!
//! `CreateSupportCase` is how a case is opened on a request BEFORE Service Quotas
//! takes it up — including one whose submitter had opted out.
//!
//! A `ContextId` names the resource the increase is asked for, on a quota AWS
//! publishes as adjustable per resource: the request is recorded at the `RESOURCE`
//! level and, once approved, its value is in force for THAT resource alone, leaving
//! every other resource and the account-wide value where they were. A `ContextId` that
//! names no resource — neither an Amazon Resource Name nor the every-resource `*` — is
//! not refused: it is the one failure the model gives a request `Status` of its own, so
//! the request is stored like any other and the step that would take it up settles it
//! to `INVALID_REQUEST`, with no support case and nothing put in force.
//!
//! Refusals:
//!
//! * an unknown `(ServiceCode, QuotaCode)` pair -> `NoSuchResourceException` (404);
//! * a quota AWS publishes as not adjustable -> `IllegalArgumentException` (400);
//! * a `DesiredValue` that is not above the value in force -> `IllegalArgumentException`
//!   (400): this operation requests an INCREASE;
//! * a `ContextId` for a quota that is adjustable at the account level only ->
//!   `IllegalArgumentException` (400), and the other half of the same rule: NO
//!   `ContextId` for a quota that is adjustable for an individual resource only ->
//!   `IllegalArgumentException` (400);
//! * a second open request for the same quota AND the same context ->
//!   `ResourceAlreadyExistsException` (400): one quota plane has at most one request
//!   awaiting a decision, whether it is `PENDING` or has a support case processing it.
//!   The plane is as wide as the value an approved increase puts in force: the whole
//!   ACCOUNT for a quota the catalog counts account-wide (`GlobalQuota`), so a request
//!   made at ANY regional endpoint blocks it, and the caller's Region for any other
//!   quota;
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
        quotas_state.settle_due_requests(now);

        let Some(default) = quotas_state.default_quota(&input.service_code, &input.quota_code)
        else {
            // Which half of the pair the catalog does not hold decides what the refusal
            // states, through the crate's one rule for it.
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(quotas_state.unresolved_quota_message(
                            &input.service_code,
                            &input.quota_code,
                        )),
                    },
                )
                .into(),
            );
        };
        let adjustable = default.adjustable;
        let resource_adjustable = default.resource_adjustable();

        match input.context_id.as_deref() {
            Some(context_id) if !resource_adjustable => {
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
            // The other half of the same rule, asked through the one
            // [`crate::state::DefaultQuota::has_plane`] predicate the read path asks:
            // a quota with no account-wide plane can only be increased for one
            // resource, so a request that names none has no plane to land on. It holds
            // for any row the catalog publishes as `PER_RESOURCE`; today every row it
            // publishes is either account-only or adjustable at both levels, so this
            // arm guards the rule rather than a seeded quota, and keeps the refusal
            // from depending on which rows exist.
            None if !default.has_plane(None) => {
                return Err(
                    RequestServiceQuotaIncreaseErrorModeled::IllegalArgumentException(
                        IllegalArgumentException {
                            message: Some(crate::state::no_account_plane_message(
                                &input.quota_code,
                            )),
                        },
                    )
                    .into(),
                );
            }
            _ => {}
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
            .applied_value(
                &region,
                &input.service_code,
                &input.quota_code,
                input.context_id.as_deref(),
            )
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

        // At most ONE request is awaiting a decision on a quota PLANE, and a plane is
        // as wide as the value an approved increase puts in force: the whole account
        // for a quota the catalog counts account-wide, the caller's region otherwise,
        // and one resource context within either — so an open increase for one
        // resource does not block another resource's.
        let open = quotas_state.open_request_on_plane(
            &region,
            &input.service_code,
            &input.quota_code,
            input.context_id.as_deref(),
        );
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
            input.context_id.clone(),
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
