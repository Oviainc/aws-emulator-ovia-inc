// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members every read of a Cloud Map resource reports because
//! they are facts OF the resource rather than of the request: the account that created
//! or owns it, and the record configuration of a service.
//!
//! The rules under test:
//!
//! * every modelled `AWSAccountId` member of a response — `Namespace$ResourceOwner`,
//!   `Service$ResourceOwner` / `$CreatedByAccount`, `ServiceAttributes$ResourceOwner`,
//!   `Instance$CreatedByAccount`, `Operation$OwnerAccount` and their listing
//!   counterparts — reports the account that holds the resource, which for this emulator
//!   is the one account its ARNs carry, on EVERY read path and for every entry of a
//!   page, however many entries it holds;
//! * a service always reports a `DnsConfig`, carrying the records it registers and an
//!   empty set of them when it registers none.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`), reusing
//! the one harness in `tests/all/cloud_map_core.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{dns_namespace, emulator, namespace, ok};

/// The `ListServices` summary of one service, out of a page that holds every stored
/// service — so the assertions below are made against a MULTI-entry page, not only
/// against a page that happens to hold one record.
async fn summary(router: &MultiServiceRouter, case: &str, service_id: &str) -> Value {
    page_summary(router, case, service_id, json!({})).await
}

/// The `ListServices` summary of one service, out of the page the given request body
/// asks for.
async fn page_summary(
    router: &MultiServiceRouter,
    case: &str,
    service_id: &str,
    body: Value,
) -> Value {
    let mut request = body;
    let mut seen = 0usize;
    loop {
        let listed = ok(router, "ListServices", request.clone()).await;
        let services = listed["Services"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListServices answers a list, got {listed}"))
            .clone();
        if let Some(found) = services
            .iter()
            .find(|summary| summary["Id"] == json!(service_id))
        {
            return found.clone();
        }
        let token = listed["NextToken"].as_str().map(str::to_string);
        seen += 1;
        assert!(
            seen <= 64,
            "case {case:?}: paging must terminate while looking for {service_id}"
        );
        match token {
            Some(token) => request["NextToken"] = json!(token),
            None => panic!("case {case:?}: {service_id} must be listed, got {listed}"),
        }
    }
}

/// Create a service, optionally with a `DnsConfig`, and return its id.
async fn service_with(
    router: &MultiServiceRouter,
    namespace_id: &str,
    name: &str,
    dns_config: &Value,
) -> (String, Value) {
    let mut body = json!({ "Name": name, "NamespaceId": namespace_id });
    if !dns_config.is_null() {
        body["DnsConfig"] = dns_config.clone();
    }
    let created = ok(router, "CreateService", body).await;
    let service_id = created["Service"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateService returns the service it stored, got {created}"))
        .to_string();
    (service_id, created)
}

/// Every account member of every read reports the account that holds the resource — on
/// the create's own answer, on each per-resource read and on every entry of a listing,
/// whatever the kind of namespace and however many records a page holds.
#[tokio::test]
async fn every_account_member_of_every_read_reports_the_owning_account() {
    let router = emulator();
    // `engine::EMULATOR_ACCOUNT_ID` is the account every emulated resource belongs to;
    // the test names the constant rather than a literal, as the ARNs do.
    let account = json!(engine::EMULATOR_ACCOUNT_ID);
    assert_eq!(
        engine::EMULATOR_ACCOUNT_ID.len(),
        12,
        "AWSAccountId is modelled @length(min 12, max 12), got {}",
        engine::EMULATOR_ACCOUNT_ID
    );

    let (create_namespace_operation, http_namespace) = namespace(&router, "owner-http-ns").await;
    let dns_namespace_id = dns_namespace(&router, "owner-dns-ns").await;

    // Two services in ONE namespace, so the page below holds more than one entry, plus a
    // third in the other namespace: the rule is about every entry, not about a page of
    // one.
    let (first, first_created) =
        service_with(&router, &http_namespace, "owner-svc-a", &Value::Null).await;
    let (second, second_created) =
        service_with(&router, &http_namespace, "owner-svc-b", &Value::Null).await;
    let (third, third_created) = service_with(
        &router,
        &dns_namespace_id,
        "owner-svc-c",
        &json!({ "DnsRecords": [{ "Type": "A", "TTL": 60 }] }),
    )
    .await;

    // One instance, so the instance reads have a record to report.
    let register = ok(
        &router,
        "RegisterInstance",
        json!({
            "ServiceId": first,
            "InstanceId": "i-owner",
            "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.4" },
        }),
    )
    .await;
    let register_operation = register["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("RegisterInstance files an operation, got {register}"))
        .to_string();
    ok(
        &router,
        "UpdateServiceAttributes",
        json!({ "ServiceId": first, "Attributes": { "stage": "test" } }),
    )
    .await;

    // The reads, one per row, each paired with the member paths that name an account.
    let service_reads: Vec<(&str, Value)> = vec![
        ("the CreateService answer, HTTP namespace", first_created),
        ("the CreateService answer, a second service", second_created),
        ("the CreateService answer, DNS namespace", third_created),
        (
            "GetService in an HTTP namespace",
            ok(&router, "GetService", json!({ "Id": first })).await,
        ),
        (
            "GetService in a DNS namespace",
            ok(&router, "GetService", json!({ "Id": third })).await,
        ),
    ];
    let mut cases: Vec<(String, Value)> = Vec::new();
    for (case, read) in service_reads {
        for member in ["ResourceOwner", "CreatedByAccount"] {
            cases.push((
                format!("{case}: Service.{member}"),
                read["Service"][member].clone(),
            ));
        }
    }

    // Every entry of the whole page, and of a page served one service at a time.
    for (case, service_id) in [
        ("first", first.as_str()),
        ("second", second.as_str()),
        ("third", third.as_str()),
    ] {
        for (page, body) in [
            ("a page holding every service", json!({})),
            ("a page of one service", json!({ "MaxResults": 1 })),
        ] {
            let found = page_summary(&router, case, service_id, body).await;
            for member in ["ResourceOwner", "CreatedByAccount"] {
                cases.push((
                    format!("the {case} service's summary in {page}: ServiceSummary.{member}"),
                    found[member].clone(),
                ));
            }
        }
    }

    // The namespace reads, for each kind of namespace, through both projections.
    for (case, namespace_id) in [
        ("an HTTP namespace", http_namespace.as_str()),
        ("a public DNS namespace", dns_namespace_id.as_str()),
    ] {
        let read = ok(&router, "GetNamespace", json!({ "Id": namespace_id })).await;
        cases.push((
            format!("GetNamespace of {case}: Namespace.ResourceOwner"),
            read["Namespace"]["ResourceOwner"].clone(),
        ));
        let listed = ok(&router, "ListNamespaces", json!({})).await;
        let found = listed["Namespaces"]
            .as_array()
            .unwrap_or_else(|| panic!("ListNamespaces answers a list, got {listed}"))
            .iter()
            .find(|summary| summary["Id"] == json!(namespace_id))
            .cloned()
            .unwrap_or_else(|| panic!("{namespace_id} must be listed, got {listed}"));
        cases.push((
            format!("the ListNamespaces summary of {case}: NamespaceSummary.ResourceOwner"),
            found["ResourceOwner"].clone(),
        ));
    }

    // The operation reads, for an operation of each target kind this run filed.
    for (case, operation_id) in [
        ("a namespace create", create_namespace_operation.as_str()),
        ("an instance register", register_operation.as_str()),
    ] {
        let read = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        cases.push((
            format!("GetOperation of {case}: Operation.OwnerAccount"),
            read["Operation"]["OwnerAccount"].clone(),
        ));
    }

    // The service-attribute and instance reads.
    let attributes = ok(
        &router,
        "GetServiceAttributes",
        json!({ "ServiceId": first }),
    )
    .await;
    cases.push((
        "GetServiceAttributes: ServiceAttributes.ResourceOwner".to_string(),
        attributes["ServiceAttributes"]["ResourceOwner"].clone(),
    ));
    let instance = ok(
        &router,
        "GetInstance",
        json!({ "ServiceId": first, "InstanceId": "i-owner" }),
    )
    .await;
    cases.push((
        "GetInstance: ResourceOwner".to_string(),
        instance["ResourceOwner"].clone(),
    ));
    cases.push((
        "GetInstance: Instance.CreatedByAccount".to_string(),
        instance["Instance"]["CreatedByAccount"].clone(),
    ));
    let instances = ok(&router, "ListInstances", json!({ "ServiceId": first })).await;
    cases.push((
        "ListInstances: ResourceOwner".to_string(),
        instances["ResourceOwner"].clone(),
    ));
    let summaries = instances["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInstances answers a list, got {instances}"))
        .clone();
    assert_eq!(
        summaries.len(),
        1,
        "the one registered instance is listed, got {instances}"
    );
    for found in &summaries {
        cases.push((
            "the ListInstances summary: InstanceSummary.CreatedByAccount".to_string(),
            found["CreatedByAccount"].clone(),
        ));
    }

    for (case, got) in cases {
        assert_eq!(
            got, account,
            "case {case:?}: expected the owning account {account}, got {got:?}"
        );
    }
}

/// A service reports a `DnsConfig` on every read path whatever records it registers —
/// including none at all, where the member is present and names no record.
///
/// The rows differ along the kind of namespace, the number of records, their type and
/// TTL, the routing policy, and whether an update deleted the records. The record types
/// are the model's `RecordType` enum and the policies its `RoutingPolicy` enum
/// (`RECORD_TYPE_VALUES` / `ROUTING_POLICY_VALUES` in `src/ops/types/shared_types.rs`).
#[tokio::test]
async fn every_service_reports_a_dns_config_whichever_records_it_registers() {
    let router = emulator();
    let (_, http_namespace) = namespace(&router, "records-http-ns").await;
    let dns_namespace_id = dns_namespace(&router, "records-dns-ns").await;

    // (case, namespace, the DnsConfig the create carries, whether an update deletes the
    // records, the records every later read must report)
    let cases: Vec<(&str, &str, Value, bool, Value)> = vec![
        (
            "a service in an HTTP namespace, which registers no record at all",
            http_namespace.as_str(),
            Value::Null,
            false,
            json!([]),
        ),
        (
            "a service in a DNS namespace created with no DnsConfig",
            dns_namespace_id.as_str(),
            Value::Null,
            false,
            json!([]),
        ),
        (
            "one A record",
            dns_namespace_id.as_str(),
            json!({ "DnsRecords": [{ "Type": "A", "TTL": 60 }] }),
            false,
            json!([{ "Type": "A", "TTL": 60 }]),
        ),
        (
            "two records at once, one with a zero TTL",
            dns_namespace_id.as_str(),
            json!({ "DnsRecords": [{ "Type": "A", "TTL": 0 },
                                   { "Type": "AAAA", "TTL": 300 }] }),
            false,
            json!([{ "Type": "A", "TTL": 0 }, { "Type": "AAAA", "TTL": 300 }]),
        ),
        (
            "one SRV record under a weighted routing policy",
            dns_namespace_id.as_str(),
            json!({ "RoutingPolicy": "WEIGHTED",
                    "DnsRecords": [{ "Type": "SRV", "TTL": 10 }] }),
            false,
            json!([{ "Type": "SRV", "TTL": 10 }]),
        ),
        (
            "a CNAME record an update deleted again",
            dns_namespace_id.as_str(),
            json!({ "DnsRecords": [{ "Type": "CNAME", "TTL": 120 }] }),
            true,
            json!([]),
        ),
    ];

    let mut expectations: Vec<(&str, String, Value)> = Vec::new();
    for (index, (case, namespace_id, dns_config, deleted, expected)) in
        cases.into_iter().enumerate()
    {
        let (service_id, created) = service_with(
            &router,
            namespace_id,
            &format!("records-svc-{index}"),
            &dns_config,
        )
        .await;
        assert!(
            created["Service"]["DnsConfig"].is_object(),
            "case {case:?}: the create answers with a DnsConfig structure, got {created}"
        );
        if deleted {
            // A `ServiceChange` that states no `DnsConfig` deletes the record
            // configurations the service had.
            ok(
                &router,
                "UpdateService",
                json!({ "Id": service_id, "Service": {} }),
            )
            .await;
        }
        expectations.push((case, service_id, expected));
    }

    for (case, service_id, expected) in expectations {
        let read = ok(&router, "GetService", json!({ "Id": service_id })).await;
        let got = read["Service"]["DnsConfig"].clone();
        assert!(
            got.is_object(),
            "case {case:?}: GetService reports a DnsConfig structure, got {read}"
        );
        assert_eq!(
            got["DnsRecords"], expected,
            "case {case:?}: GetService must report {expected}, got {got:?}"
        );

        // The same service, read out of a page that holds every service this test
        // created, so the summary projection is exercised on a multi-entry page.
        let found = summary(&router, case, &service_id).await;
        assert_eq!(
            found["DnsConfig"], got,
            "case {case:?}: the summary must report the DnsConfig GetService reports, \
             got {found:?}"
        );
    }
}
