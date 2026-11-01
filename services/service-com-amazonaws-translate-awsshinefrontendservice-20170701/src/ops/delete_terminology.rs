// SPDX-License-Identifier: CC0-1.0

//! @op DeleteTerminology
//!
//! Removes the stored custom terminology of that name, so a later
//! `GetTerminology` 404s, `ListTerminologies` no longer lists it and a
//! `TranslateText` naming it 404s. A name no import stored is a 404.
//!
//! Shared types live in `super::types`; the stored shapes and everything the
//! five terminology operations report by live in [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `DeleteTerminology`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! A synchronous action that deletes a custom terminology.
//!
//! ## Input
//!
//! * `Name` (ResourceName, len 1..256, pattern ^([A-Za-z0-9-]_?)+$) **required** — The name
//!   of the custom terminology being deleted.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{service_state, terminology_not_found, TERMINOLOGY_METADATA_RESOURCE};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteTerminologyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteTerminologyOutput;

impl DeleteTerminologyOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static DELETE_TERMINOLOGY_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteTerminologyOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteTerminologyOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteTerminologyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#DeleteTerminology")]
#[derive(Debug)]
pub enum DeleteTerminologyErrorModeled {
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_terminology::DeleteTerminologyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteTerminologyError = crate::scaffolded::Scaffolded<DeleteTerminologyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteTerminologyErrorModeled> for DeleteTerminologyError {
    fn from(modeled: DeleteTerminologyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteTerminology` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteTerminology<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteTerminology<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteTerminology<Eng>
{
    type Input = DeleteTerminologyInput;
    type Output = DeleteTerminologyOutput;
    type Error = DeleteTerminologyError;

    const NAME: &'static str = "DeleteTerminology";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TERMINOLOGY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_TERMINOLOGY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            DeleteTerminologyErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;

        let mut store = state.lock();
        if store.terminologies.remove(&input.name).is_none() {
            // A delete addresses the resource's metadata record, which is the
            // resource this refusal names.
            return Err(DeleteTerminologyErrorModeled::ResourceNotFoundException(
                terminology_not_found(TERMINOLOGY_METADATA_RESOURCE, &input.name),
            )
            .into());
        }
        Ok(DeleteTerminologyOutput)
    }
}
