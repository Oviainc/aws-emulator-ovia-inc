// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDBSnapshot`, and the `DescribeDBSnapshots` /
//! `DescribeEvents` reads that follow it.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on.
//!
//! RULE under test: `DeleteDBSnapshot` removes the DB snapshot its `DBSnapshotIdentifier`
//! names from the Region the request was signed for and answers with the record it removed,
//! so every later read of that identifier is the modelled `DBSnapshotNotFoundFault` (HTTP
//! 404) — and the only snapshot it will remove is an `available` one: "The DB snapshot must
//! be in the available state to be deleted", so one still being taken is the modelled
//! `InvalidDBSnapshotStateFault` (HTTP 400). A snapshot the Region does not hold is that
//! same 404, whether it was never taken, was taken in another Region, or has already been
//! deleted — so the delete-then-wait-for-gone idiom terminates and a reaper that runs twice
//! is told the second pass found nothing.
//!
//! The rows span what the rule generalises over: HOW the snapshot was taken (a
//! `CreateDBSnapshot` a caller asked for, and the final snapshot `DeleteDBInstance` takes on
//! the way out — a snapshot that outlives its instance), the source instance's ENGINE, the
//! snapshot's STATE when the delete arrives (still being taken, and available), how MANY
//! snapshots of one instance are stored, and the REGION the delete is signed for.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, texts, wait_for_settle,
};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: [(&str, String); 0] = [];

/// How the snapshot a row deletes was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TakenBy {
    /// `CreateDBSnapshot`, the snapshot a caller asks for.
    ACaller,
    /// The final snapshot `DeleteDBInstance` takes on the way out, which outlives the
    /// instance it came from.
    TheInstancesDelete,
}

/// One snapshot taken and then deleted.
struct DeleteCase {
    case: &'static str,
    instance: &'static str,
    engine: &'static str,
    snapshot: &'static str,
    taken_by: TakenBy,
}

const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a snapshot a caller asked for, of a MySQL instance",
        instance: "emu-ds-mysql",
        engine: "mysql",
        snapshot: "emu-ds-mysql-snap",
        taken_by: TakenBy::ACaller,
    },
    DeleteCase {
        case: "a snapshot a caller asked for, of a PostgreSQL instance",
        instance: "emu-ds-postgres",
        engine: "postgres",
        snapshot: "emu-ds-postgres-snap",
        taken_by: TakenBy::ACaller,
    },
    DeleteCase {
        case: "the final snapshot a MySQL instance's own delete took",
        instance: "emu-ds-final-mysql",
        engine: "mysql",
        snapshot: "emu-ds-final-mysql-snap",
        taken_by: TakenBy::TheInstancesDelete,
    },
    DeleteCase {
        case: "the final snapshot a MariaDB instance's own delete took",
        instance: "emu-ds-final-mariadb",
        engine: "mariadb",
        snapshot: "emu-ds-final-mariadb-snap",
        taken_by: TakenBy::TheInstancesDelete,
    },
];

/// `CreateDBInstance` for a row's source instance, waited past the span the create owes so
/// the instance is `available` — the status a snapshot can be taken from.
async fn available_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
) {
    expect_ok(
        case,
        call(
            router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => identifier,
                "Engine" => engine,
                "DBInstanceClass" => "db.t3.micro",
            ],
        )
        .await,
    );
    wait_for_settle();
}

/// Take the row's snapshot, by whichever call the row says takes it, and answer the body
/// that call reported.
async fn take_the_snapshot(router: &MultiServiceRouter, row: &DeleteCase) -> String {
    match row.taken_by {
        TakenBy::ACaller => expect_ok(
            row.case,
            call(
                router,
                "CreateDBSnapshot",
                &params![
                    "DBInstanceIdentifier" => row.instance,
                    "DBSnapshotIdentifier" => row.snapshot,
                ],
            )
            .await,
        ),
        TakenBy::TheInstancesDelete => expect_ok(
            row.case,
            call(
                router,
                "DeleteDBInstance",
                &params![
                    "DBInstanceIdentifier" => row.instance,
                    "FinalDBSnapshotIdentifier" => row.snapshot,
                ],
            )
            .await,
        ),
    }
}

/// The identifiers of every DB snapshot the Region holds, in the order it reports them.
async fn stored_snapshots(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let body = expect_ok(case, call(router, "DescribeDBSnapshots", &NO_PARAMS).await);
    texts(&body, "DBSnapshotIdentifier")
}

#[tokio::test]
async fn a_delete_removes_an_available_snapshot_and_refuses_one_still_being_taken() {
    for row in DELETE_CASES {
        // Each row gets its own emulator, so the timing of one row's transitions cannot
        // carry into the next.
        let router = emulator();
        available_instance(&router, row.case, row.instance, row.engine).await;
        take_the_snapshot(&router, row).await;

        // Inside the span the clock owes the snapshot it is still being taken, and the
        // delete is the modelled state fault rather than a removal: "The DB snapshot must
        // be in the available state to be deleted."
        let refused = expect_error(
            row.case,
            call(
                &router,
                "DeleteDBSnapshot",
                &params!["DBSnapshotIdentifier" => row.snapshot],
            )
            .await,
            400,
            "InvalidDBSnapshotState",
        );
        assert!(
            text(&refused, "Message").contains(row.snapshot),
            "case {:?}: the refusal must name the snapshot, got {:?}",
            row.case,
            text(&refused, "Message")
        );
        assert_eq!(
            stored_snapshots(&router, row.case).await,
            vec![row.snapshot.to_string()],
            "case {:?}: a refused delete must leave the snapshot where it was",
            row.case
        );

        wait_for_settle();

        // The delete of the available snapshot answers the record it removed, as it stood.
        let deleted = expect_ok(
            row.case,
            call(
                &router,
                "DeleteDBSnapshot",
                &params!["DBSnapshotIdentifier" => row.snapshot],
            )
            .await,
        );
        for (member, expected) in [
            ("DBSnapshotIdentifier", row.snapshot),
            ("DBInstanceIdentifier", row.instance),
            ("Engine", row.engine),
            ("SnapshotType", "manual"),
            ("Status", "available"),
        ] {
            assert_eq!(
                text(&deleted, member),
                expected,
                "case {:?}: expected the deleted snapshot's {member} to be {expected:?}, \
                 got {deleted}",
                row.case
            );
        }

        // And then it is gone: the read of the identifier, the unfiltered report and a
        // second delete all agree the identifier names nothing.
        let not_found = expect_error(
            row.case,
            call(
                &router,
                "DescribeDBSnapshots",
                &params!["DBSnapshotIdentifier" => row.snapshot],
            )
            .await,
            404,
            "DBSnapshotNotFound",
        );
        assert!(
            text(&not_found, "Message").contains(row.snapshot),
            "case {:?}: the read's refusal must name the snapshot, got {:?}",
            row.case,
            text(&not_found, "Message")
        );
        assert!(
            stored_snapshots(&router, row.case).await.is_empty(),
            "case {:?}: the deleted snapshot must be gone from the unfiltered report",
            row.case
        );
        let again = expect_error(
            row.case,
            call(
                &router,
                "DeleteDBSnapshot",
                &params!["DBSnapshotIdentifier" => row.snapshot],
            )
            .await,
            404,
            "DBSnapshotNotFound",
        );
        assert!(
            text(&again, "Message").contains(row.snapshot),
            "case {:?}: a second delete must name the snapshot it did not find, got {:?}",
            row.case,
            text(&again, "Message")
        );

        // The Region's event log carries the deletion, under the published source type and
        // category of a manual DB snapshot's deletion.
        let events = expect_ok(
            row.case,
            call(
                &router,
                "DescribeEvents",
                &params![
                    "SourceType" => "db-snapshot",
                    "SourceIdentifier" => row.snapshot,
                ],
            )
            .await,
        );
        assert!(
            texts(&events, "Message")
                .iter()
                .any(|message| message == "Manual snapshot deleted"),
            "case {:?}: expected the snapshot's deletion in the event log, got {events}",
            row.case
        );
        assert!(
            texts(&events, "EventCategory")
                .iter()
                .any(|category| category == "deletion"),
            "case {:?}: expected the deletion category on that event, got {events}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_delete_removes_only_the_snapshot_it_names_in_the_region_it_is_signed_for() {
    let case = "three snapshots of one instance, deleted one at a time";
    let router = emulator();
    let instance = "emu-ds-several";
    available_instance(&router, case, instance, "mysql").await;
    let snapshots = ["emu-ds-several-a", "emu-ds-several-b", "emu-ds-several-c"];
    for snapshot in snapshots {
        expect_ok(
            case,
            call(
                &router,
                "CreateDBSnapshot",
                &params![
                    "DBInstanceIdentifier" => instance,
                    "DBSnapshotIdentifier" => snapshot,
                ],
            )
            .await,
        );
    }
    wait_for_settle();

    // A Region the snapshot was NOT taken in holds no record of it, so the same delete
    // there is the modelled 404 and removes nothing: "RDS is regional" is the rule, and a
    // request's credential scope is what decides which Region it acts in.
    let elsewhere = expect_error(
        case,
        call_in(
            &router,
            "eu-west-1",
            "DeleteDBSnapshot",
            &params!["DBSnapshotIdentifier" => snapshots[0]],
        )
        .await,
        404,
        "DBSnapshotNotFound",
    );
    assert!(
        text(&elsewhere, "Message").contains(snapshots[0]),
        "case {case:?}: the other Region's refusal must name the snapshot, got {elsewhere}"
    );

    // Each delete removes exactly the snapshot it names, leaving the rest of the Region's
    // snapshots of the same instance where they were.
    let mut left: Vec<String> = snapshots.iter().map(|name| name.to_string()).collect();
    for snapshot in snapshots {
        expect_ok(
            case,
            call(
                &router,
                "DeleteDBSnapshot",
                &params!["DBSnapshotIdentifier" => snapshot],
            )
            .await,
        );
        left.retain(|name| name != snapshot);
        let body = expect_ok(
            case,
            call(
                &router,
                "DescribeDBSnapshots",
                &params!["DBInstanceIdentifier" => instance],
            )
            .await,
        );
        assert_eq!(
            texts(&body, "DBSnapshotIdentifier"),
            left,
            "case {case:?}: after deleting {snapshot} the Region must hold exactly \
             {left:?}, got {body}"
        );
    }

    // The instance the snapshots were taken of is untouched by their deletion: deleting a
    // snapshot deletes the snapshot and nothing else.
    let body = expect_ok(
        case,
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => instance],
        )
        .await,
    );
    let instances = subtrees(&body, "DBInstance");
    assert_eq!(
        instances.len(),
        1,
        "case {case:?}: the source instance must still be there, got {body}"
    );
    assert_eq!(
        text(&instances[0], "DBInstanceStatus"),
        "available",
        "case {case:?}: the source instance must still be available, got {body}"
    );
}
