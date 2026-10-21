// SPDX-License-Identifier: CC0-1.0

//! @op GetPipelineDefinition
//!
//! Reads the definition `PutPipelineDefinition` committed back out of [`crate::state`] —
//! the stored `pipelineObjects`, `parameterObjects` and `parameterValues`, never anything
//! echoed from the current request.
//!
//! `version` selects which definition, per its member documentation below: `latest` (the
//! default) is the last definition saved, `active` is the last definition that was
//! ACTIVATED. Nothing can be activated while `ActivatePipeline` is scaffolded (HTTP 501),
//! so `active` reports no definition rather than pretending the saved one is active. Any
//! other value is refused with `InvalidRequestException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPipelineDefinition`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-datapipeline-datapipeline/smithy-model/com-amazonaws-datapipeline-datapipeline.json")
//! ```
//!
//! Gets the definition of the specified pipeline. You can call GetPipelineDefinition to
//! retrieve the pipeline definition that you provided using PutPipelineDefinition. POST /
//! HTTP/1.1 Content-Type: application/x-amz-json-1.1 X-Amz-Target:
//! DataPipeline.GetPipelineDefinition Content-Length: 40 Host:
//! datapipeline.us-east-1.amazonaws.com X-Amz-Date: Mon, 12 Nov 2012 17:49:52 GMT
//! Authorization: AuthParams {"pipelineId": "df-06372391ZG65EXAMPLE"} x-amzn-RequestId:
//! e28309e5-0776-11e2-8a14-21bb8a1f50ef Content-Type: application/x-amz-json-1.1
//! Content-Length: 890 Date: Mon, 12 Nov 2012 17:50:53 GMT {"pipelineObjects": [ {"fields":
//! [ {"key": "workerGroup", "stringValue": "workerGroup"} ], "id": "Default", "name":
//! "Default"}, {"fields": [ {"key": "startDateTime", "stringValue": "2012-09-25T17:00:00"},
//! {"key": "type", "stringValue": "Schedule"}, {"key": "period", "stringValue": "1 hour"},
//! {"key": "endDateTime", "stringValue": "2012-09-25T18:00:00"} ], "id": "Schedule",
//! "name": "Schedule"}, {"fields": [ {"key": "schedule", "refValue": "Schedule"}, {"key":
//! "command", "stringValue": "echo hello"}, {"key": "parent", "refValue": "Default"},
//! {"key": "type", "stringValue": "ShellCommandActivity"} ], "id": "SayHello", "name":
//! "SayHello"} ] }
//!
//! ## Input
//!
//! * `pipelineId` (id, len 1..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) **required** — The ID of
//!   the pipeline.
//! * `version` (string, len 0..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) — The version of the
//!   pipeline definition to retrieve. Set this parameter to latest (default) to use the last
//!   definition saved to the pipeline or active to use the last definition that was
//!   activated.
//!
//! ## Output
//!
//! * `parameterObjects` (ParameterObjectList) — The parameter objects used in the pipeline
//!   definition.
//! * `parameterValues` (ParameterValueList) — The parameter values used in the pipeline
//!   definition.
//! * `pipelineObjects` (PipelineObjectList) — The objects defined in the pipeline.
//!
//! ## Errors
//!
//! * `InternalServiceError` (HTTP 500) — An internal service error occurred.
//! * `InvalidRequestException` (HTTP 400) — The request was not valid. Verify that your
//!   request was properly formatted, that the signature was generated with the correct
//!   credentials, and that you haven't exceeded any of the service limits for your account.
//! * `PipelineDeletedException` (HTTP 400) — The specified pipeline has been deleted.
//! * `PipelineNotFoundException` (HTTP 400) — The specified pipeline was not found. Verify
//!   that you used the correct user and account identifiers.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPipelineDefinitionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-datapipeline-datapipeline.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPipelineDefinitionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.datapipeline#GetPipelineDefinition")]
#[derive(Debug)]
pub enum GetPipelineDefinitionErrorModeled {
    InternalServiceError(InternalServiceError),
    InvalidRequestException(InvalidRequestException),
    PipelineDeletedException(PipelineDeletedException),
    PipelineNotFoundException(PipelineNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_pipeline_definition::GetPipelineDefinitionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPipelineDefinitionError = crate::scaffolded::Scaffolded<GetPipelineDefinitionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPipelineDefinitionErrorModeled> for GetPipelineDefinitionError {
    fn from(modeled: GetPipelineDefinitionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPipelineDefinition` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPipelineDefinition<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPipelineDefinition<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPipelineDefinition<Eng>
{
    type Input = GetPipelineDefinitionInput;
    type Output = GetPipelineDefinitionOutput;
    type Error = GetPipelineDefinitionError;

    const NAME: &'static str = "GetPipelineDefinition";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PIPELINE_DEFINITION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PIPELINE_DEFINITION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetPipelineDefinitionErrorModeled::InternalServiceError(
                InternalServiceError {
                    message: Some(
                        "the Data Pipeline store is not reachable from this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let state = state.lock();

        let pipeline = state
            .pipeline(&input.pipeline_id)
            .map_err(|lookup| lookup_error(lookup, &input.pipeline_id))?;

        let requested = input.version.as_deref().unwrap_or(VERSION_LATEST);
        let definition = match requested {
            VERSION_LATEST => pipeline.definition.as_ref(),
            // No definition has ever been activated: `ActivatePipeline` is scaffolded, so
            // there is no active version to report.
            VERSION_ACTIVE => None,
            other => {
                return Err(GetPipelineDefinitionErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        message: Some(format!(
                            "version must be one of {VERSION_LATEST}, {VERSION_ACTIVE}; got \
                             {other}"
                        )),
                    },
                )
                .into())
            }
        };

        // A pipeline with no definition for the requested version reports an empty one
        // rather than failing: the pipeline itself resolved.
        let Some(definition) = definition else {
            return Ok(GetPipelineDefinitionOutput::default());
        };

        Ok(GetPipelineDefinitionOutput {
            pipeline_objects: Some(definition.pipeline_objects.clone()),
            // Parameters are optional members: a definition committed without them reads
            // back without them, rather than with an empty list.
            parameter_objects: Some(definition.parameter_objects.clone())
                .filter(|objects| !objects.is_empty()),
            parameter_values: Some(definition.parameter_values.clone())
                .filter(|values| !values.is_empty()),
        })
    }
}

/// The `version` value selecting the last definition SAVED to the pipeline, and the
/// default when the member is absent (`GetPipelineDefinitionInput$version` doc).
pub const VERSION_LATEST: &str = "latest";

/// The `version` value selecting the last definition that was ACTIVATED
/// (`GetPipelineDefinitionInput$version` doc).
pub const VERSION_ACTIVE: &str = "active";

/// The modelled error a failed pipeline lookup becomes for this operation.
fn lookup_error(
    lookup: crate::state::PipelineLookupError,
    pipeline_id: &str,
) -> GetPipelineDefinitionError {
    let message = Some(lookup.message(pipeline_id));
    match lookup {
        crate::state::PipelineLookupError::Unknown => {
            GetPipelineDefinitionErrorModeled::PipelineNotFoundException(
                PipelineNotFoundException { message },
            )
        }
        crate::state::PipelineLookupError::Deleted => {
            GetPipelineDefinitionErrorModeled::PipelineDeletedException(
                PipelineDeletedException { message },
            )
        }
    }
    .into()
}
