// SPDX-License-Identifier: CC0-1.0

//! The request-schema check Route 53 makes before it reads a request at all.
//!
//! Route 53 is `restXml`, and its API takes a request BODY that is validated
//! against the service's request schema while it is parsed — before any
//! operation sees it. Some elements of that schema accept a narrower set of
//! values than their modelled type: the Smithy model carries
//! `HealthCheckConfig$RequestInterval` as `@range min 10 max 30`, while the
//! service accepts only the two values the schema enumerates, and the refusal it
//! answers for anything else is the schema violation itself — an XML parse
//! failure naming the offending value, the enumeration it is not in, and the
//! position in the request body where that value ends.
//!
//! That position is why the check lives here rather than in an operation's
//! handler: a handler is given the deserialized input, which no longer knows how
//! the request was spelled, while this seam is given the request. It runs for
//! every request this service serves, so an enumerated element is held to its
//! value set wherever it appears, and a request that breaks none is handed on
//! untouched. The body is read with the same maintained XML reader the rest of
//! this emulator reads restXml with, never by looking at the text; an element
//! spelled with no value at all (`<RequestInterval/>`) carries no value to judge
//! against a value set, and is refused by the codec that reads the request for
//! its type instead.
//!
//! The catalog below is the value SET of each such element. It is not in the
//! model — a model records the type, not the service's schema — so each row
//! names where its set comes from.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::{Refused, Service};

/// The wire error code Route 53 answers a request its schema does not accept
/// with: the `InvalidInput` (HTTP 400) its model declares for every input-level
/// refusal.
const INVALID_INPUT: &str = "InvalidInput";

/// One element of the request schema whose accepted values are ENUMERATED, so a
/// value of the right type can still be one the schema does not accept.
struct EnumeratedElement {
    /// The element as the request body spells it.
    element: &'static str,
    /// Every value the schema accepts for it, in the order the schema lists
    /// them — which is the order the refusal names them in.
    values: &'static [&'static str],
}

/// Every enumerated element of this service's request schema.
///
/// `RequestInterval` is "the number of seconds between the time that Amazon
/// Route 53 gets a response from your endpoint and the time that it sends the
/// next health check request"
/// (smithy_query(query="doc",shape="HealthCheckConfig",member="RequestInterval")),
/// and a health check is configured with EITHER the standard cadence of thirty
/// seconds or the fast one of ten — the two values the service's own schema
/// violation names as its enumeration — rather than with any number between
/// them, which is all the modelled `@range min 10 max 30` can express.
const ENUMERATED_ELEMENTS: &[EnumeratedElement] = &[EnumeratedElement {
    element: "RequestInterval",
    values: &["10", "30"],
}];

/// Where one element's value sits in a request body, as a parser reports a
/// position: the 1-based line, and the 1-based column the value ENDS at.
struct Position {
    line: usize,
    column: usize,
}

/// The position `offset` (a byte index into `body`) is reported as, or `None`
/// when `offset` is no character boundary of `body` — which the caller below
/// never produces, and which is answered rather than panicked on, because this
/// runs on a body a caller chose.
///
/// A parser's position is the place it has read UP TO, so the column is one past
/// the last character of the value — the `<` that opens the element's end tag;
/// lines are counted so a body spread over several of them is reported on the
/// line the value ends on.
fn position_of(body: &str, offset: usize) -> Option<Position> {
    let consumed = body.get(..offset)?;
    let line = consumed.matches('\n').count() + 1;
    let line_start = consumed.rfind('\n').map_or(0, |at| at + 1);
    Some(Position {
        line,
        column: consumed.get(line_start..)?.chars().count() + 1,
    })
}

/// The schema violation `body` carries, worded as the service states one, or
/// `None` when every enumerated element it carries holds a value the schema
/// accepts.
///
/// The body is read with the XML reader the rest of this emulator reads restXml
/// with, one event at a time, so the element is found by its place in the
/// document rather than by looking at the text — and the position the refusal
/// names is the READER's own, the offset it has consumed up to when the value
/// ends, which is what a validating parser reports a facet violation at.
///
/// A body the reader cannot read is not this check's to refuse: the codec that
/// deserializes the request answers that, in its own words.
pub fn violation_in(body: &str) -> Option<String> {
    let mut reader = Reader::from_str(body);
    let mut inside: Option<&EnumeratedElement> = None;
    loop {
        let value = match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = start.local_name();
                inside = ENUMERATED_ELEMENTS
                    .iter()
                    .find(|enumerated| enumerated.element.as_bytes() == name.as_ref());
                continue;
            }
            Ok(Event::Text(text)) => text
                .xml10_content()
                .ok()
                .and_then(|decoded| {
                    quick_xml::escape::unescape(&decoded)
                        .ok()
                        .map(|value| value.trim().to_string())
                })
                .unwrap_or_default(),
            // A value may be spelled as a CDATA section, whose content is the
            // value VERBATIM: CDATA exists to carry bytes no entity resolution
            // should touch, so it is not unescaped.
            Ok(Event::CData(cdata)) => std::str::from_utf8(cdata.as_ref())
                .map(|text| text.trim().to_string())
                .unwrap_or_default(),
            // The value of an element ends with it, so nothing after its end tag
            // is judged against its value set.
            Ok(Event::End(_)) => {
                inside = None;
                continue;
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => continue,
        };
        let Some(enumerated) = inside else {
            continue;
        };
        if enumerated.values.contains(&value.as_str()) {
            continue;
        }
        let ends_at = usize::try_from(reader.buffer_position()).ok()?;
        let position = position_of(body, ends_at)?;
        return Some(format!(
            "Invalid XML ; javax.xml.stream.XMLStreamException: \
             org.xml.sax.SAXParseException; lineNumber: {}; columnNumber: {}; \
             cvc-enumeration-valid: Value '{value}' is not facet-valid with respect to \
             enumeration '[{}]'. It must be a value from the enumeration.",
            position.line,
            position.column,
            enumerated.values.join(", ")
        ));
    }
}

/// The refusal `req` earns from the request schema, or `None` when its body
/// satisfies it — which is every request with no body and every body that
/// carries no enumerated element.
///
/// Rendered through the service's own refusal renderer, so the document is the
/// `<ErrorResponse>` envelope every other refusal this service answers is
/// rendered in, naming the request it answers.
pub fn refusal(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let body = std::str::from_utf8(req.body().bytes()?).ok()?;
    let message = violation_in(body)?;
    // `Invalid` with no violation of its own: the STATUS (400) is the protocol's
    // for an input the request schema refused, and the code and message are this
    // service's, which `chosen` carries.
    Some(svc.definition().refusal_response(
        ProtocolError::Invalid(Vec::new()),
        Some(Refused {
            code: INVALID_INPUT.to_string(),
            message,
        }),
        req,
    ))
}

#[cfg(test)]
mod tests {
    //! The violation read off bodies, with the envelope and the operations it
    //! refuses covered end to end by the crate's integration suite.

    use super::*;

    /// A `CreateHealthCheck` body whose `RequestInterval` carries `interval`.
    fn body_with(interval: &str) -> String {
        format!(
            "<CreateHealthCheckRequest><CallerReference>ref</CallerReference>\
             <HealthCheckConfig><IPAddress>192.0.2.1</IPAddress><Port>80</Port>\
             <Type>HTTP</Type><RequestInterval>{interval}</RequestInterval>\
             </HealthCheckConfig></CreateHealthCheckRequest>"
        )
    }

    /// A value the schema enumerates passes; one it does not is refused, and the
    /// refusal names the value and the enumeration — whichever of the two ways a
    /// body may spell the value it arrived in.
    #[test]
    fn only_a_value_outside_the_enumeration_is_refused() {
        for accepted in ["10", "30", "<![CDATA[30]]>"] {
            assert_eq!(
                violation_in(&body_with(accepted)),
                None,
                "case {accepted:?}: an enumerated value must be accepted"
            );
        }
        for refused in ["9", "15", "31", "0", "-1"] {
            let violation = violation_in(&body_with(refused));
            let violation = violation.unwrap_or_else(|| {
                panic!("case {refused:?}: a value outside the enumeration must be refused")
            });
            assert!(
                violation.contains(&format!("Value '{refused}' is not facet-valid"))
                    && violation.contains("enumeration '[10, 30]'"),
                "case {refused:?}: expected the schema violation to name the value and the \
                 enumeration, got {violation:?}"
            );
        }
        // The same value in a CDATA section is the same value.
        let violation =
            violation_in(&body_with("<![CDATA[15]]>")).expect("a CDATA value must be judged too");
        assert!(
            violation.contains("Value '15' is not facet-valid"),
            "expected a CDATA-spelled value to be named, got {violation:?}"
        );
    }

    /// The position is the place the parser reached: the column the element's
    /// value ends at, on the line it ends on.
    #[test]
    fn the_position_is_where_the_elements_value_ends() {
        let body = body_with("15");
        let ends_at = body
            .find("</RequestInterval>")
            .expect("the body must carry the element");
        let violation = violation_in(&body).expect("the body must be refused");
        assert!(
            violation.contains(&format!("lineNumber: 1; columnNumber: {}", ends_at + 1)),
            "expected the column the value ends at ({}), got {violation:?}",
            ends_at + 1
        );

        // The same body spread over two lines is reported on the second.
        let multiline = body.replacen("<HealthCheckConfig>", "\n<HealthCheckConfig>", 1);
        let violation = violation_in(&multiline).expect("the body must be refused");
        assert!(
            violation.contains("lineNumber: 2;"),
            "expected the line the value ends on, got {violation:?}"
        );
    }

    /// A body with no enumerated element, and one with none at all, are left
    /// alone.
    #[test]
    fn a_body_carrying_no_enumerated_element_is_accepted() {
        for case in [
            "",
            "<CreateHostedZoneRequest><Name>example.com</Name></CreateHostedZoneRequest>",
            "<CreateHealthCheckRequest><HealthCheckConfig><Type>CALCULATED</Type>\
             </HealthCheckConfig></CreateHealthCheckRequest>",
        ] {
            assert_eq!(
                violation_in(case),
                None,
                "case {case:?}: a body with no enumerated element must be accepted"
            );
        }
    }
}
