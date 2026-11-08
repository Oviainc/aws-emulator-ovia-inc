// SPDX-License-Identifier: CC0-1.0

//! Deleting an ECS cluster, and reading one back, driven end to end over HTTP.
//!
//! The rules under test:
//!
//! 1. A delete TOMBSTONES the cluster. The record is kept with `status: INACTIVE` (the
//!    `Cluster$status` doc: "The cluster has been deleted. Clusters with an INACTIVE
//!    status may remain discoverable in your account for a period of time"), so every
//!    later reader describes the deleted cluster instead of a `MISSING` failure, a repeat
//!    delete is the no-op the operation's `@idempotent` trait promises, and the name is
//!    free for a new cluster.
//! 2. A delete is refused ONLY while the cluster still holds a resource the caller has
//!    not released: an `ACTIVE` service, or a task still wanted `RUNNING`. One already
//!    being drained away or asked to stop does not hold the cluster open.
//! 3. Each refusal carries the FIXED sentence for its condition, naming no resource from
//!    the request.
//! 4. A described cluster always carries `settings`, `statistics` and `tags` as lists —
//!    `include` decides what they CARRY, not whether they are there — and the clusters
//!    come back in the order the request named them.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    count, create_cluster, identity_document, list, ok, refusal, refused, register_capacity,
    register_definition, service, string, ACCOUNT, SIGNED_REGION,
};

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The recorded `ClusterContainsServicesException` sentence (`state`'s
/// `CLUSTER_CONTAINS_SERVICES_MESSAGE`).
const CONTAINS_SERVICES: &str = "The Cluster cannot be deleted while Services are active.";

/// The `ClusterContainsTasksException` sentence: the error shape's own doc sentence, the
/// constant the refusal carries (`state`'s `CLUSTER_CONTAINS_TASKS_MESSAGE`). No
/// recording fixes this one; what the rule fixes is that it is the same for every request
/// that meets the condition.
const CONTAINS_TASKS: &str = "You can't delete a cluster that has active tasks.";

/// Describe one cluster, asserting it is reported rather than reported missing.
async fn describe(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [reference] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected the cluster to be described, not missing, got {body}"
    );
    let clusters = list(&body, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected exactly one described cluster, got {body}"
    );
    clusters[0].clone()
}

/// What a row of the tombstone table put on the cluster before deleting it. Each variant
/// is a different way for a cluster to be legitimately EMPTY at delete time.
#[derive(Debug, Clone, Copy)]
enum Emptied {
    /// Nothing was ever created in it.
    Untouched,
    /// A service was created and then deleted.
    DeletedService,
    /// A task was run and then stopped.
    StoppedTask,
}

/// How a row names the cluster it deletes: "The short name or full Amazon Resource Name
/// (ARN) of the cluster to delete."
#[derive(Debug, Clone, Copy)]
enum Reference {
    Name,
    Arn,
}

#[tokio::test]
async fn deleting_a_cluster_leaves_an_inactive_tombstone_every_reader_sees() {
    let router = service();

    // Rows differ along what the cluster HELD when it was deleted, how it was configured
    // and which of the documented identifier forms the delete names it by.
    let rows: [(&str, &str, Emptied, Reference, Value); 5] = [
        (
            "an untouched cluster by name",
            "plain",
            Emptied::Untouched,
            Reference::Name,
            json!({}),
        ),
        (
            "a tagged, configured cluster by ARN",
            "tagged",
            Emptied::Untouched,
            Reference::Arn,
            json!({
                "tags": [{ "key": "env", "value": "test" }],
                "settings": [{ "name": "containerInsights", "value": "enabled" }],
            }),
        ),
        (
            "a cluster whose service was deleted",
            "hadservice",
            Emptied::DeletedService,
            Reference::Name,
            json!({}),
        ),
        (
            "a cluster whose task was stopped",
            "hadtask",
            Emptied::StoppedTask,
            Reference::Arn,
            json!({}),
        ),
        (
            "the documented default cluster",
            "default",
            Emptied::Untouched,
            Reference::Name,
            json!({}),
        ),
    ];

    for (case, name, emptied, reference, extra) in rows {
        let mut request = json!({ "clusterName": name });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            request[member] = value.clone();
        }
        let created = ok(&router, "CreateCluster", request, case).await;
        let arn = string(&created["cluster"], "clusterArn", case);
        assert_eq!(
            arn,
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}"),
            "case {case:?}: unexpected clusterArn in {created}"
        );

        match emptied {
            Emptied::Untouched => {}
            Emptied::DeletedService => {
                register_definition(&router, &format!("{name}family"), case).await;
                ok(
                    &router,
                    "CreateService",
                    json!({
                        "cluster": name,
                        "serviceName": "web",
                        "taskDefinition": format!("{name}family"),
                        "desiredCount": 1,
                        "launchType": "EC2",
                    }),
                    case,
                )
                .await;
                let deleted = ok(
                    &router,
                    "DeleteService",
                    json!({ "cluster": name, "service": "web", "force": true }),
                    case,
                )
                .await;
                assert_eq!(
                    count(&deleted["service"], "desiredCount", case),
                    0,
                    "case {case:?}: a deleted service wants no tasks, got {deleted}"
                );
            }
            Emptied::StoppedTask => {
                register_definition(&router, &format!("{name}family"), case).await;
                // A task placed with the EC2 launch type runs on an instance registered
                // to the cluster, so the cluster needs one before `RunTask` places it.
                // The delete that follows releases it with the cluster.
                register_capacity(&router, name, case).await;
                let run = ok(
                    &router,
                    "RunTask",
                    json!({ "cluster": name, "taskDefinition": format!("{name}family") }),
                    case,
                )
                .await;
                let task_arn = string(&list(&run, "tasks", case)[0], "taskArn", case);
                ok(
                    &router,
                    "StopTask",
                    json!({ "cluster": name, "task": task_arn }),
                    case,
                )
                .await;
            }
        }

        let named = match reference {
            Reference::Name => name.to_string(),
            Reference::Arn => arn.clone(),
        };
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": named }),
            case,
        )
        .await;
        let cluster = &deleted["cluster"];
        assert_eq!(
            string(cluster, "status", case),
            "INACTIVE",
            "case {case:?}: expected the delete to report INACTIVE, got {deleted}"
        );
        assert_eq!(
            string(cluster, "clusterArn", case),
            arn,
            "case {case:?}: expected the deleted cluster's own ARN, got {deleted}"
        );

        // The tombstone is what every later reader sees, with the counters settled at
        // zero: whatever it held went with it.
        let read = describe(&router, &named, case).await;
        assert_eq!(
            string(&read, "status", case),
            "INACTIVE",
            "case {case:?}: expected a later read to report INACTIVE, got {read}"
        );
        for counter in [
            "runningTasksCount",
            "pendingTasksCount",
            "activeServicesCount",
            "registeredContainerInstancesCount",
        ] {
            assert_eq!(
                count(&read, counter, case),
                0,
                "case {case:?}: expected {counter} 0 on a deleted cluster, got {read}"
            );
        }

        // `@idempotent`: deleting the tombstone is the same delete, not a refusal.
        let repeated = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": named }),
            case,
        )
        .await;
        assert_eq!(
            string(&repeated["cluster"], "status", case),
            "INACTIVE",
            "case {case:?}: expected a repeat delete to report the tombstone, got \
             {repeated}"
        );

        // The name is free again, and the new cluster is not the tombstone.
        let recreated = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": name, "tags": [{ "key": "round", "value": "two" }] }),
            case,
        )
        .await;
        assert_eq!(
            string(&recreated["cluster"], "status", case),
            "ACTIVE",
            "case {case:?}: expected a create over a tombstone to be ACTIVE, got \
             {recreated}"
        );
        assert_eq!(
            list(&recreated["cluster"], "tags", case),
            vec![json!({ "key": "round", "value": "two" })],
            "case {case:?}: expected the new cluster's own tags, got {recreated}"
        );
    }

    // A cluster nothing ever created is not a tombstone: that is still a refusal.
    refused(
        &router,
        "DeleteCluster",
        json!({ "cluster": "never-created-at-all" }),
        "ClusterNotFoundException",
        "a name that names nothing",
    )
    .await;
}

/// One row of the refusal table: the case name, the cluster it builds, what that cluster
/// holds and the refusal expected — the modelled error code and its fixed message — or
/// `None` when the delete must be accepted.
type RefusalRow = (
    &'static str,
    &'static str,
    Holding,
    Option<(&'static str, &'static str)>,
);

/// One row of the describe table: the case name, the identifiers the request names, the
/// cluster names expected back IN ORDER, and the names expected as `MISSING` failures.
type OrderRow = (
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
    Vec<&'static str>,
);

/// What a row of the refusal table leaves in the cluster before the delete.
#[derive(Debug, Clone, Copy)]
enum Holding {
    Nothing,
    /// A service the caller has not deleted.
    ActiveService,
    /// A service the caller deleted, whose tasks are still draining away.
    DeletedService,
    /// A task the caller still wants RUNNING.
    LiveTask,
    /// A task the caller asked to stop, which has not reached STOPPED yet.
    StoppingTask,
    /// A task that has reached STOPPED.
    StoppedTask,
}

#[tokio::test]
async fn a_cluster_delete_is_refused_only_while_it_holds_something_unreleased() {
    let router = service();

    // Rows span what the cluster holds: the two resources the exceptions name, each on
    // both sides of "the caller has released it", plus the empty cluster.
    let rows: [RefusalRow; 6] = [
        ("an empty cluster", "empty", Holding::Nothing, None),
        (
            "a service that was never deleted",
            "withservice",
            Holding::ActiveService,
            Some(("ClusterContainsServicesException", CONTAINS_SERVICES)),
        ),
        (
            "a service the caller deleted",
            "drainingservice",
            Holding::DeletedService,
            None,
        ),
        (
            "a task the caller still wants running",
            "withtask",
            Holding::LiveTask,
            Some(("ClusterContainsTasksException", CONTAINS_TASKS)),
        ),
        (
            "a task the caller asked to stop",
            "stoppingtask",
            Holding::StoppingTask,
            None,
        ),
        (
            "a task that has stopped",
            "stoppedtask",
            Holding::StoppedTask,
            None,
        ),
    ];

    for (case, name, holding, expected) in rows {
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": name }),
            case,
        )
        .await;
        match holding {
            Holding::Nothing => {}
            Holding::ActiveService | Holding::DeletedService => {
                register_definition(&router, &format!("{name}family"), case).await;
                // The EC2 launch type places a task on an instance registered to the
                // cluster, so the cluster needs one before the scheduler places the
                // service's task set.
                register_capacity(&router, name, case).await;
                ok(
                    &router,
                    "CreateService",
                    json!({
                        "cluster": name,
                        "serviceName": "web",
                        "taskDefinition": format!("{name}family"),
                        "desiredCount": 2,
                        "launchType": "EC2",
                    }),
                    case,
                )
                .await;
                if matches!(holding, Holding::DeletedService) {
                    let deleted = ok(
                        &router,
                        "DeleteService",
                        json!({ "cluster": name, "service": "web", "force": true }),
                        case,
                    )
                    .await;
                    assert_eq!(
                        string(&deleted["service"], "status", case),
                        "DRAINING",
                        "case {case:?}: a service with tasks to clean up drains first, \
                         got {deleted}"
                    );
                }
            }
            Holding::LiveTask | Holding::StoppingTask | Holding::StoppedTask => {
                register_definition(&router, &format!("{name}family"), case).await;
                // The EC2 launch type places the task on an instance registered to the
                // cluster, so the cluster needs one before `RunTask` places it.
                register_capacity(&router, name, case).await;
                let run = ok(
                    &router,
                    "RunTask",
                    json!({ "cluster": name, "taskDefinition": format!("{name}family") }),
                    case,
                )
                .await;
                let task_arn = string(&list(&run, "tasks", case)[0], "taskArn", case);
                if !matches!(holding, Holding::LiveTask) {
                    ok(
                        &router,
                        "StopTask",
                        json!({ "cluster": name, "task": task_arn }),
                        case,
                    )
                    .await;
                }
                if matches!(holding, Holding::StoppedTask) {
                    tokio::time::sleep(ONE_SPAN).await;
                }
            }
        }

        match expected {
            Some((code, message)) => {
                let (answered_code, answered_message) =
                    refusal(&router, "DeleteCluster", json!({ "cluster": name }), case).await;
                assert_eq!(
                    answered_code, code,
                    "case {case:?}: expected a {code}, got {answered_code:?} with \
                     {answered_message:?}"
                );
                assert_eq!(
                    answered_message, message,
                    "case {case:?}: expected the fixed sentence for this condition, got \
                     {answered_message:?}"
                );
                // The cluster is untouched by a refusal: it is still ACTIVE.
                let read = describe(&router, name, case).await;
                assert_eq!(
                    string(&read, "status", case),
                    "ACTIVE",
                    "case {case:?}: a refused delete must change nothing, got {read}"
                );
            }
            None => {
                let deleted = ok(
                    &router,
                    "DeleteCluster",
                    json!({ "cluster": name }),
                    case,
                )
                .await;
                assert_eq!(
                    string(&deleted["cluster"], "status", case),
                    "INACTIVE",
                    "case {case:?}: expected the delete to be accepted, got {deleted}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_delete_refusal_names_no_resource_from_the_request() {
    let router = service();

    // The sentence is a CONSTANT of the condition, so it must not vary with the cluster
    // or service names the request carries.
    let names = ["alpha-cluster", "b_2", "a-very-long-cluster-name-for-one-row"];
    for name in names {
        let case = format!("contains services in {name:?}");
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": name }),
            &case,
        )
        .await;
        register_definition(&router, "messagefamily", &case).await;
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": name,
                "serviceName": format!("svc-{name}"),
                "taskDefinition": "messagefamily",
                "desiredCount": 1,
                "launchType": "EC2",
            }),
            &case,
        )
        .await;
        let (code, message) =
            refusal(&router, "DeleteCluster", json!({ "cluster": name }), &case).await;
        assert_eq!(
            code, "ClusterContainsServicesException",
            "case {case:?}: expected the contains-services refusal, got {code:?}"
        );
        assert_eq!(
            message, CONTAINS_SERVICES,
            "case {case:?}: expected the fixed contains-services sentence, got \
             {message:?}"
        );
        assert!(
            !message.contains(name),
            "case {case:?}: the sentence must name no resource, got {message:?}"
        );

        let missing_case = format!("missing cluster beside {name:?}");
        let (code, message) = refusal(
            &router,
            "DeleteCluster",
            json!({ "cluster": format!("{name}-does-not-exist") }),
            &missing_case,
        )
        .await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {missing_case:?}: expected the not-found refusal, got {code:?}"
        );
        assert_eq!(
            message, "Cluster not found.",
            "case {missing_case:?}: expected the fixed not-found sentence, got \
             {message:?}"
        );
        assert!(
            !message.contains(name),
            "case {missing_case:?}: the sentence must name no resource, got {message:?}"
        );
    }
}

#[tokio::test]
async fn described_clusters_follow_the_requested_order_and_always_carry_their_lists() {
    let router = service();

    let stored = ["one", "two", "three"];
    for name in stored {
        ok(
            &router,
            "CreateCluster",
            json!({
                "clusterName": name,
                "tags": [{ "key": "name", "value": name }],
                "settings": [{ "name": "containerInsights", "value": "disabled" }],
            }),
            "setup",
        )
        .await;
    }
    // One deleted cluster, so the order rule is exercised over a tombstone too.
    ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "three" }),
        "setup",
    )
    .await;

    // Rows differ in the ORDER, the COUNT and the repeats of the identifiers requested,
    // and in whether an id that names nothing is mixed in. A name is reported once
    // however many times it is named, in the position it was first named.
    let rows: [OrderRow; 6] = [
        ("stored order", vec!["one", "two"], vec!["one", "two"], vec![]),
        (
            "reversed",
            vec!["two", "one"],
            vec!["two", "one"],
            vec![],
        ),
        (
            "all three, rotated",
            vec!["three", "one", "two"],
            vec!["three", "one", "two"],
            vec![],
        ),
        (
            "a repeat keeps its first position",
            vec!["two", "one", "two"],
            vec!["two", "one"],
            vec![],
        ),
        (
            "an unknown id between two stored ones",
            vec!["two", "nosuchcluster", "one"],
            vec!["two", "one"],
            vec!["nosuchcluster"],
        ),
        ("one name only", vec!["three"], vec!["three"], vec![]),
    ];

    for (case, requested, expected, missing) in rows {
        let body = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": requested }),
            case,
        )
        .await;
        let described: Vec<String> = list(&body, "clusters", case)
            .iter()
            .map(|cluster| string(cluster, "clusterName", case))
            .collect();
        assert_eq!(
            described, expected,
            "case {case:?}: expected the requested order {expected:?}, got {body}"
        );
        let failed: Vec<String> = list(&body, "failures", case)
            .iter()
            .map(|failure| string(failure, "arn", case))
            .collect();
        let expected_failures: Vec<String> = missing
            .iter()
            .map(|name| format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}"))
            .collect();
        assert_eq!(
            failed, expected_failures,
            "case {case:?}: expected {expected_failures:?} reported missing, got {body}"
        );

        // Without `include` the three list members are present and EMPTY: "absent" is
        // not how AWS reports "you did not ask for it".
        for cluster in list(&body, "clusters", case) {
            for member in ["settings", "statistics", "tags"] {
                assert_eq!(
                    list(&cluster, member, case),
                    Vec::<Value>::new(),
                    "case {case:?}: expected an empty {member} without include, got \
                     {cluster}"
                );
            }
        }
    }

    // With `include`, the same members carry what is stored. The `ClusterField` enum is
    // ATTACHMENTS | CONFIGURATIONS | SETTINGS | STATISTICS | TAGS.
    let case = "include SETTINGS, STATISTICS and TAGS";
    let body = ok(
        &router,
        "DescribeClusters",
        json!({
            "clusters": ["two", "three"],
            "include": ["SETTINGS", "STATISTICS", "TAGS"],
        }),
        case,
    )
    .await;
    for cluster in list(&body, "clusters", case) {
        let name = string(&cluster, "clusterName", case);
        assert_eq!(
            list(&cluster, "settings", case),
            vec![json!({ "name": "containerInsights", "value": "disabled" })],
            "case {case:?}: expected the stored settings of {name:?}, got {cluster}"
        );
        assert_eq!(
            list(&cluster, "tags", case),
            vec![json!({ "key": "name", "value": name })],
            "case {case:?}: expected the stored tags of {name:?}, got {cluster}"
        );
        assert_eq!(
            list(&cluster, "statistics", case).len(),
            16,
            "case {case:?}: expected the launch-type statistics of {name:?}, got \
             {cluster}"
        );
    }
}

#[tokio::test]
async fn deleting_a_service_drains_its_tasks_and_settles_on_the_clock() {
    let router = service();

    // Rows span the desiredCount the service was left at and whether the caller forced
    // the delete: "You can delete a service if you have no running tasks in it and the
    // desired task count is zero", and `force` is what lifts that for a replica service.
    // The delete that goes through leaves the service DRAINING whatever it was holding:
    // the move to INACTIVE is owed by the clock, not by the call that starts the drain.
    let rows: [(&str, i64, bool); 4] = [
        ("scaled to zero, no force", 0, false),
        ("scaled up, forced", 2, true),
        ("scaled to one, forced", 1, true),
        ("scaled up, no force", 3, false),
    ];

    for (case, desired, force) in rows {
        let cluster = format!("svc{}", desired * 2 + i64::from(force));
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": cluster }),
            case,
        )
        .await;
        register_definition(&router, &format!("{cluster}family"), case).await;
        // The EC2 launch type places a task on an instance registered to the cluster, so
        // the cluster needs one before the scheduler places the service's task set.
        register_capacity(&router, &cluster, case).await;
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": cluster,
                "serviceName": "web",
                "taskDefinition": format!("{cluster}family"),
                "desiredCount": desired,
                "launchType": "EC2",
            }),
            case,
        )
        .await;

        let mut request = json!({ "cluster": cluster, "service": "web" });
        if force {
            request["force"] = json!(true);
        }
        if desired > 0 && !force {
            // A service that is still scaled up is refused, with the fixed sentence for
            // that condition.
            let (code, message) = refusal(&router, "DeleteService", request.clone(), case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the scaled-above-zero refusal, got {code:?}"
            );
            assert_eq!(
                message, "The service cannot be stopped while it is scaled above 0.",
                "case {case:?}: expected the fixed scaled-above-zero sentence, got \
                 {message:?}"
            );
            // Nothing changed: the service is still ACTIVE and still blocks its cluster.
            refused(
                &router,
                "DeleteCluster",
                json!({ "cluster": cluster }),
                "ClusterContainsServicesException",
                case,
            )
            .await;
            continue;
        }

        let deleted = ok(&router, "DeleteService", request, case).await;
        let deleted_service = &deleted["service"];
        assert_eq!(
            count(deleted_service, "desiredCount", case),
            0,
            "case {case:?}: a deleted service wants no tasks, got {deleted}"
        );
        assert_eq!(
            string(deleted_service, "status", case),
            "DRAINING",
            "case {case:?}: expected the delete to leave the service draining, got \
             {deleted}"
        );

        // Read it back through DescribeServices: the record is kept, not dropped.
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": ["web"] }),
            case,
        )
        .await;
        assert!(
            list(&read, "failures", case).is_empty(),
            "case {case:?}: a deleted service is described, not MISSING, got {read}"
        );
        let stored = &list(&read, "services", case)[0];
        assert_eq!(
            count(stored, "desiredCount", case),
            0,
            "case {case:?}: expected the stored desiredCount to be 0, got {read}"
        );

        // Its cluster is released at once: a draining service is not an ACTIVE one.
        let gone = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": cluster }),
            case,
        )
        .await;
        assert_eq!(
            string(&gone["cluster"], "status", case),
            "INACTIVE",
            "case {case:?}: expected the cluster of a deleted service to delete, got \
             {gone}"
        );
    }

    // The DRAINING -> INACTIVE move is owed by the CLOCK, not by whoever reads next:
    // "After all tasks have transitioned to either STOPPING or STOPPED status, the
    // service status moves from DRAINING to INACTIVE."
    let case = "draining settles on the clock";
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "settling" }),
        case,
    )
    .await;
    register_definition(&router, "settlingfamily", case).await;
    register_capacity(&router, "settling", case).await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": "settling",
            "serviceName": "web",
            "taskDefinition": "settlingfamily",
            "desiredCount": 2,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let deleted = ok(
        &router,
        "DeleteService",
        json!({ "cluster": "settling", "service": "web", "force": true }),
        case,
    )
    .await;
    assert_eq!(
        string(&deleted["service"], "status", case),
        "DRAINING",
        "case {case:?}: a caller acting straight after the delete sees DRAINING, got \
         {deleted}"
    );
    tokio::time::sleep(ONE_SPAN).await;
    let settled = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": "settling", "services": ["web"] }),
        case,
    )
    .await;
    let stored = &list(&settled, "services", case)[0];
    assert_eq!(
        string(stored, "status", case),
        "INACTIVE",
        "case {case:?}: expected the owed move to INACTIVE to have fallen due, got \
         {settled}"
    );
    let tasks = ok(
        &router,
        "ListTasks",
        json!({ "cluster": "settling", "desiredStatus": "STOPPED" }),
        case,
    )
    .await;
    assert_eq!(
        list(&tasks, "taskArns", case).len(),
        2,
        "case {case:?}: expected the drained tasks to have stopped, got {tasks}"
    );

    // Deleting the deleted service again is the same delete (`@idempotent`).
    let repeated = ok(
        &router,
        "DeleteService",
        json!({ "cluster": "settling", "service": "web" }),
        case,
    )
    .await;
    assert_eq!(
        string(&repeated["service"], "status", case),
        "INACTIVE",
        "case {case:?}: expected a repeat delete to report the tombstone, got {repeated}"
    );

    // A service that was never created, and a cluster that was never created, are
    // refused with their own fixed sentences.
    let (code, message) = refusal(
        &router,
        "DeleteService",
        json!({ "cluster": "settling", "service": "nosuchservice" }),
        case,
    )
    .await;
    assert_eq!(
        code, "ServiceNotFoundException",
        "case {case:?}: expected the service-not-found refusal, got {code:?}"
    );
    assert_eq!(
        message, "",
        "case {case:?}: expected `DeleteService` to carry no sentence at all, got \
         {message:?}"
    );
    let (code, message) = refusal(
        &router,
        "DeleteService",
        json!({ "cluster": "nosuchcluster", "service": "web" }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClusterNotFoundException",
        "case {case:?}: expected the cluster-not-found refusal, got {code:?}"
    );
    assert_eq!(
        message, "Cluster not found.",
        "case {case:?}: expected the fixed cluster-not-found sentence, got {message:?}"
    );
}

/// One row of the tombstone-is-not-a-work-target table: the operation and the request a
/// row builds for it against the deleted cluster.
type TombstoneRow = (&'static str, fn(&str) -> Value);

#[tokio::test]
async fn a_tombstoned_cluster_is_no_longer_a_cluster_an_operation_can_work_in() {
    let router = service();
    create_cluster(&router, "gone", "setup").await;
    register_definition(&router, "gonefamily", "setup").await;
    ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "gone" }),
        "setup",
    )
    .await;

    // "The cluster has been deleted": the tombstone is discoverable, but it is not a
    // cluster anything can run, register or write resources in — every one of those
    // operations refuses it with the same error code it answers for a name that never
    // existed, and with the sentence that says the cluster is STILL there and inactive,
    // which is what tells the two states apart. Rows: every implemented operation that
    // resolves a `cluster` to WORK IN. The describes that answer per NAME report the
    // tombstone's names as `MISSING` failures instead — their own table below.
    let rows: [TombstoneRow; 7] = [
        ("RunTask", |cluster| {
            json!({ "cluster": cluster, "taskDefinition": "gonefamily" })
        }),
        ("StopTask", |cluster| {
            json!({ "cluster": cluster, "task": "a".repeat(32) })
        }),
        ("ListTasks", |cluster| json!({ "cluster": cluster })),
        ("CreateService", |cluster| {
            json!({
                "cluster": cluster,
                "serviceName": "web",
                "taskDefinition": "gonefamily",
                "desiredCount": 1,
            })
        }),
        ("UpdateService", |cluster| {
            json!({ "cluster": cluster, "service": "web" })
        }),
        ("DeleteService", |cluster| {
            json!({ "cluster": cluster, "service": "web" })
        }),
        ("ListContainerInstances", |cluster| {
            json!({ "cluster": cluster })
        }),
    ];
    for (operation, build) in rows {
        let case = format!("{operation} against a deleted cluster");
        let (code, message) = refusal(&router, operation, build("gone"), &case).await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {case:?}: expected a deleted cluster to be not found, got {code:?} \
             with {message:?}"
        );
        assert_eq!(
            message, "The referenced cluster was inactive.",
            "case {case:?}: expected the fixed inactive-cluster sentence, got {message:?}"
        );

        // The SAME request against a name this account never held keeps the other fixed
        // sentence: the selector is the cluster's own state, not the request.
        let missing = format!("{operation} against a cluster that never existed");
        let (code, message) = refusal(&router, operation, build("nosuchcluster"), &missing).await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {missing:?}: expected a missing cluster to be not found, got {code:?} \
             with {message:?}"
        );
        assert_eq!(
            message, "Cluster not found.",
            "case {missing:?}: expected the fixed not-found sentence, got {message:?}"
        );
    }

    // The describes that answer per NAME are the other half of the rule: the tombstone is
    // a cluster record this account still holds, so the call itself answers and every name
    // it carries comes back as a `MISSING` failure — the resources the cluster held went
    // with the delete. Rows: each implemented describe whose output models `failures`, and
    // the member that carries the names it reports on.
    for (operation, request, reported) in [
        (
            "DescribeServices",
            json!({ "cluster": "gone", "services": ["web"] }),
            "services",
        ),
        (
            "DescribeTasks",
            json!({ "cluster": "gone", "tasks": ["a".repeat(32)] }),
            "tasks",
        ),
        (
            "DescribeContainerInstances",
            json!({ "cluster": "gone", "containerInstances": ["a".repeat(32)] }),
            "containerInstances",
        ),
    ] {
        let case = format!("{operation} against a deleted cluster");
        let body = ok(&router, operation, request, &case).await;
        assert!(
            list(&body, reported, &case).is_empty(),
            "case {case:?}: expected the tombstone to hold no {reported}, got {body}"
        );
        let failures = list(&body, "failures", &case);
        assert_eq!(
            failures.len(),
            1,
            "case {case:?}: expected one failure for the one name asked about, got {body}"
        );
        assert_eq!(
            string(&failures[0], "reason", &case),
            "MISSING",
            "case {case:?}: expected a MISSING failure, got {body}"
        );
    }

    // `RegisterContainerInstance` reports the same two conditions under the class IT
    // models for them — the agent plane's `ClientException`, kept apart from the
    // `InvalidParameterException` it answers a request it cannot read — and the SENTENCE
    // is still selected by the cluster's own stored state.
    for (reference, expected) in [
        ("gone", "The referenced cluster was inactive."),
        ("nosuchcluster", "Cluster not found."),
    ] {
        let case = format!("RegisterContainerInstance against cluster {reference:?}");
        let (code, message) = refusal(
            &router,
            "RegisterContainerInstance",
            json!({
                "cluster": reference,
                "instanceIdentityDocument": identity_document("i-0a1b2c3d4e5f60718"),
                "instanceIdentityDocumentSignature":
                    crate::ecs_wire::identity_document_signature(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected the agent-plane class, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Nothing leaked into the tombstone: it is still reported with zero counters, and
    // `DeleteCluster` still answers idempotently for it.
    let read = describe(&router, "gone", "after").await;
    for counter in ["runningTasksCount", "pendingTasksCount", "activeServicesCount"] {
        assert_eq!(
            count(&read, counter, "after"),
            0,
            "case \"after\": expected {counter} 0 on the tombstone, got {read}"
        );
    }
    let repeated = ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "gone" }),
        "after",
    )
    .await;
    assert_eq!(
        string(&repeated["cluster"], "status", "after"),
        "INACTIVE",
        "case \"after\": expected the tombstone back, got {repeated}"
    );
}
