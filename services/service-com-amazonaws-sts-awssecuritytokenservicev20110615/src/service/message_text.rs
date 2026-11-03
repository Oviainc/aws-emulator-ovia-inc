// SPDX-License-Identifier: CC0-1.0

//! The BYTES this service's error messages reach the wire as.
//!
//! An `awsQuery` failure is an XML `<ErrorResponse>` whose `<Message>` carries the
//! sentence the service states. The sentence itself is decided elsewhere — by a handler,
//! by the request boundary, or by the refusal wording in the parent module — and this
//! module owns one thing about it: the escaping of the element's TEXT.
//!
//! XML requires a text node to escape `&` and `<`, and `>` only where it would close a
//! `]]>` sequence. The quote characters are markup only inside an ATTRIBUTE value, so in
//! text they are written as themselves — which is how a recorded response of this service
//! delimits the member path inside a validation message (`… at 'tradeInToken' failed …`).
//! The framework's XML writer escapes all five predefined entities
//! (`vendor/smithy-schema-server/src/xml.rs`, read-only from here), so an apostrophe this
//! service's own sentences carry reached the wire as `&apos;`: invisible to a client with
//! an XML parser, and wrong for anything that reads, logs, proxies or byte-compares the
//! error body as it was sent.
//!
//! So the element's text is restated to the escaping XML asks for, once, on the finished
//! document — which is the only place every failure this service answers passes through:
//!
//! * [`StatedMessageDefinition`] covers the three the router renders OUTSIDE the dispatch
//!   path (a request naming no modelled operation, a transport-level failure, and a
//!   refusal made before any operation ran);
//! * the operation wrapper in the parent module covers a modelled error a handler raised
//!   and a refusal raised while an operation was being served, by passing its response
//!   through [`with_literal_message_text`].
//!
//! Nothing else about the document is touched: the status, the `<Code>`, the sentence's
//! words and every other element stay with whatever rendered them, and a response that is
//! not an error envelope (every success) is handed back as it came. Delete this seam once
//! the vendored writer escapes a text node as text.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    ErasedServiceDefinition, OperationRoutingTable, ServerHttpProtocol, ServiceIdentity,
};

/// The root element of the `awsQuery` error envelope: what tells a failure's document from
/// a success's.
const ERROR_RESPONSE_OPEN: &str = "<ErrorResponse";

/// The element whose text states the failure.
const MESSAGE_OPEN: &str = "<Message>";
const MESSAGE_CLOSE: &str = "</Message>";

/// The sequence a `>` must stay escaped in: the only place XML requires escaping it in a
/// text node.
const CDATA_CLOSE: &str = "]]>";
const CDATA_CLOSE_PREFIX: &str = "]]";

/// `resp` with its `<Message>` text escaped the way XML asks a text node to be.
///
/// Idempotent, and a no-op for anything that is not an `awsQuery` error envelope with a
/// message in it — so every response can be passed through without the caller having to
/// know which ones are failures.
pub(crate) fn with_literal_message_text(mut resp: RtResponse) -> RtResponse {
    let Some(body) = restated_document(&resp) else {
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

/// The document `resp` should carry, or `None` when it already carries it (and when there
/// is no error envelope to read: a streamed body, bytes that are not UTF-8, a success).
fn restated_document(resp: &RtResponse) -> Option<String> {
    let body = std::str::from_utf8(resp.body().bytes()?).ok()?;
    if !body.contains(ERROR_RESPONSE_OPEN) {
        return None;
    }
    let open = body.find(MESSAGE_OPEN)? + MESSAGE_OPEN.len();
    let close = open + body[open..].find(MESSAGE_CLOSE)?;
    let escaped = &body[open..close];
    let stated = text_node(escaped);
    (stated != escaped).then(|| format!("{}{stated}{}", &body[..open], &body[close..]))
}

/// `escaped` with the entity references XML does not ask a text node for resolved: the two
/// quote characters, and `>` wherever it does not close a `]]>`.
///
/// Reference resolution is safe as plain replacement, and needs no scanner of its own,
/// because the text came from a writer that escapes EVERY `&` as `&amp;`: the only `&`
/// left in it begins a reference that writer wrote, so a message whose own text reads
/// `&apos;` arrives as `&amp;apos;` — which holds no `&apos;` to resolve — and a reference
/// XML does require (`&amp;`, `&lt;`) or one the writer chose for a control character
/// (`&#xA;`) is named by none of the replacements and is left as it stands.
fn text_node(escaped: &str) -> String {
    let stated = escaped
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&gt;", ">");
    // `>` is markup in exactly one place in a text node — closing a CDATA section — which
    // is the one place XML requires it escaped, so that `>` goes back.
    stated.replace(CDATA_CLOSE, &format!("{CDATA_CLOSE_PREFIX}&gt;"))
}

/// The generated definition, with the `<Message>` of the three failures the router renders
/// OUTSIDE the dispatch path restated (see the module docs).
///
/// Identity, the protocol and the routing table come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here.
pub struct StatedMessageDefinition<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> StatedMessageDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads into
    /// every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for StatedMessageDefinition<Eng> {
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
        with_literal_message_text(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_literal_message_text(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_literal_message_text(ErasedServiceDefinition::refusal_response(
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

#[cfg(test)]
mod tests {
    use super::{text_node, with_literal_message_text, MESSAGE_CLOSE, MESSAGE_OPEN};
    use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
    use aws_smithy_types::body::SdkBody;

    /// RULE, over the characters a message can carry: a text node escapes `&` and `<`, and
    /// `>` only where it would close a `]]>`; everything else is written as itself.
    #[test]
    fn a_text_node_escapes_only_what_xml_requires() {
        // (case, the text as the framework's XML writer escaped it, the text XML asks for)
        let rows: [(&str, &str, &str); 10] = [
            (
                "the apostrophes a validation message delimits a member path with",
                "Value at &apos;tradeInToken&apos; failed",
                "Value at 'tradeInToken' failed",
            ),
            (
                "the quotes a message quotes a document with",
                "the token &quot;abc&quot; is not readable",
                "the token \"abc\" is not readable",
            ),
            (
                "an ampersand, which XML requires escaped",
                "a &amp; b",
                "a &amp; b",
            ),
            (
                "a less-than, which XML requires escaped",
                "expected &lt;Policy&gt;",
                "expected &lt;Policy>",
            ),
            (
                "a greater-than that would close a CDATA section",
                "the document ends ]]&gt; here",
                "the document ends ]]&gt; here",
            ),
            (
                "an escaped reference, which must not be resolved twice",
                "the text &amp;apos; itself",
                "the text &amp;apos; itself",
            ),
            (
                "the numeric references the writer uses for control characters",
                "line one&#xA;line two",
                "line one&#xA;line two",
            ),
            (
                "text with nothing to restate",
                "Invalid trade-in token",
                "Invalid trade-in token",
            ),
            ("empty text", "", ""),
            (
                "an unterminated reference",
                "a &apos; and a bare & there",
                "a ' and a bare & there",
            ),
        ];

        for (case, escaped, expected) in rows {
            let stated = text_node(escaped);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
            // Idempotent: restating a text node that already escapes only what XML
            // requires changes nothing.
            let again = text_node(&stated);
            assert_eq!(
                again, stated,
                "case {case:?}: restating must be idempotent, got {again:?}"
            );
        }
    }

    /// One response, as a renderer hands it over.
    fn rendered(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            StatusCode::try_from(status).expect("the status is valid"),
            SdkBody::from(body.to_string()),
        )
    }

    /// The body of a response, as text.
    fn body_of(resp: &RtResponse) -> String {
        String::from_utf8(resp.body().bytes().expect("an in-memory body").to_vec())
            .expect("a UTF-8 body")
    }

    /// RULE: the restatement touches the `<Message>` text of an error envelope and nothing
    /// else — not the other elements, and not a document that is no error envelope.
    #[test]
    fn only_an_error_envelopes_message_is_restated() {
        let envelope = |message: &str| {
            format!(
                "<ErrorResponse><Error><Type>Sender</Type><Code>ValidationError</Code>\
                 {MESSAGE_OPEN}{message}{MESSAGE_CLOSE}</Error></ErrorResponse>"
            )
        };

        let restated = with_literal_message_text(rendered(
            400,
            &envelope("Value at &apos;tradeInToken&apos; failed"),
        ));
        assert_eq!(
            body_of(&restated),
            envelope("Value at 'tradeInToken' failed"),
            "an error envelope's message states the apostrophes as themselves"
        );

        // A success document is left exactly as it came, whatever it carries.
        let success = "<GetCallerIdentityResponse><Arn>arn:aws:iam::1:user/a&apos;b</Arn>\
                       </GetCallerIdentityResponse>";
        let served = with_literal_message_text(rendered(200, success));
        assert_eq!(
            body_of(&served),
            success,
            "a response that is no error envelope is handed back untouched"
        );

        // An envelope with no message element is an envelope this does not rewrite.
        let codeless = "<ErrorResponse><Error><Code>ValidationError</Code></Error></ErrorResponse>";
        let served = with_literal_message_text(rendered(400, codeless));
        assert_eq!(
            body_of(&served),
            codeless,
            "an envelope carrying no message is handed back untouched"
        );
    }
}
