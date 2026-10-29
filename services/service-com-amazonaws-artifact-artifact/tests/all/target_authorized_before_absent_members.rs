// SPDX-License-Identifier: CC0-1.0

//! The ORDER the front door judges one request in, where the request is incomplete.
//!
//! The rule, over the whole class of requests rather than any one of them: **the resource
//! a request targets is authorized before anything else the request carried is judged**,
//! so an identifier that names no resource of its kind is refused 401
//! `UnauthorizedException` whichever other members of the operation's input the request
//! carries or leaves out — including a member the modelled shape declares `@required`.
//! Absence is one more thing the request carried (nothing) at one more member, judged
//! after the target like any value it did carry.
//!
//! The second half of the rule is that the order is ONLY an order: once the target is
//! authorized, the absent required member is still refused, in whichever envelope its
//! binding belongs to — the gateway's 400 `BadRequestException` for a body-bound member,
//! the service's own 400 `ValidationException` for one the URI carries. Either way the
//! request is JUDGED: a request that addresses a modelled operation is never answered as
//! an operation this emulator does not serve, however incomplete it is.
//!
//! Which members a row can leave out comes from the model
//! (`smithy_query(query="operation", operation="GetReport")` and
//! `…operation="PutComplianceInquiryFeedback"`): `GetReport$termToken` (`@required`,
//! `@httpQuery`) and `PutComplianceInquiryFeedback$rating` (`@required`, body-bound) are
//! the second required members of the operations that also name a target, so they are the
//! two members a request can omit while still naming one.
//!
//! Every case goes through the router, so each exercises the path a client takes:
//! routing, the front door, deserialization, validation and the handler.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, Answer, CATALOG};

/// The front door's authorization refusal.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");
/// The front door's request-body refusal.
const INVALID_BODY: (u16, &str, &str) = (400, "BadRequestException", "Invalid request body");

/// `ValidationException`'s `reason` for a member the request's values failed.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// An `InquiryId` satisfying `^compliance-inquiry-[a-zA-Z0-9]{16}$`: a target the front
/// door can parse, so a row using it is judged on what else it carried. It names no
/// stored inquiry, which no row here depends on. Spelled out rather than taken from the
/// crate, so a wrong predicate in `src/state.rs` cannot make this suite pass.
const WELL_FORMED_INQUIRY: &str = "compliance-inquiry-0123456789abcdef";

/// Identifiers that satisfy no resource-id syntax of this service, so each names nothing
/// the front door could authorize. The rows differ along what both id patterns
/// (`^report-[a-zA-Z0-9]{16}$`, `^compliance-inquiry-[a-zA-Z0-9]{16}$`) generalise over:
/// the prefix, the suffix length and the suffix alphabet.
const MALFORMED_IDS: &[(&str, &str)] = &[
    ("free text", "not-a-valid-id"),
    ("no prefix", "0123456789abcdef"),
    ("the other kind's prefix", "compliance-inquiry-abc"),
    ("suffix too short", "report-abc"),
    ("suffix too long", "report-0123456789abcdef0"),
    ("suffix outside the alphabet", "report-0123456789abcde_"),
    ("empty", ""),
];

/// One request a row sends: the operation, the method and URI the model binds it to, and
/// the body it carries (`None` for an operation whose input is all URI-bound).
struct Row {
    operation: &'static str,
    /// Which other `@required` member of the input this row leaves out, for the case
    /// name — `"nothing"` for a row carrying every one of them.
    omits: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation whose input names a target, at its modelled URI, with `id` as that
/// target — each with and without the other required member it has, and with and without
/// optional members, because what the rest of the request carries is what the rule
/// generalises over.
fn rows(id: &str) -> Vec<Row> {
    vec![
        Row {
            operation: "GetReportMetadata",
            omits: "nothing",
            method: "GET",
            uri: format!("/v1/report/getMetadata?reportId={id}"),
            body: None,
        },
        Row {
            operation: "GetReportMetadata",
            omits: "nothing, with the optional reportVersion",
            method: "GET",
            uri: format!("/v1/report/getMetadata?reportId={id}&reportVersion=2"),
            body: None,
        },
        Row {
            operation: "ListReportVersions",
            omits: "nothing",
            method: "GET",
            uri: format!("/v1/report/listVersions?reportId={id}"),
            body: None,
        },
        Row {
            operation: "GetTermForReport",
            omits: "nothing",
            method: "GET",
            uri: format!("/v1/report/getTermForReport?reportId={id}"),
            body: None,
        },
        Row {
            operation: "GetReport",
            omits: "nothing",
            method: "GET",
            uri: format!("/v1/report/get?reportId={id}&termToken=never-issued"),
            body: None,
        },
        // The URI-bound required member left out: the target is still what decides.
        Row {
            operation: "GetReport",
            omits: "termToken",
            method: "GET",
            uri: format!("/v1/report/get?reportId={id}"),
            body: None,
        },
        Row {
            operation: "GetReport",
            omits: "termToken, with the optional reportVersion",
            method: "GET",
            uri: format!("/v1/report/get?reportId={id}&reportVersion=2"),
            body: None,
        },
        Row {
            operation: "GetComplianceInquiryMetadata",
            omits: "nothing",
            method: "GET",
            uri: format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
            body: None,
        },
        Row {
            operation: "ListComplianceInquiryQueries",
            omits: "nothing",
            method: "GET",
            uri: format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
            body: None,
        },
        // A body-bound target, with and without the other body members it admits.
        Row {
            operation: "ExportComplianceInquiry",
            omits: "nothing",
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Some(json!({ "complianceInquiryId": id })),
        },
        Row {
            operation: "ExportComplianceInquiry",
            omits: "nothing, with the optional queryIdentifiers and includeCitations",
            method: "POST",
            uri: "/v1/compliance-inquiry/export".to_string(),
            body: Some(json!({
                "complianceInquiryId": id,
                "queryIdentifiers": [1, 2],
                "includeCitations": true
            })),
        },
        Row {
            operation: "PutComplianceInquiryFeedback",
            omits: "nothing",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({ "complianceInquiryId": id, "rating": "THUMBS_UP" })),
        },
        // The body-bound required member left out.
        Row {
            operation: "PutComplianceInquiryFeedback",
            omits: "rating",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({ "complianceInquiryId": id })),
        },
        Row {
            operation: "PutComplianceInquiryFeedback",
            omits: "rating, with other body members around the id",
            method: "PUT",
            uri: "/v1/compliance-inquiry/putFeedback".to_string(),
            body: Some(json!({
                "clientToken": "token-1",
                "comment": "please review",
                "complianceInquiryId": id,
                "reasonCodes": ["INCORRECT"]
            })),
        },
    ]
}

/// Send one row.
async fn send(router: &MultiServiceRouter, row: &Row) -> Answer {
    let payload = row
        .body
        .as_ref()
        .map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
    call(router, row.method, &row.uri, payload).await
}

/// The error body's `message`, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// Assert that `answer` is the refusal `expected`: status, wire error code and message.
fn assert_refused(case: &str, answer: &Answer, expected: (u16, &str, &str)) {
    let (status, code, text) = expected;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        message(answer),
        text,
        "case {case:?}: expected message {text:?}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_unauthorizable_target_is_refused_whatever_else_the_request_omits() {
    let router = emulator();

    for (id_case, id) in MALFORMED_IDS {
        for row in rows(id) {
            let case = format!(
                "{} targeting a {id_case} id, omitting {}",
                row.operation, row.omits
            );
            let answer = send(&router, &row).await;
            assert_refused(&case, &answer, UNAUTHORIZED);
        }
    }
}

#[tokio::test]
async fn a_required_member_left_out_is_still_refused_once_the_target_is_authorized() {
    let router = emulator();
    let published = CATALOG[0].id;

    // A URI-bound required member the request omits: the service's own modelled refusal,
    // naming the member in the message and in `fieldList`. The rows differ along which
    // member is left out and whether the operation names a target at all.
    let uri_bound = [
        (
            "GetReport on a published report, omitting termToken",
            "GET",
            format!("/v1/report/get?reportId={published}"),
            "termToken",
        ),
        (
            "GetReportMetadata omitting the reportId that names its target",
            "GET",
            "/v1/report/getMetadata".to_string(),
            "reportId",
        ),
        (
            "UntagResource omitting tagKeys",
            "DELETE",
            "/tags/tagged-thing".to_string(),
            "tagKeys",
        ),
    ];
    for (case, method, uri, member) in uri_bound {
        let answer = call(&router, method, &uri, bytes::Bytes::new()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: a URI-bound member is the service's own to refuse, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
            answer.body
        );
        assert!(
            message(&answer).contains(member),
            "case {case:?}: the message must name {member}, got {:?}",
            answer.body
        );
        let named: Vec<&str> = answer.body["fieldList"]
            .as_array()
            .map_or_else(Vec::new, |fields| {
                fields
                    .iter()
                    .filter_map(|field| field["name"].as_str())
                    .collect()
            });
        assert_eq!(
            named,
            vec![member],
            "case {case:?}: expected one fieldList entry naming {member}, got {:?}",
            answer.body
        );
    }

    // A body-bound one: the gateway's refusal, which names nothing — it knows only that
    // it could not bind the body.
    let answer = call(
        &router,
        "PUT",
        "/v1/compliance-inquiry/putFeedback",
        bytes::Bytes::from(json!({ "complianceInquiryId": WELL_FORMED_INQUIRY }).to_string()),
    )
    .await;
    assert_refused(
        "PutComplianceInquiryFeedback on a well-formed inquiry id, omitting rating",
        &answer,
        INVALID_BODY,
    );

    // And an operation whose input names no target at all keeps the same refusal for a
    // body member it requires and the body omits.
    let answer = call(
        &router,
        "POST",
        "/v1/compliance-inquiry/create",
        bytes::Bytes::from(
            json!({ "inquiryContent": { "query": "does the report cover eu-west-1?" } })
                .to_string(),
        ),
    )
    .await;
    assert_refused(
        "CreateComplianceInquiry omitting name",
        &answer,
        INVALID_BODY,
    );
}

#[tokio::test]
async fn a_complete_request_for_an_authorizable_target_still_reaches_the_operation() {
    let router = emulator();

    // The target is read off the request ahead of the input binding, so this is what
    // says that reading did not start refusing requests the front door authorizes: every
    // published report is served by the operation that reads its metadata.
    for expected in CATALOG {
        let answer = call(
            &router,
            "GET",
            &format!("/v1/report/getMetadata?reportId={}", expected.id),
            bytes::Bytes::new(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a published report must be served, got {} with {:?}",
            expected.id, answer.status, answer.body
        );
        assert_eq!(
            answer.body["reportDetails"]["id"].as_str(),
            Some(expected.id),
            "case {:?}: the served report must be the one targeted, got {:?}",
            expected.id,
            answer.body
        );
    }
}
