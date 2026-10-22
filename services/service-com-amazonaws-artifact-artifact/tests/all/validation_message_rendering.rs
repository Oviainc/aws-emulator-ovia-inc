// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request it refuses, over HTTP.
//!
//! Two rules are under test, and both are about the body of a 400 the status and the
//! error code of which this emulator already got right:
//!
//! * a value the model's own constraints refuse is reported in the AWS validation
//!   envelope — `N validation error(s) detected: …` — whose clause names the member path
//!   and the requirement it failed and does NOT echo the rejected value back;
//! * a token this service did not issue is refused with the service's ONE fixed
//!   invalid-token text, which names neither the member nor the check that refused it and
//!   does not echo the token — so the text does not vary with the request.
//!
//! Both are about what this service refuses ITSELF: the gateway in front of it answers a
//! body it cannot bind, and an identifier naming no resource, in its own shapes before
//! dispatch (see `src/service.rs`), and the last test here pins that boundary.
//!
//! The rows below differ along what each rule generalises over: the constraint KIND
//! (numeric range, string length, pattern), the member and the operation carrying it, and
//! the rejected value itself.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The fixed text this service answers for a token it did not issue.
///
/// Spelled out here rather than imported from the crate so a changed constant fails this
/// suite instead of redefining what it asserts.
const INVALID_TOKEN: &str = "The provided token is invalid.";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `message` the error body carries, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Bytes) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", SIGNED)
        .body(body)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, "GET", uri, Bytes::new()).await
}

/// The first report id the catalog publishes, read back through `ListReports` rather than
/// hard-coded, so these rows name a report that really exists and only the member under
/// test is refused.
async fn a_published_report_id(router: &MultiServiceRouter) -> String {
    let listed = get(router, "/v1/report/list").await;
    assert_eq!(
        listed.status, 200,
        "ListReports must serve the catalog, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["reports"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the catalog must publish a report, got {:?}", listed.body))
        .to_string()
}

/// One refused request: how it is spelled, and the clause AWS's envelope states for it.
struct Refused {
    /// What this row is a case of.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Bytes,
    /// The member path the clause names.
    member: &'static str,
    /// The requirement the clause states, from the member's own modelled constraint.
    requirement: String,
}

#[tokio::test]
async fn a_refused_value_names_its_member_and_constraint_without_echoing_the_value() {
    let router = emulator();
    let report = a_published_report_id(&router).await;

    // Every row is a URI-bound member, which is the class this service refuses itself:
    // a BODY the gateway in front of it cannot bind, and an identifier that names no
    // resource, are answered before dispatch in the gateway's own shapes
    // (`BadRequestException` / `UnauthorizedException`, see `src/service.rs`), so they
    // are not cases of this rule.
    //
    // The rows differ along the constraint kind, the member, the operation and the value.
    // Every `requirement` is the member's modelled constraint, as `smithy_query` reports
    // it for this service's model:
    //   ListReports.maxResults / ListReportVersions.maxResults /
    //     ListCustomerAgreements.maxResults -> MaxResultsAttribute @range(min: 1, max: 300)
    //   ListReports.nextToken -> NextTokenAttribute @length(1..2048)
    //   GetReportMetadata.reportVersion -> VersionAttribute @range(min: 1)
    //   GetReport.termToken -> ShortStringAttribute @length(1..256),
    //     @pattern ^[a-zA-Z0-9_\-\s]*$
    let rows = vec![
        Refused {
            case: "maxResults one past the ceiling",
            method: "GET",
            uri: "/v1/report/list?maxResults=301".to_string(),
            body: Bytes::new(),
            member: "maxResults",
            requirement: "Member must have value less than or equal to 300".to_string(),
        },
        Refused {
            case: "maxResults far past the ceiling",
            method: "GET",
            uri: "/v1/report/list?maxResults=100000".to_string(),
            body: Bytes::new(),
            member: "maxResults",
            requirement: "Member must have value less than or equal to 300".to_string(),
        },
        Refused {
            case: "maxResults one below the floor",
            method: "GET",
            uri: "/v1/report/list?maxResults=0".to_string(),
            body: Bytes::new(),
            member: "maxResults",
            requirement: "Member must have value greater than or equal to 1".to_string(),
        },
        Refused {
            case: "the same range on another operation",
            method: "GET",
            uri: format!("/v1/report/listVersions?reportId={report}&maxResults=301"),
            body: Bytes::new(),
            member: "maxResults",
            requirement: "Member must have value less than or equal to 300".to_string(),
        },
        Refused {
            case: "a version below its range",
            method: "GET",
            uri: format!("/v1/report/getMetadata?reportId={report}&reportVersion=0"),
            body: Bytes::new(),
            member: "reportVersion",
            requirement: "Member must have value greater than or equal to 1".to_string(),
        },
        Refused {
            case: "the same range on a third operation",
            method: "GET",
            uri: "/v1/customer-agreement/list?maxResults=301".to_string(),
            body: Bytes::new(),
            member: "maxResults",
            requirement: "Member must have value less than or equal to 300".to_string(),
        },
        Refused {
            case: "a pagination token past its length",
            method: "GET",
            uri: format!("/v1/report/list?nextToken={}", "t".repeat(2049)),
            body: Bytes::new(),
            member: "nextToken",
            requirement: "Member must have length less than or equal to 2048".to_string(),
        },
        Refused {
            case: "a pagination token below its length",
            method: "GET",
            uri: "/v1/report/list?nextToken=".to_string(),
            body: Bytes::new(),
            member: "nextToken",
            requirement: "Member must have length greater than or equal to 1".to_string(),
        },
        Refused {
            case: "a term token past its length",
            method: "GET",
            uri: format!(
                "/v1/report/get?reportId={report}&termToken={}",
                "a".repeat(257)
            ),
            body: Bytes::new(),
            member: "termToken",
            requirement: "Member must have length less than or equal to 256".to_string(),
        },
        Refused {
            case: "a term token outside its pattern",
            method: "GET",
            uri: format!("/v1/report/get?reportId={report}&termToken=token.with.dots"),
            body: Bytes::new(),
            member: "termToken",
            requirement: "Member must satisfy regular expression pattern: \
                          ^[a-zA-Z0-9_\\-\\s]*$"
                .to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = send(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value outside its modelled constraint must be refused with \
             HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let expected = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: {}",
            row.member, row.requirement
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        // The generic half of the rule, independent of any one row's wording: no clause
        // ever quotes the value it refused.
        assert!(
            !answer.message().contains("Value '"),
            "case {case:?}: the clause must not quote the rejected value, got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn a_body_the_gateway_refuses_is_not_worded_as_this_rule() {
    let router = emulator();

    // The boundary of the rule above, asserted rather than assumed: a value the request
    // carried in its BODY is judged by the gateway in front of this service, which answers
    // in its own shape before dispatch — so a body refusal is NOT this service's validation
    // envelope, whatever this suite says about URI-bound members. The rows break a body
    // member in each of the two ways the gateway judges: an absent required member and a
    // value outside its modelled constraint.
    let rows = [
        (
            "a required body member absent",
            json!({"inquiryContent": {"query": "which controls apply?"}}),
        ),
        ("another required body member absent", json!({"name": "sim-probe"})),
        (
            "a body member outside its enum value set",
            json!({
                "name": "sim-probe",
                "inquiryContent": {"query": "which controls apply?"},
                "supportMode": "BOGUS_MODE",
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = send(
            &router,
            "POST",
            "/v1/compliance-inquiry/create",
            Bytes::from(body.to_string()),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a body the gateway cannot bind must be refused with HTTP 400, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: a body refusal is the gateway's, not this service's validation \
             envelope, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().contains("validation error detected"),
            "case {case:?}: a body refusal must not be worded as the validation envelope, \
             got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn a_token_this_service_never_issued_is_refused_with_one_fixed_text() {
    let router = emulator();
    let report = a_published_report_id(&router).await;

    // Every row satisfies the member's modelled constraints, so each one reaches the
    // handler and is refused for what it IS rather than for how it is spelled: a token
    // this service never issued. The rows differ along the token member (the two
    // pagination tokens and the download term token), the operation, and the token's own
    // shape — a word, a shape that looks like one of this service's tokens, a number, a
    // long one.
    let rows = [
        (
            "ListReports word",
            "/v1/report/list?nextToken=nota".to_string(),
        ),
        (
            "ListReports report-shaped",
            "/v1/report/list?nextToken=report-short".to_string(),
        ),
        (
            "ListReports numeric",
            "/v1/report/list?nextToken=42".to_string(),
        ),
        (
            "ListReportVersions word",
            format!("/v1/report/listVersions?reportId={report}&nextToken=nota"),
        ),
        (
            "ListReportVersions numeric",
            format!("/v1/report/listVersions?reportId={report}&nextToken=7"),
        ),
        (
            "ListReportVersions token issued for another report",
            format!("/v1/report/listVersions?reportId={report}&nextToken=report-elsewhere:1"),
        ),
        (
            "GetReport word",
            format!("/v1/report/get?reportId={report}&termToken=neverissued"),
        ),
        (
            "GetReport token-shaped",
            format!("/v1/report/get?reportId={report}&termToken=AAAA-BBBB_CCCC"),
        ),
        (
            "GetReport single character",
            format!("/v1/report/get?reportId={report}&termToken=a"),
        ),
        (
            "GetReport long token",
            format!("/v1/report/get?reportId={report}&termToken={}", "z".repeat(256)),
        ),
    ];

    for (case, uri) in rows {
        let answer = get(&router, &uri).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token this service never issued must be refused with HTTP 400, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            INVALID_TOKEN,
            "case {case:?}: expected the fixed invalid-token text {INVALID_TOKEN:?}, got {:?}",
            answer.message()
        );
        assert_eq!(
            answer.body["reason"].as_str(),
            Some("invalidToken"),
            "case {case:?}: the refusal must report the invalidToken reason, got {:?}",
            answer.body
        );
    }
}
