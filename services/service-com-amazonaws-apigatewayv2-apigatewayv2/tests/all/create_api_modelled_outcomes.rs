// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the statuses `CreateApi` may answer with.
//!
//! The rule: **a modelled operation answers every request with a status its model
//! declares — its success code, or the HTTP status of one of its modelled errors — so no
//! request, however malformed, is answered with a status the model does not carry.**
//! `smithy_query(query="operation", operation="CreateApi")` declares
//! `http {method: POST, uri: /v2/apis, code: 201}` and exactly four errors —
//! BadRequestException (400), ConflictException (409), NotFoundException (404),
//! TooManyRequestsException (429) — so 201, 400, 404, 409 and 429 are the whole catalog
//! and a 5xx names no shape a client of this service can resolve.
//!
//! The rows span what the rule generalises over: which members a request carries, the
//! JSON type each carries, whether the body is an object at all, whether it is JSON at
//! all, and whether it is present — with each string member driven at the length its
//! target shape documents and one character past it. The recorded request that filed the
//! issue (a `Name` and a `ProtocolType`, nothing else) is the first row.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_created_now, assert_minted_id, call, emulator, error_code,
};

/// The region the raw-body request below is signed for — the same one
/// [`crate::api_lifecycle::call`] signs for, so the two rows of a table are answered by
/// one service in one region.
const REGION: &str = "eu-west-1";

/// Every status `CreateApi` models, from its `http` trait and its four modelled errors.
const MODELLED_STATUSES: &[u16] = &[201, 400, 404, 409, 429];

/// The error shapes `CreateApi` models, by the code a client resolves them by.
const MODELLED_ERRORS: &[&str] = &[
    "BadRequestException",
    "ConflictException",
    "NotFoundException",
    "TooManyRequestsException",
];

/// The documented length bounds of the shapes `CreateApi`'s string members target:
/// `Name` -> `StringWithLengthBetween1And128` ("A string with a length between
/// [1-128]"), `Description` -> `StringWithLengthBetween0And1024`, `Version` ->
/// `StringWithLengthBetween1And64`.
const MAX_NAME_CHARS: usize = 128;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_VERSION_CHARS: usize = 64;

/// What a row must be answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// 201, with the api's stored record.
    Created,
    /// Refused with this modelled error code, at that error's modelled status.
    Refused(&'static str),
    /// Refused at the PROTOCOL boundary, before the operation's own error channel is
    /// reached: a body the codec could not read as this operation's input at all.
    ///
    /// The status must still be one the model declares, and the refusal must say what
    /// was wrong. The CODE is the framework's own: only an absent required member and a
    /// broken modelled constraint reach `Service::override_refusal`
    /// (`smithy_server_runtime`'s `refusal_of` describes nothing else structurally), so
    /// a service crate cannot word this one and this suite pins what it governs.
    RefusedAtTheBoundary,
}

/// The body a row sends: a JSON document, or bytes that are not one.
#[derive(Debug, Clone)]
enum Body {
    Json(Value),
    Raw(&'static str),
}

/// One row of the table.
struct Row {
    case: String,
    body: Body,
    expect: Expect,
}

fn json_row(case: &str, body: Value, expect: Expect) -> Row {
    Row {
        case: case.to_string(),
        body: Body::Json(body),
        expect,
    }
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        // The recorded request: the two required members and nothing else.
        json_row(
            "the required members only, HTTP",
            json!({ "name": "modelled-http", "protocolType": "HTTP" }),
            Expect::Created,
        ),
        json_row(
            "the required members and the expression a WEBSOCKET api requires",
            json!({
                "name": "modelled-socket",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action"
            }),
            Expect::Created,
        ),
        json_row(
            "a member the model does not declare, alongside the required two",
            json!({
                "name": "modelled-extra-member",
                "protocolType": "HTTP",
                "notAModelledMember": { "nested": [1, 2, 3] }
            }),
            Expect::Created,
        ),
        json_row(
            "Name absent",
            json!({ "protocolType": "HTTP" }),
            Expect::Refused("BadRequestException"),
        ),
        json_row(
            "ProtocolType absent",
            json!({ "name": "no-protocol" }),
            Expect::Refused("BadRequestException"),
        ),
        json_row(
            "an empty JSON object",
            json!({}),
            Expect::Refused("BadRequestException"),
        ),
        json_row(
            "Name of the wrong JSON type: a number",
            json!({ "name": 7, "protocolType": "HTTP" }),
            Expect::RefusedAtTheBoundary,
        ),
        json_row(
            "DisableExecuteApiEndpoint of the wrong JSON type: a string",
            json!({
                "name": "wrong-typed-flag",
                "protocolType": "HTTP",
                "disableExecuteApiEndpoint": "true"
            }),
            Expect::RefusedAtTheBoundary,
        ),
        json_row(
            "Tags of the wrong JSON type: a list",
            json!({ "name": "wrong-typed-tags", "protocolType": "HTTP", "tags": ["a", "b"] }),
            Expect::RefusedAtTheBoundary,
        ),
        json_row(
            "CorsConfiguration of the wrong JSON type: a string",
            json!({
                "name": "wrong-typed-cors",
                "protocolType": "HTTP",
                "corsConfiguration": "https://example.com"
            }),
            Expect::RefusedAtTheBoundary,
        ),
        json_row(
            "a body that is a JSON list, not the input structure",
            json!([{ "name": "listed", "protocolType": "HTTP" }]),
            Expect::RefusedAtTheBoundary,
        ),
        json_row(
            "a body that is a JSON string, not the input structure",
            json!("name=stringly&protocolType=HTTP"),
            Expect::RefusedAtTheBoundary,
        ),
        Row {
            case: "a body that is not JSON at all".to_string(),
            body: Body::Raw("name: modelled, protocolType: HTTP"),
            expect: Expect::RefusedAtTheBoundary,
        },
        Row {
            case: "a body of unbalanced JSON".to_string(),
            body: Body::Raw("{\"name\": \"truncated\", \"protocolType\": "),
            expect: Expect::RefusedAtTheBoundary,
        },
        Row {
            case: "no body at all".to_string(),
            body: Body::Raw(""),
            expect: Expect::Refused("BadRequestException"),
        },
    ];

    // Each string member at the length its shape documents, and one character past it.
    // A value AT the bound is in range, so it is accepted; one past it is not.
    rows.push(json_row(
        "Name at its documented maximum length",
        json!({ "name": "n".repeat(MAX_NAME_CHARS), "protocolType": "HTTP" }),
        Expect::Created,
    ));
    rows.push(json_row(
        "Name one character past its documented maximum length",
        json!({ "name": "n".repeat(MAX_NAME_CHARS + 1), "protocolType": "HTTP" }),
        Expect::Refused("BadRequestException"),
    ));
    rows.push(json_row(
        "Name at its documented minimum length",
        json!({ "name": "n", "protocolType": "HTTP" }),
        Expect::Created,
    ));
    rows.push(json_row(
        "Name one character below its documented minimum length: empty",
        json!({ "name": "", "protocolType": "HTTP" }),
        Expect::Refused("BadRequestException"),
    ));
    rows.push(json_row(
        "Description at its documented maximum length",
        json!({
            "name": "max-description",
            "protocolType": "HTTP",
            "description": "d".repeat(MAX_DESCRIPTION_CHARS)
        }),
        Expect::Created,
    ));
    rows.push(json_row(
        "Description one character past its documented maximum length",
        json!({
            "name": "over-description",
            "protocolType": "HTTP",
            "description": "d".repeat(MAX_DESCRIPTION_CHARS + 1)
        }),
        Expect::Refused("BadRequestException"),
    ));
    rows.push(json_row(
        "Version at its documented maximum length",
        json!({
            "name": "max-version",
            "protocolType": "HTTP",
            "version": "v".repeat(MAX_VERSION_CHARS)
        }),
        Expect::Created,
    ));
    rows.push(json_row(
        "Version one character past its documented maximum length",
        json!({
            "name": "over-version",
            "protocolType": "HTTP",
            "version": "v".repeat(MAX_VERSION_CHARS + 1)
        }),
        Expect::Refused("BadRequestException"),
    ));
    rows.push(json_row(
        "Version one character below its documented minimum length: empty",
        json!({ "name": "empty-version", "protocolType": "HTTP", "version": "" }),
        Expect::Refused("BadRequestException"),
    ));
    rows
}

/// Send one `CreateApi` request whose body is arbitrary bytes.
///
/// [`crate::api_lifecycle::call`] serializes a `serde_json::Value`, so it cannot express
/// a body that is not a JSON document; this sends the bytes as given, with the same
/// headers, so the two differ only in the body.
async fn post_bytes(emulator: &MultiServiceRouter, payload: &'static str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/v2/apis")
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/apigateway/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from_static(payload.as_bytes()))
        .expect("build the request");

    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

#[tokio::test]
async fn create_api_answers_only_a_status_its_model_declares() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case.as_str();
        let (status, body) = match &row.body {
            Body::Json(document) => call(&emulator, "POST", "/v2/apis", Some(document.clone())).await,
            Body::Raw(payload) => post_bytes(&emulator, payload).await,
        };

        // The rule itself: whatever the request, the status is one the model declares.
        assert!(
            MODELLED_STATUSES.contains(&status),
            "case {case:?}: CreateApi answered {status}, which its model does not declare \
             (one of {MODELLED_STATUSES:?} is required), body {body}"
        );

        match row.expect {
            Expect::Created => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from CreateApi, got {status} {body}"
                );
                // Every member the response must carry for the api to be usable: the id
                // a dependent call keys off, the endpoint a client is told to reach, and
                // the record's own instant.
                let api_id = as_str(&body, "apiId");
                assert_minted_id(case, "ApiId", &api_id);
                assert!(
                    as_str(&body, "apiEndpoint").contains(&api_id),
                    "case {case:?}: ApiEndpoint must be this api's own host, got {body}"
                );
                assert!(
                    !as_str(&body, "name").is_empty(),
                    "case {case:?}: the response must report the stored Name, got {body}"
                );
                assert!(
                    !as_str(&body, "protocolType").is_empty(),
                    "case {case:?}: the response must report the stored ProtocolType, got {body}"
                );
                assert!(
                    !as_str(&body, "routeSelectionExpression").is_empty(),
                    "case {case:?}: the response must report a RouteSelectionExpression, got {body}"
                );
                assert!(
                    !as_str(&body, "apiKeySelectionExpression").is_empty(),
                    "case {case:?}: the response must report an ApiKeySelectionExpression, got {body}"
                );
                assert_created_now(case, &body);

                // And the record it reports is the one a dependent step reads back.
                let (status, fetched) =
                    call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetApi, got {status} {fetched}"
                );
                assert_eq!(
                    fetched, body,
                    "case {case:?}: GetApi must report the record CreateApi stored"
                );
            }
            Expect::Refused(code) => {
                let actual = error_code(&body);
                assert_eq!(
                    actual, code,
                    "case {case:?}: expected {code}, got {status} {body}"
                );
                assert!(
                    MODELLED_ERRORS.contains(&actual.as_str()),
                    "case {case:?}: {actual:?} is no error CreateApi models, got {body}"
                );
                assert!(
                    !as_str(&body, "message").is_empty(),
                    "case {case:?}: a refusal must carry a message, got {body}"
                );
                // A refused create stores nothing, which the count below checks for the
                // whole table.
            }
            Expect::RefusedAtTheBoundary => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a body this operation's input cannot be read from is a \
                     400, got {status} {body}"
                );
                assert!(
                    !as_str(&body, "message").is_empty(),
                    "case {case:?}: a refusal must carry a message, got {body}"
                );
            }
        }
    }

    // Exactly the accepted rows were stored: no refused request left a record behind,
    // and no accepted one was stored twice.
    let accepted = rows()
        .iter()
        .filter(|row| row.expect == Expect::Created)
        .count();
    let (status, listed) = call(&emulator, "GET", "/v2/apis", None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetApis, got {status} {listed}"
    );
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(accepted),
        "expected the {accepted} accepted rows to be the whole store, got {listed}"
    );
}

/// The windows above, as the patch that rewrites each member: (member, the value, and
/// whether the api must accept it). A value AT a bound is in range; one past it is not.
fn patch_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        ("name", "n".repeat(MAX_NAME_CHARS), true),
        ("name", "n".repeat(MAX_NAME_CHARS + 1), false),
        ("name", String::new(), false),
        ("description", "d".repeat(MAX_DESCRIPTION_CHARS), true),
        ("description", "d".repeat(MAX_DESCRIPTION_CHARS + 1), false),
        ("description", String::new(), true),
        ("version", "v".repeat(MAX_VERSION_CHARS), true),
        ("version", "v".repeat(MAX_VERSION_CHARS + 1), false),
        ("version", String::new(), false),
    ]
}

#[tokio::test]
async fn update_api_holds_a_patch_to_the_same_documented_windows() {
    let emulator = emulator();

    let (status, created) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "patched", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi, got {status} {created}"
    );
    let api_id = as_str(&created, "apiId");
    let uri = format!("/v2/apis/{api_id}");

    for (member, value, accepted) in patch_rows() {
        let case = format!("{member} of {} characters", value.chars().count());
        let (before_status, before) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            before_status, 200,
            "case {case:?}: expected 200 from GetApi, got {before_status} {before}"
        );

        let (status, body) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ member: value.clone() })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: a value inside the documented window must be accepted, \
                 got {status} {body}"
            );
            assert_eq!(
                as_str(&body, member),
                value,
                "case {case:?}: UpdateApi must report the value it stored, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a value outside the documented window must be refused, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            let (_, after) = call(&emulator, "GET", &uri, None).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused patch must leave the api exactly as it was"
            );
        }
    }
}
