// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ownership members of the INSTANCE read paths:
//! `GetInstanceResponse$ResourceOwner` / `Instance$CreatedByAccount` and
//! `ListInstancesResponse$ResourceOwner` / `InstanceSummary$CreatedByAccount`.
//!
//! The rule under test: an instance read reports the account that registered the
//! instance on every item it returns (`CreatedByAccount`) and the account that created
//! the namespace containing the service at the response level (`ResourceOwner`), both
//! modelled `AWSAccountId` (`@length(min: 12, max: 12)`) and both unconditional — the
//! response-level member is a property of the containing namespace, so it is there even
//! when the page holds no instance at all. The recorded cases (one read, a one-entry
//! page, an empty page, a multi-entry page, a second service's page) are rows of the
//! tables below.
//!
//! The account the emulator presents is read from `engine::emulator_account_id()`, the
//! single source the ARNs it mints also carry — never a literal.
//!
//! Requests go through `MultiServiceRouter::dispatch`, reusing the harness in
//! `cloud_map_core.rs`.

use serde_json::{json, Value};

use crate::cloud_map_core::{dns_namespace, emulator, namespace, ok, service};
use smithy_server_runtime::MultiServiceRouter;

/// `AWSAccountId` is modelled `@length(min: 12, max: 12)`
/// (`smithy_query(query="shape", shape="AWSAccountId")`).
const ACCOUNT_ID_LENGTH: usize = 12;

/// The account every resource this emulator holds belongs to, as the emulator itself
/// defines it.
fn account() -> Value {
    json!(engine::emulator_account_id())
}

/// One `ListInstances` page: its `ResourceOwner`, the `(Id, CreatedByAccount)` pairs it
/// reported in order, and the `NextToken` it handed back. A member the response omits
/// reads back as `null`, so a missing one fails an assertion rather than panicking.
async fn page(
    router: &MultiServiceRouter,
    body: Value,
) -> (Value, Vec<(String, Value)>, Option<String>) {
    let listed = ok(router, "ListInstances", body).await;
    let instances: Vec<(String, Value)> = listed["Instances"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            let id = summary["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary names its instance, got {summary}"))
                .to_string();
            (id, summary["CreatedByAccount"].clone())
        })
        .collect();
    let next_token = listed["NextToken"].as_str().map(str::to_string);
    (listed["ResourceOwner"].clone(), instances, next_token)
}

/// Register `ids` under `service_id`, each with an attribute that names it.
async fn register(router: &MultiServiceRouter, service_id: &str, ids: &[&str]) {
    for (index, id) in ids.iter().enumerate() {
        ok(
            router,
            "RegisterInstance",
            json!({
                "ServiceId": service_id,
                "InstanceId": id,
                "Attributes": { "AWS_INSTANCE_IPV4": format!("192.0.2.{}", index + 1) },
            }),
        )
        .await;
    }
}

/// Assert that `value` is the modelled account id the emulator presents.
fn assert_account(value: &Value, case: &str, member: &str) {
    let actual = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be reported as a string, got {value}")
    });
    assert_eq!(
        value,
        &account(),
        "case {case:?}: {member} must be the account the emulator presents, got {actual:?}"
    );
    assert_eq!(
        actual.chars().count(),
        ACCOUNT_ID_LENGTH,
        "case {case:?}: {member} is a {ACCOUNT_ID_LENGTH}-character AWSAccountId, got {actual:?}"
    );
}

/// Every shape of instance set — none, one, several, over several pages — reports the
/// ownership members on the response and on each item, through both read paths.
#[tokio::test]
async fn every_instance_set_shape_reports_the_ownership_members() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "owner-shapes-ns").await;

    // (case, service name, the instances registered before reading)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("a service with nothing registered", "owner-none-svc", vec![]),
        ("a single-entry page", "owner-one-svc", vec!["i-1"]),
        ("a two-entry page", "owner-two-svc", vec!["i-a", "i-b"]),
        (
            "a page of five, more than one default page holds at MaxResults 2",
            "owner-five-svc",
            vec!["i-01", "i-02", "i-03", "i-04", "i-05"],
        ),
    ];

    for (case, service_name, instances) in &cases {
        let service_id = service(&router, &namespace_id, service_name).await;
        register(&router, &service_id, instances).await;

        // The whole set in one page, and the same set walked at every page size the
        // set can be split at: the response-level member comes back on every page.
        let sizes: Vec<Option<i64>> = vec![None, Some(1), Some(2), Some(100)];
        for max_results in sizes {
            let mut body = json!({ "ServiceId": service_id });
            if let Some(max_results) = max_results {
                body["MaxResults"] = json!(max_results);
            }
            let case = format!("{case} at MaxResults {max_results:?}");

            let mut reported: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut body = body.clone();
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let (owner, page, next) = page(&router, body).await;
                pages += 1;
                assert_account(&owner, &case, "ListInstances ResourceOwner");
                for (id, created_by) in &page {
                    assert_account(
                        created_by,
                        &format!("{case}, summary {id}"),
                        "InstanceSummary CreatedByAccount",
                    );
                    reported.push(id.clone());
                }
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
                assert!(
                    pages <= instances.len() + 1,
                    "case {case:?}: pagination must terminate, it has reported {reported:?}"
                );
            }
            assert_eq!(
                reported, *instances,
                "case {case:?}: the walk must report exactly what was registered"
            );
        }

        // The single read of each instance reports both members too.
        for instance_id in instances {
            let read = ok(
                &router,
                "GetInstance",
                json!({ "ServiceId": service_id, "InstanceId": instance_id }),
            )
            .await;
            let case = format!("{case}, GetInstance {instance_id}");
            assert_account(&read["ResourceOwner"], &case, "GetInstance ResourceOwner");
            assert_account(
                &read["Instance"]["CreatedByAccount"],
                &case,
                "Instance CreatedByAccount",
            );
        }
    }
}

/// The response-level member is a property of the containing namespace, not of the
/// items: it is reported on a page that holds no instance, however the page came to be
/// empty.
#[tokio::test]
async fn an_empty_page_still_reports_the_namespace_owner() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "owner-empty-ns").await;

    // (case, the instances to register, the instances to deregister again)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("a service that never held an instance", vec![], vec![]),
        (
            "every instance deregistered again",
            vec!["i-1"],
            vec!["i-1"],
        ),
        (
            "several registered, then all deregistered",
            vec!["i-1", "i-2", "i-3"],
            vec!["i-1", "i-2", "i-3"],
        ),
    ];

    for (index, (case, registered, deregistered)) in cases.iter().enumerate() {
        let service_id = service(&router, &namespace_id, &format!("owner-empty-svc-{index}")).await;
        register(&router, &service_id, registered).await;
        for instance_id in deregistered {
            ok(
                &router,
                "DeregisterInstance",
                json!({ "ServiceId": service_id, "InstanceId": instance_id }),
            )
            .await;
        }

        let (owner, page, token) = page(&router, json!({ "ServiceId": service_id })).await;
        assert!(
            page.is_empty(),
            "case {case:?}: the page must hold no instance, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: an empty page hands back no token, got {token:?}"
        );
        assert_account(&owner, case, "ListInstances ResourceOwner");
    }
}

/// Each service reports the instances registered under it with their own registering
/// account, in any namespace type and whichever way the service is named.
#[tokio::test]
async fn each_service_reports_its_own_instances_ownership() {
    let router = emulator();
    let (_, http_namespace) = namespace(&router, "owner-multi-ns").await;
    let dns_namespace_id = dns_namespace(&router, "owner-multi-dns").await;

    // (case, the namespace the service lives in, service name, its instances)
    let cases: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        (
            "the first service of an HTTP namespace",
            http_namespace.as_str(),
            "owner-multi-a",
            vec!["i-a1", "i-a2"],
        ),
        (
            "a second service of the same namespace",
            http_namespace.as_str(),
            "owner-multi-b",
            vec!["i-b1"],
        ),
        (
            "a service of a public DNS namespace",
            dns_namespace_id.as_str(),
            "owner-multi-c",
            vec!["i-c1", "i-c2", "i-c3"],
        ),
    ];

    for (case, namespace_id, service_name, instances) in &cases {
        let created = ok(
            &router,
            "CreateService",
            json!({ "Name": service_name, "NamespaceId": namespace_id }),
        )
        .await;
        let service_id = created["Service"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateService returns the service it stored"))
            .to_string();
        let service_arn = created["Service"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateService returns the service's ARN"))
            .to_string();
        register(&router, &service_id, instances).await;

        // A service is addressable by its id and by its ARN; both name the one stored
        // service, so both listings report the same ownership.
        for reference in [service_id.as_str(), service_arn.as_str()] {
            let case = format!("{case} named by {reference}");
            let (owner, page, _) = page(&router, json!({ "ServiceId": reference })).await;
            let ids: Vec<&str> = page.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(
                ids, *instances,
                "case {case:?}: the listing reports this service's own instances"
            );
            assert_account(&owner, &case, "ListInstances ResourceOwner");
            for (id, created_by) in &page {
                assert_account(
                    created_by,
                    &format!("{case}, summary {id}"),
                    "InstanceSummary CreatedByAccount",
                );
            }
        }

        // The owner reported is the account the service's own ARN carries, so the two
        // can never disagree about whose resources these are.
        let owner = account();
        let owner = owner.as_str().expect("the account is a string");
        assert!(
            service_arn.contains(&format!(":{owner}:")),
            "case {case:?}: the reported owner {owner} must be the account segment of \
             {service_arn}"
        );
    }
}

/// The two read paths agree about one instance's registering account, the account
/// survives a re-registration that replaces the instance, and reading reports it
/// without changing what a later caller sees.
#[tokio::test]
async fn the_two_read_paths_agree_and_reading_changes_nothing() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "owner-agree-ns").await;
    let service_id = service(&router, &namespace_id, "owner-agree-svc").await;
    register(&router, &service_id, &["i-1", "i-2"]).await;

    // (case, what to do before reading)
    let steps: Vec<(&str, Option<Value>)> = vec![
        ("as registered", None),
        (
            "after the instance is re-registered with new attributes",
            Some(json!({ "ServiceId": service_id, "InstanceId": "i-1",
                         "Attributes": { "AWS_INSTANCE_IPV4": "198.51.100.7" } })),
        ),
        (
            "after a second registration carrying a CreatorRequestId",
            Some(json!({ "ServiceId": service_id, "InstanceId": "i-1",
                         "CreatorRequestId": "owner-agree-token",
                         "Attributes": { "AWS_INSTANCE_IPV4": "198.51.100.8" } })),
        ),
    ];

    for (case, action) in steps {
        if let Some(body) = action {
            ok(&router, "RegisterInstance", body).await;
        }

        let read = ok(
            &router,
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
        )
        .await;
        assert_account(&read["ResourceOwner"], case, "GetInstance ResourceOwner");
        assert_account(
            &read["Instance"]["CreatedByAccount"],
            case,
            "Instance CreatedByAccount",
        );

        let (owner, page, _) = page(&router, json!({ "ServiceId": service_id })).await;
        assert_eq!(
            owner, read["ResourceOwner"],
            "case {case:?}: both read paths report one owner, got {owner} and {}",
            read["ResourceOwner"]
        );
        let summary = page
            .iter()
            .find(|(id, _)| id == "i-1")
            .unwrap_or_else(|| panic!("case {case:?}: i-1 must be listed, got {page:?}"));
        assert_eq!(
            summary.1, read["Instance"]["CreatedByAccount"],
            "case {case:?}: the summary and the single read report one registering \
             account, got {} and {}",
            summary.1, read["Instance"]["CreatedByAccount"]
        );
    }

    // Reporting them writes nothing: a repeated read answers identically.
    let first = ok(&router, "ListInstances", json!({ "ServiceId": service_id })).await;
    let read_first = ok(
        &router,
        "GetInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-2" }),
    )
    .await;
    for round in 1..=3 {
        let again = ok(&router, "ListInstances", json!({ "ServiceId": service_id })).await;
        assert_eq!(
            again, first,
            "case \"round {round}\": repeating the listing must answer identically, got {again}"
        );
        let read_again = ok(
            &router,
            "GetInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-2" }),
        )
        .await;
        assert_eq!(
            read_again, read_first,
            "case \"round {round}\": repeating the read must answer identically, got \
             {read_again}"
        );
    }
}
