// SPDX-License-Identifier: CC0-1.0

//! Deleting a daemon task-definition revision.
//!
//! The rules under test, each asserted over a table that spans its class rather than over
//! one request:
//!
//! * `DeleteDaemonTaskDefinition`: "Deletes the specified daemon task definition. ... A
//!   daemon task definition must be in an ACTIVE state to be deleted" — the call acts on
//!   the ONE revision its reference names, answers with that revision's stored ARN, and
//!   leaves every other revision where it was;
//! * `daemonTaskDefinition`: "The family and revision (family:revision) or full Amazon
//!   Resource Name (ARN) of the daemon task definition to delete" — every form that names
//!   one revision reaches the same record, and a value that names a whole family or no
//!   stored revision at all is refused and deletes nothing;
//! * `DaemonTaskDefinition$status`/`$deleteRequestedAt`: a requested delete puts the
//!   revision into `DELETE_IN_PROGRESS` with the instant it was requested, and the delete
//!   completes on the clock, so the revision reaches `DELETED` without anybody calling;
//! * a revision whose delete was already asked for is reported as it stands: the repeated
//!   call has nothing left to start, so it answers with the same revision and neither
//!   resurrects it nor re-dates its deletion, and the family on its own resolves to "the
//!   latest ACTIVE revision in that family", so a deleted revision is no longer what the
//!   family names;
//! * "Each time that you register a new revision of a daemon task definition in the same
//!   family, the revision value always increases by one" — a delete frees no revision
//!   number;
//! * `ListDaemonTaskDefinitions$status`: "By default, only ACTIVE daemon task definitions
//!   are listed. If you set this parameter to DELETE_IN_PROGRESS, only daemon task
//!   definitions that are in the process of being deleted are listed" — a listing reports
//!   the status each revision stands in at that instant, so a deleted revision leaves the
//!   default listing and the deletion's completion is visible there too.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{ok, refusal, service, string, ACCOUNT, SIGNED_REGION};

/// The statuses the model defines for a daemon task-definition revision
/// (`smithy_query(query="shape", shape="DaemonTaskDefinitionStatus")`), as this crate
/// generated them — so the catalog under test is the model's and not a copy of it.
use service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::DAEMON_TASK_DEFINITION_STATUS_VALUES;

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) to fall
/// due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The image every container here starts from; irrelevant to every rule, so one value.
const IMAGE: &str = "public.ecr.aws/aws-observability/aws-otel-collector:latest";

/// The ARN a daemon revision of `family` carries, as
/// `arn:aws:ecs:<region>:<account>:daemon-task-definition/<family>:<revision>`.
fn daemon_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-task-definition/{family}:{revision}")
}

/// The ARN a PLAIN task-definition revision of `family` carries — the other resource,
/// used only to show that neither kind's reference reaches the other's record.
fn plain_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

/// Register one daemon revision of `family`, named after `slot` so revisions are told
/// apart by their CONTENT and not only by their number, and return its ARN.
async fn register(router: &MultiServiceRouter, family: &str, slot: i64, case: &str) -> String {
    let body = ok(
        router,
        "RegisterDaemonTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [
                { "name": format!("agent{slot}"), "image": IMAGE, "memory": 128 },
            ],
        }),
        case,
    )
    .await;
    string(&body, "daemonTaskDefinitionArn", case)
}

/// `DescribeDaemonTaskDefinition` for one reference, as the reader a delete is proved
/// through.
async fn describe(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeDaemonTaskDefinition",
        json!({ "daemonTaskDefinition": reference }),
        case,
    )
    .await;
    match body.get("daemonTaskDefinition") {
        Some(value) if value.is_object() => value.clone(),
        other => {
            panic!("case {case:?}: expected a daemonTaskDefinition structure, got {other:?}")
        }
    }
}

/// `DeleteDaemonTaskDefinition` for one reference, returning the ARN it answered with.
async fn delete(router: &MultiServiceRouter, reference: &str, case: &str) -> String {
    let body = ok(
        router,
        "DeleteDaemonTaskDefinition",
        json!({ "daemonTaskDefinition": reference }),
        case,
    )
    .await;
    string(&body, "daemonTaskDefinitionArn", case)
}

/// The `status` a reference reports, checked against the model's own catalog so a status
/// outside the enum fails here rather than being read as a pass.
async fn status(router: &MultiServiceRouter, reference: &str, case: &str) -> String {
    let view = describe(router, reference, case).await;
    let reported = string(&view, "status", case);
    assert!(
        DAEMON_TASK_DEFINITION_STATUS_VALUES.contains(&reported.as_str()),
        "case {case:?}: expected a modelled status out of {DAEMON_TASK_DEFINITION_STATUS_VALUES:?}, \
         got {reported:?} in {view}"
    );
    reported
}

/// The `deleteRequestedAt` a reference reports, if any.
async fn delete_requested_at(
    router: &MultiServiceRouter,
    reference: &str,
    case: &str,
) -> Option<f64> {
    describe(router, reference, case)
        .await
        .get("deleteRequestedAt")
        .and_then(Value::as_f64)
}

/// One row of the deletion table: which revision of a three-revision family is deleted,
/// and how the request names it.
struct Row {
    case: &'static str,
    family: &'static str,
    /// The revision the row deletes, out of the three its family holds.
    revision: i64,
    /// How the request spells that revision: `family:revision`, or its full ARN.
    reference: fn(&str, i64) -> String,
}

/// How many revisions each row's family holds, so a row can delete the first, a middle
/// one or the latest and the suite can assert the others are untouched.
const REVISIONS: i64 = 3;

#[tokio::test]
async fn a_delete_moves_only_the_named_revision_through_the_documented_statuses() {
    let router = service();

    // The rows differ along what the rule generalises over: WHICH revision of a family is
    // deleted (the first, a middle one, the latest) and the FORM the reference takes.
    let rows = [
        Row {
            case: "the first revision, by family:revision",
            family: "daemondelete-first",
            revision: 1,
            reference: |family, revision| format!("{family}:{revision}"),
        },
        Row {
            case: "a middle revision, by family:revision",
            family: "daemondelete-middle",
            revision: 2,
            reference: |family, revision| format!("{family}:{revision}"),
        },
        Row {
            case: "the latest revision, by family:revision",
            family: "daemondelete-latest",
            revision: REVISIONS,
            reference: |family, revision| format!("{family}:{revision}"),
        },
        Row {
            case: "the first revision, by its full ARN",
            family: "daemondelete-arn-first",
            revision: 1,
            reference: daemon_arn,
        },
        Row {
            case: "the latest revision, by its full ARN",
            family: "daemondelete-arn-latest",
            revision: REVISIONS,
            reference: daemon_arn,
        },
    ];

    for row in &rows {
        let case = row.case;
        for slot in 1..=REVISIONS {
            register(&router, row.family, slot, case).await;
        }
        let reference = (row.reference)(row.family, row.revision);
        let registered_at = describe(&router, &reference, case)
            .await
            .get("registeredAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a registeredAt"));

        // "The full Amazon Resource Name (ARN) of the deleted daemon task definition":
        // the stored revision's own ARN, whichever form the request named it by.
        let answered = delete(&router, &reference, case).await;
        assert_eq!(
            answered,
            daemon_arn(row.family, row.revision),
            "case {case:?}: expected the deleted revision's stored ARN back, got {answered:?}"
        );

        // The delete is under way from the instant it was requested, and the revision
        // carries that instant — a caller reading straight after the call sees it.
        let in_progress = status(&router, &reference, case).await;
        assert_eq!(
            in_progress, "DELETE_IN_PROGRESS",
            "case {case:?}: expected the requested delete to be under way, got \
             {in_progress:?}"
        );
        let requested_at = delete_requested_at(&router, &reference, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected a deleteRequestedAt"));
        assert!(
            requested_at >= registered_at,
            "case {case:?}: expected the delete to be requested no earlier than the \
             registration at {registered_at}, got {requested_at}"
        );

        // Every OTHER revision of the family is untouched: the call deletes the one
        // revision its reference named.
        for slot in (1..=REVISIONS).filter(|slot| *slot != row.revision) {
            let neighbour = format!("{}:{slot}", row.family);
            let neighbour_status = status(&router, &neighbour, case).await;
            assert_eq!(
                neighbour_status, "ACTIVE",
                "case {case:?}: expected revision {slot} of {:?} to stay ACTIVE, got \
                 {neighbour_status:?}",
                row.family
            );
            assert!(
                delete_requested_at(&router, &neighbour, case)
                    .await
                    .is_none(),
                "case {case:?}: no delete was asked for revision {slot} of {:?}, so it \
                 carries no deleteRequestedAt",
                row.family
            );
        }

        // "You can simply specify the family to find the latest ACTIVE revision in that
        // family": the family no longer names a revision whose delete was requested.
        let latest = describe(&router, row.family, case).await;
        let expected_latest = if row.revision == REVISIONS {
            REVISIONS - 1
        } else {
            REVISIONS
        };
        assert_eq!(
            latest["revision"].as_i64(),
            Some(expected_latest),
            "case {case:?}: expected the family to resolve to revision {expected_latest}, \
             got {latest}"
        );

        // The delete completes on the clock: nobody has to call for the revision to reach
        // the terminal status, and the instant the delete was requested does not move.
        tokio::time::sleep(ONE_SPAN).await;
        let deleted = status(&router, &reference, case).await;
        assert_eq!(
            deleted, "DELETED",
            "case {case:?}: expected the delete to have completed on the clock, got \
             {deleted:?}"
        );
        let after = delete_requested_at(&router, &reference, case)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected a deleteRequestedAt"));
        assert_eq!(
            after, requested_at,
            "case {case:?}: expected the instant the delete was requested to stand, got \
             {after} against {requested_at}"
        );
    }
}

#[tokio::test]
async fn deleting_a_revision_whose_delete_was_already_asked_for_changes_nothing() {
    let router = service();
    const FAMILY: &str = "daemonredelete";
    register(&router, FAMILY, 1, "the revision").await;
    let reference = format!("{FAMILY}:1");
    let arn = delete(&router, &reference, "the first delete").await;
    let requested_at = delete_requested_at(&router, &reference, "the first delete")
        .await
        .expect("the first delete recorded when it was requested");

    // Both statuses a requested delete leads through are out of ACTIVE, so in each of
    // them the repeated call has nothing left to start: it reports the same revision and
    // leaves the deletion already under way exactly where it was.
    for (case, wait) in [
        ("while the delete is under way", Duration::ZERO),
        ("once the delete has completed", ONE_SPAN),
    ] {
        tokio::time::sleep(wait).await;
        let before = status(&router, &reference, case).await;
        assert_ne!(
            before, "ACTIVE",
            "case {case:?}: expected the revision to be out of ACTIVE before the repeated \
             delete, got {before:?}"
        );
        let repeated = delete(&router, &reference, case).await;
        assert_eq!(
            repeated, arn,
            "case {case:?}: expected the repeated delete to report the same revision, got \
             {repeated:?} against {arn:?}"
        );
        let after = status(&router, &reference, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a repeated delete moves no status, got {after:?} against \
             {before:?}"
        );
        assert_eq!(
            delete_requested_at(&router, &reference, case).await,
            Some(requested_at),
            "case {case:?}: a repeated delete does not re-date the delete already \
             requested"
        );
    }

    // And the repeated calls did not extend the deletion: the revision still reaches the
    // terminal status on the clock the FIRST delete started.
    let terminal = status(&router, &reference, "after the repeated deletes").await;
    assert_eq!(
        terminal, "DELETED",
        "expected the deletion the first call started to have completed, got {terminal:?}"
    );
}

#[tokio::test]
async fn a_reference_that_names_no_one_stored_revision_is_refused_and_deletes_nothing() {
    let router = service();
    const FAMILY: &str = "daemondeleterefusals";
    for slot in 1..=2 {
        register(&router, FAMILY, slot, "the control revisions").await;
    }
    // A PLAIN task definition registered under the SAME family name: a different
    // resource, so neither kind's reference may reach the other's record.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [{ "name": "app", "image": IMAGE, "memory": 128 }],
        }),
        "the plain revision",
    )
    .await;

    // The rows span the ways a reference can fail to name ONE stored daemon revision:
    // a family with no revision at all, a family that names every revision, a revision
    // outside the family, a value that is no reference, and the other resource's ARN.
    let rows = [
        ("a family nothing was registered under", "neverregistered".to_string()),
        ("the family on its own", FAMILY.to_string()),
        ("the family naming every revision", format!("{FAMILY}:*")),
        ("a revision the family never minted", format!("{FAMILY}:9")),
        ("revision zero", format!("{FAMILY}:0")),
        ("not a reference at all", "not a reference".to_string()),
        ("the daemon ARN of another family", daemon_arn("neverregistered", 1)),
        (
            "the ARN of the plain revision of the same family",
            plain_arn(FAMILY, 1),
        ),
    ];
    for (case, reference) in rows {
        let (code, message) = refusal(
            &router,
            "DeleteDaemonTaskDefinition",
            json!({ "daemonTaskDefinition": reference.clone() }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected a ClientException for {reference:?}, got {code} with \
             {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got {message:?}"
        );
    }

    // Nothing was deleted: both daemon revisions are still ACTIVE, and so is the plain
    // revision of the same family name.
    for slot in 1..=2 {
        let reference = format!("{FAMILY}:{slot}");
        let reported = status(&router, &reference, "the control reads").await;
        assert_eq!(
            reported, "ACTIVE",
            "expected revision {slot} of {FAMILY:?} to be untouched, got {reported:?}"
        );
    }
    let plain = ok(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": FAMILY }),
        "the control reads",
    )
    .await;
    assert_eq!(
        string(&plain["taskDefinition"], "status", "the control reads"),
        "ACTIVE",
        "expected the plain revision of {FAMILY:?} to be untouched, got {plain}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_status_a_deleted_revision_stands_in() {
    let router = service();
    const FAMILY: &str = "daemondeletelisting";
    for slot in 1..=3 {
        register(&router, FAMILY, slot, "the revisions").await;
    }
    delete(&router, &format!("{FAMILY}:2"), "the delete").await;

    // The rows are the listings a client can ask for, each one narrowed by the status
    // filter the model defines, and each answered from the statuses the store holds at
    // that instant.
    async fn arns(router: &MultiServiceRouter, status: Option<&str>, case: &str) -> Vec<String> {
        let mut request = json!({ "family": FAMILY });
        if let Some(status) = status {
            request["status"] = json!(status);
        }
        let body = ok(router, "ListDaemonTaskDefinitions", request, case).await;
        crate::ecs_wire::list(&body, "daemonTaskDefinitions", case)
            .iter()
            .map(|summary| string(summary, "arn", case))
            .collect()
    }

    // "By default, only ACTIVE daemon task definitions are listed."
    let case = "the default listing while the delete is under way";
    let active = arns(&router, None, case).await;
    assert_eq!(
        active,
        vec![daemon_arn(FAMILY, 1), daemon_arn(FAMILY, 3)],
        "case {case:?}: expected only the revisions still ACTIVE, got {active:?}"
    );
    // "If you set this parameter to DELETE_IN_PROGRESS, only daemon task definitions that
    // are in the process of being deleted are listed."
    let case = "the in-progress listing while the delete is under way";
    let deleting = arns(&router, Some("DELETE_IN_PROGRESS"), case).await;
    assert_eq!(
        deleting,
        vec![daemon_arn(FAMILY, 2)],
        "case {case:?}: expected the revision whose delete is under way, got {deleting:?}"
    );

    // Once the deletion completes on the clock, the same two listings report it: the
    // revision is in neither, and only the listing that asks for every status holds it.
    tokio::time::sleep(ONE_SPAN).await;
    let case = "the in-progress listing once the delete has completed";
    let deleting = arns(&router, Some("DELETE_IN_PROGRESS"), case).await;
    assert!(
        deleting.is_empty(),
        "case {case:?}: expected no delete still under way, got {deleting:?}"
    );
    let case = "the listing of every status";
    let all = arns(&router, Some("ALL"), case).await;
    assert_eq!(
        all,
        vec![
            daemon_arn(FAMILY, 1),
            daemon_arn(FAMILY, 2),
            daemon_arn(FAMILY, 3)
        ],
        "case {case:?}: expected every stored revision regardless of status, got {all:?}"
    );
}

#[tokio::test]
async fn a_delete_frees_no_revision_number_of_its_family() {
    let router = service();

    // The families differ in how many revisions are registered and deleted before the
    // next registration, so a revision number can come neither from the count of live
    // revisions nor from the order of the calls.
    let rows: [(&str, &str, i64, &[i64]); 4] = [
        ("one revision, deleted", "daemonnumbers-one", 1, &[1]),
        (
            "three revisions, the latest deleted",
            "daemonnumbers-latest",
            3,
            &[3],
        ),
        (
            "three revisions, the first deleted",
            "daemonnumbers-first",
            3,
            &[1],
        ),
        (
            "three revisions, all deleted",
            "daemonnumbers-all",
            3,
            &[1, 2, 3],
        ),
    ];
    for (case, family, registered, deleted) in rows {
        for slot in 1..=registered {
            register(&router, family, slot, case).await;
        }
        for slot in deleted {
            delete(&router, &format!("{family}:{slot}"), case).await;
        }
        // "Each time that you register a new revision ... the revision value always
        // increases by one": the next registration takes the number after the highest
        // ever minted, whatever has since been deleted.
        let next = register(&router, family, registered + 1, case).await;
        assert_eq!(
            next,
            daemon_arn(family, registered + 1),
            "case {case:?}: expected the next registration of {family:?} to mint revision \
             {}, got {next:?}",
            registered + 1
        );
        // And the family resolves to that newest revision, the only ACTIVE one when every
        // earlier revision was deleted.
        let latest = describe(&router, family, case).await;
        assert_eq!(
            latest["revision"].as_i64(),
            Some(registered + 1),
            "case {case:?}: expected the family to resolve to revision {}, got {latest}",
            registered + 1
        );
    }
}
