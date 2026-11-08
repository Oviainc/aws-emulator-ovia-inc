// SPDX-License-Identifier: CC0-1.0

//! @op GetAccessToken
//! Reports the status of a just-in-time node access request and, for an approved one, the
//! temporary security credentials its approved window is exchanged for — both held in
//! [`crate::state::just_in_time_node_access`], where `StartAccessRequest` raised the
//! request.
//!
//! Three decisions, in the order AWS takes them:
//!
//! 1. the CALLER's own identity-based IAM policies, which decide the call before anything is
//!    resolved ([`crate::state::identity_authorization`]), so a caller holding no grant is
//!    told it is not authorized rather than told whether the request exists;
//! 2. whether the account has just-in-time node access in this Region at all — it has it
//!    once it holds an access-request approval policy — and an account that holds none is
//!    told the service is not enabled, before the id is looked up;
//! 3. the access request the id names: one this Region never issued is refused, one its
//!    policy has not approved is reported in the status it is in and vends nothing, and an
//!    approved one is answered with the credentials it holds for its window.
//!
//! The only writes are the two the workflow owes: materialising the approval an
//! auto-approval policy's clock already made due, and minting the credentials this operation
//! exists to return, which are then held against the request so two reads report one set.
//!
//! ---
//!
//! # Modeled contract for `GetAccessToken`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Returns a credentials set to be used with just-in-time node access.
//!
//! ## Input
//!
//! * `AccessRequestId` (AccessRequestId, pattern ^(oi)-[0-9a-f]{12}$) **required** — The ID
//!   of a just-in-time node access request.
//!
//! ## Output
//!
//! * `AccessRequestStatus` (AccessRequestStatus, one of: Approved | Expired | Pending |
//!   Rejected | Revoked) — The status of the access request.
//! * `Credentials` (Credentials) — The temporary security credentials which can be used to
//!   start just-in-time node access sessions.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — The requester doesn't have permissions to perform
//!   the requested operation.
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `ResourceNotFoundException` (HTTP 400) — The specified parameter to be shared could not
//!   be found.
//! * `ThrottlingException` (HTTP 400) — The request or operation couldn't be performed
//!   because the service is throttling requests.
//! * `ValidationException` (HTTP 400) — The request isn't valid. Verify that you entered
//!   valid contents for the command and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetAccessTokenError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetAccessTokenError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#GetAccessToken")]
#[derive(Debug)]
pub enum GetAccessTokenErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerError(InternalServerError),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_access_token::GetAccessTokenError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetAccessTokenError = crate::scaffolded::Scaffolded<GetAccessTokenErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetAccessTokenErrorModeled> for GetAccessTokenError {
    fn from(modeled: GetAccessTokenErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccessToken` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetAccessToken<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccessToken<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccessToken<Eng>
{
    type Input = GetAccessTokenInput;
    type Output = GetAccessTokenOutput;
    type Error = GetAccessTokenError;

    const NAME: &'static str = "GetAccessToken";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACCESS_TOKEN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACCESS_TOKEN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::just_in_time_node_access as jitna;

        // `AccessRequestId`'s `^(oi)-[0-9a-f]{12}$` pattern is a MODELLED constraint, so the
        // centralized walk has already refused anything outside it: reaching here means the
        // id is well formed and names either a request this Region issued or none at all.
        let region = crate::state::request_region(ctx.parts());

        // The caller's own identity-based policies decide this call before anything is
        // resolved, about the ARN the access request the request NAMES would carry — so the
        // refusal does not depend on whether that request exists.
        let resource = jitna::access_request_arn(&region, &input.access_request_id);
        if let Some(message) =
            crate::state::identity_authorization::refusal(&*self.engine, ctx, Self::NAME, &resource)
        {
            return Err(
                GetAccessTokenErrorModeled::AccessDeniedException(AccessDeniedException {
                    message,
                })
                .into(),
            );
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetAccessTokenErrorModeled::InternalServerError(InternalServerError {
                message: Some("the just-in-time node access store is unavailable".to_string()),
            })
        })?;
        let not_valid = |message: &str| {
            GetAccessTokenError::from(GetAccessTokenErrorModeled::ValidationException(
                ValidationException {
                    message: Some(message.to_string()),
                    reason_code: None,
                },
            ))
        };

        let now = ::std::time::SystemTime::now();
        let (status, live_credential) = {
            let mut store = state.lock();
            // An account that holds no access-request approval policy has not turned the
            // workflow on, which is answered before the id is looked up.
            if !store.has_access_request_approval_policy(&region) {
                return Err(not_valid(jitna::NOT_ENABLED_MESSAGE));
            }
            let Some(record) = store.access_request_mut(&region, &input.access_request_id) else {
                return Err(not_valid(jitna::ACCESS_REQUEST_NOT_FOUND_MESSAGE));
            };
            // The approval an auto-approval policy owes this request fell due at its stored
            // instant, whether or not anybody called: materialising it is what the clock
            // owes, not something this read decided.
            record.materialize_due(now);
            (
                record.status.clone(),
                record.live_credential(now).cloned(),
            )
        };

        // A request its policy has not approved is reported in the status it is in and vends
        // nothing: the credentials exist for an APPROVED window.
        if status != jitna::STATUS_APPROVED {
            return Ok(GetAccessTokenOutput {
                credentials: None,
                access_request_status: Some(status),
            });
        }
        // An approved request holds its credentials for as long as they are valid, so a
        // second read reports the set the first one vended rather than a fresh one.
        if let Some(held) = live_credential {
            return Ok(GetAccessTokenOutput {
                credentials: Some(held.to_credentials()),
                access_request_status: Some(status),
            });
        }

        // The credentials this read vends: created in the emulator's credential store, so the
        // emulator accepts requests signed with them and neither the id nor the secrets are
        // chosen here. Minted with no store guard held, then held against the access request.
        let minted = ctx
            .credential_store()
            .create(&::emulator_server::aws_credentials::AWS_SESSION);
        let (Some(secret_access_key), Some(session_token)) = (
            minted
                .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
                .map(str::to_string),
            minted
                .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
                .map(str::to_string),
        ) else {
            let _ = ctx.credential_store().delete(&minted.id);
            return Err(GetAccessTokenErrorModeled::InternalServerError(
                InternalServerError {
                    message: Some(
                        "the credentials minted for this access request carry no secrets"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let vended = jitna::VendedCredential {
            access_key_id: minted.id.clone(),
            secret_access_key,
            session_token,
            expires_at: now + jitna::CREDENTIAL_LIFETIME,
        };

        let reported = {
            let mut store = state.lock();
            let Some(record) = store.access_request_mut(&region, &input.access_request_id) else {
                drop(store);
                let _ = ctx.credential_store().delete(&minted.id);
                return Err(not_valid(jitna::ACCESS_REQUEST_NOT_FOUND_MESSAGE));
            };
            match record.live_credential(now) {
                // Another read of this request vended first: its set is the one this request
                // holds, so the set minted here is forgotten rather than left behind.
                Some(held) => {
                    let held = held.to_credentials();
                    drop(store);
                    let _ = ctx.credential_store().delete(&minted.id);
                    held
                }
                None => {
                    record.credential = Some(vended.clone());
                    vended.to_credentials()
                }
            }
        };
        Ok(GetAccessTokenOutput {
            credentials: Some(reported),
            access_request_status: Some(status),
        })
    }
}
