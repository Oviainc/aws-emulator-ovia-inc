// SPDX-License-Identifier: CC0-1.0

//! `ListVirtualClusters`, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A list reports the virtual clusters this region's endpoint HOLDS, newest first, and
//!    every reported cluster is the record the matching describe answers with — nothing is
//!    canned and nothing is echoed from the request that asked for it.
//! 2. `ListVirtualClusters` is `@paginated`, so `maxResults` + `nextToken` walk the stored
//!    set with no repeats and no gaps, every page is at most `maxResults` long, and a
//!    `nextToken` is reported only while clusters remain.
//! 3. `states`, `containerProviderId`, `containerProviderType` and
//!    `eksAccessEntryIntegrated` filter the listing, and a filter the request omits
//!    constrains nothing.
//! 4. `createdAfter` / `createdBefore` bound `createdAt` strictly, so a bound keeps all,
//!    some or none of the stored clusters and the two together name a window.
//! 5. A list is a read: it does not change what the next reader sees, and the state it
//!    reports follows the stored record as the clock moves it.
//! 6. A request the operation cannot answer is refused with the modelled
//!    `ValidationException` (HTTP 400): a page of fewer than one item, and a `nextToken`
//!    this listing never issued.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::{
    CONTAINER_PROVIDER_TYPE_VALUES, VIRTUAL_CLUSTER_STATE_VALUES,
};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::wait_for_the_clock;
use crate::virtual_cluster_job_run_loop::{
    assert_resource_id, field, instant_millis, query_instant, text, Answer, Emr, REGION,
};

/// `GET /virtualclusters{query}` — `ListVirtualClusters`.
async fn list_clusters(emr: &Emr, query: &str) -> Answer {
    emr.send("GET", &format!("/virtualclusters{query}"), None)
        .await
}

/// `POST /virtualclusters` — `CreateVirtualCluster`.
async fn create_cluster(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/virtualclusters", Some(body)).await
}

/// `GET /virtualclusters/{id}` — `DescribeVirtualCluster`.
async fn describe_cluster(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/virtualclusters/{id}"), None).await
}

/// `DELETE /virtualclusters/{id}` — `DeleteVirtualCluster`.
async fn delete_cluster(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/virtualclusters/{id}"), None)
        .await
}

/// The `virtualClusters` a list answer carries, in the order it reported them.
fn listed(body: &Value, case: &str) -> Vec<Value> {
    field(body, "/virtualClusters", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: virtualClusters is not a list: {body}"))
        .clone()
}

/// The ids a list answer carries, in the order it reported them.
fn listed_ids(body: &Value, case: &str) -> Vec<String> {
    listed(body, case)
        .iter()
        .map(|cluster| text(cluster, "/id", case))
        .collect()
}

/// The ids a list answer carries, as a set.
fn listed_set(body: &Value, case: &str) -> BTreeSet<String> {
    listed_ids(body, case).into_iter().collect()
}

/// The `CreateVirtualCluster` bodies the suites below store, spanning what a virtual
/// cluster carries: which optional members are present, the shape of
/// `containerProvider.info`, the container cluster it registers, the form of the name and
/// how many tags there are.
///
/// Each row registers its own `(containerProvider.id, namespace)` pair, because one
/// Kubernetes namespace of one container cluster carries at most one live virtual cluster.
fn stored_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            json!({
                "name": "list-minimal",
                "clientToken": "vc-list-tok-minimal",
                "containerProvider": {"id": "eks-alpha", "type": "EKS"},
            }),
        ),
        (
            "a namespace, two tags and sessionEnabled",
            json!({
                "name": "list.tagged_1",
                "clientToken": "vc-list-tok-tagged",
                "containerProvider": {
                    "id": "eks-alpha",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "ns-one"}},
                },
                "tags": {"team": "data", "env": "test"},
                "sessionEnabled": true,
            }),
        ),
        (
            "another container cluster in the same namespace",
            json!({
                "name": "list#beta",
                "clientToken": "vc-list-tok-beta",
                "containerProvider": {
                    "id": "eks-beta",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "ns-one"}},
                },
                "tags": {"only": "one"},
            }),
        ),
        (
            "a scheduler configuration",
            json!({
                "name": "list/scheduled-9",
                "clientToken": "vc-list-tok-scheduled",
                "containerProvider": {
                    "id": "eks-beta",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "ns-two"}},
                },
                "schedulerConfiguration": {"maxConcurrentJobRuns": 4, "maxInQueueJobRuns": 100},
            }),
        ),
        (
            "a node label at the name length bound",
            json!({
                // `ResourceNameString` is len 1..64: the upper bound.
                "name": "n".repeat(64),
                "clientToken": "t".repeat(64),
                "containerProvider": {
                    "id": "eks-gamma",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "ns-three", "nodeLabel": "spark-nodes"}},
                },
                "sessionEnabled": false,
            }),
        ),
    ]
}

/// Store every row of [`stored_bodies`], in order, and return the ids in creation order.
async fn store_every_body(emr: &Emr) -> Vec<String> {
    let mut ids = Vec::new();
    for (label, body) in stored_bodies() {
        let created = create_cluster(emr, body).await;
        let id = text(created.ok(label), "/id", label);
        assert_resource_id(label, "the minted virtual cluster id", &id);
        ids.push(id);
    }
    ids
}

/// RULES 1 and 5: a list reports the stored clusters, newest first, each identical to the
/// record the matching describe answers with, and listing twice answers the same thing.
#[tokio::test]
async fn listing_virtual_clusters_reports_the_stored_records_newest_first() {
    let emr = Emr::new();
    let case = "the stored set";
    let created = store_every_body(&emr).await;

    let answer = list_clusters(&emr, "").await;
    let body = answer.ok(case).clone();
    assert!(
        body.get("nextToken").is_none(),
        "case {case:?}: an unpaged list that reported everything must carry no nextToken: {body}"
    );

    // Newest first: the reverse of creation order. The stored creation sequence breaks a
    // tie between two clusters created in the same millisecond, so this order is total
    // however fast the creates ran.
    let mut newest_first = created.clone();
    newest_first.reverse();
    assert_eq!(
        listed_ids(&body, case),
        newest_first,
        "case {case:?}: the list must report every stored cluster newest first: {body}"
    );

    // Every reported cluster is the record the describe answers with: the list reads the
    // same state rather than rebuilding a cluster of its own.
    for (index, cluster) in listed(&body, case).iter().enumerate() {
        let id = text(cluster, "/id", case);
        let described = describe_cluster(&emr, &id).await;
        let described = field(described.ok(case), "/virtualCluster", case).clone();
        assert_eq!(
            cluster, &described,
            "case {case:?}: listed cluster {index} differs from the described record"
        );
        // The ARN carries the region the caller signed for and the emulator's account.
        assert_eq!(
            text(cluster, "/arn", case),
            format!(
                "arn:aws:emr-containers:{REGION}:{}:/virtualclusters/{id}",
                engine::emulator_account_id()
            ),
            "case {case:?}: listed cluster {index} carries the wrong arn"
        );
    }

    // RULE 5: the list changed nothing a later reader sees.
    let again = list_clusters(&emr, "").await;
    assert_eq!(
        again.ok(case),
        &body,
        "case {case:?}: listing twice answered two different sets"
    );
}

/// RULE 2: `maxResults` + `nextToken` walk the stored set with no repeats and no gaps, and
/// a `nextToken` is reported only while clusters remain.
#[tokio::test]
async fn listing_virtual_clusters_pages_the_stored_set() {
    let emr = Emr::new();
    let created = store_every_body(&emr).await;
    let stored: BTreeSet<String> = created.iter().cloned().collect();
    assert_eq!(
        stored.len(),
        created.len(),
        "every row must have minted its own cluster, got {created:?}"
    );

    // Rows differ in the page size: one item at a time, divisors and non-divisors of the
    // stored count, exactly the stored count, and one past it.
    for page_size in [1usize, 2, 3, created.len(), created.len() + 2] {
        let label = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let answer = list_clusters(&emr, &query).await;
            let body = answer.ok(&label).clone();
            let page = listed_ids(&body, &label);
            assert!(
                page.len() <= page_size,
                "case {label:?}: a page carried {} clusters, more than maxResults",
                page.len()
            );
            walked.extend(page);
            match body.get("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        walked.len() < created.len(),
                        "case {label:?}: a nextToken was reported after all {} clusters",
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
            "case {label:?}: paging repeated a cluster, walked {walked:?}"
        );
        assert_eq!(
            unique.into_iter().cloned().collect::<BTreeSet<String>>(),
            stored,
            "case {label:?}: paging did not walk every stored cluster, walked {walked:?}"
        );
    }

    // A page is still a page under a filter: paging and filtering compose.
    let label = "one at a time under a provider filter";
    let mut walked: Vec<String> = Vec::new();
    let mut query = "?maxResults=1&containerProviderId=eks-beta".to_string();
    loop {
        let answer = list_clusters(&emr, &query).await;
        let body = answer.ok(label).clone();
        walked.extend(listed_ids(&body, label));
        match body.get("nextToken").and_then(Value::as_str) {
            Some(token) => {
                query = format!("?maxResults=1&containerProviderId=eks-beta&nextToken={token}");
            }
            None => break,
        }
    }
    assert_eq!(
        walked.len(),
        2,
        "case {label:?}: the two clusters of that container cluster must be walked once each, got {walked:?}"
    );
}

/// RULE 3: every filter the operation models keeps exactly the clusters it names, and one
/// the request omits constrains nothing.
///
/// The `states` rows are driven from the generated `VIRTUAL_CLUSTER_STATE_VALUES` catalog
/// (`smithy_query(query="shape", shape="VirtualClusterState")`), so every modelled state —
/// including the ones no stored cluster is in — is asserted, not only the one the fixtures
/// create.
#[tokio::test]
async fn listing_virtual_clusters_honours_every_modelled_filter() {
    let emr = Emr::new();
    let created = store_every_body(&emr).await;
    let everything: BTreeSet<String> = created.iter().cloned().collect();
    let nothing: BTreeSet<String> = BTreeSet::new();
    // The container cluster each fixture registered, in the creation order of
    // `stored_bodies`.
    let alpha: BTreeSet<String> = created[0..2].iter().cloned().collect();
    let beta: BTreeSet<String> = created[2..4].iter().cloned().collect();
    let gamma: BTreeSet<String> = created[4..5].iter().cloned().collect();

    let mut cases: Vec<(String, String, BTreeSet<String>)> = vec![
        ("no filter".to_string(), String::new(), everything.clone()),
        (
            "containerProviderId naming two clusters".to_string(),
            "?containerProviderId=eks-alpha".to_string(),
            alpha.clone(),
        ),
        (
            "containerProviderId naming another container cluster".to_string(),
            "?containerProviderId=eks-beta".to_string(),
            beta.clone(),
        ),
        (
            "containerProviderId naming one cluster".to_string(),
            "?containerProviderId=eks-gamma".to_string(),
            gamma,
        ),
        (
            "containerProviderId naming no container cluster".to_string(),
            "?containerProviderId=eks-nothing".to_string(),
            nothing.clone(),
        ),
        (
            "containerProviderId that is a prefix of a stored one".to_string(),
            "?containerProviderId=eks".to_string(),
            nothing.clone(),
        ),
        (
            "eksAccessEntryIntegrated=false".to_string(),
            "?eksAccessEntryIntegrated=false".to_string(),
            everything.clone(),
        ),
        (
            "eksAccessEntryIntegrated=true".to_string(),
            "?eksAccessEntryIntegrated=true".to_string(),
            nothing.clone(),
        ),
        (
            "a container cluster and its state together".to_string(),
            format!(
                "?containerProviderId=eks-alpha&states={}",
                state::CLUSTER_RUNNING
            ),
            alpha,
        ),
        (
            "a container cluster and a state nothing is in together".to_string(),
            format!(
                "?containerProviderId=eks-beta&states={}",
                state::CLUSTER_TERMINATED
            ),
            nothing.clone(),
        ),
    ];
    // `containerProviderType` is an enum, so the catalog is the whole set of values a
    // request can carry; every fixture registers an Amazon EKS container cluster, which is
    // the only type the model declares.
    for provider_type in CONTAINER_PROVIDER_TYPE_VALUES {
        cases.push((
            format!("containerProviderType {provider_type}"),
            format!("?containerProviderType={provider_type}"),
            everything.clone(),
        ));
    }
    // Every created cluster is RUNNING, so the catalog's other states keep nothing.
    for cluster_state in VIRTUAL_CLUSTER_STATE_VALUES {
        let expected = if *cluster_state == state::CLUSTER_RUNNING {
            everything.clone()
        } else {
            nothing.clone()
        };
        cases.push((
            format!("states {cluster_state}"),
            format!("?states={cluster_state}"),
            expected,
        ));
    }
    // Two states at once keep the union of what each keeps.
    cases.push((
        "two states, one of them the current one".to_string(),
        format!(
            "?states={}&states={}",
            state::CLUSTER_TERMINATING,
            state::CLUSTER_RUNNING
        ),
        everything.clone(),
    ));
    cases.push((
        "two states, neither of them the current one".to_string(),
        format!(
            "?states={}&states={}",
            state::CLUSTER_TERMINATING,
            state::CLUSTER_TERMINATED
        ),
        nothing,
    ));

    for (label, query, expected) in &cases {
        let answer = list_clusters(&emr, query).await;
        let body = answer.ok(label).clone();
        assert_eq!(
            &listed_set(&body, label),
            expected,
            "case {label:?}: {query} reported the wrong clusters: {body}"
        );
        // Whatever a filter kept really satisfies it: the reported members are the
        // records' own, not the filter's.
        for cluster in listed(&body, label) {
            assert!(
                everything.contains(&text(&cluster, "/id", label)),
                "case {label:?}: the list reported a cluster nothing created: {cluster}"
            );
        }
    }
}

/// RULE 4: `createdAfter` and `createdBefore` bound the stored `createdAt` strictly, so a
/// bound keeps all, some or none of the stored clusters.
///
/// The fixture creates one group, lets the clock move past it, takes a MIDPOINT, lets the
/// clock move again and creates a second group, so a bound that falls between the two
/// groups has clusters on either side of it.
#[tokio::test]
async fn listing_virtual_clusters_honours_the_created_at_bounds() {
    let emr = Emr::new();
    // Wider than the one-second resolution an ISO-8601 instant can be rendered at, so a
    // bound taken between the two groups stays between them however the wire renders it.
    let gap = Duration::from_millis(1_100);

    let before_everything = state::now_millis() - 2_000;
    let mut early = Vec::new();
    for index in 0..3 {
        let label = format!("early {index}");
        early.push(
            emr.cluster(&format!("bound-early-{index}"), &format!("vc-bound-early-{index}"))
                .await,
        );
        assert_resource_id(&label, "the minted virtual cluster id", &early[index]);
    }

    tokio::time::sleep(gap).await;
    let midpoint = state::now_millis();
    tokio::time::sleep(gap).await;

    let mut late = Vec::new();
    for index in 0..2 {
        late.push(
            emr.cluster(&format!("bound-late-{index}"), &format!("vc-bound-late-{index}"))
                .await,
        );
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
            "createdAfter before every cluster".to_string(),
            format!("?createdAfter={}", query_instant(before_everything)),
            &everything,
        ),
        (
            "createdAfter after every cluster".to_string(),
            format!("?createdAfter={}", query_instant(after_everything)),
            &nothing,
        ),
        (
            "createdAfter at the midpoint".to_string(),
            format!("?createdAfter={}", query_instant(midpoint)),
            &late_set,
        ),
        (
            "createdBefore after every cluster".to_string(),
            format!("?createdBefore={}", query_instant(after_everything)),
            &everything,
        ),
        (
            "createdBefore before every cluster".to_string(),
            format!("?createdBefore={}", query_instant(before_everything)),
            &nothing,
        ),
        (
            "createdBefore at the midpoint".to_string(),
            format!("?createdBefore={}", query_instant(midpoint)),
            &early_set,
        ),
        (
            "a window around every cluster".to_string(),
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
        let answer = list_clusters(&emr, query).await;
        let body = answer.ok(label).clone();
        assert_eq!(
            &listed_set(&body, label),
            *expected,
            "case {label:?}: {query} reported the wrong clusters: {body}"
        );
        // Whatever a bound kept really is inside it: the reported `createdAt` values are
        // the records' own instants, not the bounds.
        for cluster in listed(&body, label) {
            let created_at = instant_millis(&cluster, "/createdAt", label);
            assert!(
                created_at >= before_everything && created_at <= after_everything,
                "case {label:?}: createdAt {created_at} is outside [{before_everything}, {after_everything}]"
            );
        }
    }
}

/// RULE 5 for the state the listing reports, and the regional rule: a cluster's listed
/// state follows the stored record as the clock moves it, and a cluster created at another
/// region's endpoint is not one this endpoint lists.
#[tokio::test]
async fn a_listed_cluster_follows_the_clock_and_its_own_region() {
    let emr = Emr::new();
    let here = emr.cluster("clock-here", "vc-clock-here").await;
    // The same request signed for another region: a cluster exists at the endpoint that
    // minted it and nowhere else.
    let elsewhere = text(
        emr.send_in(
            "eu-west-1",
            "POST",
            "/virtualclusters",
            Some(json!({
                "name": "clock-elsewhere",
                "clientToken": "vc-clock-elsewhere",
                "containerProvider": {"id": "eks-elsewhere", "type": "EKS"},
            })),
        )
        .await
        .ok("a cluster at another region's endpoint"),
        "/id",
        "a cluster at another region's endpoint",
    );

    let case = "two regions, one endpoint";
    let body = list_clusters(&emr, "").await.ok(case).clone();
    assert_eq!(
        listed_set(&body, case),
        BTreeSet::from([here.clone()]),
        "case {case:?}: this endpoint must list only the cluster it minted: {body}"
    );
    let other = emr
        .send_in("eu-west-1", "GET", "/virtualclusters", None)
        .await;
    let other_body = other.ok("the other region's endpoint").clone();
    assert_eq!(
        listed_set(&other_body, "the other region's endpoint"),
        BTreeSet::from([elsewhere]),
        "the other region's endpoint must list only its own cluster: {other_body}"
    );

    // A teardown starts a transition the clock owes: the listing reports the transient
    // state first and the settled one once the span has passed, in step with the describe.
    delete_cluster(&emr, &here).await.ok("the teardown");
    for (label, expected) in [
        ("right after the teardown", state::CLUSTER_TERMINATING),
        ("once the clock has settled it", state::CLUSTER_TERMINATED),
    ] {
        if expected == state::CLUSTER_TERMINATED {
            wait_for_the_clock();
        }
        let body = list_clusters(&emr, "").await.ok(label).clone();
        let clusters = listed(&body, label);
        assert_eq!(
            clusters.len(),
            1,
            "case {label:?}: a torn-down cluster stays listed: {body}"
        );
        assert_eq!(
            text(&clusters[0], "/state", label),
            expected,
            "case {label:?}: the listed state must be the stored one: {body}"
        );
        let described = describe_cluster(&emr, &here).await;
        assert_eq!(
            &clusters[0],
            field(described.ok(label), "/virtualCluster", label),
            "case {label:?}: the listed cluster and the described record disagree"
        );
        // The state filter follows the stored state too.
        let filtered = list_clusters(&emr, &format!("?states={expected}")).await;
        assert_eq!(
            listed_set(filtered.ok(label), label),
            BTreeSet::from([here.clone()]),
            "case {label:?}: a states filter naming the current state must keep the cluster"
        );
    }
}

/// RULE 6, and RULE 5 once more: a request the operation cannot answer is refused with the
/// modelled `ValidationException` rather than answered with a page, and the refusals leave
/// the stored set untouched.
#[tokio::test]
async fn an_unanswerable_virtual_cluster_listing_is_refused() {
    let emr = Emr::new();
    let case = "an empty store";

    // An empty store lists an empty set, never a canned record.
    let empty = list_clusters(&emr, "").await;
    let empty = empty.ok(case).clone();
    assert_eq!(
        listed_ids(&empty, case),
        Vec::<String>::new(),
        "case {case:?}: a store holding no cluster must list none: {empty}"
    );
    assert!(
        empty.get("nextToken").is_none(),
        "case {case:?}: an empty list must carry no nextToken: {empty}"
    );

    let created = store_every_body(&emr).await;
    let stored: BTreeSet<String> = created.iter().cloned().collect();

    // A page of fewer than one cluster is not a page, and a `nextToken` this service never
    // issued for THIS listing is not a position in its order. The scoped cursor of another
    // listing comes from the one encoder every paginated list here uses
    // (`state::encode_list_cursor`), so this row cannot drift from the format.
    let foreign_cursor = state::encode_list_cursor(state::LIST_SECURITY_CONFIGURATIONS_SCOPE, 1, 2);
    for (label, query) in [
        ("maxResults below one", "?maxResults=0".to_string()),
        ("a negative maxResults", "?maxResults=-3".to_string()),
        ("a forged nextToken", "?nextToken=not-a-cursor".to_string()),
        ("a nextToken with one component", "?nextToken=12345".to_string()),
        ("a nextToken carrying no scope", "?nextToken=1:2".to_string()),
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
        list_clusters(&emr, &query)
            .await
            .refused(label, 400, "ValidationException");
    }

    // The refusals changed nothing: the store still lists exactly what it held.
    let after = list_clusters(&emr, "").await;
    let body = after.ok("after the refusals").clone();
    assert_eq!(
        listed_set(&body, "after the refusals"),
        stored,
        "a refused list must leave the stored set untouched: {body}"
    );
}
