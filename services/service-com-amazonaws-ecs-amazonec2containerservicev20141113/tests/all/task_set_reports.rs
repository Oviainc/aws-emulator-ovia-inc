// SPDX-License-Identifier: CC0-1.0

//! What a TASK-SET response reports — the projection every task-set-returning operation
//! answers with, and how a batch read names a reference it could not resolve.
//!
//! Two rules, each asserted over a table of inputs rather than one recorded case:
//!
//! 1. Every operation that answers with a `TaskSet` projects the STORED record, so the
//!    same task set reads the same through all of them: `serviceArn` is the one ARN the
//!    record was stored with and is never recomposed per response, the `tags` MEMBER is
//!    always serialized (an empty list where there are none to report), `scale` is the
//!    percentage the last write left, and `stabilityStatus` is `STABILIZING` until the
//!    scheduler has confirmed a steady state — "The task runningCount is equal to the
//!    computedDesiredCount ... If any of those conditions aren't met, the stability
//!    status returns STABILIZING"
//!    (`smithy_query(query="doc",shape="TaskSet",member="stabilityStatus")`).
//! 2. `DescribeTaskSets` reports a reference that resolves to no task set as a `MISSING`
//!    failure naming that task set by the ID the reference carried, alone under the
//!    `task-set` resource type: the cluster and service segments of a stored task set's
//!    ARN say which service the task set exists IN, which one that resolves to nothing is
//!    held by none of.
//!
//! The first table varies the task set along what the rule generalises over — the
//! service's desired count, the percentage it is created at and later scaled to, and how
//! many tags it carries — and reads each row back through `CreateTaskSet`,
//! `DescribeTaskSets` (with and without `include: [TAGS]`), `UpdateTaskSet` and
//! `UpdateServicePrimaryTaskSet`. The second varies the reference along its form (a bare
//! id or a full ARN, qualified by this service, by another, or by neither), its spelling,
//! how many are named at once, and whether a resolvable one is named beside it.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};

use crate::ecs_wire::{
    create_cluster, list, ok, register_capacity, register_definition, service, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_sets::{create_external_service, describe_task_sets};

/// The task definition family every task set below runs.
const FAMILY: &str = "tsreportfamily";

/// The `Failure$reason` for an identifier that names nothing, from the API failure
/// reasons: "MISSING — the described resource could not be found".
const MISSING: &str = "MISSING";

/// `TaskSet$stabilityStatus` for a task set whose steady state the scheduler has not
/// confirmed, from the `StabilityStatus` enum (`STEADY_STATE`, `STABILIZING`).
const STABILIZING: &str = "STABILIZING";

/// The ARN of a task set under the resource path `path`, in the signed region and the
/// emulator's account.
fn task_set_arn(path: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{path}")
}

/// `Scale$value` of a task set, with the modelled default of `0` for a member the wire
/// omits because it equals that default.
fn scale_value(task_set: &Value, case: &str) -> f64 {
    match &task_set["scale"]["value"] {
        Value::Null => 0.0,
        value => value
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a numeric scale.value, got {value}")),
    }
}

/// The `(key, value)` pair of every tag a task set reports, in the order it reports them.
fn tag_pairs(task_set: &Value, case: &str) -> Vec<(String, String)> {
    list(task_set, "tags", case)
        .iter()
        .map(|tag| (string(tag, "key", case), string(tag, "value", case)))
        .collect()
}

/// The `(arn, reason)` pair of every failure a describe reports, in order.
fn failures(body: &Value, case: &str) -> Vec<(String, String)> {
    list(body, "failures", case)
        .iter()
        .map(|failure| (string(failure, "arn", case), string(failure, "reason", case)))
        .collect()
}

/// Assert that `view` — a `TaskSet` some operation answered with — carries the service
/// ARN `service_arn`, a serialized `tags` member holding `tags`, and the scale `scale`.
fn assert_projection(
    view: &Value,
    source: &str,
    case: &str,
    service_arn: &str,
    tags: &[(String, String)],
    scale: f64,
) {
    assert_eq!(
        string(view, "serviceArn", case),
        service_arn,
        "case {case:?}: expected {source} to report the service ARN {service_arn:?}, got \
         {view}"
    );
    // The MEMBER itself, not just its content: an untagged task set carries an empty
    // list rather than no member at all.
    assert!(
        view["tags"].is_array(),
        "case {case:?}: expected {source} to serialize the tags list, got {view}"
    );
    let reported = tag_pairs(view, case);
    assert_eq!(
        reported,
        tags.to_vec(),
        "case {case:?}: expected {source} to report the tags {tags:?}, got {reported:?} in \
         {view}"
    );
    let reported = scale_value(view, case);
    assert!(
        (reported - scale).abs() < f64::EPSILON,
        "case {case:?}: expected {source} to report scale.value {scale}, got {reported} in \
         {view}"
    );
}

/// One row of the projection table.
struct ProjectionCase {
    /// What the row is, for the failure message.
    name: &'static str,
    /// The `desiredCount` of the service the task set is created in.
    desired: i64,
    /// The percentage the task set is created at.
    created_scale: f64,
    /// The tags it is created with.
    tags: &'static [(&'static str, &'static str)],
    /// The percentage `UpdateTaskSet` then scales it to.
    updated_scale: f64,
}

#[tokio::test]
async fn every_task_set_response_projects_the_stored_task_set() {
    let router = service();
    let case = "setup";
    let cluster = "tsreports";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, FAMILY, case).await;

    let cases = [
        ProjectionCase {
            name: "an untagged task set at half the service's count",
            desired: 2,
            created_scale: 50.0,
            tags: &[],
            updated_scale: 100.0,
        },
        ProjectionCase {
            name: "a tagged task set created wanting nothing",
            desired: 1,
            created_scale: 0.0,
            tags: &[("stage", "blue")],
            updated_scale: 25.0,
        },
        ProjectionCase {
            name: "a task set at the top of the percentage range, with two tags",
            desired: 1,
            created_scale: 100.0,
            tags: &[("stage", "green"), ("owner", "platform")],
            updated_scale: 0.0,
        },
        ProjectionCase {
            name: "a fractional percentage",
            desired: 2,
            created_scale: 12.5,
            tags: &[],
            updated_scale: 87.5,
        },
    ];

    for (index, test) in cases.iter().enumerate() {
        let case = test.name;
        // One service per row, so no row can see another row's task sets and the
        // promotion below has exactly one task set to promote.
        let service_name = format!("tsreport{index}");
        let created_service =
            create_external_service(&router, cluster, &service_name, test.desired, case).await;
        let service_arn = string(&created_service, "serviceArn", case);
        // The service's own reads carry one ARN, whichever of them answers.
        let described_service = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": [service_name.clone()] }),
            case,
        )
        .await;
        assert_eq!(
            string(&list(&described_service, "services", case)[0], "serviceArn", case),
            service_arn,
            "case {case:?}: the service's own reads must carry one ARN, got {described_service}"
        );
        // The ARN a STORED task set reports for the service it exists in: recorded
        // behavior only — the task sets of the real service name the service WITHOUT the
        // cluster qualification their sibling `taskSetArn` and `clusterArn` carry, and the
        // create is the one answer carrying the service's own ARN instead. Either way it
        // is stored once, so every read below is asserted against the same value.
        let stored_service_arn =
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{service_name}");

        let tags: Vec<(String, String)> = test
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let created = ok(
            &router,
            "CreateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskDefinition": FAMILY,
                "launchType": "EC2",
                "scale": { "value": test.created_scale, "unit": "PERCENT" },
                "tags": test.tags
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect::<Vec<Value>>(),
            }),
            case,
        )
        .await;
        let created = created["taskSet"].clone();
        let id = string(&created, "id", case);
        // `CreateTaskSet` has no `include` member to narrow its answer with, so it
        // reports the tags the task set was just created with.
        assert_projection(
            &created,
            "the create",
            case,
            &service_arn,
            &tags,
            test.created_scale,
        );
        // A task set the create has just placed has not converged on what it wants.
        assert_eq!(
            string(&created, "stabilityStatus", case),
            STABILIZING,
            "case {case:?}: a just-created task set is still stabilizing, got {created}"
        );

        // ROUND TRIP: the same task set through the operation a client reads one with.
        // Without `include`, the tags LIST is still serialized and reports nothing.
        let read = describe_task_sets(&router, cluster, &service_name, None, false, case).await;
        let task_sets = list(&read, "taskSets", case);
        assert_eq!(
            task_sets.len(),
            1,
            "case {case:?}: expected exactly the created task set back, got {read}"
        );
        assert_eq!(
            string(&task_sets[0], "id", case),
            id,
            "case {case:?}: describe must report the created task set, got {read}"
        );
        assert_projection(
            &task_sets[0],
            "the read-back",
            case,
            &stored_service_arn,
            &[],
            test.created_scale,
        );
        // And with it, the stored tags come back.
        let with_tags =
            describe_task_sets(&router, cluster, &service_name, None, true, case).await;
        assert_projection(
            &list(&with_tags, "taskSets", case)[0],
            "the read-back asking for TAGS",
            case,
            &stored_service_arn,
            &tags,
            test.created_scale,
        );

        // A scale change is a WRITE: the percentage it names is what every later read
        // reports, the mutating call included.
        let updated = ok(
            &router,
            "UpdateTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskSet": id,
                "scale": { "value": test.updated_scale, "unit": "PERCENT" },
            }),
            case,
        )
        .await;
        assert_projection(
            &updated["taskSet"],
            "the scale update",
            case,
            &stored_service_arn,
            &[],
            test.updated_scale,
        );
        let read = describe_task_sets(
            &router,
            cluster,
            &service_name,
            Some(vec![id.clone()]),
            true,
            case,
        )
        .await;
        assert_projection(
            &list(&read, "taskSets", case)[0],
            "the read-back after the scale update",
            case,
            &stored_service_arn,
            &tags,
            test.updated_scale,
        );

        // Promoting the task set reports the same stored record, and restarts the
        // convergence the scheduler has to confirm.
        let promoted = ok(
            &router,
            "UpdateServicePrimaryTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "primaryTaskSet": id,
            }),
            case,
        )
        .await;
        let promoted = promoted["taskSet"].clone();
        assert_projection(
            &promoted,
            "the promotion",
            case,
            &stored_service_arn,
            &[],
            test.updated_scale,
        );
        assert_eq!(
            string(&promoted, "stabilityStatus", case),
            STABILIZING,
            "case {case:?}: a just-promoted task set is still stabilizing, got {promoted}"
        );
        assert_eq!(
            string(&promoted, "status", case),
            "PRIMARY",
            "case {case:?}: the promoted task set is the service's primary one, got {promoted}"
        );
    }
}

#[tokio::test]
async fn an_unresolvable_task_set_reference_is_named_by_its_id_alone() {
    let router = service();
    let case = "setup";
    let cluster = "tsmissing";
    // A second cluster and service, so a row may name a reference qualified by a service
    // other than the one being read.
    let other_cluster = "tsmissingelsewhere";
    let service_name = "tsmissingext";
    let other_service = "tsmissingother";
    for name in [cluster, other_cluster] {
        create_cluster(&router, name, case).await;
        register_capacity(&router, name, case).await;
    }
    register_definition(&router, FAMILY, case).await;
    create_external_service(&router, cluster, service_name, 1, case).await;
    create_external_service(&router, other_cluster, other_service, 1, case).await;
    let created = ok(
        &router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": FAMILY,
            "launchType": "EC2",
            "scale": { "value": 100, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    let stored_id = string(&created["taskSet"], "id", case);
    let stored_arn = string(&created["taskSet"], "taskSetArn", case);

    // Ids no task set of this service carries. The minted form is `ecs-svc/` followed by
    // 19 digits (`TaskSet$id`), and the member takes "the ID or full Amazon Resource Name
    // (ARN)" of a task set, so a row may also name something shaped unlike a minted id.
    let unknown = "ecs-svc/0000000000000009999";
    let other_unknown = "ecs-svc/0000000000000000042";
    let unshaped = "not-a-minted-id";

    // Rows: the case, what the request names, the failure ARNs expected back IN THE
    // REQUEST'S ORDER, and how many task sets the same answer reports.
    let rows: [(&str, Vec<String>, Vec<String>, usize); 9] = [
        (
            "a minted-looking id that names no task set",
            vec![unknown.to_string()],
            vec![task_set_arn(unknown)],
            0,
        ),
        (
            "another id of the same form",
            vec![other_unknown.to_string()],
            vec![task_set_arn(other_unknown)],
            0,
        ),
        (
            "an id shaped unlike a minted one",
            vec![unshaped.to_string()],
            vec![task_set_arn(unshaped)],
            0,
        ),
        (
            "the ARN form, qualified by the service being read",
            vec![task_set_arn(&format!("{cluster}/{service_name}/{unknown}"))],
            vec![task_set_arn(unknown)],
            0,
        ),
        (
            "the ARN form, qualified by another service",
            vec![task_set_arn(&format!(
                "{other_cluster}/{other_service}/{unknown}"
            ))],
            vec![task_set_arn(unknown)],
            0,
        ),
        (
            "the ARN form already naming the id alone",
            vec![task_set_arn(unknown)],
            vec![task_set_arn(unknown)],
            0,
        ),
        (
            "three unknown ids at once",
            vec![
                unknown.to_string(),
                other_unknown.to_string(),
                unshaped.to_string(),
            ],
            vec![
                task_set_arn(unknown),
                task_set_arn(other_unknown),
                task_set_arn(unshaped),
            ],
            0,
        ),
        (
            "the same unknown task set named in both forms",
            vec![
                unknown.to_string(),
                task_set_arn(&format!("{cluster}/{service_name}/{unknown}")),
            ],
            vec![task_set_arn(unknown)],
            0,
        ),
        (
            "the stored task set beside an unknown id",
            vec![stored_id.clone(), unknown.to_string()],
            vec![task_set_arn(unknown)],
            1,
        ),
    ];

    for (case, named, expected, described) in rows {
        let body = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(named.clone()),
            false,
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
            "case {case:?}: expected the failures {expected:?} for {named:?}, got \
             {reported:?} in {body}"
        );
        assert_eq!(
            list(&body, "taskSets", case).len(),
            described,
            "case {case:?}: expected {described} task set(s) reported beside the failures, \
             got {body}"
        );
    }

    // The control: a reference that DOES resolve, in either form, is answered as a task
    // set and never as a failure — so the rule above is about the failure channel alone.
    for (form, reference) in [("an id", stored_id), ("an ARN", stored_arn)] {
        let body = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(vec![reference.clone()]),
            false,
            form,
        )
        .await;
        assert!(
            failures(&body, form).is_empty(),
            "case {form:?}: {reference:?} names a stored task set, got {body}"
        );
        assert_eq!(
            list(&body, "taskSets", form).len(),
            1,
            "case {form:?}: expected the stored task set back for {reference:?}, got {body}"
        );
    }
}
