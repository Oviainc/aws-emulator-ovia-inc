// SPDX-License-Identifier: CC0-1.0

//! How this service names the exceptions it answers on the wire: it does not.
//!
//! AmplifyUIBuilder answers a refused call with the status and a `message`, and with NO
//! error-type discriminator — neither the `x-amzn-errortype` header restJson1 resolves an
//! error shape from, nor a `__type` / `code` / `Code` member in the body:
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//!
//! {"message":"The item specified was not found"}
//! ```
//!
//! A client therefore classifies the failure by STATUS: botocore, finding no code in
//! either place, reports `Error.Code` as the bare status string (`"404"`, `"400"`,
//! `"409"`) rather than a shape name, and the SDK's modelled-exception classes never
//! match. The emulator's framework renderer, by contrast, writes the discriminator in both
//! places for every modelled error, so the same refusal arrives typed — and a client
//! written against either one mishandles the other.
//!
//! This module removes the discriminator from the error responses of this service's own
//! modelled exceptions. Everything else about the response is left exactly as the
//! framework rendered it: the status, the `message`, every other member and every other
//! header, and the whole success path.
//!
//! What it deliberately leaves typed is a code that is NOT one of this service's modelled
//! exceptions (see [`MODELLED_EXCEPTIONS`]): the scaffold's `NotImplemented` (501) and the
//! framework's `ValidationException` (400) are the emulator's own signals for "nobody has
//! written this operation yet" and "this request was refused before any handler ran" —
//! AmplifyUIBuilder has no such errors, so dressing them as one of its refusals would hide
//! what they report.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Every exception shape this service's model declares — the codes an AmplifyUIBuilder
/// error body can name, and so the ones whose discriminator is removed here.
///
/// Source: the exception shapes of this service's model, which are the `errors` entries of
/// its operations, e.g.
/// `smithy_query(query="operation",operation="CreateComponent,GetComponent,DeleteComponent,UpdateComponent,PutMetadataFlag",model_path="services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/smithy-model/com-amazonaws-amplifyuibuilder-amplifyuibuilder.json")`
/// — `InternalServerException` (500), `InvalidParameterException` (400),
/// `ResourceConflictException` (409), `ResourceNotFoundException` (404),
/// `ServiceQuotaExceededException` (402), `ThrottlingException` (429) and
/// `UnauthorizedException` (401).
const MODELLED_EXCEPTIONS: [&str; 7] = [
    "InternalServerException",
    "InvalidParameterException",
    "ResourceConflictException",
    "ResourceNotFoundException",
    "ServiceQuotaExceededException",
    "ThrottlingException",
    "UnauthorizedException",
];

/// The response header restJson1 resolves an error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The body members that carry an error-type discriminator on the JSON protocols, in the
/// order botocore consults them: `__type` is what this emulator's restJson1 renderer
/// writes, and `code` / `Code` are the spellings the same parser also accepts.
const DISCRIMINATOR_MEMBERS: [&str; 3] = ["__type", "code", "Code"];

/// The bare shape name a discriminator value names.
///
/// A value may arrive namespaced (`com.amazonaws.amplifyuibuilder#ResourceNotFoundException`)
/// or with a trailing locator (`ResourceNotFoundException:http://…`); neither is part of
/// the code.
fn shape_name(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace.split(':').next().unwrap_or(after_namespace)
}

/// Whether `raw` names one of this service's own modelled exceptions.
fn is_modelled_exception(raw: &str) -> bool {
    MODELLED_EXCEPTIONS.contains(&shape_name(raw))
}

/// The error body with its discriminator removed, or `None` when there is nothing to
/// remove: the body is not a JSON object, it carries no discriminator member, or the code
/// it names is not one of this service's modelled exceptions.
///
/// `header_code` only classifies — it decides whether a code found in the BODY belongs to
/// this service when the body's own value is ambiguous — so a body carrying no
/// discriminator member is left to its renderer byte for byte rather than re-serialized.
fn without_discriminator(body: &[u8], header_code: Option<&str>) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let named = DISCRIMINATOR_MEMBERS
        .iter()
        .find_map(|member| object.get(*member).and_then(Value::as_str))
        .map(str::to_string)?;
    if !is_modelled_exception(&named) && header_code.is_none_or(|code| !is_modelled_exception(code))
    {
        return None;
    }
    for member in DISCRIMINATOR_MEMBERS {
        object.remove(member);
    }
    serde_json::to_vec(&document).ok()
}

/// The response as AmplifyUIBuilder answers it: an error body of one of this service's
/// modelled exceptions with no error-type discriminator in it, and no `x-amzn-errortype`
/// header beside it.
///
/// A response that is not a failure, or whose error body this service does not own (see
/// [`without_discriminator`]), is returned untouched — status, headers and every byte of
/// the body exactly as the renderer wrote them.
pub fn untyped(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    // The header and the body are two independent places a client reads the code from, so
    // each is removed on its own: a failure whose body is absent, streamed or not JSON
    // still loses the header when the header names one of this service's exceptions.
    let header_code = resp.headers().get(ERROR_TYPE_HEADER).map(str::to_string);
    if header_code
        .as_deref()
        .is_some_and(is_modelled_exception)
    {
        resp.headers_mut().remove(ERROR_TYPE_HEADER);
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match without_discriminator(bytes, header_code.as_deref()) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert("content-length", length);
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// The operation the generated routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &'s RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`UntypedErrorOperation`].
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then strips the error-type discriminator from the response.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// message and member it answers with; this wrapper owns only whether the error names its
/// shape on the wire.
pub struct UntypedErrorOperation;

impl ErasedOperation for UntypedErrorOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some(op) => untyped(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("the rewritten body is JSON")
    }

    /// One row per exception this service models, at the status the model declares for it,
    /// in each spelling a discriminator can arrive in: the code goes, the message stays.
    #[test]
    fn every_modelled_exception_loses_its_discriminator() {
        for (code, status) in [
            ("InternalServerException", 500u16),
            ("InvalidParameterException", 400),
            ("ResourceConflictException", 409),
            ("ResourceNotFoundException", 404),
            ("ServiceQuotaExceededException", 402),
            ("ThrottlingException", 429),
            ("UnauthorizedException", 401),
        ] {
            for member in DISCRIMINATOR_MEMBERS {
                let body = format!(r#"{{"{member}":"{code}","Message":"no"}}"#);
                let rewritten = without_discriminator(body.as_bytes(), None).unwrap_or_else(|| {
                    panic!("case {code:?}/{member:?} at {status}: expected a rewrite")
                });
                let document = decoded(&rewritten);
                assert_eq!(
                    document.get(member),
                    None,
                    "case {code:?}/{member:?}: the discriminator must be gone, got {document:?}"
                );
                assert_eq!(
                    document.get("Message").and_then(Value::as_str),
                    Some("no"),
                    "case {code:?}/{member:?}: the message must survive, got {document:?}"
                );
            }
        }
    }

    /// The code is recognised whichever form it arrives in, including from the header
    /// alone — and then the header is what the response drops.
    #[test]
    fn the_code_is_recognised_in_every_form_it_arrives_in() {
        for (label, body, header) in [
            ("bare __type", r#"{"__type":"ResourceNotFoundException"}"#, None),
            (
                "namespaced __type",
                r#"{"__type":"com.amazonaws.amplifyuibuilder#ResourceNotFoundException"}"#,
                None,
            ),
            (
                "__type with a locator",
                r#"{"__type":"ResourceNotFoundException:http://internal.amazon.com/coral/"}"#,
                None,
            ),
            (
                "header only",
                r#"{"Message":"no"}"#,
                Some("ResourceNotFoundException"),
            ),
        ] {
            let rewritten = without_discriminator(body.as_bytes(), header);
            if label == "header only" {
                // Nothing to remove from the body: it carried no discriminator member, so
                // it is left to its renderer byte for byte (the header is dropped by
                // `untyped`, which the case below covers).
                assert!(
                    rewritten.is_none(),
                    "case {label:?}: a body with no discriminator member must not be \
                     rewritten"
                );
                continue;
            }
            let rewritten =
                rewritten.unwrap_or_else(|| panic!("case {label:?}: expected a rewrite"));
            let document = decoded(&rewritten);
            assert!(
                DISCRIMINATOR_MEMBERS
                    .iter()
                    .all(|member| document.get(*member).is_none()),
                "case {label:?}: no discriminator may survive, got {document:?}"
            );
        }
    }

    /// Nothing to strip: a code this service does not model — the scaffold's 501 and the
    /// framework's refusal — a body carrying no code at all, and a body that is not a JSON
    /// object are all left alone.
    #[test]
    fn a_body_this_service_does_not_own_is_left_alone() {
        for (label, body) in [
            ("the scaffold's 501", r#"{"__type":"NotImplemented","Message":"no"}"#),
            (
                "a framework refusal",
                r#"{"__type":"ValidationException","Message":"no"}"#,
            ),
            ("no code at all", r#"{"Message":"no"}"#),
            ("not an object", r#"["ResourceNotFoundException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                without_discriminator(body.as_bytes(), None).is_none(),
                "case {label:?}: expected no rewrite"
            );
        }
    }

    /// End to end on a response: the header goes with the body member, the status stays,
    /// and a success is byte-identical.
    #[test]
    fn the_response_keeps_its_status_and_loses_only_the_discriminator() {
        let body = r#"{"__type":"ResourceNotFoundException","Message":"no"}"#;
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(404u16).expect("404"),
            SdkBody::from(body),
        );
        resp.headers_mut()
            .insert(ERROR_TYPE_HEADER, "ResourceNotFoundException");
        let answered = untyped(resp);
        assert_eq!(
            answered.status().as_u16(),
            404,
            "the status must not move, got {}",
            answered.status().as_u16()
        );
        assert_eq!(
            answered.headers().get(ERROR_TYPE_HEADER),
            None,
            "the error-type header must be gone, got {:?}",
            answered.headers().get(ERROR_TYPE_HEADER)
        );
        let document = decoded(answered.body().bytes().expect("bytes"));
        assert_eq!(
            document.get("__type"),
            None,
            "the body must carry no __type, got {document:?}"
        );

        let success = r#"{"id":"kept"}"#;
        let answered = untyped(RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(success),
        ));
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            success.as_bytes(),
            "a success body must be byte-identical"
        );
    }

    /// A failure whose body carries no code — an empty one, or one that is not JSON —
    /// still loses the header when the header names one of this service's exceptions, and
    /// keeps it when it names something else.
    #[test]
    fn the_header_is_dropped_even_when_the_body_cannot_be_rewritten() {
        for (case, body, code, kept) in [
            ("an empty body", "", "ResourceNotFoundException", false),
            ("a non-JSON body", "<Error/>", "InvalidParameterException", false),
            ("the scaffold's 501 body", r#"{"__type":"NotImplemented"}"#, "NotImplemented", true),
        ] {
            let mut resp = RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(404u16).expect("404"),
                SdkBody::from(body),
            );
            resp.headers_mut().insert(ERROR_TYPE_HEADER, code);
            let answered = untyped(resp);
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER).is_some(),
                kept,
                "case {case:?}: expected the header kept={kept}, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }
}
