// SPDX-License-Identifier: CC0-1.0

//! CloudFront's error envelope: the one place every failure this service answers is
//! finished off before it goes on the wire.
//!
//! CloudFront is `restXml`, and its error document is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`:
//! the `Error` element states WHAT failed, and the `RequestId` BESIDE it names the
//! request it failed for, so a caller can correlate the failure with the service's own
//! record of it. `RequestId` is a property of the ENVELOPE, not a member of any error
//! shape the model declares, so the protocol serializer — which renders an error from
//! its shape's members — cannot put it there. [`ErrorEnvelope`] does, for every response
//! this service answers: a modelled error a handler raised, and a refusal the request
//! boundary made before any handler ran, alike.
//!
//! Where the element BELONGS is the vendored wrapped-restXml renderer
//! (`aws_smithy_schema_server::rest_xml`'s `serialize_error_wrapped`, which already
//! writes a `RequestId` on the `noErrorWrapping` path beside the `x-amz-request-id`
//! header mirroring it). That tree is read-only from here, and the framework's
//! pre-dispatch renderer for a boundary refusal lives in it too, so the element is
//! added to the finished document instead — delete both halves of this seam once the
//! vendored renderer carries it. Two sibling services splice the same element into their
//! own (awsQuery) envelope the same way —
//! `service-com-amazonaws-cloudformation-cloudformation`'s `query_error_envelope` and
//! `service-com-amazonaws-sns-amazonsimplenotificationservice`'s `service` — so the three
//! are named here together: the splice is per-service today because the envelope and the
//! renderer that writes it are, and hoisting all three belongs with the vendored change
//! that makes them unnecessary rather than with this fix.
//!
//! EVERY failure this service answers passes through here: a modelled error and a
//! boundary refusal reach [`ErrorEnvelope`] on the dispatch path, and the three the
//! router renders OUTSIDE it — a request that names no modelled operation, a
//! transport-level failure, and a refusal the router makes before any operation runs —
//! reach [`CloudFrontDefinition`], which wraps the generated definition's own renderers.
//!
//! The same seam is where a refusal the boundary states in the FRAMEWORK's vocabulary is
//! restated in CloudFront's own. The framework refuses a request it cannot read into the
//! operation's input with `400 ValidationException` and the codec's own text;
//! `ValidationException` is not a shape this service's model declares at all, so a client
//! branching on the error code falls through to a generic failure and never reaches its
//! input-correction path. Three such refusals have a modelled CloudFront answer, and each
//! is read off the REQUEST and the refusal rather than off the operation:
//!
//! * a `MaxItems` the parameter's integer type cannot take is `InvalidArgument` (400),
//!   stating the constraint on the parameter — the same refusal, in the same words, a
//!   list handler makes for a value that parses but names no page
//!   ([`crate::state::MAX_ITEMS_INVALID_MESSAGE`]) — and only where the refusal shows
//!   that value as the one it refused, so a request refused for something else keeps
//!   the cause its caller has to fix;
//! * a value the model's ENUM does not admit is no value of that member at all: the value
//!   set is part of the schema this API publishes for the request, so a request naming a
//!   value outside it is one this API cannot read, and the answer is `MalformedXML` (400)
//!   under the sentence the boundary already wrote, which names the member and the value
//!   set it broke. The BINDING the value travelled in does not change that — a query
//!   parameter, a member of the request's document and a member nested inside its payload
//!   are all the same schema — so the condition reads the constraint the refusal reports
//!   and nothing else. Neither does the LAYER that read the set: a set the model documents
//!   in prose but declares nowhere (the event types a function association admits) is
//!   checked by the operation, which states the same sentence, and the same code answers
//!   it. The value set that sentence lists is a property of the ENUM, so it
//!   is listed in the order this service publishes that enum's values
//!   ([`STATED_VALUE_ORDERS`]);
//! * a body-bearing request that carries NO body cannot have a payload at all, so the
//!   null member the boundary reports is one nested inside that payload and the member
//!   that is really absent is the payload itself: the answer is `MalformedXML` (400)
//!   naming that payload member. A refusal naming a member nested in nothing is about a
//!   member bound outside the body, and keeps the boundary's own wording.
//!
//! All of them leave every other refusal exactly as the layer that made it rendered it: a
//! request the boundary read, and an operation's refusal that reports another rule, are
//! never second-guessed here.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema_server::{missing_member_message, xml_escape};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    percent_decode, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext,
    OperationRoutingTable, ServerHttpProtocol, Service, ServiceIdentity,
};

/// This service's `@xmlNamespace`, which its error envelope carries exactly as its
/// success documents do (`smithy_query(query="service_info")`:
/// `xmlNamespace.uri = http://cloudfront.amazonaws.com/doc/2020-05-31/`).
pub const XML_NAMESPACE: &str = "http://cloudfront.amazonaws.com/doc/2020-05-31/";

/// The envelope element the request-correlation id sits beside `Error` in.
const REQUEST_ID_ELEMENT: &str = "RequestId";

/// The envelope root, whose close is where a missing [`REQUEST_ID_ELEMENT`] is added.
const ERROR_RESPONSE_OPEN: &str = "<ErrorResponse";
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The error code the framework names a request it refused before any handler ran.
/// Not a shape this service's model declares, which is why a response carrying it is
/// a refusal to restate rather than a CloudFront error.
const FRAMEWORK_REFUSAL_CODE: &str = "ValidationException";

/// The status every refusal restated here answers with: the model gives `InvalidArgument`
/// HTTP 400, and an unreadable body is a client error too.
const REFUSED: u16 = 400;

/// `<Error><Type>`: a 4xx is the caller's fault, which restXml words `Sender`.
const SENDER: &str = "Sender";

/// The wire code for an argument the operation models but cannot take
/// (`com.amazonaws.cloudfront#InvalidArgument`, HTTP 400).
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The wire code CloudFront answers a request whose XML body it cannot read — including
/// a required payload that is not there at all.
///
/// The model declares no shape for it (`smithy_query(query="shape",shape="MalformedXML")`
/// finds none), but the service answers it: the recorded responses in this project's
/// corpus carry `<Code>MalformedXML</Code>` for a `CreateDistribution` with an empty
/// body. The alternative the model does declare, `MissingBody`, describes a request with
/// no body AND no content-type; what a client branches on here is the code the service
/// sends, so the code the service sends is what this answers.
const MALFORMED_XML: &str = "MalformedXML";

/// The query parameter every CloudFront list operation binds its page size to
/// (`@httpQuery("MaxItems")` over an `integer`, from the model's list operations).
const MAX_ITEMS_PARAM: &str = "MaxItems";

/// How the request boundary states the constraint an ENUM member carries (`Member must
/// satisfy enum value set: [...]`), the one refusal that says the request named a value the
/// model's enum does not admit.
///
/// It identifies the refusal by the CONSTRAINT it reports rather than by the member or the
/// operation, so it holds for every enum-constrained member of every operation this service
/// serves.
const ENUM_VALUE_SET_MARKER: &str = "Member must satisfy enum value set:";

/// The marker the framework's refusal message states an absent member with
/// (`Value null at '<member path>' failed to satisfy constraint: Member must not be
/// null`), with the apostrophes as the XML envelope escapes them.
const NULL_MEMBER_MARKERS: [(&str, &str); 2] = [
    ("Value null at &apos;", "&apos;"),
    ("Value null at '", "'"),
];

/// A fresh request id for one response, from the one mint this service has for it
/// ([`crate::state::new_request_id`]).
///
/// The VALUE is per-request and is not a requirement — nothing can predict it and no
/// caller may depend on a particular one; its PRESENCE is what the envelope requires,
/// and two responses carry two different ids so a caller can tell them apart. The mint
/// renders the 8-4-4-4-12 lowercase hex an AWS request id takes, the same form this
/// service's other opaque identifiers are rendered in.
fn new_request_id() -> String {
    crate::state::new_request_id()
}

/// The body of `resp` as text, when it is in memory and valid UTF-8.
fn body_text(resp: &RtResponse) -> Option<&str> {
    std::str::from_utf8(resp.body().bytes()?).ok()
}

/// The text of the first `<name>` element of `document`.
///
/// Enough for the envelope's own flat elements (`Code`, `Message`), which is all this
/// module reads: the documents it reads are the ones this service just serialized.
fn element_text<'a>(document: &'a str, name: &str) -> Option<&'a str> {
    let after_open = document.split_once(&format!("<{name}>"))?.1;
    Some(after_open.split_once(&format!("</{name}>"))?.0)
}

/// The error envelope this service answers `status` / `code` / `message` with, carrying
/// its request id.
///
/// The one renderer for an error this module raises itself; a modelled error a handler
/// raised is rendered by the protocol and reaches the same envelope through
/// [`with_request_id`].
fn error_response(status: u16, code: &str, message: &str) -> RtResponse {
    let body = format!(
        "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"{namespace}\"><Error>\
         <Type>{SENDER}</Type><Code>{code}</Code><Message>{message}</Message></Error>\
         <{REQUEST_ID_ELEMENT}>{request_id}</{REQUEST_ID_ELEMENT}></ErrorResponse>",
        namespace = xml_escape(XML_NAMESPACE),
        message = xml_escape(message),
        request_id = new_request_id(),
    );
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(REFUSED).expect("400 is a valid status code"),
        ),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/xml");
    resp
}

/// The body `resp` should carry so its error envelope names the request, or `None` when
/// it is not an envelope that is missing one (a success, an empty body, an envelope that
/// already carries the element).
fn request_id_insertion(resp: &RtResponse) -> Option<String> {
    // A success is never an error envelope, whatever its document happens to contain:
    // the status is what says this response states a failure.
    if resp.status().as_u16() < 400 {
        return None;
    }
    let body = body_text(resp)?;
    if !body.contains(ERROR_RESPONSE_OPEN) || body.contains(&format!("<{REQUEST_ID_ELEMENT}>")) {
        return None;
    }
    let close_at = body.rfind(ERROR_RESPONSE_CLOSE)?;
    let mut rewritten = String::with_capacity(body.len() + 64);
    rewritten.push_str(&body[..close_at]);
    rewritten.push_str(&format!(
        "<{REQUEST_ID_ELEMENT}>{}</{REQUEST_ID_ELEMENT}>",
        new_request_id()
    ));
    rewritten.push_str(&body[close_at..]);
    Some(rewritten)
}

/// `resp` with its error envelope naming the request it answers.
///
/// A response that is not a restXml error envelope, or one that already names the
/// request, is handed back untouched — so this is applied to EVERY response without
/// having to know which ones are errors.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let Some(body) = request_id_insertion(&resp) else {
        return resp;
    };
    let length = body.len();
    *resp.body_mut() = SdkBody::from(body);
    // Only a response that already states its length gets the new one: a renderer that
    // left the framing to the transport keeps doing so.
    if resp.headers().get("content-length").is_some() {
        resp.headers_mut()
            .insert("content-length", length.to_string());
    }
    resp
}

/// The query component of a request URI (everything after the first `?`).
fn request_query(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The `(key, value)` pairs a query component carries — the one split every lookup in this
/// module reads a query string through.
fn query_pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query.split('&').filter_map(|pair| pair.split_once('='))
}

/// The raw value a query component gives `name`, or `None` when it names it no value.
fn query_value<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query_pairs(query).find_map(|(key, value)| (key == name).then_some(value))
}

/// The `MaxItems` the request asks for, read the way the request boundary reads a query
/// value (`+` is a space, `%XX` is the byte it names) through the one decoder the
/// framework vends, so this cannot drift from the boundary it mirrors.
fn requested_max_items(req: &RtRequest) -> Option<String> {
    query_value(request_query(req.uri()), MAX_ITEMS_PARAM).map(percent_decode)
}

/// Whether `value` is a page size `MaxItems` can take.
///
/// Delegates to [`crate::state::page_size`], the one place this service decides what a
/// page size is, so a refusal made at the request boundary and one made by a list
/// handler cannot disagree about which values are usable.
fn is_usable_max_items(value: &str) -> bool {
    value
        .parse::<i32>()
        .is_ok_and(|requested| crate::state::page_size(Some(requested)).is_some())
}

/// How the request boundary shows a value its codec could not read as the parameter's
/// type: ``invalid i32 `x` ``, the shape its integer coercion failure takes. `MaxItems`
/// is modelled as `integer` (the model's list operations) and carries no `@range`, so a
/// value the parameter cannot take fails at THAT coercion and nowhere else — which is
/// why this, and not the value appearing anywhere in the refusal, is what identifies the
/// refusal as being about `MaxItems`.
///
/// Both the raw marker and the one the envelope's escaping produces are returned: the
/// message is XML-escaped into the document, so a value carrying `&`, `<` or an
/// apostrophe reaches the body in its escaped form.
fn coercion_markers(value: &str) -> [String; 2] {
    [
        format!("invalid i32 `{value}`"),
        format!("invalid i32 `{}`", xml_escape(value)),
    ]
}

/// Whether `refusal` is the boundary refusing the request OVER its `MaxItems`: the value
/// is one the parameter cannot take, AND the refusal is the coercion failure of that
/// value.
///
/// The second half matters because a request can carry an unusable `MaxItems` and be
/// refused for something else entirely (a value the boundary reads happily beside an
/// absent member) — restating that refusal as an argument error would hide the cause the
/// caller has to fix, and a value echoed into some other refusal's text cannot stand in
/// for the coercion that never happened.
fn refused_over_max_items(req: &RtRequest, refusal: &str) -> bool {
    let Some(value) = requested_max_items(req) else {
        return false;
    };
    if is_usable_max_items(&value) {
        return false;
    }
    coercion_markers(&value)
        .iter()
        .any(|marker| refusal.contains(marker))
}

/// Whether `refusal` is the boundary refusing the request over a value an enum-constrained
/// member of it carries.
///
/// Read off the REFUSAL alone: the value set an enum declares is part of the schema this API
/// publishes for the request, so the binding the value travelled in — a query parameter, a
/// member of the request's document, a member nested inside its payload — says nothing about
/// whether the request describes something this API can read. What identifies the refusal is
/// the CONSTRAINT it reports, which holds for every enum-constrained member of every
/// operation this service serves.
fn refused_over_enum_value(refusal: &str) -> bool {
    element_text(refusal, "Message").is_some_and(|message| message.contains(ENUM_VALUE_SET_MARKER))
}

/// The value sets this service publishes in an order of its own, each listed in that order.
///
/// A refusal reports the SET a member admits rather than the shape that set came from, so a
/// set is recognised by the values it holds: every member targeting that enum is stated
/// alike, in every operation and in whatever binding carried the value. A value set with no
/// row here is listed in the order it ARRIVED in — the order this API's schema declares the
/// enum in, which is the only order there is evidence for.
const STATED_VALUE_ORDERS: &[&[&str]] = &[
    // `FunctionStage`, the stage every function and connection-function operation names a
    // copy of a function by: the service lists the published stage first, which is not the
    // order the schema declares (`smithy_query(query="shape",shape="FunctionStage")` ->
    // DEVELOPMENT, LIVE).
    &["LIVE", "DEVELOPMENT"],
    // `CachePolicyType`, the kind a cache policy listing filters on: the service lists the
    // caller-owned kind first, which is not the order the schema declares
    // (`smithy_query(query="shape",shape="CachePolicyType")` -> managed, custom).
    &["custom", "managed"],
];

/// `refusal` with the enum's admitted values listed in the order this service publishes that
/// enum's values.
///
/// The value set a refusal reports is a property of the ENUM the member targets, not of the
/// request that broke it, so the order it is listed in is the service's own for that enum
/// ([`STATED_VALUE_ORDERS`]) and nothing the emulator chooses: a set whose published order is
/// the order it arrived in is handed back untouched.
fn with_stated_enum_values(refusal: &str) -> String {
    let Some(message) = element_text(refusal, "Message") else {
        return refusal.to_string();
    };
    let Some(stated) = stated_enum_values(message) else {
        return refusal.to_string();
    };
    refusal.replacen(
        &format!("<Message>{message}</Message>"),
        &format!("<Message>{stated}</Message>"),
        1,
    )
}

/// `message` with the value set it lists after [`ENUM_VALUE_SET_MARKER`] listed in this
/// service's order for that set, or `None` when it lists no set or lists one this service
/// states in the order it already stands in — the rewrite reads the MESSAGE rather than the
/// document, so nothing else the envelope carries can be touched by it.
fn stated_enum_values(message: &str) -> Option<String> {
    let marker_at = message.find(ENUM_VALUE_SET_MARKER)?;
    let tail = &message[marker_at..];
    let open = tail.find('[')?;
    let close = open + tail[open..].find(']')?;
    let values: Vec<&str> = tail[open + 1..close].split(',').map(str::trim).collect();
    let stated = stated_value_order(&values)?;
    Some(format!(
        "{}{}{}",
        &message[..marker_at + open + 1],
        stated.join(", "),
        &message[marker_at + close..]
    ))
}

/// The order this service lists `values` in, or `None` when that is the order they already
/// stand in — every value set but the ones [`STATED_VALUE_ORDERS`] pins.
fn stated_value_order(values: &[&str]) -> Option<Vec<&'static str>> {
    STATED_VALUE_ORDERS
        .iter()
        .find(|stated| is_same_value_set(stated, values))
        .map(|stated| stated.to_vec())
}

/// Whether two value sets hold the same values. An enum's values are unique by definition,
/// so equal lengths plus containment is set equality.
fn is_same_value_set(stated: &[&str], values: &[&str]) -> bool {
    stated.len() == values.len() && stated.iter().all(|value| values.contains(value))
}

/// `refusal` answered under `code`, with everything else the boundary rendered — its
/// `Message` above all — left exactly as it stands.
///
/// Used where what this service restates is the CODE alone: the sentence the boundary wrote
/// already states the member and the constraint it broke, which is the rule the refusal is
/// built from, so rewriting it would say less rather than more. Rewriting the element
/// in place is also what keeps the text's escaping as the renderer produced it.
fn recoded(refusal: &str, from: &str, code: &str) -> RtResponse {
    let body = refusal.replacen(
        &format!("<Code>{from}</Code>"),
        &format!("<Code>{code}</Code>"),
        1,
    );
    let mut resp = RtResponse::new(
        StatusCode::try_from(REFUSED).expect("400 is a valid status code"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/xml");
    resp
}

/// Whether the request carries no document at all: a body that is there and holds
/// nothing but whitespace.
///
/// A body the transport has not buffered reads as UNKNOWN, not as blank: answering
/// "the payload is null" for a document nobody looked at would hide the member a caller
/// actually has to fix.
fn body_is_blank(req: &RtRequest) -> bool {
    req.body()
        .bytes()
        .is_some_and(|bytes| bytes.iter().all(u8::is_ascii_whitespace))
}

/// The member path a refusal message states as null, as the framework rendered it.
fn null_member_path(refusal: &str) -> Option<&str> {
    NULL_MEMBER_MARKERS.iter().find_map(|(open, close)| {
        let after = refusal.split_once(open)?.1;
        Some(after.split_once(close)?.0)
    })
}

/// `resp`'s body when `resp` is a refusal the request boundary made before any handler
/// ran, and `None` for anything else — including every modelled CloudFront error, whose
/// code is a shape this model declares.
fn framework_refusal(resp: &RtResponse) -> Option<&str> {
    if resp.status().as_u16() != REFUSED {
        return None;
    }
    let body = body_text(resp)?;
    (element_text(body, "Code")? == FRAMEWORK_REFUSAL_CODE).then_some(body)
}

/// `resp`'s body and its code when `resp` is a refusal an OPERATION of this service wrote —
/// a 400 under a code this model declares — and `None` for anything else.
fn operation_refusal(resp: &RtResponse) -> Option<(&str, &str)> {
    if resp.status().as_u16() != REFUSED {
        return None;
    }
    let body = body_text(resp)?;
    let code = element_text(body, "Code")?;
    (code != FRAMEWORK_REFUSAL_CODE).then_some((body, code))
}

/// This service's own answer to a refusal the boundary stated in the framework's
/// vocabulary, or `None` to keep the framework's rendering.
///
/// Reads the REQUEST and the REFUSAL, not the operation: every condition is a property of
/// one of them (the value the request gives its page-size parameter, the constraint the
/// refusal reports, and whether the request carries a body at all), so they hold for every
/// operation that binds them rather than for a list of operations named here.
fn restated(req: &RtRequest, resp: &RtResponse) -> Option<RtResponse> {
    // A value outside the set its member admits is no value of that member at all,
    // whichever layer read the set: the boundary refuses one outside a set the SCHEMA
    // carries, and an operation refuses one outside a set the model documents in prose but
    // declares nowhere (a function association's event type). Both report the same defect in
    // the same sentence, so both are answered under `MalformedXML` — the code this API
    // answers a request document it cannot read with.
    if let Some((refusal, code)) = operation_refusal(resp) {
        if refused_over_enum_value(refusal) {
            return Some(recoded(&with_stated_enum_values(refusal), code, MALFORMED_XML));
        }
        return None;
    }

    let refusal = framework_refusal(resp)?;

    // "An argument is invalid": the page size the request asks for is not one.
    if refused_over_max_items(req, refusal) {
        return Some(error_response(
            REFUSED,
            INVALID_ARGUMENT,
            crate::state::MAX_ITEMS_INVALID_MESSAGE,
        ));
    }

    // A value the model's enum does not admit is no value of that member at all: the value
    // set is part of the schema this API publishes for the request, so a request naming a
    // value outside it is one CloudFront cannot read, which it answers with `MalformedXML`
    // (400) — the same code it answers any unreadable request document with, in whatever
    // binding the value travelled. The member and the values it admits are what the
    // boundary's own sentence states, so the code is restated and the sentence is left as it
    // stands — except for the ORDER the value set is listed in, which belongs to the enum and
    // is this service's own for it.
    if refused_over_enum_value(refusal) {
        return Some(recoded(
            &with_stated_enum_values(refusal),
            FRAMEWORK_REFUSAL_CODE,
            MALFORMED_XML,
        ));
    }

    // A request with a body to give carried none, so the whole payload is null. The
    // member the boundary found null is one NESTED inside that payload — which is what an
    // absent payload produces, since a member the payload itself requires cannot be read
    // from a document that is not there — and the member that is actually absent is the
    // payload, the ROOT of that path. A refusal naming a member that is nested in
    // nothing is about a member bound somewhere other than the body (a header, a query
    // parameter), so it keeps the wording the boundary gave it.
    if matches!(req.method(), "POST" | "PUT") && body_is_blank(req) {
        let (payload, _) = null_member_path(refusal)?.split_once('.')?;
        return Some(error_response(
            REFUSED,
            MALFORMED_XML,
            &missing_member_message(payload),
        ));
    }

    None
}

/// The generated definition, with the error envelope finished on the three failures the
/// router renders OUTSIDE the dispatch path: a request that names no modelled operation,
/// a transport-level failure, and a refusal made before any operation runs. Each is the
/// generated renderer's own document with the envelope's request id added, so a caller
/// correlating a failure never has to care which layer refused it.
///
/// Identity, the protocol and the routing table come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here. The
/// one thing between this wrapper and that entry is the AUDIT leg
/// ([`super::management_events`]): the table every request is routed from is the audited
/// one, whose keys are the same modelled operation names, so no request can be served by a
/// path that records nothing.
pub struct CloudFrontDefinition<Eng: Send + Sync + 'static> {
    entry: ::service_common::audited_operation::AuditedDefinition<crate::ServiceDefinition<Eng>>,
}

impl<Eng: Send + Sync + 'static> CloudFrontDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self {
            entry: ::service_common::audited_operation::AuditedDefinition::new(
                crate::ServiceDefinition::new(::std::sync::Arc::clone(&engine)),
                super::management_events::audited_table(&engine),
            ),
        }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`ErrorEnvelope`].
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for CloudFrontDefinition<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_request_id(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(ErasedServiceDefinition::refusal_response(
            &self.entry,
            error,
            chosen,
            req,
        ))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
    }
}

/// The modelled operation the request names, read from the model-derived routing table
/// (never from the service's own override, which routes straight back here).
fn modelled_operation<'s>(
    svc: &'s dyn Service,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// The seam every request this service serves runs through: it dispatches the modelled
/// operation unchanged, then finishes the response's error envelope (see the module
/// docs).
///
/// Stateless and engine-free: the operation it runs is looked up on the service the
/// router hands it, so this holds nothing per request and nothing per engine.
#[derive(Debug, Default)]
pub struct ErrorEnvelope;

impl ErasedOperation for ErrorEnvelope {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The generated deserialize -> validate -> handle -> serialize walk, run
            // exactly as the router would have run it. A request that names no modelled
            // operation never reaches here: the service claims only the ones its routing
            // table resolves.
            let Some(op) = modelled_operation(svc, req) else {
                return with_request_id(svc.definition().routing_error_response(req));
            };
            let resp = op.run(svc, req, ctx).await;
            let resp = restated(req, &resp).unwrap_or(resp);
            with_request_id(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    //! The envelope surgery and the three restatement conditions, read off documents and
    //! requests rather than through the router (the crate's integration suite drives the
    //! same rules end to end over HTTP).

    use super::*;

    /// An error envelope as the protocol serializer renders one: no `RequestId`.
    fn serialized_error(code: &str) -> RtResponse {
        let body = format!(
            "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
             <Code>{code}</Code><Message>something</Message></Error></ErrorResponse>"
        );
        RtResponse::new(
            StatusCode::try_from(400u16).expect("400"),
            SdkBody::from(body),
        )
    }

    /// An enum refusal's value set is listed in the order this service publishes that enum's
    /// values, and a set this service has no order of its own for keeps the order it arrived
    /// in. Nothing else the sentence says changes either way.
    #[test]
    fn an_enum_refusals_value_set_is_listed_in_the_services_order() {
        // (case, the set as the boundary listed it, the set as this service lists it). The
        // value sets come from `smithy_query(query="shape",shape="…")`: FunctionStage
        // (DEVELOPMENT, LIVE), CachePolicyType (managed, custom) and IpAddressType (ipv4,
        // ipv6, dualstack).
        let rows: [(&str, &str, &str); 5] = [
            (
                "the stage set as the schema declares it",
                "[DEVELOPMENT, LIVE]",
                "[LIVE, DEVELOPMENT]",
            ),
            (
                "the stage set already in the order the service lists it",
                "[LIVE, DEVELOPMENT]",
                "[LIVE, DEVELOPMENT]",
            ),
            (
                "the policy-kind set as the schema declares it",
                "[managed, custom]",
                "[custom, managed]",
            ),
            (
                "a three-value set with no order of its own",
                "[ipv4, ipv6, dualstack]",
                "[ipv4, ipv6, dualstack]",
            ),
            (
                "a one-value set holding a stage",
                "[DEVELOPMENT]",
                "[DEVELOPMENT]",
            ),
        ];

        for (case, listed, expected) in rows {
            let refusal = format!(
                "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
                 <Code>{FRAMEWORK_REFUSAL_CODE}</Code><Message>1 validation error detected: \
                 Value &apos;bogus&apos; at &apos;stage&apos; failed to satisfy constraint: \
                 {ENUM_VALUE_SET_MARKER} {listed}</Message></Error></ErrorResponse>"
            );
            let stated = with_stated_enum_values(&refusal);
            let actual = element_text(&stated, "Message").unwrap_or_default();
            let wanted = format!(
                "1 validation error detected: Value &apos;bogus&apos; at &apos;stage&apos; \
                 failed to satisfy constraint: {ENUM_VALUE_SET_MARKER} {expected}"
            );
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected the sentence {wanted:?}, got {actual:?}"
            );
        }

        // A refusal that lists no value set is handed back exactly as it stands.
        let other = "<ErrorResponse><Error><Code>ValidationException</Code>\
                     <Message>Value null at &apos;x.y&apos; failed to satisfy constraint: \
                     Member must not be null</Message></Error></ErrorResponse>";
        let actual = with_stated_enum_values(other);
        assert_eq!(
            actual, other,
            "case \"another constraint\": expected the refusal left alone, got {actual:?}"
        );
    }

    #[test]
    fn request_id_is_added_beside_error_and_only_once() {
        let first = with_request_id(serialized_error("NoSuchDistribution"));
        let body = body_text(&first).expect("an in-memory body").to_string();
        let id = element_text(&body, REQUEST_ID_ELEMENT).expect("a request id");
        assert!(
            body.contains(&format!("</Error><RequestId>{id}</RequestId></ErrorResponse>")),
            "the id must sit beside Error inside the envelope, got {body}"
        );

        // Idempotent: an envelope that already names the request keeps the id it has.
        let again = with_request_id(first);
        assert_eq!(
            element_text(body_text(&again).expect("a body"), REQUEST_ID_ELEMENT),
            Some(id),
            "a second pass must not add a second id"
        );
    }

    #[test]
    fn each_response_carries_its_own_request_id() {
        let one = with_request_id(serialized_error("NoSuchDistribution"));
        let two = with_request_id(serialized_error("NoSuchDistribution"));
        let left = body_text(&one).and_then(|b| element_text(b, REQUEST_ID_ELEMENT).map(str::to_string));
        let right = body_text(&two).and_then(|b| element_text(b, REQUEST_ID_ELEMENT).map(str::to_string));
        assert!(
            left.is_some() && left != right,
            "two responses must carry two ids, got {left:?} and {right:?}"
        );
    }

    #[test]
    fn a_success_document_is_untouched() {
        let body = "<DistributionList><Quantity>0</Quantity></DistributionList>";
        let resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_request_id(resp);
        assert_eq!(
            body_text(&answered),
            Some(body),
            "a response that is no error envelope must be handed back as it is"
        );
    }

    #[test]
    fn max_items_is_usable_only_as_a_non_negative_integer() {
        // (query, the decoded value, whether it is a page size) — the class is "what the
        // integer page size can take", read as the boundary reads a query value. A zero
        // bounds the page at no items, which is a page this service serves.
        let cases = [
            ("MaxItems=not-a-number", Some("not-a-number"), false),
            ("MaxItems=", Some(""), false),
            ("MaxItems=0", Some("0"), true),
            ("MaxItems=-1", Some("-1"), false),
            ("MaxItems=3.5", Some("3.5"), false),
            ("MaxItems=99999999999", Some("99999999999"), false),
            ("MaxItems=%20", Some(" "), false),
            ("MaxItems=1", Some("1"), true),
            // `%31` is the digit `1`: the boundary decodes it, so this does too.
            ("MaxItems=%31", Some("1"), true),
            ("MaxItems=100&Marker=E1", Some("100"), true),
            ("Marker=E1", None, false),
            ("", None, false),
        ];
        for (query, decoded, usable) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri(format!("/2020-05-31/distribution?{query}"))
                .expect("a valid uri");
            let actual = requested_max_items(&req);
            assert_eq!(
                actual.as_deref(),
                decoded,
                "case {query:?}: expected the value {decoded:?}, got {actual:?}"
            );
            if let Some(value) = actual {
                let got = is_usable_max_items(&value);
                assert_eq!(
                    got, usable,
                    "case {query:?}: expected usable={usable}, got {got}"
                );
            }
        }
    }

    #[test]
    fn only_a_refusal_over_max_items_is_restated_as_an_argument_error() {
        // The boundary refuses an unreadable value by showing it; a request carrying a
        // value the boundary READS happily beside a different refusal keeps that refusal.
        let coercion = "<ErrorResponse><Error><Code>ValidationException</Code>\
             <Message>invalid i32 `not-a-number`: invalid digit found in string</Message>\
             </Error></ErrorResponse>"
            .to_string();
        let absent_member = format!(
            "<ErrorResponse><Error><Code>ValidationException</Code><Message>{}</Message>\
             </Error></ErrorResponse>",
            xml_escape(&missing_member_message("distributionConfig.callerReference"))
        );
        let cases = [
            ("MaxItems=not-a-number", coercion.as_str(), true),
            ("MaxItems=0", absent_member.as_str(), false),
            ("MaxItems=10", coercion.as_str(), false),
            // A value echoed into ANOTHER refusal's text is not a coercion of it: the
            // member path a null-member refusal names cannot make the page size the
            // cause.
            (
                "MaxItems=distributionConfig.callerReference",
                absent_member.as_str(),
                false,
            ),
        ];
        for (query, refusal, expected) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri(format!("/2020-05-31/distribution?{query}"))
                .expect("a valid uri");
            let actual = refused_over_max_items(&req, refusal);
            assert_eq!(
                actual, expected,
                "case {query:?}: expected restated={expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_null_member_path_is_read_from_the_framework_refusal() {
        let refusal = format!(
            "<ErrorResponse><Error><Code>ValidationException</Code><Message>{}</Message>\
             </Error></ErrorResponse>",
            xml_escape(&missing_member_message("invalidationBatch.callerReference"))
        );
        assert_eq!(
            null_member_path(&refusal),
            Some("invalidationBatch.callerReference"),
            "the member path must be read back out of the rendered refusal: {refusal}"
        );
    }

    #[test]
    fn only_a_member_nested_in_a_payload_is_restated_as_malformed_xml() {
        // (the null member the boundary reports, the code the service answers) — a member
        // nested inside the body's payload is the absent payload; one nested in nothing
        // is bound somewhere other than the body, so the boundary's refusal stands.
        let cases = [
            ("distributionConfig.callerReference", MALFORMED_XML),
            ("invalidationBatch.paths.quantity", MALFORMED_XML),
            ("alias", FRAMEWORK_REFUSAL_CODE),
            ("ifMatch", FRAMEWORK_REFUSAL_CODE),
        ];
        for (member, expected) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri("/2020-05-31/distribution").expect("a valid uri");
            req.set_method("POST").expect("a valid method");
            let refused = {
                let mut resp = RtResponse::new(
                    StatusCode::try_from(REFUSED).expect("400"),
                    SdkBody::from(format!(
                        "<ErrorResponse><Error><Code>{FRAMEWORK_REFUSAL_CODE}</Code>\
                         <Message>{}</Message></Error></ErrorResponse>",
                        xml_escape(&missing_member_message(member))
                    )),
                );
                resp.headers_mut().insert("content-type", "application/xml");
                resp
            };
            let answered = restated(&req, &refused).unwrap_or(refused);
            let body = body_text(&answered).expect("an in-memory body").to_string();
            let code = element_text(&body, "Code");
            assert_eq!(
                code,
                Some(expected),
                "case {member:?}: expected code {expected}, got {code:?} in {body}"
            );
        }
    }
}
