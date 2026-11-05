// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB-instance DELETE lifecycle: `DeleteDBInstance` and the
//! `DescribeDBInstances` / `DescribeDBSnapshots` reads that follow it.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting
//! helper this test target has, as a real awsQuery form POST carrying the SigV4
//! credential scope the router keys on.
//!
//! The rule under test: `DeleteDBInstance` accepts the delete of a stored DB instance
//! whatever status it is in, with the sole exceptions the operation documents — an
//! instance whose deletion protection is enabled, one whose delete is already running
//! ("This operation can't be canceled or reverted after it begins"), and, only when a
//! final snapshot is asked for, one in a failure state. An accepted delete answers HTTP
//! 200 reporting the instance as `deleting`, and the instance is then removed, so later
//! reads of the identifier are the modelled `DBInstanceNotFoundFault` (HTTP 404) — the
//! delete-then-wait-for-gone idiom terminates.
//!
//! The table below spans what the rule generalises over: the status the instance is in
//! when the delete arrives (a provisioning status the clock has yet to settle, a settled
//! one, and one a modify put it back into), both documented final-snapshot forms
//! (`SkipFinalSnapshot` and a `FinalDBSnapshotIdentifier`), and several engine/class
//! combinations. The failure statuses (`failed`, `incompatible-restore`,
//! `incompatible-network`), which a final snapshot cannot be taken from, are the one
//! class no implemented operation of this emulator can put an instance in, so there is
//! no wire input that reaches them.

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text, texts};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// How long a test waits before expecting a change this service owes to have settled:
/// the span itself plus slack for a loaded test machine.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// The status the instance is in when the delete arrives, and how the test gets it
/// there. Every one of them must accept the delete.
#[derive(Debug, Clone, Copy)]
enum StartingStatus {
    /// Straight after `CreateDBInstance`, before the clock has settled the instance:
    /// the status the recorded teardown deleted from.
    Creating,
    /// After the create has settled.
    Available,
    /// Straight after a `ModifyDBInstance` that has not settled either.
    Modifying,
}

impl StartingStatus {
    /// The `DBInstanceStatus` the instance reports at this point.
    fn reported(self) -> &'static str {
        match self {
            StartingStatus::Creating => "creating",
            StartingStatus::Available => "available",
            StartingStatus::Modifying => "modifying",
        }
    }
}

/// One row of the delete table.
struct DeleteCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
    status: StartingStatus,
    /// `Some(identifier)` asks for the final snapshot; `None` skips it.
    final_snapshot: Option<&'static str>,
}

/// `CreateDBInstance` with its three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    class: &str,
    extra: &[(&str, String)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => engine,
        "DBInstanceClass" => class,
    ]
    .to_vec();
    request.extend(extra.iter().cloned());
    call(router, "CreateDBInstance", &request).await
}

/// The `DBInstanceStatus` `DescribeDBInstances` reports for `identifier`, or `None`
/// when the read is the modelled 404.
async fn reported_status(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => identifier],
    )
    .await;
    if status == 404 {
        return None;
    }
    assert_eq!(
        status, 200,
        "reading {identifier} must be a 200 or the modelled 404, got {status} with body {body}"
    );
    let instances = subtrees(&body, "DBInstance");
    assert_eq!(
        instances.len(),
        1,
        "reading {identifier} must report exactly one DBInstance, got {}",
        instances.len()
    );
    Some(text(&instances[0], "DBInstanceStatus"))
}

#[tokio::test]
async fn a_delete_is_accepted_from_every_status_and_the_instance_then_disappears() {
    let table: &[DeleteCase] = &[
        DeleteCase {
            case: "still creating, skipping the final snapshot",
            identifier: "emu-dd-creating-skip",
            engine: "postgres",
            class: "db.t3.micro",
            status: StartingStatus::Creating,
            final_snapshot: None,
        },
        DeleteCase {
            case: "still creating, taking the final snapshot",
            identifier: "emu-dd-creating-final",
            engine: "mysql",
            class: "db.m5.large",
            status: StartingStatus::Creating,
            final_snapshot: Some("emu-dd-creating-final-snap"),
        },
        DeleteCase {
            case: "available, skipping the final snapshot",
            identifier: "emu-dd-available-skip",
            engine: "mariadb",
            class: "db.t3.small",
            status: StartingStatus::Available,
            final_snapshot: None,
        },
        DeleteCase {
            case: "available, taking the final snapshot",
            identifier: "emu-dd-available-final",
            engine: "postgres",
            class: "db.r5.large",
            status: StartingStatus::Available,
            final_snapshot: Some("emu-dd-available-final-snap"),
        },
        DeleteCase {
            case: "modifying, skipping the final snapshot",
            identifier: "emu-dd-modifying-skip",
            engine: "oracle-se2",
            class: "db.m5.xlarge",
            status: StartingStatus::Modifying,
            final_snapshot: None,
        },
        DeleteCase {
            case: "modifying, taking the final snapshot",
            identifier: "emu-dd-modifying-final",
            engine: "sqlserver-ex",
            class: "db.t3.medium",
            status: StartingStatus::Modifying,
            final_snapshot: Some("emu-dd-modifying-final-snap"),
        },
    ];

    for row in table {
        // Each row gets its own emulator, so the timing of one row's transitions
        // cannot carry into the next.
        let router = emulator();
        expect_ok(
            row.case,
            create_instance(
                &router,
                row.identifier,
                row.engine,
                row.class,
                &params!["AllocatedStorage" => 20, "MasterUsername" => "emuadmin"],
            )
            .await,
        );
        match row.status {
            StartingStatus::Creating => {}
            StartingStatus::Available => wait_for_settle(),
            StartingStatus::Modifying => {
                wait_for_settle();
                expect_ok(
                    row.case,
                    call(
                        &router,
                        "ModifyDBInstance",
                        &params![
                            "DBInstanceIdentifier" => row.identifier,
                            "AllocatedStorage" => 50,
                        ],
                    )
                    .await,
                );
            }
        }
        let before = reported_status(&router, row.identifier).await;
        assert_eq!(
            before.as_deref(),
            Some(row.status.reported()),
            "case {:?}: the instance must be deleted from status {:?}, got {before:?}",
            row.case,
            row.status.reported()
        );

        let mut request: Vec<(&str, String)> =
            params!["DBInstanceIdentifier" => row.identifier].to_vec();
        match row.final_snapshot {
            Some(snapshot) => request.push(("FinalDBSnapshotIdentifier", snapshot.to_string())),
            None => request.push(("SkipFinalSnapshot", "true".to_string())),
        }
        let accepted = expect_ok(row.case, call(&router, "DeleteDBInstance", &request).await);
        let reported = subtrees(&accepted, "DBInstance");
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the delete must report the instance it accepted, got {accepted}",
            row.case
        );
        let status_in_response = text(&reported[0], "DBInstanceStatus");
        assert_eq!(
            status_in_response, "deleting",
            "case {:?}: an accepted delete must report `deleting`, got {status_in_response:?}",
            row.case
        );

        // Inside the span the clock owes, the instance is still readable and reports
        // the same transient status the delete answered with.
        let during = reported_status(&router, row.identifier).await;
        assert_eq!(
            during.as_deref(),
            Some("deleting"),
            "case {:?}: a read inside the deleting window must report `deleting`, got {during:?}",
            row.case
        );

        wait_for_settle();

        // And then it is gone: the read, the unfiltered report and a second delete all
        // agree the identifier names nothing.
        let after = reported_status(&router, row.identifier).await;
        assert!(
            after.is_none(),
            "case {:?}: the instance must be gone once the delete has run, got {after:?}",
            row.case
        );
        let listed = expect_ok(row.case, call(&router, "DescribeDBInstances", &[] as &[(&str, String)]).await);
        let identifiers = subtrees(&listed, "DBInstance")
            .iter()
            .map(|instance| text(instance, "DBInstanceIdentifier"))
            .collect::<Vec<_>>();
        assert!(
            !identifiers.contains(&row.identifier.to_string()),
            "case {:?}: the deleted instance must be gone from the unfiltered report, got {identifiers:?}",
            row.case
        );
        let body = expect_error(
            row.case,
            call(&router, "DeleteDBInstance", &request).await,
            404,
            "DBInstanceNotFound",
        );
        assert!(
            text(&body, "Message").contains(row.identifier),
            "case {:?}: the refusal must name the identifier, got {:?}",
            row.case,
            text(&body, "Message")
        );

        // The final snapshot the delete was asked for outlives the instance; skipping
        // it stores nothing.
        let snapshots = expect_ok(
            row.case,
            call(&router, "DescribeDBSnapshots", &[] as &[(&str, String)]).await,
        );
        let stored = texts(&snapshots, "DBSnapshotIdentifier");
        match row.final_snapshot {
            Some(snapshot) => {
                assert_eq!(
                    stored,
                    vec![snapshot.to_string()],
                    "case {:?}: the final snapshot must outlive the instance, got {stored:?}",
                    row.case
                );
                let subtree = &subtrees(&snapshots, "DBSnapshot")[0];
                assert_eq!(
                    text(subtree, "DBInstanceIdentifier"),
                    row.identifier,
                    "case {:?}: the final snapshot must name the instance it came from",
                    row.case
                );
                assert_eq!(
                    text(subtree, "Engine"),
                    row.engine,
                    "case {:?}: the final snapshot must copy the instance's engine",
                    row.case
                );
            }
            None => assert!(
                stored.is_empty(),
                "case {:?}: skipping the final snapshot must store none, got {stored:?}",
                row.case
            ),
        }
    }
}

#[tokio::test]
async fn the_only_refusals_are_deletion_protection_and_a_delete_already_running() {
    let router = emulator();
    expect_ok(
        "create the protected instance",
        create_instance(
            &router,
            "emu-dd-protected",
            "postgres",
            "db.t3.micro",
            &params!["DeletionProtection" => true],
        )
        .await,
    );
    expect_ok(
        "create the instance deleted twice",
        create_instance(&router, "emu-dd-twice", "mysql", "db.t3.small", &[]).await,
    );
    wait_for_settle();

    // Deletion protection holds the instance whichever final-snapshot form is asked
    // for, and leaves it exactly as it was. It is a setting the request conflicts with
    // rather than a state the instance is passing through, so the refusal carries the
    // Query API's parameter-combination code and the fixed sentence of that class.
    for (case, extra) in [
        ("protected, skipping the final snapshot", ("SkipFinalSnapshot", "true")),
        (
            "protected, taking the final snapshot",
            ("FinalDBSnapshotIdentifier", "emu-dd-protected-snap"),
        ),
    ] {
        let body = expect_error(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &params!["DBInstanceIdentifier" => "emu-dd-protected", extra.0 => extra.1],
            )
            .await,
            400,
            "InvalidParameterCombination",
        );
        assert_eq!(
            text(&body, "Message"),
            "Cannot delete protected DB Instance, please disable deletion protection and try \
             again.",
            "case {case:?}: the refusal must carry the sentence this class is worded with, \
             got {:?}",
            text(&body, "Message")
        );
        let still_there = reported_status(&router, "emu-dd-protected").await;
        assert_eq!(
            still_there.as_deref(),
            Some("available"),
            "case {case:?}: a refused delete must leave the instance as it was, got {still_there:?}"
        );
    }

    // A delete that is already running cannot be started again: it "can't be canceled
    // or reverted after it begins".
    expect_ok(
        "the first delete",
        call(
            &router,
            "DeleteDBInstance",
            &params!["DBInstanceIdentifier" => "emu-dd-twice", "SkipFinalSnapshot" => true],
        )
        .await,
    );
    let body = expect_error(
        "a second delete inside the deleting window",
        call(
            &router,
            "DeleteDBInstance",
            &params!["DBInstanceIdentifier" => "emu-dd-twice", "SkipFinalSnapshot" => true],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("emu-dd-twice") && message.contains("deleting"),
        "the refusal must say the delete is already running, got {message:?}"
    );
}
