// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for deleting a DB instance that is a MEMBER OF A DB CLUSTER.
//!
//! The rule under test: a cluster member holds no storage of its own — the data is the
//! cluster volume's, and a snapshot of it is a DB CLUSTER snapshot — so there is no final DB
//! snapshot for `DeleteDBInstance` to create or to skip, and a delete that names neither
//! `SkipFinalSnapshot` nor `FinalDBSnapshotIdentifier` is served. A standalone DB instance
//! does hold its own storage, so its delete still has to say what becomes of it: one of the
//! two members is required, and naming both is refused for either kind of instance.
//!
//! The rows span what the rule generalises over: both kinds of instance (a cluster member
//! and a standalone instance), each combination of the two members, and both cluster engines
//! whose members this front door creates.

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, text};

/// What `DeleteDBInstance` answers a request whose final-snapshot members cannot hold
/// together: the Query API code RDS answers a member combination with.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// One row: the kind of instance, the members the delete names, and what the call answers.
struct Row {
    case: &'static str,
    /// The cluster the instance is created in, for a member; `None` for a standalone
    /// instance.
    cluster: Option<(&'static str, &'static str)>,
    identifier: &'static str,
    engine: &'static str,
    /// `SkipFinalSnapshot`, where the delete names it.
    skip: Option<&'static str>,
    /// `FinalDBSnapshotIdentifier`, where the delete names it.
    final_snapshot: Option<&'static str>,
    /// The HTTP status the delete answers, and the wire `<Code>` of a refusal.
    expected: (u16, Option<&'static str>),
}

const ROWS: &[Row] = &[
    Row {
        case: "an Aurora PostgreSQL cluster member, naming neither member",
        cluster: Some(("emu-member-pg", "aurora-postgresql")),
        identifier: "emu-member-pg-w",
        engine: "aurora-postgresql",
        skip: None,
        final_snapshot: None,
        expected: (200, None),
    },
    Row {
        case: "an Aurora MySQL cluster member, naming neither member",
        cluster: Some(("emu-member-my", "aurora-mysql")),
        identifier: "emu-member-my-w",
        engine: "aurora-mysql",
        skip: None,
        final_snapshot: None,
        expected: (200, None),
    },
    Row {
        case: "a cluster member whose delete skips the final snapshot anyway",
        cluster: Some(("emu-member-skip", "aurora-postgresql")),
        identifier: "emu-member-skip-w",
        engine: "aurora-postgresql",
        skip: Some("true"),
        final_snapshot: None,
        expected: (200, None),
    },
    Row {
        case: "a cluster member whose delete names both members",
        cluster: Some(("emu-member-both", "aurora-postgresql")),
        identifier: "emu-member-both-w",
        engine: "aurora-postgresql",
        skip: Some("true"),
        final_snapshot: Some("emu-member-both-final"),
        expected: (400, Some(INVALID_PARAMETER_COMBINATION)),
    },
    Row {
        case: "a standalone DB instance, naming neither member",
        cluster: None,
        identifier: "emu-standalone-neither",
        engine: "postgres",
        skip: None,
        final_snapshot: None,
        expected: (400, Some(INVALID_PARAMETER_COMBINATION)),
    },
    Row {
        case: "a standalone DB instance whose delete skips the final snapshot",
        cluster: None,
        identifier: "emu-standalone-skip",
        engine: "postgres",
        skip: Some("true"),
        final_snapshot: None,
        expected: (200, None),
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cluster_members_delete_needs_no_final_snapshot_while_a_standalone_instances_does() {
    let router = emulator();
    for row in ROWS {
        let case = row.case;
        let mut create: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", row.identifier.to_string()),
            ("Engine", row.engine.to_string()),
            ("DBInstanceClass", "db.r5.large".to_string()),
        ];
        match row.cluster {
            Some((cluster, engine)) => {
                expect_ok(
                    case,
                    call(
                        &router,
                        "CreateDBCluster",
                        &[
                            ("DBClusterIdentifier", cluster),
                            ("Engine", engine),
                            ("MasterUsername", "emuadmin"),
                            ("MasterUserPassword", "emu-Secret-99"),
                        ],
                    )
                    .await,
                );
                create.push(("DBClusterIdentifier", cluster.to_string()));
            }
            None => {
                create.push(("AllocatedStorage", "20".to_string()));
                create.push(("MasterUsername", "emuadmin".to_string()));
                create.push(("MasterUserPassword", "emu-Secret-99".to_string()));
            }
        }
        expect_ok(case, call(&router, "CreateDBInstance", &create).await);

        let mut delete: Vec<(&str, String)> =
            vec![("DBInstanceIdentifier", row.identifier.to_string())];
        if let Some(skip) = row.skip {
            delete.push(("SkipFinalSnapshot", skip.to_string()));
        }
        if let Some(named) = row.final_snapshot {
            delete.push(("FinalDBSnapshotIdentifier", named.to_string()));
        }
        let answer = call(&router, "DeleteDBInstance", &delete).await;
        match row.expected {
            (status, None) => {
                let body = expect_ok(case, (answer.0, answer.1));
                assert_eq!(
                    status, 200,
                    "case {case:?}: a served delete answers 200, which this row expects"
                );
                let reported = text(&body, "DBInstanceStatus");
                assert_eq!(
                    reported, "deleting",
                    "case {case:?}: an accepted delete reports deleting, got {reported:?}"
                );
            }
            (status, Some(code)) => {
                expect_error(case, answer, status, code);
            }
        }

        // No final DB snapshot is taken for a cluster member, whatever its delete named: the
        // data is the cluster's, so a snapshot of it is the cluster's to take.
        if row.cluster.is_some() {
            let snapshots = expect_ok(
                case,
                call(
                    &router,
                    "DescribeDBSnapshots",
                    &[("DBInstanceIdentifier", row.identifier)],
                )
                .await,
            );
            assert_eq!(
                optional_text(&snapshots, "DBSnapshotIdentifier"),
                None,
                "case {case:?}: a cluster member's delete must take no DB snapshot, got \
                 {snapshots}"
            );
        }
    }
}
