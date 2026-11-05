// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which instances a `DiscoverInstances` request gets when it names
//! no `HealthStatus` filter.
//!
//! The rule under test: for a service that configures health checking, the view a
//! request that names no `HealthStatus` gets is the instances the service still routes
//! traffic to — every instance except those reporting `UNHEALTHY`, which Cloud Map
//! "stops routing traffic to" — while `ALL` serves them with their status, `HEALTHY`
//! serves only the healthy ones, and a service with no health checking at all ignores
//! the filter entirely.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

// The router/dispatch harness is this crate's one test harness, in
// `tests/all/cloud_map_core.rs`.
use crate::cloud_map_core::{discovered, emulator, namespace, ok, service};

/// The namespace every service below is created in.
const NAMESPACE: &str = "suppression-ns";

/// Create a service carrying `health`, returning its id.
async fn health_checked_service(
    router: &MultiServiceRouter,
    namespace_id: &str,
    name: &str,
    health: Value,
) -> String {
    let mut body = json!({ "Name": name, "NamespaceId": namespace_id });
    if let Value::Object(members) = health {
        for (key, value) in members {
            body[key.as_str()] = value;
        }
    }
    let created = ok(router, "CreateService", body.clone()).await;
    created["Service"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateService with {body} returns an Id, got {created}"))
        .to_string()
}

/// Register one instance, optionally stating its initial custom health status.
async fn register(
    router: &MultiServiceRouter,
    service_id: &str,
    instance_id: &str,
    initial: Option<&str>,
) {
    let mut attributes = json!({ "AWS_INSTANCE_IPV4": "10.0.0.77", "AWS_INSTANCE_PORT": "8080" });
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

/// The ids one `DiscoverInstances` request answers with, for a filter that may be
/// absent.
async fn view(
    router: &MultiServiceRouter,
    service_name: &str,
    filter: Option<&str>,
) -> BTreeSet<String> {
    let mut body = json!({ "NamespaceName": NAMESPACE, "ServiceName": service_name });
    if let Some(filter) = filter {
        body["HealthStatus"] = json!(filter);
    }
    discovered(router, body).await
}

/// Submit a custom health status and wait for the clock to owe it, so the view below is
/// read against the status the submit asked for.
///
/// `UpdateInstanceCustomHealthStatus` stores the status as due later
/// (`CUSTOM_HEALTH_STATUS_SPAN`) rather than applying it at once, so a test that wants
/// the settled view waits for that instant. Blocking is fine here: the test owns the
/// runtime, and the service never sleeps for the span.
async fn settled_status(
    router: &MultiServiceRouter,
    service_id: &str,
    instance_id: &str,
    status: &str,
) {
    ok(
        router,
        "UpdateInstanceCustomHealthStatus",
        json!({ "ServiceId": service_id, "InstanceId": instance_id, "Status": status }),
    )
    .await;
    std::thread::sleep(cloudmap::state::CUSTOM_HEALTH_STATUS_SPAN + Duration::from_millis(50));
}

/// An instance a custom health check reports `UNHEALTHY` is out of the default
/// discovery view and back in it once it is reported healthy again, while `ALL` serves
/// it throughout with the status it reports.
///
/// The filter values are the model's `HealthStatusFilter` enum
/// (`HEALTH_STATUS_FILTER_VALUES` in `src/ops/types/shared_types.rs`), and the statuses
/// its `CustomHealthStatus` enum (`CUSTOM_HEALTH_STATUS_VALUES`).
#[tokio::test]
async fn an_unhealthy_instance_leaves_the_default_discovery_view_and_comes_back() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, NAMESPACE).await;
    let service_id = health_checked_service(
        &router,
        &namespace_id,
        "custom-svc",
        json!({ "HealthCheckCustomConfig": { "FailureThreshold": 1 } }),
    )
    .await;
    register(&router, &service_id, "inst-1", None).await;

    let only = BTreeSet::from(["inst-1".to_string()]);
    let none = BTreeSet::new();

    // (case, the status to settle on first, then whether each filter serves the
    // instance). The filters are in the order below, one row per stage of the chain.
    let filters = [
        None,
        Some("ALL"),
        Some("HEALTHY"),
        Some("UNHEALTHY"),
        Some("HEALTHY_OR_ELSE_ALL"),
    ];
    let cases: Vec<(&str, Option<&str>, [bool; 5])> = vec![
        (
            "the documented initial status of a custom health check",
            None,
            [true, true, true, false, true],
        ),
        (
            "reported unhealthy, so the service stops routing to it",
            Some("UNHEALTHY"),
            // The last column is failing open: nothing is healthy, so everything is
            // returned.
            [false, true, false, true, true],
        ),
        (
            "repaired back to healthy",
            Some("HEALTHY"),
            [true, true, true, false, true],
        ),
    ];

    for (case, submit, served) in cases {
        if let Some(status) = submit {
            settled_status(&router, &service_id, "inst-1", status).await;
        }
        for (filter, serves) in filters.iter().zip(served) {
            let expected = if serves { &only } else { &none };
            let found = view(&router, "custom-svc", *filter).await;
            assert_eq!(
                &found, expected,
                "case {case:?} with filter {filter:?}: expected {expected:?}, got {found:?}"
            );
        }
        // Whatever the view serves, the status every reader reports is the one stored.
        let reported = ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": service_id }),
        )
        .await;
        let expected_status = submit.unwrap_or("HEALTHY");
        assert_eq!(
            reported["Status"]["inst-1"],
            json!(expected_status),
            "case {case:?}: GetInstancesHealthStatus reports the stored status, got {reported}"
        );
        let served = ok(
            &router,
            "DiscoverInstances",
            json!({ "NamespaceName": NAMESPACE, "ServiceName": "custom-svc",
                    "HealthStatus": "ALL" }),
        )
        .await;
        assert_eq!(
            served["Instances"][0]["HealthStatus"],
            json!(expected_status),
            "case {case:?}: the ALL view reports the same status, got {served}"
        );
    }
}

/// The default view over a SET of instances and health configurations: it holds for
/// every status an instance can report and for every way a service configures health
/// checking, including one that configures none (where the filter is ignored).
#[tokio::test]
async fn the_default_view_is_every_instance_the_service_still_routes_to() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, NAMESPACE).await;

    // A custom-health service whose three instances start on three different statuses:
    // the two `AWS_INIT_HEALTH_STATUS` values and the documented default.
    let custom_id = health_checked_service(
        &router,
        &namespace_id,
        "mixed-svc",
        json!({ "HealthCheckCustomConfig": { "FailureThreshold": 1 } }),
    )
    .await;
    for (instance_id, initial) in [
        ("i-default", None),
        ("i-healthy", Some("HEALTHY")),
        ("i-unhealthy", Some("UNHEALTHY")),
    ] {
        register(&router, &custom_id, instance_id, initial).await;
    }

    // A Route 53 health check, whose verdict the service computes once the check is due
    // — before that nothing has reported on the instance, so it is still routed to.
    let checked_id = health_checked_service(
        &router,
        &namespace_id,
        "route53-svc",
        json!({ "HealthCheckConfig": { "Type": "HTTP", "ResourcePath": "/ping",
                                       "FailureThreshold": 2 } }),
    )
    .await;
    register(&router, &checked_id, "i-probed", None).await;
    let before = view(&router, "route53-svc", None).await;
    assert_eq!(
        before,
        BTreeSet::from(["i-probed".to_string()]),
        "an instance no checker has reported on yet is still routed to, got {before:?}"
    );

    // And a service with no health checking at all, where the filter is ignored.
    let plain_id = service(&router, &namespace_id, "plain-svc").await;
    register(&router, &plain_id, "i-plain", None).await;

    std::thread::sleep(cloudmap::state::ROUTE53_HEALTH_CHECK_SPAN + Duration::from_millis(50));

    // (case, service name, filter, expected ids)
    let cases: Vec<(&str, &str, Option<&str>, Vec<&str>)> = vec![
        (
            "three statuses, no filter: the unhealthy one is left out",
            "mixed-svc",
            None,
            vec!["i-default", "i-healthy"],
        ),
        (
            "three statuses under ALL",
            "mixed-svc",
            Some("ALL"),
            vec!["i-default", "i-healthy", "i-unhealthy"],
        ),
        (
            "three statuses under HEALTHY",
            "mixed-svc",
            Some("HEALTHY"),
            vec!["i-default", "i-healthy"],
        ),
        (
            "three statuses under UNHEALTHY",
            "mixed-svc",
            Some("UNHEALTHY"),
            vec!["i-unhealthy"],
        ),
        (
            "a Route 53 verdict, no filter",
            "route53-svc",
            None,
            vec![],
        ),
        (
            "a Route 53 verdict under ALL",
            "route53-svc",
            Some("ALL"),
            vec!["i-probed"],
        ),
        (
            "no health checking at all, no filter",
            "plain-svc",
            None,
            vec!["i-plain"],
        ),
        (
            "no health checking at all under HEALTHY, which is ignored",
            "plain-svc",
            Some("HEALTHY"),
            vec!["i-plain"],
        ),
        (
            "no health checking at all under UNHEALTHY, which is ignored",
            "plain-svc",
            Some("UNHEALTHY"),
            vec!["i-plain"],
        ),
    ];
    for (case, service_name, filter, expected) in cases {
        let found = view(&router, service_name, filter).await;
        let expected: BTreeSet<String> = expected.into_iter().map(String::from).collect();
        assert_eq!(
            found, expected,
            "case {case:?} on {service_name} with filter {filter:?}: expected {expected:?}, \
             got {found:?}"
        );
    }
}
