// SPDX-License-Identifier: CC0-1.0

//! What a refused event pattern, and an `Event` that is not an envelope, answer —
//! end to end over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target:
//! AWSEvents.<Operation>`) through the real `MultiServiceRouter`.
//!
//! The RULES asserted here:
//!
//! * a refusal of an event PATTERN is `InvalidEventPatternException` (HTTP 400)
//!   whose message is `Event pattern is not valid. Reason: <reason>` followed by
//!   the source location the validator reached, `\n at [Source: (String)"<the
//!   pattern exactly as submitted>"; line: <n>, column: <c>]` — every part of it
//!   derived from the request, the position being the one just past the text the
//!   validator had to read to find the fault;
//! * text that is not JSON at all is refused with the PARSE's own diagnostic: the
//!   character it stopped on and what was expected there, the whole word it does
//!   not recognise, or what was left unclosed when the text ran out — at the
//!   position the parse reports, whose column counts bytes;
//! * the same pattern answers the same message whichever operation was handed it,
//!   because one validator reads them all (`TestEventPattern`, `PutRule`,
//!   `CreateArchive`, `UpdateArchive`);
//! * `TestEventPattern`'s `Event` member is the event ENVELOPE, not the pattern,
//!   so an envelope the service cannot read is refused as a bad request member
//!   (`ValidationException`, HTTP 400) and only a bad `EventPattern` is an invalid
//!   event pattern;
//! * `exists: true` holds for a leaf whose value is JSON `null` — `null` is a
//!   value the event carries — while an object-valued field and an empty array
//!   carry no leaf and are absent.
//!
//! Each table spans its class rather than one case: the location rows differ along
//! which construct is at fault, how far into the pattern it sits, what kind of
//! node the validator stopped on (a string's opening quote, a whole list, a
//! scalar, a member name), how the pattern is spelled (one line or several,
//! multi-byte characters before the fault), and which operation was handed it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// One `awsJson1_1` call to this service.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A valid event envelope: every field `TestEventPattern$Event` makes mandatory
/// ("the following fields are mandatory: id account source time region resources
/// detail-type"), carrying `detail`.
fn envelope(detail: Value) -> String {
    json!({
        "version": "0",
        "id": "9c2f6f6a-6f0e-4a3e-9f1b-7a1d2c3b4e5f",
        "account": engine::emulator_account_id(),
        "source": "com.demo.orders",
        "time": "2026-03-04T10:00:00Z",
        "region": REGION,
        "resources": [],
        "detail-type": "Order Placed",
        "detail": detail,
    })
    .to_string()
}

/// The ARN of the bus an archive names as its source. The default bus of this
/// account is there without being created, and the pattern is validated before
/// any bus is looked up, so every row reaches the validator.
fn default_bus_arn() -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/default",
        engine::emulator_account_id()
    )
}

/// The whole message a refused pattern must answer with: the opening, the reason,
/// and the source location of the position just past `read` — the text the
/// validator had to read to find the fault.
///
/// The line and column are COMPUTED from that text rather than written down, so
/// each row states the rule (where the validator got to) instead of a number: the
/// column counts characters from the start of the line the position falls on.
fn expected_refusal(pattern: &str, reason: &str, read: &str) -> String {
    let line = 1 + read.matches('\n').count();
    let last = read.rsplit_once('\n').map_or(read, |(_, after)| after);
    let column = 1 + last.chars().count();
    format!(
        "Event pattern is not valid. Reason: {reason}\n at [Source: (String)\"{pattern}\"; \
         line: {line}, column: {column}]"
    )
}

/// One malformed-pattern row: the case, the pattern as submitted, the reason the
/// refusal states, and the PREFIX of the pattern the validator had to read before
/// it could state it — a string's opening quote for a fault its type alone shows,
/// the whole of a value for a fault read out of its content, and the first
/// character of a member's value for a fault about that member's NAME, which is
/// where reading the name leaves the reader.
struct Row {
    case: &'static str,
    pattern: &'static str,
    reason: &'static str,
    read: &'static str,
}

/// Every row below is checked against the pattern it belongs to, so a row that
/// does not describe its own pattern fails rather than passing vacuously.
fn location_rows() -> Vec<Row> {
    vec![
        // A field's value is a term list or a nested object; a bare scalar is
        // neither, which its opening token alone shows.
        Row {
            case: "a leaf value not wrapped in an array",
            pattern: r#"{"source": "com.demo.orders"}"#,
            reason: r#""source" must be an object or an array"#,
            read: r#"{"source": ""#,
        },
        Row {
            case: "a leaf number not wrapped in an array, two fields in",
            pattern: r#"{"source": ["com.demo.orders"], "detail-type": 7}"#,
            reason: r#""detail-type" must be an object or an array"#,
            read: r#"{"source": ["com.demo.orders"], "detail-type": 7"#,
        },
        Row {
            case: "a leaf value nested inside detail",
            pattern: r#"{"detail": {"status": "FULFILLED"}}"#,
            reason: r#""status" must be an object or an array"#,
            read: r#"{"detail": {"status": ""#,
        },
        // A numeric range whose bottom is not below its top admits no number at
        // all, which takes reading the whole list of pairs.
        Row {
            case: "a numeric range whose bottom is above its top",
            pattern: r#"{"detail": {"amount": [{"numeric": [">", 100, "<", 50]}]}}"#,
            reason: "Bottom must be less than top",
            read: r#"{"detail": {"amount": [{"numeric": [">", 100, "<", 50]"#,
        },
        Row {
            case: "a numeric range whose ends meet while one of them excludes",
            pattern: r#"{"detail": {"amount": [{"numeric": [">=", 42, "<", 42]}]}}"#,
            reason: "Bottom must be less than top",
            read: r#"{"detail": {"amount": [{"numeric": [">=", 42, "<", 42]"#,
        },
        // A match type the language does not define is found once the validator
        // has entered the argument that name introduces.
        Row {
            case: "a match type the language does not define",
            pattern: r#"{"source": ["com.demo.orders"], "detail": {"n": [{"nosuchop": [1]}]}}"#,
            reason: "Unrecognized match type nosuchop",
            read: r#"{"source": ["com.demo.orders"], "detail": {"n": [{"nosuchop": ["#,
        },
        Row {
            case: "a match type spelled the way another language spells prefix",
            pattern: r#"{"detail": {"k": [{"startswith": "ok"}]}}"#,
            reason: "Unrecognized match type startswith",
            read: r#"{"detail": {"k": [{"startswith": ""#,
        },
        Row {
            case: "a match type nested under anything-but that it cannot negate",
            pattern: r#"{"detail": {"amount": [{"anything-but": {"numeric": [">", 100]}}]}}"#,
            reason: "Unsupported anything-but pattern: numeric",
            read: r#"{"detail": {"amount": [{"anything-but": {"numeric": ["#,
        },
        Row {
            case: "an exists nested under anything-but, which it cannot negate either",
            pattern: r#"{"detail": {"k": [{"anything-but": {"exists": true}}]}}"#,
            reason: "Unsupported anything-but pattern: exists",
            read: r#"{"detail": {"k": [{"anything-but": {"exists": true"#,
        },
        // A pattern is a filter over an event's fields, so it is an object.
        Row {
            case: "a pattern that is a JSON list",
            pattern: r#"["com.demo.orders"]"#,
            reason: "Filter is not an object",
            read: "[",
        },
        Row {
            case: "a pattern that is a bare string",
            pattern: r#""com.demo.orders""#,
            reason: "Filter is not an object",
            read: "\"",
        },
        // A list with no member, and an object with no member.
        Row {
            case: "a field whose term list is empty",
            pattern: r#"{"detail": {"k": []}}"#,
            reason: "Empty arrays are not allowed",
            read: r#"{"detail": {"k": []"#,
        },
        Row {
            case: "a nested pattern object naming no field",
            pattern: r#"{"source": ["com.demo.orders"], "detail": {}}"#,
            reason: "Empty objects are not allowed",
            read: r#"{"source": ["com.demo.orders"], "detail": {}"#,
        },
        // A mask as wide as the address family: read by the ADDRESS parser, so
        // the reason stands alone (see the table below). Text that is no block
        // at all is this operator's own argument check, which is placed like
        // every other one.
        Row {
            case: "a cidr operand that names no block at all",
            pattern: r#"{"detail": {"ip": [{"cidr": "10.0.0.0/notanumber"}]}}"#,
            reason: "detail.ip: cidr takes an IPv4 or IPv6 CIDR block",
            read: r#"{"detail": {"ip": [{"cidr": ""#,
        },
        Row {
            case: "a cidr operand that is not even text",
            pattern: r#"{"detail": {"ip": [{"cidr": 10}]}}"#,
            reason: "detail.ip: cidr takes an IPv4 or IPv6 CIDR block",
            read: r#"{"detail": {"ip": [{"cidr": 10"#,
        },
        // Two wildcard characters in a row, read out of the value's text.
        Row {
            case: "a wildcard value carrying two characters in a row",
            pattern: r#"{"detail": {"k": [{"wildcard": "a**b"}]}}"#,
            reason: "Consecutive wildcard characters at pos 2",
            read: r#"{"detail": {"k": [{"wildcard": "a**b""#,
        },
        // A numeric comparison the operator set does not hold, reported once the
        // operator and the bound it compares against have both been read — the
        // list is read pair by pair; a bound of the wrong type, shown by its
        // opening token.
        Row {
            case: "a numeric comparison the operator set does not hold",
            pattern: r#"{"detail": {"n": [{"numeric": ["!=", 1]}]}}"#,
            reason: "Unrecognized numeric range operator: !=",
            read: r#"{"detail": {"n": [{"numeric": ["!=", 1"#,
        },
        Row {
            case: "a numeric comparison the operator set does not hold, with no bound after it",
            pattern: r#"{"detail": {"n": [{"numeric": ["!="]}]}}"#,
            reason: "Unrecognized numeric range operator: !=",
            read: r#"{"detail": {"n": [{"numeric": ["!="]"#,
        },
        Row {
            case: "a numeric bound that is text",
            pattern: r#"{"detail": {"n": [{"numeric": [">", "1"]}]}}"#,
            reason: "Value of > must be numeric",
            read: r#"{"detail": {"n": [{"numeric": [">", ""#,
        },
        Row {
            case: "a numeric pair the list stops short of a bound for",
            pattern: r#"{"detail": {"n": [{"numeric": [">", 1, ">="]}]}}"#,
            reason: "Value of >= must be numeric",
            read: r#"{"detail": {"n": [{"numeric": [">", 1, ">="]"#,
        },
        // `exists` takes a boolean, whichever other kind of value is given.
        Row {
            case: "exists given the text of a boolean",
            pattern: r#"{"detail": {"k": [{"exists": "false"}]}}"#,
            reason: "exists match pattern must be either true or false.",
            read: r#"{"detail": {"k": [{"exists": ""#,
        },
        Row {
            case: "exists given a number, which is one whole token",
            pattern: r#"{"detail": {"k": [{"exists": 1}]}}"#,
            reason: "exists match pattern must be either true or false.",
            read: r#"{"detail": {"k": [{"exists": 1"#,
        },
        // A match expression names ONE match type, so the validator stops on
        // the second one it names: reading that member's name consumes the
        // colon after it and the first character of its argument too, so the
        // position is one character into that argument.
        Row {
            case: "a match expression carrying two match types",
            pattern: r#"{"detail": {"k": [{"prefix": "a", "suffix": "b"}]}}"#,
            reason: "Only one key allowed in match expression",
            read: r#"{"detail": {"k": [{"prefix": "a", "suffix": ""#,
        },
        Row {
            // The column counts CHARACTERS, so a multi-byte character before the
            // position counts once.
            case: "a match expression whose first argument is multi-byte",
            pattern: r#"{"detail": {"k": [{"prefix": "é", "suffix": "b"}]}}"#,
            reason: "Only one key allowed in match expression",
            read: r#"{"detail": {"k": [{"prefix": "é", "suffix": ""#,
        },
        Row {
            // The position is a fact about the TEXT, so a pattern spelled over
            // several lines reports the line the fault falls on.
            case: "a pattern spelled over several lines",
            pattern: "{\n  \"detail\": {\n    \"k\": \"ok\"\n  }\n}",
            reason: r#""k" must be an object or an array"#,
            read: "{\n  \"detail\": {\n    \"k\": \"",
        },
        Row {
            case: "a pattern whose fault falls on the first line of several",
            pattern: "{\"source\": \"com.demo.orders\",\n \"detail\": {\"k\": [\"ok\"]}}",
            reason: r#""source" must be an object or an array"#,
            read: "{\"source\": \"",
        },
        // Text that is not JSON at all is refused by the JSON READER, which
        // states the character it stopped on and what it was expecting there.
        Row {
            case: "a word where a member name was due",
            pattern: "{ bad json",
            reason: "Unexpected character ('b' (code 98)): was expecting double-quote to start \
                     field name",
            read: "{ ",
        },
        Row {
            case: "a member name with no colon after it",
            pattern: r#"{"source" ["com.demo.orders"]}"#,
            reason: "Unexpected character ('[' (code 91)): was expecting a colon to separate \
                     field name and value",
            read: r#"{"source" "#,
        },
        Row {
            case: "two members with no comma between them",
            pattern: r#"{"source": ["a"] "detail-type": ["b"]}"#,
            reason: "Unexpected character ('\"' (code 34)): was expecting comma to separate \
                     Object entries",
            read: r#"{"source": ["a"] "#,
        },
        Row {
            case: "two list elements with no comma between them",
            pattern: r#"{"source": ["a" "b"]}"#,
            reason: "Unexpected character ('\"' (code 34)): was expecting comma to separate \
                     Array entries",
            read: r#"{"source": ["a" "#,
        },
        Row {
            case: "a member with no value at all",
            pattern: r#"{"source": }"#,
            reason: "Unexpected character ('}' (code 125)): was expecting a valid value (JSON \
                     String, Number, Array, Object or token 'null', 'true' or 'false')",
            read: r#"{"source": "#,
        },
        Row {
            // A word where a value belongs is named WHOLE, so the position is
            // the one the word STARTS at.
            case: "a token the language does not define, inside a term list",
            pattern: r#"{"source": [nul]}"#,
            reason: "Unrecognized token 'nul': was expecting (JSON String, Number, Array, Object \
                     or token 'null', 'true' or 'false')",
            read: r#"{"source": ["#,
        },
        Row {
            case: "a pattern that is one bare word",
            pattern: "not json",
            reason: "Unrecognized token 'not': was expecting (JSON String, Number, Array, Object \
                     or token 'null', 'true' or 'false')",
            read: "",
        },
    ]
}

/// Every refusal of a pattern states the reason for its class and then the source
/// location the validator reached in the pattern as submitted.
#[tokio::test]
async fn a_refused_pattern_states_the_reason_and_where_in_the_pattern_it_stopped() {
    let router = emulator();
    let event = envelope(json!({ "k": "ok", "n": 1, "ip": "10.0.1.7" }));
    for row in location_rows() {
        let Row {
            case,
            pattern,
            reason,
            read,
        } = row;
        assert!(
            pattern.starts_with(read),
            "case {case:?}: the row's read prefix {read:?} must be a prefix of {pattern:?}"
        );
        let expected = expected_refusal(pattern, reason, read);
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({ "EventPattern": pattern, "Event": event }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidEventPatternException",
            "case {case:?}: expected InvalidEventPatternException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal to state {expected:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// A pattern whose text ENDS before the JSON it begins is complete names what
/// the parse was still reading when it ran out of text: the collection or string
/// whose close marker never came.
///
/// The rows span the class: an object left open, the list inside it left open,
/// and a string — a value's and a member name's — left unterminated.
#[tokio::test]
async fn a_pattern_that_ends_mid_json_names_what_was_left_unclosed() {
    let router = emulator();
    let event = envelope(json!({ "k": "ok" }));
    // (case, the pattern, the reason the refusal states)
    let rows: [(&str, &str, &str); 4] = [
        (
            "an object left open",
            r#"{"source": ["a"]"#,
            "Unexpected end-of-input: expected close marker for Object",
        ),
        (
            "the list inside it left open",
            r#"{"source": ["a""#,
            "Unexpected end-of-input: expected close marker for Array",
        ),
        (
            "a value string left unterminated",
            r#"{"source": ["a"#,
            "Unexpected end-of-input: expected close marker for String",
        ),
        (
            "a member name left unterminated",
            r#"{"sou"#,
            "Unexpected end-of-input: expected close marker for String",
        ),
    ];

    for (case, pattern, reason) in rows {
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({ "EventPattern": pattern, "Event": event }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidEventPatternException",
            "case {case:?}: expected InvalidEventPatternException, got {body}"
        );
        // The parse ran out of text, so the position it reports is the end of
        // the pattern it was given, counted in bytes like every column it
        // reports.
        let expected = format!(
            "Event pattern is not valid. Reason: {reason}\n at [Source: (String)\"{pattern}\"; \
             line: 1, column: {}]",
            pattern.len()
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal to state {expected:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// The position an UNREADABLE pattern reports is the parse's own, and a parser
/// counts a column in BYTES, so a multi-byte character before the fault counts
/// as the bytes it occupies rather than as one character.
#[tokio::test]
async fn an_unreadable_pattern_reports_the_position_the_parse_stopped_at() {
    let router = emulator();
    let event = envelope(json!({ "k": "ok" }));
    // A member name with no colon after it, with a multi-byte character in the
    // text the parse read to get there.
    let pattern = "{\"d\u{e9}tail\": {\"k\" [\"ok\"]}}";
    let read = "{\"d\u{e9}tail\": {\"k\" ";
    assert!(
        pattern.starts_with(read),
        "the prefix {read:?} must be a prefix of {pattern:?}"
    );
    let expected = format!(
        "Event pattern is not valid. Reason: Unexpected character ('[' (code 91)): was expecting \
         a colon to separate field name and value\n at [Source: (String)\"{pattern}\"; line: 1, \
         column: {}]",
        read.len() + 1
    );
    assert!(
        read.len() > read.chars().count(),
        "the row only states the byte convention if the prefix is multi-byte, got {read:?}"
    );

    let (status, body) = events(
        &router,
        "TestEventPattern",
        &json!({ "EventPattern": pattern, "Event": event }),
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidEventPatternException",
        "expected InvalidEventPatternException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        expected,
        "expected the refusal to state {expected:?}, got {:?}",
        error_message(&body)
    );
}

/// A `cidr` operand is read by the ADDRESS parser rather than by the pattern
/// reader, so what that parser says about the text is the whole refusal: the
/// reason stands alone, with no source location after it.
///
/// The rows span the operand class: a mask as wide as each address family, a mask
/// wider than it, and — on the other side of the bound — the narrowest and widest
/// masks the notation does admit, which are accepted.
#[tokio::test]
async fn a_cidr_operand_the_address_parser_refuses_states_the_reason_alone() {
    let router = emulator();
    let event = envelope(json!({ "ip": "10.0.1.7" }));
    // (case, the pattern, the whole message the refusal carries)
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a mask as wide as the IPv4 family",
            json!({ "detail": { "ip": [{ "cidr": "10.0.1.7/32" }] } }).to_string(),
            "Event pattern is not valid. Reason: IPv4 mask bits must be < 32".to_string(),
        ),
        (
            "a mask wider than the IPv4 family",
            json!({ "detail": { "ip": [{ "cidr": "10.0.0.0/33" }] } }).to_string(),
            "Event pattern is not valid. Reason: IPv4 mask bits must be < 32".to_string(),
        ),
        (
            "a mask as wide as the IPv6 family",
            json!({ "detail": { "ip": [{ "cidr": "2001:db8::1/128" }] } }).to_string(),
            "Event pattern is not valid. Reason: IPv6 mask bits must be < 128".to_string(),
        ),
        (
            "a mask wider than the IPv6 family",
            json!({ "detail": { "ip": [{ "cidr": "2001:db8::/129" }] } }).to_string(),
            "Event pattern is not valid. Reason: IPv6 mask bits must be < 128".to_string(),
        ),
    ];
    for (case, pattern, expected) in refused {
        for operation in ["TestEventPattern", "PutRule"] {
            let request = if operation == "TestEventPattern" {
                json!({ "EventPattern": pattern, "Event": event })
            } else {
                json!({ "Name": "probe", "EventPattern": pattern })
            };
            let (status, body) = events(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to answer HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidEventPatternException",
                "case {case:?}: expected {operation} to answer InvalidEventPatternException, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected {operation} to state {expected:?} and nothing after it, \
                 got {:?}",
                error_message(&body)
            );
        }
    }

    // One bit narrower than each family, and the mask that covers every address,
    // are blocks the notation admits: the refusals above reach no valid operand.
    for (case, pattern) in [
        (
            "one bit narrower than the IPv4 family",
            json!({ "detail": { "ip": [{ "cidr": "10.0.1.6/31" }] } }),
        ),
        (
            "every IPv4 address",
            json!({ "detail": { "ip": [{ "cidr": "0.0.0.0/0" }] } }),
        ),
        (
            "one bit narrower than the IPv6 family",
            json!({ "detail": { "ip": [{ "cidr": "2001:db8::/127" }] } }),
        ),
        (
            "every IPv6 address",
            json!({ "detail": { "ip": [{ "cidr": "::/0" }] } }),
        ),
    ] {
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({ "EventPattern": pattern.to_string(), "Event": event }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the operand to be admitted, got {status} {body}"
        );
    }
}

/// One validator reads every pattern this service is handed, so the refusal — the
/// reason AND the source location — is the same whichever operation took it.
#[tokio::test]
async fn every_operation_handed_a_pattern_answers_the_same_refusal() {
    let router = emulator();
    let event = envelope(json!({ "k": "ok", "n": 1, "ip": "10.0.1.7" }));
    // An archive to aim `UpdateArchive` at, created with a pattern the language
    // admits so only the refused update is in question.
    let (status, body) = events(
        &router,
        "CreateArchive",
        &json!({
            "ArchiveName": "refusals",
            "EventSourceArn": default_bus_arn(),
            "EventPattern": r#"{"source": ["com.demo.orders"]}"#,
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateArchive answered {status} {body}");

    for row in location_rows() {
        let Row {
            case,
            pattern,
            reason,
            read,
        } = row;
        let expected = expected_refusal(pattern, reason, read);
        // (the operation, the request carrying the pattern)
        let calls: [(&str, Value); 4] = [
            (
                "TestEventPattern",
                json!({ "EventPattern": pattern, "Event": event }),
            ),
            ("PutRule", json!({ "Name": "probe", "EventPattern": pattern })),
            (
                "CreateArchive",
                json!({
                    "ArchiveName": "probe",
                    "EventSourceArn": default_bus_arn(),
                    "EventPattern": pattern,
                }),
            ),
            (
                "UpdateArchive",
                json!({ "ArchiveName": "refusals", "EventPattern": pattern }),
            ),
        ];
        for (operation, request) in calls {
            let (status, body) = events(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to answer HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidEventPatternException",
                "case {case:?}: expected {operation} to answer InvalidEventPatternException, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected {operation} to state {expected:?}, got {:?}",
                error_message(&body)
            );
        }
    }

    // The archive kept the pattern it was created with: every refusal above
    // stored nothing.
    let (status, body) = events(&router, "DescribeArchive", &json!({ "ArchiveName": "refusals" })).await;
    assert_eq!(status, 200, "DescribeArchive answered {status} {body}");
    assert_eq!(
        body["EventPattern"],
        json!(r#"{"source": ["com.demo.orders"]}"#),
        "a refused update must leave the archive's pattern in place, got {body}"
    );
}

/// `Event` is the event ENVELOPE, so an envelope the service cannot read is a bad
/// request member — `ValidationException` — while only a bad `EventPattern` is an
/// invalid event pattern. The rows span the envelope's own rules: each mandatory
/// field missing in turn, a `time` that is not an instant (which the refusal names,
/// because `time` is the one member whose value is read), text that is not an
/// object, and text that is not JSON at all.
#[tokio::test]
async fn an_event_that_is_not_an_envelope_is_refused_as_the_request_member_it_is() {
    let router = emulator();
    let pattern = r#"{"source": ["com.demo.orders"]}"#;
    // The fields `TestEventPattern$Event` makes mandatory and the service
    // refuses an envelope without, each dropped in turn.
    let mandatory = ["id", "account", "source", "time", "region", "detail-type"];
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for field in mandatory {
        let mut event: Value = serde_json::from_str(&envelope(json!({ "k": "ok" })))
            .expect("the envelope this test builds is JSON");
        event
            .as_object_mut()
            .expect("the envelope is a JSON object")
            .remove(field);
        rows.push((
            format!("an envelope without {field:?}"),
            event.to_string(),
            "Parameter Event is not valid.".to_string(),
        ));
    }
    let mut unparseable: Value = serde_json::from_str(&envelope(json!({ "k": "ok" })))
        .expect("the envelope this test builds is JSON");
    unparseable
        .as_object_mut()
        .expect("the envelope is a JSON object")
        .insert("time".to_string(), json!("the day before yesterday"));
    // `time` is the one member whose VALUE the service reads, so the refusal
    // names what it could not read.
    rows.push((
        "an envelope whose time is not an instant".to_string(),
        unparseable.to_string(),
        "Parameter Event is not valid. Reason: Invalid time.".to_string(),
    ));
    let mut not_a_time: Value = serde_json::from_str(&envelope(json!({ "k": "ok" })))
        .expect("the envelope this test builds is JSON");
    not_a_time
        .as_object_mut()
        .expect("the envelope is a JSON object")
        .insert("time".to_string(), json!(1_767_225_600_i64));
    rows.push((
        "an envelope whose time is a number rather than an instant".to_string(),
        not_a_time.to_string(),
        "Parameter Event is not valid. Reason: Invalid time.".to_string(),
    ));
    rows.push((
        "an event that is a JSON list".to_string(),
        json!(["com.demo.orders"]).to_string(),
        "Parameter Event is not valid.".to_string(),
    ));
    rows.push((
        "an event that is not JSON at all".to_string(),
        "{not an event".to_string(),
        "Parameter Event is not valid.".to_string(),
    ));

    for (case, event, expected) in rows {
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({ "EventPattern": pattern, "Event": event }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: an unreadable Event is a bad request member, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal to state {expected:?}, got {:?}",
            error_message(&body)
        );
    }

    // And the pattern member is still its own refusal: a valid envelope with a
    // pattern the language does not admit is the invalid event pattern.
    let (status, body) = events(
        &router,
        "TestEventPattern",
        &json!({
            "EventPattern": r#"{"source": "com.demo.orders"}"#,
            "Event": envelope(json!({ "k": "ok" })),
        }),
    )
    .await;
    assert_eq!(status, 400, "a bad pattern answered {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidEventPatternException",
        "a bad EventPattern is the invalid event pattern, got {body}"
    );

    // An envelope that omits `resources` is read as one listing none, so it is
    // matched rather than refused — and a `resources` clause then matches
    // nothing, exactly as the member spelled out as `[]` does.
    let mut without_resources: Value = serde_json::from_str(&envelope(json!({ "k": "ok" })))
        .expect("the envelope this test builds is JSON");
    without_resources
        .as_object_mut()
        .expect("the envelope is a JSON object")
        .remove("resources");
    for (case, asked, expected) in [
        ("the event's own source", json!({ "source": ["com.demo.orders"] }), true),
        (
            "a resources clause against an envelope listing none",
            json!({ "resources": ["arn:aws:s3:::demo-bucket"] }),
            false,
        ),
    ] {
        let (status, body) = events(
            &router,
            "TestEventPattern",
            &json!({
                "EventPattern": asked.to_string(),
                "Event": without_resources.to_string(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an envelope without resources is matched, got {status} {body}"
        );
        assert_eq!(
            body["Result"],
            json!(expected),
            "case {case:?}: expected Result {expected}, got {body}"
        );
    }
}

/// `exists` is about PRESENCE: a leaf whose value is JSON `null` is present, so
/// `exists: true` holds for it and `exists: false` does not — while a field whose
/// value is an object, or an array carrying no leaf, is absent for `exists`.
///
/// The rows span the kinds of value a field can hold: every scalar (`null`
/// included), the empty string, an object, a list with a leaf in it, a list with
/// none, a list of nulls, a field the event does not carry at all, and a field
/// whose PARENT it does not carry.
#[tokio::test]
async fn exists_counts_a_null_leaf_as_present_and_an_object_or_empty_list_as_absent() {
    let router = emulator();
    // (case, the detail the event carries, the field the pattern asks about,
    //  whether that field exists)
    let rows: Vec<(&str, Value, &str, bool)> = vec![
        ("a null leaf", json!({ "items": null }), "items", true),
        ("a string leaf", json!({ "items": "one" }), "items", true),
        ("an empty string leaf", json!({ "items": "" }), "items", true),
        ("a number leaf", json!({ "items": 0 }), "items", true),
        ("a boolean leaf", json!({ "items": false }), "items", true),
        (
            "a list carrying a leaf",
            json!({ "items": ["one"] }),
            "items",
            true,
        ),
        (
            "a list carrying a null, which is a value",
            json!({ "items": [null] }),
            "items",
            true,
        ),
        ("an empty list", json!({ "items": [] }), "items", false),
        (
            "an object-valued field",
            json!({ "items": { "one": 1 } }),
            "items",
            false,
        ),
        (
            "a list carrying only objects",
            json!({ "items": [{ "one": 1 }] }),
            "items",
            false,
        ),
        (
            "a field the event does not carry",
            json!({ "other": "one" }),
            "items",
            false,
        ),
    ];

    for (case, detail, field, present) in rows {
        let event = envelope(detail);
        for asked in [true, false] {
            let pattern = json!({ "detail": { field: [{ "exists": asked }] } }).to_string();
            let (status, body) = events(
                &router,
                "TestEventPattern",
                &json!({ "EventPattern": pattern, "Event": event }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            let expected = asked == present;
            assert_eq!(
                body["Result"],
                json!(expected),
                "case {case:?}: exists:{asked} on a field that {} must answer {expected}, got \
                 {body}",
                if present { "is present" } else { "is absent" }
            );
        }
    }
}
