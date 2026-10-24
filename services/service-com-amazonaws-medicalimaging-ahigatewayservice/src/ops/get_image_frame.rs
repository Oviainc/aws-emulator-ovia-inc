// SPDX-License-Identifier: CC0-1.0

//! @op GetImageFrame
//!
//! Serves one image frame's PIXEL DATA from persisted state: the frame the request's
//! `imageFrameInformation.imageFrameId` names, of the image set the data store holds.
//! The blob is the bytes as they were stored — nothing is transcoded, and nothing is
//! synthesised for a frame no pixel data was ever delivered for, which is 404
//! `ResourceNotFoundException` like any other absent resource.
//!
//! `contentType` is COMPUTED from the transfer syntax the pixel data is stored in,
//! through the catalog the response member documents
//! ([`crate::state::image_frame_content_type`]): a stored UID the documentation lists
//! gets the content type it lists, a frame whose instance declared no transfer syntax
//! gets `image/jph`, and any other UID the documented default,
//! `application/octet-stream`.
//!
//! A REPORTING operation (`@readonly` in the model), so it writes nothing of its own.
//! Like every handler it calls
//! [`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`]
//! first, which materialises only what the CLOCK already owed at that instant — the
//! completion of a copy, which is what delivers pixel data to the destination image
//! set it was writing.
//!
//! ---
//!
//! # Modeled contract for `GetImageFrame`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Get an image frame (pixel data) for an image set.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `imageFrameInformation` (ImageFrameInformation) **required** — Information about the
//!   image frame (pixel data) identifier.
//! * `imageSetId` (ImageSetId, pattern ^[0-9a-z]{32}$) **required** — The image set
//!   identifier.
//!
//! ## Output
//!
//! * `contentType` (String) — The format in which the image frame information is returned to
//!   the customer. Default is application/octet-stream. If the stored transfer syntax is
//!   1.2.840.10008.1.2.1, the returned contentType is application/octet-stream. If the stored
//!   transfer syntax is 1.2.840.10008.1.2.4.50, the returned contentType is image/jpeg. If
//!   the stored transfer syntax is 1.2.840.10008.1.2.4.91, the returned contentType is
//!   image/j2c. If the stored transfer syntax is MPEG2, 1.2.840.10008.1.2.4.100,
//!   1.2.840.10008.1.2.4.100.1, 1.2.840.10008.1.2.4.101, or 1.2.840.10008.1.2.4.101.1, the
//!   returned contentType is video/mpeg. If the stored transfer syntax is MPEG-4 AVC/H.264,
//!   UID 1.2.840.10008.1.2.4.102, …
//! * `imageFrameBlob` (PayloadBlob) **required** — The blob containing the aggregated image
//!   frame information.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `BadRequestException` (HTTP 400) — The request is invalid or malformed.
//! * `ConflictException` (HTTP 409) — Updating or deleting a resource can cause an
//!   inconsistent state.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `NotAcceptableException` (HTTP 406) — The request content type or accept header is not
//!   supported.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resource which does
//!   not exist.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetImageFrameError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetImageFrameError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#GetImageFrame")]
#[derive(Debug)]
pub enum GetImageFrameErrorModeled {
    AccessDeniedException(AccessDeniedException),
    BadRequestException(BadRequestException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    NotAcceptableException(NotAcceptableException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_image_frame::GetImageFrameError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetImageFrameError = crate::scaffolded::Scaffolded<GetImageFrameErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetImageFrameErrorModeled> for GetImageFrameError {
    fn from(modeled: GetImageFrameErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetImageFrame` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetImageFrame<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetImageFrame<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetImageFrame<Eng>
{
    type Input = GetImageFrameInput;
    type Output = GetImageFrameOutput;
    type Error = GetImageFrameError;

    const NAME: &'static str = "GetImageFrame";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IMAGE_FRAME_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IMAGE_FRAME_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetImageFrameErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let mut store = state.lock();
        store.advance_clock(now);

        let datastore = store.datastores.get(&input.datastore_id).ok_or_else(|| {
            GetImageFrameErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        let image_set = datastore.image_sets.get(&input.image_set_id).ok_or_else(|| {
            GetImageFrameErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        // A DELETED image set's pixel data went with it, so what is absent is the image
        // set itself, not one frame of it.
        if image_set.latest_version.image_set_state == crate::state::ImageSetState::Deleted {
            return Err(GetImageFrameErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::IMAGE_SET_NOT_FOUND_MESSAGE.to_string(),
                },
            )
            .into());
        }

        // Pixel data is served, never invented: a frame identifier this image set holds
        // nothing under is an absent resource.
        let frame = image_set
            .image_frame(&input.image_frame_information.image_frame_id)
            .ok_or_else(|| {
                GetImageFrameErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: crate::state::IMAGE_FRAME_NOT_FOUND_MESSAGE.to_string(),
                })
            })?;

        Ok(GetImageFrameOutput {
            image_frame_blob: frame.pixel_data.clone(),
            content_type: Some(
                crate::state::image_frame_content_type(frame.transfer_syntax_uid.as_deref())
                    .to_string(),
            ),
        })
    }
}
