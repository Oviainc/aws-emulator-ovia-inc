// SPDX-License-Identifier: CC0-1.0

//! @op UpdateConnectionRecordingPreferences
//!
//! Creates-or-replaces the per-account/per-Region connection recording preferences
//! singleton held in [`crate::state`], and reports back what was stored.
//!
//! `@idempotent` with an `@idempotencyToken` `ClientToken`, so a replay of the same
//! token returns the first outcome instead of writing a second time; a request that
//! omits the token gets a service-provided one
//! ([`crate::state::mint_client_token`]).
//!
//! GATED on the account: configuring RDP connection recording requires the account to
//! be enrolled in just-in-time node access, so an account that is not enrolled is
//! refused with the modeled `AccessDeniedException` (403) — after the request-shape
//! validation AWS performs first, and before anything the request names is looked up.
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
//! # Modeled contract for `UpdateConnectionRecordingPreferences`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json")
//! ```
//!
//! Updates the preferences for recording RDP connections.
//!
//! ## Input
//!
//! * `ClientToken` (ClientToken, len 1..64, @idempotencyToken) — User-provided idempotency
//!   token.
//! * `ConnectionRecordingPreferences` (ConnectionRecordingPreferences) **required** — The set
//!   of preferences used for recording RDP connections in the requesting Amazon Web Services
//!   account and Amazon Web Services Region. This includes details such as which S3 bucket
//!   recordings are stored in.
//!
//! ## Output
//!
//! * `ClientToken` (ClientToken, len 1..64) — Service-provided idempotency token.
//! * `ConnectionRecordingPreferences` (ConnectionRecordingPreferences) — The set of
//!   preferences used for recording RDP connections in the requesting Amazon Web Services
//!   account and Amazon Web Services Region. This includes details such as which S3 bucket
//!   recordings are stored in.
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

use crate::state::{
    audit, AuditOutcome, IdempotentOperation, RecordingPrefsRecord, StoredResponse,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateConnectionRecordingPreferencesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.ssmguiconnect#UpdateConnectionRecordingPreferences")]
#[derive(Debug)]
pub enum UpdateConnectionRecordingPreferencesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_connection_recording_preferences::UpdateConnectionRecordingPreferencesError`).
pub type UpdateConnectionRecordingPreferencesError = crate::scaffolded::Scaffolded<UpdateConnectionRecordingPreferencesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<UpdateConnectionRecordingPreferencesErrorModeled> for UpdateConnectionRecordingPreferencesError {
    fn from(modeled: UpdateConnectionRecordingPreferencesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateConnectionRecordingPreferences` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct UpdateConnectionRecordingPreferences<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateConnectionRecordingPreferences<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateConnectionRecordingPreferences<Eng>
{
    type Input = UpdateConnectionRecordingPreferencesInput;
    type Output = UpdateConnectionRecordingPreferencesOutput;
    type Error = UpdateConnectionRecordingPreferencesError;

    const NAME: &'static str = "UpdateConnectionRecordingPreferences";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CONNECTION_RECORDING_PREFERENCES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CONNECTION_RECORDING_PREFERENCES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every branch below records its outcome through `crate::state::audit`, so a
        // call that replaces the singleton leaves an attributable trail.

        let Some(state) = crate::state::service_state(&*self.engine) else {
            audit(Self::NAME, ctx, AuditOutcome::StoreUnavailable);
            return Err(UpdateConnectionRecordingPreferencesErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the connection recording preferences store is unavailable"
                        .to_string(),
                },
            )
            .into());
        };

        // Modeled constraints the boundary validator cannot compile (see
        // `crate::state::refuse_preferences`); everything else it declares was
        // already refused before this handler ran.
        //
        // REQUEST-SHAPE VALIDATION COMES FIRST, ahead of the account-level gate
        // below: a request whose members are outside their modeled constraints is
        // refused as invalid whatever the account is enrolled in.
        if let Some(message) = crate::state::refuse_preferences(&input.connection_recording_preferences) {
            audit(Self::NAME, ctx, AuditOutcome::ValidationRefused);
            return Err(UpdateConnectionRecordingPreferencesErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }

        let mut store = state.lock();

        // THE ACCOUNT-LEVEL GATE: configuring RDP connection recording requires the
        // account to be enrolled in just-in-time node access, so an account that is
        // not enrolled is refused outright — before any lookup of the resources the
        // request names (a bucket, a key), and before the idempotency record, so the
        // denial is the same for a fresh token and for a replayed one. The enrollment
        // is persisted account state (`crate::state`), not a property of the request.
        if !store.just_in_time_node_access_enabled() {
            drop(store);
            audit(Self::NAME, ctx, AuditOutcome::AccessDenied);
            return Err(UpdateConnectionRecordingPreferencesErrorModeled::AccessDeniedException(
                AccessDeniedException {
                    message: crate::state::JUST_IN_TIME_NODE_ACCESS_DISABLED.to_string(),
                },
            )
            .into());
        }

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

        let key = (IdempotentOperation::Update, client_token.clone());

        // This operation is `@idempotent`: a replayed token answers with the record
        // the FIRST call persisted and writes nothing — but only while that record is
        // still the stored singleton, so a replay can never report a resource a later
        // Delete or Update has already changed.
        if let Some(StoredResponse::Updated(record)) = store.replayable_outcome(&key) {
            let replayed = UpdateConnectionRecordingPreferencesOutput {
                client_token: Some(record.client_token.clone()),
                connection_recording_preferences: Some(record.to_preferences()),
            };
            drop(store);
            audit(Self::NAME, ctx, AuditOutcome::IdempotentReplay);
            return Ok(replayed);
        }

        // Create-or-REPLACE: the singleton holds exactly what this request asked for,
        // never the union of this request and the last one.
        let record = RecordingPrefsRecord::from_request(
            &input.connection_recording_preferences,
            client_token,
        );
        store.put_preferences(record.clone());
        drop(store);
        audit(Self::NAME, ctx, AuditOutcome::PreferencesWritten);

        Ok(UpdateConnectionRecordingPreferencesOutput {
            client_token: Some(record.client_token.clone()),
            connection_recording_preferences: Some(record.to_preferences()),
        })
    }
}
