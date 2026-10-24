// SPDX-License-Identifier: CC0-1.0

//! The upload plane: the target `CreateUploadUrl` publishes as `s3Url`.
//!
//! # Not a modelled operation
//!
//! `CreateUploadUrlResponse$s3Url` is documented as a pre-signed URL a caller uploads the
//! code file it wants to scan to, "with the required `requestHeaders` using any HTTP
//! client". That upload is a plain HTTP `PUT` against the published URL: it carries no
//! `X-Amz-Target`, names no modelled operation, and a pre-signed URL is followed with no
//! credential of its own. Nothing in the generated routing table can name it, so it is
//! reached through the two seams any emulated data plane uses:
//!
//! * [`crate::service::EmulatedService::can_handle`] also claims a request addressed to
//!   the upload host this service publishes ([`crate::state::is_upload_host`]), whose
//!   first label names no service, so the router does not answer `UnknownService` for a
//!   URL this service handed out;
//! * [`smithy_server_runtime::Service::override_routing_error`] serves it, and the router
//!   reaches that seam BEFORE authentication — which is what lets the published URL be
//!   followed unsigned, as a pre-signed URL is.
//!
//! # What is served
//!
//! `PUT /<account>/<scanName>/<codeArtifactId>` stores the request body as the code
//! resource of the artifact the path names, so the resource a later `CreateScan` is
//! started on is the archive the caller uploaded. A target holds ONE resource, so a
//! second upload replaces the first and an interrupted upload can simply be retried.
//!
//! Every other method on this path space is refused: the published URL describes an
//! upload, and answering a read on it would serve a resource the URL grants no read of.
//!
//! # The error envelope
//!
//! This plane is plain HTTPS rather than a modelled operation, so a refusal is answered
//! in its own small JSON envelope carrying the status, the code and a reason — never in
//! the service's restJson1 shapes, which belong to the modelled operations. Every message
//! is built from the constants below and carries nothing from the request, so a caller
//! cannot probe for the targets it was not handed.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

use crate::state::{UploadOutcome, UploadTarget};

/// The media type a refusal of this plane is stated in.
const ERROR_CONTENT_TYPE: &str = "application/json";

/// What this plane says to a method the published URL does not describe.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "This upload target is written with PUT";

/// What this plane says when the path names no upload target of this account — an
/// artifact this emulator never minted, one whose lifetime has ended, or a path that
/// crosses one artifact's identifier with another scan name or account. One sentence
/// carrying nothing from the request.
const NO_SUCH_TARGET_MESSAGE: &str = "The specified upload target does not exist";

/// What this plane says when the body could not be read at all. The router buffers every
/// request before dispatch, so a body it cannot hand back in memory is a failure of the
/// server, never a caller's malformed upload.
const UNREADABLE_BODY_MESSAGE: &str = "The server could not read the uploaded resource";

/// What this plane says when this service's store is not reachable.
const STORE_UNAVAILABLE_MESSAGE: &str = "The CodeGuru Security store is unavailable";

/// Serve one request to a published upload target.
///
/// The judgement about WHICH artifact the target names, and whether it may hold the
/// archive, is [`crate::state::Store::receive_upload`]'s: this function reads the request,
/// hands the bytes to the store under one lock, and renders the store's answer.
pub fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    target: &UploadTarget,
) -> RtResponse {
    if req.method() != "PUT" {
        return refusal(405, "MethodNotAllowed", METHOD_NOT_ALLOWED_MESSAGE);
    }
    let Some(body) = req.body().bytes() else {
        return refusal(500, "InternalFailure", UNREADABLE_BODY_MESSAGE);
    };
    let Some(state) = crate::state::service_state(engine) else {
        return refusal(500, "InternalServerException", STORE_UNAVAILABLE_MESSAGE);
    };
    // The BOUND precedes the COPY: an archive past what one target retains is refused
    // here, before the buffered body is copied into the store's own allocation, so an
    // over-large upload costs one length check rather than a second copy of itself.
    // `Store::receive_upload` checks the same ceiling again, because the store is the
    // authority on what it holds.
    if body.len() > crate::state::UPLOAD_CONTENT_MAX_BYTES {
        return too_large();
    }
    // "A set of key-value pairs that contain the required headers when uploading your
    // resource" (`CreateUploadUrlResponse$requestHeaders`) names the media type of the
    // archive, so the one the upload declared is recorded with it rather than assumed.
    let content_type = req
        .headers()
        .get(::http::header::CONTENT_TYPE.as_str())
        .map(str::to_string);
    let now = crate::state::now_millis();
    let outcome = state
        .lock()
        .receive_upload(target, body.to_vec(), content_type, now);
    match outcome {
        UploadOutcome::Stored => stored(),
        UploadOutcome::NoSuchTarget => refusal(404, "NoSuchKey", NO_SUCH_TARGET_MESSAGE),
        UploadOutcome::TooLarge => too_large(),
        UploadOutcome::StoreFull => refusal(
            507,
            "InsufficientStorage",
            &format!(
                "The upload targets of this account already hold the maximum of {max} \
                 bytes this emulator retains; scan a resource or wait for a minted target \
                 to expire",
                max = crate::state::UPLOAD_CONTENT_TOTAL_MAX_BYTES,
            ),
        ),
    }
}

/// The answer to an archive larger than one target retains
/// ([`crate::state::UPLOAD_CONTENT_MAX_BYTES`]).
fn too_large() -> RtResponse {
    refusal(
        400,
        "EntityTooLarge",
        &format!(
            "This upload target retains at most {max} bytes",
            max = crate::state::UPLOAD_CONTENT_MAX_BYTES,
        ),
    )
}

/// The answer to an upload this plane stored: the bytes are held, and there is nothing to
/// report back about them.
fn stored() -> RtResponse {
    let mut resp = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::empty(),
    );
    resp.headers_mut().insert("content-length", "0");
    resp
}

/// One upload-plane refusal: the status, the code in `x-amzn-errortype`, and a JSON body
/// carrying the reason.
fn refusal(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "code": code, "message": message }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", ERROR_CONTENT_TYPE);
    resp.headers_mut().insert("x-amzn-errortype", code.to_string());
    resp
}
