// SPDX-License-Identifier: CC0-1.0

//! @op DeleteConnectionRecordingPreferences
//!
//! Removes the per-account/per-Region connection recording preferences singleton held
//! in [`crate::state`], so the next read reports `ResourceNotFoundException`.
//!
//! `@idempotent` with an `@idempotencyToken` `ClientToken`, so a replay of the same
//! token returns the first outcome instead of refusing a second time; a request that
//! omits the token gets a service-provided one
//! ([`crate::state::mint_client_token`]).
//!
//! Idempotent in the full sense: "nothing configured" is the state this operation is
//! asked for, so a Delete on an account with no preferences SUCCEEDS (200, echoing the
//! effective `ClientToken`) rather than reporting a resource it did not find. It is
//! also not gated by just-in-time node access — unlike
//! `UpdateConnectionRecordingPreferences`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read and write it
//! through the guard it hands back; never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteConnectionRecordingPreferences`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json")
//! ```
//!
//! Deletes the preferences for recording RDP connections.
//!
//! ## Input
//!
//! * `ClientToken` (ClientToken, len 1..64, @idempotencyToken) — User-provided idempotency
//!   token.
//!
//! ## Output
//!
//! * `ClientToken` (ClientToken, len 1..64) — Service-provided idempotency token.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `ConflictException` (HTTP 409) — An error occurred due to a conflict.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//! * `ResourceNotFoundException` (HTTP 404) — The resource could not be found.
//! * `ServiceQuotaExceededException` (HTTP 402) — Your request exceeds a service quota.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an AWS service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{audit, AuditOutcome, IdempotentOperation, StoredResponse};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteConnectionRecordingPreferencesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.ssmguiconnect#DeleteConnectionRecordingPreferences")]
#[derive(Debug)]
pub enum DeleteConnectionRecordingPreferencesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_connection_recording_preferences::DeleteConnectionRecordingPreferencesError`).
pub type DeleteConnectionRecordingPreferencesError = crate::scaffolded::Scaffolded<DeleteConnectionRecordingPreferencesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<DeleteConnectionRecordingPreferencesErrorModeled> for DeleteConnectionRecordingPreferencesError {
    fn from(modeled: DeleteConnectionRecordingPreferencesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteConnectionRecordingPreferences` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DeleteConnectionRecordingPreferences<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteConnectionRecordingPreferences<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteConnectionRecordingPreferences<Eng>
{
    type Input = DeleteConnectionRecordingPreferencesInput;
    type Output = DeleteConnectionRecordingPreferencesOutput;
    type Error = DeleteConnectionRecordingPreferencesError;

    const NAME: &'static str = "DeleteConnectionRecordingPreferences";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CONNECTION_RECORDING_PREFERENCES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CONNECTION_RECORDING_PREFERENCES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every branch below records its outcome through `crate::state::audit`, so a
        // call that removes the singleton leaves an attributable trail.

        let Some(state) = crate::state::service_state(&*self.engine) else {
            audit(Self::NAME, ctx, AuditOutcome::StoreUnavailable);
            return Err(DeleteConnectionRecordingPreferencesErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the connection recording preferences store is unavailable"
                        .to_string(),
                },
            )
            .into());
        };

        // `ClientToken` is `@idempotencyToken`: the service provides one when the
        // caller does not, and the response reports whichever value was used. Its
        // modeled `@length(1, 64)` was already enforced by the framework's boundary
        // walk before this handler ran, so the value here is inside that bound.
        //
        // BOUNDED STATE: the outcome recorded under this token lives in a store capped
        // at `crate::state::MAX_IDEMPOTENCY_OUTCOMES` with FIFO eviction (see
        // `ComAmazonawsSsmguiconnectSsmguiconnectState::record_outcome`), so a caller
        // sending a fresh token per request cannot grow process-lifetime state. The cap
        // is applied by the state API this handler writes through, not here, because
        // both mutating operations must share one policy.
        let client_token = input
            .client_token
            .unwrap_or_else(crate::state::mint_client_token);

        let mut store = state.lock();
        let key = (IdempotentOperation::Delete, client_token.clone());

        // This operation is `@idempotent`: a replayed token answers with the outcome
        // of the FIRST call — which succeeded — rather than reporting the singleton
        // it already removed as missing. The replay holds only while the singleton is
        // still absent, so a 200 from here always means "there are no preferences
        // now": once a later Update reconfigures the resource, the token describes a
        // state the store no longer holds and the call runs for real.
        if matches!(
            store.replayable_outcome(&key),
            Some(StoredResponse::Deleted)
        ) {
            drop(store);
            audit(Self::NAME, ctx, AuditOutcome::IdempotentReplay);
            return Ok(DeleteConnectionRecordingPreferencesOutput {
                client_token: Some(client_token),
            });
        }

        // Deleting with nothing configured removes nothing and still succeeds: the
        // caller asked for a state the store is already in. The flag distinguishes the
        // two for the audit record only — both answer 200 with the effective token.
        let removed = store.remove_preferences(&client_token);
        drop(store);
        audit(
            Self::NAME,
            ctx,
            if removed {
                AuditOutcome::PreferencesDeleted
            } else {
                AuditOutcome::PreferencesAlreadyAbsent
            },
        );

        Ok(DeleteConnectionRecordingPreferencesOutput {
            client_token: Some(client_token),
        })
    }
}
