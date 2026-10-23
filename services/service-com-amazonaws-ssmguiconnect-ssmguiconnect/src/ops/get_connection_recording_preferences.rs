// SPDX-License-Identifier: CC0-1.0

//! @op GetConnectionRecordingPreferences
//!
//! Reports the per-account/per-Region connection recording preferences singleton
//! held in [`crate::state`], as the last successful
//! `UpdateConnectionRecordingPreferences` wrote it.
//!
//! `@readonly`: this handler only reads the store — it never writes, so a Get leaves
//! the next caller seeing exactly what it saw. With nothing configured there is no
//! resource to report, which is `ResourceNotFoundException` (HTTP 404), never an
//! empty 200.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this
//! operation because its input is `smithy.api#Unit` (recorded in
//! `src/ops/SKIPPED_STUBS.md`), so it had no handler file and no entry in the
//! service's `operations(...)` allow-list and was answered by the framework's
//! unroutable-operation path. `build.rs` derives that allow-list from the
//! `//! @op <Name>` header of every `src/ops/*.rs`, so this file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read it through the
//! guard it hands back; never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetConnectionRecordingPreferences`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json")
//! ```
//!
//! Returns the preferences specified for recording RDP connections in the requesting
//! Amazon Web Services account and Amazon Web Services Region.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
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

use crate::state::{audit, AuditOutcome};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetConnectionRecordingPreferencesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.ssmguiconnect#GetConnectionRecordingPreferences")]
#[derive(Debug)]
pub enum GetConnectionRecordingPreferencesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_connection_recording_preferences::GetConnectionRecordingPreferencesError`).
pub type GetConnectionRecordingPreferencesError = crate::scaffolded::Scaffolded<GetConnectionRecordingPreferencesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetConnectionRecordingPreferencesErrorModeled> for GetConnectionRecordingPreferencesError {
    fn from(modeled: GetConnectionRecordingPreferencesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetConnectionRecordingPreferences` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetConnectionRecordingPreferences<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetConnectionRecordingPreferences<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetConnectionRecordingPreferences<Eng>
{
    type Input = GetConnectionRecordingPreferencesInput;
    type Output = GetConnectionRecordingPreferencesOutput;
    type Error = GetConnectionRecordingPreferencesError;

    const NAME: &'static str = "GetConnectionRecordingPreferences";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CONNECTION_RECORDING_PREFERENCES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_CONNECTION_RECORDING_PREFERENCES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it. Every
        // branch below records its outcome through `crate::state::audit`.
        let _ = &input;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            audit(Self::NAME, ctx, AuditOutcome::StoreUnavailable);
            return Err(GetConnectionRecordingPreferencesErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the connection recording preferences store is unavailable"
                        .to_string(),
                },
            )
            .into());
        };

        // Read-only: take the guard, copy what is stored, release it. No branch of
        // this handler writes.
        let stored = state.lock().preferences().cloned();

        let Some(record) = stored else {
            audit(Self::NAME, ctx, AuditOutcome::NotFound);
            return Err(GetConnectionRecordingPreferencesErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::resource_not_found_message(
                        crate::state::CONNECTION_RECORDING_PREFERENCES,
                    ),
                },
            )
            .into());
        };

        audit(Self::NAME, ctx, AuditOutcome::PreferencesRead);
        Ok(GetConnectionRecordingPreferencesOutput {
            client_token: Some(record.client_token.clone()),
            connection_recording_preferences: Some(record.to_preferences()),
        })
    }
}
