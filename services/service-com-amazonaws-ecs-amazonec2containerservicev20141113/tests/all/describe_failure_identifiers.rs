// SPDX-License-Identifier: CC0-1.0

//! How a batch describe NAMES an identifier it could not resolve, driven end to end over
//! the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: a batch read reports every identifier it cannot resolve as a
//! `MISSING` failure that names THE IDENTIFIER THE CALLER SENT — the value itself when it
//! is already an ARN, otherwise the ARN of that id alone. The cluster segment of a task or
//! container-instance ARN names the cluster the resource is registered in, which only a
//! stored resource has, so no cluster is inserted into the ARN a failure echoes. That is
//! what lets a client match each failure entry back to the identifier it asked about.
//!
//! The table varies the reference along what the rule generalises over — its form (bare id
//! or full ARN, with and without a cluster in the resource path), its alphabet and length,
//! the cluster it names, how many are asked for at once and whether a resolvable one is
//! asked for beside it — and the batch read it is sent to (`DescribeTasks` and
//! `DescribeContainerInstances`, the two reads whose references are "the id or full ARN").
//! The two recorded divergences (a bare task id and a bare container-instance id, each
//! answered under a cluster-qualified ARN) are two rows.

use serde_json::json;

use crate::ecs_wire::{
    cluster_with_capacity, create_cluster, list, ok, register_instance, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// The cluster every request below names.
const CLUSTER: &str = "demo";

/// Another cluster, for a reference that names a resource registered elsewhere.
const OTHER_CLUSTER: &str = "elsewhere";

/// The `Failure$reason` for an identifier that names nothing, from the API failure
/// reasons: "MISSING — the described resource could not be found".
const MISSING: &str = "MISSING";

/// The ARN of an ECS resource of `resource_type` in the signed region, under the resource
/// path `path` — the form every failure below is expected to echo.
fn arn(resource_type: &str, path: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:{resource_type}/{path}")
}

/// The `(arn, reason)` pair of every failure in a describe response, in order.
fn failures(body: &serde_json::Value, case: &str) -> Vec<(String, String)> {
    list(body, "failures", case)
        .iter()
        .map(|failure| {
            (
                string(failure, "arn", case),
                string(failure, "reason", case),
            )
        })
        .collect()
}

#[tokio::test]
async fn an_unresolvable_task_id_is_echoed_as_the_caller_sent_it() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "tasks").await;
    cluster_with_capacity(&router, OTHER_CLUSTER, "tasks").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "web",
            "containerDefinitions": [
                { "name": "app", "image": "public.ecr.aws/nginx/nginx:latest", "memory": 128 },
            ],
        }),
        "tasks",
    )
    .await;
    let run = ok(
        &router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": "web" }),
        "tasks",
    )
    .await;
    let stored = string(&list(&run, "tasks", "tasks")[0], "taskArn", "tasks");
    let stored_id = stored
        .rsplit('/')
        .next()
        .expect("a task ARN ends in its id")
        .to_string();

    // Rows: the case, what the request names, and the failure ARNs expected back IN THE
    // REQUEST'S ORDER.
    let rows: [(&str, Vec<String>, Vec<String>); 8] = [
        (
            "a 32-hex id that names nothing",
            vec!["9".repeat(32)],
            vec![arn("task", &"9".repeat(32))],
        ),
        (
            "a short id",
            vec!["bogus".to_string()],
            vec![arn("task", "bogus")],
        ),
        (
            "a uuid-shaped id",
            vec!["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()],
            vec![arn("task", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")],
        ),
        (
            "a full ARN with no cluster in it",
            vec![arn("task", &"8".repeat(32))],
            vec![arn("task", &"8".repeat(32))],
        ),
        (
            "a full ARN qualified by this cluster",
            vec![arn("task", &format!("{CLUSTER}/{}", "7".repeat(32)))],
            vec![arn("task", &format!("{CLUSTER}/{}", "7".repeat(32)))],
        ),
        (
            "a full ARN qualified by another cluster",
            vec![arn("task", &format!("{OTHER_CLUSTER}/{}", "6".repeat(32)))],
            vec![arn("task", &format!("{OTHER_CLUSTER}/{}", "6".repeat(32)))],
        ),
        (
            "three unknown ids at once",
            vec!["a".repeat(32), "b".repeat(32), "c".repeat(32)],
            vec![
                arn("task", &"a".repeat(32)),
                arn("task", &"b".repeat(32)),
                arn("task", &"c".repeat(32)),
            ],
        ),
        (
            "the stored task beside an unknown id",
            vec![stored_id.clone(), "d".repeat(32)],
            vec![arn("task", &"d".repeat(32))],
        ),
    ];

    for (case, named, expected) in rows {
        let body = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": CLUSTER, "tasks": named }),
            case,
        )
        .await;
        let reported = failures(&body, case);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|arn| (arn, MISSING.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the failures {expected:?}, got {reported:?} in {body}"
        );
    }

    // The resolvable id is still answered as a task, so the rule above is about the
    // failure channel and not about the read losing resources it holds.
    let described = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [stored_id.clone(), "e".repeat(32)] }),
        "mixed",
    )
    .await;
    let read: Vec<String> = list(&described, "tasks", "mixed")
        .iter()
        .map(|task| string(task, "taskArn", "mixed"))
        .collect();
    assert_eq!(
        read,
        vec![stored.clone()],
        "case \"mixed\": expected the stored task back beside the failure, got {described}"
    );
}

#[tokio::test]
async fn an_unresolvable_container_instance_id_is_echoed_as_the_caller_sent_it() {
    let router = service();
    create_cluster(&router, CLUSTER, "instances").await;
    create_cluster(&router, OTHER_CLUSTER, "instances").await;
    let stored = register_instance(&router, CLUSTER, "i-0a0a0a0a0a0a0a0a0", "instances").await;
    let stored_id = stored
        .rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
        .to_string();
    let elsewhere =
        register_instance(&router, OTHER_CLUSTER, "i-0b0b0b0b0b0b0b0b0", "instances").await;

    // Rows: the case, what the request names, and the failure ARNs expected back IN THE
    // REQUEST'S ORDER.
    let rows: [(&str, Vec<String>, Vec<String>); 8] = [
        (
            "a 32-hex id that names nothing",
            vec!["9".repeat(32)],
            vec![arn("container-instance", &"9".repeat(32))],
        ),
        (
            "a short id",
            vec!["x1".to_string()],
            vec![arn("container-instance", "x1")],
        ),
        (
            "a uuid-shaped id",
            vec!["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()],
            vec![arn(
                "container-instance",
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            )],
        ),
        (
            "a full ARN with no cluster in it",
            vec![arn("container-instance", &"8".repeat(32))],
            vec![arn("container-instance", &"8".repeat(32))],
        ),
        (
            "a full ARN qualified by this cluster",
            vec![arn(
                "container-instance",
                &format!("{CLUSTER}/{}", "7".repeat(32)),
            )],
            vec![arn(
                "container-instance",
                &format!("{CLUSTER}/{}", "7".repeat(32)),
            )],
        ),
        (
            "the ARN of an instance registered into another cluster",
            vec![elsewhere.clone()],
            vec![elsewhere.clone()],
        ),
        (
            "three unknown ids at once",
            vec!["a".repeat(32), "b".repeat(32), "c".repeat(32)],
            vec![
                arn("container-instance", &"a".repeat(32)),
                arn("container-instance", &"b".repeat(32)),
                arn("container-instance", &"c".repeat(32)),
            ],
        ),
        (
            "the registered instance beside an unknown id",
            vec![stored_id.clone(), "d".repeat(32)],
            vec![arn("container-instance", &"d".repeat(32))],
        ),
    ];

    for (case, named, expected) in rows {
        let body = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": CLUSTER, "containerInstances": named }),
            case,
        )
        .await;
        let reported = failures(&body, case);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|arn| (arn, MISSING.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the failures {expected:?}, got {reported:?} in {body}"
        );
    }

    // The registered instance is still described by its bare id, so an id that DOES
    // resolve is unaffected by how an unresolvable one is echoed.
    let described = ok(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [stored_id, "e".repeat(32)] }),
        "mixed",
    )
    .await;
    let read: Vec<String> = list(&described, "containerInstances", "mixed")
        .iter()
        .map(|instance| string(instance, "containerInstanceArn", "mixed"))
        .collect();
    assert_eq!(
        read,
        vec![stored],
        "case \"mixed\": expected the registered instance back beside the failure, got \
         {described}"
    );
}
