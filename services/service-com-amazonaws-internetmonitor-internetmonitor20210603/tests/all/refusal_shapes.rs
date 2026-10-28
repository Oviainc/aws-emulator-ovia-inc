// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the shapes this service refuses with, and the words it refuses in.
//!
//! Three rules are under test, each over a SET of inputs rather than one recorded request:
//!
//! 1. A request this service refuses for failing input validation — a value outside a
//!    constraint the model declares on an input member, an absent required member, or a
//!    documented per-resource limit no modelled constraint expresses — is answered with
//!    HTTP 400 and the error code `BadRequestException`, stating the class of the failure
//!    (`Invalid request body`). The modelled `ValidationException` shape is what an
//!    operation answers for a refusal of its OWN semantics, which the last case below
//!    pins so the two families stay distinguishable.
//! 2. A request whose addressed resource IS the monitor it names by name, for a monitor
//!    this account does not hold, is a not-found condition: HTTP 404
//!    `ResourceNotFoundException`, whose message is built from the caller's account and
//!    the monitor's own resource path.
//! 3. A `CreateMonitor` of a name that already exists is refused with
//!    `ConflictException` (409) worded from the request: it names the submitted
//!    `MonitorName`.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator binary
//! mounts, so the boundary validator, the handlers and the wire envelope are exercised
//! the way a client exercises them.
//!
//! The harness (`emulator`, `call`, `label`, `create_monitor`, `expected_arn`) lives in
//! `crate::monitor_lifecycle` and is reused here rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::{
    invalid_resource_id_message, monitor_not_found_message, monitor_resource_path,
    INVALID_REQUEST_BODY_MESSAGE, MAX_TAGS_PER_MONITOR, MONITOR_STATE_INACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::monitor_lifecycle::{
    call, create_monitor, emulator, error_code, error_message, expected_arn, label, REGION,
};

/// `count` distinct tags, for the rows about how many tags one request may carry.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key-{index:04}"), json!(format!("value-{index}"))))
            .collect(),
    )
}

/// One refused request: how it is sent, and what the refusal must be.
struct RefusalCase {
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
    /// The error code the refusal must carry.
    code: &'static str,
    /// The message the refusal must carry.
    message: String,
}

/// Send `case` and assert its status, code and message.
async fn assert_refused(router: &MultiServiceRouter, case: RefusalCase) {
    let (status, body) = call(router, case.method, &case.path, case.body).await;
    assert_eq!(
        status, case.status,
        "case {:?}: expected {}, got {status} with {body}",
        case.case, case.status
    );
    assert_eq!(
        error_code(&body),
        case.code,
        "case {:?}: expected error code {}, got {body}",
        case.case,
        case.code
    );
    assert_eq!(
        error_message(&body),
        case.message,
        "case {:?}: expected message {:?}, got {body}",
        case.case,
        case.message
    );
}

// ---------------------------------------------------------------------------------------
// Rule 1: input validation is refused with BadRequestException
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_input_validation_failure_is_refused_with_bad_request_exception() {
    let router = emulator();
    let arn = create_monitor(&router, json!({ "MonitorName": "validated-monitor" })).await;
    let tags_path = format!("/tags/{}", label(&arn));
    let full_arn = create_monitor(
        &router,
        json!({ "MonitorName": "tag-full-monitor", "Tags": tag_map(MAX_TAGS_PER_MONITOR) }),
    )
    .await;
    let full_tags_path = format!("/tags/{}", label(&full_arn));

    // The rows span the class the rule generalises over: which operation is called, which
    // member is at fault, whether the value is numeric, a string, a map or absent
    // altogether, and whether the rule is one the model declares (so the boundary
    // validator refuses it) or one only the documentation states (so a handler does).
    // Every published bound comes from the model
    // (`smithy_query(query="operation",operation="CreateMonitor"|"TagResource")`):
    // TrafficPercentageToMonitor 1..100, MaxCityNetworksToMonitor 1..500000,
    // MonitorName len 1..255 pattern ^[a-zA-Z0-9_.-]+$, Tags 0..200 entries — and the
    // documented per-monitor maximum of 50 tags.
    let long_name = "n".repeat(256);
    let cases = vec![
        RefusalCase {
            case: "traffic percentage one below its minimum".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": "tp-under", "TrafficPercentageToMonitor": 0 })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "traffic percentage one past its maximum".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": "tp-over", "TrafficPercentageToMonitor": 101 })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "city-network maximum one below its minimum".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": "cn-under", "MaxCityNetworksToMonitor": 0 })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "city-network maximum one past its maximum".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(
                json!({ "MonitorName": "cn-over", "MaxCityNetworksToMonitor": 500_001 }),
            ),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "a name one past its maximum length".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": long_name })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "a name outside the published pattern".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": "not a valid name!" })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "the required name absent altogether".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "TrafficPercentageToMonitor": 15 })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "a create carrying one entry past the modelled tag-map length".to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({ "MonitorName": "tag-map-over", "Tags": tag_map(201) })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "a create carrying one tag past the documented per-monitor maximum"
                .to_string(),
            method: "POST",
            path: "/v20210603/Monitors".to_string(),
            body: Some(json!({
                "MonitorName": "tag-doc-over",
                "Tags": tag_map(MAX_TAGS_PER_MONITOR + 1),
            })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "a tagging call one entry past the modelled tag-map length".to_string(),
            method: "POST",
            path: tags_path.clone(),
            body: Some(json!({ "Tags": tag_map(201) })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "a merge past the documented per-monitor maximum".to_string(),
            method: "POST",
            path: full_tags_path,
            body: Some(json!({ "Tags": { "one-too-many": "v" } })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
        RefusalCase {
            case: "an update carrying a traffic percentage past its maximum".to_string(),
            method: "PATCH",
            path: "/v20210603/Monitors/validated-monitor".to_string(),
            body: Some(json!({ "TrafficPercentageToMonitor": 101 })),
            status: 400,
            code: "BadRequestException",
            message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
        },
    ];

    for case in cases {
        assert_refused(&router, case).await;
    }

    // None of the refused creates stored anything, and the refused tagging calls left the
    // stored maps as they were.
    let (_, listed) = call(&router, "GET", "/v20210603/Monitors", None).await;
    let names: Vec<String> = listed["Monitors"]
        .as_array()
        .map(|monitors| {
            monitors
                .iter()
                .map(|monitor| monitor["MonitorName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![
            "tag-full-monitor".to_string(),
            "validated-monitor".to_string()
        ],
        "a refused create must store nothing, got {listed}"
    );
    let (_, tags) = call(&router, "GET", &tags_path, None).await;
    assert_eq!(
        tags["Tags"],
        json!({}),
        "a refused tagging call must leave the stored tags as they were, got {tags}"
    );
}

// ---------------------------------------------------------------------------------------
// Rule 1, the other family: an operation's own semantics still answer ValidationException
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_refusal_of_an_operations_own_semantics_still_answers_validation_exception() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "semantic-monitor" })).await;

    // These rows are NOT input validation: each names a value the model accepts, which the
    // operation itself cannot serve. They keep the two refusal families distinguishable.
    let cases = vec![
        RefusalCase {
            case: "a Status UpdateMonitor does not accept".to_string(),
            method: "PATCH",
            path: "/v20210603/Monitors/semantic-monitor".to_string(),
            body: Some(json!({ "Status": "PENDING" })),
            status: 400,
            code: "ValidationException",
            message:
                "Status PENDING cannot be set with UpdateMonitor; accepted values are: ACTIVE, INACTIVE."
                    .to_string(),
        },
        RefusalCase {
            case: "a LinkedAccountId this monitoring account is not linked to".to_string(),
            method: "GET",
            path: "/v20210603/Monitors/semantic-monitor?LinkedAccountId=123456789012"
                .to_string(),
            body: None,
            status: 400,
            code: "ValidationException",
            message: "LinkedAccountId 123456789012 is not an account linked to this monitoring account."
                .to_string(),
        },
        RefusalCase {
            case: "a health event nothing recorded".to_string(),
            method: "GET",
            path: "/v20210603/Monitors/semantic-monitor/HealthEvents/AWS-event-0001"
                .to_string(),
            body: None,
            status: 400,
            code: "ValidationException",
            message: invalid_resource_id_message("health-event/semantic-monitor/AWS-event-0001"),
        },
    ];

    for case in cases {
        assert_refused(&router, case).await;
    }
}

// ---------------------------------------------------------------------------------------
// Rule 2: a monitor this account does not hold is a not-found condition
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_monitor_this_account_does_not_hold_is_refused_as_not_found() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "Held-Monitor" })).await;

    // The rows vary the name the request asks for along what the rule generalises over:
    // a name nothing ever created, one that differs from a held name only in case, one
    // carrying each character class the published pattern allows, and one at the
    // published maximum length.
    let names = vec![
        "never-created".to_string(),
        "held-monitor".to_string(),
        "mixed_Name.0-9".to_string(),
        "m".repeat(255),
    ];

    for name in names {
        for (operation, method, body) in [
            ("GetMonitor", "GET", None),
            (
                "UpdateMonitor",
                "PATCH",
                Some(json!({ "TrafficPercentageToMonitor": 5 })),
            ),
        ] {
            assert_refused(
                &router,
                RefusalCase {
                    case: format!("{operation} for {name}"),
                    method,
                    path: format!("/v20210603/Monitors/{}", label(&name)),
                    body,
                    status: 404,
                    code: "ResourceNotFoundException",
                    message: monitor_not_found_message(&name),
                },
            )
            .await;
        }
    }

    // The monitor that IS held answers neither refusal.
    let (status, held) = call(&router, "GET", "/v20210603/Monitors/Held-Monitor", None).await;
    assert_eq!(
        status, 200,
        "the held monitor: expected 200, got {status} with {held}"
    );
}

#[tokio::test]
async fn a_deleted_monitor_is_refused_as_not_found_by_the_reader() {
    let router = emulator();
    let name = "deleted-monitor";
    create_monitor(&router, json!({ "MonitorName": name })).await;

    let (status, updated) = call(
        &router,
        "PATCH",
        &format!("/v20210603/Monitors/{name}"),
        Some(json!({ "Status": MONITOR_STATE_INACTIVE })),
    )
    .await;
    assert_eq!(
        status, 200,
        "deactivating: expected 200, got {status} with {updated}"
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/v20210603/Monitors/{name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting: expected 200, got {status} with {deleted}"
    );

    // The delete is observable as an absence, and the absence is a not-found condition —
    // the one a cleanup or waiter loop terminates on.
    assert_refused(
        &router,
        RefusalCase {
            case: format!("GetMonitor after deleting {name}"),
            method: "GET",
            path: format!("/v20210603/Monitors/{name}"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            message: monitor_not_found_message(name),
        },
    )
    .await;

    // The not-found message names the monitor's own resource path, the same one its ARN
    // carries.
    assert!(
        expected_arn(REGION, name).ends_with(&monitor_resource_path(name)),
        "the resource path in the message must be the one the ARN carries, got {}",
        expected_arn(REGION, name)
    );
}

// ---------------------------------------------------------------------------------------
// Rule 3: a duplicate name conflicts, worded from the request
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_create_of_a_name_that_exists_conflicts_naming_that_name() {
    let router = emulator();

    // The rows vary the name along what the rule generalises over: its length and the
    // character classes the published pattern allows.
    let names = vec![
        "x".to_string(),
        "dup-monitor".to_string(),
        "Dup_Monitor.1-2".to_string(),
        "d".repeat(255),
    ];

    for name in names {
        create_monitor(&router, json!({ "MonitorName": name })).await;

        // A different client token (or none) for a name that exists conflicts, and the
        // message names the name the request submitted — never the one some other request
        // used.
        for (case, body) in [
            (
                "no client token",
                json!({ "MonitorName": name, "TrafficPercentageToMonitor": 90 }),
            ),
            (
                "a client token of its own",
                json!({ "MonitorName": name, "ClientToken": format!("token-{name}") }),
            ),
        ] {
            assert_refused(
                &router,
                RefusalCase {
                    case: format!("{case} for {name}"),
                    method: "POST",
                    path: "/v20210603/Monitors".to_string(),
                    body: Some(body),
                    status: 409,
                    code: "ConflictException",
                    message: format!("Monitor with name {name} already exists"),
                },
            )
            .await;
        }
    }
}
