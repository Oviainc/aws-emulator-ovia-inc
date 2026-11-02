// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetInstancesHealthStatus`: the health status it reports comes
//! from the stored instance records, the `Instances` list selects which of them are
//! reported, `MaxResults`/`NextToken` cut the report into pages, and a request naming
//! something that is not stored is refused with the error the model declares.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`), reusing
//! the harness in `cloud_map_core`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::cloud_map_core::{call, emulator, failure, namespace, ok, service};

/// The `HealthStatus` values this API reports, from the model's `HealthStatus` enum
/// (`smithy_query(query="shape",shape="HealthStatus")` -> `HEALTHY`, `UNHEALTHY`,
/// `UNKNOWN`). Every one of them is exercised by the table below.
const HEALTH_STATUS_VALUES: [&str; 3] = ["HEALTHY", "UNHEALTHY", "UNKNOWN"];

/// Create a service with a custom health check in `namespace_id` and return its id.
async fn custom_health_service(
    router: &smithy_server_runtime::MultiServiceRouter,
    namespace_id: &str,
    name: &str,
) -> String {
    let created = ok(
        router,
        "CreateService",
        json!({
            "Name": name,
            "NamespaceId": namespace_id,
            "HealthCheckCustomConfig": { "FailureThreshold": 1 },
        }),
    )
    .await;
    created["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string()
}

/// The `Status` map `GetInstancesHealthStatus` answers with, as an ordered map.
fn status_map(reported: &Value) -> BTreeMap<String, String> {
    reported["Status"]
        .as_object()
        .expect("GetInstancesHealthStatus returns a Status map")
        .iter()
        .map(|(instance_id, status)| {
            (
                instance_id.clone(),
                status
                    .as_str()
                    .expect("a HealthStatus is a string")
                    .to_string(),
            )
        })
        .collect()
}

/// Register `instance_id` in `service_id`, with the given extra attributes.
async fn register(
    router: &smithy_server_runtime::MultiServiceRouter,
    service_id: &str,
    instance_id: &str,
    attributes: Value,
) {
    let mut body = json!({ "ServiceId": service_id, "InstanceId": instance_id });
    body["Attributes"] = attributes;
    ok(router, "RegisterInstance", body).await;
}

/// The report is computed from the stored records: a custom health check's instance
/// carries the status its registration stored (`AWS_INIT_HEALTH_STATUS`, defaulting to
/// `HEALTHY`), while an instance of a service with no health check at all is `UNKNOWN`,
/// because nothing has reported on it.
#[tokio::test]
async fn health_status_is_reported_from_the_stored_instance_records() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "health-status-ns").await;
    let custom_id = custom_health_service(&router, &namespace_id, "custom-svc").await;
    let plain_id = service(&router, &namespace_id, "plain-svc").await;

    // (case, service, instance, registration attributes, expected HealthStatus)
    let rows: [(&str, &str, &str, Value, &str); 4] = [
        (
            "custom health check, registered HEALTHY",
            custom_id.as_str(),
            "i-healthy",
            json!({ "AWS_INIT_HEALTH_STATUS": "HEALTHY", "AWS_INSTANCE_IPV4": "10.0.0.1" }),
            "HEALTHY",
        ),
        (
            "custom health check, registered UNHEALTHY",
            custom_id.as_str(),
            "i-unhealthy",
            json!({ "AWS_INIT_HEALTH_STATUS": "UNHEALTHY", "AWS_INSTANCE_IPV4": "10.0.0.2" }),
            "UNHEALTHY",
        ),
        (
            "custom health check, no initial status named",
            custom_id.as_str(),
            "i-default",
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.3" }),
            "HEALTHY",
        ),
        (
            "no health check configured",
            plain_id.as_str(),
            "i-plain",
            json!({ "AWS_INSTANCE_IPV4": "10.0.0.4" }),
            "UNKNOWN",
        ),
    ];

    for (_, service_id, instance_id, attributes, _) in &rows {
        register(&router, service_id, instance_id, attributes.clone()).await;
    }

    // Every row, asked for one instance at a time.
    for (case, service_id, instance_id, _, expected) in &rows {
        let reported = ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": service_id, "Instances": [instance_id] }),
        )
        .await;
        let status = status_map(&reported);
        assert_eq!(
            status,
            BTreeMap::from([((*instance_id).to_string(), (*expected).to_string())]),
            "case {case:?}: expected only {instance_id}={expected}, got {status:?}"
        );
    }

    // Every status in the model's enum is covered by the table above.
    let covered: BTreeSet<&str> = rows.iter().map(|(_, _, _, _, status)| *status).collect();
    let mut missing: Vec<&str> = HEALTH_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !covered.contains(status))
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "the table must cover every modelled HealthStatus, missing {missing:?}"
    );

    // Omitting `Instances` reports every instance of the named service, and only those.
    let whole_custom = status_map(
        &ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": custom_id }),
        )
        .await,
    );
    assert_eq!(
        whole_custom,
        BTreeMap::from([
            ("i-default".to_string(), "HEALTHY".to_string()),
            ("i-healthy".to_string(), "HEALTHY".to_string()),
            ("i-unhealthy".to_string(), "UNHEALTHY".to_string()),
        ]),
        "case \"omitted Instances, custom service\": got {whole_custom:?}"
    );
    let whole_plain = status_map(
        &ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": plain_id }),
        )
        .await,
    );
    assert_eq!(
        whole_plain,
        BTreeMap::from([("i-plain".to_string(), "UNKNOWN".to_string())]),
        "case \"omitted Instances, plain service\": got {whole_plain:?}"
    );

    // A service with no instances reports an empty map rather than failing.
    let empty_id = service(&router, &namespace_id, "empty-svc").await;
    let empty = status_map(
        &ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": empty_id }),
        )
        .await,
    );
    assert!(
        empty.is_empty(),
        "case \"service with no instances\": expected an empty Status, got {empty:?}"
    );
}

/// `Instances` selects exactly the instances named, whatever their number, order or
/// repetition — and the service may be named by its ARN as well as by its id, because
/// the member is modelled as `Arn`.
#[tokio::test]
async fn an_instances_list_reports_exactly_the_distinct_instances_it_names() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "select-ns").await;
    let service_id = custom_health_service(&router, &namespace_id, "select-svc").await;
    let service_arn = ok(&router, "GetService", json!({ "Id": service_id }))
        .await["Service"]["Arn"]
        .as_str()
        .expect("GetService reports the ARN it minted")
        .to_string();

    for index in 0..4 {
        let initial = if index % 2 == 0 { "HEALTHY" } else { "UNHEALTHY" };
        register(
            &router,
            &service_id,
            &format!("i-{index}"),
            json!({ "AWS_INIT_HEALTH_STATUS": initial }),
        )
        .await;
    }

    // (case, the service reference, the Instances list, the ids expected back)
    let rows: [(&str, &str, Vec<&str>, Vec<&str>); 5] = [
        ("one id", service_id.as_str(), vec!["i-1"], vec!["i-1"]),
        (
            "two ids, requested out of order",
            service_id.as_str(),
            vec!["i-2", "i-0"],
            vec!["i-0", "i-2"],
        ),
        (
            "a repeated id names one instance",
            service_id.as_str(),
            vec!["i-3", "i-3"],
            vec!["i-3"],
        ),
        (
            "every id",
            service_id.as_str(),
            vec!["i-0", "i-1", "i-2", "i-3"],
            vec!["i-0", "i-1", "i-2", "i-3"],
        ),
        (
            "the service named by its ARN",
            service_arn.as_str(),
            vec!["i-0", "i-1"],
            vec!["i-0", "i-1"],
        ),
    ];

    for (case, reference, requested, expected) in &rows {
        let reported = ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": reference, "Instances": requested }),
        )
        .await;
        let status = status_map(&reported);
        let reported_ids: Vec<&str> = status.keys().map(String::as_str).collect();
        assert_eq!(
            reported_ids, *expected,
            "case {case:?}: expected {expected:?}, got {status:?}"
        );
        for (instance_id, reported_status) in &status {
            let expected_status = if instance_id
                .rsplit('-')
                .next()
                .and_then(|index| index.parse::<u32>().ok())
                .is_some_and(|index| index % 2 == 0)
            {
                "HEALTHY"
            } else {
                "UNHEALTHY"
            };
            assert_eq!(
                reported_status, expected_status,
                "case {case:?}: {instance_id} was registered {expected_status}, \
                 got {reported_status}"
            );
        }
    }
}

/// `MaxResults` caps a page and `NextToken` resumes after it: walking the pages reports
/// every instance exactly once, and only a truncated page carries a token. The declared
/// range is 1..100, so each bound is accepted and one past each bound is refused before
/// the handler runs.
#[tokio::test]
async fn max_results_cuts_pages_and_next_token_resumes_after_the_last_one() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "page-ns").await;
    let service_id = custom_health_service(&router, &namespace_id, "page-svc").await;

    const STORED: usize = 7;
    let mut everything: BTreeMap<String, String> = BTreeMap::new();
    for index in 0..STORED {
        let instance_id = format!("i-{index}");
        let initial = if index % 3 == 0 { "UNHEALTHY" } else { "HEALTHY" };
        register(
            &router,
            &service_id,
            &instance_id,
            json!({ "AWS_INIT_HEALTH_STATUS": initial }),
        )
        .await;
        everything.insert(instance_id, initial.to_string());
    }

    for page_size in [1_usize, 2, 3, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let mut collected: BTreeMap<String, String> = BTreeMap::new();
        let mut pages = 0_usize;
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({ "ServiceId": service_id, "MaxResults": page_size });
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let reported = ok(&router, "GetInstancesHealthStatus", body).await;
            let page = status_map(&reported);
            pages += 1;
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
            );
            for (instance_id, status) in page {
                assert!(
                    collected.insert(instance_id.clone(), status).is_none(),
                    "case {case:?}: {instance_id} was reported on two pages"
                );
            }
            next_token = reported["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= STORED + 1,
                "case {case:?}: the paging loop must terminate, got {pages} pages"
            );
        }
        assert_eq!(
            collected, everything,
            "case {case:?}: paging must report every stored instance exactly once"
        );
        assert_eq!(
            pages,
            STORED.div_ceil(page_size),
            "case {case:?}: expected {} pages for {STORED} instances",
            STORED.div_ceil(page_size)
        );
    }

    // No `MaxResults` reports up to the documented default of 100, i.e. everything here.
    let all = status_map(
        &ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": service_id }),
        )
        .await,
    );
    assert_eq!(
        all, everything,
        "case \"no MaxResults\": every stored instance is reported, got {all:?}"
    );

    // The published range's bounds, and one past each of them. An explicitly empty
    // `Instances` list is outside the modelled `length min 1` and is refused too, rather
    // than answered with an empty report a caller cannot tell from an empty service.
    for (case, body, expected_status) in [
        (
            "the low bound",
            json!({ "ServiceId": service_id, "MaxResults": 1 }),
            200_u16,
        ),
        (
            "the high bound",
            json!({ "ServiceId": service_id, "MaxResults": 100 }),
            200,
        ),
        (
            "one below the low bound",
            json!({ "ServiceId": service_id, "MaxResults": 0 }),
            400,
        ),
        (
            "one above the high bound",
            json!({ "ServiceId": service_id, "MaxResults": 101 }),
            400,
        ),
        (
            "an empty Instances list",
            json!({ "ServiceId": service_id, "Instances": [] }),
            400,
        ),
    ] {
        let (status, reported) = call(&router, "GetInstancesHealthStatus", body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {reported}"
        );
    }
}

/// Every modelled refusal, over a table of requests that name something this service
/// does not hold.
#[tokio::test]
async fn requests_naming_something_unstored_answer_their_modelled_errors() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "refuse-ns").await;
    let service_id = custom_health_service(&router, &namespace_id, "refuse-svc").await;
    register(&router, &service_id, "i-1", json!({ "stage": "prod" })).await;

    // A token for another scope is well formed but was not issued by this operation.
    let foreign_token = hex::encode(format!("ListInstances{}i-1", '\u{1f}'));

    for (case, body, expected_status, expected_code) in [
        (
            // This read reports whether instances have a status, so a service it
            // cannot resolve has none to report: the refusal is an instance miss, not
            // a service miss.
            "a service id nothing was created under",
            json!({ "ServiceId": "srv-nosuchservice" }),
            400,
            "InstanceNotFound",
        ),
        (
            "an instance that is not registered with the service",
            json!({ "ServiceId": service_id, "Instances": ["i-9"] }),
            400,
            "InstanceNotFound",
        ),
        (
            "one known and one unknown instance",
            json!({ "ServiceId": service_id, "Instances": ["i-1", "i-9"] }),
            400,
            "InstanceNotFound",
        ),
        (
            "a NextToken that is not hex at all",
            json!({ "ServiceId": service_id, "NextToken": "not-a-token" }),
            400,
            "InvalidInput",
        ),
        (
            "a well-formed NextToken from another paginated read",
            json!({ "ServiceId": service_id, "NextToken": foreign_token }),
            400,
            "InvalidInput",
        ),
    ] {
        let (status, code) = failure(&router, "GetInstancesHealthStatus", body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {code}"
        );
    }
}

/// Reporting health status is a read: it changes nothing a later caller sees.
#[tokio::test]
async fn reporting_health_status_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "readonly-ns").await;
    let service_id = custom_health_service(&router, &namespace_id, "readonly-svc").await;
    for index in 0..3 {
        register(
            &router,
            &service_id,
            &format!("i-{index}"),
            json!({ "AWS_INSTANCE_IPV4": format!("10.0.0.{index}") }),
        )
        .await;
    }

    let before = ok(
        &router,
        "GetInstancesHealthStatus",
        json!({ "ServiceId": service_id }),
    )
    .await;
    let revision_before = ok(
        &router,
        "DiscoverInstances",
        json!({ "NamespaceName": "readonly-ns", "ServiceName": "readonly-svc" }),
    )
    .await["InstancesRevision"]
        .clone();

    // Read it again, and through a paged walk, which also decodes a cursor.
    for _ in 0..3 {
        ok(
            &router,
            "GetInstancesHealthStatus",
            json!({ "ServiceId": service_id, "MaxResults": 1 }),
        )
        .await;
    }
    let after = ok(
        &router,
        "GetInstancesHealthStatus",
        json!({ "ServiceId": service_id }),
    )
    .await;
    let revision_after = ok(
        &router,
        "DiscoverInstances",
        json!({ "NamespaceName": "readonly-ns", "ServiceName": "readonly-svc" }),
    )
    .await["InstancesRevision"]
        .clone();

    assert_eq!(
        status_map(&before),
        status_map(&after),
        "case \"repeated reads\": the report must not change, got {after}"
    );
    assert_eq!(
        revision_before, revision_after,
        "case \"repeated reads\": InstancesRevision must not move, got {revision_after}"
    );
    let instance_count = ok(&router, "GetService", json!({ "Id": service_id })).await["Service"]
        ["InstanceCount"]
        .clone();
    assert_eq!(
        instance_count,
        json!(3),
        "case \"repeated reads\": the service still holds its 3 instances, got {instance_count}"
    );
}
