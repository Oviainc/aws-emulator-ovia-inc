// SPDX-License-Identifier: CC0-1.0

//! `DeleteTaskDefinitions`, and what a `DELETE_IN_PROGRESS` revision still answers.
//!
//! The rules under test, from the operation's own doc:
//!
//! * "Deletes one or more task definitions. ... When you delete a task definition
//!   revision, it is immediately transitions from the `INACTIVE` to
//!   `DELETE_IN_PROGRESS`" — exactly the revisions the request names move, each stamped
//!   with `deleteRequestedAt`, and every other member of the revision and every other
//!   revision of the family is left as it was stored.
//! * "You must deregister a task definition revision before you delete it" — an entry
//!   naming a revision that is still `ACTIVE` is a failure OF THAT ENTRY, reported in
//!   `failures` beside the revisions the same call did delete, as is an entry that
//!   resolves to no stored revision (`MISSING`).
//! * "You must specify a revision" — an entry that names no ONE revision misses the
//!   member's own constraint, so the request is refused whole and deletes nothing.
//! * "A task definition revision will stay in `DELETE_IN_PROGRESS` status until all the
//!   associated tasks and services have been terminated. ... If a task definition
//!   revisions are in the `DELETE_IN_PROGRESS` state, the task definition name is
//!   displayed in the console and returned in the API" — the revision stays readable and
//!   its family stays listed, and "the revision is incremented the next time you create a
//!   task definition with that name".
//! * "You can't use a `DELETE_IN_PROGRESS` task definition revision to run new tasks or
//!   create new services ... Existing services that reference a `DELETE_IN_PROGRESS` task
//!   definition revision can still scale up or down by modifying the service's desired
//!   count."
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, count, list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The image every revision here starts from; irrelevant to the rules, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The ARN a revision of `family` carries, as
/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`.
fn definition_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

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

/// Deregister one revision, the step the delete requires ("You must deregister a task
/// definition revision before you delete it"), and return the revision it reports.
async fn deregister(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    definition_of(
        &ok(
            router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": reference }),
            case,
        )
        .await,
        case,
    )
}

/// One `DeleteTaskDefinitions` call over `references`, returning
/// `(taskDefinitions, failures)`.
async fn delete(
    router: &MultiServiceRouter,
    references: &[String],
    case: &str,
) -> (Vec<Value>, Vec<Value>) {
    let body = ok(
        router,
        "DeleteTaskDefinitions",
        json!({ "taskDefinitions": references }),
        case,
    )
    .await;
    (
        list(&body, "taskDefinitions", case),
        list(&body, "failures", case),
    )
}

/// The ARNs one `ListTaskDefinitions` page reports for `family` in `status`.
async fn list_in_status(
    router: &MultiServiceRouter,
    family: &str,
    status: &str,
    case: &str,
) -> Vec<String> {
    let body = ok(
        router,
        "ListTaskDefinitions",
        json!({ "familyPrefix": family, "status": status }),
        case,
    )
    .await;
    list(&body, "taskDefinitionArns", case)
        .iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected ARN strings, got {arn}"))
                .to_string()
        })
        .collect()
}

/// One row of the deletion table: how many revisions the family holds, WHICH of them one
/// call deletes, and whether the request names them by `family:revision` or by their full
/// ARN (the two forms `DeleteTaskDefinitionsRequest$taskDefinitions` documents).
struct Row {
    case: &'static str,
    revisions: i64,
    targets: &'static [i64],
    by_arn: bool,
}

#[tokio::test]
async fn deleting_deregistered_revisions_moves_only_those_to_delete_in_progress() {
    let router = service();

    // The rows differ along the size of the family, HOW MANY revisions one call deletes
    // (one, several, the documented maximum of ten, the same one named twice), WHICH of
    // them, and the reference form.
    let rows = [
        Row { case: "sole revision, by family:revision", revisions: 1, targets: &[1], by_arn: false },
        Row { case: "sole revision, by ARN", revisions: 1, targets: &[1], by_arn: true },
        Row { case: "three revisions, the first", revisions: 3, targets: &[1], by_arn: false },
        Row { case: "three revisions, the latest, by ARN", revisions: 3, targets: &[3], by_arn: true },
        Row { case: "five revisions, the middle three", revisions: 5, targets: &[2, 3, 4], by_arn: false },
        Row { case: "five revisions, the ends, by ARN", revisions: 5, targets: &[1, 5], by_arn: true },
        Row { case: "ten revisions, all ten in one call", revisions: 10, targets: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], by_arn: false },
        Row { case: "three revisions, one named twice", revisions: 3, targets: &[2, 2], by_arn: false },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let family = format!("delfamily{index}");
        for slot in 1..=row.revisions {
            register_revision(&router, &family, slot, case).await;
        }

        // The revisions this row deletes, deregistered first and remembered as stored.
        let mut unique: Vec<i64> = Vec::new();
        let mut inactive: Vec<Value> = Vec::new();
        for target in row.targets {
            if unique.contains(target) {
                continue;
            }
            unique.push(*target);
            inactive.push(deregister(&router, &format!("{family}:{target}"), case).await);
        }

        let references: Vec<String> = row
            .targets
            .iter()
            .map(|target| {
                if row.by_arn {
                    definition_arn(&family, *target)
                } else {
                    format!("{family}:{target}")
                }
            })
            .collect();
        let (deleted, failures) = delete(&router, &references, case).await;
        assert!(
            failures.is_empty(),
            "case {case:?}: expected no failures for deregistered revisions, got {failures:?}"
        );
        // A revision named twice is one revision, so it is reported once.
        assert_eq!(
            deleted.len(),
            unique.len(),
            "case {case:?}: expected {} deleted revisions for {references:?}, got {deleted:?}",
            unique.len()
        );

        for (slot, revision) in unique.iter().enumerate() {
            let stored = &inactive[slot];
            let reported = &deleted[slot];
            assert_eq!(
                count(reported, "revision", case),
                *revision,
                "case {case:?}: expected the revisions back in the order they were \
                 named, got {deleted:?}"
            );
            assert_eq!(
                string(reported, "status", case),
                "DELETE_IN_PROGRESS",
                "case {case:?}: expected revision {revision} to be DELETE_IN_PROGRESS, \
                 got {reported}"
            );
            let stamp = reported
                .get("deleteRequestedAt")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a deleteRequestedAt, got {reported}")
                });
            let deregistered_at = stored
                .get("deregisteredAt")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a deregisteredAt, got {stored}")
                });
            assert!(
                stamp >= deregistered_at,
                "case {case:?}: expected deleteRequestedAt {stamp} to be at or after \
                 deregisteredAt {deregistered_at}"
            );
            // Everything but the two members the delete owns is the stored revision.
            let mut expected = stored.clone();
            expected["status"] = json!("DELETE_IN_PROGRESS");
            expected["deleteRequestedAt"] = json!(stamp);
            assert_eq!(
                reported, &expected,
                "case {case:?}: a delete must change only status and deleteRequestedAt, \
                 got {reported} for a stored {stored}"
            );

            // Read back through the reader: the write is in the store, not in the
            // response — "the task definition name is displayed in the console and
            // returned in the API" while the delete is outstanding.
            for reference in [format!("{family}:{revision}"), definition_arn(&family, *revision)] {
                let read_back = describe(&router, &reference, case).await;
                assert_eq!(
                    &read_back, reported,
                    "case {case:?}: DescribeTaskDefinition must report the revision \
                     {reference:?} as the delete left it, got {read_back}"
                );
            }
        }

        // Every revision the call did not name is untouched.
        for slot in 1..=row.revisions {
            if unique.contains(&slot) {
                continue;
            }
            let sibling = describe(&router, &format!("{family}:{slot}"), case).await;
            assert_eq!(
                string(&sibling, "status", case),
                "ACTIVE",
                "case {case:?}: revision {slot} was not named by the delete, got {sibling}"
            );
            assert!(
                sibling.get("deleteRequestedAt").is_none(),
                "case {case:?}: revision {slot} carries no deleteRequestedAt, got {sibling}"
            );
        }

        // The listing reports each revision under the status it is now in, and under no
        // other: nothing is left behind in `INACTIVE`.
        let mut expected_deleted: Vec<String> = unique
            .iter()
            .map(|revision| definition_arn(&family, *revision))
            .collect();
        expected_deleted.sort();
        let mut listed = list_in_status(&router, &family, "DELETE_IN_PROGRESS", case).await;
        listed.sort();
        assert_eq!(
            listed, expected_deleted,
            "case {case:?}: expected the deleted revisions to be listed as \
             DELETE_IN_PROGRESS, got {listed:?}"
        );
        let still_inactive = list_in_status(&router, &family, "INACTIVE", case).await;
        assert!(
            still_inactive.is_empty(),
            "case {case:?}: expected no INACTIVE revision left, got {still_inactive:?}"
        );
        let mut expected_active: Vec<String> = (1..=row.revisions)
            .filter(|slot| !unique.contains(slot))
            .map(|slot| definition_arn(&family, slot))
            .collect();
        expected_active.sort();
        let mut active = list_in_status(&router, &family, "ACTIVE", case).await;
        active.sort();
        assert_eq!(
            active, expected_active,
            "case {case:?}: expected the untouched revisions to stay ACTIVE, got {active:?}"
        );

        // "If a task definition revisions are in the DELETE_IN_PROGRESS state, the task
        // definition name is ... returned in the API."
        let families = ok(
            &router,
            "ListTaskDefinitionFamilies",
            json!({ "familyPrefix": family.clone() }),
            case,
        )
        .await;
        assert_eq!(
            list(&families, "families", case),
            vec![json!(family.clone())],
            "case {case:?}: expected the family to still be listed, got {families}"
        );

        // "The task definition name is retained by Amazon ECS and the revision is
        // incremented the next time you create a task definition with that name": a
        // deleted revision number is not handed out again.
        let next = register_revision(&router, &family, row.revisions + 1, case).await;
        assert_eq!(
            next,
            row.revisions + 1,
            "case {case:?}: expected the next registration to mint revision {}, got {next}",
            row.revisions + 1
        );
    }
}

#[tokio::test]
async fn a_second_delete_reports_the_revision_unchanged() {
    let router = service();
    let case = "repeat delete";
    let family = "redeletefamily";
    register_revision(&router, family, 1, case).await;
    deregister(&router, &format!("{family}:1"), case).await;

    let (first, failures) = delete(&router, &[format!("{family}:1")], case).await;
    assert!(
        failures.is_empty(),
        "case {case:?}: expected no failures, got {failures:?}"
    );
    // The transition happened once, so a repeat reports the revision as it stands rather
    // than re-stamping it with a later instant.
    for attempt in ["second", "third"] {
        let (again, failures) = delete(&router, &[definition_arn(family, 1)], attempt).await;
        assert!(
            failures.is_empty(),
            "case {attempt:?}: expected no failures, got {failures:?}"
        );
        assert_eq!(
            again, first,
            "case {attempt:?}: a repeated delete must report the revision unchanged, \
             got {again:?} after {first:?}"
        );
    }
}

#[tokio::test]
async fn an_entry_that_cannot_be_deleted_fails_on_its_own_and_changes_nothing() {
    let router = service();
    let case = "setup";
    let family = "failurefamily";
    for slot in 1..=3 {
        register_revision(&router, family, slot, case).await;
    }
    // Revision 3 is the one entry of every call below that CAN be deleted, so each row
    // proves the failure is per entry and not a refusal of the request.
    deregister(&router, &format!("{family}:3"), case).await;

    /// The `Failure$reason` for an entry that names nothing in this account: the ECS
    /// "API failure reasons" list's `MISSING`.
    const MISSING: &str = "MISSING";
    /// The `Failure$reason` for an entry naming a revision that was never deregistered:
    /// Amazon ECS names the status that blocks the delete and the step that clears it.
    const NOT_INACTIVE: &str = "The specified task definition is still in ACTIVE status. \
                                Please deregister the target and try again.";

    // The rows span the ways ONE entry can fail: a revision that is still ACTIVE (in
    // both reference forms), a revision number the family never minted, a revision of a
    // family that was never registered, and the same by ARN.
    let rows: [(&str, String, &str, String); 5] = [
        (
            "ACTIVE revision by family:revision",
            format!("{family}:1"),
            NOT_INACTIVE,
            definition_arn(family, 1),
        ),
        (
            "ACTIVE revision by ARN",
            definition_arn(family, 2),
            NOT_INACTIVE,
            definition_arn(family, 2),
        ),
        (
            "revision the family never minted",
            format!("{family}:9"),
            MISSING,
            definition_arn(family, 9),
        ),
        (
            "revision of a family that was never registered",
            "neverregistered:1".to_string(),
            MISSING,
            definition_arn("neverregistered", 1),
        ),
        (
            "ARN of a family that was never registered",
            definition_arn("neverregistered", 4),
            MISSING,
            definition_arn("neverregistered", 4),
        ),
    ];

    let before = describe(&router, &format!("{family}:1"), case).await;
    for (case, reference, reason, arn) in rows {
        // The failing entry travels WITH a deletable one, so the response has to report
        // both halves: the deletion that went through and the failure that did not.
        let (deleted, failures) =
            delete(&router, &[reference.clone(), format!("{family}:3")], case).await;
        assert_eq!(
            failures.len(),
            1,
            "case {case:?}: expected exactly one failure for {reference:?}, got {failures:?}"
        );
        assert_eq!(
            string(&failures[0], "reason", case),
            reason,
            "case {case:?}: unexpected failure reason for {reference:?}, got {:?}",
            failures[0]
        );
        assert_eq!(
            string(&failures[0], "arn", case),
            arn,
            "case {case:?}: expected the failure to name the ARN the entry pointed at, \
             got {:?}",
            failures[0]
        );
        assert_eq!(
            deleted.len(),
            1,
            "case {case:?}: expected the deregistered entry to be deleted anyway, got \
             {deleted:?}"
        );
        assert_eq!(
            count(&deleted[0], "revision", case),
            3,
            "case {case:?}: expected revision 3 to be the deleted one, got {deleted:?}"
        );
    }

    // A failed entry wrote nothing: the ACTIVE revisions are exactly as they were.
    for slot in 1..=2 {
        let still = describe(&router, &format!("{family}:{slot}"), "after the failures").await;
        assert_eq!(
            string(&still, "status", "after the failures"),
            "ACTIVE",
            "expected revision {slot} to be untouched by the failed entries, got {still}"
        );
        assert!(
            still.get("deleteRequestedAt").is_none(),
            "expected revision {slot} to carry no deleteRequestedAt, got {still}"
        );
    }
    let unchanged = describe(&router, &format!("{family}:1"), "after the failures").await;
    assert_eq!(
        unchanged, before,
        "expected the ACTIVE revision to be reported exactly as before the failed \
         entries, got {unchanged} for a stored {before}"
    );
    // A revision that never existed was not created by being named.
    let (code, message) = refusal(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": "neverregistered:1" }),
        "a MISSING entry creates nothing",
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "expected the unknown revision to still be unknown, got {code} with {message:?}"
    );
}

#[tokio::test]
async fn an_entry_that_names_no_one_revision_is_refused_and_deletes_nothing() {
    let router = service();
    let case = "setup";
    let family = "deleterefusalfamily";
    for slot in 1..=2 {
        register_revision(&router, family, slot, case).await;
    }
    for slot in 1..=2 {
        deregister(&router, &format!("{family}:{slot}"), case).await;
    }

    // Every row is a `ClientException` — the error for a request "specifying an identifier
    // that isn't valid" — because none of these entries names the ONE revision the call
    // acts on: "You must specify a revision." A row whose FAMILY part is not a family name
    // fails on the family first, and carries the fixed sentence Amazon ECS states for
    // that: nothing of the request reaches it.
    let eleven: Vec<String> = (1..=11).map(|slot| format!("{family}:{slot}")).collect();
    let arn_without_revision =
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}");
    const FAMILY_CHARACTERS: &str = "Family contains invalid characters.";
    /// The sentence a reference that names a family but no revision earns, for the
    /// `taskDefinitions` entry `reference`.
    fn needs_a_revision(reference: &str) -> String {
        format!(
            "taskDefinitions entries must each name a revision (family:revision or a \
             task-definition ARN); got {reference:?}"
        )
    }
    let rows: [(&str, Vec<String>, String); 6] = [
        (
            "family only",
            vec![family.to_string()],
            needs_a_revision(family),
        ),
        (
            "revision zero",
            vec![format!("{family}:0")],
            FAMILY_CHARACTERS.to_string(),
        ),
        (
            "family and the wildcard revision",
            vec![format!("{family}:*")],
            needs_a_revision(&format!("{family}:*")),
        ),
        (
            "ARN without a revision",
            vec![arn_without_revision.clone()],
            needs_a_revision(&arn_without_revision),
        ),
        (
            "not a task-definition reference at all",
            vec!["not a reference".to_string()],
            FAMILY_CHARACTERS.to_string(),
        ),
        (
            "a deletable entry beside one that names no revision",
            vec![format!("{family}:1"), family.to_string()],
            needs_a_revision(family),
        ),
    ];

    for (case, references, expected) in rows {
        let (code, message) = refusal(
            &router,
            "DeleteTaskDefinitions",
            json!({ "taskDefinitions": references.clone() }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected a ClientException for {references:?}, got {code} \
             with {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?} for {references:?}, got {message:?}"
        );
    }

    // The documented batch size is a property of the REQUEST rather than of one entry, so
    // its refusal states the entries and the bound instead of naming a member: "You can
    // specify up to 10 task definitions as a comma separated list."
    let (code, message) = refusal(
        &router,
        "DeleteTaskDefinitions",
        json!({ "taskDefinitions": eleven.clone() }),
        "eleven entries",
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "ClientException",
            "task definitions can have at most 10 items."
        ),
        "case \"eleven entries\": expected the batch-size refusal, got {code} with \
         {message:?}"
    );

    // A refusal deletes nothing: both revisions are still INACTIVE.
    for slot in 1..=2 {
        let still = describe(&router, &format!("{family}:{slot}"), "after the refusals").await;
        assert_eq!(
            string(&still, "status", "after the refusals"),
            "INACTIVE",
            "expected revision {slot} to be untouched by the refusals, got {still}"
        );
    }
}

#[tokio::test]
async fn a_delete_in_progress_revision_cannot_run_new_work_but_its_service_still_scales() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, "delcluster", case).await;
    let family = "deleteusagefamily";
    for slot in 1..=2 {
        register_revision(&router, family, slot, case).await;
    }
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": "delcluster",
            "serviceName": "running",
            "taskDefinition": format!("{family}:1"),
            "desiredCount": 1,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    deregister(&router, &format!("{family}:1"), case).await;
    let (deleted, failures) = delete(&router, &[format!("{family}:1")], case).await;
    assert!(
        failures.is_empty(),
        "case {case:?}: expected the revision the service runs to be deletable, got \
         {failures:?}"
    );
    assert_eq!(
        string(&deleted[0], "status", case),
        "DELETE_IN_PROGRESS",
        "case {case:?}: expected DELETE_IN_PROGRESS, got {deleted:?}"
    );

    // "You can't use a DELETE_IN_PROGRESS task definition revision to run new tasks or
    // create new services. You also can't update an existing service to reference a
    // DELETE_IN_PROGRESS task definition revision." The reference RESOLVES — the revision
    // is held, in a status the call may not use — so it is the bad parameter it is.
    let request_for = |operation: &str, reference: &str| -> Value {
        match operation {
            "RunTask" => json!({ "cluster": "delcluster", "taskDefinition": reference }),
            "CreateService" => json!({
                "cluster": "delcluster",
                "serviceName": format!("fresh{}", reference.len()),
                "taskDefinition": reference,
                "desiredCount": 1,
                "launchType": "EC2",
            }),
            "UpdateService" => json!({
                "cluster": "delcluster",
                "service": "running",
                "taskDefinition": reference,
            }),
            other => panic!("unexpected operation {other:?}"),
        }
    };
    for operation in ["RunTask", "CreateService", "UpdateService"] {
        for reference in [format!("{family}:1"), definition_arn(family, 1)] {
            let case = format!("{operation} on the DELETE_IN_PROGRESS revision {reference:?}");
            let (code, message) =
                refusal(&router, operation, request_for(operation, &reference), &case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException, got {code} with \
                 {message:?}"
            );
        }
    }

    // "Existing tasks and services that reference a DELETE_IN_PROGRESS task definition
    // revision continue to run without disruption. Existing services that reference a
    // DELETE_IN_PROGRESS task definition revision can still scale up or down by modifying
    // the service's desired count."
    for target in [2_i64, 0, 1] {
        let case = format!("scale a service on a DELETE_IN_PROGRESS revision to {target}");
        let updated = ok(
            &router,
            "UpdateService",
            json!({
                "cluster": "delcluster",
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
            definition_arn(family, 1),
            "case {case:?}: expected the service to keep its own revision, got {updated}"
        );
    }
}
