// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS on-demand cluster insights refresh slice:
//! `StartInsightsRefresh` and `DescribeInsightsRefresh`, driven over the wire
//! through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (cluster name shape
//! and length, how many starts a caller makes, how many refreshes a cluster runs in
//! a row, which cluster status the call lands on, which operation is called, which
//! cluster the refresh belongs to), and each assertion names its case and prints the
//! actual value.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, CLUSTER_ACTIVATION_DELAY, INSIGHTS_REFRESH_DURATION,
    MAX_CLUSTER_NAME_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test binary shares, declared once in
// `tests/all/wire.rs`: the router under test, one request, and the answer reduced
// to what a client branches on.
use crate::wire::{call, emulator, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// The cluster role `CreateCluster` passes, and the connector role
/// `RegisterCluster` passes: a role of this emulator's own account, so neither
/// create is refused for the role it names.
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// Slack added to every clock-owed wait, so a loaded machine still observes the
/// move the clock owed.
const SLACK: Duration = Duration::from_millis(250);

async fn start_refresh(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/insights-refresh", encode_label(cluster)),
        "",
    )
    .await
}

async fn describe_refresh(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}/insights-refresh", encode_label(cluster)),
        "",
    )
    .await
}

async fn describe_cluster(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(cluster)),
        "",
    )
    .await
}

async fn create_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    let body = serde_json::json!({
        "name": name,
        "roleArn": PASSABLE_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    call(router, REGION, "POST", "/clusters", &body.to_string()).await
}

/// Create every named cluster and wait out the one clock-owed `CREATING` ->
/// `ACTIVE` move, because an insights refresh only runs on an `ACTIVE` cluster.
async fn active_clusters(router: &MultiServiceRouter, names: &[&str]) {
    for name in names {
        let created = create_cluster(router, name).await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateCluster for {name:?}, got {created:?}"
        );
    }
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + SLACK).await;
    for name in names {
        let described = describe_cluster(router, name).await;
        assert_eq!(
            described.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "setup: cluster {name:?} must be ACTIVE before a refresh is started, got {described:?}"
        );
    }
}

/// A timestamp member of an answer, as the epoch-seconds number restJson1 carries.
fn instant(answer: &Answer, member: &str) -> f64 {
    answer.body[member].as_f64().unwrap_or_else(|| {
        panic!("expected a numeric {member} in {answer:?}");
    })
}

/// Assert that a described refresh is in one of the two states a STARTED refresh
/// can be in, carrying the `endedAt` that status implies — `IN_PROGRESS` has not
/// ended and reports none, an ended one reports it — and answer its `startedAt`.
///
/// Which of the two it is depends only on whether the clock has reached the stored
/// due instant, so a suite that is not about that span asserts this INVARIANT
/// rather than the status, and never flakes on a loaded machine. The status is
/// asserted exactly where it is not timing-dependent: on the answer the start
/// itself computes, and on the read after the wait below.
fn started_refresh_instant(case: &str, described: &Answer) -> f64 {
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribeInsightsRefresh, got {described:?}"
    );
    let status = described.body["status"].as_str().unwrap_or_default();
    let ended = described
        .body
        .get("endedAt")
        .is_some_and(|value| !value.is_null());
    match status {
        "IN_PROGRESS" => assert!(
            !ended,
            "case {case:?}: a running refresh has not ended, got {described:?}"
        ),
        "COMPLETED" => assert!(
            ended,
            "case {case:?}: an ended refresh reports endedAt, got {described:?}"
        ),
        other => panic!(
            "case {case:?}: expected IN_PROGRESS or COMPLETED, got {other:?} in {described:?}"
        ),
    }
    instant(described, "startedAt")
}

// ---------------------------------------------------------------------------
// RULE: `StartInsightsRefresh` begins ONE refresh of the named cluster's insights
// and reports it `IN_PROGRESS`; `DescribeInsightsRefresh` reports that same
// refresh — its status, its `startedAt`, and an `endedAt` only once it has ended —
// and the clock completes it after a bounded span, after which every read agrees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_started_refresh_is_read_back_and_completes() {
    // Rows differ along the cluster name the refresh hangs off: the plainest name,
    // a mixed-case name with both punctuation characters the name pattern allows
    // (`^[0-9A-Za-z][A-Za-z0-9\-_]*$`), a single leading-digit name, and a name at
    // the published maximum length.
    let long_name = format!("insights{}", "x".repeat(MAX_CLUSTER_NAME_LEN - "insights".len()));
    let cases: Vec<(&str, String)> = vec![
        ("plain name", "insights".to_string()),
        (
            "mixed case, hyphen and underscore",
            "Insights-Refresh_01".to_string(),
        ),
        ("single leading digit", "9".to_string()),
        ("maximum length name", long_name.clone()),
    ];

    for (case, name) in &cases {
        assert!(
            name.len() <= MAX_CLUSTER_NAME_LEN,
            "case {case:?}: the row's own name must be a legal cluster name, got {} characters",
            name.len()
        );
        let router = emulator();
        active_clusters(&router, &[name.as_str()]).await;

        let started = start_refresh(&router, name).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from StartInsightsRefresh, got {started:?}"
        );
        assert_eq!(
            started.body["status"],
            serde_json::json!("IN_PROGRESS"),
            "case {case:?}: a refresh starts IN_PROGRESS, got {:?}",
            started.body["status"]
        );
        assert!(
            started.body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a non-empty message, got {:?}",
            started.body["message"]
        );

        let running = describe_refresh(&router, name).await;
        let started_at = started_refresh_instant(case, &running);

        tokio::time::sleep(INSIGHTS_REFRESH_DURATION + SLACK).await;

        let completed = describe_refresh(&router, name).await;
        assert_eq!(
            completed.status, 200,
            "case {case:?}: expected 200 from DescribeInsightsRefresh, got {completed:?}"
        );
        assert_eq!(
            completed.body["status"],
            serde_json::json!("COMPLETED"),
            "case {case:?}: the clock completes the refresh, got {completed:?}"
        );
        assert_eq!(
            instant(&completed, "startedAt"),
            started_at,
            "case {case:?}: completing a refresh does not move its startedAt, got {completed:?}"
        );
        let ended_at = instant(&completed, "endedAt");
        assert!(
            ended_at >= started_at,
            "case {case:?}: expected endedAt >= startedAt ({started_at}), got {ended_at}"
        );

        // A read changes nothing a later caller sees: the same answer, twice.
        let again = describe_refresh(&router, name).await;
        assert_eq!(
            again.body, completed.body,
            "case {case:?}: a second read must report the same refresh, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster runs at most ONE on-demand refresh at a time, so a start while
// the cluster's refresh is still `IN_PROGRESS` is refused with
// `InvalidRequestException` and the running refresh is left exactly as it was.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_start_while_a_refresh_runs_is_refused_and_changes_nothing() {
    // Rows differ along how many extra starts the caller piles on: every one of
    // them is refused, not just the second.
    let cases: Vec<(&str, usize)> = vec![
        ("one extra start", 1),
        ("two extra starts", 2),
        ("five extra starts", 5),
    ];

    for (index, (case, extra)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("conflict-{index}");
        active_clusters(&router, &[name.as_str()]).await;

        let started = start_refresh(&router, &name).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from the first StartInsightsRefresh, got {started:?}"
        );
        let running = describe_refresh(&router, &name).await;
        let started_at = instant(&running, "startedAt");

        for attempt in 1..=*extra {
            let refused = start_refresh(&router, &name).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: start {attempt} must be refused with 400, got {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidRequestException",
                "case {case:?}: start {attempt} expected InvalidRequestException, got {refused:?}"
            );
            assert_eq!(
                refused.body["clusterName"],
                serde_json::json!(name),
                "case {case:?}: the refusal names the cluster, got {refused:?}"
            );
        }

        let still = describe_refresh(&router, &name).await;
        assert_eq!(
            still.body["status"],
            serde_json::json!("IN_PROGRESS"),
            "case {case:?}: the running refresh survives the refusals, got {still:?}"
        );
        assert_eq!(
            instant(&still, "startedAt"),
            started_at,
            "case {case:?}: a refused start must not restart the running refresh, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: once a refresh has ended, another can be started, and what
// `DescribeInsightsRefresh` reports is the LATEST one: a later `startedAt` and no
// `endedAt` until the clock ends it too.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_refresh_replaces_the_one_that_ended() {
    // Rows differ along how many refreshes the cluster runs in a row.
    let cases: Vec<(&str, usize)> = vec![("two refreshes", 2), ("three refreshes", 3)];

    for (index, (case, rounds)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("successive-{index}");
        active_clusters(&router, &[name.as_str()]).await;

        let mut previous_start: Option<f64> = None;
        for round in 1..=*rounds {
            let started = start_refresh(&router, &name).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: round {round} expected 200 from StartInsightsRefresh, \
                 got {started:?}"
            );
            assert_eq!(
                started.body["status"],
                serde_json::json!("IN_PROGRESS"),
                "case {case:?}: round {round} starts a new refresh IN_PROGRESS, got {started:?}"
            );
            let running = describe_refresh(&router, &name).await;
            let started_at = started_refresh_instant(case, &running);
            if let Some(earlier) = previous_start {
                assert!(
                    started_at >= earlier,
                    "case {case:?}: round {round} expected a startedAt at or after the \
                     previous refresh's ({earlier}), got {started_at}"
                );
            }
            previous_start = Some(started_at);

            tokio::time::sleep(INSIGHTS_REFRESH_DURATION + SLACK).await;
            let completed = describe_refresh(&router, &name).await;
            assert_eq!(
                completed.body["status"],
                serde_json::json!("COMPLETED"),
                "case {case:?}: round {round} must complete, got {completed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the analysis needs a live control plane Amazon EKS manages, so a refresh
// starts only on a cluster that is `ACTIVE` — any other status is
// `InvalidRequestException` — and not at all on a cluster EKS does not manage,
// which is the permanent `ClientException` refusal. Nothing is recorded either way.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refresh_only_starts_on_an_active_cluster() {
    // Rows differ along what stops the refresh: the `CREATING` of a cluster whose
    // activation the clock has not owed yet (a state refusal), and a connected
    // cluster, which has no Amazon EKS control plane at all (the gate).
    let cases: Vec<(&str, bool, &str)> = vec![
        ("CREATING cluster", false, "InvalidRequestException"),
        ("connected cluster", true, "ClientException"),
    ];

    for (index, (case, connected, expected_code)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("inactive-{index}");
        let created = if *connected {
            let body = serde_json::json!({
                "name": name,
                "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": "OTHER" },
            });
            call(
                &router,
                REGION,
                "POST",
                "/cluster-registrations",
                &body.to_string(),
            )
            .await
        } else {
            create_cluster(&router, &name).await
        };
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup expected 200 creating the cluster, got {created:?}"
        );
        let status = created.cluster()["status"].clone();
        assert_ne!(
            status,
            serde_json::json!("ACTIVE"),
            "case {case:?}: the row's cluster must start out non-ACTIVE, got {status:?}"
        );

        let refused = start_refresh(&router, &name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a cluster with no live EKS control plane refuses the refresh, got \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );

        // And nothing was recorded for it: there is no refresh to report. A connected
        // cluster answers the gate here too — the describe is an operation of the same
        // EKS-managed plane — so only the non-connected row can read the 404 back.
        let described = describe_refresh(&router, &name).await;
        if *connected {
            assert_eq!(
                error_code(&described),
                "ClientException",
                "case {case:?}: the read is gated too, got {described:?}"
            );
        } else {
            assert_eq!(
                described.status, 404,
                "case {case:?}: a refused start records no refresh, got {described:?}"
            );
            assert_eq!(
                error_code(&described),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a refresh is addressed through its cluster, so an unknown cluster is
// reported as the missing cluster by both operations; a cluster that has never had
// one started has no refresh operation to report.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_cluster_and_an_unstarted_refresh_are_not_found() {
    // Rows differ along which operation is called and whether the cluster exists:
    // the missing half is the cluster for the first two rows and the refresh for
    // the last two.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("start, no such cluster", "POST", false),
        ("describe, no such cluster", "GET", false),
        ("describe, cluster without a refresh", "GET", true),
    ];

    for (index, (case, method, exists)) in cases.iter().enumerate() {
        let router = emulator();
        let name = format!("absent-{index}");
        if *exists {
            active_clusters(&router, &[name.as_str()]).await;
        }

        let answer = if *method == "POST" {
            start_refresh(&router, &name).await
        } else {
            describe_refresh(&router, &name).await
        };
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        if *exists {
            // The cluster is not the resource being reported, so it is not named in
            // the payload's identity members.
            assert!(
                answer.body.get("clusterName").is_none_or(serde_json::Value::is_null),
                "case {case:?}: a missing refresh names no cluster member, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.body["clusterName"],
                serde_json::json!(name),
                "case {case:?}: a missing cluster is named, got {answer:?}"
            );
            assert_eq!(
                answer.body["message"],
                serde_json::json!(cluster_not_found_message(&name)),
                "case {case:?}: expected the service's missing-cluster sentence, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a refresh belongs to exactly ONE cluster: starting one on a cluster says
// nothing about any other cluster's refresh.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refresh_belongs_to_one_cluster() {
    let router = emulator();
    // Rows: the cluster the refresh is started on, and two bystanders.
    let names = ["refreshed", "bystander-a", "bystander-b"];
    active_clusters(&router, &names).await;

    let started = start_refresh(&router, names[0]).await;
    assert_eq!(
        started.status, 200,
        "expected 200 from StartInsightsRefresh for {:?}, got {started:?}",
        names[0]
    );

    for name in &names[1..] {
        let described = describe_refresh(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: a cluster nobody refreshed reports no refresh, got {described:?}"
        );
        // And a start on the bystander is NOT refused as a conflict: the refresh
        // running on another cluster is not this cluster's.
        let own = start_refresh(&router, name).await;
        assert_eq!(
            own.status, 200,
            "case {name:?}: expected 200 starting this cluster's own refresh, got {own:?}"
        );
    }

    for name in &names {
        let described = describe_refresh(&router, name).await;
        // Each cluster's own refresh, in whichever of the two states the clock has
        // taken it to by now: this suite is about WHOSE refresh it is.
        started_refresh_instant(name, &described);
    }
}
