// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the health status of a Cloud Map instance: which services let a
//! client submit one through `UpdateInstanceCustomHealthStatus`, which compute one
//! themselves, and what `GetInstancesHealthStatus` and `DiscoverInstances` report.
//!
//! The rule under test: an instance's health status is settable by a client ONLY where
//! the service's health checking is a custom health check (`HealthCheckCustomConfig`);
//! every other service — one checked by Route 53 (`HealthCheckConfig`) and one that
//! configures no health checking at all — refuses the call with `CustomHealthNotFound`,
//! and where it is accepted the submitted status becomes what every later read reports
//! once the clock reaches the instant the submit made it due.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

// The router/dispatch harness is this crate's one test harness, in
// `tests/all/cloud_map_core.rs`.
use crate::cloud_map_core::{call, emulator_with_engine, namespace, ok};

/// Dispatch an operation that must fail, returning its HTTP status, wire error code and
/// message. `cloud_map_core::failure` answers the first two; the message matters here
/// because `CustomHealthNotFound`'s text is built from the request and the stored
/// service.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, String) {
    let (status, json) = call(router, operation, body).await;
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    // The awsJson error envelope carries the message as `message` or `Message`.
    let message = json["message"]
        .as_str()
        .or_else(|| json["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_ne!(status, 200, "{operation} must fail, got {json}");
    (status, code, message)
}

/// Create a service from a body that already carries its health-check configuration,
/// returning `(id, arn)` — `cloud_map_core::service` creates one with neither, and the
/// ARN is needed because `ServiceId` is modelled `Arn`.
async fn service(router: &MultiServiceRouter, body: Value) -> (String, String) {
    let created = ok(router, "CreateService", body).await;
    let service = &created["Service"];
    (
        service["Id"]
            .as_str()
            .expect("CreateService returns the service it stored")
            .to_string(),
        service["Arn"]
            .as_str()
            .expect("CreateService returns the ARN it minted")
            .to_string(),
    )
}

/// Register one instance, optionally with an `AWS_INIT_HEALTH_STATUS`.
async fn register(
    router: &MultiServiceRouter,
    service_id: &str,
    instance_id: &str,
    initial: Option<&str>,
) {
    let mut attributes = json!({ "AWS_INSTANCE_IPV4": "10.0.0.7" });
    if let Some(initial) = initial {
        attributes["AWS_INIT_HEALTH_STATUS"] = json!(initial);
    }
    ok(
        router,
        "RegisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": instance_id, "Attributes": attributes }),
    )
    .await;
}

/// The `HealthStatus` a read reports for one instance, through the implemented
/// `DiscoverInstances` reader — the round trip that proves the submit was persisted.
async fn reported_health(
    router: &MultiServiceRouter,
    namespace_name: &str,
    service_name: &str,
    instance_id: &str,
) -> String {
    let found = ok(
        router,
        "DiscoverInstances",
        json!({ "NamespaceName": namespace_name, "ServiceName": service_name, "HealthStatus": "ALL" }),
    )
    .await;
    found["Instances"]
        .as_array()
        .expect("Instances is a list")
        .iter()
        .find(|instance| instance["InstanceId"] == json!(instance_id))
        .map(|instance| {
            instance["HealthStatus"]
                .as_str()
                .expect("a discovered instance reports a HealthStatus")
                .to_string()
        })
        .unwrap_or_else(|| panic!("instance {instance_id} must be discoverable, got {found}"))
}

/// The `Status` map `GetInstancesHealthStatus` answers with, as an ordered map.
async fn health_status_map(
    router: &MultiServiceRouter,
    body: Value,
) -> (BTreeMap<String, String>, Option<String>) {
    let reported = ok(router, "GetInstancesHealthStatus", body).await;
    let statuses = reported["Status"]
        .as_object()
        .expect("Status is a map")
        .iter()
        .map(|(instance_id, status)| {
            (
                instance_id.clone(),
                status
                    .as_str()
                    .expect("a health status is a string")
                    .to_string(),
            )
        })
        .collect();
    let next = reported["NextToken"].as_str().map(String::from);
    (statuses, next)
}

/// Submit a custom health status for one instance, expecting it to be accepted.
async fn submit(router: &MultiServiceRouter, service: &str, instance_id: &str, status: &str) {
    let accepted = ok(
        router,
        "UpdateInstanceCustomHealthStatus",
        json!({ "ServiceId": service, "InstanceId": instance_id, "Status": status }),
    )
    .await;
    assert_eq!(
        accepted,
        json!({}),
        "case {status}: the operation is modelled with no output, got {accepted}"
    );
}

/// Only a service whose health checking is a custom health check accepts a submitted
/// status; a Route 53 health check answers `CustomHealthNotFound`, and a
/// service that configures no health checking at all is in no health registry and
/// answers `ServiceNotFound` — whatever status is submitted.
///
/// The Route 53 check types come from the model's `HealthCheckType` enum
/// (`HEALTH_CHECK_TYPE_VALUES` in `src/ops/types/shared_types.rs`) and the statuses from
/// its `CustomHealthStatus` enum (`CUSTOM_HEALTH_STATUS_VALUES`).
#[tokio::test]
async fn a_status_is_client_settable_only_for_a_custom_health_check() {
    let (router, _engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "settable-ns").await;

    // (case name, the service's health-check configuration, the code a submit answers
    // with — `None` when the submit is accepted)
    let mut cases: Vec<(String, Value, Option<&str>)> = vec![
        ("a custom health check".to_string(), json!({ "HealthCheckCustomConfig": { "FailureThreshold": 1 } }), None),
        ("no health checking at all".to_string(), json!({}), Some("ServiceNotFound")),
    ];
    for check_type in cloudmap::ops::types::HEALTH_CHECK_TYPE_VALUES {
        cases.push((
            format!("a Route 53 {check_type} health check"),
            json!({ "HealthCheckConfig": { "Type": check_type, "FailureThreshold": 2 } }),
            Some("CustomHealthNotFound"),
        ));
    }

    for (index, (case, configuration, refused_with)) in cases.iter().enumerate() {
        let service_name = format!("settable-svc-{index}");
        let mut body = json!({ "Name": service_name, "NamespaceId": namespace_id });
        for (member, value) in configuration.as_object().expect("an object") {
            body[member] = value.clone();
        }
        let (service_id, service_arn) = service(&router, body).await;
        register(&router, &service_id, "i-1", None).await;

        // `ServiceId` is modelled `Arn`, so each case is submitted both ways.
        for reference in [service_id.as_str(), service_arn.as_str()] {
            for status in cloudmap::ops::types::CUSTOM_HEALTH_STATUS_VALUES {
                let request =
                    json!({ "ServiceId": reference, "InstanceId": "i-1", "Status": status });
                let Some(expected_code) = *refused_with else {
                    let accepted = ok(
                        &router,
                        "UpdateInstanceCustomHealthStatus",
                        request.clone(),
                    )
                    .await;
                    assert_eq!(
                        accepted,
                        json!({}),
                        "case {case:?} status {status}: expected an empty success, got {accepted}"
                    );
                    continue;
                };
                let (http_status, code, message) = refusal(
                    &router,
                    "UpdateInstanceCustomHealthStatus",
                    request.clone(),
                )
                .await;
                assert_eq!(
                    (http_status, code.as_str()),
                    (400, expected_code),
                    "case {case:?} status {status}: expected 400 {expected_code}, \
                     got {http_status} {code}"
                );
                if expected_code == "CustomHealthNotFound" {
                    // The message is built from the request and the stored service: it
                    // names the instance the call asked about and the service by its
                    // own ARN, however the call addressed it. The service wraps the
                    // whole sentence in single quotes.
                    let expected = format!(
                        "'No Custom Health Check for instance 'i-1' in service '{service_arn}''"
                    );
                    assert_eq!(
                        message, expected,
                        "case {case:?} status {status}: expected {expected:?}, got {message:?}"
                    );
                }
            }
        }
    }
}

/// An accepted status is what every later read reports, once the clock reaches the
/// instant the submit made it due — and the most recent submit is the one that lands.
#[tokio::test]
async fn a_submitted_status_is_read_back_once_the_clock_owes_it() {
    let (router, _engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "readback-ns").await;
    let (service_id, _) = service(
        &router,
        json!({
            "Name": "readback-svc",
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;

    // (case name, AWS_INIT_HEALTH_STATUS, the statuses submitted in order, what a read
    // reports once the clock owes them)
    let cases: Vec<(&str, &str, Vec<&str>, &str)> = vec![
        ("healthy to unhealthy", "HEALTHY", vec!["UNHEALTHY"], "UNHEALTHY"),
        ("unhealthy to healthy", "UNHEALTHY", vec!["HEALTHY"], "HEALTHY"),
        (
            "the most recent submit is the one that lands",
            "HEALTHY",
            vec!["UNHEALTHY", "HEALTHY", "UNHEALTHY"],
            "UNHEALTHY",
        ),
        (
            "a repeated submit lands the same status",
            "HEALTHY",
            vec!["UNHEALTHY", "UNHEALTHY"],
            "UNHEALTHY",
        ),
        (
            "a submit withdrawn before it is due leaves the status alone",
            "UNHEALTHY",
            vec!["HEALTHY", "UNHEALTHY"],
            "UNHEALTHY",
        ),
    ];

    for (index, (case, initial, submitted, expected)) in cases.iter().enumerate() {
        let instance_id = format!("i-{index}");
        register(&router, &service_id, &instance_id, Some(initial)).await;
        let before = reported_health(&router, "readback-ns", "readback-svc", &instance_id).await;
        assert_eq!(
            before, *initial,
            "case {case:?}: a registration's initial status is reported, expected {initial}, \
             got {before}"
        );

        for status in submitted {
            submit(&router, &service_id, &instance_id, status).await;
        }

        // The change is owed, not instant: a caller reading straight after the submit
        // still sees the status the instance reported before it.
        let immediate = reported_health(&router, "readback-ns", "readback-svc", &instance_id).await;
        assert_eq!(
            immediate, *initial,
            "case {case:?}: a read before the change is due still reports {initial}, \
             got {immediate}"
        );

        // The service never sleeps for this span — it stores the due instant and
        // materialises it on read; the test is what waits for the clock to reach it.
        tokio::time::sleep(cloudmap::state::CUSTOM_HEALTH_STATUS_SPAN + Duration::from_millis(50))
            .await;

        let settled = reported_health(&router, "readback-ns", "readback-svc", &instance_id).await;
        assert_eq!(
            settled, *expected,
            "case {case:?}: expected {expected} once the clock owes it, got {settled}"
        );
        // And the stored status stays put: a second read reports the same thing.
        let again = reported_health(&router, "readback-ns", "readback-svc", &instance_id).await;
        assert_eq!(
            again, *expected,
            "case {case:?}: reading does not change the stored status, expected {expected}, \
             got {again}"
        );
    }
}

/// `HealthCheckCustomConfig`: "Sending a second or subsequent
/// UpdateInstanceCustomHealthStatus request with the same value before 30 seconds has
/// passed doesn't accelerate the change." So a repeat of the status already owed leaves
/// the stored due instant exactly where the first submit put it, while asking for a
/// different status starts the wait again.
#[tokio::test]
async fn a_repeated_submit_does_not_move_the_due_instant() {
    let (router, engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "due-ns").await;
    let (service_id, _) = service(
        &router,
        json!({
            "Name": "due-svc",
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;
    register(&router, &service_id, "i-1", Some("HEALTHY")).await;

    let store =
        engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let owed = |expected_status: &str| {
        let guard = store.lock();
        let pending = guard
            .services
            .get(&service_id)
            .and_then(|service| service.instances.get("i-1"))
            .and_then(|instance| instance.pending_health_status.clone())
            .expect("a submitted status the clock still owes");
        assert_eq!(
            pending.status, expected_status,
            "the owed status is the one most recently submitted, expected {expected_status}, \
             got {}",
            pending.status
        );
        pending.due_at
    };

    submit(&router, &service_id, "i-1", "UNHEALTHY").await;
    let first_due = owed("UNHEALTHY");

    submit(&router, &service_id, "i-1", "UNHEALTHY").await;
    let repeated_due = owed("UNHEALTHY");
    assert_eq!(
        repeated_due, first_due,
        "case repeat of the owed status: the due instant must not move, \
         expected {first_due:?}, got {repeated_due:?}"
    );

    submit(&router, &service_id, "i-1", "HEALTHY").await;
    let changed_due = owed("HEALTHY");
    assert!(
        changed_due > first_due,
        "case a different status: the wait starts again, expected later than {first_due:?}, \
         got {changed_due:?}"
    );
}

/// The call's modelled refusals: an unknown service, an unknown instance of a service
/// that does have a custom health check, and a `Status` outside the modelled enum. A
/// service may be named by its id or its ARN, since `ServiceId` is modelled `Arn`.
#[tokio::test]
async fn unknown_resources_and_bad_statuses_answer_their_modelled_errors() {
    let (router, engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "errors-ns").await;
    let (service_id, service_arn) = service(
        &router,
        json!({
            "Name": "errors-svc",
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;
    register(&router, &service_id, "i-1", None).await;

    // (case name, the request, expected HTTP status, expected wire error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a service id nothing was created with",
            json!({ "ServiceId": "srv-nothinghere0000", "InstanceId": "i-1", "Status": "HEALTHY" }),
            400,
            "ServiceNotFound",
        ),
        (
            "a service ARN nothing was created with",
            json!({ "ServiceId": format!("{service_arn}-other"), "InstanceId": "i-1",
                    "Status": "HEALTHY" }),
            400,
            "ServiceNotFound",
        ),
        (
            "an instance that was never registered",
            json!({ "ServiceId": service_id, "InstanceId": "i-absent", "Status": "HEALTHY" }),
            400,
            "InstanceNotFound",
        ),
        (
            // The modelled `CustomHealthStatus` enum is enforced before the handler
            // runs, and this service's client-input error is `InvalidInput`.
            "a status outside the CustomHealthStatus enum",
            json!({ "ServiceId": service_id, "InstanceId": "i-1", "Status": "MAYBE" }),
            400,
            "InvalidInput",
        ),
        (
            "the status UNKNOWN, which only a health checker reports",
            json!({ "ServiceId": service_id, "InstanceId": "i-1", "Status": "UNKNOWN" }),
            400,
            "InvalidInput",
        ),
    ];
    for (case, body, expected_status, expected_code) in &cases {
        let (status, code, message) =
            refusal(&router, "UpdateInstanceCustomHealthStatus", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code} \
             {message:?}"
        );
    }

    // The ARN resolves to the same service the id does, and the refusals above stored
    // nothing: the instance is still heading nowhere.
    let store =
        engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let operations_before = store.lock().operations.len();
    submit(&router, &service_arn, "i-1", "UNHEALTHY").await;
    {
        let guard = store.lock();
        let instance = guard
            .services
            .get(&service_id)
            .and_then(|service| service.instances.get("i-1"))
            .expect("the registered instance");
        assert_eq!(
            instance.pending_health_status.as_ref().map(|pending| pending.status.clone()),
            Some("UNHEALTHY".to_string()),
            "a service named by ARN reaches the same stored instance, got {instance:?}"
        );
        assert_eq!(
            guard.operations.len(),
            operations_before,
            "the operation is modelled with no OperationId, so it files no operation record"
        );
    }
}

/// A service that configures a Route 53 health check (`HealthCheckConfig`) computes the
/// status itself: the instance reports `UNKNOWN` until the check is due — "there's a
/// brief delay between when you register an instance and when the health status for the
/// instance is available" — and then the verdict the service reaches
/// (`ROUTE53_HEALTH_CHECK_VERDICT`, never `HEALTHY`, since no probe of the instance's
/// endpoint can be observed from inside the emulator). The client cannot override it.
///
/// The check types come from the model's `HealthCheckType` enum
/// (`HEALTH_CHECK_TYPE_VALUES`), and `HealthCheckConfig$FailureThreshold` is published
/// with the range 1..10, so each published bound is exercised.
#[tokio::test]
async fn a_route53_health_check_reports_the_status_the_service_computes() {
    let (router, _engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "route53-ns").await;

    // (case name, the HealthCheckConfig the service is created with)
    let mut cases: Vec<(String, Value)> = Vec::new();
    for check_type in cloudmap::ops::types::HEALTH_CHECK_TYPE_VALUES {
        for failure_threshold in [1, 10] {
            cases.push((
                format!("a {check_type} check with FailureThreshold {failure_threshold}"),
                json!({ "Type": check_type, "FailureThreshold": failure_threshold }),
            ));
        }
    }

    for (index, (case, configuration)) in cases.iter().enumerate() {
        let service_name = format!("route53-svc-{index}");
        let mut body = json!({ "Name": service_name, "NamespaceId": namespace_id });
        body["HealthCheckConfig"] = configuration.clone();
        // An HTTP check names a resource path; a TCP one does not, so the row carries
        // whatever the configuration under test needs.
        if configuration["Type"] != json!("TCP") {
            body["HealthCheckConfig"]["ResourcePath"] = json!("/ping");
        }
        let (service_id, _) = service(&router, body).await;
        register(&router, &service_id, "i-1", None).await;

        let immediate = reported_health(&router, "route53-ns", &service_name, "i-1").await;
        assert_eq!(
            immediate, "UNKNOWN",
            "case {case:?}: the service has not reported yet, expected UNKNOWN, got {immediate}"
        );
        let (reported, next) =
            health_status_map(&router, json!({ "ServiceId": service_id })).await;
        assert_eq!(
            reported,
            BTreeMap::from([("i-1".to_string(), "UNKNOWN".to_string())]),
            "case {case:?}: GetInstancesHealthStatus reports the same, got {reported:?}"
        );
        assert_eq!(
            next, None,
            "case {case:?}: one instance fits in one page, got {next:?}"
        );

        // A client cannot set the status of a check the service performs itself.
        let (status, code, _) = refusal(
            &router,
            "UpdateInstanceCustomHealthStatus",
            json!({ "ServiceId": service_id, "InstanceId": "i-1", "Status": "HEALTHY" }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "CustomHealthNotFound"),
            "case {case:?}: expected 400 CustomHealthNotFound, got {status} {code}"
        );

        // The service never sleeps for the span — it stores the due instant and
        // materialises it on read; the test is what waits for the clock to reach it.
        tokio::time::sleep(cloudmap::state::ROUTE53_HEALTH_CHECK_SPAN + Duration::from_millis(50))
            .await;

        let settled = reported_health(&router, "route53-ns", &service_name, "i-1").await;
        assert_eq!(
            settled,
            cloudmap::state::ROUTE53_HEALTH_CHECK_VERDICT,
            "case {case:?}: expected the verdict the service computes ({}), got {settled}",
            cloudmap::state::ROUTE53_HEALTH_CHECK_VERDICT
        );
        assert_ne!(
            settled, "HEALTHY",
            "case {case:?}: nothing probed the endpoint, so HEALTHY is never claimed, \
             got {settled}"
        );
        let (reported, _) = health_status_map(&router, json!({ "ServiceId": service_id })).await;
        assert_eq!(
            reported.get("i-1").map(String::as_str),
            Some(cloudmap::state::ROUTE53_HEALTH_CHECK_VERDICT),
            "case {case:?}: both readers report one stored status, got {reported:?}"
        );
    }
}

/// A status submitted for a custom health check is what `GetInstancesHealthStatus`
/// reports too, once the clock owes it — the same materialisation `DiscoverInstances`
/// performs, through the other reader.
///
/// This suite asserts only that round trip for this operation; its `Instances`
/// selection, paging and modelled refusals are covered by `tests/all/instance_health_status.rs`.
#[tokio::test]
async fn a_submitted_status_is_reported_by_get_instances_health_status_too() {
    let (router, _engine) = emulator_with_engine();
    let (_, namespace_id) = namespace(&router, "status-ns").await;
    let (service_id, _) = service(
        &router,
        json!({
            "Name": "status-svc",
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;

    // Two instances whose stored statuses differ, so a report of one is distinguishable
    // from a report of the other.
    for (instance_id, initial) in [("i-0", "HEALTHY"), ("i-1", "UNHEALTHY")] {
        register(&router, &service_id, instance_id, Some(initial)).await;
    }
    let (registered, _) = health_status_map(&router, json!({ "ServiceId": service_id })).await;
    assert_eq!(
        registered,
        BTreeMap::from([
            ("i-0".to_string(), "HEALTHY".to_string()),
            ("i-1".to_string(), "UNHEALTHY".to_string()),
        ]),
        "the registrations' statuses are what this reader reports, got {registered:?}"
    );

    // Each instance is submitted the status the other one carries, so a report that
    // ignored the submits would be indistinguishable from one that applied them.
    submit(&router, &service_id, "i-0", "UNHEALTHY").await;
    submit(&router, &service_id, "i-1", "HEALTHY").await;
    tokio::time::sleep(cloudmap::state::CUSTOM_HEALTH_STATUS_SPAN + Duration::from_millis(50))
        .await;

    let (reported, _) = health_status_map(&router, json!({ "ServiceId": service_id })).await;
    assert_eq!(
        reported,
        BTreeMap::from([
            ("i-0".to_string(), "UNHEALTHY".to_string()),
            ("i-1".to_string(), "HEALTHY".to_string()),
        ]),
        "a submitted status is read back here too, got {reported:?}"
    );
}
