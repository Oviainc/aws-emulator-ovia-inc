// SPDX-License-Identifier: CC0-1.0

//! @op RecognizeCelebrities
//!
//! ## What this handler does
//!
//! `RecognizeCelebrities` "lists the recognized celebrities in the CelebrityFaces array and
//! any unrecognized faces in the UnrecognizedFaces array", so one answer is a PARTITION of
//! the faces the submitted image carries: every face measured is reported on one side of it
//! or the other, and none is left out of the answer.
//!
//! The faces are MEASURED off the decoded pixels by [`crate::state::face_detection`] — the
//! same geometric detector `DetectFaces` reports and `CompareFaces` compares — over the
//! image's own bytes, which are the request's inline `Bytes` or the object this emulator's
//! Amazon S3 holds at the `S3Object` it named ([`crate::state::image_source`]). One answer
//! covers the [`MAX_REPORTED_FACES`] largest of them, because the operation documents that
//! it "returns the 64 largest faces in the image" and "doesn't return celebrities whose
//! faces aren't among the largest 64 faces".
//!
//! WHICH SIDE of the partition a face falls on is an IDENTITY: AWS answers it against its
//! own catalogue of celebrities, and neither that catalogue nor any trained recogniser
//! exists in this process — nothing here maps a face to the person it belongs to (see
//! [`crate::state`]). So no face is reported as a celebrity, and no face is DROPPED for
//! that: each one is reported as an unrecognized face, which is where the operation itself
//! puts a face it does not recognise, carrying the members the detector measured (a
//! `BoundingBox`, a `Confidence`, `Landmarks`, a `Pose.Roll` and a `Quality`) and leaving
//! `Emotions` and `Smile` — a classifier's judgement — absent. An image carrying no face at
//! all is answered with both lists empty, which is what was measured, and a refusal is
//! reserved for an image this service could not read at all.
//!
//! It PERSISTS NOTHING and reads no stored resource: "Amazon Rekognition doesn't retain
//! information about which images a celebrity has been recognized in. Your application must
//! store this information" (the operation's own documentation, below). The whole answer is a
//! function of the submitted image, so two identical calls answer identically without either
//! having changed anything, and the store this service keeps for collections is neither read
//! nor written here.
//!
//! `OrientationCorrection` is absent because the member's own documentation says it always
//! is: "any returned values for this field included in an API response will always be NULL".
//!
//! ---
//!
//! # Modeled contract for `RecognizeCelebrities`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json")
//! ```
//!
//! Returns an array of celebrities recognized in the input image. For more information, see
//! Recognizing celebrities in the Amazon Rekognition Developer Guide. RecognizeCelebrities
//! returns the 64 largest faces in the image. It lists the recognized celebrities in the
//! CelebrityFaces array and any unrecognized faces in the UnrecognizedFaces array.
//! RecognizeCelebrities doesn't return celebrities whose faces aren't among the largest 64
//! faces in the image. For each celebrity recognized, RecognizeCelebrities returns a
//! Celebrity object. The Celebrity object contains the celebrity name, ID, URL links to
//! additional information, match confidence, and a ComparedFace object that you can use to
//! locate the celebrity's face on the image. Amazon Rekognition doesn't retain information
//! about which images a celebrity has been recognized in. Your application must store this
//! information and use the Celebrity ID property as a unique identifier for the celebrity.
//! If you don't store the celebrity name or additional information URLs returned by
//! RecognizeCelebrities, you will need the ID to identify the celebrity in a call to the
//! GetCelebrityInfo operation. You pass the input image either as base64-encoded image
//! bytes or as a reference to an image in an Amazon S3 bucket. If you use the AWS CLI to
//! call Amazon Rekognition operations, passing image bytes is not supported. The image must
//! be either a PNG or JPEG formatted file. For an example, see Recognizing celebrities in
//! an image in the Amazon Rekognition Developer Guide. This operation requires permissions
//! to perform the rekognition:RecognizeCelebrities operation.
//!
//! ## Input
//!
//! * `Image` (Image) **required** — The input image as base64-encoded bytes or an S3 object.
//!   If you use the AWS CLI to call Amazon Rekognition operations, passing base64-encoded
//!   image bytes is not supported. If you are using an AWS SDK to call Amazon Rekognition,
//!   you might not need to base64-encode image bytes passed using the Bytes field. For more
//!   information, see Images in the Amazon Rekognition developer guide.
//!
//! ## Output
//!
//! * `CelebrityFaces` (CelebrityList) — Details about each celebrity found in the image.
//!   Amazon Rekognition can detect a maximum of 64 celebrities in an image. Each celebrity
//!   object includes the following attributes: Face, Confidence, Emotions, Landmarks, Pose,
//!   Quality, Smile, Id, KnownGender, MatchConfidence, Name, Urls.
//! * `OrientationCorrection` (OrientationCorrection, one of: ROTATE_0 | ROTATE_180 |
//!   ROTATE_270 | ROTATE_90) — Support for estimating image orientation using the the
//!   OrientationCorrection field has ceased as of August 2021. Any returned values for this
//!   field included in an API response will always be NULL. The orientation of the input
//!   image (counterclockwise direction). If your application displays the image, you can use
//!   this value to correct the orientation. The bounding box coordinates returned in
//!   CelebrityFaces and UnrecognizedFaces represent face locations before the image
//!   orientation is corrected. If the input image is in .jpeg format, it might contain
//!   exchangeable image (Exif) metadata that includes the image's orientation. If so, and the
//!   Exif metadata for the input image populates the …
//! * `UnrecognizedFaces` (ComparedFaceList) — Details about each unrecognized face in the
//!   image.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — You are not authorized to perform the action.
//! * `ImageTooLargeException` (HTTP 400) — The input image size exceeds the allowed limit. If
//!   you are calling DetectProtectiveEquipment, the image size or resolution exceeds the
//!   allowed limit. For more information, see Guidelines and quotas in Amazon Rekognition in
//!   the Amazon Rekognition Developer Guide.
//! * `InternalServerError` (HTTP 500) — Amazon Rekognition experienced a service issue. Try
//!   your call again.
//! * `InvalidImageFormatException` (HTTP 400) — The provided image format is not supported.
//! * `InvalidParameterException` (HTTP 400) — Input parameter violated a constraint. Validate
//!   your parameter before calling the API operation again.
//! * `InvalidS3ObjectException` (HTTP 400) — Amazon Rekognition is unable to access the S3
//!   object specified in the request.
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The number of requests exceeded
//!   your throughput limit. If you want to increase this limit, contact Amazon Rekognition.
//! * `ThrottlingException` (HTTP 500) — Amazon Rekognition is temporarily unable to process
//!   the request. Try your call again.

#[allow(unused_imports)]
use super::types::*;

// A reported face is built by the one conversion every operation that reports a
// `ComparedFace` shares, so two operations cannot drift apart over what a face carries.
use crate::state::face_detection::compared_face;

// Every modelled error this handler answers with is built through this trait, so each
// one carries its `Code` member beside its `Message`.
use crate::state::modeled_error::ModeledError;

/// The most faces one answer reports: "RecognizeCelebrities returns the 64 largest faces in
/// the image", and "doesn't return celebrities whose faces aren't among the largest 64 faces
/// in the image" (the operation's own documentation).
pub const MAX_REPORTED_FACES: usize = 64;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RecognizeCelebritiesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-rekognition-rekognitionservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RecognizeCelebritiesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.rekognition#RecognizeCelebrities")]
#[derive(Debug)]
pub enum RecognizeCelebritiesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ImageTooLargeException(ImageTooLargeException),
    InternalServerError(InternalServerError),
    InvalidImageFormatException(InvalidImageFormatException),
    InvalidParameterException(InvalidParameterException),
    InvalidS3ObjectException(InvalidS3ObjectException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::recognize_celebrities::RecognizeCelebritiesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RecognizeCelebritiesError = crate::scaffolded::Scaffolded<RecognizeCelebritiesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RecognizeCelebritiesErrorModeled> for RecognizeCelebritiesError {
    fn from(modeled: RecognizeCelebritiesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RecognizeCelebrities` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RecognizeCelebrities<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RecognizeCelebrities<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RecognizeCelebrities<Eng>
{
    type Input = RecognizeCelebritiesInput;
    type Output = RecognizeCelebritiesOutput;
    type Error = RecognizeCelebritiesError;

    const NAME: &'static str = "RecognizeCelebrities";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RECOGNIZE_CELEBRITIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RECOGNIZE_CELEBRITIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Nothing is persisted — "Amazon Rekognition doesn't retain information about which
        // images a celebrity has been recognized in" — and the whole answer is measured off
        // the image the request submitted, so there is no stored resource to read or write
        // and the engine this operation was constructed with goes unused. `ctx` is what an
        // S3-sourced read of the image goes through.
        let _ = &self.engine;
        let faces = crate::state::face_detection::faces_in_submitted_image(ctx, &input.image)
            .await
            .map_err(unreadable_error)?;
        // "RecognizeCelebrities returns the 64 largest faces in the image": the detector
        // reports the faces it found largest first, so the ones past the cap are the
        // smallest of them.
        let unrecognized: Vec<ComparedFace> = faces
            .iter()
            .take(MAX_REPORTED_FACES)
            .map(compared_face)
            .collect();
        Ok(RecognizeCelebritiesOutput {
            // No face is reported as a celebrity: deciding that is an identity, nothing in
            // this process holds the identity of a face, and a name nothing established is
            // not reported. The faces themselves are not lost by it — each one is on the
            // unrecognized side of the partition below.
            celebrity_faces: Some(Vec::new()),
            // "Details about each unrecognized face in the image", which is every face this
            // answer covers.
            unrecognized_faces: Some(unrecognized),
            // "Any returned values for this field included in an API response will always be
            // NULL."
            orientation_correction: None,
        })
    }
}

/// The modelled answer for an image whose faces could not be detected: one modelled error
/// per reason, each carrying the reason's own message. The reason type and its wording are
/// shared (`crate::state::image_source`); the mapping is written per operation because each
/// operation's error enum is its own type, generated from the errors the model declares for
/// it, so there is no one enum for a shared function to return.
fn unreadable_error(
    reason: crate::state::image_source::UnreadableImage,
) -> RecognizeCelebritiesError {
    use crate::state::image_source::UnreadableImage;

    let message = reason.message().to_string();
    match reason {
        UnreadableImage::InvalidFormat(_) => {
            RecognizeCelebritiesErrorModeled::InvalidImageFormatException(
                InvalidImageFormatException::with_message(message),
            )
        }
        UnreadableImage::TooLarge(_) => RecognizeCelebritiesErrorModeled::ImageTooLargeException(
            ImageTooLargeException::with_message(message),
        ),
        UnreadableImage::S3Unreadable(_) => {
            RecognizeCelebritiesErrorModeled::InvalidS3ObjectException(
                InvalidS3ObjectException::with_message(message),
            )
        }
        UnreadableImage::InvalidParameter(_) => {
            RecognizeCelebritiesErrorModeled::InvalidParameterException(
                InvalidParameterException::with_message(message),
            )
        }
        // Nothing was established about the image itself, so this is the server fault it
        // is rather than anything about the request.
        UnreadableImage::NotAnalysed(_) => RecognizeCelebritiesErrorModeled::InternalServerError(
            InternalServerError::with_message(message),
        ),
    }
    .into()
}
