// SPDX-License-Identifier: CC0-1.0

//! How the ENVIRONMENT data endpoint answers a request that names an environment it does
//! not front.
//!
//! # The rule
//!
//! The operations the model hosts on that endpoint (`@endpoint` host prefix `env.`) are
//! addressed to a per-environment front door, which has to resolve the request to one
//! environment's Apache Airflow web server before any of this service's own code runs. A
//! name that resolves to nothing is therefore refused BY THAT FRONT DOOR, ahead of the
//! layer that owns the modelled errors and their serialization — so the 404 it answers
//! carries the failure SENTENCE as its whole body, as the recorded wire shows:
//!
//! ```text
//! HTTP/1.1 404 Not Found
//! x-amzn-errortype: ResourceNotFoundException
//!
//! Environment not found
//! ```
//!
//! No JSON envelope around it and no discriminator member inside it: the body is the
//! sentence and nothing else. The error CODE still travels in the `x-amzn-errortype`
//! header, which is where restJson1 resolves an error shape from, so a client still reads
//! the modelled code — and a client that reads the body as the human-readable reason gets
//! that text rather than a serialized object. An SDK's JSON error parser, finding a body
//! that is not JSON, reports the body itself as the message, so both the code and the
//! sentence still reach the caller.
//!
//! Every OTHER refusal of this service keeps the protocol's own error document: the
//! control-plane (`api.`) lookups, which word the condition from the request, and every
//! refusal the application layer behind this front door raises — a request outside a
//! modelled constraint, an Apache Airflow call the environment's own API refused — are
//! answered by the layer that models them, not by the front door.
//!
//! Only the body is the front door's: the status, the `x-amzn-errortype` header and the
//! media type the protocol announces are left where their renderer put them, because what
//! the wire record settles is the body. A client's JSON error parser reads the body it
//! cannot parse as a document as the message either way, so nothing a caller reads depends
//! on re-announcing it.
//!
//! # Where it is applied
//!
//! [`EnvironmentDataEndpoint`] is the wrapper the service hands back from its
//! `override_operation` seam for exactly the operations hosted on that endpoint
//! ([`routes`]). It serves the operation the generated routing table names — the framework
//! keeps the whole deserialize → validate → handle → serialize walk, and the handler keeps
//! raising its modelled `ResourceNotFoundException`, so the status, the code and the
//! sentence are all still decided there — and then answers that one refusal the way the
//! front door answers it. Nothing else is touched: every other status, header and body
//! byte is left exactly as its renderer wrote it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Every operation the model hosts on the environment data endpoint, by the routing key
/// (the operation's shape name) the protocol resolves for it.
///
/// Source: this service's Smithy model —
/// `smithy_query(query="operation",operation="CreateCliToken,CreateWebLoginToken,InvokeRestApi",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")`
/// reports `endpoint.hostPrefix` `env.` for these three, and `api.` for every other
/// operation of this service.
pub const ENVIRONMENT_DATA_OPERATIONS: [&str; 3] =
    ["CreateCliToken", "CreateWebLoginToken", "InvokeRestApi"];

/// The response header restJson1 resolves an error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The response header a renderer that advertises its body's length writes.
const CONTENT_LENGTH: &str = "content-length";

/// The shape the front door's refusal of an unresolvable name names — the only error any
/// of [`ENVIRONMENT_DATA_OPERATIONS`] models at HTTP 404.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The members a JSON error document can carry its error code in, in the order an SDK's
/// parser consults them.
const DISCRIMINATOR_MEMBERS: [&str; 3] = ["__type", "code", "Code"];

/// The members it can carry the sentence in.
const MESSAGE_MEMBERS: [&str; 2] = ["message", "Message"];

/// The status the front door refuses an unresolvable name with.
const NOT_FOUND: u16 = 404;

/// The bare shape name a discriminator value names: it may arrive namespaced
/// (`com.amazonaws.mwaa#ResourceNotFoundException`) or with a trailing locator, and
/// neither is part of the code.
fn shape_name(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace.split(':').next().unwrap_or(after_namespace)
}

/// The sentence the front door's refusal of an unresolvable name carries, read out of the
/// error document the framework rendered for it — or `None` when this response is not that
/// refusal.
///
/// It is read rather than restated, so the body a client receives cannot drift from the
/// message the handler raised: the handler decides the sentence, this decides the shape it
/// travels in. The code is taken from the `x-amzn-errortype` header, falling back to the
/// document's own discriminator, so a refusal of some OTHER condition at the same status
/// (were one ever modelled) keeps its envelope.
fn front_door_sentence(status: u16, code: Option<&str>, body: &[u8]) -> Option<String> {
    if status != NOT_FOUND {
        return None;
    }
    let document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object()?;
    let named = code
        .or_else(|| {
            DISCRIMINATOR_MEMBERS
                .iter()
                .find_map(|member| object.get(*member).and_then(Value::as_str))
        })
        .map(shape_name)?;
    if named != RESOURCE_NOT_FOUND_EXCEPTION {
        return None;
    }
    MESSAGE_MEMBERS
        .iter()
        .find_map(|member| object.get(*member).and_then(Value::as_str))
        .map(str::to_string)
}

/// `resp` as the environment data endpoint's front door answers it: a refusal of a name it
/// cannot resolve carries the sentence alone, and everything else is returned untouched.
///
/// The status, the `x-amzn-errortype` header and every other header are left where their
/// renderer put them — only the body bytes are the front door's own — and a renderer that
/// advertised a length has it corrected to the bytes a client now reads.
pub fn fronted(mut resp: RtResponse) -> RtResponse {
    let sentence = {
        // An error body is built in memory by its renderer, so the bytes are readable here;
        // a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let code = resp.headers().get(ERROR_TYPE_HEADER);
        match front_door_sentence(resp.status().as_u16(), code, bytes) {
            Some(sentence) => sentence,
            None => return resp,
        }
    };
    if resp.headers().get(CONTENT_LENGTH).is_some() {
        let length = sentence.len().to_string();
        let _ = resp.headers_mut().try_insert(CONTENT_LENGTH, length);
    }
    *resp.body_mut() = SdkBody::from(sentence);
    resp
}

/// The operation the generated routing table names for this request, when it is one the
/// environment data endpoint hosts.
fn environment_data_operation<'s>(
    svc: &'s dyn Service,
    req: &'s RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let definition = svc.definition();
    let key = definition.protocol().routing_key(req).ok()?;
    if !ENVIRONMENT_DATA_OPERATIONS.contains(&key) {
        return None;
    }
    definition.routing_table().get(key)
}

/// Whether this request names an operation the environment data endpoint hosts — the
/// condition under which the service claims it for [`EnvironmentDataEndpoint`].
///
/// Every other request is declined here and resolves through the generated routing table
/// untouched, so this front door cannot change how a control-plane lookup or an unroutable
/// request is answered.
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .filter(|key| ENVIRONMENT_DATA_OPERATIONS.contains(key))
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves one operation of the environment data endpoint exactly as the generated routing
/// table would, then answers the front door's own refusal of an unresolvable name the way
/// the front door answers it ([`fronted`]).
///
/// It holds no operation of its own: the one it delegates to is the one the routing table
/// names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`].
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentDataEndpoint;

impl ErasedOperation for EnvironmentDataEndpoint {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match environment_data_operation(svc, req) {
                Some(op) => fronted(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a request
                // only when the routing table holds the key of an operation this endpoint
                // hosts ([`routes`]); answered as the framework answers an unroutable
                // request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rendered response carrying `body`, at `status`, with the error-type header when
    /// `code` is `Some`.
    fn response(status: u16, code: Option<&str>, body: &'static str) -> RtResponse {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
            SdkBody::from(body),
        );
        if let Some(code) = code {
            resp.headers_mut()
                .insert(ERROR_TYPE_HEADER, code.to_string());
        }
        resp
    }

    /// The body a client reads off a response.
    fn text(resp: &RtResponse) -> String {
        String::from_utf8_lossy(resp.body().bytes().expect("in-memory bytes")).to_string()
    }

    /// The front door's refusal becomes the sentence alone — whichever member the renderer
    /// put the code and the sentence in, and whatever the sentence says.
    #[test]
    fn the_refusal_of_an_unresolvable_name_carries_the_sentence_alone() {
        for discriminator in DISCRIMINATOR_MEMBERS {
            for message in MESSAGE_MEMBERS {
                for sentence in ["Environment not found", "Any sentence the handler raised"] {
                    let case = format!("{discriminator}/{message}/{sentence}");
                    let body = format!(
                        r#"{{"{discriminator}":"{RESOURCE_NOT_FOUND_EXCEPTION}","{message}":"{sentence}"}}"#
                    );
                    let answered = front_door_sentence(NOT_FOUND, None, body.as_bytes());
                    assert_eq!(
                        answered.as_deref(),
                        Some(sentence),
                        "case {case:?}: expected the sentence alone, got {answered:?}"
                    );
                }
            }
        }
    }

    /// The code is recognised in each form it can arrive in: the header the protocol
    /// resolves the shape from, bare or namespaced or with a locator, and the document's
    /// own discriminator when the header is absent.
    #[test]
    fn the_code_is_recognised_in_every_form_it_arrives_in() {
        let body = r#"{"message":"Environment not found"}"#;
        for (case, code) in [
            ("a bare code", "ResourceNotFoundException"),
            ("a namespaced code", "com.amazonaws.mwaa#ResourceNotFoundException"),
            (
                "a code with a locator",
                "ResourceNotFoundException:http://internal.amazon.com/coral/",
            ),
        ] {
            let answered = front_door_sentence(NOT_FOUND, Some(code), body.as_bytes());
            assert_eq!(
                answered.as_deref(),
                Some("Environment not found"),
                "case {case:?}: expected the sentence, got {answered:?}"
            );
        }
        let document = r#"{"__type":"ResourceNotFoundException","message":"Environment not found"}"#;
        let answered = front_door_sentence(NOT_FOUND, None, document.as_bytes());
        assert_eq!(
            answered.as_deref(),
            Some("Environment not found"),
            "case \"the document's own discriminator\": expected the sentence, got {answered:?}"
        );
    }

    /// Nothing the front door does not refuse is reshaped: another status, another code, a
    /// document with no sentence in it, a body that is not a JSON object, and a success.
    #[test]
    fn every_other_answer_is_left_exactly_as_rendered() {
        let not_found = r#"{"__type":"ResourceNotFoundException","message":"Environment not found"}"#;
        for (case, status, code, body) in [
            ("a served call", 200u16, None, r#"{"RestApiStatusCode":200}"#),
            (
                "an Apache Airflow refusal",
                400,
                Some("RestApiClientException"),
                r#"{"RestApiStatusCode":404}"#,
            ),
            (
                "a constraint refusal",
                400,
                Some("ValidationException"),
                r#"{"__type":"ValidationException","message":"1 validation error detected"}"#,
            ),
            (
                "the scaffold's 501",
                501,
                Some("NotImplemented"),
                r#"{"__type":"NotImplemented","message":"no"}"#,
            ),
            (
                "a 404 of another shape",
                404,
                Some("NotFoundException"),
                r#"{"__type":"NotFoundException","message":"Resource was not found"}"#,
            ),
            (
                "a 404 document with no sentence",
                404,
                Some("ResourceNotFoundException"),
                r#"{"__type":"ResourceNotFoundException"}"#,
            ),
            (
                "a 404 whose body is not an object",
                404,
                Some("ResourceNotFoundException"),
                r#"["Environment not found"]"#,
            ),
            (
                "a 404 whose body is already the sentence",
                404,
                Some("ResourceNotFoundException"),
                "Environment not found",
            ),
        ] {
            let answered = fronted(response(status, code, body));
            assert_eq!(
                text(&answered),
                body,
                "case {case:?}: the body must be byte-identical, got {}",
                text(&answered)
            );
            assert_eq!(
                answered.status().as_u16(),
                status,
                "case {case:?}: the status must not move, got {}",
                answered.status().as_u16()
            );
        }

        // The positive control for the rows above, through the same one function.
        let answered = fronted(response(404, Some(RESOURCE_NOT_FOUND_EXCEPTION), not_found));
        assert_eq!(
            text(&answered),
            "Environment not found",
            "the front door's refusal must carry the sentence alone, got {}",
            text(&answered)
        );
        assert_eq!(
            answered.headers().get(ERROR_TYPE_HEADER),
            Some(RESOURCE_NOT_FOUND_EXCEPTION),
            "the code must still travel in the header, got {:?}",
            answered.headers().get(ERROR_TYPE_HEADER)
        );
    }

    /// A renderer that advertised a length gets it corrected to the bytes the client reads.
    #[test]
    fn an_advertised_length_follows_the_body() {
        let body = r#"{"__type":"ResourceNotFoundException","message":"Environment not found"}"#;
        let mut resp = response(404, Some(RESOURCE_NOT_FOUND_EXCEPTION), body);
        resp.headers_mut()
            .insert(CONTENT_LENGTH, body.len().to_string());
        let answered = fronted(resp);
        let expected = "Environment not found".len().to_string();
        assert_eq!(
            answered.headers().get(CONTENT_LENGTH),
            Some(expected.as_str()),
            "expected the length of the sentence ({expected}), got {:?}",
            answered.headers().get(CONTENT_LENGTH)
        );
    }
}
