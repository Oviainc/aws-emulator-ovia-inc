// SPDX-License-Identifier: CC0-1.0

//! The filters a TASK LISTING selects by, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! Three rules, each asserted over a table that spans its class rather than over one
//! request:
//!
//! * the `desiredStatus` a listing filters on is resolved against the member's value set
//!   WITHOUT regard to ASCII case — Amazon ECS answers a listing that names a member in
//!   another case with the tasks of that desired status — and a value the set holds in no
//!   case is refused with the member's own sentence;
//! * a filter that identifies the tasks by itself is the only filter its listing may carry:
//!   `ListTasksRequest$startedBy` states it ("When you specify startedBy as the filter, it
//!   must be the only filter that you use") and `serviceName` is held to it too, while the
//!   `cluster` the listing is scoped to, the `desiredStatus` every listing filters on and
//!   the members that PAGE it are not filters this rule is about;
//! * a filter SELECTS: a `containerInstance` no instance of the cluster carries is matched
//!   by no task of it, so the listing is answered with the tasks that match — none — rather
//!   than refused, which is what separates a filter from the identifier an operation has to
//!   resolve to act on ([`crate::container_instance_not_found`] pins the other half).
//!
//! The value sets come from the model, read with `smithy_query(query="shape",
//! shape="DesiredStatus")` -> RUNNING, PENDING, STOPPED.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, register_instance,
    service, string,
};
use crate::task_lifecycle::container;

/// The cluster every listing below is scoped to.
const CLUSTER: &str = "listingfilters";

/// The family the running task belongs to.
const FAMILY: &str = "listingfamily";

/// The family the task that gets stopped belongs to, so a `family` filter tells the two
/// apart.
const STOPPED_FAMILY: &str = "listingstoppedfamily";

/// The `startedBy` value the launches carry, the exclusive filter's own subject.
const STARTED_BY: &str = "listing-suite";

/// The one error code a refused listing carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// What this suite's fixture holds: a cluster with capacity, one RUNNING task of
/// [`FAMILY`] and one STOPPED task of [`STOPPED_FAMILY`].
struct Fixture {
    router: MultiServiceRouter,
    running: String,
    stopped: String,
}

/// Register one `awsvpc` revision of `family` and run a task of it, returning the task ARN.
async fn run_one(router: &MultiServiceRouter, family: &str, case: &str) -> String {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
    let run = ok(
        router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "launchType": "EC2",
            "startedBy": STARTED_BY,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    let tasks = list(&run, "tasks", case);
    assert_eq!(tasks.len(), 1, "case {case:?}: expected one task, got {run}");
    string(&tasks[0], "taskArn", case)
}

/// One cluster with capacity, one task left RUNNING and one task asked to STOP — so every
/// value of the `DesiredStatus` set selects a known set of tasks.
async fn fixture() -> Fixture {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    let running = run_one(&router, FAMILY, "fixture").await;
    let stopped = run_one(&router, STOPPED_FAMILY, "fixture").await;
    ok(
        &router,
        "StopTask",
        json!({ "cluster": CLUSTER, "task": stopped.clone(), "reason": "the stopped row" }),
        "fixture",
    )
    .await;
    Fixture {
        router,
        running,
        stopped,
    }
}

/// The task ARNs a listing answers, sorted so a row's expectation is order-free.
async fn listed(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let body = ok(router, "ListTasks", request, case).await;
    let mut arns: Vec<String> = list(&body, "taskArns", case)
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    arns.sort();
    arns
}

/// Which of the fixture's tasks a row expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    Running,
    Stopped,
    None,
}

#[tokio::test]
async fn the_desired_status_filter_is_resolved_without_regard_to_case() {
    let Fixture {
        router,
        running,
        stopped,
    } = fixture().await;

    // The rows differ along the MEMBER named (every value of the set, plus the listing that
    // names none and takes the documented default) and along the CASE it is spelled in —
    // the set's own spelling, all lower case, mixed case, and a single flipped letter.
    let rows: [(&str, Option<&str>, Expected); 12] = [
        ("no filter at all takes the default RUNNING", None, Expected::Running),
        ("RUNNING as the set spells it", Some("RUNNING"), Expected::Running),
        ("running in lower case", Some("running"), Expected::Running),
        ("Running in mixed case", Some("Running"), Expected::Running),
        ("rUNNINg with its ends flipped", Some("rUNNINg"), Expected::Running),
        // The recorded case: a listing of the STOPPED tasks, spelled in lower case.
        ("stopped in lower case", Some("stopped"), Expected::Stopped),
        ("STOPPED as the set spells it", Some("STOPPED"), Expected::Stopped),
        ("Stopped in mixed case", Some("Stopped"), Expected::Stopped),
        ("sToPpEd in alternating case", Some("sToPpEd"), Expected::Stopped),
        // "Although you can filter results based on a desired status of PENDING, this
        // doesn't return any results. Amazon ECS never sets the desired status of a task to
        // that value."
        ("PENDING as the set spells it", Some("PENDING"), Expected::None),
        ("pending in lower case", Some("pending"), Expected::None),
        ("PeNdInG in alternating case", Some("PeNdInG"), Expected::None),
    ];

    for (case, desired_status, expected) in rows {
        let mut request = json!({ "cluster": CLUSTER });
        if let Some(desired_status) = desired_status {
            request["desiredStatus"] = json!(desired_status);
        }
        let answered = listed(&router, request, case).await;
        let want: Vec<String> = match expected {
            Expected::Running => vec![running.clone()],
            Expected::Stopped => vec![stopped.clone()],
            Expected::None => Vec::new(),
        };
        assert_eq!(
            answered, want,
            "case {case:?}: expected {expected:?} ({want:?}), got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_desired_status_the_set_holds_in_no_case_is_refused_naming_the_member_and_the_set() {
    let Fixture { router, .. } = fixture().await;

    // The rows differ along the FORM of the value outside the set: a status of the task
    // lifecycle that is not a DESIRED status, a truncation of a member, a member with
    // something appended, a value of another enum, an empty value and a padded one.
    let rows: [(&str, &str); 7] = [
        ("a lastStatus that is no desired status", "DEACTIVATING"),
        ("another lastStatus that is no desired status", "PROVISIONING"),
        ("a truncated member", "STOPPE"),
        ("a member with a suffix", "STOPPEDD"),
        ("a value of another enum", "FARGATE"),
        ("no value at all", ""),
        ("a member padded with spaces", " STOPPED "),
    ];

    for (case, desired_status) in rows {
        let (code, message) = refusal(
            &router,
            "ListTasks",
            json!({ "cluster": CLUSTER, "desiredStatus": desired_status }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                INVALID_PARAMETER,
                "desired status should be one of [PENDING,RUNNING,STOPPED]"
            ),
            "case {case:?}: expected the member and its value set, got {code:?} with \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn an_exclusive_filter_is_refused_beside_another_filter_and_served_on_its_own() {
    let Fixture { router, running, .. } = fixture().await;
    // A service to name, so a `serviceName` row is refused for the COMBINATION rather than
    // for a name that resolves to nothing.
    let service_name = "listingservice";
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        "fixture",
    )
    .await;

    // The rows differ along WHICH exclusive filter the listing carries, and what it carries
    // beside it: another filter (each of the three that narrow a listing, and the other
    // exclusive filter itself), or a member that is not one of them.
    let rows: [(&str, Value, Option<&str>); 10] = [
        (
            "startedBy on its own",
            json!({ "startedBy": STARTED_BY }),
            None,
        ),
        (
            "startedBy beside the family filter",
            json!({ "startedBy": STARTED_BY, "family": FAMILY }),
            Some("startedBy"),
        ),
        (
            "startedBy beside the launch type filter",
            json!({ "startedBy": STARTED_BY, "launchType": "EC2" }),
            Some("startedBy"),
        ),
        (
            "startedBy beside a container instance",
            json!({ "startedBy": STARTED_BY, "containerInstance": "0".repeat(32) }),
            Some("startedBy"),
        ),
        (
            // The recorded combination: a listing by service, narrowed by a family too.
            "serviceName beside the family filter",
            json!({ "serviceName": service_name, "family": FAMILY }),
            Some("serviceName"),
        ),
        (
            "serviceName beside the launch type filter",
            json!({ "serviceName": service_name, "launchType": "EC2" }),
            Some("serviceName"),
        ),
        (
            // Both exclusive filters at once: the sentence names the one the caller has to
            // drop first.
            "serviceName beside startedBy",
            json!({ "serviceName": service_name, "startedBy": STARTED_BY }),
            Some("serviceName"),
        ),
        (
            // `desiredStatus` is a filter EVERY listing carries ("The default status filter
            // is RUNNING"), so it is not another filter beside an exclusive one.
            "startedBy beside the desired status",
            json!({ "startedBy": STARTED_BY, "desiredStatus": "RUNNING" }),
            None,
        ),
        (
            // Paging a listing is not filtering it.
            "startedBy beside a page size",
            json!({ "startedBy": STARTED_BY, "maxResults": 10 }),
            None,
        ),
        (
            "the family filter beside the launch type filter",
            json!({ "family": FAMILY, "launchType": "EC2" }),
            None,
        ),
    ];

    for (case, filters, exclusive) in rows {
        let mut request = json!({ "cluster": CLUSTER });
        for (member, value) in filters.as_object().expect("a row's filters are an object") {
            request[member] = value.clone();
        }
        match exclusive {
            Some(member) => {
                let (code, message) = refusal(&router, "ListTasks", request, case).await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    (
                        INVALID_PARAMETER,
                        format!("cannot specify {member} with other arguments").as_str()
                    ),
                    "case {case:?}: expected the refusal to name {member:?}, got {code:?} \
                     with {message:?}"
                );
            }
            None => {
                let answered = listed(&router, request, case).await;
                assert!(
                    answered.contains(&running) || answered.is_empty(),
                    "case {case:?}: expected the listing to be answered, got {answered:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_container_instance_filter_matching_nothing_answers_an_empty_listing() {
    let Fixture { router, .. } = fixture().await;
    // A SECOND instance, registered into the cluster: a filter naming it matches nothing
    // either, because no task of this cluster was placed on it.
    let registered = register_instance(&router, CLUSTER, "i-0a0a0a0a0a0a0a0a0", "fixture").await;
    let registered_id = registered
        .rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
        .to_string();

    // The rows differ along whether the instance the filter names is REGISTERED in the
    // cluster at all, and along the form of the reference (an id, a full ARN) — the two
    // forms `containerInstance` accepts ("The container instance ID or full ARN").
    let rows: [(&str, String); 4] = [
        ("an id no instance of the account carries", "a".repeat(32)),
        (
            "an ARN no instance of the account carries",
            format!(
                "arn:aws:ecs:{}:{}:container-instance/{CLUSTER}/{}",
                crate::ecs_wire::SIGNED_REGION,
                crate::ecs_wire::ACCOUNT,
                "b".repeat(32)
            ),
        ),
        ("the id of an instance registered in the cluster", registered_id),
        ("the ARN of that instance", registered),
    ];

    for (case, reference) in rows {
        let answered = listed(
            &router,
            json!({ "cluster": CLUSTER, "containerInstance": reference }),
            case,
        )
        .await;
        assert!(
            answered.is_empty(),
            "case {case:?}: no task of this cluster sits on that instance, so the listing \
             is empty, got {answered:?}"
        );
    }
}
