// SPDX-License-Identifier: CC0-1.0

//! `ListSecurityConfigurations`, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A list reports the security configurations the store HOLDS, newest first, and every
//!    reported record is the one the matching describe answers with — nothing is canned
//!    and nothing is echoed from the request that asked for it.
//! 2. `ListSecurityConfigurations` is `@paginated`, so `maxResults` + `nextToken` walk the
//!    stored set with no repeats and no gaps, every page is at most `maxResults` long, and
//!    a `nextToken` is reported only while configurations remain.
//! 3. `createdAfter` / `createdBefore` bound `createdAt` strictly, so a bound keeps all,
//!    some or none of the stored configurations, and the two together name a window.
//! 4. A list is a read: it does not change what the next reader sees, and a configuration
//!    leaves the list only when a delete removes it from state.
//! 5. A request the operation cannot answer is refused with the modelled
//!    `ValidationException` (HTTP 400): a page of fewer than one item, and a `nextToken`
//!    this service never issued.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{
    field, instant_millis, query_instant, text, Answer, Emr,
};

/// `GET /securityconfigurations{query}` — `ListSecurityConfigurations`.
async fn list_configurations(emr: &Emr, query: &str) -> Answer {
    emr.send("GET", &format!("/securityconfigurations{query}"), None)
        .await
}

/// `POST /securityconfigurations` — `CreateSecurityConfiguration`.
async fn create_configuration(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/securityconfigurations", Some(body)).await
}

/// `GET /securityconfigurations/{id}` — `DescribeSecurityConfiguration`.
async fn describe_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// `DELETE /securityconfigurations/{id}` — `DeleteSecurityConfiguration`.
async fn delete_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// The `securityConfigurations` a list answer carries, in the order it reported them.
fn listed(body: &Value, case: &str) -> Vec<Value> {
    field(body, "/securityConfigurations", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: securityConfigurations is not a list: {body}"))
        .clone()
}

/// The ids a list answer carries, in the order it reported them.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    listed(body, case)
        .iter()
        .map(|configuration| text(configuration, "/id", case))
        .collect()
}

/// The `CreateSecurityConfiguration` bodies the suites below store, spanning what a
/// security configuration carries: which half of `securityConfigurationData` is populated
/// and how deeply, whether a `containerProvider` is given, and how many tags there are.
fn stored_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            json!({
                "name": "list-minimal",
                "clientToken": "sc-list-tok-minimal",
                "securityConfigurationData": {},
            }),
        ),
        (
            "lake formation, a container provider and two tags",
            json!({
                "name": "list.lake_1",
                "clientToken": "sc-list-tok-lake",
                "containerProvider": {
                    "id": "eks-list",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "emr-list", "nodeLabel": "spark-nodes"}},
                },
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {
                            "authorizedSessionTagValue": "Emr Session Tag",
                            "queryEngineRoleArn":
                                "arn:aws:iam::000000000000:role/EMRQueryEngineRole",
                        },
                    },
                },
                "tags": {"team": "data", "env": "test"},
            }),
        ),
        (
            "iam authentication and one tag",
            json!({
                "name": "list#iam",
                "clientToken": "sc/list.tok-iam",
                "securityConfigurationData": {
                    "authenticationConfiguration": {
                        "iamConfiguration": {
                            "systemRole": "arn:aws:iam::000000000000:role/EMRSystemRole",
                        },
                    },
                },
                "tags": {"only": "one"},
            }),
        ),
        (
            "identity center and no tags",
            json!({
                "name": "list-identity-center",
                "clientToken": "sc-list-tok-idc",
                "securityConfigurationData": {
                    "authenticationConfiguration": {
                        "identityCenterConfiguration": {
                            "enableIdentityCenter": true,
                            "identityCenterInstanceARN":
                                "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                        },
                    },
                },
            }),
        ),
        (
            "both halves of the configuration data, at the name length bound",
            json!({
                // `ResourceNameString` is len 1..64: the upper bound.
                "name": "n".repeat(64),
                "clientToken": "t".repeat(64),
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {"authorizedSessionTagValue": "Both"},
                    },
                    "authenticationConfiguration": {
                        "iamConfiguration": {
                            "systemRole": "arn:aws:iam::000000000000:role/EMRBothRole",
                        },
                    },
                },
                "tags": {"a": "1", "b": "2", "c": "3"},
            }),
        ),
    ]
}

/// Store every row of [`stored_bodies`], in order, and return the ids in creation order.
async fn store_every_body(emr: &Emr) -> Vec<String> {
    let mut ids = Vec::new();
    for (label, body) in stored_bodies() {
        let created = create_configuration(emr, body).await;
        ids.push(text(created.ok(label), "/id", label));
    }
    ids
}

/// RULES 1 and 4: a list reports the stored records, newest first, each identical to the
/// record the matching describe answers with, and listing twice answers the same thing.
#[tokio::test]
async fn listing_security_configurations_reports_the_stored_records_newest_first() {
    let emr = Emr::new();
    let case = "the stored set";
    let created = store_every_body(&emr).await;

    let answer = list_configurations(&emr, "").await;
    let body = answer.ok(case).clone();
    assert!(
        body.get("nextToken").is_none(),
        "case {case:?}: an unpaged list that reported everything must carry no nextToken: {body}"
    );

    // Newest first: the reverse of creation order. The stored creation sequence breaks a
    // tie between two configurations created in the same millisecond, so this order is
    // total however fast the creates ran.
    let mut newest_first = created.clone();
    newest_first.reverse();
    assert_eq!(
        listed_ids(&body, case),
        newest_first,
        "case {case:?}: the list must report every stored configuration newest first: {body}"
    );

    // Every reported record is the record the describe answers with: the list reads the
    // same state rather than rebuilding a configuration of its own.
    for (index, configuration) in listed(&body, case).iter().enumerate() {
        let id = text(configuration, "/id", case);
        let described = describe_configuration(&emr, &id).await;
        let described = field(described.ok(case), "/securityConfiguration", case).clone();
        assert_eq!(
            configuration, &described,
            "case {case:?}: listed configuration {index} differs from the described record"
        );
    }

    // RULE 4: the list changed nothing a later reader sees.
    let again = list_configurations(&emr, "").await;
    assert_eq!(
        again.ok(case),
        &body,
        "case {case:?}: listing twice answered two different sets"
    );
}

/// RULE 2: `maxResults` + `nextToken` walk the stored set with no repeats and no gaps, and
/// a `nextToken` is reported only while configurations remain.
#[tokio::test]
async fn listing_security_configurations_pages_the_stored_set() {
    let emr = Emr::new();
    let created = store_every_body(&emr).await;
    let stored: BTreeSet<String> = created.iter().cloned().collect();
    assert_eq!(
        stored.len(),
        created.len(),
        "every row must have minted its own configuration, got {created:?}"
    );

    // Rows differ in the page size: one item at a time, divisors and non-divisors of the
    // stored count, exactly the stored count, and one past it.
    for page_size in [1usize, 2, 3, created.len(), created.len() + 2] {
        let label = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let answer = list_configurations(&emr, &query).await;
            let body = answer.ok(&label).clone();
            let page = listed_ids(&body, &label);
            assert!(
                page.len() <= page_size,
                "case {label:?}: a page carried {} configurations, more than maxResults",
                page.len()
            );
            walked.extend(page);
            match body.get("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        walked.len() < created.len(),
                        "case {label:?}: a nextToken was reported after all {} configurations",
                        created.len()
                    );
                    query = format!("?maxResults={page_size}&nextToken={token}");
                }
                None => break,
            }
        }

        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {label:?}: paging repeated a configuration, walked {walked:?}"
        );
        assert_eq!(
            unique.into_iter().cloned().collect::<BTreeSet<String>>(),
            stored,
            "case {label:?}: paging did not walk every stored configuration, walked {walked:?}"
        );
    }
}

/// RULE 3: `createdAfter` and `createdBefore` bound the stored `createdAt` strictly, so a
/// bound keeps all, some or none of the stored configurations.
///
/// The fixture creates one group, lets the clock move past it, takes a MIDPOINT, lets the
/// clock move again and creates a second group, so a bound that falls between the two
/// groups has configurations on either side of it.
#[tokio::test]
async fn listing_security_configurations_honours_the_created_at_bounds() {
    let emr = Emr::new();
    // Wider than the one-second resolution an ISO-8601 instant can be rendered at, so a
    // bound taken between the two groups stays between them however the wire renders it.
    let gap = Duration::from_millis(1_100);

    let before_everything = state::now_millis() - 2_000;
    let mut early = Vec::new();
    for index in 0..3 {
        let label = format!("early {index}");
        let created = create_configuration(
            &emr,
            json!({
                "name": format!("early-{index}"),
                "clientToken": format!("sc-bound-early-{index}"),
                "securityConfigurationData": {},
            }),
        )
        .await;
        early.push(text(created.ok(&label), "/id", &label));
    }

    tokio::time::sleep(gap).await;
    let midpoint = state::now_millis();
    tokio::time::sleep(gap).await;

    let mut late = Vec::new();
    for index in 0..2 {
        let label = format!("late {index}");
        let created = create_configuration(
            &emr,
            json!({
                "name": format!("late-{index}"),
                "clientToken": format!("sc-bound-late-{index}"),
                "securityConfigurationData": {},
            }),
        )
        .await;
        late.push(text(created.ok(&label), "/id", &label));
    }
    let after_everything = state::now_millis() + 2_000;

    let everything: BTreeSet<String> = early.iter().chain(late.iter()).cloned().collect();
    let early_set: BTreeSet<String> = early.iter().cloned().collect();
    let late_set: BTreeSet<String> = late.iter().cloned().collect();
    let nothing: BTreeSet<String> = BTreeSet::new();

    // Rows differ along which bound is applied and whether it keeps all, some or none.
    let cases: Vec<(String, String, &BTreeSet<String>)> = vec![
        ("no bound".to_string(), String::new(), &everything),
        (
            "createdAfter before every configuration".to_string(),
            format!("?createdAfter={}", query_instant(before_everything)),
            &everything,
        ),
        (
            "createdAfter after every configuration".to_string(),
            format!("?createdAfter={}", query_instant(after_everything)),
            &nothing,
        ),
        (
            "createdAfter at the midpoint".to_string(),
            format!("?createdAfter={}", query_instant(midpoint)),
            &late_set,
        ),
        (
            "createdBefore after every configuration".to_string(),
            format!("?createdBefore={}", query_instant(after_everything)),
            &everything,
        ),
        (
            "createdBefore before every configuration".to_string(),
            format!("?createdBefore={}", query_instant(before_everything)),
            &nothing,
        ),
        (
            "createdBefore at the midpoint".to_string(),
            format!("?createdBefore={}", query_instant(midpoint)),
            &early_set,
        ),
        (
            "a window around every configuration".to_string(),
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(before_everything),
                query_instant(after_everything)
            ),
            &everything,
        ),
        (
            "a window holding only the later group".to_string(),
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(midpoint),
                query_instant(after_everything)
            ),
            &late_set,
        ),
        (
            "a window holding only the earlier group".to_string(),
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(before_everything),
                query_instant(midpoint)
            ),
            &early_set,
        ),
        (
            "an empty window".to_string(),
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(after_everything),
                query_instant(before_everything)
            ),
            &nothing,
        ),
    ];

    for (label, query, expected) in &cases {
        let answer = list_configurations(&emr, query).await;
        let body = answer.ok(label).clone();
        let reported: BTreeSet<String> = listed_ids(&body, label).into_iter().collect();
        assert_eq!(
            &reported, *expected,
            "case {label:?}: {query} reported the wrong configurations: {body}"
        );
        // Whatever a bound kept really is inside it: the reported `createdAt` values are
        // the records' own instants, not the bounds.
        for configuration in listed(&body, label) {
            let created_at = instant_millis(&configuration, "/createdAt", label);
            assert!(
                created_at >= before_everything && created_at <= after_everything,
                "case {label:?}: createdAt {created_at} is outside [{before_everything}, {after_everything}]"
            );
        }
    }
}

/// RULE 4 for the other direction, and RULE 5: a configuration leaves the list only when a
/// delete removes it from state, and a request the operation cannot answer is refused with
/// the modelled `ValidationException` rather than answered with a page.
#[tokio::test]
async fn a_deleted_security_configuration_leaves_the_list_and_bad_requests_are_refused() {
    let emr = Emr::new();
    let case = "an empty store";

    // An empty store lists an empty set, never a canned record.
    let empty = list_configurations(&emr, "").await;
    let empty = empty.ok(case).clone();
    assert_eq!(
        listed_ids(&empty, case),
        Vec::<String>::new(),
        "case {case:?}: a store holding no configuration must list none: {empty}"
    );
    assert!(
        empty.get("nextToken").is_none(),
        "case {case:?}: an empty list must carry no nextToken: {empty}"
    );

    let created = store_every_body(&emr).await;
    let mut remaining: BTreeSet<String> = created.iter().cloned().collect();

    // Rows differ in which stored configuration is deleted: the newest, the oldest, and
    // one in the middle.
    let deletions = [
        ("the newest", created[created.len() - 1].clone()),
        ("the oldest", created[0].clone()),
        ("one in the middle", created[created.len() / 2].clone()),
    ];
    for (label, id) in &deletions {
        delete_configuration(&emr, id).await.ok(label);
        remaining.remove(id);
        let answer = list_configurations(&emr, "").await;
        let body = answer.ok(label).clone();
        let reported: BTreeSet<String> = listed_ids(&body, label).into_iter().collect();
        assert_eq!(
            reported, remaining,
            "case {label:?}: after deleting {id:?} the list must report exactly what is left: {body}"
        );
    }

    // RULE 5: a page of fewer than one configuration is not a page, and a `nextToken` this
    // service never issued for THIS listing is not a position in its order. The scoped
    // cursor of another listing comes from the one encoder every paginated list here uses
    // (`state::encode_list_cursor`), so this row cannot drift from the format.
    let foreign_cursor = state::encode_list_cursor(state::LIST_JOB_TEMPLATES_SCOPE, 1, 2);
    for (label, query) in [
        ("maxResults below one", "?maxResults=0".to_string()),
        ("a negative maxResults", "?maxResults=-3".to_string()),
        ("a forged nextToken", "?nextToken=not-a-cursor".to_string()),
        ("a nextToken with one component", "?nextToken=12345".to_string()),
        (
            "a nextToken carrying no scope",
            "?nextToken=1:2".to_string(),
        ),
        (
            "a nextToken with three components",
            "?nextToken=1:2:3".to_string(),
        ),
        (
            "a nextToken whose parts are not numbers",
            "?nextToken=a:b".to_string(),
        ),
        (
            "a cursor another listing issued",
            format!("?nextToken={foreign_cursor}"),
        ),
    ] {
        list_configurations(&emr, &query)
            .await
            .refused(label, 400, "ValidationException");
    }

    // The refusals changed nothing: the store still lists exactly what is left.
    let after = list_configurations(&emr, "").await;
    let body = after.ok("after the refusals").clone();
    assert_eq!(
        listed_ids(&body, "after the refusals")
            .into_iter()
            .collect::<BTreeSet<String>>(),
        remaining,
        "a refused list must leave the stored set untouched: {body}"
    );
}
