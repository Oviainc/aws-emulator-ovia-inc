// SPDX-License-Identifier: CC0-1.0

//! The CLASS of the fault every error body of this service names, in its `Type` member.
//!
//! # The rule
//!
//! Every error shape this service's model declares carries a `Type` member — "The
//! exception type"
//! (`smithy_query(query="shape",shape="InvalidParameterValueException,ResourceNotFoundException,ResourceConflictException,ServiceException,TooManyRequestsException",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`)
//! — and what it states is whose fault the failure is: the request's, or the service's.
//! The model already classifies that for every one of them: an `@error("client")` shape
//! carries a 4xx `@httpError` and an `@error("server")` shape a 5xx one, so the status
//! class of a failure IS its fault class. So an error body of this service reads
//!
//! ```json
//! HTTP/1.1 400 Bad Request
//! content-type: application/json
//!
//! {"Type":"User","message":"One or more of your tags had a problem, …"}
//! ```
//!
//! and a failure inside the service names [`SERVICE_FAULT`] there instead.
//!
//! # Why it is stated here
//!
//! The class is a property of the failure, not of the call that hit it, so no raise site
//! carries it: a handler builds the modelled error shape with its message and leaves the
//! member unset, and the JSON codec writes nothing at all for an unset member — so every
//! refusal this service answered went out with no `Type` member, whatever its status. The
//! error bodies the FRAMEWORK renders (the pre-handler refusal of a value outside a
//! declared bound, and the scaffold's 501) are built from synthetic shapes that do not
//! declare the member at all, so they could not carry it either.
//!
//! This module therefore names the class on the error bodies of this service, wherever
//! they were produced: one place that sees a whole rendered response, rather than the
//! hundreds of raise sites, so no later handler can forget it. The status, the error code,
//! the message, every other member, every header and the whole success path stay with the
//! renderer that owns them; a body that already names a class keeps the one it names, and
//! the body is re-serialized through `serde_json`, so its member names and values survive
//! while its byte-level layout is that serializer's.
//!
//! The same mechanism, for the same kind of reason, is stated in
//! `service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706`
//! (`src/service/error_message_member.rs`) and
//! `service-com-amazonaws-connecthealth-connecthealth` (`src/service/error_detail.rs`);
//! only the member and the value differ. The structural home for it is the error renderer
//! itself, which is in the agent-immutable `vendor/` tree, so this service states it at the
//! seams that can see a whole rendered response: the operation wrapper every generated
//! modelled operation is served behind, and the one hand-written entry served outside that
//! wrapper (see below).
//!
//! # What it does NOT reach
//!
//! That wrapper is this service's GENERATED modelled-operation surface, so two answers are
//! outside it and keep the envelope their own renderer wrote:
//!
//! * the DOWNLOAD plane (`crate::service::Downloads`), which serves a deployment package
//!   or a layer archive rather than a modelled operation — its refusals are object-fetch
//!   refusals, not Lambda API errors, so a Lambda error shape's member has no place in
//!   them;
//! * the routing-error and framework-error paths, which answer a request that named no
//!   modelled operation or failed at the transport, so no error shape of this model is
//!   what they render.
//!
//! `InvokeWithResponseStream` is a modelled operation the wrapper does not serve — its
//! output binds a `@streaming` union, so it is served by its own hand-written entry — and
//! its errors are error shapes of this model like any other, so that entry names the class
//! on them through [`with_fault_class`] itself
//! (`crate::ops::invoke_with_response_stream`).
//!
//! And one answer inside that surface is held back from it deliberately: the OPAQUE
//! refusal of a service-minted token (`crate::service::opaque_members`), whose recorded
//! body is its fixed sentence and no other member at all.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

/// The class a failure the CALLER can fix names: a request this service refused.
pub const USER_FAULT: &str = "User";

/// The class a failure inside the service names.
pub const SERVICE_FAULT: &str = "Service";

/// The error-body member the class travels in, as every error shape of this model spells
/// it.
const FAULT_CLASS_MEMBER: &str = "Type";

/// The response header a renderer uses to advertise its body's length.
const CONTENT_LENGTH: &str = "content-length";

/// The class of fault a response at `status` reports, or `None` for a status that reports
/// no failure at all.
///
/// The model's own classification: a client error is answered at a 4xx status and a server
/// error at a 5xx one, so the status class is what says which of the two a body names.
#[must_use]
pub fn fault_class(status: u16) -> Option<&'static str> {
    match status {
        400..=499 => Some(USER_FAULT),
        500..=599 => Some(SERVICE_FAULT),
        _ => None,
    }
}

/// The error body with `class` named in its [`FAULT_CLASS_MEMBER`], or `None` when there
/// is nothing to name it on: the body is not a JSON object, or it already names one.
///
/// Idempotent, and non-destructive: a body that already carries the member keeps the value
/// it carries, so a handler that states the class itself always wins.
fn naming_the_fault_class(body: &[u8], class: &str) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    if object.contains_key(FAULT_CLASS_MEMBER) {
        return None;
    }
    object.insert(
        FAULT_CLASS_MEMBER.to_string(),
        Value::String(class.to_string()),
    );
    serde_json::to_vec(&document).ok()
}

/// The response as this service answers it: an error body naming the class of its fault.
///
/// A response that reports no failure, or whose body is not a JSON object (a streamed
/// body, an empty one, a download the service served), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them.
#[must_use]
pub fn with_fault_class(mut resp: RtResponse) -> RtResponse {
    let Some(class) = fault_class(resp.status().as_u16()) else {
        return resp;
    };
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match naming_the_fault_class(bytes, class) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get(CONTENT_LENGTH).is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the member was added, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert(CONTENT_LENGTH, length)
            .is_err()
        {
            resp.headers_mut().remove(CONTENT_LENGTH);
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response at `status` carrying `body`.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("the rewritten body is JSON")
    }

    /// The class named at each status this service's error shapes are answered at — every
    /// `@httpError` the model pins, one row each, plus the statuses no error of this
    /// service carries, so the rule is a property of the status class rather than of the
    /// one status the issue recorded.
    #[test]
    fn every_failure_status_names_its_own_fault_class() {
        let cases: &[(u16, Option<&str>)] = &[
            // The modelled client errors: 400 InvalidParameterValueException,
            // 404 ResourceNotFoundException, 409 ResourceConflictException,
            // 412 PreconditionFailedException, 429 TooManyRequestsException.
            (400, Some(USER_FAULT)),
            (403, Some(USER_FAULT)),
            (404, Some(USER_FAULT)),
            (409, Some(USER_FAULT)),
            (412, Some(USER_FAULT)),
            (429, Some(USER_FAULT)),
            // The modelled server errors: 500 ServiceException, 502 the KMS and mount
            // failures, and the scaffold's own 501.
            (500, Some(SERVICE_FAULT)),
            (501, Some(SERVICE_FAULT)),
            (502, Some(SERVICE_FAULT)),
            // Not failures at all: every success this service answers.
            (200, None),
            (201, None),
            (202, None),
            (204, None),
            (399, None),
        ];
        for (status, expected) in cases {
            let actual = fault_class(*status);
            assert_eq!(
                actual, *expected,
                "case {status}: expected the fault class {expected:?}, got {actual:?}"
            );
        }
    }

    /// One row per class of error body this service can answer with — a modelled
    /// exception at each status, and the two bodies the framework renders itself — so the
    /// member is a property of the error envelope rather than of one refusal.
    #[test]
    fn every_error_body_names_the_class_of_its_fault() {
        let cases: &[(u16, &str, &str, &str)] = &[
            (
                400,
                "InvalidParameterValueException",
                "The qualifier '1' in 'functionName' names a version or alias",
                USER_FAULT,
            ),
            (
                404,
                "ResourceNotFoundException",
                "Function not found: arn:aws:lambda:us-east-1:000000000000:function:absent",
                USER_FAULT,
            ),
            (
                409,
                "ResourceConflictException",
                "Function already exist: probe",
                USER_FAULT,
            ),
            (
                429,
                "TooManyRequestsException",
                "Rate exceeded",
                USER_FAULT,
            ),
            (
                500,
                "ServiceException",
                "Lambda state is not reachable on this engine",
                SERVICE_FAULT,
            ),
            // The framework's own signals, rendered by the boundary validator and the
            // scaffold channel rather than by a handler.
            (
                400,
                "ValidationException",
                "1 validation error detected: Value 'bad name!' at 'functionName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern",
                USER_FAULT,
            ),
            (
                501,
                "NotImplemented",
                "operation Invoke is scaffolded but not implemented in this emulator",
                SERVICE_FAULT,
            ),
        ];
        for (status, code, text, expected) in cases {
            let body = format!(r#"{{"__type":"{code}","message":"{text}"}}"#);
            let answered = with_fault_class(response(*status, &body));
            let document = decoded(answered.body().bytes().expect("an in-memory body"));
            assert_eq!(
                document.get(FAULT_CLASS_MEMBER).and_then(Value::as_str),
                Some(*expected),
                "case {code:?} at {status}: expected the class {expected:?}, got {document}"
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some(*text),
                "case {code:?} at {status}: the message must survive, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(*code),
                "case {code:?} at {status}: the discriminator must survive, got {document}"
            );
        }
    }

    /// Nothing to name a class on: a body that already names one keeps it, a body that is
    /// not an object and one that is not JSON at all are left alone, and so is every
    /// success.
    #[test]
    fn a_body_that_names_no_class_of_its_own_is_left_alone() {
        let already = r#"{"__type":"ResourceInUseException","Type":"Service","Message":"busy"}"#;
        let answered = with_fault_class(response(400, already));
        assert_eq!(
            answered.body().bytes().expect("an in-memory body"),
            already.as_bytes(),
            "a body that names its own class must be byte-identical"
        );

        for (case, status, body) in [
            ("not an object", 400, r#"["InvalidParameterValueException"]"#),
            ("not JSON", 404, "<Error/>"),
            ("an empty body", 400, ""),
            (
                "a success",
                200,
                r#"{"ReservedConcurrentExecutions":null}"#,
            ),
            ("an empty success", 204, ""),
        ] {
            let answered = with_fault_class(response(status, body));
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: expected the body untouched"
            );
        }
    }

    /// A length a renderer advertised is corrected to the body it now describes.
    #[test]
    fn an_advertised_length_describes_the_body_that_names_the_class() {
        let body = r#"{"__type":"InvalidParameterValueException","message":"Invalid parameter."}"#;
        let mut resp = response(400, body);
        resp.headers_mut()
            .insert(CONTENT_LENGTH, body.len().to_string());
        let answered = with_fault_class(resp);
        let bytes = answered.body().bytes().expect("an in-memory body").to_vec();
        assert_eq!(
            answered.headers().get(CONTENT_LENGTH),
            Some(bytes.len().to_string().as_str()),
            "expected the advertised length to describe the rewritten body"
        );
    }
}
