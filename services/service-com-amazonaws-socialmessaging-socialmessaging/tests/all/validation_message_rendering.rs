// SPDX-License-Identifier: CC0-1.0

//! What a `ValidationException` SAYS, over `restJson1`, for a request that fails a
//! constraint the model declares.
//!
//! AWS builds this message from the request — `1 validation error detected: Value
//! '<value>' at '<member path>' failed to satisfy constraint: <requirement>` — so the
//! three rules below are the service's behaviour and not prose:
//!
//! 1. a `@pattern` requirement names the expression AWS reports, which is NOT the model
//!    literal: the outer `^`/`$` anchors are dropped, and an expression that is not
//!    outer-anchored is wrapped in `.*` … `.*`;
//! 2. the value is quoted back only when the service has it as TEXT and it is short
//!    enough to carry: a blob member (whose bytes have no text rendering) and a value
//!    past the quoting bound are stated as `Value at '<member path>' …` with no value at
//!    all — never as a byte count, which is a form AWS never emits;
//! 3. a list-valued member is quoted AS THE LIST — `[<element>, <element>, …]`, with a
//!    structure element rendered `<ShapeName>(<member>=<value>, …)` — never as an element
//!    count.
//!
//! Every row drives one emulator through the real router dispatch (the same path a
//! socket-served request takes) and asserts the whole message, so a clause that lost a
//! part of itself fails here.

use aws_smithy_types::base64;
use serde_json::{json, Value};

use crate::harness::{call, emulator, query_escape, ACCOUNT_ID};

/// A well-formed `LinkedWhatsAppBusinessAccountId`. It names no account: every row is
/// refused BEFORE a handler runs, so the only thing this value has to do is satisfy its
/// own member's constraints.
const WELL_FORMED_WABA: &str = "waba-00000000000000000000000000000000";

/// A well-formed `WhatsAppPhoneNumberId`, for the same reason.
const WELL_FORMED_PHONE: &str = "phone-number-id-00000000000000000000000000000000";

/// The longest value this service quotes back in a clause (`MAX_QUOTED_VALUE` in
/// `src/service.rs`): at it the value is quoted, one byte past it the clause states the
/// member alone.
const QUOTING_BOUND: usize = 512;

/// One request, and the whole message the service answers it with.
struct Case {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    expected: String,
}

/// The envelope a single-clause refusal carries, over the subject and the requirement.
fn one(subject: &str, requirement: &str) -> String {
    format!("1 validation error detected: {subject} failed to satisfy constraint: {requirement}")
}

/// The requirement a `@pattern` violation states, over the expression AWS reports.
fn pattern(rendered: &str) -> String {
    format!("Member must satisfy regular expression pattern: {rendered}")
}

/// The requirement a `@length` maximum states.
fn at_most(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// A GET whose only member is bound to the query string.
fn get(uri: String) -> (&'static str, String, Option<Value>) {
    ("GET", uri, None)
}

/// RULE 1: a `@pattern` requirement names the expression AWS reports — the outer anchors
/// dropped, and an expression that is not outer-anchored wrapped in `.*` … `.*`.
///
/// The rows are every `@pattern` this service's INPUT members declare, read from the
/// model (`smithy_query(query="shape", shape="Arn,WhatsAppDatasetId,MetaFlowId,
/// MetaTemplateId,WhatsAppMediaId,WhatsAppPhoneNumberId,LinkedWhatsAppBusinessAccountId,
/// KmsKeyArn,WhatsAppBusinessAccountEventDestination")`), so an expression in a form none
/// of them takes today cannot slip through unrendered: both outer-anchored and
/// alternating-over-anchored-branches are covered, and the two patterns on
/// `WhatsAppBusinessAccountEventDestination`'s members are reached through a LIST ELEMENT,
/// where the member path names the position.
#[tokio::test]
async fn a_pattern_violation_reports_the_expression_aws_reports() {
    let waba_pattern = ".*(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$).*";
    let phone_pattern =
        ".*(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$).*";
    let cases: Vec<Case> = vec![
        Case {
            case: "a waba id matching no branch of its pattern",
            method: "GET",
            uri: format!(
                "/v1/whatsapp/waba/details?id={}",
                query_escape("not-a-valid-waba-id")
            ),
            body: None,
            expected: one(
                "Value 'not-a-valid-waba-id' at 'id'",
                &pattern(waba_pattern),
            ),
        },
        Case {
            case: "a phone-number id member handed a waba id",
            method: "GET",
            uri: format!(
                "/v1/whatsapp/waba/phone/details?id={}",
                query_escape(WELL_FORMED_WABA)
            ),
            body: None,
            expected: one(
                &format!("Value '{WELL_FORMED_WABA}' at 'id'"),
                &pattern(phone_pattern),
            ),
        },
        Case {
            case: "an ARN without the arn: prefix",
            method: "GET",
            uri: format!(
                "/v1/tags/list?resourceArn={}",
                query_escape("not-an-arn")
            ),
            body: None,
            expected: one("Value 'not-an-arn' at 'resourceArn'", &pattern("arn:.*")),
        },
        Case {
            case: "a flow id that is not numeric",
            method: "GET",
            uri: format!(
                "/v1/whatsapp/flow?id={}&flowId={}",
                query_escape(WELL_FORMED_WABA),
                query_escape("not-numeric")
            ),
            body: None,
            expected: one("Value 'not-numeric' at 'flowId'", &pattern("[0-9]+")),
        },
        Case {
            case: "a meta template id that is not numeric",
            method: "GET",
            uri: format!(
                "/v1/whatsapp/template?id={}&metaTemplateId={}",
                query_escape(WELL_FORMED_WABA),
                query_escape("abc")
            ),
            body: None,
            expected: one("Value 'abc' at 'metaTemplateId'", &pattern("[0-9]+")),
        },
        Case {
            case: "a media id outside the alphanumeric set",
            method: "POST",
            uri: "/v1/whatsapp/media/get".to_string(),
            body: Some(json!({
                "mediaId": "bad id!",
                "originationPhoneNumberId": WELL_FORMED_PHONE,
            })),
            expected: one("Value 'bad id!' at 'mediaId'", &pattern("[A-Za-z0-9]+")),
        },
        Case {
            case: "a KMS key ARN that is not a KMS ARN",
            method: "PUT",
            uri: "/v1/whatsapp/business-public-key".to_string(),
            body: Some(json!({
                "originationPhoneNumberId": WELL_FORMED_PHONE,
                "kmsKeyArn": "arn:aws:s3:::a-bucket",
            })),
            expected: one(
                "Value 'arn:aws:s3:::a-bucket' at 'kmsKeyArn'",
                &pattern("arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:(key/.+|alias/.+)"),
            ),
        },
        Case {
            case: "an event-destination ARN inside a list element",
            method: "PUT",
            uri: "/v1/whatsapp/waba/eventdestinations".to_string(),
            body: Some(json!({
                "id": WELL_FORMED_WABA,
                "eventDestinations": [{ "eventDestinationArn": "nope" }],
            })),
            expected: one(
                "Value 'nope' at 'eventDestinations.1.member.eventDestinationArn'",
                &pattern("arn:.*:[a-z-]+([/:](.*))?"),
            ),
        },
        Case {
            case: "a role ARN that names no IAM role",
            method: "PUT",
            uri: "/v1/whatsapp/waba/eventdestinations".to_string(),
            body: Some(json!({
                "id": WELL_FORMED_WABA,
                "eventDestinations": [{
                    "eventDestinationArn": format!("arn:aws:sns:us-east-1:{ACCOUNT_ID}:topic-a"),
                    "roleArn": format!("arn:aws:iam::{ACCOUNT_ID}:user/not-a-role"),
                }],
            })),
            expected: one(
                &format!(
                    "Value 'arn:aws:iam::{ACCOUNT_ID}:user/not-a-role' at \
                     'eventDestinations.1.member.roleArn'"
                ),
                &pattern(r"arn:.*:iam::\d{12}:role\/[a-zA-Z0-9+=,.@\-_]+"),
            ),
        },
    ];

    run(cases).await;
}

/// RULE 2: the value is quoted back only when the service HAS it as text and it is short
/// enough to carry; otherwise the clause states the member and the requirement alone.
///
/// The rows span what the rule generalises over: a value's SIZE (inside the quoting
/// bound, exactly at it, one byte past it, far past it) and its TYPE (a blob, whose bytes
/// have no text rendering, at both ends of its own `@length` — a 0-byte blob is smaller
/// than any bound, so its clause omitting the value is about the type and not the size).
#[tokio::test]
async fn a_clause_quotes_the_value_only_when_it_has_it_and_can_carry_it() {
    // 116 characters: the shortest value past `LinkedWhatsAppBusinessAccountId`'s 115
    // maximum, and well inside the quoting bound.
    let over_long_id = format!("waba-{}", "0".repeat(111));
    let at_bound = format!("waba-{}", "0".repeat(QUOTING_BOUND - 5));
    let past_bound = format!("waba-{}", "0".repeat(QUOTING_BOUND - 4));
    let cases: Vec<Case> = vec![
        Case {
            case: "a 116-character id, inside the quoting bound",
            method: "GET",
            uri: format!(
                "/v1/whatsapp/waba/details?id={}",
                query_escape(&over_long_id)
            ),
            body: None,
            expected: one(
                &format!("Value '{over_long_id}' at 'id'"),
                &at_most(115),
            ),
        },
        Case {
            case: "a value exactly at the quoting bound",
            method: "GET",
            uri: format!("/v1/whatsapp/waba/details?id={}", query_escape(&at_bound)),
            body: None,
            expected: one(&format!("Value '{at_bound}' at 'id'"), &at_most(115)),
        },
        Case {
            case: "a value one byte past the quoting bound",
            method: "GET",
            uri: format!("/v1/whatsapp/waba/details?id={}", query_escape(&past_bound)),
            body: None,
            expected: one("Value at 'id'", &at_most(115)),
        },
        Case {
            case: "an oversize access token inside a nested structure",
            method: "POST",
            uri: "/v1/whatsapp/signup".to_string(),
            body: Some(json!({ "signupCallback": { "accessToken": "a".repeat(1001) } })),
            expected: one("Value at 'signupCallback.accessToken'", &at_most(1000)),
        },
        Case {
            case: "an oversize template definition, a blob member",
            method: "POST",
            uri: "/v1/whatsapp/template/put".to_string(),
            body: Some(json!({
                "id": WELL_FORMED_WABA,
                "templateDefinition": base64::encode("x".repeat(6115)),
            })),
            expected: one("Value at 'templateDefinition'", &at_most(6000)),
        },
        Case {
            case: "an empty template definition, a blob member below its minimum",
            method: "POST",
            uri: "/v1/whatsapp/template/put".to_string(),
            body: Some(json!({
                "id": WELL_FORMED_WABA,
                "templateDefinition": base64::encode(""),
            })),
            expected: one(
                "Value at 'templateDefinition'",
                "Member must have length greater than or equal to 1",
            ),
        },
    ];

    run(cases).await;
}

/// RULE 3: a list-valued member is quoted as the LIST — the elements in order,
/// comma-separated, in brackets, a structure element as
/// `<ShapeName>(<member>=<value>, …)` over the members that were present.
///
/// The rows differ in what the elements ARE: an enumerated string (the values come from
/// the model, `smithy_query(query="shape", shape="MetaFlowCategory")`) and a structure,
/// one of which omits its optional member so the rendering of a structure is over what
/// was sent rather than over the shape.
#[tokio::test]
async fn a_list_length_violation_quotes_the_list_it_was_sent() {
    // Every member of `MetaFlowCategory`, in the order the model declares them, plus one
    // repeat: 10 elements against a maximum of 9.
    let categories = [
        "SIGN_UP",
        "SIGN_IN",
        "APPOINTMENT_BOOKING",
        "LEAD_GENERATION",
        "SHOPPING",
        "CONTACT_US",
        "CUSTOMER_SUPPORT",
        "SURVEY",
        "OTHER",
        "OTHER",
    ];
    let topic_a = format!("arn:aws:sns:us-east-1:{ACCOUNT_ID}:topic-a");
    let topic_b = format!("arn:aws:sns:us-east-1:{ACCOUNT_ID}:topic-b");
    let role = format!("arn:aws:iam::{ACCOUNT_ID}:role/event-writer");
    let cases: Vec<Case> = vec![
        Case {
            case: "ten flow categories against a maximum of nine",
            method: "POST",
            uri: "/v1/whatsapp/flow/update".to_string(),
            body: Some(json!({
                "id": WELL_FORMED_WABA,
                "flowId": "1",
                "categories": categories,
            })),
            expected: one(
                &format!("Value '[{}]' at 'categories'", categories.join(", ")),
                "Member must have length less than or equal to 9",
            ),
        },
        Case {
            case: "two event destinations against a maximum of one",
            method: "PUT",
            uri: "/v1/whatsapp/waba/eventdestinations".to_string(),
            body: Some(json!({
                "id": WELL_FORMED_WABA,
                "eventDestinations": [
                    { "eventDestinationArn": topic_a, "roleArn": role },
                    { "eventDestinationArn": topic_b },
                ],
            })),
            expected: one(
                &format!(
                    "Value '[WhatsAppBusinessAccountEventDestination(\
                     eventDestinationArn={topic_a}, roleArn={role}), \
                     WhatsAppBusinessAccountEventDestination(\
                     eventDestinationArn={topic_b})]' at 'eventDestinations'"
                ),
                &at_most(1),
            ),
        },
    ];

    run(cases).await;
}

/// RULE: an ABSENT member keeps `Value null at '<path>'` — the `null` is a fact of the
/// request's shape rather than a rendering of anything the caller sent — and a numeric
/// member states the number it was sent.
#[tokio::test]
async fn a_clause_states_an_absent_member_and_a_numeric_bound() {
    let (method, uri, body) = get("/v1/whatsapp/waba/list?maxResults=0".to_string());
    let cases: Vec<Case> = vec![
        Case {
            case: "a required member the request never sent",
            method: "POST",
            uri: "/v1/whatsapp/flow/update".to_string(),
            body: Some(json!({ "id": WELL_FORMED_WABA })),
            expected: one("Value null at 'flowId'", "Member must not be null"),
        },
        Case {
            case: "a number below its range minimum",
            method,
            uri,
            body,
            expected: one(
                "Value '0' at 'maxResults'",
                "Member must have value greater than or equal to 1",
            ),
        },
    ];

    run(cases).await;
}

/// Drive every row against one emulator and assert the status, the error code and the
/// WHOLE message.
async fn run(cases: Vec<Case>) {
    let router = emulator();
    for Case {
        case,
        method,
        uri,
        body,
        expected,
    } in cases
    {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        let actual = answer.message();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}
