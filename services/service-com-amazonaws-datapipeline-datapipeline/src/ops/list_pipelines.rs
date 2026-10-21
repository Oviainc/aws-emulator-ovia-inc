// SPDX-License-Identifier: CC0-1.0

//! @op ListPipelines
//!
//! Pages over the live pipelines persisted in [`crate::state`], in creation order,
//! returning `{id, name}` for each. A reporting operation: it writes nothing.
//!
//! The page cursor is a pipeline's creation sequence number rather than an offset into
//! the live set, so deleting a pipeline between two pages can neither skip nor repeat
//! another one. `marker` is the cursor to resume from; a marker this service did not
//! issue is refused with `InvalidRequestException`. The last page carries
//! `hasMoreResults: false` and no `marker`, as the output member documents ("If the value
//! is null, there are no more results").
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListPipelines`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-datapipeline-datapipeline/smithy-model/com-amazonaws-datapipeline-datapipeline.json")
//! ```
//!
//! Lists the pipeline identifiers for all active pipelines that you have permission to
//! access. POST / HTTP/1.1 Content-Type: application/x-amz-json-1.1 X-Amz-Target:
//! DataPipeline.ListPipelines Content-Length: 14 Host: datapipeline.us-east-1.amazonaws.com
//! X-Amz-Date: Mon, 12 Nov 2012 17:49:52 GMT Authorization: AuthParams {} Status:
//! x-amzn-RequestId: b3104dc5-0734-11e2-af6f-6bc7a6be60d9 Content-Type:
//! application/x-amz-json-1.1 Content-Length: 39 Date: Mon, 12 Nov 2012 17:50:53 GMT
//! {"PipelineIdList": [ {"id": "df-08785951KAKJEXAMPLE", "name": "MyPipeline"}, {"id":
//! "df-08662578ISYEXAMPLE", "name": "MySecondPipeline"} ] }
//!
//! ## Input
//!
//! * `marker` (string, len 0..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) — The starting point for
//!   the results to be returned. For the first call, this value should be empty. As long as
//!   there are more results, continue to call ListPipelines with the marker value from the
//!   previous call to retrieve the next set of results.
//!
//! ## Output
//!
//! * `hasMoreResults` (boolean, default false) **required** — Indicates whether there are
//!   more results that can be obtained by a subsequent call.
//! * `marker` (string, len 0..1024, pattern
//!   ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$) — The starting point for
//!   the next page of results. To view the next page of results, call ListPipelinesOutput
//!   again with this marker value. If the value is null, there are no more results.
//! * `pipelineIdList` (pipelineList) **required** — The pipeline identifiers. If you require
//!   additional information about the pipelines, you can use these identifiers to call
//!   DescribePipelines and GetPipelineDefinition.
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
// `ListPipelinesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-datapipeline-datapipeline.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPipelinesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.datapipeline#ListPipelines")]
#[derive(Debug)]
pub enum ListPipelinesErrorModeled {
    InternalServiceError(InternalServiceError),
    InvalidRequestException(InvalidRequestException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_pipelines::ListPipelinesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPipelinesError = crate::scaffolded::Scaffolded<ListPipelinesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPipelinesErrorModeled> for ListPipelinesError {
    fn from(modeled: ListPipelinesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPipelines` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPipelines<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPipelines<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPipelines<Eng>
{
    type Input = ListPipelinesInput;
    type Output = ListPipelinesOutput;
    type Error = ListPipelinesError;

    const NAME: &'static str = "ListPipelines";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PIPELINES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_PIPELINES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListPipelinesErrorModeled::InternalServiceError(
                InternalServiceError {
                    message: Some(
                        "the Data Pipeline store is not reachable from this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let state = state.lock();

        // An absent or empty marker starts at the beginning: "For the first call, this
        // value should be empty" (the `marker` input member doc).
        let after = match input.marker.as_deref().filter(|marker| !marker.is_empty()) {
            None => 0,
            Some(marker) => decode_marker(marker).ok_or_else(|| {
                ListPipelinesErrorModeled::InvalidRequestException(InvalidRequestException {
                    message: Some(format!(
                        "marker is not a marker this service issued: {marker}"
                    )),
                })
            })?,
        };

        let remaining: Vec<&crate::state::Pipeline> = state
            .pipelines_in_creation_order()
            .into_iter()
            .filter(|pipeline| pipeline.seq > after)
            .collect();
        let page = &remaining[..remaining.len().min(PAGE_SIZE)];
        let has_more_results = remaining.len() > page.len();
        let next_marker = if has_more_results {
            page.last().map(|pipeline| encode_marker(pipeline.seq))
        } else {
            None
        };

        Ok(ListPipelinesOutput {
            pipeline_id_list: page
                .iter()
                .map(|pipeline| PipelineIdName {
                    id: Some(pipeline.pipeline_id.clone()),
                    name: Some(pipeline.name.clone()),
                })
                .collect(),
            marker: next_marker,
            has_more_results,
        })
    }
}

/// How many pipelines one `ListPipelines` page carries.
///
/// AWS does not publish a page size for this operation, so this is a chosen constant: it
/// only has to be stable, so a caller that follows `marker` until `hasMoreResults` is
/// false sees every pipeline exactly once.
pub const PAGE_SIZE: usize = 25;

/// Render a creation sequence number as the opaque `marker` the caller sends back.
fn encode_marker(seq: u64) -> String {
    format!("{MARKER_PREFIX}{seq:016x}")
}

/// Read a `marker` this service issued back into the creation sequence number it stands
/// for, or `None` when it does not have the shape of one.
fn decode_marker(marker: &str) -> Option<u64> {
    let digits = marker.strip_prefix(MARKER_PREFIX)?;
    u64::from_str_radix(digits, 16).ok()
}

/// Marks a `marker` as this service's own, so a token from somewhere else is refused
/// rather than silently read as a position. This is a SHAPE check, not proof the marker
/// was issued here: a fabricated but well-formed token is read as the cursor it spells,
/// which is a creation sequence number and so can only move the page window.
const MARKER_PREFIX: &str = "dp1:";
