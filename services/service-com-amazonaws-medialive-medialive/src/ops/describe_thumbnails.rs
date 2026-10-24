// SPDX-License-Identifier: CC0-1.0

//! @op DescribeThumbnails
//!
//! Reports the latest thumbnail frame one pipeline of one channel has captured,
//! out of that channel's stored capture log
//! (`ComAmazonawsMedialiveMedialiveState::capture_channel_thumbnail` is the one
//! writer of it).
//!
//! A frame is reported only while the pipeline it came from is up and the
//! channel's thumbnail feature is on, which
//! [`crate::state::ChannelRecord::latest_thumbnail`] owns; a channel that has
//! captured nothing reportable answers an EMPTY `ThumbnailDetails` list, which
//! is what every channel created through this emulator's API answers, because
//! this emulator encodes no media and so captures no frames of its own. Nothing
//! here invents image data.
//!
//! A REPORTING operation: it materialises the transitions the clock had already
//! made due and writes nothing else.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeThumbnails`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json")
//! ```
//!
//! Describe the latest thumbnails data.
//!
//! ## Input
//!
//! * `ChannelId` (__string) **required** — Unique ID of the channel
//! * `PipelineId` (__string) **required** — Pipeline ID ("0" or "1")
//! * `ThumbnailType` (__string) **required** — thumbnail type
//!
//! ## Output
//!
//! * `ThumbnailDetails` (__listOfThumbnailDetail) — Placeholder documentation for
//!   __listOfThumbnailDetail
//!
//! ## Errors
//!
//! * `BadGatewayException` (HTTP 502) — Placeholder documentation for BadGatewayException
//! * `BadRequestException` (HTTP 400) — Placeholder documentation for BadRequestException
//! * `ConflictException` (HTTP 409) — Placeholder documentation for ConflictException
//! * `ForbiddenException` (HTTP 403) — Placeholder documentation for ForbiddenException
//! * `GatewayTimeoutException` (HTTP 504) — Placeholder documentation for
//!   GatewayTimeoutException
//! * `InternalServerErrorException` (HTTP 500) — Placeholder documentation for
//!   InternalServerErrorException
//! * `NotFoundException` (HTTP 404) — Placeholder documentation for NotFoundException
//! * `TooManyRequestsException` (HTTP 429) — Placeholder documentation for
//!   TooManyRequestsException

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeThumbnailsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medialive-medialive.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeThumbnailsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medialive#DescribeThumbnails")]
#[derive(Debug)]
pub enum DescribeThumbnailsErrorModeled {
    BadGatewayException(BadGatewayException),
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    ForbiddenException(ForbiddenException),
    GatewayTimeoutException(GatewayTimeoutException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_thumbnails::DescribeThumbnailsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeThumbnailsError = crate::scaffolded::Scaffolded<DescribeThumbnailsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeThumbnailsErrorModeled> for DescribeThumbnailsError {
    fn from(modeled: DescribeThumbnailsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeThumbnails` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeThumbnails<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeThumbnails<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeThumbnails<Eng>
{
    type Input = DescribeThumbnailsInput;
    type Output = DescribeThumbnailsOutput;
    type Error = DescribeThumbnailsError;

    const NAME: &'static str = "DescribeThumbnails";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_THUMBNAILS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_THUMBNAILS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;

        let mut state = state.lock();
        // Reporting only: `settle` materialises what the clock already made due,
        // so the channel this read resolves is the one whose start, stop, restart
        // or delete had already come due at this instant — and nothing else is
        // written.
        state.settle(::std::time::SystemTime::now());
        let state = &*state;

        let channel = state.channels.get(&input.channel_id).ok_or_else(|| {
            DescribeThumbnailsErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::not_found_message("Channel", &input.channel_id)),
            })
        })?;

        // `PipelineId` is documented `Pipeline ID ("0" or "1")`, and WHICH of
        // those a channel has is fixed by its class (the `ChannelClass` doc: "A
        // standard channel has two encoding pipelines and a single pipeline
        // channel only has one"), so a value that is not one of that channel's
        // pipeline numbers names nothing whose thumbnails could be reported. The
        // member targets `__string`, so the boundary cannot refuse it and this
        // operation's own 400 does.
        //
        // Matched against the DECIMAL SPELLING of each of the channel's
        // pipelines rather than parsed, because the documented surface form is
        // the digit itself: a padded or signed spelling of the same number
        // ("01", "+1") is not a `PipelineId` the service hands out, so it is
        // refused like any other value that names no pipeline.
        let pipeline = (0..channel.pipeline_count())
            .find(|pipeline| pipeline.to_string() == input.pipeline_id)
            .ok_or_else(|| {
                bad_request(&format!(
                    "pipelineId names {}, which channel {} does not have: a {} channel has {} pipeline(s)",
                    input.pipeline_id,
                    channel.id,
                    channel.channel_class,
                    channel.pipeline_count()
                ))
            })?;

        // `ThumbnailType` targets `__string` on the wire too, so the boundary
        // admits any string; the kinds of thumbnail that exist are the modelled
        // `ThumbnailType` values (`smithy_query(query="shape",
        // shape="ThumbnailType")`), and a request asking for a kind outside them
        // asks for nothing this service has.
        if !THUMBNAIL_TYPE_VALUES.contains(&input.thumbnail_type.as_str()) {
            return Err(bad_request(&format!(
                "thumbnailType must be one of {}, got {}",
                THUMBNAIL_TYPE_VALUES.join(", "),
                input.thumbnail_type
            )));
        }

        // One `ThumbnailDetail` per pipeline reported, and this operation reads
        // exactly one pipeline: the latest frame that pipeline captured of the
        // requested kind, or no detail at all when it has none that is still
        // reportable. An EMPTY list, never an absent member: no thumbnail is an
        // empty set of thumbnails, not an unknown one.
        let thumbnail_details: Vec<ThumbnailDetail> = channel
            .latest_thumbnail(pipeline, &input.thumbnail_type)
            .map(crate::state::thumbnail_detail_output)
            .into_iter()
            .collect();

        Ok(DescribeThumbnailsOutput {
            thumbnail_details: Some(thumbnail_details),
        })
    }
}

/// A request this operation cannot read, as its modelled 400.
fn bad_request(message: &str) -> DescribeThumbnailsError {
    DescribeThumbnailsErrorModeled::BadRequestException(BadRequestException {
        message: Some(message.to_string()),
    })
    .into()
}

/// The engine this operation was built from is not the shared in-memory engine,
/// so there is no channel store to read a captured thumbnail out of. A modelled
/// server error, never a fresh engine (which would answer from an empty private
/// store).
fn unavailable() -> DescribeThumbnailsError {
    DescribeThumbnailsErrorModeled::InternalServerErrorException(InternalServerErrorException {
        message: Some(crate::state::NO_STORE.to_string()),
    })
    .into()
}
