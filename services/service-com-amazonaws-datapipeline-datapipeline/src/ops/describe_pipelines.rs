// SPDX-License-Identifier: CC0-1.0

//! @op DescribePipelines
//!
//! Builds one `PipelineDescription` per requested id out of the stored pipeline record in
//! [`crate::state`]. A reporting operation: it writes nothing.
//!
//! `PipelineDescription$fields` carries the read-only metadata the member documents ("a
//! list of read-only fields that contain metadata about the pipeline: @userId, @accountId,
//! and @pipelineState"). The exact field set emitted is the one the `DescribePipelines`
//! response example in this service's Smithy model documentation shows — see
//! [`METADATA_FIELD_KEYS`].
//!
//! Every requested id must resolve: an unknown id is `PipelineNotFoundException` and a
//! deleted one `PipelineDeletedException`, both HTTP 400. `pipelineIds` carries no
//! modelled length constraint, so the count is checked against the limit the member
//! documents ([`MAX_PIPELINE_IDS`]) before anything is allocated — one description costs
//! far more than the id that asks for it, so an unbounded list would be an unbounded
//! amplification.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribePipelines`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-datapipeline-datapipeline/smithy-model/com-amazonaws-datapipeline-datapipeline.json")
//! ```
//!
//! Retrieves metadata about one or more pipelines. The information retrieved includes the
//! name of the pipeline, the pipeline identifier, its current state, and the user account
//! that owns the pipeline. Using account credentials, you can retrieve metadata about
//! pipelines that you or your IAM users have created. If you are using an IAM user account,
//! you can retrieve metadata about only those pipelines for which you have read
//! permissions. To retrieve the full pipeline definition instead of metadata about the
//! pipeline, call GetPipelineDefinition. POST / HTTP/1.1 Content-Type:
//! application/x-amz-json-1.1 X-Amz-Target: DataPipeline.DescribePipelines Content-Length:
//! 70 Host: datapipeline.us-east-1.amazonaws.com X-Amz-Date: Mon, 12 Nov 2012 17:49:52 GMT
//! Authorization: AuthParams {"pipelineIds": ["df-08785951KAKJEXAMPLE"] } x-amzn-RequestId:
//! 02870eb7-0736-11e2-af6f-6bc7a6be60d9 Content-Type: application/x-amz-json-1.1
//! Content-Length: 767 Date: Mon, 12 Nov 2012 17:50:53 GMT {"pipelineDescriptionList": [
//! {"description": "This is my first pipeline", "fields": [ {"key": "@pipelineState",
//! "stringValue": "SCHEDULED"}, {"key": "description", "stringValue": "This is my first
//! pipeline"}, {"key": "name", "stringValue": "myPipeline"}, {"key": "@creationTime",
//! "stringValue": "2012-12-13T01:24:06"}, {"key": "@id", "stringValue":
//! "df-0937003356ZJEXAMPLE"}, {"key": "@sphere", "stringValue": "PIPELINE"}, {"key":
//! "@version", "stringValue": "1"}, {"key": "@userId", "stringValue": "924374875933"},
//! {"key": "@accountId", "stringValue": "924374875933"}, {"key": "uniqueId", "stringValue":
//! "1234567890"} ], "name": "myPipeline", "pipelineId": "df-0937003356ZJEXAMPLE"} ] }
//!
//! ## Input
//!
//! * `pipelineIds` (idList) **required** — The IDs of the pipelines to describe. You can pass
//!   as many as 25 identifiers in a single call. To obtain pipeline IDs, call ListPipelines.
//!
//! ## Output
//!
//! * `pipelineDescriptionList` (PipelineDescriptionList) **required** — An array of
//!   descriptions for the specified pipelines.
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
// `DescribePipelinesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-datapipeline-datapipeline.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribePipelinesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.datapipeline#DescribePipelines")]
#[derive(Debug)]
pub enum DescribePipelinesErrorModeled {
    InternalServiceError(InternalServiceError),
    InvalidRequestException(InvalidRequestException),
    PipelineDeletedException(PipelineDeletedException),
    PipelineNotFoundException(PipelineNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_pipelines::DescribePipelinesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribePipelinesError = crate::scaffolded::Scaffolded<DescribePipelinesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribePipelinesErrorModeled> for DescribePipelinesError {
    fn from(modeled: DescribePipelinesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribePipelines` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribePipelines<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribePipelines<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribePipelines<Eng>
{
    type Input = DescribePipelinesInput;
    type Output = DescribePipelinesOutput;
    type Error = DescribePipelinesError;

    const NAME: &'static str = "DescribePipelines";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_PIPELINES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_PIPELINES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // `pipelineIds` has no modelled bound, and each entry turns ~26 request bytes
        // into a full PipelineDescription, so the caller-chosen count is clamped to the
        // documented maximum BEFORE the result list is allocated.
        if input.pipeline_ids.len() > MAX_PIPELINE_IDS {
            return Err(DescribePipelinesErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "pipelineIds holds {} identifiers; at most {MAX_PIPELINE_IDS} may be \
                         passed in a single call",
                        input.pipeline_ids.len()
                    )),
                },
            )
            .into());
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribePipelinesErrorModeled::InternalServiceError(
                InternalServiceError {
                    message: Some(
                        "the Data Pipeline store is not reachable from this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let state = state.lock();

        let mut pipeline_description_list = Vec::new();
        for pipeline_id in &input.pipeline_ids {
            let pipeline = state
                .pipeline(pipeline_id)
                .map_err(|lookup| lookup_error(lookup, pipeline_id))?;
            pipeline_description_list.push(describe(pipeline));
        }

        Ok(DescribePipelinesOutput {
            pipeline_description_list,
        })
    }
}

/// How many pipeline identifiers one `DescribePipelines` call may name.
///
/// Source: the `pipelineIds` member documentation — "You can pass as many as 25
/// identifiers in a single call". The model declares no `length` constraint for the list,
/// so the boundary validator does not enforce it and this handler does.
pub const MAX_PIPELINE_IDS: usize = 25;

/// The metadata field keys a `PipelineDescription` carries, in the order they are
/// emitted.
///
/// Source: the `DescribePipelines` response example in this service's Smithy model
/// documentation (`smithy_query(query="doc",shape="com.amazonaws.datapipeline#DescribePipelines")`),
/// which lists `@pipelineState`, `description`, `name`, `@creationTime`, `@id`,
/// `@sphere`, `@version`, `@userId`, `@accountId` and `uniqueId`. `description` is
/// emitted only when the pipeline has one.
pub const METADATA_FIELD_KEYS: &[&str] = &[
    "@pipelineState",
    "description",
    "name",
    "@creationTime",
    "@id",
    "@sphere",
    "@version",
    "@userId",
    "@accountId",
    "uniqueId",
];

/// The `@sphere` of a pipeline object that IS the pipeline (as opposed to a component,
/// instance or attempt), per the response example above.
const PIPELINE_SPHERE: &str = "PIPELINE";

/// One `PipelineDescription` computed from a stored pipeline.
fn describe(pipeline: &crate::state::Pipeline) -> PipelineDescription {
    let mut fields = vec![field("@pipelineState", pipeline.state.as_str())];
    if let Some(description) = &pipeline.description {
        fields.push(field("description", description));
    }
    fields.push(field("name", &pipeline.name));
    fields.push(field(
        "@creationTime",
        &crate::state::format_creation_time(pipeline.created_at),
    ));
    fields.push(field("@id", &pipeline.pipeline_id));
    fields.push(field("@sphere", PIPELINE_SPHERE));
    fields.push(field("@version", &pipeline.reported_version()));
    // The response example shows `@userId` and `@accountId` carrying the same account
    // identifier for an account-credential caller; this emulator presents one canonical
    // account, so both report it.
    fields.push(field("@userId", ::engine::emulator_account_id()));
    fields.push(field("@accountId", ::engine::emulator_account_id()));
    fields.push(field("uniqueId", &pipeline.unique_id));

    PipelineDescription {
        pipeline_id: pipeline.pipeline_id.clone(),
        name: pipeline.name.clone(),
        fields,
        description: pipeline.description.clone(),
        tags: Some(pipeline.tags.clone()).filter(|tags| !tags.is_empty()),
    }
}

/// A read-only metadata `Field`: these always carry a string value, never a reference.
fn field(key: &str, string_value: &str) -> Field {
    Field {
        key: key.to_string(),
        string_value: Some(string_value.to_string()),
        ref_value: None,
    }
}

/// The modelled error a failed pipeline lookup becomes for this operation.
fn lookup_error(
    lookup: crate::state::PipelineLookupError,
    pipeline_id: &str,
) -> DescribePipelinesError {
    let message = Some(lookup.message(pipeline_id));
    match lookup {
        crate::state::PipelineLookupError::Unknown => {
            DescribePipelinesErrorModeled::PipelineNotFoundException(PipelineNotFoundException {
                message,
            })
        }
        crate::state::PipelineLookupError::Deleted => {
            DescribePipelinesErrorModeled::PipelineDeletedException(PipelineDeletedException {
                message,
            })
        }
    }
    .into()
}
