// SPDX-License-Identifier: CC0-1.0

//! @op UpdateIntegration
//!
//! Syncs ONE workload with an integrated service, recording the sync in
//! [`crate::state`]: every question of the workload's lens reviews the integration covers
//! is recorded as synced at the instant this request is served, which is what every read
//! of an answer reports as `JiraConfiguration$LastSyncedTime` (`GetAnswer`, `ListAnswers`
//! and `ListLensReviewImprovements`, through the one projection they share,
//! [`crate::state::WorkloadRecord::question_jira_configuration`]).
//!
//! Which questions a sync covers follows the workload's own Jira configuration: under the
//! manual issue management type it is exactly the questions that review's selection names
//! (`LensReview$JiraConfiguration` -> `SelectedPillar$SelectedQuestionIds`, recorded by
//! `UpdateLensReview`), and otherwise every question the lens asks. That rule lives with
//! the state the sync writes
//! ([`crate::state::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::sync_workload_integration`]).
//!
//! A workload this store does not hold is a 404, worded in the one place every operation
//! that resolves a `WorkloadId` reads it from, and the `@idempotencyToken` is honoured:
//! the same token with the same parameters replays the original sync's empty success
//! without syncing again, and with DIFFERENT parameters is the modelled conflict.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request, so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back;
//! never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateIntegration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Update integration features.
//!
//! ## Input
//!
//! * `ClientRequestToken` (ClientRequestToken, len 1..2048, pattern ^[\x00-\x7F]*$,
//!   @idempotencyToken) **required** — A unique case-sensitive string used to ensure that
//!   this request is idempotent (executes only once). You should not reuse the same token for
//!   other requests. If you retry a request with the same client request token and the same
//!   parameters after the original request has completed successfully, the result of the
//!   original request is returned. This token is listed as required, however, if you do not
//!   specify it, the Amazon Web Services SDKs automatically generate one for you. If you are
//!   not using the Amazon Web Services SDK or the CLI, you must provide this token or the
//!   request will fail.
//! * `IntegratingService` (IntegratingService, one of: JIRA) **required** — Which integrated
//!   service to update.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) **required** — The ID
//!   assigned to the workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — The resource has already been processed, was deleted,
//!   or is too large.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateIntegrationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

#[derive(Debug, Clone, Default)]
pub struct UpdateIntegrationOutput;

impl UpdateIntegrationOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/workloads/{WorkloadId}/updateIntegration",
        Some(200u16),
    ));
}

pub static UPDATE_INTEGRATION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UpdateIntegrationOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UpdateIntegrationOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateIntegrationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#UpdateIntegration")]
#[derive(Debug)]
pub enum UpdateIntegrationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_integration::UpdateIntegrationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateIntegrationError = crate::scaffolded::Scaffolded<UpdateIntegrationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateIntegrationErrorModeled> for UpdateIntegrationError {
    fn from(modeled: UpdateIntegrationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateIntegration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateIntegration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateIntegration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateIntegration<Eng>
{
    type Input = UpdateIntegrationInput;
    type Output = UpdateIntegrationOutput;
    type Error = UpdateIntegrationError;

    const NAME: &'static str = "UpdateIntegration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_INTEGRATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UPDATE_INTEGRATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Nothing here is minted (the workload is named by the id the request carries),
        // so the only request metadata read is the Region whose store serves it.
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            UpdateIntegrationErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool workload store is unavailable".to_string(),
            })
        })?;

        // The instant this request is served at: what the sync it records is reported as
        // having happened at, read once so every question it covers carries one reading of
        // the clock.
        let now = std::time::SystemTime::now();
        let fingerprint = crate::state::parameter_fingerprint(&::serde_json::json!({
            "IntegratingService": &input.integrating_service,
            "WorkloadId": &input.workload_id,
        }));

        let mut store = state.lock();

        // `@idempotencyToken` replay: "if you retry a request with the same client request
        // token and the same parameters after the original request has completed
        // successfully, the result of the original request is returned"
        // (`UpdateIntegrationInput$ClientRequestToken`). The result here is an EMPTY body,
        // so the replay's answer is that success, and it syncs nothing a second time, which
        // is what keeps the instant the original sync recorded, and already reported, where
        // it was.
        //
        // Resolved BEFORE the workload, deliberately: a retry is answered from the ledger
        // whatever has happened since, and the ledger holds only tokens whose workload is
        // still stored, because `remove_workload` drops the ones that resolved to a
        // workload a delete removed.
        if let Some(recorded) = store
            .update_integration_tokens
            .get(&input.client_request_token)
        {
            if recorded.fingerprint != fingerprint {
                let workload_id = recorded.workload_id.clone();
                return Err(UpdateIntegrationErrorModeled::ConflictException(
                    crate::state::idempotency_token_conflict(
                        crate::state::WORKLOAD_RESOURCE_TYPE,
                        workload_id,
                        &input.client_request_token,
                    ),
                )
                .into());
            }
            return Ok(UpdateIntegrationOutput);
        }

        // What is synced is the WORKLOAD's integration, so the workload has to exist to
        // have one: the same 404 every operation that resolves a `WorkloadId` answers,
        // worded in the one place they all read it from.
        if !store.workloads.contains_key(&input.workload_id) {
            return Err(UpdateIntegrationErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
            .into());
        }

        // The integration to sync is the one the request names, and `IntegratingService`
        // publishes exactly `JIRA` (`smithy_query(query="shape",
        // shape="IntegratingService")`, enforced before this handler runs), so the sync a
        // request can ask for is of the Jira configuration this workload and its lens
        // reviews carry. Which questions it covers is decided where that state lives; how
        // MANY it covered is not this handler's to report, because the operation answers an
        // empty body.
        let _covered = store.sync_workload_integration(&input.workload_id, now);

        // Written through the ledger's bounded `remember`, which holds the remembered
        // tokens at `MAX_REMEMBERED_REQUEST_TOKENS` by forgetting the oldest: the token is
        // caller-chosen, so the ledger cannot be allowed to grow with the stream of them.
        store.update_integration_tokens.remember(
            input.client_request_token,
            crate::state::UpdateIntegrationToken {
                workload_id: input.workload_id,
                fingerprint,
            },
        );

        Ok(UpdateIntegrationOutput)
    }
}
