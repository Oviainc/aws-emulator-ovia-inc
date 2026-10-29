// SPDX-License-Identifier: CC0-1.0

//! The modelled `ValidationException` this service answers for the refusal the FRAMEWORK
//! raised before any handler ran.
//!
//! `com.amazonaws.evs#ValidationException` declares two `@required` members — `message`
//! and `reason`, a `ValidationExceptionReason` — plus the optional `fieldList`
//! (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionReason")`).
//! Every refusal a handler of this crate builds carries all of them, because the generated
//! type makes `reason` non-optional. The one refusal that does not come from a handler is
//! the one the framework raises while reading and validating the request — an absent
//! `@required` member, a value outside a declared `@length`/`@range`, a `@pattern` or enum
//! violation, or a body it cannot read at all. The hook a service has for that refusal
//! (`Service::override_refusal`) chooses an error code and a message and nothing else, so
//! the body reached the caller as `{"__type":"ValidationException","message":"…"}`: a
//! client that builds the modelled shape from it has no value for a member the model
//! requires and cannot deserialize the error at all.
//!
//! [`ModelledRefusal`] is the wrapper this service hands back from
//! `Service::override_operation`, and it is where that body is completed. It serves the
//! modelled operation the generated routing table names for the request — the framework
//! still owns the whole deserialize → validate → handle → serialize sequence, and the
//! status and the error code it answered with — and then, for a `ValidationException`
//! rendered without its `reason`, re-states that refusal as the WHOLE modelled shape,
//! serialized through the service's own protocol: the same path every modelled error of
//! this service takes, so one refused member answers one shape whichever layer refused it.
//!
//! The shape it states:
//!
//! * `message` is the one the response already carried, built by
//!   [`crate::state::constraint_validation_error`] for a refusal this service worded;
//! * `reason` classifies the refusal — the value that renderer states for the model's own
//!   constraint violations, and [`crate::state::REASON_CANNOT_PARSE`] for a
//!   `ValidationException` the framework rendered for a body it could not read as the
//!   operation's input, which reached no member to check;
//! * `fieldList` carries the entries that same renderer built, one per refused member
//!   path, as the handler-side refusals of this service report theirs.
//!
//! What is deliberately left alone: the HTTP status, the error code, the message and the
//! whole success path; a body that already states its `reason` — every refusal a handler
//! returned — is returned untouched; and a failure answered before any operation is served
//! (a request naming no modelled operation, a transport-level failure, an authentication
//! refusal) keeps the framework's rendering, because none of those is this service's
//! `ValidationException`.

use std::cell::RefCell;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use serde_json::{Map, Value};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use crate::ops::types::{ValidationException, SCHEMA_VALIDATION_EXCEPTION};

/// The error shape this module states — the only client-input error this service's model
/// declares, and the `__type` the framework's refusal rendering names.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `@required` member a refusal rendered outside a handler arrives without.
const REASON: &str = "reason";

tokio::task_local! {
    /// The `ValidationException` this service worded for the framework-level refusal of
    /// the request this task is serving, recorded by `Service::override_refusal` — which
    /// the framework hands no request, and from which only a code and a message can be
    /// returned — and read back by [`ModelledRefusal::run`] once the response exists.
    ///
    /// Scoped around ONE operation's run, so the refusal recorded in it belongs to that
    /// request and to no other. Empty means no such refusal was worded: either the request
    /// was not refused at all, or it was refused for something no member of the input
    /// explains.
    static WORDED_REFUSAL: RefCell<Option<ValidationException>>;
}

/// Record the `ValidationException` this service worded for a framework-level refusal, so
/// the refusal can be answered as the whole modelled shape.
///
/// A refusal raised outside an operation's run — an authentication refusal, which this
/// service does not reword — is recorded nowhere and changes nothing.
pub fn note_worded_refusal(refusal: &ValidationException) {
    let _ = WORDED_REFUSAL.try_with(|slot| {
        *slot.borrow_mut() = Some(refusal.clone());
    });
}

/// The refusal recorded for this request, taken out of the scope so it is read once.
fn worded_refusal() -> Option<ValidationException> {
    WORDED_REFUSAL
        .try_with(|slot| slot.borrow_mut().take())
        .ok()
        .flatten()
}

/// The error code a body names, reduced to the bare shape name.
///
/// Read from the body's `__type`, which is what this service's awsJson error rendering
/// writes; it may carry a namespace (`com.amazonaws.evs#ValidationException`) or a
/// trailing locator, neither of which is part of the code.
fn error_code(object: &Map<String, Value>) -> Option<&str> {
    let raw = object.get("__type").and_then(Value::as_str)?.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    let code = after_namespace.split(':').next().unwrap_or(after_namespace);
    (!code.is_empty()).then_some(code)
}

/// The `message` of a `ValidationException` body that states no `reason` — the refusal
/// this module completes — and `None` for every other body: one that is not a JSON
/// object, one naming another error code or none, and one that already states its reason
/// (every refusal a handler built, which is complete as the renderer wrote it).
fn incomplete_refusal(body: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object()?;
    if error_code(object)? != VALIDATION_EXCEPTION || object.contains_key(REASON) {
        return None;
    }
    Some(
        object
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// The response the caller gets for one served request: the one the framework rendered,
/// unless it is a `ValidationException` stated without the members the model requires, in
/// which case the same refusal is re-stated as the whole modelled shape.
///
/// The status is the one the response already carried, and the error code is the shape's
/// own name, so neither changes; a response that is not a failure, or whose body is not
/// such a refusal, is returned untouched. A shape the protocol cannot serialize leaves the
/// framework's own answer in place rather than failing the request.
fn with_modelled_refusal(
    svc: &dyn Service,
    resp: RtResponse,
    worded: Option<ValidationException>,
) -> RtResponse {
    let status = resp.status().as_u16();
    if status < 400 {
        return resp;
    }
    let message = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match incomplete_refusal(bytes) {
            Some(message) => message,
            None => return resp,
        }
    };
    let refusal = worded.unwrap_or_else(|| ValidationException {
        message,
        reason: crate::state::REASON_CANNOT_PARSE.to_string(),
        field_list: None,
    });
    svc.definition()
        .protocol()
        .serialize_error(
            &refusal,
            &SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            status,
        )
        .unwrap_or(resp)
}

/// The operation the generated routing table names for this request.
fn modeled_operation<'s>(
    svc: &'s dyn Service,
    req: &'s RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ModelledRefusal`].
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
/// then states a `ValidationException` the framework rendered for it as the whole modelled
/// shape.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk, and the status,
/// the error code and the message of everything it answers; this wrapper owns only the
/// `reason` and `fieldList` of a refusal body that reached the caller without them.
pub struct ModelledRefusal;

impl ErasedOperation for ModelledRefusal {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let Some(op) = modeled_operation(svc, req) else {
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                return svc.definition().routing_error_response(req);
            };
            WORDED_REFUSAL
                .scope(RefCell::new(None), async move {
                    let resp = op.run(svc, req, ctx).await;
                    with_modelled_refusal(svc, resp, worded_refusal())
                })
                .await
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

    /// A refusal stated without its `reason` is the one body this module completes, and
    /// the message it carries is the one re-stated — whichever form the code arrives in.
    #[test]
    fn a_refusal_without_its_reason_is_the_body_to_complete() {
        for (case, body, message) in [
            (
                "a bare code",
                r#"{"__type":"ValidationException","message":"1 validation error detected"}"#,
                "1 validation error detected",
            ),
            (
                "a namespaced code",
                r#"{"__type":"com.amazonaws.evs#ValidationException","message":"no"}"#,
                "no",
            ),
            (
                "a code with a locator",
                r#"{"__type":"ValidationException:http://internal.amazon.com/coral/","message":"no"}"#,
                "no",
            ),
            (
                "a message the renderer left out",
                r#"{"__type":"ValidationException"}"#,
                "",
            ),
        ] {
            assert_eq!(
                incomplete_refusal(body.as_bytes()).as_deref(),
                Some(message),
                "case {case:?}: expected the message {message:?} to be re-stated"
            );
        }
    }

    /// Nothing to complete: a body that already states its reason (every refusal a
    /// handler built), another error shape, a body naming no code, and one that is not a
    /// JSON object, are all left alone.
    #[test]
    fn a_body_this_module_does_not_own_is_left_alone() {
        for (case, body) in [
            (
                "a handler's own refusal",
                r#"{"__type":"ValidationException","message":"no","reason":"other"}"#,
            ),
            (
                "another error shape",
                r#"{"__type":"ResourceNotFoundException","message":"no"}"#,
            ),
            ("no code at all", r#"{"message":"no"}"#),
            ("an empty code", r#"{"__type":"","message":"no"}"#),
            ("not an object", r#"["ValidationException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                incomplete_refusal(body.as_bytes()).is_none(),
                "case {case:?}: expected the body to be left alone"
            );
        }
    }
}
