// SPDX-License-Identifier: CC0-1.0

//! @op DeletePipeline
//!
//! Removes the pipeline and its definition from [`crate::state`] and TOMBSTONES its id.
//!
//! "Deleting a pipeline cannot be undone. You cannot query or restore a deleted
//! pipeline" (the operation doc below), so the id is remembered as deleted: a later
//! `DescribePipelines` / `GetPipelineDefinition` / `PutPipelineDefinition` on it answers
//! `PipelineDeletedException` rather than claiming it never existed, and `ListPipelines`
//! no longer lists it. `DeletePipeline` itself models no `PipelineDeletedException`, so a
//! repeat delete of the same id answers `PipelineNotFoundException` — the only error its
//! model gives it for an id that names no live pipeline.
//!
//! Deleting also releases the `uniqueId` the pipeline claimed: `CreatePipeline`'s
//! idempotency holds only while "a pipeline already exists with the same name and unique
//! identifier", and after a delete none does.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeletePipeline`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-datapipeline-datapipeline/smithy-model/com-amazonaws-datapipeline-datapipeline.json")
//! ```
//!
//! Deletes a pipeline, its pipeline definition, and its run history. AWS Data Pipeline
//! attempts to cancel instances associated with the pipeline that are currently being
//! processed by task runners. Deleting a pipeline cannot be undone. You cannot query or
//! restore a deleted pipeline. To temporarily pause a pipeline instead of deleting it, call
//! SetStatus with the status set to PAUSE on individual components. Components that are
//! paused by SetStatus can be resumed. POST / HTTP/1.1 Content-Type:
//! application/x-amz-json-1.1 X-Amz-Target: DataPipeline.DeletePipeline Content-Length: 50
//! Host: datapipeline.us-east-1.amazonaws.com X-Amz-Date: Mon, 12 Nov 2012 17:49:52 GMT
//! Authorization: AuthParams {"pipelineId": "df-06372391ZG65EXAMPLE"} x-amzn-RequestId:
//! b7a88c81-0754-11e2-af6f-6bc7a6be60d9 Content-Type: application/x-amz-json-1.1
//! Content-Length: 0 Date: Mon, 12 Nov 2012 17:50:53 GMT Unexpected response: 200, OK,
//! undefined
//!
//! ## Input
//!
//! * `pipelineId` (id, len 1..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) **required** — The ID of
//!   the pipeline.
//!
//! ## Errors
//!
//! * `InternalServiceError` (HTTP 500) — An internal service error occurred.
//! * `InvalidRequestException` (HTTP 400) — The request was not valid. Verify that your
//!   request was properly formatted, that the signature was generated with the correct
//!   credentials, and that you haven't exceeded any of the service limits for your account.
//! * `PipelineNotFoundException` (HTTP 400) — The specified pipeline was not found. Verify
//!   that you used the correct user and account identifiers.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeletePipelineError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-datapipeline-datapipeline.json");

#[derive(Debug, Clone, Default)]
pub struct DeletePipelineOutput;

impl DeletePipelineOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static DELETE_PIPELINE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeletePipelineOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeletePipelineOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeletePipelineError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.datapipeline#DeletePipeline")]
#[derive(Debug)]
pub enum DeletePipelineErrorModeled {
    InternalServiceError(InternalServiceError),
    InvalidRequestException(InvalidRequestException),
    PipelineNotFoundException(PipelineNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_pipeline::DeletePipelineError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeletePipelineError = crate::scaffolded::Scaffolded<DeletePipelineErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeletePipelineErrorModeled> for DeletePipelineError {
    fn from(modeled: DeletePipelineErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeletePipeline` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeletePipeline<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeletePipeline<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeletePipeline<Eng>
{
    type Input = DeletePipelineInput;
    type Output = DeletePipelineOutput;
    type Error = DeletePipelineError;

    const NAME: &'static str = "DeletePipeline";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_PIPELINE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_PIPELINE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeletePipelineErrorModeled::InternalServiceError(
                InternalServiceError {
                    message: Some(
                        "the Data Pipeline store is not reachable from this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let mut state = state.lock();

        state.remove_pipeline(&input.pipeline_id).map_err(|_| {
            // This operation models no `PipelineDeletedException`, so an already-deleted
            // id and an id that never existed both answer `PipelineNotFoundException`.
            DeletePipelineErrorModeled::PipelineNotFoundException(PipelineNotFoundException {
                message: Some(
                    crate::state::PipelineLookupError::Unknown.message(&input.pipeline_id),
                ),
            })
        })?;

        Ok(DeletePipelineOutput)
    }
}
