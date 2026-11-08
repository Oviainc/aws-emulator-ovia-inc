// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateService`.
//!
//! The rule under test: an update applies the `ServiceChange` it is given to the stored
//! service and files an `UPDATE_SERVICE` operation for it, and what it may change is
//! exactly what the model says is mutable — a `DnsRecord`'s `TTL` for a record type the
//! service already carries (never the record type itself: "The record type of a service
//! specified in a `DnsRecord` object can't be updated"), and the `HealthCheckConfig`,
//! which may be added, replaced or deleted but never have its `Type` changed ("You can't
//! change the value of `Type` after you create a health check"). A configuration the
//! change omits is deleted, a `HealthCheckCustomConfig` is left alone, and a service in
//! an HTTP namespace cannot be updated at all. Every answer is read back through
//! `GetService`, `GetOperation` and `GetInstancesHealthStatus`, so it comes from state.
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
use crate::cloud_map_core::{call, dns_namespace, emulator, failure, namespace, ok};

/// The record types the model's `RecordType` enum publishes and the `RecordTTL` range,
/// taken from the crate's own catalogs rather than copied
/// (`cloudmap::ops::types::RECORD_TYPE_VALUES`, and
/// `smithy_query(query="shape",shape="RecordTTL")`: `range 0..=2147483647`).
const RECORD_TYPES: &[&str] = cloudmap::ops::types::RECORD_TYPE_VALUES;
const MIN_TTL: i64 = 0;
const MAX_TTL: i64 = 2_147_483_647;

/// The health check types the model's `HealthCheckType` enum publishes, and the range
/// `HealthCheckConfig$FailureThreshold` is published with (1..10).
const HEALTH_CHECK_TYPES: &[&str] = cloudmap::ops::types::HEALTH_CHECK_TYPE_VALUES;
const MIN_FAILURE_THRESHOLD: i64 = 1;
const MAX_FAILURE_THRESHOLD: i64 = 10;

/// Create a service in `namespace_id` from the extra request members a row needs,
/// returning its id.
async fn dns_service(
    router: &MultiServiceRouter,
    namespace_id: &str,
    name: &str,
    extra: Value,
) -> String {
    let mut body = json!({ "Name": name, "NamespaceId": namespace_id });
    if let Value::Object(members) = extra {
        for (key, value) in members {
            body[key.as_str()] = value;
        }
    }
    let created = ok(router, "CreateService", body).await;
    created["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string()
}

/// `GetService`'s view of one service: the reader every assertion below goes through.
async fn read(router: &MultiServiceRouter, reference: &str) -> Value {
    ok(router, "GetService", json!({ "Id": reference })).await["Service"].clone()
}

/// `UpdateService` with `change` as the `ServiceChange`, returning the `OperationId`.
async fn update(router: &MultiServiceRouter, reference: &str, change: Value) -> String {
    let answered = ok(
        router,
        "UpdateService",
        json!({ "Id": reference, "Service": change }),
    )
    .await;
    answered["OperationId"]
        .as_str()
        .expect("UpdateService returns the OperationId of the operation it filed")
        .to_string()
}

/// The `Status` map `GetInstancesHealthStatus` answers for one service.
async fn health_statuses(
    router: &MultiServiceRouter,
    service_id: &str,
) -> BTreeMap<String, String> {
    let reported = ok(
        router,
        "GetInstancesHealthStatus",
        json!({ "ServiceId": service_id }),
    )
    .await;
    reported["Status"]
        .as_object()
        .expect("GetInstancesHealthStatus returns a Status map")
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
        .collect()
}

/// A `DnsRecord` object.
fn record(record_type: &str, ttl: i64) -> Value {
    json!({ "Type": record_type, "TTL": ttl })
}

/// A change may restate a record type the service already carries — with any `TTL` the
/// published range admits — or leave one out, which deletes that record configuration;
/// it may never name a record type the service does not carry, state one twice, or carry
/// a value outside the published range or the published type catalog.
#[tokio::test]
async fn a_change_may_retime_a_stored_record_type_but_never_introduce_one() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "records").await;

    // (case, the records the service is created with, the records the change states,
    //  the records stored afterwards — `None` when the change must be refused)
    let cases: Vec<(&str, Value, Value, Option<Value>)> = vec![
        (
            "a new TTL for the one stored record type",
            json!([record("A", 60)]),
            json!([record("A", 300)]),
            Some(json!([record("A", 300)])),
        ),
        (
            "the TTL at the published minimum",
            json!([record("SRV", 60)]),
            json!([record("SRV", MIN_TTL)]),
            Some(json!([record("SRV", MIN_TTL)])),
        ),
        (
            "the TTL at the published maximum",
            json!([record("CNAME", 60)]),
            json!([record("CNAME", MAX_TTL)]),
            Some(json!([record("CNAME", MAX_TTL)])),
        ),
        (
            "the same TTL restated, which changes nothing",
            json!([record("A", 60)]),
            json!([record("A", 60)]),
            Some(json!([record("A", 60)])),
        ),
        (
            "both stored record types retimed in one change",
            json!([record("A", 60), record("AAAA", 60)]),
            json!([record("A", 10), record("AAAA", 20)]),
            Some(json!([record("A", 10), record("AAAA", 20)])),
        ),
        (
            "omitting one of two record types deletes that configuration",
            json!([record("A", 60), record("AAAA", 60)]),
            json!([record("AAAA", 20)]),
            Some(json!([record("AAAA", 20)])),
        ),
        (
            "a changed record type is refused",
            json!([record("A", 60)]),
            json!([record("AAAA", 60)]),
            None,
        ),
        (
            "a record type added beside a stored one is refused",
            json!([record("A", 60)]),
            json!([record("A", 60), record("SRV", 60)]),
            None,
        ),
        (
            "a record type the service never carried is refused",
            json!([record("SRV", 60)]),
            json!([record("CNAME", 60)]),
            None,
        ),
        (
            "one stored record type restated twice is refused",
            json!([record("A", 60)]),
            json!([record("A", 60), record("A", 120)]),
            None,
        ),
        (
            "a TTL one past the published maximum is refused",
            json!([record("A", 60)]),
            json!([record("A", MAX_TTL + 1)]),
            None,
        ),
        (
            "a TTL one below the published minimum is refused",
            json!([record("A", 60)]),
            json!([record("A", MIN_TTL - 1)]),
            None,
        ),
        (
            "a record type outside the published catalog is refused",
            json!([record("A", 60)]),
            json!([record("TXT", 60)]),
            None,
        ),
    ];

    for (index, (case, stored, change, want)) in cases.into_iter().enumerate() {
        let service_id = dns_service(
            &router,
            &namespace_id,
            &format!("records-{index}"),
            json!({ "DnsConfig": { "DnsRecords": stored.clone() } }),
        )
        .await;

        match want {
            Some(expected) => {
                update(
                    &router,
                    &service_id,
                    json!({ "DnsConfig": { "DnsRecords": change } }),
                )
                .await;
                let service = read(&router, &service_id).await;
                assert_eq!(
                    service["DnsConfig"]["DnsRecords"], expected,
                    "case {case:?}: expected the stored records to be {expected}, got {service}"
                );
            }
            None => {
                let (status, code) = failure(
                    &router,
                    "UpdateService",
                    json!({ "Id": service_id, "Service": { "DnsConfig": { "DnsRecords": change } } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "InvalidInput"),
                    "case {case:?}: expected 400 InvalidInput, got {status} {code}"
                );
                let service = read(&router, &service_id).await;
                assert_eq!(
                    service["DnsConfig"]["DnsRecords"], stored,
                    "case {case:?}: a refused change stores nothing, got {service}"
                );
            }
        }
    }
}

/// The same rule over the whole published `RecordType` catalog: whichever type a service
/// carries, a change may retime that type and is refused for every other one.
#[tokio::test]
async fn every_published_record_type_is_retimable_and_immutable() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "catalog").await;

    for (index, stored_type) in RECORD_TYPES.iter().enumerate() {
        let service_id = dns_service(
            &router,
            &namespace_id,
            &format!("catalog-{index}"),
            json!({ "DnsConfig": { "DnsRecords": [record(stored_type, 60)] } }),
        )
        .await;

        update(
            &router,
            &service_id,
            json!({ "DnsConfig": { "DnsRecords": [record(stored_type, 120)] } }),
        )
        .await;
        let service = read(&router, &service_id).await;
        assert_eq!(
            service["DnsConfig"]["DnsRecords"],
            json!([record(stored_type, 120)]),
            "case {stored_type:?}: the stored record type takes a new TTL, got {service}"
        );

        for other_type in RECORD_TYPES.iter().filter(|other| *other != stored_type) {
            let (status, code) = failure(
                &router,
                "UpdateService",
                json!({
                    "Id": service_id,
                    "Service": { "DnsConfig": { "DnsRecords": [record(other_type, 120)] } },
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInput"),
                "case {stored_type:?} -> {other_type:?}: a record type change is refused, got \
                 {status} {code}"
            );
        }

        let service = read(&router, &service_id).await;
        assert_eq!(
            service["DnsConfig"]["DnsRecords"],
            json!([record(stored_type, 120)]),
            "case {stored_type:?}: no refused change reached the store, got {service}"
        );
    }
}

/// Omitting the whole `DnsConfig`, or stating no record in it, deletes the service's
/// record configurations — and with them its DNS discoverability, so `Service$Type`
/// follows: "If you omit any existing DnsRecords … from an UpdateService request, the
/// configurations are deleted from the service."
#[tokio::test]
async fn a_change_that_states_no_records_deletes_the_record_configurations() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "omitted").await;

    // (case, the change that drops the records)
    for (index, (case, change)) in [
        ("the DnsConfig omitted entirely", json!({})),
        (
            "a DnsConfig that states no record",
            json!({ "DnsConfig": { "DnsRecords": [] } }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let service_id = dns_service(
            &router,
            &namespace_id,
            &format!("omitted-{index}"),
            json!({ "DnsConfig": { "DnsRecords": [record("A", 60), record("AAAA", 60)] } }),
        )
        .await;
        let before = read(&router, &service_id).await;
        assert_eq!(
            before["Type"],
            json!("DNS_HTTP"),
            "case {case:?}: a service with DNS records is DNS_HTTP, got {before}"
        );

        update(&router, &service_id, change).await;

        let service = read(&router, &service_id).await;
        // The member itself stays on the shape — a service always reports a
        // `DnsConfig` — so "deleted" is read off the records it no longer registers.
        assert_eq!(
            service["DnsConfig"]["DnsRecords"],
            json!([]),
            "case {case:?}: the record configurations are deleted, got {service}"
        );
        assert_eq!(
            service["Type"],
            json!("HTTP"),
            "case {case:?}: a service that registers no record is HTTP, got {service}"
        );

        // And the records cannot come back through an update: the type is not there to
        // retime any more.
        let (status, code) = failure(
            &router,
            "UpdateService",
            json!({
                "Id": service_id,
                "Service": { "DnsConfig": { "DnsRecords": [record("A", 60)] } },
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?}: a deleted record type can't be re-added, got {status} {code}"
        );
    }
}

/// One row of the health-check table below: the case name, the `HealthCheckConfig` the
/// service is created with, the one the change states (`None` meaning the change omits
/// it) and the outcome — `None` for "the change is refused", `Some(None)` for "no check
/// is stored afterwards", `Some(Some(config))` for the check stored afterwards.
type CheckRow = (
    &'static str,
    Option<Value>,
    Option<Value>,
    Option<Option<Value>>,
);

/// `HealthCheckConfig` may be added, replaced or deleted; its `Type` may not change, a
/// value outside a published catalog or range is refused, and a service checked by a
/// `HealthCheckCustomConfig` cannot be given one.
#[tokio::test]
async fn a_health_check_is_addable_replaceable_and_deletable_but_its_type_is_fixed() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "checks").await;

    // (case, the HealthCheckConfig the service is created with, the one the change
    //  states, the one stored afterwards — `None` as the change means "omitted", and
    //  `None` as the outcome means the change must be refused)
    let cases: Vec<CheckRow> = vec![
        (
            "a check added to a service that had none",
            None,
            Some(json!({ "Type": "HTTP", "ResourcePath": "/health", "FailureThreshold": 2 })),
            Some(Some(
                json!({ "Type": "HTTP", "ResourcePath": "/health", "FailureThreshold": 2 }),
            )),
        ),
        (
            "a stored check replaced member by member",
            Some(json!({ "Type": "HTTP", "ResourcePath": "/a", "FailureThreshold": 2 })),
            Some(json!({ "Type": "HTTP", "ResourcePath": "/b", "FailureThreshold": 3 })),
            Some(Some(
                json!({ "Type": "HTTP", "ResourcePath": "/b", "FailureThreshold": 3 })),
            ),
        ),
        (
            "a replacement that states only the required Type drops the rest",
            Some(json!({ "Type": "HTTP", "ResourcePath": "/a", "FailureThreshold": 2 })),
            Some(json!({ "Type": "HTTP" })),
            Some(Some(json!({ "Type": "HTTP" }))),
        ),
        (
            "the check omitted, which deletes it",
            Some(json!({ "Type": "HTTP", "ResourcePath": "/a" })),
            None,
            Some(None),
        ),
        (
            "no check either side",
            None,
            None,
            Some(None),
        ),
        (
            "the FailureThreshold at the published minimum",
            Some(json!({ "Type": "HTTPS", "ResourcePath": "/x", "FailureThreshold": 5 })),
            Some(json!({
                "Type": "HTTPS",
                "ResourcePath": "/x",
                "FailureThreshold": MIN_FAILURE_THRESHOLD,
            })),
            Some(Some(json!({
                "Type": "HTTPS",
                "ResourcePath": "/x",
                "FailureThreshold": MIN_FAILURE_THRESHOLD,
            }))),
        ),
        (
            "the FailureThreshold at the published maximum",
            Some(json!({ "Type": "TCP", "FailureThreshold": 5 })),
            Some(json!({ "Type": "TCP", "FailureThreshold": MAX_FAILURE_THRESHOLD })),
            Some(Some(
                json!({ "Type": "TCP", "FailureThreshold": MAX_FAILURE_THRESHOLD }),
            )),
        ),
        (
            "a FailureThreshold one below the published minimum is refused",
            Some(json!({ "Type": "HTTP" })),
            Some(json!({ "Type": "HTTP", "FailureThreshold": MIN_FAILURE_THRESHOLD - 1 })),
            None,
        ),
        (
            "a FailureThreshold one past the published maximum is refused",
            Some(json!({ "Type": "HTTP" })),
            Some(json!({ "Type": "HTTP", "FailureThreshold": MAX_FAILURE_THRESHOLD + 1 })),
            None,
        ),
        (
            "a check type outside the published catalog is refused",
            Some(json!({ "Type": "HTTP" })),
            Some(json!({ "Type": "SMTP" })),
            None,
        ),
    ];

    for (index, (case, stored, change, want)) in cases.into_iter().enumerate() {
        let mut create = json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } });
        if let Some(stored) = stored.clone() {
            create["HealthCheckConfig"] = stored;
        }
        let service_id = dns_service(
            &router,
            &namespace_id,
            &format!("checks-{index}"),
            create,
        )
        .await;

        // Every change restates the stored record, so a row says only what the health
        // check does.
        let mut body = json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } });
        if let Some(change) = change.clone() {
            body["HealthCheckConfig"] = change;
        }

        match want {
            Some(expected) => {
                update(&router, &service_id, body).await;
                let service = read(&router, &service_id).await;
                match expected {
                    Some(expected) => assert_eq!(
                        service["HealthCheckConfig"], expected,
                        "case {case:?}: expected the stored check to be {expected}, got {service}"
                    ),
                    None => assert!(
                        service["HealthCheckConfig"].is_null(),
                        "case {case:?}: the check is deleted, got {service}"
                    ),
                }
                assert_eq!(
                    service["DnsConfig"]["DnsRecords"],
                    json!([record("A", 60)]),
                    "case {case:?}: the restated record survives the change, got {service}"
                );
            }
            None => {
                let (status, code) = failure(
                    &router,
                    "UpdateService",
                    json!({ "Id": service_id, "Service": body }),
                )
                .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "InvalidInput"),
                    "case {case:?}: expected 400 InvalidInput, got {status} {code}"
                );
                let service = read(&router, &service_id).await;
                match stored {
                    Some(stored) => assert_eq!(
                        service["HealthCheckConfig"], stored,
                        "case {case:?}: a refused change stores nothing, got {service}"
                    ),
                    None => assert!(
                        service["HealthCheckConfig"].is_null(),
                        "case {case:?}: a refused change stores nothing, got {service}"
                    ),
                }
            }
        }
    }

    // Over the whole published `HealthCheckType` catalog: a stored check takes a
    // replacement of its own type and refuses every other one.
    for (index, stored_type) in HEALTH_CHECK_TYPES.iter().enumerate() {
        let service_id = dns_service(
            &router,
            &namespace_id,
            &format!("check-type-{index}"),
            json!({
                "DnsConfig": { "DnsRecords": [record("A", 60)] },
                "HealthCheckConfig": { "Type": stored_type, "FailureThreshold": 2 },
            }),
        )
        .await;

        update(
            &router,
            &service_id,
            json!({
                "DnsConfig": { "DnsRecords": [record("A", 60)] },
                "HealthCheckConfig": { "Type": stored_type, "FailureThreshold": 4 },
            }),
        )
        .await;
        let service = read(&router, &service_id).await;
        assert_eq!(
            service["HealthCheckConfig"],
            json!({ "Type": stored_type, "FailureThreshold": 4 }),
            "case {stored_type:?}: a check of the stored type is replaced, got {service}"
        );

        for other_type in HEALTH_CHECK_TYPES
            .iter()
            .filter(|other| *other != stored_type)
        {
            let (status, code) = failure(
                &router,
                "UpdateService",
                json!({
                    "Id": service_id,
                    "Service": {
                        "DnsConfig": { "DnsRecords": [record("A", 60)] },
                        "HealthCheckConfig": { "Type": other_type },
                    },
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInput"),
                "case {stored_type:?} -> {other_type:?}: a check type change is refused, got \
                 {status} {code}"
            );
        }
    }

    // A service checked by a `HealthCheckCustomConfig` keeps it — the change has no
    // member for it — and cannot be given a `HealthCheckConfig` beside it.
    let custom = dns_service(
        &router,
        &namespace_id,
        "custom-checked",
        json!({
            "DnsConfig": { "DnsRecords": [record("A", 60)] },
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;
    let (status, code) = failure(
        &router,
        "UpdateService",
        json!({
            "Id": custom,
            "Service": {
                "DnsConfig": { "DnsRecords": [record("A", 60)] },
                "HealthCheckConfig": { "Type": "HTTP" },
            },
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInput"),
        "a custom-checked service can't also be Route 53 checked, got {status} {code}"
    );
    update(
        &router,
        &custom,
        json!({ "DnsConfig": { "DnsRecords": [record("A", 120)] } }),
    )
    .await;
    let service = read(&router, &custom).await;
    assert_eq!(
        service["HealthCheckCustomConfig"],
        json!({ "FailureThreshold": 1 }),
        "an update that omits it does not delete the custom health check, got {service}"
    );
}

/// An update files one `UPDATE_SERVICE` operation per call, targeting the service it
/// changed, which `GetOperation` resolves — `SUBMITTED` until the clock reaches the
/// instant the call made its success due.
#[tokio::test]
async fn an_update_files_an_update_service_operation_for_the_service() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "operations").await;
    let service_id = dns_service(
        &router,
        &namespace_id,
        "operations-svc",
        json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } }),
    )
    .await;

    let mut filed = Vec::new();
    for ttl in [10, 20, 30] {
        let operation_id = update(
            &router,
            &service_id,
            json!({ "DnsConfig": { "DnsRecords": [record("A", ttl)] } }),
        )
        .await;
        let operation = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        assert_eq!(
            operation["Operation"]["Type"],
            json!("UPDATE_SERVICE"),
            "case TTL {ttl}: the filed operation is an UPDATE_SERVICE, got {operation}"
        );
        assert_eq!(
            operation["Operation"]["Targets"]["SERVICE"],
            json!(service_id),
            "case TTL {ttl}: the operation targets the updated service, got {operation}"
        );
        assert_eq!(
            operation["Operation"]["Status"],
            json!("SUBMITTED"),
            "case TTL {ttl}: a fresh operation has not succeeded yet, got {operation}"
        );
        filed.push(operation_id);
    }

    // The service never sleeps for the span — it stores the due instant and materialises
    // the transition on read — so the test waits for it.
    tokio::time::sleep(cloudmap::state::OPERATION_SUCCESS_SPAN + Duration::from_millis(50)).await;
    for operation_id in &filed {
        let operation = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        assert_eq!(
            operation["Operation"]["Status"],
            json!("SUCCESS"),
            "case {operation_id:?}: the operation succeeds once it is due, got {operation}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> = filed.iter().collect();
    assert_eq!(
        distinct.len(),
        filed.len(),
        "each update files its own operation, got {filed:?}"
    );
}

/// `ServiceChange$Description`: a stated description replaces the stored one, the empty
/// string clears it, and a change that states none leaves it alone — the documented
/// deletion-by-omission covers `DnsRecords` and `HealthCheckConfig` only.
#[tokio::test]
async fn a_stated_description_replaces_the_stored_one() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "descriptions").await;
    let service_id = dns_service(
        &router,
        &namespace_id,
        "described",
        json!({
            "Description": "the first description",
            "DnsConfig": { "DnsRecords": [record("A", 60)] },
        }),
    )
    .await;

    // (case, the Description the change states, the description stored afterwards)
    let cases: Vec<(&str, Option<Value>, Option<&str>)> = vec![
        ("no Description, so the stored one stays", None, Some("the first description")),
        ("a replacement", Some(json!("the second")), Some("the second")),
        ("unicode text", Some(json!("日本語 — ✅")), Some("日本語 — ✅")),
        ("text at the published 1024 maximum", Some(json!("d".repeat(1024))), None),
        ("the empty string, which clears it", Some(json!("")), None),
        ("still nothing stated", None, None),
    ];

    for (case, stated, want) in cases {
        let mut change = json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } });
        if let Some(stated) = stated.clone() {
            change["Description"] = stated;
        }
        update(&router, &service_id, change).await;

        let service = read(&router, &service_id).await;
        // The 1024-character row expects the text it stated, which the row table cannot
        // hold inline; every other row states its expectation directly.
        let expected = match (case, want) {
            ("text at the published 1024 maximum", _) => json!("d".repeat(1024)),
            (_, Some(text)) => json!(text),
            (_, None) => Value::Null,
        };
        assert_eq!(
            service["Description"], expected,
            "case {case:?}: expected the stored description to be {expected}, got {service}"
        );
    }
}

/// "You can't call UpdateService and update settings … When the service is associated
/// with an HTTP namespace": such a service is refused with `InvalidInput`, and a
/// reference nothing stored with `ServiceNotFound`.
#[tokio::test]
async fn an_http_namespace_service_and_an_unknown_reference_are_refused() {
    let router = emulator();
    let (_, http_namespace) = namespace(&router, "http-ns").await;
    let service_id = dns_service(&router, &http_namespace, "http-svc", json!({})).await;

    let (status, code) = failure(
        &router,
        "UpdateService",
        json!({ "Id": service_id, "Service": { "Description": "a new description" } }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInput"),
        "a service in an HTTP namespace can't be updated, got {status} {code}"
    );
    let service = read(&router, &service_id).await;
    assert!(
        service["Description"].is_null(),
        "the refused change stored nothing, got {service}"
    );

    for (case, reference) in [
        ("an id no service carries", "srv-000000000000000"),
        (
            "an ARN no service carries",
            "arn:aws:servicediscovery:us-west-2:000000000000:service/srv-0",
        ),
        ("an empty reference", ""),
        ("a namespace id, not a service id", http_namespace.as_str()),
    ] {
        let (status, code) = failure(
            &router,
            "UpdateService",
            json!({ "Id": reference, "Service": { "Description": "x" } }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ServiceNotFound"),
            "case {case:?}: expected 400 ServiceNotFound, got {status} {code}"
        );
    }
}

/// A service is addressable by its id or its ARN, and the update is applied to the one
/// stored record either way.
#[tokio::test]
async fn a_service_is_updatable_by_id_and_by_arn() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "references").await;
    let service_id = dns_service(
        &router,
        &namespace_id,
        "by-reference",
        json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } }),
    )
    .await;
    let service_arn = read(&router, &service_id).await["Arn"]
        .as_str()
        .expect("a stored service carries its ARN")
        .to_string();

    for (case, reference, ttl) in [
        ("the id", service_id.clone(), 11),
        ("the ARN", service_arn, 22),
    ] {
        update(
            &router,
            &reference,
            json!({ "DnsConfig": { "DnsRecords": [record("A", ttl)] } }),
        )
        .await;
        let service = read(&router, &service_id).await;
        assert_eq!(
            service["DnsConfig"]["DnsRecords"],
            json!([record("A", ttl)]),
            "case {case:?}: both references reach the one stored service, got {service}"
        );
    }
}

/// "When you update settings for a service, Cloud Map also updates the corresponding
/// settings in all the records and health checks that were created by using the
/// specified service": a change to the health check re-creates every instance's check,
/// so each instance is back to `UNKNOWN` with the new check's verdict owed; deleting the
/// check leaves nothing to report; and a change that leaves the check as it was does not
/// disturb the statuses.
#[tokio::test]
async fn a_health_check_change_re_creates_every_instance_health_check() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "instances").await;
    let service_id = dns_service(
        &router,
        &namespace_id,
        "instance-checks",
        json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } }),
    )
    .await;
    for instance_id in ["i-1", "i-2"] {
        ok(
            &router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": instance_id,
                "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.7" },
            }),
        )
        .await;
    }

    let unknown = BTreeMap::from([
        ("i-1".to_string(), cloudmap::state::HEALTH_STATUS_UNKNOWN.to_string()),
        ("i-2".to_string(), cloudmap::state::HEALTH_STATUS_UNKNOWN.to_string()),
    ]);
    let checked = BTreeMap::from([
        (
            "i-1".to_string(),
            cloudmap::state::ROUTE53_HEALTH_CHECK_VERDICT.to_string(),
        ),
        (
            "i-2".to_string(),
            cloudmap::state::ROUTE53_HEALTH_CHECK_VERDICT.to_string(),
        ),
    ]);

    let actual = health_statuses(&router, &service_id).await;
    assert_eq!(
        actual, unknown,
        "with no health check configured nothing reports on an instance, got {actual:?}"
    );

    // Adding the check: the instances' checks are created, so nothing has reported yet.
    update(
        &router,
        &service_id,
        json!({
            "DnsConfig": { "DnsRecords": [record("A", 60)] },
            "HealthCheckConfig": { "Type": "HTTP", "ResourcePath": "/ping" },
        }),
    )
    .await;
    let actual = health_statuses(&router, &service_id).await;
    assert_eq!(
        actual, unknown,
        "a freshly created check has not reported yet, got {actual:?}"
    );
    tokio::time::sleep(cloudmap::state::ROUTE53_HEALTH_CHECK_SPAN + Duration::from_millis(50))
        .await;
    let actual = health_statuses(&router, &service_id).await;
    assert_eq!(
        actual, checked,
        "once the check is due it reports the verdict the service computes, got {actual:?}"
    );

    // A change that leaves the check as it was does not re-create it.
    update(
        &router,
        &service_id,
        json!({
            "DnsConfig": { "DnsRecords": [record("A", 120)] },
            "HealthCheckConfig": { "Type": "HTTP", "ResourcePath": "/ping" },
        }),
    )
    .await;
    let actual = health_statuses(&router, &service_id).await;
    assert_eq!(
        actual, checked,
        "restating the stored check leaves the reported statuses alone, got {actual:?}"
    );

    // Deleting the check: nothing checks the instances any more, at any later instant.
    update(
        &router,
        &service_id,
        json!({ "DnsConfig": { "DnsRecords": [record("A", 120)] } }),
    )
    .await;
    for when in ["immediately", "after the check span"] {
        let actual = health_statuses(&router, &service_id).await;
        assert_eq!(
            actual, unknown,
            "case {when:?}: a deleted health check reports nothing, got {actual:?}"
        );
        tokio::time::sleep(cloudmap::state::ROUTE53_HEALTH_CHECK_SPAN + Duration::from_millis(50))
            .await;
    }
}

/// A read is free of side effects: `GetService` repeated after an update answers the
/// same service every time, and the update itself is the only writer.
#[tokio::test]
async fn repeated_reads_after_an_update_answer_the_same_service() {
    let router = emulator();
    let namespace_id = dns_namespace(&router, "reads").await;
    let service_id = dns_service(
        &router,
        &namespace_id,
        "reads-svc",
        json!({ "DnsConfig": { "DnsRecords": [record("A", 60)] } }),
    )
    .await;
    update(
        &router,
        &service_id,
        json!({
            "Description": "settled",
            "DnsConfig": { "DnsRecords": [record("A", 45)] },
        }),
    )
    .await;

    let first = read(&router, &service_id).await;
    for attempt in 1..=3 {
        let again = read(&router, &service_id).await;
        assert_eq!(
            again, first,
            "read {attempt}: a reporting call changes nothing, got {again}"
        );
    }

    // The same change applied twice is the same stored service, so an update is
    // idempotent in its effect.
    let before = first.clone();
    update(
        &router,
        &service_id,
        json!({
            "Description": "settled",
            "DnsConfig": { "DnsRecords": [record("A", 45)] },
        }),
    )
    .await;
    let after = read(&router, &service_id).await;
    assert_eq!(
        after, before,
        "re-applying the same change leaves the same service, got {after}"
    );

    // And the wire answer carries nothing but the OperationId.
    let (status, body) = call(
        &router,
        "UpdateService",
        json!({
            "Id": service_id,
            "Service": { "DnsConfig": { "DnsRecords": [record("A", 45)] } },
        }),
    )
    .await;
    assert_eq!(status, 200, "the update must succeed, got {status}: {body}");
    let members: Vec<&String> = body
        .as_object()
        .expect("an awsJson1_1 output is an object")
        .keys()
        .collect();
    assert_eq!(
        members,
        vec!["OperationId"],
        "UpdateService answers with the OperationId alone, got {body}"
    );
}
