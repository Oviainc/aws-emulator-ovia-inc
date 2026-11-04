// SPDX-License-Identifier: CC0-1.0

//! What a SERVICE's deployment reports about the rollout the CLOCK owes it, AS THE
//! CLIENT'S OWN CALL SEQUENCE RUNS ON — end to end over the wire through the shared
//! harness in [`crate::ecs_wire`].
//!
//! The rules under test, each asserted over a table rather than over one recorded call:
//!
//! 1. For the rollout a `CreateService` starts, every member that reports whether it has
//!    taken the service over moves TOGETHER: while it is under way the deployment reports
//!    `rolloutState` `IN_PROGRESS` and a share of the service's desired count of
//!    `MIN_SCALE_PERCENT`, and the service deployment that records it is `IN_PROGRESS`;
//!    once the clock has settled it, the three are `COMPLETED`, the whole of the service
//!    (`MAX_SCALE_PERCENT`) and `SUCCESSFUL`. `Scale` is "a floating-point percentage of
//!    your desired number of tasks"
//!    (`smithy_query(query="doc",shape="com.amazonaws.ecs#Scale")`), so the share is
//!    always reported in `PERCENT` and never in anything else. A reader that reports one
//!    of the three settled and another still under way contradicts itself.
//! 2. Only the CLOCK settles it. Every call the client makes before it looks — describing
//!    the service, describing the cluster, listing the rollouts, and a `DeleteCluster`
//!    the cluster's own services REFUSE — leaves the deployment exactly where the clock
//!    had it, so the rows differ along HOW MANY rounds of calls the client makes first,
//!    and the phase a read reports never goes back to under-way once it has settled.
//! 3. The rule holds whatever the rollout has to place, zero tasks included: the share is
//!    read off whether the rollout took the service over, not off how many tasks it holds.
//!
//! Nothing here reaches into the state struct or nudges a stored due instant: these rules
//! are about the real clock and the wire, so every claim is read back through the
//! operations a client has.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, refused, register_definition, service, string,
};
use service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::{
    MAX_SCALE_PERCENT, MIN_SCALE_PERCENT, ROLLOUT_COMPLETED, ROLLOUT_IN_PROGRESS,
    SCALE_UNIT_PERCENT, SERVICE_DEPLOYMENT_IN_PROGRESS, SERVICE_DEPLOYMENT_SUCCESSFUL,
    TRANSITION_SPAN,
};

/// Long enough for the rollout the clock owes to fall due: the deployment reaches its
/// steady state only once the tasks it placed are running, which is one span of its own,
/// plus margin for a loaded test machine. Derived from the span the service itself holds,
/// so the two cannot drift.
fn past_the_rollout() -> Duration {
    2 * TRANSITION_SPAN + Duration::from_millis(400)
}

/// Which side of the clock-owed steady state a rollout is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The rollout is still under way, so it holds none of the service yet.
    UnderWay,
    /// The clock has settled it and it holds the service.
    TakenOver,
}

/// One row: what it exercises, the service's `desiredCount` and how many rounds of its
/// own calls the client makes before it reads the deployment.
struct Row {
    case: &'static str,
    desired: i64,
    rounds: usize,
}

/// Rows differ along what the rules generalise over: how many tasks the rollout has to
/// place (none, one, several) and how many calls the client makes before it looks (none,
/// one, a handful, a long run).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a service of no tasks, read straight back",
            desired: 0,
            rounds: 0,
        },
        Row {
            case: "a service of one task, after one round of client calls",
            desired: 1,
            rounds: 1,
        },
        Row {
            case: "a service of three tasks, after three rounds of client calls",
            desired: 3,
            rounds: 3,
        },
        Row {
            case: "a service of two tasks, after eight rounds of client calls",
            desired: 2,
            rounds: 8,
        },
    ]
}

/// Describe one service and return it.
async fn describe(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service_name] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected {service_name} to be described, got {body}"
    );
    list(&body, "services", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected one described service, got {body}"))
}

/// The service's current (`PRIMARY`) deployment, which is the first one reported.
fn primary(service_view: &Value, case: &str) -> Value {
    list(service_view, "deployments", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a deployment, got {service_view}"))
}

/// The share of the service's desired count a deployment reports, in percent.
///
/// `Scale$value` is a `Double` whose modelled default is `0`, which an `awsJson1_1` body
/// may leave out — but only a deployment that reports a `Scale` at all has a share, so an
/// absent block reads as `None` rather than as zero: an assertion of a zero share cannot
/// pass against a projection that wrote no scale.
fn share(deployment: &Value, case: &str) -> Option<f64> {
    let scale = &deployment["scale"];
    if scale.is_null() {
        return None;
    }
    assert_eq!(
        scale["unit"].as_str(),
        Some(SCALE_UNIT_PERCENT),
        "case {case:?}: a share of a service is measured in {SCALE_UNIT_PERCENT}, got \
         {deployment}"
    );
    match &scale["value"] {
        Value::Null => Some(MIN_SCALE_PERCENT),
        value => Some(value.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a numeric scale.value, got {deployment}")
        })),
    }
}

/// Every service deployment of one service, newest first.
async fn rollouts(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "ListServiceDeployments",
        json!({ "cluster": cluster, "service": service_name }),
        case,
    )
    .await;
    list(&body, "serviceDeployments", case)
}

/// RULE 1, applied to one reported service: the phase its rollout is in, asserting that
/// every member that reports it agrees about it.
fn phase_of(case: &str, label: &str, service_view: &Value, rollout: &Value) -> Phase {
    let deployment = primary(service_view, case);
    let rollout_state = string(&deployment, "rolloutState", case);
    let recorded_status = string(rollout, "status", case);
    let share = share(&deployment, case).unwrap_or_else(|| {
        panic!("case {case:?} ({label}): a deployment reports the share of the service it holds, got {deployment}")
    });

    let by_rollout_state = if rollout_state == ROLLOUT_IN_PROGRESS {
        Phase::UnderWay
    } else {
        assert_eq!(
            rollout_state, ROLLOUT_COMPLETED,
            "case {case:?} ({label}): expected {ROLLOUT_IN_PROGRESS} or \
             {ROLLOUT_COMPLETED}, got {deployment}"
        );
        Phase::TakenOver
    };
    let by_share = if share == MIN_SCALE_PERCENT {
        Phase::UnderWay
    } else {
        assert_eq!(
            share, MAX_SCALE_PERCENT,
            "case {case:?} ({label}): a rollout holds either none of the service or all \
             of it, got {deployment}"
        );
        Phase::TakenOver
    };
    let by_record = if recorded_status == SERVICE_DEPLOYMENT_IN_PROGRESS {
        Phase::UnderWay
    } else {
        assert_eq!(
            recorded_status, SERVICE_DEPLOYMENT_SUCCESSFUL,
            "case {case:?} ({label}): expected {SERVICE_DEPLOYMENT_IN_PROGRESS} or \
             {SERVICE_DEPLOYMENT_SUCCESSFUL}, got {rollout}"
        );
        Phase::TakenOver
    };

    assert_eq!(
        (by_share, by_record),
        (by_rollout_state, by_rollout_state),
        "case {case:?} ({label}): the share a deployment holds, the rollout state it \
         reports and the status of the service deployment that records it are one fact, \
         got {deployment} and {rollout}"
    );
    by_rollout_state
}

/// Read the deployment and the record of the same rollout, and assert they agree.
async fn phase(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    case: &str,
    label: &str,
) -> Phase {
    let service_view = describe(router, cluster, service_name, case).await;
    let recorded = rollouts(router, cluster, service_name, case).await;
    let current = string(&service_view, "currentServiceDeployment", case);
    let rollout = recorded
        .iter()
        .find(|entry| string(entry, "serviceDeploymentArn", case) == current)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?} ({label}): the rollout the service reports as current must \
                 be one the history holds, got {current:?} and {recorded:?}"
            )
        });
    phase_of(case, label, &service_view, &rollout)
}

#[tokio::test]
async fn a_rollouts_reported_share_follows_the_clock_and_no_call_of_the_clients_moves_it() {
    for Row {
        case,
        desired,
        rounds,
    } in rows()
    {
        // A store per row, so one row's service cannot explain another's.
        let router = service();
        let cluster = "rollouts";
        cluster_with_capacity(&router, cluster, case).await;
        let definition_arn = register_definition(&router, "rolloutfamily", case).await;
        let service_name = "web";

        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": service_name,
                "taskDefinition": definition_arn,
                "desiredCount": desired,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
        let created = created["service"].clone();
        assert_eq!(
            share(&primary(&created, case), case),
            Some(MIN_SCALE_PERCENT),
            "case {case:?}: the create's own answer reports a rollout that has taken \
             nothing over yet, got {created}"
        );

        // The create answered an instant ago, so the rollout is still under way: this is
        // the state a client that reads straight back observes.
        assert_eq!(
            phase(&router, cluster, service_name, case, "the first read").await,
            Phase::UnderWay,
            "case {case:?}: a rollout read straight after the create is still under way"
        );

        // The client's own calls come next. None of them settles the rollout — not even
        // the one the service refuses — so the phase a read reports never goes back.
        let mut seen = Phase::UnderWay;
        for round in 0..rounds {
            let label = &format!("round {round}");
            describe(&router, cluster, service_name, case).await;
            ok(
                &router,
                "DescribeClusters",
                json!({ "clusters": [cluster] }),
                case,
            )
            .await;
            // "You can't delete a cluster that contains services": a refused call writes
            // nothing, so it moves no rollout either.
            refused(
                &router,
                "DeleteCluster",
                json!({ "cluster": cluster }),
                "ClusterContainsServicesException",
                case,
            )
            .await;
            let now = phase(&router, cluster, service_name, case, label).await;
            assert!(
                !(seen == Phase::TakenOver && now == Phase::UnderWay),
                "case {case:?} ({label}): a rollout the clock settled never goes back to \
                 under way"
            );
            seen = now;
        }

        // Past the span the clock owed the steady state in: every member that reports the
        // rollout has moved with it.
        tokio::time::sleep(past_the_rollout()).await;
        assert_eq!(
            phase(&router, cluster, service_name, case, "past the span").await,
            Phase::TakenOver,
            "case {case:?}: past the span the clock owed, the rollout holds the service"
        );

        // And the settled rollout stays settled: reading it again reports the same share.
        let settled = describe(&router, cluster, service_name, case).await;
        assert_eq!(
            share(&primary(&settled, case), case),
            Some(MAX_SCALE_PERCENT),
            "case {case:?}: a rollout that took the service over holds all of its desired \
             count, got {settled}"
        );
    }
}
