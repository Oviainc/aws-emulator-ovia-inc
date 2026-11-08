// SPDX-License-Identifier: CC0-1.0

//! @op StartAccessRequest
//! Raises a just-in-time node access request for the nodes it targets and persists it in
//! [`crate::state::just_in_time_node_access`], where `GetAccessToken` reads it back.
//!
//! The decisions, in the order AWS takes them:
//!
//! 1. the CALLER's own identity-based IAM policies, which decide the call before anything is
//!    resolved ([`crate::state::identity_authorization`]), so a caller holding no grant is
//!    told it is not authorized rather than told what the account holds;
//! 2. whether the account has just-in-time node access in this Region at all — it has it
//!    once it holds an access-request approval policy — and an account that holds none is
//!    told the service is not enabled, before any target is looked at;
//! 3. the emulator's own ceiling on how many requests this store holds.
//!
//! A raised request is `Pending`: an auto-approval policy approves it when the approval the
//! clock owes falls due, and a manual one leaves it to an approver, whose decision no
//! operation of this API records.
//!
//! ---
//!
//! # Modeled contract for `StartAccessRequest`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Starts the workflow for just-in-time node access sessions.
//!
//! ## Input
//!
//! * `Reason` (String1to256, len 1..256) **required** — A brief description explaining why
//!   you are requesting access to the node.
//! * `Tags` (TagList, len 0..1000) — Key-value pairs of metadata you want to assign to the
//!   access request.
//! * `Targets` (Targets, len 0..5) **required** — The node you are requesting access to.
//!
//! ## Output
//!
//! * `AccessRequestId` (AccessRequestId, pattern ^(oi)-[0-9a-f]{12}$) — The ID of the access
//!   request.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — The requester doesn't have permissions to perform
//!   the requested operation.
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `ResourceNotFoundException` (HTTP 400) — The specified parameter to be shared could not
//!   be found.
//! * `ServiceQuotaExceededException` (HTTP 400) — The request exceeds the service quota.
//!   Service quotas, also referred to as limits, are the maximum number of service resources
//!   or operations for your Amazon Web Services account.
//! * `ThrottlingException` (HTTP 400) — The request or operation couldn't be performed
//!   because the service is throttling requests.
//! * `ValidationException` (HTTP 400) — The request isn't valid. Verify that you entered
//!   valid contents for the command and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartAccessRequestError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartAccessRequestError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#StartAccessRequest")]
#[derive(Debug)]
pub enum StartAccessRequestErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerError(InternalServerError),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_access_request::StartAccessRequestError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartAccessRequestError = crate::scaffolded::Scaffolded<StartAccessRequestErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartAccessRequestErrorModeled> for StartAccessRequestError {
    fn from(modeled: StartAccessRequestErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartAccessRequest` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartAccessRequest<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartAccessRequest<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartAccessRequest<Eng>
{
    type Input = StartAccessRequestInput;
    type Output = StartAccessRequestOutput;
    type Error = StartAccessRequestError;

    const NAME: &'static str = "StartAccessRequest";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_ACCESS_REQUEST_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_ACCESS_REQUEST_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::just_in_time_node_access as jitna;

        // `Reason`'s 1..256 length and `Targets`' 0..5 length are MODELLED constraints, so
        // the centralized walk has already refused anything outside them.
        let region = crate::state::request_region(ctx.parts());

        // The caller's own identity-based policies decide this call before anything is
        // resolved. A start names no single resource of this service — the nodes it targets
        // are another service's — so the decision is about the account's resources, spelled
        // with IAM's own wildcard.
        if let Some(message) = crate::state::identity_authorization::refusal(
            &*self.engine,
            ctx,
            Self::NAME,
            crate::state::identity_authorization::ANY_RESOURCE,
        ) {
            return Err(
                StartAccessRequestErrorModeled::AccessDeniedException(AccessDeniedException {
                    message,
                })
                .into(),
            );
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartAccessRequestErrorModeled::InternalServerError(InternalServerError {
                message: Some("the just-in-time node access store is unavailable".to_string()),
            })
        })?;

        let now = ::std::time::SystemTime::now();
        // Resolved before the store guard is taken: it reads the IAM and STS stores.
        let requester_arn =
            crate::state::identity_authorization::caller_arn(&*self.engine, ctx);
        let mut store = state.lock();

        // An account that holds no access-request approval policy has not turned the
        // workflow on, which is answered before any target is looked at.
        if !store.has_access_request_approval_policy(&region) {
            return Err(
                StartAccessRequestErrorModeled::ValidationException(ValidationException {
                    message: Some(jitna::NOT_ENABLED_MESSAGE.to_string()),
                    reason_code: None,
                })
                .into(),
            );
        }

        // The emulator's own bound on the store, named as the emulator's: the published
        // just-in-time node access quota is not restated here. A start past it writes
        // nothing — the check is ahead of the id mint, so a refused start does not advance
        // the mint counter either.
        if store.access_requests_at_capacity() {
            return Err(
                StartAccessRequestErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: jitna::access_request_ceiling_message(),
                        resource_id: None,
                        resource_type: None,
                        quota_code: jitna::ACCESS_REQUEST_QUOTA_CODE.to_string(),
                        service_code: crate::state::ARN_SERVICE.to_string(),
                    },
                )
                .into(),
            );
        }

        // An auto-approval policy approves what it covers without anybody acting, so the
        // approval is owed from here on; a manual policy leaves the decision to an approver,
        // and nothing in this API records one, so such a request is owed nothing.
        let approval_due = store
            .access_requests_are_auto_approved(&region)
            .then(|| now + jitna::APPROVAL_DELAY);
        let record = jitna::AccessRequestRecord {
            access_request_id: store.next_access_request_id(now),
            region: region.clone(),
            reason: input.reason.clone(),
            targets: input.targets.clone(),
            // Who is asking for access: the principal this start's credential speaks for,
            // read from the same place `sts:GetCallerIdentity` answers from.
            requester_arn,
            tags: crate::state::to_btree_map(input.tags.clone()),
            // A request nobody has decided yet is pending, whichever policy decides it.
            status: jitna::STATUS_PENDING.to_string(),
            approval_due,
            // Nothing is vended until an approved request is asked for a token.
            credential: None,
            created_at: now,
        };
        let access_request_id = record.access_request_id.clone();
        store.insert_access_request(record);

        Ok(StartAccessRequestOutput {
            access_request_id: Some(access_request_id),
        })
    }
}
