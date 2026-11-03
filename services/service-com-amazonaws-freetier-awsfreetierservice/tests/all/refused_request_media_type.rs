// SPDX-License-Identifier: CC0-1.0

//! One service, one announced media type — including on the requests it never
//! serves.
//!
//! Every response of this service announces the media type its codec serializes,
//! `application/x-amz-json-1.1`, whatever layer answered: a handler's success, a
//! modelled failure a handler raises, a request the framework refuses in FRONT of
//! the handler (an absent `@required` member, a value outside a declared `@range`,
//! `@length`, `@pattern` or enum set, a body the codec cannot read), a routing key
//! naming no modelled operation, and a request carrying no routing key at all.
//! A client that keys its parsing or its protocol assertion off the announced
//! media type therefore reads one protocol from this service.
//!
//! The rows below span the refusal classes rather than one recorded request: the
//! constraint each one violates is read off the model
//! (`smithy_query(query="operation", operation="…")`), each `@range` is driven at
//! both published bounds and one past each, and every row sends the OTHER awsJson
//! revision as its own `content-type`, so a response cannot pass by echoing what
//! the request announced.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{error_code, harness};

/// The media type every response of this service announces, read from the service
/// itself so the expectation and the implementation share one source.
const SERVICE_MEDIA_TYPE: &str =
    service_com_amazonaws_freetier_awsfreetierservice::service::announced_media_type::RESPONSE_MEDIA_TYPE
        .announced();

/// The media type the model's `aws.protocols#awsJson1_0` protocol trait declares.
/// Every row sends it, and no response announces it.
const MODEL_MEDIA_TYPE: &str =
    service_com_amazonaws_freetier_awsfreetierservice::service::announced_media_type::RESPONSE_MEDIA_TYPE
        .implied();

/// One row: a request, and what this service must answer it with.
struct Case {
    /// What distinguishes this row.
    name: &'static str,
    /// The `X-Amz-Target` value, or `None` to send no routing key at all.
    target: Option<&'static str>,
    /// The request body, as the bytes the client sends — so a row can send a body
    /// the codec cannot read.
    body: &'static str,
    expected_status: u16,
    /// The `__type` of the answer, without its namespace; empty for a success.
    expected_code: &'static str,
}

/// Dispatch one row through the same multi-service router the emulator boots, and
/// report the status, the announced media type and the parsed body.
async fn answer(router: &MultiServiceRouter, case: &Case) -> (u16, String, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("https://freetier.us-east-1.amazonaws.com/")
        // The credential scope is what claims the request for this service, so a
        // row that sends no `x-amz-target` still reaches it.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR/20260101/us-east-1/freetier/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .header("content-type", MODEL_MEDIA_TYPE);
    if let Some(target) = case.target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from_static(case.body.as_bytes()))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .map(|value| value.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, media_type, payload)
}

/// Assert one row: the announced media type first, then the status, the error code
/// and that a refusal still states why.
async fn assert_answered_in_the_services_media_type(case: &Case) {
    let (router, _engine) = harness();
    let (status, media_type, payload) = answer(&router, case).await;
    assert_eq!(
        media_type, SERVICE_MEDIA_TYPE,
        "case {:?}: expected the response to announce {SERVICE_MEDIA_TYPE:?}, got \
         {media_type:?} with HTTP {status} and {payload}",
        case.name
    );
    assert_eq!(
        status, case.expected_status,
        "case {:?}: expected HTTP {}, got {status} with {payload}",
        case.name, case.expected_status
    );
    assert_eq!(
        error_code(&payload),
        case.expected_code,
        "case {:?}: expected the error code {:?}, got {payload}",
        case.name,
        case.expected_code
    );
    if !case.expected_code.is_empty() {
        let message = payload["message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {:?}: expected the refusal to state why, got {payload}",
            case.name
        );
    }
}

#[tokio::test]
async fn every_refused_request_is_answered_in_the_services_media_type() {
    let cases = [
        // ---- a request the framework refuses in front of the handler ----
        Case {
            name: "an absent @required member (UpgradeAccountPlanRequest$accountPlanType)",
            target: Some("AWSFreeTierService.UpgradeAccountPlan"),
            body: "{}",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "an absent @required member (GetAccountActivityRequest$activityId)",
            target: Some("AWSFreeTierService.GetAccountActivity"),
            body: "{}",
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "a value outside the AccountPlanType enum",
            target: Some("AWSFreeTierService.UpgradeAccountPlan"),
            body: r#"{"accountPlanType":"GOLD"}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "a value outside the LanguageCode enum",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"languageCode":"xx-XX"}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "a value outside the ActivityStatus enum, inside a list member",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"filterActivityStatuses":["ABANDONED"]}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        // `MaxResults` is `@range(min: 1, max: 1000)`: the value one past each
        // published bound is refused (the bounds themselves are served below).
        Case {
            name: "@range below the minimum (maxResults 0)",
            target: Some("AWSFreeTierService.GetFreeTierUsage"),
            body: r#"{"maxResults":0}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "@range above the maximum (maxResults 1001)",
            target: Some("AWSFreeTierService.GetFreeTierUsage"),
            body: r#"{"maxResults":1001}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "@range far above the maximum, on another operation",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"maxResults":5000}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        // One past the ceiling the ACTIVITY listing itself serves, which is below
        // the modelled maximum: the same status, code and media type as a bound
        // the model declares.
        Case {
            name: "one past the ceiling the activity listing serves",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"maxResults":101}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        // `ActivityId` is `@length(min: 32, max: 32)` with
        // `@pattern("^[a-zA-Z0-9]+$")`.
        Case {
            name: "@length below the minimum (activityId shorter than 32)",
            target: Some("AWSFreeTierService.GetAccountActivity"),
            body: r#"{"activityId":"short"}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "@length above the maximum (33 characters)",
            target: Some("AWSFreeTierService.GetAccountActivity"),
            body: r#"{"activityId":"000000000000000000000000000000000"}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "@pattern violation (an illegal character at length 32)",
            target: Some("AWSFreeTierService.GetAccountActivity"),
            body: r#"{"activityId":"0000000000000000000000000000000-"}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "@length below the minimum (an empty nextToken)",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"nextToken":""}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "a member of the wrong type",
            target: Some("AWSFreeTierService.GetFreeTierUsage"),
            body: r#"{"maxResults":"ten"}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "a body the codec cannot read at all",
            target: Some("AWSFreeTierService.UpgradeAccountPlan"),
            body: r#"{"accountPlanType":"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        // ---- a request that never names an operation of this service ----
        Case {
            name: "a target this service models no operation for",
            target: Some("AWSFreeTierService.NoSuchOperation"),
            body: "{}",
            expected_status: 501,
            expected_code: "NotImplemented",
        },
        Case {
            name: "no routing key at all",
            target: None,
            body: "{}",
            expected_status: 400,
            expected_code: "SerializationException",
        },
        // ---- the two layers this service's own codec already answered, for
        // contrast: one service announces one media type across all of them ----
        Case {
            name: "a modelled failure the handler raises",
            target: Some("AWSFreeTierService.GetFreeTierUsage"),
            body: r#"{"filter":{"Dimensions":{"Key":"SERVICE","Values":["AWSLambda"],
                      "MatchOptions":["EQUALS","CONTAINS"]}}}"#,
            expected_status: 400,
            expected_code: "ValidationException",
        },
        Case {
            name: "a well-formed activityId the store does not hold",
            target: Some("AWSFreeTierService.GetAccountActivity"),
            body: r#"{"activityId":"00000000000000000000000000000000"}"#,
            expected_status: 400,
            expected_code: "ResourceNotFoundException",
        },
        Case {
            // The FREE plan, because the PAID one is open only to an account
            // whose payment instrument has been vetted (`paid_plan_vetting.rs`)
            // and this row is about the media type of a success.
            name: "a handler's success",
            target: Some("AWSFreeTierService.UpgradeAccountPlan"),
            body: r#"{"accountPlanType":"FREE"}"#,
            expected_status: 200,
            expected_code: "",
        },
    ];

    for case in &cases {
        assert_answered_in_the_services_media_type(case).await;
    }
}

/// The accepted side of the same bound: a `@range` member is refused only OUTSIDE
/// its published minimum and maximum, so the rows above are not passing merely
/// because every request of this service is refused.
#[tokio::test]
async fn the_published_bounds_themselves_are_served() {
    let cases = [
        Case {
            name: "maxResults at the published minimum",
            target: Some("AWSFreeTierService.GetFreeTierUsage"),
            body: r#"{"maxResults":1}"#,
            expected_status: 200,
            expected_code: "",
        },
        Case {
            name: "maxResults at the published maximum",
            target: Some("AWSFreeTierService.GetFreeTierUsage"),
            body: r#"{"maxResults":1000}"#,
            expected_status: 200,
            expected_code: "",
        },
        Case {
            name: "maxResults at the published minimum, on another operation",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"maxResults":1}"#,
            expected_status: 200,
            expected_code: "",
        },
        Case {
            name: "maxResults at the ceiling the activity listing serves",
            target: Some("AWSFreeTierService.ListAccountActivities"),
            body: r#"{"maxResults":100}"#,
            expected_status: 200,
            expected_code: "",
        },
    ];

    for case in &cases {
        assert_answered_in_the_services_media_type(case).await;
    }
}
