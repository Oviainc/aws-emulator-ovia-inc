// SPDX-License-Identifier: CC0-1.0

//! `DeregisterTaskDefinition`, and what an `INACTIVE` revision may still be used for.
//!
//! The rules under test, from the operation's own doc:
//!
//! * "Deregisters the specified task definition by family and revision. Upon
//!   deregistration, the task definition is marked as `INACTIVE`" — exactly the ONE
//!   revision the request names moves to `INACTIVE` and records the instant it was
//!   deregistered; every other revision of the family, and every other member of the
//!   revision itself, is left as it was stored.
//! * "INACTIVE task definitions remain discoverable in your account indefinitely" — the
//!   revision is still readable through `DescribeTaskDefinition` by its own
//!   family:revision, while a reference that names no revision resolves to "the latest
//!   `ACTIVE` revision", so it skips the deregistered ones.
//! * "You can't use an `INACTIVE` task definition to run new tasks or create new
//!   services, and you can't update an existing service to reference an `INACTIVE` task
//!   definition. ... Existing services that reference an `INACTIVE` task definition can
//!   still scale up or down by modifying the service's desired count." Such a reference
//!   RESOLVES — the revision is held, in a status the call may not use — so it is refused
//!   as the bad parameter it is, while a reference that resolves to no revision at all is
//!   the unknown identifier it is; the two conditions keep two distinct error codes.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, count, list, ok, refusal, refused, service, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The image every revision here starts from; irrelevant to the rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// Register one more revision of `family`, whose single container is named after the
/// revision so the stored revisions are told apart by their CONTENT and not only by
/// their number. Returns the revision number the registration minted.
async fn register_revision(
    router: &MultiServiceRouter,
    family: &str,
    slot: i64,
    case: &str,
) -> i64 {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container(&format!("app{slot}"), IMAGE)],
        }),
        case,
    )
    .await;
    count(&body["taskDefinition"], "revision", case)
}

/// The `taskDefinition` structure of a response that must carry one.
fn definition_of(body: &Value, case: &str) -> Value {
    match body.get("taskDefinition") {
        Some(value) if value.is_object() => value.clone(),
        other => panic!("case {case:?}: expected a taskDefinition structure, got {other:?}"),
    }
}

/// `DescribeTaskDefinition` for one reference, as the reader a write is proved through.
async fn describe(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": reference }),
        case,
    )
    .await;
    definition_of(&body, case)
}

/// The ARN a revision of `family` carries, as
/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`.
fn definition_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

/// One row of the deregistration table: how many revisions the family holds, which one
/// is deregistered, and whether the request names it by `family:revision` or by its full
/// ARN (the two forms `DeregisterTaskDefinition$taskDefinition` documents).
struct Row {
    case: &'static str,
    revisions: i64,
    target: i64,
    by_arn: bool,
}

#[tokio::test]
async fn deregistering_one_revision_marks_only_that_revision_inactive() {
    let router = service();

    // The rows differ along the size of the family, WHICH revision of it is
    // deregistered (the first, a middle one, the latest) and the reference form.
    let rows = [
        Row { case: "sole revision, by family:revision", revisions: 1, target: 1, by_arn: false },
        Row { case: "sole revision, by ARN", revisions: 1, target: 1, by_arn: true },
        Row { case: "three revisions, the first", revisions: 3, target: 1, by_arn: false },
        Row { case: "three revisions, a middle one, by ARN", revisions: 3, target: 2, by_arn: true },
        Row { case: "three revisions, the latest", revisions: 3, target: 3, by_arn: false },
        Row { case: "five revisions, a middle one", revisions: 5, target: 3, by_arn: false },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let family = format!("deregfamily{index}");
        for slot in 1..=row.revisions {
            let minted = register_revision(&router, &family, slot, case).await;
            assert_eq!(
                minted, slot,
                "case {case:?}: expected registration {slot} of the family to mint \
                 revision {slot}, got {minted}"
            );
        }

        let target_arn = definition_arn(&family, row.target);
        let reference = if row.by_arn {
            target_arn.clone()
        } else {
            format!("{family}:{}", row.target)
        };
        let before = describe(&router, &reference, case).await;
        assert_eq!(
            string(&before, "status", case),
            "ACTIVE",
            "case {case:?}: a registered revision is ACTIVE before it is deregistered, \
             got {before}"
        );
        assert!(
            before.get("deregisteredAt").is_none(),
            "case {case:?}: an ACTIVE revision carries no deregisteredAt, got {before}"
        );

        let deregistered = definition_of(
            &ok(
                &router,
                "DeregisterTaskDefinition",
                json!({ "taskDefinition": reference }),
                case,
            )
            .await,
            case,
        );

        // The response describes the revision that was deregistered: INACTIVE, stamped
        // with the instant of this call, and otherwise the revision as it was stored.
        assert_eq!(
            string(&deregistered, "status", case),
            "INACTIVE",
            "case {case:?}: expected status INACTIVE after deregistration, got \
             {deregistered}"
        );
        let stamp = deregistered
            .get("deregisteredAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a deregisteredAt timestamp, got {deregistered}")
            });
        let registered_at = before
            .get("registeredAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a registeredAt, got {before}"));
        assert!(
            stamp >= registered_at,
            "case {case:?}: expected deregisteredAt {stamp} to be at or after \
             registeredAt {registered_at}"
        );
        assert_eq!(
            string(&deregistered, "taskDefinitionArn", case),
            target_arn,
            "case {case:?}: expected the stored ARN back, got {deregistered}"
        );
        assert_eq!(
            count(&deregistered, "revision", case),
            row.target,
            "case {case:?}: expected revision {} back, got {deregistered}",
            row.target
        );
        assert_eq!(
            string(&deregistered, "family", case),
            family,
            "case {case:?}: expected family {family:?} back, got {deregistered}"
        );
        // Everything but the two members deregistration owns is the stored revision.
        let mut expected = before.clone();
        expected["status"] = json!("INACTIVE");
        expected["deregisteredAt"] = json!(stamp);
        assert_eq!(
            deregistered, expected,
            "case {case:?}: deregistration must change only status and deregisteredAt, \
             got {deregistered} for a stored {before}"
        );

        // Read back through the reader: the write is in the store, not in the response.
        let read_back = describe(&router, &reference, case).await;
        assert_eq!(
            read_back, deregistered,
            "case {case:?}: DescribeTaskDefinition must report the deregistered \
             revision, got {read_back}"
        );
        // Addressed the OTHER way round it is the same revision.
        let other_form = if row.by_arn {
            format!("{family}:{}", row.target)
        } else {
            target_arn.clone()
        };
        let by_other_form = describe(&router, &other_form, case).await;
        assert_eq!(
            by_other_form, deregistered,
            "case {case:?}: {other_form:?} names the same revision, got {by_other_form}"
        );

        // Every other revision of the family is untouched.
        for slot in 1..=row.revisions {
            if slot == row.target {
                continue;
            }
            let sibling = describe(&router, &format!("{family}:{slot}"), case).await;
            assert_eq!(
                string(&sibling, "status", case),
                "ACTIVE",
                "case {case:?}: revision {slot} was not deregistered, got {sibling}"
            );
            assert!(
                sibling.get("deregisteredAt").is_none(),
                "case {case:?}: revision {slot} carries no deregisteredAt, got {sibling}"
            );
        }

        // A reference with no revision takes "the latest ACTIVE revision", so it skips
        // the deregistered one — and resolves to nothing once the family has none left.
        let latest_active = (1..=row.revisions)
            .filter(|slot| *slot != row.target)
            .max();
        match latest_active {
            Some(slot) => {
                let latest = describe(&router, &family, case).await;
                assert_eq!(
                    count(&latest, "revision", case),
                    slot,
                    "case {case:?}: expected the family to resolve to its latest ACTIVE \
                     revision {slot}, got {latest}"
                );
            }
            None => {
                refused(
                    &router,
                    "DescribeTaskDefinition",
                    json!({ "taskDefinition": family.clone() }),
                    "ClientException",
                    case,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn a_second_deregistration_changes_nothing_and_the_family_keeps_counting() {
    let router = service();
    let case = "repeat deregistration";
    let family = "repeatfamily";
    for slot in 1..=2 {
        register_revision(&router, family, slot, case).await;
    }

    let first = definition_of(
        &ok(
            &router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": format!("{family}:1") }),
            case,
        )
        .await,
        case,
    );
    // The transition happened once, so a repeat reports the revision as it stands
    // rather than re-stamping it with a later instant.
    for attempt in ["second", "third"] {
        let again = definition_of(
            &ok(
                &router,
                "DeregisterTaskDefinition",
                json!({ "taskDefinition": definition_arn(family, 1) }),
                attempt,
            )
            .await,
            attempt,
        );
        assert_eq!(
            again, first,
            "case {attempt:?}: a repeated deregistration must report the revision \
             unchanged, got {again} after {first}"
        );
    }

    // "Each time that you register a new revision of a task definition in the same
    // family, the revision value always increases by one": a deregistered revision
    // number is not handed out again.
    let next = register_revision(&router, family, 3, case).await;
    assert_eq!(
        next, 3,
        "case {case:?}: expected the next registration to mint revision 3, got {next}"
    );
    let after = describe(&router, family, case).await;
    assert_eq!(
        count(&after, "revision", case),
        3,
        "case {case:?}: expected the family to resolve to revision 3, got {after}"
    );
    assert_eq!(
        string(&describe(&router, &format!("{family}:1"), case).await, "status", case),
        "INACTIVE",
        "case {case:?}: the deregistered revision stays INACTIVE"
    );
}

#[tokio::test]
async fn a_reference_that_names_no_single_revision_is_refused_and_persists_nothing() {
    let router = service();
    let case = "setup";
    let family = "refusalfamily";
    for slot in 1..=2 {
        register_revision(&router, family, slot, case).await;
    }

    // The rows span the ways a `taskDefinition` can fail to name ONE stored revision, and
    // differ in WHICH way they fail. `NAMES_NO_REVISION` spells no revision where "you
    // must specify a revision", so the refusal is the `InvalidParameterException` naming
    // the missing revision, in fixed text; `UNRESOLVABLE` names a revision that resolves
    // to no stored one, which is the condition `DescribeTaskDefinition` answers too, with
    // the same `ClientException` and the same sentence.
    const NAMES_NO_REVISION: &str = "names no revision";
    const UNRESOLVABLE: &str = "resolves to no stored revision";
    /// The fixed sentence a reference that spells no revision earns, from
    /// [`crate::task_definition_revision_requirement`].
    const REVISION_MISSING: &str = "Revision is missing";
    let arn_without_revision =
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}");
    let rows: [(&str, String, &str); 8] = [
        ("family only", family.to_string(), NAMES_NO_REVISION),
        (
            "family and the wildcard revision",
            format!("{family}:*"),
            NAMES_NO_REVISION,
        ),
        (
            "ARN without a revision",
            arn_without_revision,
            NAMES_NO_REVISION,
        ),
        (
            "revision the family never minted",
            format!("{family}:9"),
            UNRESOLVABLE,
        ),
        (
            "revision of a family that was never registered",
            "neverregistered:1".to_string(),
            UNRESOLVABLE,
        ),
        (
            "ARN of a family that was never registered",
            definition_arn("neverregistered", 1),
            UNRESOLVABLE,
        ),
        ("revision zero", format!("{family}:0"), UNRESOLVABLE),
        (
            "not a task-definition reference at all",
            "not a reference".to_string(),
            NAMES_NO_REVISION,
        ),
    ];

    for (case, reference, kind) in rows {
        let (code, message) = refusal(
            &router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": reference.clone() }),
            case,
        )
        .await;
        if kind == UNRESOLVABLE {
            assert_eq!(
                code, "ClientException",
                "case {case:?}: expected a ClientException for {reference:?}, got {code} \
                 with {message:?}"
            );
            // ONE condition, so the reader of a revision and the deregistration of one
            // refuse it with the same sentence.
            let (describe_code, describe_message) = refusal(
                &router,
                "DescribeTaskDefinition",
                json!({ "taskDefinition": reference.clone() }),
                case,
            )
            .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (describe_code.as_str(), describe_message.as_str()),
                "case {case:?}: expected the same refusal DescribeTaskDefinition gives \
                 for {reference:?}, got {code}/{message:?} against \
                 {describe_code}/{describe_message:?}"
            );
        } else {
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException for {reference:?}, \
                 got {code} with {message:?}"
            );
            // FIXED text: what the value misses is the revision, whatever it spelled
            // instead, so the sentence carries nothing of the request.
            assert_eq!(
                message, REVISION_MISSING,
                "case {case:?}: expected {REVISION_MISSING:?} for {reference:?}, got \
                 {message:?}"
            );
        }
    }

    // A refusal deregisters nothing: both revisions are still ACTIVE.
    for slot in 1..=2 {
        let still = describe(&router, &format!("{family}:{slot}"), "after the refusals").await;
        assert_eq!(
            string(&still, "status", "after the refusals"),
            "ACTIVE",
            "expected revision {slot} to be untouched by the refusals, got {still}"
        );
    }
}

#[tokio::test]
async fn an_inactive_revision_cannot_be_used_to_run_new_work() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, "deregcluster", case).await;
    let family = "usagefamily";
    for slot in 1..=2 {
        register_revision(&router, family, slot, case).await;
    }
    // The service the update cases act on runs the revision that stays ACTIVE.
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": "deregcluster",
            "serviceName": "running",
            "taskDefinition": format!("{family}:2"),
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    ok(
        &router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{family}:1") }),
        case,
    )
    .await;

    // The rule holds for every operation that takes a `taskDefinition` the caller
    // chooses, so the table spans the three of them rather than the one the report
    // named. Each row is a request body with `{reference}` to fill in.
    let request_for = |operation: &str, reference: &str, slot: usize| -> Value {
        match operation {
            "RunTask" => json!({ "cluster": "deregcluster", "taskDefinition": reference }),
            "CreateService" => json!({
                "cluster": "deregcluster",
                "serviceName": format!("fresh{slot}"),
                "taskDefinition": reference,
                "desiredCount": 1,
                "launchType": "EC2",
            }),
            "UpdateService" => json!({
                "cluster": "deregcluster",
                "service": "running",
                "taskDefinition": reference,
            }),
            other => panic!("unexpected operation {other:?}"),
        }
    };
    let operations = ["RunTask", "CreateService", "UpdateService"];

    // The deregistered revision, in both reference forms.
    let inactive_references = [format!("{family}:1"), definition_arn(family, 1)];
    let mut refusals = Vec::new();
    for operation in operations {
        for (slot, reference) in inactive_references.iter().enumerate() {
            let case = format!("{operation} on the INACTIVE revision as {reference:?}");
            let (code, message) =
                refusal(&router, operation, request_for(operation, reference, slot), &case).await;
            // A reference that RESOLVES to a revision this service holds, in a status
            // this call may not use, is a parameter whose value is wrong for the call —
            // not an unknown identifier, which is what the family-with-no-ACTIVE-revision
            // rows further down report as a ClientException.
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException, got {code} with \
                 {message:?}"
            );
            refusals.push((case, code, message));
        }
    }
    // ONE condition, so every operation reports the same refusal for it.
    let (first_case, _, first_message) = &refusals[0];
    for (case, _, message) in &refusals {
        assert_eq!(
            message, first_message,
            "case {case:?}: expected the same refusal as {first_case:?} ({first_message:?}), \
             got {message:?}"
        );
    }

    // The control: the SAME operations accept the revision that is still ACTIVE, named
    // explicitly and through the family (which resolves to the latest ACTIVE revision),
    // so the refusals above are keyed on the status and not on the operation. Each
    // success has to land on revision 2 — the one that is still ACTIVE — whichever way
    // the request spelled it.
    let active_arn = definition_arn(family, 2);
    for (slot, reference) in [format!("{family}:2"), family.to_string()]
        .iter()
        .enumerate()
    {
        for operation in operations {
            let case = format!("{operation} on the ACTIVE revision as {reference:?}");
            let body = ok(
                &router,
                operation,
                request_for(operation, reference, 10 + slot * 10 + operation.len()),
                &case,
            )
            .await;
            // Each operation reports the revision it resolved, in its own member.
            let resolved = match operation {
                "RunTask" => {
                    let tasks = list(&body, "tasks", &case);
                    assert_eq!(
                        tasks.len(),
                        1,
                        "case {case:?}: expected one task, got {body}"
                    );
                    string(&tasks[0], "taskDefinitionArn", &case)
                }
                _ => string(&body["service"], "taskDefinition", &case),
            };
            assert_eq!(
                resolved, active_arn,
                "case {case:?}: expected the ACTIVE revision {active_arn} to be \
                 resolved, got {resolved} in {body}"
            );
        }
    }

    // "Existing services that reference an INACTIVE task definition continue to run
    // without disruption ... can still scale up or down by modifying the service's
    // desired count": deregister the revision the service runs, then scale it.
    ok(
        &router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{family}:2") }),
        "deregister the running revision",
    )
    .await;
    for target in [3_i64, 0, 2] {
        let case = format!("scale a service on a deregistered revision to {target}");
        let updated = ok(
            &router,
            "UpdateService",
            json!({
                "cluster": "deregcluster",
                "service": "running",
                "desiredCount": target,
            }),
            &case,
        )
        .await;
        assert_eq!(
            count(&updated["service"], "desiredCount", &case),
            target,
            "case {case:?}: expected desiredCount {target}, got {updated}"
        );
        assert_eq!(
            string(&updated["service"], "taskDefinition", &case),
            definition_arn(family, 2),
            "case {case:?}: expected the service to keep its own revision, got {updated}"
        );
    }

    // With every revision of the family INACTIVE, a reference that names no revision
    // resolves to nothing at all.
    for operation in operations {
        let case = format!("{operation} on a family with no ACTIVE revision");
        let (code, message) =
            refusal(&router, operation, request_for(operation, family, 99), &case).await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected a ClientException, got {code} with {message:?}"
        );
    }
    // Nothing above placed work on the deregistered revision: the tasks the cluster
    // holds all belong to the ACTIVE-revision runs and to the service.
    let tasks = ok(
        &router,
        "ListTasks",
        json!({ "cluster": "deregcluster" }),
        "no task on an INACTIVE revision",
    )
    .await;
    for arn in list(&tasks, "taskArns", "no task on an INACTIVE revision") {
        let described = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": "deregcluster", "tasks": [arn] }),
            "no task on an INACTIVE revision",
        )
        .await;
        let task = &list(&described, "tasks", "no task on an INACTIVE revision")[0];
        assert_ne!(
            string(task, "taskDefinitionArn", "no task on an INACTIVE revision"),
            definition_arn(family, 1),
            "no task may run the revision that was INACTIVE when the run was asked for, \
             got {task}"
        );
    }
}
