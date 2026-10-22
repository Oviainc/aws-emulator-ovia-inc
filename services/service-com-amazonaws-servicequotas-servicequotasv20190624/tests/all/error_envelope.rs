// SPDX-License-Identifier: CC0-1.0

//! How Service Quotas answers a request it will not serve: the HTTP status, the error
//! shape, and the body member that says which input was wrong.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, so what is exercised is the wire behaviour.
//!
//! Two rules, each asserted over a TABLE of inputs rather than one recorded case:
//!
//! * Service Quotas speaks `awsJson1_1`, and an awsJson front end answers a modeled
//!   failure on the status of its FAULT — every modeled client error on HTTP 400, with
//!   the body's `__type` telling one from another, and a server error on its own 5xx.
//!   The `@httpError` an error shape declares is not the status on the wire
//!   ([`service_com_amazonaws_servicequotas_servicequotasv20190624::scaffolded::wire_http_status`]).
//! * a request that breaks a rule the model declares is refused with the MODELED
//!   `IllegalArgumentException`, whose `Message` member names the member the caller got
//!   wrong — spelled as the model spells it — and the constraint that member failed.
//!
//! This suite carries its own client because it reads the whole response BODY (the
//! `Message` member), while `quota_workflow`'s refusal helper reports only the status
//! and the `__type`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The routing header from this crate's `OPERATIONS.md`: `POST /` with
/// `X-Amz-Target: ServiceQuotasV20190624.<OperationName>`.
const TARGET_PREFIX: &str = "ServiceQuotasV20190624.";

/// The history listing, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// A caller pointed at one regional endpoint of one emulator.
struct Client {
    router: MultiServiceRouter,
    region: String,
}

/// A caller for `region`, serving out of one shared engine — the way the router builds
/// it.
fn client(region: &str) -> Client {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests sign nothing, so they must not be refused for it.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(engine),
    );
    Client {
        router,
        region: region.to_string(),
    }
}

impl Client {
    /// Dispatch one awsJson1_1 call and return its status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("servicequotas.{}.amazonaws.com", self.region))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            // The credential scope is what routes the request to this service and
            // names the region it is addressed to.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTCLIENT/20260101/{}/servicequotas/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=notchecked",
                    self.region
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }
}

/// The error code a refusal carries (`__type` on awsJson), and its `Message` member.
fn error_of(body: &Value) -> (String, Option<String>) {
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string();
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .map(str::to_string);
    (code, message)
}

/// A lookup that names no quota, service or increase request is `NoSuchResourceException`
/// on the CLIENT-FAULT status — 400, not the `@httpError(404)` the shape declares — and
/// it says what was not found.
#[tokio::test]
async fn a_lookup_that_names_nothing_is_a_client_fault_on_400() {
    let client = client("us-east-1");

    // Rows differ in the operation, in WHICH member names nothing (service code, quota
    // code, the pair, a request id), and in form (single read vs listing): the one rule
    // covers all of them. The known codes are the seeded catalog
    // (`src/state.rs::seed_services`).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "an unknown service code",
            "GetServiceQuota",
            json!({"ServiceCode": "not-a-real-service-xyz", "QuotaCode": "L-00000000"}),
        ),
        (
            "an unknown quota code under a known service",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-00000000"}),
        ),
        (
            "a quota code that belongs to another service",
            "GetServiceQuota",
            json!({"ServiceCode": "lambda", "QuotaCode": "L-F678F1CE"}),
        ),
        (
            "an unknown pair on the default read",
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": "not-a-real-service-xyz", "QuotaCode": "L-00000000"}),
        ),
        (
            "an unknown service code on the applied listing",
            "ListServiceQuotas",
            json!({"ServiceCode": "not-a-real-service-xyz"}),
        ),
        (
            "an unknown service code on the default listing",
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "not-a-real-service-xyz"}),
        ),
        (
            "an unknown service code on the change history",
            HISTORY,
            json!({"ServiceCode": "not-a-real-service-xyz", "QuotaCode": "L-00000000"}),
        ),
        (
            "an unknown quota code on the change history",
            HISTORY,
            json!({"ServiceCode": "vpc", "QuotaCode": "L-00000000"}),
        ),
        (
            "an unknown request id",
            "GetRequestedServiceQuotaChange",
            json!({"RequestId": "00000000-0000-0000-0000-000000000000"}),
        ),
        (
            "another unknown request id, in another form",
            "GetRequestedServiceQuotaChange",
            json!({"RequestId": "0123456789abcdef0123456789abcdef"}),
        ),
    ];

    for (why, operation, body) in rows {
        let case = format!("{why} ({operation})");
        let (status, parsed) = client.call(operation, body).await;
        let (code, message) = error_of(&parsed);
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert!(
            code.contains("NoSuchResourceException"),
            "case {case}: expected NoSuchResourceException, got {code:?} in {parsed}"
        );
        assert!(
            message.as_deref().is_some_and(|text| !text.is_empty()),
            "case {case}: expected a non-empty Message member, got {parsed}"
        );
    }
}

/// A value that breaks a rule the model declares is refused with the modeled
/// `IllegalArgumentException`, and its `Message` names the member and the constraint.
#[tokio::test]
async fn a_modelled_constraint_refusal_names_the_member_and_the_rule() {
    let client = client("us-east-1");
    let over_long_service_code = "v".repeat(64);
    let over_long_quota_code = "v".repeat(129);

    // Each row: why it is invalid, the operation, the request, the member the refusal
    // must name (as the MODEL spells it), and the rule it must state. Every constraint
    // and every bound below is read off the model
    // (`smithy_query(query="operation", operation="GetServiceQuota,ListServiceQuotas,\
    // GetRequestedServiceQuotaChange,ListRequestedServiceQuotaChangeHistoryByQuota,\
    // ListServices,CreateSupportCase")`), so the rows span the constraint KINDS the
    // model uses — `@pattern`, both `@length` bounds, both `@range` bounds, an
    // enumerated value set and `@required` — on several members and operations.
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "a quota code outside its pattern",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "1-bad code"}),
            "QuotaCode",
            // `@pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$` on GetServiceQuotaRequest$QuotaCode.
            "Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9-]{1,128}"
                .to_string(),
        ),
        (
            "a service code outside its pattern",
            "ListServiceQuotas",
            json!({"ServiceCode": "1-not-a-service"}),
            "ServiceCode",
            // `@pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$` on ListServiceQuotasRequest$ServiceCode.
            "Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9-]{1,63}"
                .to_string(),
        ),
        (
            "a request id outside its pattern",
            "GetRequestedServiceQuotaChange",
            json!({"RequestId": "-leading-dash"}),
            "RequestId",
            // `@pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$` — a different character class
            // from the two above, so the rule is not read off one member.
            "Member must satisfy regular expression pattern: [0-9a-zA-Z][a-zA-Z0-9-]{1,128}"
                .to_string(),
        ),
        (
            "a service code past its length maximum",
            "ListServiceQuotas",
            json!({"ServiceCode": over_long_service_code}),
            "ServiceCode",
            // `@length 1..63`.
            "Member must have length less than or equal to 63".to_string(),
        ),
        (
            "a quota code past its length maximum",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": over_long_quota_code}),
            "QuotaCode",
            // `@length 1..128`.
            "Member must have length less than or equal to 128".to_string(),
        ),
        (
            "an empty quota code, below its length minimum",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": ""}),
            "QuotaCode",
            "Member must have length greater than or equal to 1".to_string(),
        ),
        (
            "a page size below its range",
            "ListServices",
            json!({"MaxResults": 0}),
            "MaxResults",
            // `@range 1..100` on MaxResults.
            "Member must have value greater than or equal to 1".to_string(),
        ),
        (
            "a page size above its range",
            "ListServiceQuotas",
            json!({"ServiceCode": "vpc", "MaxResults": 101}),
            "MaxResults",
            "Member must have value less than or equal to 100".to_string(),
        ),
        (
            "a status outside the enumerated set",
            HISTORY,
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "Status": "NOT_A_STATUS"}),
            "Status",
            // The `RequestStatus` enum: the refusal states the published value set,
            // which this row only pins the opening of — the members and their order are
            // the model's.
            "Member must satisfy enum value set: [".to_string(),
        ),
        (
            "an absent required member",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc"}),
            "QuotaCode",
            // `@required`, found while READING the request rather than by walking it.
            "Member must not be null".to_string(),
        ),
        (
            "an absent required member on a listing",
            HISTORY,
            json!({"ServiceCode": "vpc"}),
            "QuotaCode",
            "Member must not be null".to_string(),
        ),
        (
            // A constraint is a property of the INPUT SHAPE, so it is refused the same
            // way whoever would serve the request — including an operation this
            // emulator still answers with a 501.
            "a bad request id on an operation that is still a stub",
            "CreateSupportCase",
            json!({"RequestId": "-leading-dash"}),
            "RequestId",
            "Member must satisfy regular expression pattern: [0-9a-zA-Z][a-zA-Z0-9-]{1,128}"
                .to_string(),
        ),
    ];

    for (why, operation, body, member, rule) in rows {
        let case = format!("{why} ({operation})");
        let (status, parsed) = client.call(operation, body).await;
        let (code, message) = error_of(&parsed);
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert!(
            code.contains("IllegalArgumentException"),
            "case {case}: expected IllegalArgumentException, got {code:?} in {parsed}"
        );
        let expected = format!("{member} failed to satisfy constraint: {rule}");
        let message = message.unwrap_or_else(|| {
            panic!("case {case}: expected a Message member, got {parsed}");
        });
        assert!(
            message.starts_with(&expected),
            "case {case}: expected Message starting {expected:?}, got {message:?}"
        );
    }

    // The construction rule stated whole, on the member the pattern rule is published
    // for: nothing before the member name, nothing after the pattern.
    let (_, parsed) = client
        .call(
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "1-bad code"}),
        )
        .await;
    let (_, message) = error_of(&parsed);
    assert_eq!(
        message.as_deref(),
        Some(
            "QuotaCode failed to satisfy constraint: Member must satisfy regular expression \
             pattern: [a-zA-Z][a-zA-Z0-9-]{1,128}"
        ),
        "case the whole constraint clause: got {parsed}"
    );
}

/// The status a modeled error goes out on, for EVERY `@httpError` this service's model
/// declares — a client fault on 400, a server fault on its own 5xx.
#[test]
fn the_wire_status_reports_the_fault_for_every_modelled_http_error() {
    use service_com_amazonaws_servicequotas_servicequotasv20190624::scaffolded::wire_http_status;

    // The catalog of statuses the model declares across this service's error shapes
    // (`smithy_query(query="shape", shape="TagPolicyViolationException,\
    // AccessDeniedException,NoSuchResourceException,InvalidResourceStateException,\
    // QuotaExceededException,TooManyRequestsException,IllegalArgumentException,\
    // ServiceException")`), with the error that carries each and the status this
    // service answers it on. The 401/405/409 entries are what show these are legacy
    // error numbers rather than HTTP semantics.
    let rows = [
        ("IllegalArgumentException", 400_u16, 400_u16),
        ("TagPolicyViolationException", 401, 400),
        ("AccessDeniedException", 403, 400),
        ("NoSuchResourceException", 404, 400),
        ("InvalidResourceStateException", 405, 400),
        ("QuotaExceededException", 409, 400),
        ("TooManyRequestsException", 429, 400),
        ("ServiceException", 500, 500),
    ];

    for (error, modelled, expected) in rows {
        let actual = wire_http_status(modelled);
        assert_eq!(
            actual, expected,
            "case {error} (@httpError {modelled}): expected HTTP {expected}, got {actual}"
        );
    }

    // And the bounds of the classification itself, one past each side: anything below
    // 500 is a client fault, and every server status is kept as declared.
    for (modelled, expected) in [(499_u16, 400_u16), (500, 500), (503, 503)] {
        let actual = wire_http_status(modelled);
        assert_eq!(
            actual, expected,
            "case the bound at {modelled}: expected HTTP {expected}, got {actual}"
        );
    }
}

/// A request the model ACCEPTS is still served by its operation: the refusal path in
/// front of every operation refuses nothing the model allows, and does not swallow a
/// stub's 501.
#[tokio::test]
async fn a_request_the_model_accepts_still_reaches_its_operation() {
    let client = client("us-east-1");

    // Rows: an implemented operation whose request satisfies every modelled rule,
    // including the optional members a refusal path could trip over.
    let served: Vec<(&str, Value)> = vec![
        ("ListServices", json!({})),
        ("ListServices", json!({"MaxResults": 1})),
        ("ListServiceQuotas", json!({"ServiceCode": "vpc"})),
        (
            "ListServiceQuotas",
            json!({"ServiceCode": "vpc", "MaxResults": 100}),
        ),
        (
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"}),
        ),
        (
            HISTORY,
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "Status": "PENDING"}),
        ),
    ];

    for (operation, body) in served {
        let case = format!("{operation} {body}");
        let (status, parsed) = client.call(operation, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {parsed}"
        );
    }

    // A stub still answers the scaffold's 501 for a request the model accepts — the
    // stub census is not a 400.
    let (status, parsed) = client
        .call("CreateSupportCase", json!({"RequestId": "abc123"}))
        .await;
    assert_eq!(
        status, 501,
        "case a stub with a valid request: expected HTTP 501, got {status} with {parsed}"
    );
}
