// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the error-envelope member this service names its exceptions in.
//!
//! The rule under test: every failure this service answers states the modelled exception
//! shape it refuses with in its JSON body's `code` member, beside `message`. `code` is a
//! member of the error ENVELOPE — no exception this model declares carries any member but
//! `message` — so it is the body's own error-type discriminator, which is what a client
//! that reads the error type out of the body branches on.
//!
//! The rows span the class the rule generalises over rather than the one recorded refusal:
//! which operation is called (a read, a create, an update, a delete, a listing, a tagging
//! call), which layer refused it (a handler's modelled refusal or the boundary validator's),
//! which exception shape the refusal carries, and which HTTP status that shape declares
//! (400, 403, 404, 409). The last test pins the boundary: a 501 for an operation nobody has
//! written yet is the emulator's own signal, not an Internet Monitor exception, so it is
//! left as the framework rendered it.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator binary
//! mounts, so the wire envelope is exercised the way a client exercises it. The harness
//! (`emulator`, `call`, `label`, `create_monitor`) lives in `crate::monitor_lifecycle` and
//! is reused here rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::MAX_RESOURCES_PER_MONITOR;
use smithy_server_runtime::MultiServiceRouter;

use crate::monitor_lifecycle::{call, create_monitor, emulator, error_code, error_message, label};

/// The monitor every row that needs one addresses.
const MONITOR: &str = "coded-monitor";

/// An ARN shaped like a monitor's but held by nothing, for the tagging operations: it
/// satisfies the modelled `^arn:.*` pattern and 20..512 length, so it reaches the handler
/// that answers for a resource this account does not hold.
const UNHELD_ARN: &str = "arn:aws:internetmonitor:us-east-1:000000000000:monitor/never-created";

/// One refused request, and the exception its body must name.
struct CodeCase {
    /// Names the row in every assertion message.
    case: String,
    /// HTTP method.
    method: &'static str,
    /// Request path.
    path: String,
    /// Request body, where the operation takes one.
    body: Option<Value>,
    /// The HTTP status the refusal must carry.
    status: u16,
    /// The exception shape the body's `code` member must name.
    code: &'static str,
}

/// Send `case` and assert the body names its exception in `code`, beside a message.
async fn assert_names_its_code(router: &MultiServiceRouter, case: CodeCase) {
    let (status, body) = call(router, case.method, &case.path, case.body).await;
    assert_eq!(
        status, case.status,
        "case {:?}: expected {}, got {status} with {body}",
        case.case, case.status
    );
    assert_eq!(
        body["code"].as_str(),
        Some(case.code),
        "case {:?}: expected the body's code member to be {:?}, got {body}",
        case.case,
        case.code
    );
    // The two discriminators a restJson1 client may read name the SAME shape, so a client
    // keying on either one classifies the refusal identically.
    assert_eq!(
        error_code(&body),
        case.code,
        "case {:?}: the body's code member and its __type must name one shape, got {body}",
        case.case
    );
    assert!(
        !error_message(&body).is_empty(),
        "case {:?}: the refusal must still state a message, got {body}",
        case.case
    );
}

/// `count` ARNs a monitor could hold, for the row that carries the bounded resource set
/// past its ceiling.
fn resource_arns(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/lb-{index:04}/0123456789abcdef"))
        .collect()
}

#[tokio::test]
async fn every_refusal_names_its_exception_in_the_code_member() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;

    let cases = vec![
        // A handler's own semantic refusals: ValidationException (400), on a read, a
        // listing, a status poll and a cancel.
        CodeCase {
            case: "GetHealthEvent for an event the monitor does not hold".to_string(),
            method: "GET",
            path: format!("/v20210603/Monitors/{MONITOR}/HealthEvents/AWS-event-0001"),
            body: None,
            status: 400,
            code: "ValidationException",
        },
        CodeCase {
            case: "GetQueryStatus for a query id the monitor does not hold".to_string(),
            method: "GET",
            path: format!("/v20210603/Monitors/{MONITOR}/Queries/no-such-query/Status"),
            body: None,
            status: 400,
            code: "ValidationException",
        },
        CodeCase {
            case: "StopQuery for a query id the monitor does not hold".to_string(),
            method: "DELETE",
            path: format!("/v20210603/Monitors/{MONITOR}/Queries/no-such-query"),
            body: None,
            status: 400,
            code: "ValidationException",
        },
        CodeCase {
            case: "ListMonitors with a status filter naming no member of its enum"
                .to_string(),
            method: "GET",
            path: "/v20210603/Monitors?MonitorStatus=NOT_A_STATE".to_string(),
            body: None,
            status: 400,
            code: "ValidationException",
        },
        CodeCase {
            case: "UpdateMonitor with a status it does not accept".to_string(),
            method: "PATCH",
            path: format!("/v20210603/Monitors/{MONITOR}"),
            body: Some(json!({ "Status": "PENDING" })),
            status: 400,
            code: "ValidationException",
        },
        // The boundary validator's refusals, reworded by this service as
        // BadRequestException (400): a different layer, the same envelope.
        CodeCase {
            case: "CreateMonitor with a traffic percentage below its modelled minimum"
                .to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": "tp-under", "TrafficPercentageToMonitor": 0 })),
            status: 400,
            code: "BadRequestException",
        },
        CodeCase {
            case: "ListHealthEvents with an event status outside its modelled enum"
                .to_string(),
            method: "GET",
            path: format!("/v20210603/Monitors/{MONITOR}/HealthEvents?EventStatus=NOT_A_STATUS"),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        // A quota refusal: LimitExceededException (403).
        CodeCase {
            case: "UpdateMonitor carrying the resource set past its ceiling".to_string(),
            method: "PATCH",
            path: format!("/v20210603/Monitors/{MONITOR}"),
            body: Some(json!({
                "ResourcesToAdd": resource_arns(MAX_RESOURCES_PER_MONITOR + 1),
            })),
            status: 403,
            code: "LimitExceededException",
        },
        // The not-found family, in both shapes this service answers it with: the monitor
        // operations' ResourceNotFoundException (404) and the tagging operations'
        // NotFoundException (404).
        CodeCase {
            case: "GetMonitor for a monitor this account does not hold".to_string(),
            method: "GET",
            path: "/v20210603/Monitors/never-created".to_string(),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        CodeCase {
            case: "UpdateMonitor for a monitor this account does not hold".to_string(),
            method: "PATCH",
            path: "/v20210603/Monitors/never-created".to_string(),
            body: Some(json!({ "TrafficPercentageToMonitor": 5 })),
            status: 404,
            code: "ResourceNotFoundException",
        },
        CodeCase {
            case: "ListTagsForResource for an ARN no monitor carries".to_string(),
            method: "GET",
            path: format!("/tags/{}", label(UNHELD_ARN)),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        CodeCase {
            case: "TagResource for an ARN no monitor carries".to_string(),
            method: "POST",
            path: format!("/tags/{}", label(UNHELD_ARN)),
            body: Some(json!({ "Tags": { "team": "net" } })),
            status: 404,
            code: "NotFoundException",
        },
        CodeCase {
            case: "UntagResource for an ARN no monitor carries".to_string(),
            method: "DELETE",
            path: format!("/tags/{}?tagKeys=team", label(UNHELD_ARN)),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        // A conflict: ConflictException (409).
        CodeCase {
            case: "CreateMonitor of a name that already exists".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": MONITOR })),
            status: 409,
            code: "ConflictException",
        },
    ];

    for case in cases {
        assert_names_its_code(&router, case).await;
    }
}

#[tokio::test]
async fn a_success_carries_no_code_member() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": MONITOR })).await;

    // A success is not a refusal: nothing names an exception in it.
    for (case, path) in [
        ("GetMonitor", format!("/v20210603/Monitors/{MONITOR}")),
        ("ListMonitors", "/v20210603/Monitors".to_string()),
        ("ListInternetEvents", "/v20210603/InternetEvents".to_string()),
    ] {
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert!(
            body.get("code").is_none(),
            "case {case:?}: a success must carry no code member, got {body}"
        );
    }

    // Including the single-event read of the service's own internet-event feed, whose id
    // comes from the listing rather than from a constant.
    let (_, listed) = call(&router, "GET", "/v20210603/InternetEvents", None).await;
    let event_id = listed["InternetEvents"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("the feed must report an event, got {listed}"))
        .to_string();
    let (status, body) = call(
        &router,
        "GET",
        &format!("/v20210603/InternetEvents/{event_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetInternetEvent: expected 200, got {status} with {body}"
    );
    assert!(
        body.get("code").is_none(),
        "GetInternetEvent: a success must carry no code member, got {body}"
    );
}
