// SPDX-License-Identifier: CC0-1.0

//! How this service NAMES the class of a failure it answers — twice, as the wire protocol of
//! a service that migrated off the query protocol requires.
//!
//! This service's model declares `awsJson1_0` beside `awsQuery` and carries the
//! `awsQueryCompatible` modifier
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json")`),
//! and every client error its model declares carries an `awsQueryError` code — the code a
//! client that still speaks the query vocabulary branches on
//! (`InvalidParameterValueException` -> `InvalidParameterValue`,
//! `MissingRequiredParameterException` -> `MissingParameter`). A JSON answer has nowhere in
//! its body for that code, because the body names the error SHAPE, so such a service states
//! the class in two places at once:
//!
//! ```json
//! HTTP/1.1 400 Bad Request
//! content-type: application/x-amz-json-1.0
//! x-amzn-query-error: ValidationError;Sender
//!
//! {"__type":"com.amazon.coral.validate#ValidationException","message":"…"}
//! ```
//!
//! * the body's `__type` is the FULLY QUALIFIED shape id of the class — the
//!   request-validation class in one package ([`VALIDATION`]) and this service's own
//!   modelled faults in another ([`CORAL_SERVICE_PACKAGE`]);
//! * the `x-amzn-query-error` header carries that class's legacy query code and the FAULT
//!   side (`Sender` for a refusal the caller caused, `Receiver` for a failure this service
//!   caused), for a client that opted into the report with `x-amzn-query-mode: true`.
//!
//! Neither is anything one request chooses: both are fixed text of the error CLASS, and only
//! the class varies. A client branching on the legacy code gets it whichever layer refused
//! the request, and a client matching the shape id can tell a sender fault from a receiver
//! fault.
//!
//! WHY A WRAPPER, and not each refusal site: a failure is rendered by three different layers
//! — a handler's modelled error and the constraint walk's refusal, both inside the
//! framework's per-request sequence, and a refusal this file words ahead of that sequence —
//! and only the first of them has an error VALUE to read a legacy code off. Completing the
//! envelope once, on the way out, is what makes the three read alike;
//! [`QueryCompatibleErrors`] is that one place, and it serves the very operation the
//! generated routing table names for the request, so the framework still owns the whole
//! deserialize -> validate -> handle -> serialize sequence and every status, shape and
//! message it answers with.
//!
//! What is deliberately LEFT ALONE: the HTTP status, the `message`, every other member and
//! header, the whole success path, a header a layer below already reported the class in, and
//! the emulator's own synthetic signals ([`NOT_A_SERVICE_CLASS`]) — the scaffold's 501 and
//! the authorization layer's 403 belong to no class of this service's API, so they are
//! answered exactly as the layer that built them wrote them.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// One error class of this service, with every name an answer states it under.
///
/// The two names of a class are what `awsQueryCompatible` means, and each class states them
/// in ONE place so the body and the header of a single answer cannot name two classes.
#[derive(Debug)]
pub struct ErrorClass {
    /// The local shape name the body of an answer names the class under — and therefore the
    /// code a refusal this service words itself states, so that refusal and the framework's
    /// own render the same class.
    pub shape_name: &'static str,
    /// The fully qualified shape id the body's `__type` carries for the class.
    pub shape_id: &'static str,
    /// The class's `awsQueryError` code: what the `x-amzn-query-error` header reports, and
    /// what a query-era client branches on.
    pub query_code: &'static str,
    /// Every OTHER local name a layer of this service renders this class under, recognised
    /// here so a refusal worded elsewhere is completed into the same class.
    pub also_named: &'static [&'static str],
}

impl ErrorClass {
    /// Whether `named` is one of the names a layer of this service renders this class under.
    fn answers_to(&self, named: &str) -> bool {
        self.shape_name == named || self.also_named.contains(&named)
    }
}

/// The REQUEST-VALIDATION class: a request whose values the service will not accept.
///
/// One class covers both halves of that, because AWS answers them alike: a value the model
/// itself does not admit (the constraint walk's refusal, which the framework already names
/// `ValidationException`) and a precondition this service enforces that its model cannot
/// express (`crate::scaffolded::Scaffolded::validation_error`, whose code is the legacy query
/// code below — the name this class `also_named`). It is declared by no shape of this
/// service's model: the shape belongs to the request-validation layer in front of the
/// service, which is why the body's id names that layer's package rather than this service's.
pub const VALIDATION: ErrorClass = ErrorClass {
    shape_name: "ValidationException",
    shape_id: "com.amazon.coral.validate#ValidationException",
    query_code: "ValidationError",
    also_named: &["ValidationError"],
};

/// The modelled PARAMETER-VALUE fault: "The value of an input parameter is bad or
/// out-of-range".
///
/// Its legacy code is the `awsQueryError` code the model binds to
/// `InvalidParameterValueException` (`smithy_query(query="operation",operation="PutMetricData",model_path="services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json")`),
/// which is what a client of the metric operations branches on. Stated here so the refusals
/// this service words ahead of a handler ([`super::parameter_fault`]) name the same class the
/// handlers' own modelled error does.
pub const PARAMETER_VALUE: ErrorClass = ErrorClass {
    shape_name: "InvalidParameterValueException",
    shape_id: "com.amazon.coral.service#InvalidParameterValueException",
    query_code: "InvalidParameterValue",
    also_named: &[],
};

/// The modelled MISSING-PARAMETER fault: "An input parameter that is required is missing".
///
/// Its legacy code is the `awsQueryError` code the model binds to
/// `MissingRequiredParameterException`, which is what a client that branches on the
/// query-era vocabulary matches on. CloudWatch answers this class for a parameter it was not
/// given a VALUE for, which includes one supplied EMPTY — see
/// [`super::empty_required_value_refusal`].
pub const MISSING_PARAMETER: ErrorClass = ErrorClass {
    shape_name: "MissingRequiredParameterException",
    shape_id: "com.amazon.coral.service#MissingRequiredParameterException",
    query_code: "MissingParameter",
    also_named: &[],
};

/// Every class this service's OWN layers can name a failure under without an error value to
/// read a legacy code off — the ones the refusal seams in [`super`] and
/// `crate::scaffolded` word. A modelled error keeps its own `awsQueryError` code, which the
/// framework reports from the error value itself.
const ERROR_CLASSES: &[&ErrorClass] = &[&VALIDATION, &PARAMETER_VALUE, &MISSING_PARAMETER];

/// The package this service's own modelled faults are named in.
///
/// A fault the model declares is answered by the service framework in front of CloudWatch,
/// which names its own package rather than the model's — so a body states
/// `com.amazon.coral.service#ResourceNotFoundException` for the shape the model declares as
/// `com.amazonaws.cloudwatch#ResourceNotFoundException`. The local name is the model's and is
/// taken from the response rather than restated here, so this file holds no second copy of
/// the model's error list.
const CORAL_SERVICE_PACKAGE: &str = "com.amazon.coral.service";

/// The names that are NOT classes of this service's API, and so are stated under no package.
///
/// Both are the emulator's own synthetic signals (`crate::scaffolded`): the scaffold's
/// not-implemented answer, which says an operation has not been written rather than naming
/// anything AWS answers, and the authorization refusal, which the layer in front of the
/// service makes and no recording of this service shows the shape id of.
const NOT_A_SERVICE_CLASS: &[&str] = &["NotImplemented", "AccessDenied"];

/// The body member the JSON error renderers name the error shape in.
const TYPE_MEMBER: &str = "__type";

/// The response header the legacy query error code and fault side are reported in.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The request header a query-compatible SDK opts into that report with.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";

/// The value that opt-in carries.
const QUERY_MODE_ON: &str = "true";

/// The fault side reported for a failure the CALLER caused — every 4xx.
const SENDER_FAULT: &str = "Sender";

/// The fault side reported for a failure the SERVICE caused — every 5xx.
const RECEIVER_FAULT: &str = "Receiver";

/// The lowest status that reports a failure rather than an answer, and so the lowest one that
/// names an error class at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// The lowest status whose fault is the service's own.
const FIRST_SERVER_FAULT_STATUS: u16 = 500;

/// The local shape name a discriminator value names: the part after the package, since a
/// value may already arrive qualified.
fn local_name(raw: &str) -> &str {
    let raw = raw.trim();
    raw.rsplit('#').next().unwrap_or(raw)
}

/// The class `named` belongs to, or `None` when it is a name no layer of this service words a
/// refusal under — a modelled fault, whose own legacy code the framework reports.
fn class_of(named: &str) -> Option<&'static ErrorClass> {
    ERROR_CLASSES
        .iter()
        .copied()
        .find(|class| class.answers_to(named))
}

/// The fully qualified shape id a body naming `named` states, or `None` when this service
/// states none for it ([`NOT_A_SERVICE_CLASS`]).
fn shape_id_of(named: &str, class: Option<&ErrorClass>) -> Option<String> {
    if let Some(class) = class {
        return Some(class.shape_id.to_string());
    }
    if NOT_A_SERVICE_CLASS.contains(&named) {
        return None;
    }
    Some(format!("{CORAL_SERVICE_PACKAGE}#{named}"))
}

/// Whether `req` asked for the legacy query error report.
///
/// A query-compatible SDK sends the opt-in on every request it makes to this service; a
/// native JSON client sends nothing and gets the unadorned JSON error. The report is
/// additive, so only the opt-in decides it — nothing of the answer is withheld without it.
fn asked_for_the_query_error(req: &RtRequest) -> bool {
    req.headers()
        .get(QUERY_MODE_HEADER)
        .is_some_and(|value| value.eq_ignore_ascii_case(QUERY_MODE_ON))
}

/// The error document `body` carries and the local shape name it names its class under, or
/// `None` when it is not a JSON object naming one.
///
/// ONE parse for both halves of the completion below: the document is what the qualified id is
/// written into, and the name is what the class is resolved from.
fn error_document(body: &[u8]) -> Option<(Value, String)> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let named = local_name(document.as_object()?.get(TYPE_MEMBER)?.as_str()?).to_string();
    (!named.is_empty()).then_some((document, named))
}

/// `document` with `__type` restated as `shape_id`, or `None` when it already names it.
fn with_qualified_type(mut document: Value, shape_id: &str) -> Option<Vec<u8>> {
    let object = document.as_object_mut()?;
    if object.get(TYPE_MEMBER).and_then(Value::as_str) == Some(shape_id) {
        return None;
    }
    object.insert(TYPE_MEMBER.to_string(), Value::from(shape_id));
    serde_json::to_vec(&document).ok()
}

/// `resp` as this `awsQueryCompatible` service answers it: the class named in full in the
/// body, and its legacy query code reported in the header for a client that asked for one.
///
/// A success names no class and is returned untouched, and so is a failure whose class this
/// service states no qualified name for. A header a layer below already reported the class in
/// is kept: the framework writes it from a modelled error's own `awsQueryError` code, which
/// is the class's one source for a modelled fault.
pub fn completing_the_envelope(req: &RtRequest, mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS {
        return resp;
    }
    // An error body is built in memory by its renderer, so the bytes are readable here; a
    // streamed body is not an error body and is left alone.
    let Some((document, named)) = resp.body().bytes().and_then(error_document) else {
        return resp;
    };
    let class = class_of(&named);
    if let Some(rewritten) = shape_id_of(&named, class)
        .and_then(|shape_id| with_qualified_type(document, &shape_id))
    {
        // Only a renderer that advertised a length needs it corrected; the framework's error
        // renderers set `content-type` alone and leave framing to the transport.
        if resp.headers().get("content-length").is_some() {
            let length = rewritten.len().to_string();
            let _ = resp.headers_mut().try_insert("content-length", length);
        }
        *resp.body_mut() = SdkBody::from(rewritten);
    }
    if let Some(class) = class {
        if resp.headers().get(QUERY_ERROR_HEADER).is_none() && asked_for_the_query_error(req) {
            let fault = if resp.status().as_u16() >= FIRST_SERVER_FAULT_STATUS {
                RECEIVER_FAULT
            } else {
                SENDER_FAULT
            };
            // The code and the fault side are both fixed text of the class, so the value is
            // always a representable header value; the fallible insert is used all the same,
            // so that a header which could not be built could only cost the answer its
            // header and never the answer itself.
            let _ = resp
                .headers_mut()
                .try_insert(QUERY_ERROR_HEADER, format!("{};{fault}", class.query_code));
        }
    }
    resp
}

/// One operation, served so that whatever failure it answers names its class the way this
/// `awsQueryCompatible` service names one ([`completing_the_envelope`]).
///
/// It re-implements nothing and makes no routing decision: the wrapped operation is the one
/// this service would have served anyway, and only the rendered answer is completed on the
/// way out. Every operation of this service is served through one of these, so a failure
/// raised by ANY layer — a handler's modelled error, the constraint walk's refusal, or a
/// refusal this file words ahead of the framework's sequence — passes through the one place
/// that can name its class in full.
#[derive(Debug)]
pub struct QueryCompatibleErrors<O> {
    operation: O,
}

impl<O> QueryCompatibleErrors<O> {
    /// Wrap the operation, already constructed from the shared engine.
    pub fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for QueryCompatibleErrors<O>
where
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let response = self.operation.run(svc, req, ctx).await;
            completing_the_envelope(req, response)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        completing_the_envelope, local_name, shape_id_of, with_qualified_type, PARAMETER_VALUE,
        VALIDATION,
    };
    use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
    use aws_smithy_types::body::SdkBody;

    /// One failure answer, as a renderer below this wrapper built it.
    fn answered(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            StatusCode::try_from(status).expect("a status code"),
            SdkBody::from(body.to_string()),
        )
    }

    /// A request, with the query-compatible opt-in when `query_mode`.
    fn asked(query_mode: bool) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        if query_mode {
            req.headers_mut().insert("x-amzn-query-mode", "true");
        }
        req
    }

    /// The `__type` of a completed answer.
    fn named(resp: &RtResponse) -> String {
        let document: serde_json::Value =
            serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
                .expect("a JSON error body");
        document["__type"].as_str().unwrap_or_default().to_string()
    }

    /// Every name a layer of this service renders a class under resolves to the SAME
    /// qualified shape id, and a modelled fault is stated in this service's own package.
    #[test]
    fn a_class_is_named_in_full_whichever_layer_named_it() {
        // (case, the local name a layer below rendered, the shape id a body then states)
        let rows: [(&str, &str, Option<&str>); 6] = [
            (
                "the constraint walk's own refusal",
                VALIDATION.shape_name,
                Some(VALIDATION.shape_id),
            ),
            (
                "this service's own wording of the same class",
                "ValidationError",
                Some(VALIDATION.shape_id),
            ),
            (
                "the modelled parameter-value fault",
                PARAMETER_VALUE.shape_name,
                Some(PARAMETER_VALUE.shape_id),
            ),
            (
                "another modelled fault, stated in the service package",
                "ResourceNotFound",
                Some("com.amazon.coral.service#ResourceNotFound"),
            ),
            ("the scaffold's own signal", "NotImplemented", None),
            ("the authorization layer's refusal", "AccessDenied", None),
        ];
        for (case, rendered, expected) in rows {
            let actual = shape_id_of(rendered, super::class_of(rendered));
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The local name is recovered from a qualified id as well as a bare one, so completing
    /// an already-completed answer changes nothing.
    #[test]
    fn a_qualified_id_is_restated_as_itself() {
        assert_eq!(
            local_name("com.amazon.coral.validate#ValidationException"),
            "ValidationException",
            "the local name of a qualified id"
        );
        let already = format!(r#"{{"__type":"{}","message":"no"}}"#, VALIDATION.shape_id);
        let (document, named) =
            super::error_document(already.as_bytes()).expect("an error document");
        assert_eq!(
            named, "ValidationException",
            "the class named by a qualified id"
        );
        assert!(
            with_qualified_type(document, VALIDATION.shape_id).is_none(),
            "an answer already naming the class in full is left byte for byte"
        );
    }

    /// The header reports the class's legacy code and the fault side, only for a client that
    /// asked for it, and only on a failure.
    #[test]
    fn the_legacy_code_is_reported_for_a_client_that_asked() {
        // (case, status, the name a layer rendered, whether the client opted in, the report)
        let rows: [(&str, u16, &str, bool, Option<&str>); 5] = [
            (
                "a refused request, query mode",
                400,
                "ValidationError",
                true,
                Some("ValidationError;Sender"),
            ),
            (
                "the same refusal for a native JSON client",
                400,
                "ValidationError",
                false,
                None,
            ),
            (
                "a parameter-value refusal, query mode",
                400,
                PARAMETER_VALUE.shape_name,
                true,
                Some("InvalidParameterValue;Sender"),
            ),
            (
                "a service fault of the same class, query mode",
                500,
                "ValidationException",
                true,
                Some("ValidationError;Receiver"),
            ),
            (
                "a class this service words no refusal under",
                404,
                "ResourceNotFound",
                true,
                None,
            ),
        ];
        for (case, status, rendered, query_mode, expected) in rows {
            let body = format!(r#"{{"__type":"{rendered}","message":"no"}}"#);
            let completed =
                completing_the_envelope(&asked(query_mode), answered(status, &body));
            let actual = completed.headers().get("x-amzn-query-error");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the report {expected:?}, got {actual:?}"
            );
        }
    }

    /// A successful answer is returned untouched: it names no class to complete.
    #[test]
    fn a_success_is_left_alone() {
        let served = answered(200, r#"{"Datapoints":[]}"#);
        let completed = completing_the_envelope(&asked(true), served);
        assert_eq!(
            completed.headers().get("x-amzn-query-error"),
            None,
            "a success carries no class report"
        );
        assert_eq!(
            completed.body().bytes().map(<[u8]>::to_vec),
            Some(br#"{"Datapoints":[]}"#.to_vec()),
            "a success keeps every byte of its body"
        );
    }

    /// A refusal rendered by the framework under the generic validation name is completed
    /// into the same class as this service's own wording of it.
    #[test]
    fn the_framework_refusal_is_completed_into_the_validation_class() {
        let body = r#"{"__type":"ValidationException","message":"1 validation error detected"}"#;
        let completed = completing_the_envelope(&asked(true), answered(400, body));
        assert_eq!(
            named(&completed),
            VALIDATION.shape_id,
            "the framework's refusal names the request-validation class in full"
        );
    }
}
