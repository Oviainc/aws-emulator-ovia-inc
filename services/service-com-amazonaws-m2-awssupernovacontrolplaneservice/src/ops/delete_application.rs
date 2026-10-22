// SPDX-License-Identifier: CC0-1.0

//! @op DeleteApplication
//!
//! Removes a persisted application, every version it recorded, its name reservation and
//! its tags, so a later `GetApplication` answers 404 and `ListApplications` no longer
//! lists it.
//!
//! The model declares no `ResourceNotFoundException` for this operation and marks it
//! `@idempotent`, so deleting an application that is already gone is a success that
//! removes nothing — a retried delete answers the same way as the first.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DeleteApplication`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json")
//! ```
//!
//! Deletes a specific application. You cannot delete a running application.
//!
//! ## Input
//!
//! * `applicationId` (Identifier, pattern ^\S{1,80}$) **required** — The unique identifier of
//!   the application you want to delete.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The account or role doesn't have the right
//!   permissions to make the request.
//! * `ConflictException` (HTTP 409) — The parameters provided in the request conflict with
//!   existing resources.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of the request.
//! * `ThrottlingException` (HTTP 429) — The number of requests made exceeds the limit.
//! * `ValidationException` (HTTP 400) — One or more parameters provided in the request is not
//!   valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.m2#DeleteApplication")]
#[derive(Debug)]
pub enum DeleteApplicationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_application::DeleteApplicationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteApplicationError = crate::scaffolded::Scaffolded<DeleteApplicationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteApplicationErrorModeled> for DeleteApplicationError {
    fn from(modeled: DeleteApplicationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteApplication` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteApplication<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteApplication<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteApplication<Eng>
{
    type Input = DeleteApplicationInput;
    type Output = DeleteApplicationOutput;
    type Error = DeleteApplicationError;

    const NAME: &'static str = "DeleteApplication";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_APPLICATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteApplicationErrorModeled::InternalServerException(InternalServerException {
                message: "the m2 application store is unavailable".to_string(),
                retry_after_seconds: 0,
            })
        })?;

        let mut store = state.lock();
        if let Some(removed) = store.applications.remove(&input.application_id) {
            // The name is free again, the tags go with the resource, and a client token
            // that minted this application no longer replays a response naming it.
            store.app_names.remove(&removed.name);
            store.tags_by_arn.remove(&removed.application_arn);
            store
                .client_tokens
                .retain(|_, minted| *minted != removed.application_id);
        }

        Ok(DeleteApplicationOutput {})
    }
}
