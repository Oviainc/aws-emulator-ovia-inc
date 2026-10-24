// SPDX-License-Identifier: CC0-1.0

//! Where the FRONT DOOR's judgement ends: the values it does not judge, and the bounds
//! it lets through.
//!
//! `front_door_errors.rs` pins what the gateway in front of this service refuses — a
//! target it cannot authorize (401 `UnauthorizedException`) and a request BODY it cannot
//! bind (400 `BadRequestException`). This suite pins the other side of that boundary, on
//! the operations the sibling suites do not reach: a value the request carries in its
//! URI is NOT the gateway's, so the SERVICE refuses it, as its own modelled 400
//! `ValidationException` in the envelope `validation_message_rendering.rs` pins (naming
//! the member and the requirement, never echoing the rejected value), and a value inside
//! the modelled bound reaches the operation and is served.
//!
//! The rows are chosen for reach past the sibling suites, because the boundary is decided
//! in ONE seam (`src/service.rs`'s `front_door` and `override_refusal`):
//!
//! * the two COMPLIANCE-INQUIRY paginated operations, whose `maxResults` declares the
//!   same `@range(min: 1, max: 300)`, including the one that also names a resource — so
//!   the front door judges its target first and still leaves the range to the service;
//! * a value far below the floor, where the clause must still name the floor;
//! * both PUBLISHED bounds, which are served — so the refusals above are about the bound
//!   and not about the member.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to it.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The endpoint this service is addressed by, named once.
const HOST: &str = "artifact.eu-west-1.amazonaws.com";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error body's `message`, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// `GET uri`, signed for this service.
async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, "GET", uri, Bytes::new()).await
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, payload: Bytes) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer { status, body }
}

/// One compliance inquiry this account HOLDS, created through the API, so a request
/// naming it is past the front door's authorization check and reaches the member
/// constraints this suite is about.
async fn create_an_inquiry(router: &MultiServiceRouter) -> String {
    let body = serde_json::json!({
        "name": "Which regions does the SOC 2 report cover?",
        "inquiryContent": { "query": "Which regions does the SOC 2 report cover?" },
        "supportMode": "AI_ONLY"
    })
    .to_string();
    let created = send(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the control create must succeed, got {} with {:?}",
        created.status, created.body
    );
    created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_uri_bound_page_size_outside_its_range_is_the_services_refusal_not_the_front_doors() {
    let router = emulator();

    // A HELD inquiry, so the only thing wrong with each request below is its
    // `maxResults`: an id the account does not hold is refused by the front door's 401
    // before any member constraint is reached, which would pin layering rather than this
    // rule.
    let created = create_an_inquiry(&router).await;

    // The members are the model's: `maxResults : MaxResultsAttribute @httpQuery
    // maxResults @range(min: 1, max: 300)` on ListComplianceInquiries and on
    // ListComplianceInquiryQueries (which also carries `complianceInquiryId`).
    let mut cases: Vec<(String, String, String)> = Vec::new();
    for (operation, uri) in [
        (
            "ListComplianceInquiries",
            "/v1/compliance-inquiry/list?maxResults=".to_string(),
        ),
        (
            "ListComplianceInquiryQueries",
            format!(
                "/v1/compliance-inquiry/listQueries?complianceInquiryId={created}\
                 &maxResults="
            ),
        ),
    ] {
        for (value, expected) in [
            (
                "0",
                "1 validation error detected: Value at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
            (
                "-7",
                "1 validation error detected: Value at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
            (
                "301",
                "1 validation error detected: Value at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 300",
            ),
        ] {
            cases.push((
                format!("{operation} maxResults={value}"),
                format!("{uri}{value}"),
                expected.to_string(),
            ));
        }
    }

    for (case, uri, expected) in &cases {
        let answer = get(&router, uri).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a URI-bound value outside its constraint must answer 400, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: a URI-bound value is the service's to refuse, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the member and the bound without the rejected value, got \
             {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn a_page_size_inside_the_bound_passes_the_front_door_and_is_served() {
    let router = emulator();

    // `@range(min: 1, max: 300)`: the bounds themselves are admissible, so a refusal of
    // one past them is about the bound and not about the member.
    for value in ["1", "300"] {
        let answer = get(&router, &format!("/v1/report/list?maxResults={value}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={value}: a published bound must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body["reports"].is_array(),
            "case maxResults={value}: a served page reports the catalog, got {:?}",
            answer.body
        );
    }
}
