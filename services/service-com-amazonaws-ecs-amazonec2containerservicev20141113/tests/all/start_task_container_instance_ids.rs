// SPDX-License-Identifier: CC0-1.0

//! The container-instance IDENTIFIERS a `StartTask` may direct a placement at.
//!
//! One rule, asserted over a table of inputs rather than one case: a container instance is
//! identified by a UUID, written either as its 32 hexadecimal digits or as the 36
//! characters of the hyphenated form, so a `containerInstances` entry whose id part is of
//! neither length names no container instance of any account and the whole call is refused
//! with `InvalidParameterException` BEFORE anything is placed — while an entry of either
//! length is accepted and resolved against the cluster's capacity, which answers with a
//! task or with a `MISSING` failure for that entry.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{create_cluster, list, ok, refusal, register_definition, string};

/// The family every case of this suite places.
const FAMILY: &str = "lengthchecked";

/// The sentence a `containerInstances` entry of neither accepted length earns. Fixed text:
/// it names the two lengths an Amazon ECS container-instance id has, never the value the
/// request sent.
const LENGTH_REFUSAL: &str = "instanceId length should be one of [32,36]";

/// A router with one revision of [`FAMILY`] registered.
async fn router_with_definition() -> MultiServiceRouter {
    let router = crate::ecs_wire::service();
    register_definition(&router, FAMILY, "setup").await;
    router
}

/// The container-instance id of a container-instance ARN: its last segment, the form a
/// caller may name instead of the whole ARN.
fn instance_id(instance_arn: &str) -> String {
    instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// An ARN shaped like a container instance's, carrying `id` as its id segment — the form a
/// caller builds (or mis-slices) by hand.
fn instance_arn_with_id(cluster: &str, id: &str) -> String {
    format!(
        "arn:aws:ecs:{}:{}:container-instance/{cluster}/{id}",
        crate::ecs_wire::SIGNED_REGION,
        crate::ecs_wire::ACCOUNT,
    )
}

#[tokio::test]
async fn start_task_refuses_a_container_instance_id_of_neither_accepted_length() {
    let router = router_with_definition().await;
    let case = "setup";
    let cluster = "idlengths";
    create_cluster(&router, cluster, case).await;
    let host = crate::ecs_wire::register_instance(&router, cluster, "i-0len0len0len0len0", case)
        .await;
    let placeable = instance_id(&host);

    // The rows walk the LENGTH of the id an entry carries: both sides of each accepted
    // length, the gap between them, the empty entry, an id given inside an ARN, and a
    // malformed entry standing beside a perfectly placeable one.
    let rows: [(&str, Vec<String>); 7] = [
        ("one character short of the hex form", vec!["a".repeat(31)]),
        ("one character past the hex form", vec!["a".repeat(33)]),
        (
            "between the hex form and the hyphenated one",
            vec!["b".repeat(34)],
        ),
        (
            "one character past the hyphenated form",
            vec!["c".repeat(37)],
        ),
        ("an empty entry", vec![String::new()]),
        (
            "an ARN whose id segment was truncated",
            vec![instance_arn_with_id(cluster, &placeable[..18])],
        ),
        (
            "a malformed entry beside a placeable instance",
            vec![placeable.clone(), "d".repeat(8)],
        ),
    ];

    for (case, references) in rows {
        let (code, message) = refusal(
            &router,
            "StartTask",
            json!({
                "cluster": cluster,
                "taskDefinition": FAMILY,
                "containerInstances": references,
            }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", LENGTH_REFUSAL),
            "case {case:?}: expected the id-length refusal, got {code:?} with {message:?}"
        );
        // The refusal comes BEFORE the placement, so not even the entry that names
        // placeable capacity got a task.
        let listed = ok(&router, "ListTasks", json!({ "cluster": cluster }), case).await;
        assert!(
            list(&listed, "taskArns", case).is_empty(),
            "case {case:?}: a refused StartTask must place no task, got {listed}"
        );
    }
}

#[tokio::test]
async fn start_task_accepts_a_container_instance_id_of_either_accepted_length() {
    let router = router_with_definition().await;
    let case = "setup";
    let cluster = "idlengthsaccepted";
    create_cluster(&router, cluster, case).await;
    let host =
        crate::ecs_wire::register_instance(&router, cluster, "i-0ok00ok00ok00ok00", case).await;
    let placeable = instance_id(&host);
    assert_eq!(
        placeable.chars().count(),
        32,
        "setup: a registered instance's id is the 32-digit form, got {placeable:?}"
    );

    // The rows differ along the form AND the length of an accepted entry: the id of the
    // registered instance, its full ARN, and the 36-character hyphenated form — which
    // passes validation and is then resolved like any other, so the instance nothing
    // registered under it is reported through `failures` instead of refusing the call.
    let rows: [(&str, String, usize, usize); 3] = [
        ("the registered instance by its 32-digit id", placeable.clone(), 1, 0),
        ("the registered instance by its full ARN", host.clone(), 1, 0),
        (
            "a 36-character hyphenated id nothing registered",
            "0123abcd-0123-0123-0123-0123456789ab".to_string(),
            0,
            1,
        ),
    ];

    for (case, reference, placed, failed) in rows {
        let body = ok(
            &router,
            "StartTask",
            json!({
                "cluster": cluster,
                "taskDefinition": FAMILY,
                "containerInstances": [reference.clone()],
            }),
            case,
        )
        .await;
        let tasks = list(&body, "tasks", case);
        assert_eq!(
            tasks.len(),
            placed,
            "case {case:?}: expected {placed} task(s) placed, got {body}"
        );
        let failures = list(&body, "failures", case);
        assert_eq!(
            failures.len(),
            failed,
            "case {case:?}: expected {failed} failure(s), got {body}"
        );
        for failure in &failures {
            assert_eq!(
                string(failure, "reason", case),
                "MISSING",
                "case {case:?}: an id that names no registered instance is MISSING, got \
                 {failure}"
            );
        }

        // Each placed task is STORED on the instance the entry named: the read-back, not
        // the response of the call that placed it.
        for task in &tasks {
            let task_arn = string(task, "taskArn", case);
            let read = ok(
                &router,
                "DescribeTasks",
                json!({ "cluster": cluster, "tasks": [task_arn.clone()] }),
                case,
            )
            .await;
            let stored = list(&read, "tasks", case)
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {read}"));
            assert_eq!(
                string(&stored, "containerInstanceArn", case),
                host,
                "case {case:?}: expected the stored task on the instance the call named, \
                 got {stored}"
            );
        }
    }
}
