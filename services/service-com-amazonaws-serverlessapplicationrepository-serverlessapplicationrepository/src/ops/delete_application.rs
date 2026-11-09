// SPDX-License-Identifier: CC0-1.0

//! @op DeleteApplication
//! Removes one application, and with it every version it holds, from
//! [`crate::state`] — so the next GetApplication is a 404 and the next
//! ListApplications no longer carries it.
//!
//! This is the ONLY file to touch for this operation.
//!
//! The engine is held on `self` (constructor-injected); reach the store through
//! [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
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
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Deletes the specified application.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — One of the parameters in the request is invalid.
//! * `ConflictException` (HTTP 409) — The resource already exists.
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `NotFoundException` (HTTP 404) — The resource (for example, an access policy statement)
//!   specified in the request doesn't exist.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteApplicationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteApplicationOutput;

impl DeleteApplicationOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/applications/{ApplicationId}",
        Some(204u16),
    ));
}

pub static DELETE_APPLICATION_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteApplicationOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteApplicationOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteApplicationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.serverlessapplicationrepository#DeleteApplication")]
#[derive(Debug)]
pub enum DeleteApplicationErrorModeled {
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
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
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_APPLICATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 204u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides a signed `serverlessrepo` call against the CALLER's identity-based
        // policies BEFORE the operation writes anything, per the application the request
        // addresses, so a refused delete removes nothing; see
        // [`crate::state::authorization`].
        crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.application_id,
        )?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteApplicationErrorModeled::InternalServerErrorException(
                crate::state::internal_server_error("the service state is unavailable"),
            )
            .into());
        };

        let mut store = state.lock();
        // Deleting what was never created is an absent resource, not a no-op success.
        let Some(removed) = store.applications.remove(&input.application_id) else {
            return Err(DeleteApplicationErrorModeled::NotFoundException(crate::state::not_found(
                crate::state::ErrorCode::ApplicationNotFound,
                crate::state::application_not_found_message(&input.application_id),
            ))
            .into());
        };
        // The record owns its versions, so removing it removes them with it; the name
        // index is the only other place the application is held. Its entry is dropped
        // only when it still names THIS application, so a concurrent re-create of the
        // same name cannot have its index entry removed by this delete.
        let name_key = crate::state::name_key(&removed.region, &removed.name);
        if store
            .name_index
            .get(&name_key)
            .is_some_and(|indexed| *indexed == input.application_id)
        {
            store.name_index.remove(&name_key);
        }
        // A prepared template is a sub-resource of the application it was prepared from,
        // so the delete releases this application's templates with it rather than leaving
        // records no operation can ever reach or expire out of the store.
        store
            .templates
            .retain(|_, template| template.application_id != input.application_id);

        Ok(DeleteApplicationOutput)
    }
}
