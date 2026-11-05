// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members of a stored `DnsConfig` that the SERVICE decides
//! rather than the caller.
//!
//! The rule under test: a `DnsConfig` member a service resolves for itself is reported
//! by every read of that service whether or not the request that created it carried the
//! member — its `RoutingPolicy` (the multivalue policy when the caller named none, the
//! named one otherwise) and its deprecated `NamespaceId` (the namespace that owns the
//! service, as an id, however the request named that namespace).
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router/dispatch harness is this crate's one test harness, in
// `tests/all/cloud_map_core.rs`.
use crate::cloud_map_core::{dns_namespace, ok};

/// The `DnsConfig` every read path reports for one service: the create's own answer,
/// `GetService` and the service's `ListServices` summary, asserted to agree.
async fn reported_dns_config(
    router: &MultiServiceRouter,
    case: &str,
    created: &Value,
    service_id: &str,
) -> Value {
    let read = ok(router, "GetService", json!({ "Id": service_id })).await;
    let listed = ok(router, "ListServices", json!({})).await;
    let summary = listed["Services"]
        .as_array()
        .expect("ListServices answers a list")
        .iter()
        .find(|summary| summary["Id"] == json!(service_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {service_id} must be listed, got {listed}"));
    assert_eq!(
        read["Service"]["DnsConfig"], created["Service"]["DnsConfig"],
        "case {case:?}: GetService must report the DnsConfig the create answered with, \
         got {read}"
    );
    assert_eq!(
        summary["DnsConfig"], created["Service"]["DnsConfig"],
        "case {case:?}: the ListServices summary must report the same DnsConfig, got {summary}"
    );
    read["Service"]["DnsConfig"].clone()
}

/// A `DnsConfig` carries its `RoutingPolicy` and its owning namespace on every read,
/// whether or not the create named them.
///
/// The rows differ in record type, record count, TTL, whether a routing policy was
/// named, how the namespace was named and whether a health check was configured. The
/// record types are the model's `RecordType` enum and the policies its `RoutingPolicy`
/// enum (`RECORD_TYPE_VALUES` / `ROUTING_POLICY_VALUES` in
/// `src/ops/types/shared_types.rs`), so each row names a published value — including the
/// `AAAA` and `CNAME` types and the `WEIGHTED` policy no reported case used.
#[tokio::test]
async fn a_stored_dns_config_reports_its_routing_policy_and_namespace_on_every_read() {
    let (router, (namespace_id, namespace_arn)) = emulator_with_dns_namespace().await;

    // (case, how the namespace is named, the DnsConfig the create carries, extra
    // members, the RoutingPolicy every read must report)
    let cases: Vec<(&str, &str, Value, Value, &str)> = vec![
        (
            "one SRV record and no routing policy",
            "id",
            json!({ "DnsRecords": [{ "Type": "SRV", "TTL": 60 }] }),
            json!({}),
            "MULTIVALUE",
        ),
        (
            "one SRV record beside a custom health check",
            "id",
            json!({ "DnsRecords": [{ "Type": "SRV", "TTL": 60 }] }),
            json!({ "HealthCheckCustomConfig": { "FailureThreshold": 1 } }),
            "MULTIVALUE",
        ),
        (
            "one A record under an explicitly multivalue policy",
            "id",
            json!({ "RoutingPolicy": "MULTIVALUE",
                    "DnsRecords": [{ "Type": "A", "TTL": 300 }] }),
            json!({}),
            "MULTIVALUE",
        ),
        (
            "one CNAME under a weighted policy",
            "id",
            json!({ "RoutingPolicy": "WEIGHTED",
                    "DnsRecords": [{ "Type": "CNAME", "TTL": 10 }] }),
            json!({}),
            "WEIGHTED",
        ),
        (
            "two records at once, one with a zero TTL",
            "id",
            json!({ "DnsRecords": [{ "Type": "A", "TTL": 0 },
                                   { "Type": "AAAA", "TTL": 60 }] }),
            json!({}),
            "MULTIVALUE",
        ),
        (
            "a namespace named by its ARN",
            "arn",
            json!({ "DnsRecords": [{ "Type": "A", "TTL": 15 }] }),
            json!({}),
            "MULTIVALUE",
        ),
        (
            "a namespace named through the deprecated DnsConfig.NamespaceId",
            "dns-config",
            json!({ "DnsRecords": [{ "Type": "AAAA", "TTL": 45 }] }),
            json!({}),
            "MULTIVALUE",
        ),
    ];

    for (index, (case, named_by, dns_config, extra, expected_policy)) in
        cases.into_iter().enumerate()
    {
        let mut config = dns_config.clone();
        let mut body = json!({ "Name": format!("dns-defaults-svc-{index}") });
        match named_by {
            "arn" => body["NamespaceId"] = json!(namespace_arn),
            "dns-config" => config["NamespaceId"] = json!(namespace_id),
            _ => body["NamespaceId"] = json!(namespace_id),
        }
        body["DnsConfig"] = config;
        if let Value::Object(members) = extra {
            for (key, value) in members {
                body[key.as_str()] = value;
            }
        }

        let created = ok(&router, "CreateService", body.clone()).await;
        let service_id = created["Service"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateService returns an Id, got {created}"))
            .to_string();
        let reported = reported_dns_config(&router, case, &created, &service_id).await;

        assert_eq!(
            reported["RoutingPolicy"],
            json!(expected_policy),
            "case {case:?} from {body}: expected RoutingPolicy {expected_policy}, got {reported}"
        );
        assert_eq!(
            reported["NamespaceId"],
            json!(namespace_id),
            "case {case:?} from {body}: the DnsConfig names the owning namespace by id, \
             got {reported}"
        );
        assert_eq!(
            reported["DnsRecords"], dns_config["DnsRecords"],
            "case {case:?} from {body}: the records the create stated round-trip, got {reported}"
        );
    }
}

/// A change to the records through `UpdateService` leaves the service-decided members
/// where they were: the policy the create resolved (defaulted or named) and the owning
/// namespace are still reported after the update.
#[tokio::test]
async fn an_update_of_the_records_keeps_the_service_decided_members() {
    let (router, (namespace_id, _)) = emulator_with_dns_namespace().await;

    // (case, the policy the create named, the policy every read must report)
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("a defaulted policy", None, "MULTIVALUE"),
        ("an explicitly weighted policy", Some("WEIGHTED"), "WEIGHTED"),
        ("an explicitly multivalue policy", Some("MULTIVALUE"), "MULTIVALUE"),
    ];
    for (index, (case, named_policy, expected_policy)) in cases.into_iter().enumerate() {
        let service_name = format!("dns-update-svc-{index}");
        let mut dns_config = json!({ "DnsRecords": [{ "Type": "A", "TTL": 60 }] });
        if let Some(policy) = named_policy {
            dns_config["RoutingPolicy"] = json!(policy);
        }
        let created = ok(
            &router,
            "CreateService",
            json!({ "Name": service_name, "NamespaceId": namespace_id,
                    "DnsConfig": dns_config }),
        )
        .await;
        let service_id = created["Service"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateService returns an Id, got {created}"))
            .to_string();

        // The one member `DnsConfigChange` carries is the record list, so this states a
        // new TTL for the same record type.
        ok(
            &router,
            "UpdateService",
            json!({ "Id": service_id,
                    "Service": { "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 120 }] } } }),
        )
        .await;

        let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
        let reported = read["Service"]["DnsConfig"].clone();
        assert_eq!(
            reported["RoutingPolicy"],
            json!(expected_policy),
            "case {case:?}: expected RoutingPolicy {expected_policy} after the update, \
             got {reported}"
        );
        assert_eq!(
            reported["NamespaceId"],
            json!(namespace_id),
            "case {case:?}: the DnsConfig still names the owning namespace, got {reported}"
        );
        assert_eq!(
            reported["DnsRecords"],
            json!([{ "Type": "A", "TTL": 120 }]),
            "case {case:?}: the update's records are what a read reports, got {reported}"
        );
    }
}

/// One emulator plus `(namespace id, namespace ARN)` of a DNS namespace, which is the
/// kind that holds the hosted zone a `DnsConfig` registers records in.
async fn emulator_with_dns_namespace() -> (MultiServiceRouter, (String, String)) {
    let router = crate::cloud_map_core::emulator();
    let namespace_id = dns_namespace(&router, "dns-defaults-ns").await;
    let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
    let namespace_arn = read["Namespace"]["Arn"]
        .as_str()
        .expect("a namespace carries the ARN it was minted with")
        .to_string();
    (router, (namespace_id, namespace_arn))
}
