// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeElasticsearchDomains`, the BATCH describe,
//! driven as real restJson1 requests through the router.
//!
//! The rule under test, asserted over a table of requests rather than one
//! recorded call: the answer carries one `ElasticsearchDomainStatus` for each
//! requested name the store holds in the Region the request was signed for, in
//! the order the request lists those names, and nothing else — a name no stored
//! domain carries contributes no entry and no error, and a name the request
//! repeats names one domain, so it is reported once.
//!
//! It is also a pure read: each reported entry is the SAME record the
//! single-domain describe answers with, and repeating the batch call changes
//! nothing a later caller sees.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for; every ARN and endpoint must carry it.
const REGION: &str = "us-west-2";

/// A second region, so the batch describe's Region scoping is observable.
const OTHER_REGION: &str = "eu-west-1";

/// Longer than the emulator's provisioning span, so every created domain has
/// settled and two reads of it agree member for member.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    call_in(router, region, "POST", "/2015-01-01/es/domain", Some(body)).await
}

async fn describe_in(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call_in(
        router,
        region,
        "GET",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

/// The batch describe, signed for `region`.
async fn describe_batch_in(
    router: &MultiServiceRouter,
    region: &str,
    names: &[&str],
) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/2015-01-01/es/domain-info",
        Some(&json!({"DomainNames": names})),
    )
    .await
}

/// The `DomainName` of every entry of a batch answer, in the order answered.
fn reported_names(case: &str, answer: &Answer) -> Vec<String> {
    answer
        .at(case, "/DomainStatusList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: DomainStatusList is not a list, got {body}",
                body = answer.body
            )
        })
        .iter()
        .map(|entry| {
            entry
                .pointer("/DomainName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: an entry carries no DomainName, got {body}",
                        body = answer.body
                    )
                })
                .to_string()
        })
        .collect()
}

/// The domains the table's rows select from. Each one carries its own cluster
/// size and version form, so an entry reported for the wrong name is visible in
/// more than its `DomainName`.
const STORED: [(&str, &str, i64); 4] = [
    ("alpha-domain", "7.10", 1),
    ("beta-domain", "6.8", 2),
    ("gamma-domain", "OpenSearch_2.5", 3),
    ("delta-domain", "5.6", 4),
];

/// A router holding [`STORED`] in [`REGION`], plus one domain in
/// [`OTHER_REGION`] that SHARES a name with the first — so a batch answer that
/// ignored the request's Region would report the wrong record.
async fn populated() -> MultiServiceRouter {
    let router = router();
    for (name, version, instance_count) in STORED {
        let created = create_in(
            &router,
            REGION,
            &json!({
                "DomainName": name,
                "ElasticsearchVersion": version,
                "ElasticsearchClusterConfig": {
                    "InstanceType": "t2.small.elasticsearch",
                    "InstanceCount": instance_count
                }
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "setup {name:?}: create must succeed, got {} {}",
            created.status, created.body
        );
    }
    let elsewhere = create_in(
        &router,
        OTHER_REGION,
        &json!({
            "DomainName": STORED[0].0,
            "ElasticsearchVersion": "7.10",
            "ElasticsearchClusterConfig": {
                "InstanceType": "t2.small.elasticsearch",
                "InstanceCount": 9
            }
        }),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "setup: the other region's domain must be creatable, got {} {}",
        elsewhere.status, elsewhere.body
    );
    tokio::time::sleep(PAST_PROVISIONING).await;
    router
}

/// RULE: the answer is the requested names the store holds, in request order.
/// Rows differ along what the rule generalises over: how many names are asked
/// for, which of them exist, the order they are listed in, and whether one is
/// repeated.
#[tokio::test]
async fn a_batch_describe_reports_exactly_the_requested_domains_it_holds() {
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one stored name", vec!["beta-domain"], vec!["beta-domain"]),
        (
            "every stored name",
            vec![
                "alpha-domain",
                "beta-domain",
                "gamma-domain",
                "delta-domain",
            ],
            vec![
                "alpha-domain",
                "beta-domain",
                "gamma-domain",
                "delta-domain",
            ],
        ),
        (
            "a subset, listed in an order the store does not hold them in",
            vec!["delta-domain", "alpha-domain"],
            vec!["delta-domain", "alpha-domain"],
        ),
        (
            "stored and unknown names interleaved",
            vec![
                "absent-domain",
                "gamma-domain",
                "missing-domain",
                "beta-domain",
            ],
            vec!["gamma-domain", "beta-domain"],
        ),
        (
            "only unknown names",
            vec!["absent-domain", "missing-domain"],
            vec![],
        ),
        ("no names at all", vec![], vec![]),
        (
            "one name repeated",
            vec!["alpha-domain", "alpha-domain", "alpha-domain"],
            vec!["alpha-domain"],
        ),
        (
            "a repeat interleaved with another name",
            vec![
                "gamma-domain",
                "alpha-domain",
                "gamma-domain",
                "absent-domain",
            ],
            vec!["gamma-domain", "alpha-domain"],
        ),
    ];

    let router = populated().await;
    for (label, requested, expected) in cases {
        let answer = describe_batch_in(&router, REGION, &requested).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the batch describe must answer 200, got {} {}",
            answer.status, answer.body
        );
        let got = reported_names(label, &answer);
        assert_eq!(
            got, expected,
            "case {label:?}: expected {expected:?}, got {got:?}"
        );

        // Each reported entry is the record the single-domain describe answers
        // with — the batch reports stored state, not something built for it.
        for name in &expected {
            let single = describe_in(&router, REGION, name).await;
            let entry = answer
                .at(label, "/DomainStatusList")
                .as_array()
                .expect("DomainStatusList is a list")
                .iter()
                .find(|entry| entry.pointer("/DomainName") == Some(&json!(name)))
                .unwrap_or_else(|| panic!("case {label:?}: {name:?} is reported"));
            assert_eq!(
                Some(entry),
                single.body.pointer("/DomainStatus"),
                "case {label:?}: the entry for {name:?} must equal the single describe's, got {entry}"
            );
        }
    }
}

/// RULE, on the Region the request is scoped to: a name is resolved among the
/// domains of the Region the call was signed for, so the same name in two
/// Regions answers each caller with its own Region's record.
#[tokio::test]
async fn a_batch_describe_resolves_names_in_the_requests_own_region() {
    let router = populated().await;
    let shared = STORED[0].0;

    let here = describe_batch_in(&router, REGION, &[shared, "beta-domain"]).await;
    assert_eq!(
        reported_names("signing region", &here),
        vec![shared.to_string(), "beta-domain".to_string()],
        "the signing region's own domains must be reported, got {}",
        here.body
    );
    assert_eq!(
        here.at("signing region", "/DomainStatusList/0/ARN"),
        &json!(format!("arn:aws:es:{REGION}:000000000000:domain/{shared}")),
        "the reported ARN must name the region the request was signed for"
    );
    assert_eq!(
        here.at(
            "signing region",
            "/DomainStatusList/0/ElasticsearchClusterConfig/InstanceCount"
        ),
        &json!(STORED[0].2),
        "the reported record must be the signing region's"
    );

    // `beta-domain` exists only in REGION, so the other region reports only its
    // own copy of the shared name.
    let elsewhere = describe_batch_in(&router, OTHER_REGION, &[shared, "beta-domain"]).await;
    assert_eq!(
        reported_names("other region", &elsewhere),
        vec![shared.to_string()],
        "a name held only in another region must contribute no entry, got {}",
        elsewhere.body
    );
    assert_eq!(
        elsewhere.at(
            "other region",
            "/DomainStatusList/0/ElasticsearchClusterConfig/InstanceCount"
        ),
        &json!(9),
        "the other region must answer from its own stored record"
    );
    assert_eq!(
        elsewhere.at("other region", "/DomainStatusList/0/ARN"),
        &json!(format!(
            "arn:aws:es:{OTHER_REGION}:000000000000:domain/{shared}"
        )),
        "the other region's ARN must name that region"
    );
}

/// The batch describe REPORTS: calling it twice, and calling it at all, leaves
/// every later read answering exactly what it answered before.
#[tokio::test]
async fn a_batch_describe_changes_nothing_a_later_caller_sees() {
    let router = populated().await;
    let names: Vec<&str> = STORED.iter().map(|(name, _, _)| *name).collect();

    let before: Vec<Value> = {
        let mut reads = Vec::new();
        for name in &names {
            reads.push(describe_in(&router, REGION, name).await.body);
        }
        reads
    };

    let first = describe_batch_in(&router, REGION, &names).await;
    let second = describe_batch_in(&router, REGION, &names).await;
    assert_eq!(
        first.body, second.body,
        "two batch describes of an unchanged store must agree, got {} then {}",
        first.body, second.body
    );

    for (name, was) in names.iter().zip(before) {
        let now = describe_in(&router, REGION, name).await;
        assert_eq!(
            now.body, was,
            "case {name:?}: the batch describe must not change what a later read sees, got {}",
            now.body
        );
    }
}

/// A domain removed by `DeleteElasticsearchDomain` is gone from the batch
/// answer, and only that one: the batch reads the same store the delete wrote.
#[tokio::test]
async fn a_deleted_domain_leaves_the_batch_answer() {
    let router = populated().await;
    let names: Vec<&str> = STORED.iter().map(|(name, _, _)| *name).collect();

    let deleted = call_in(
        &router,
        REGION,
        "DELETE",
        &format!("/2015-01-01/es/domain/{}", names[1]),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "delete must succeed, got {} {}",
        deleted.status, deleted.body
    );

    let answer = describe_batch_in(&router, REGION, &names).await;
    let got = reported_names("after delete", &answer);
    let expected: Vec<String> = names
        .iter()
        .filter(|name| **name != names[1])
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        got, expected,
        "after a delete the batch must report {expected:?}, got {got:?}"
    );
}
