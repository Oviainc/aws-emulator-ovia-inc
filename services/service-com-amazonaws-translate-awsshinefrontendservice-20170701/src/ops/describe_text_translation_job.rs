// SPDX-License-Identifier: CC0-1.0

//! @op DescribeTextTranslationJob
//!
//! Reports the properties of one submitted batch translation job: its name and
//! id, the status the clock owes it, the counts of the documents it processed,
//! its languages, and the configuration the submission was made with.
//!
//! Everything reported comes from the record
//! [`crate::ops::start_text_translation_job`] persisted and from the instants
//! that record carries, so this is a pure READ: polling a job never moves it.
//! The record and the status rule live in [`crate::state::batch_translation`],
//! so every operation that reports a job reports it by one rule.
//!
//! ---
//!
//! # Modeled contract for `DescribeTextTranslationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Gets the properties associated with an asynchronous batch translation job including
//! name, ID, status, source and target languages, input/output S3 buckets, and so on.
//!
//! ## Input
//!
//! * `JobId` (JobId, len 1..32, pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$) **required** — The
//!   identifier that Amazon Translate generated for the job. The StartTextTranslationJob
//!   operation returns this identifier in its response.
//!
//! ## Output
//!
//! * `TextTranslationJobProperties` (TextTranslationJobProperties) — An object that contains
//!   the properties associated with an asynchronous batch translation job.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The resource you are looking for has not been
//!   found. Review the resource you're looking for and see if a different resource will
//!   accomplish your needs before retrying the revised request.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

use crate::state::{resource_not_found, service_state};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeTextTranslationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeTextTranslationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#DescribeTextTranslationJob")]
#[derive(Debug)]
pub enum DescribeTextTranslationJobErrorModeled {
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_text_translation_job::DescribeTextTranslationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeTextTranslationJobError = crate::scaffolded::Scaffolded<DescribeTextTranslationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeTextTranslationJobErrorModeled> for DescribeTextTranslationJobError {
    fn from(modeled: DescribeTextTranslationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeTextTranslationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeTextTranslationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeTextTranslationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeTextTranslationJob<Eng>
{
    type Input = DescribeTextTranslationJobInput;
    type Output = DescribeTextTranslationJobOutput;
    type Error = DescribeTextTranslationJobError;

    const NAME: &'static str = "DescribeTextTranslationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_TEXT_TRANSLATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_TEXT_TRANSLATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = service_state(&*self.engine).ok_or_else(|| {
            DescribeTextTranslationJobErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "the batch translation job store is unavailable".to_string(),
                    ),
                },
            )
        })?;
        let now = SystemTime::now();

        let store = state.lock();
        let Some(job) = store.jobs.get(&input.job_id) else {
            return Err(
                DescribeTextTranslationJobErrorModeled::ResourceNotFoundException(
                    resource_not_found(),
                )
                .into(),
            );
        };
        Ok(DescribeTextTranslationJobOutput {
            text_translation_job_properties: Some(job.properties(now)),
        })
    }
}
