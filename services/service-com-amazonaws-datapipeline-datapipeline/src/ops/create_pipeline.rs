// SPDX-License-Identifier: CC0-1.0

//! @op CreatePipeline
//!
//! Mints a `pipelineId` and PERSISTS an empty pipeline (no definition) in
//! [`crate::state`], so every later `PutPipelineDefinition` / `GetPipelineDefinition` /
//! `DescribePipelines` / `ListPipelines` / `DeletePipeline` call resolves that id out of
//! the store.
//!
//! The documented `uniqueId` idempotency contract (see the `uniqueId` member doc below)
//! is enforced on the `uniqueId` alone:
//!
//! * the same `(name, uniqueId)` returns the `pipelineId` the first call assigned, and
//!   creates nothing;
//! * the same `uniqueId` under a different `name` is refused with
//!   `InvalidRequestException` — the claim is held by the pipeline that took it.
//!
//! This is the one request path that grows the store, so a create that would take the
//! account past [`crate::state::MAX_PIPELINES`] live pipelines is refused with
//! `InvalidRequestException`. A repeat of an existing `(name, uniqueId)` creates nothing
//! and so is answered from the claim regardless of the quota.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePipeline`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-datapipeline-datapipeline/smithy-model/com-amazonaws-datapipeline-datapipeline.json")
//! ```
//!
//! Creates a new, empty pipeline. Use PutPipelineDefinition to populate the pipeline. POST
//! / HTTP/1.1 Content-Type: application/x-amz-json-1.1 X-Amz-Target:
//! DataPipeline.CreatePipeline Content-Length: 91 Host:
//! datapipeline.us-east-1.amazonaws.com X-Amz-Date: Mon, 12 Nov 2012 17:49:52 GMT
//! Authorization: AuthParams {"name": "myPipeline", "uniqueId": "123456789", "description":
//! "This is my first pipeline"} HTTP/1.1 200 x-amzn-RequestId:
//! b16911ce-0774-11e2-af6f-6bc7a6be60d9 Content-Type: application/x-amz-json-1.1
//! Content-Length: 40 Date: Mon, 12 Nov 2012 17:50:53 GMT {"pipelineId":
//! "df-06372391ZG65EXAMPLE"}
//!
//! ## Input
//!
//! * `description` (string, len 0..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) — The description for
//!   the pipeline.
//! * `name` (id, len 1..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) **required** — The name
//!   for the pipeline. You can use the same name for multiple pipelines associated with your
//!   AWS account, because AWS Data Pipeline assigns each pipeline a unique pipeline
//!   identifier.
//! * `tags` (tagList, len 0..10) — A list of tags to associate with the pipeline at creation.
//!   Tags let you control access to pipelines. For more information, see Controlling User
//!   Access to Pipelines in the AWS Data Pipeline Developer Guide.
//! * `uniqueId` (id, len 1..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) **required** — A unique
//!   identifier. This identifier is not the same as the pipeline identifier assigned by AWS
//!   Data Pipeline. You are responsible for defining the format and ensuring the uniqueness
//!   of this identifier. You use this parameter to ensure idempotency during repeated calls
//!   to CreatePipeline. For example, if the first call to CreatePipeline does not succeed,
//!   you can pass in the same unique identifier and pipeline name combination on a subsequent
//!   call to CreatePipeline. CreatePipeline ensures that if a pipeline already exists with
//!   the same name and unique identifier, a new pipeline is not created. Instead, you'll
//!   receive the pipeline identifier from the previous attempt. The uniqueness of the …
//!
//! ## Output
//!
//! * `pipelineId` (id, len 1..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) **required** — The ID
//!   that AWS Data Pipeline assigns the newly created pipeline. For example,
//!   df-06372391ZG65EXAMPLE.
//!
//! ## Errors
//!
//! * `InternalServiceError` (HTTP 500) — An internal service error occurred.
//! * `InvalidRequestException` (HTTP 400) — The request was not valid. Verify that your
//!   request was properly formatted, that the signature was generated with the correct
//!   credentials, and that you haven't exceeded any of the service limits for your account.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePipelineError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-datapipeline-datapipeline.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePipelineError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.datapipeline#CreatePipeline")]
#[derive(Debug)]
pub enum CreatePipelineErrorModeled {
    InternalServiceError(InternalServiceError),
    InvalidRequestException(InvalidRequestException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_pipeline::CreatePipelineError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreatePipelineError = crate::scaffolded::Scaffolded<CreatePipelineErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreatePipelineErrorModeled> for CreatePipelineError {
    fn from(modeled: CreatePipelineErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePipeline` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreatePipeline<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePipeline<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePipeline<Eng>
{
    type Input = CreatePipelineInput;
    type Output = CreatePipelineOutput;
    type Error = CreatePipelineError;

    const NAME: &'static str = "CreatePipeline";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PIPELINE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_PIPELINE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_service_error());
        };
        let mut state = state.lock();

        // The documented idempotency contract: a `uniqueId` already claimed answers with
        // the pipeline that claimed it when the name matches, and is refused otherwise.
        if let Some(claim) = state.unique_ids.get(&input.unique_id) {
            if claim.name == input.name {
                return Ok(CreatePipelineOutput {
                    pipeline_id: claim.pipeline_id.clone(),
                });
            }
            return Err(CreatePipelineErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "uniqueId {} is already in use by pipeline {} under the name {}",
                        input.unique_id, claim.pipeline_id, claim.name
                    )),
                },
            )
            .into());
        }

        // A fresh `uniqueId` means a new pipeline, so this is the one request path that
        // grows the store: refuse past the account quota rather than letting a caller
        // loop `CreatePipeline` without limit.
        if state.pipelines.len() >= crate::state::MAX_PIPELINES {
            return Err(CreatePipelineErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "this account already holds the maximum of {} pipelines; delete one \
                         before creating another",
                        crate::state::MAX_PIPELINES
                    )),
                },
            )
            .into());
        }

        let (pipeline_id, seq) = state.mint_pipeline_id();
        let pipeline = crate::state::Pipeline {
            pipeline_id: pipeline_id.clone(),
            name: input.name.clone(),
            unique_id: input.unique_id.clone(),
            description: input.description,
            tags: input.tags.unwrap_or_default(),
            // A new pipeline holds no definition, so it is PENDING until a definition is
            // committed and activated.
            state: crate::state::PipelineState::Pending,
            created_at: ::std::time::SystemTime::now(),
            seq,
            definition_version: 0,
            definition: None,
        };
        state.pipelines.insert(pipeline_id.clone(), pipeline);
        state.unique_ids.insert(
            input.unique_id,
            crate::state::UniqueIdClaim {
                name: input.name,
                pipeline_id: pipeline_id.clone(),
            },
        );

        Ok(CreatePipelineOutput { pipeline_id })
    }
}

/// The modelled server error for a request that cannot reach this service's store.
///
/// Answered instead of constructing an engine: a handler-built engine is a private store
/// whose writes nothing else ever sees.
fn internal_service_error() -> CreatePipelineError {
    CreatePipelineErrorModeled::InternalServiceError(InternalServiceError {
        message: Some("the Data Pipeline store is not reachable from this engine".to_string()),
    })
    .into()
}
