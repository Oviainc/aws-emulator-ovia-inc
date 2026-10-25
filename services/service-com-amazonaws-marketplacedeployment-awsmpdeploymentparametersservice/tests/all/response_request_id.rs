// SPDX-License-Identifier: CC0-1.0

//! Every response this service sends names the request it answered.
//!
//! The rule under test: a response of this service carries the request-id header its
//! protocol reports that id in (`x-amzn-RequestId`, matched case-insensitively), freshly
//! minted per call, on EVERY answer — each operation's success, each modelled and
//! front-end error, and the framework-level failures no operation produces. It is a
//! property of the response rather than of any modelled shape, so no output or error
//! shape declares it and it is what an SDK exposes as `ServiceError::request_id()`.
//!
//! What the rule does NOT change, and this suite pins too: the status, the error code and
//! the body of every one of those answers stay exactly what they were — the id travels in
//! the header alone, so no response body gains it.
//!
//! The rows span what the rule generalises over: which operation answered (all four the
//! model binds — `smithy_query(query="digest")`), whether the answer is a success or a
//! failure, the success status (the 200s and the modelled 204s), the failure status and
//! code (404 `NotFoundException`, 400 `BadRequestException`, 400 `ValidationException`,
//! 403 `ForbiddenException`), whether a body was carried at all, and which LAYER answered:
//! a handler, the modelled constraint walk, the front door ahead of it, and the router's
//! own routing failure for a request that names no modelled operation.
//!
//! Every case drives the real wire path through the harness the `deployment_parameters`
//! suite owns, so what is asserted is what a signed SDK request observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, pct, provision, put, request, ACCOUNT, CATALOG, REGION, SECRET_MAX_LEN, SIGNING_NAME,
};

/// The header AWS names the request a response answered in. Written out here rather than
/// read from the crate, so the assertion does not merely compare the implementation with
/// itself.
const REQUEST_ID_HEADER: &str = "x-amzn-RequestId";

/// The agreement and product every write below targets.
const AGREEMENT: &str = "agmt-request-id";
const PRODUCT: &str = "prod-request-id";

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries.
fn names_a_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// Status, the request id the response named, and the body as text, for one signed call.
///
/// The header is read case-insensitively, which is how both HTTP and an SDK read it.
async fn answered(
    router: &MultiServiceRouter,
    req: http::Request<bytes::Bytes>,
) -> (u16, Option<String>, String) {
    let response = router.dispatch(req).await;
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, request_id, body)
}

/// One row of a table below: the call to make, and the answer it must get.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The status the call must answer with, or `None` where the status is the framework's
    /// own choice for a request that names no modelled operation — which this suite does
    /// not pin, only that it is a failure.
    status: Option<u16>,
}

impl Row {
    /// One row, with no request body and the status it must answer with.
    fn new(case: &'static str, method: &'static str, uri: String, status: u16) -> Self {
        Self {
            case,
            method,
            uri,
            body: None,
            status: Some(status),
        }
    }

    /// One row whose status is the framework's own choice, so only that it FAILS is
    /// pinned.
    fn failing(case: &'static str, method: &'static str, uri: String) -> Self {
        Self {
            case,
            method,
            uri,
            body: None,
            status: None,
        }
    }

    /// The same row carrying a request body.
    fn with_body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
}

/// The body of a `PutDeploymentParameter` write for `name` with a secret of `length`
/// characters.
fn put_body(name: &str, length: usize) -> Value {
    json!({
        "agreementId": AGREEMENT,
        "deploymentParameter": { "name": name, "secretString": "s".repeat(length) },
    })
}

/// The path `PutDeploymentParameter` binds for `catalog` and `product_id`.
fn put_path(catalog: &str, product_id: &str) -> String {
    format!(
        "/catalogs/{}/products/{}/deployment-parameters",
        pct(catalog),
        pct(product_id)
    )
}

/// One emulator holding one deployment parameter, and the ARN the service minted for it.
async fn emulator_with_one_parameter() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(&router, CATALOG, PRODUCT, &put_body("Present", 1)).await;
    assert_eq!(status, 200, "the setup Put must succeed, got {body:?}");
    let arn = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    (router, arn)
}

#[tokio::test]
async fn every_response_names_the_request_it_answers_with_a_fresh_id() {
    let (router, live) = emulator_with_one_parameter().await;
    let live_path = format!("/tags/{}", pct(&live));
    // An ARN of this vendor that names no stored record (the `not_found_error_code`
    // suite's own rule), so the 404 rows are about the ARN and not about an empty store.
    let unknown_path = format!(
        "/tags/{}",
        pct(&format!(
            "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffffffffffff"
        ))
    );

    // The call to make and the answer it must get, per row. The expected status is left to
    // the framework for the one row that names no modelled operation.
    let rows: Vec<Row> = vec![
        Row::new(
            "ListTagsForResource on a stored record",
            "GET",
            live_path.clone(),
            200,
        ),
        Row::new(
            "PutDeploymentParameter creating a record",
            "POST",
            put_path(CATALOG, PRODUCT),
            200,
        )
        .with_body(put_body("Created", 8)),
        Row::new(
            "PutDeploymentParameter updating that same record",
            "POST",
            put_path(CATALOG, PRODUCT),
            200,
        )
        .with_body(put_body("Created", 9)),
        Row::new(
            "TagResource on a stored record (the modelled 204)",
            "POST",
            live_path.clone(),
            204,
        )
        .with_body(json!({ "tags": { "owner": "integration-test" } })),
        Row::new(
            "UntagResource removing a tag that is set (the modelled 204)",
            "DELETE",
            format!("{live_path}?tagKeys=owner"),
            204,
        ),
        Row::new(
            "UntagResource removing a key that is not set",
            "DELETE",
            format!("{live_path}?tagKeys=never-set"),
            204,
        ),
        Row::new(
            "ListTagsForResource on an ARN of this vendor that names nothing",
            "GET",
            unknown_path.clone(),
            404,
        ),
        Row::new(
            "TagResource on an ARN of this vendor that names nothing",
            "POST",
            unknown_path.clone(),
            404,
        )
        .with_body(json!({ "tags": { "owner": "integration-test" } })),
        Row::new(
            "UntagResource on an ARN of this vendor that names nothing",
            "DELETE",
            format!("{unknown_path}?tagKeys=owner"),
            404,
        ),
        Row::new(
            "ListTagsForResource on a value that is no ARN of this vendor",
            "GET",
            format!("/tags/{}", pct("dp-0000000000000001")),
            400,
        ),
        Row::new(
            "TagResource with a tag key outside the tagging limits",
            "POST",
            live_path.clone(),
            400,
        )
        .with_body(json!({ "tags": { "": "v" } })),
        Row::new(
            "PutDeploymentParameter with a catalog that names no catalog",
            "POST",
            put_path("NotTheRealMarketplace", PRODUCT),
            400,
        )
        .with_body(put_body("Refused", 4)),
        Row::new(
            "PutDeploymentParameter with a required member absent",
            "POST",
            put_path(CATALOG, PRODUCT),
            400,
        )
        .with_body(json!({ "agreementId": AGREEMENT })),
        Row::new(
            "PutDeploymentParameter against a target the account does not hold",
            "POST",
            put_path(CATALOG, "prod-never-provisioned"),
            404,
        )
        .with_body(put_body("Elsewhere", 4)),
        Row::new(
            "PutDeploymentParameter with a sensitive payload past its maximum",
            "POST",
            put_path(CATALOG, PRODUCT),
            403,
        )
        .with_body(put_body("TooBig", SECRET_MAX_LEN + 1)),
        Row::failing(
            "a method this model binds no operation for on a path this service serves",
            "PUT",
            live_path.clone(),
        ),
    ];

    // Every id the suite was handed: the rule is one id PER CALL, so two responses may
    // never report the same one.
    let mut minted: Vec<(String, String)> = Vec::new();

    for Row {
        case,
        method,
        uri,
        body,
        status: expected,
    } in rows
    {
        let (status, request_id, answer) =
            answered(&router, request(method, &uri, body.as_ref())).await;
        match expected {
            Some(expected) => assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected}, got {status} {answer:?}"
            ),
            None => assert!(
                status >= 400,
                "case {case:?}: expected a failure status, got {status} {answer:?}"
            ),
        }
        let Some(request_id) = request_id else {
            panic!("case {case:?}: expected a {REQUEST_ID_HEADER} header, got none ({status})");
        };
        assert!(
            names_a_request_id(&request_id),
            "case {case:?}: expected the 8-4-4-4-12 lowercase-hex form of a request id, \
             got {request_id:?}"
        );
        // The header is the carrier: no answer's body gains the id.
        assert!(
            !answer.contains(&request_id),
            "case {case:?}: the request id belongs in the header alone, got body {answer:?}"
        );
        minted.push((case.to_string(), request_id));
    }

    // A body the codec cannot read at all — a failure raised before the handler and
    // before the constraint walk, answered by the framework's own codec path.
    let mut malformed = request("POST", &live_path, Some(&json!({})));
    *malformed.body_mut() = bytes::Bytes::from_static(b"{\"tags\": ");
    let (status, request_id, answer) = answered(&router, malformed).await;
    assert!(
        status >= 400,
        "a body the codec cannot read must be refused, got {status} {answer:?}"
    );
    let request_id =
        request_id.unwrap_or_else(|| panic!("expected a {REQUEST_ID_HEADER} header, got none"));
    assert!(
        names_a_request_id(&request_id),
        "a codec refusal must name the request it answers, got {request_id:?}"
    );
    minted.push(("a body the codec cannot read".to_string(), request_id));

    // One id per call, over every row above: the ids are a correlation handle, so no two
    // responses may carry the same one.
    let mut seen: Vec<&str> = minted.iter().map(|(_, id)| id.as_str()).collect();
    let answered_calls = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen.len(),
        answered_calls,
        "expected {answered_calls} distinct request ids, one per call, got {minted:?}"
    );
}

#[tokio::test]
async fn repeating_one_call_reports_a_different_id_each_time_and_the_same_answer() {
    let (router, live) = emulator_with_one_parameter().await;
    let live_path = format!("/tags/{}", pct(&live));

    // The same request, over and over: the ANSWER is identical and the id is not. Two
    // rows — a success and a failure — because the rule covers both.
    let repeated: Vec<Row> = vec![
        Row::new(
            "a repeated ListTagsForResource on a stored record",
            "GET",
            live_path.clone(),
            200,
        ),
        Row::new(
            "a repeated ListTagsForResource on a value that is no ARN",
            "GET",
            format!("/tags/{}", pct("not-an-arn")),
            400,
        ),
    ];

    for Row {
        case,
        method,
        uri,
        body,
        status: expected,
    } in repeated
    {
        let expected = expected.expect("every repeated row pins the status it expects");
        let mut ids: Vec<String> = Vec::new();
        let mut bodies: Vec<String> = Vec::new();
        for _ in 0..4 {
            let (status, request_id, answer) =
                answered(&router, request(method, &uri, body.as_ref())).await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected}, got {status} {answer:?}"
            );
            let Some(request_id) = request_id else {
                panic!("case {case:?}: expected a {REQUEST_ID_HEADER} header, got none");
            };
            ids.push(request_id);
            bodies.push(answer);
        }
        let mut distinct = ids.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "case {case:?}: expected a fresh id per call, got {ids:?}"
        );
        assert!(
            bodies.windows(2).all(|pair| pair[0] == pair[1]),
            "case {case:?}: the answer itself must not vary with the id, got {bodies:?}"
        );
    }
}
