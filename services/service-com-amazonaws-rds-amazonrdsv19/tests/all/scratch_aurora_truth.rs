// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONTROL-PLANE half of an Aurora engine session: the sequence a
//! client that wants to run SQL against a cluster drives before it opens a connection, and
//! the teardown it runs afterwards.
//!
//! The rule under test: `CreateDBCluster` plus a writer `CreateDBInstance` in that cluster
//! both converge to `available` in bounded time without any mutating call of their own, the
//! cluster publishes the endpoint and the port its engine accepts connections on, and the
//! pair can then be torn down — the writer first, since an Aurora cluster that still has
//! members refuses its own delete, and the cluster once it has none.
//!
//! Each rule is asserted over a TABLE spanning what it generalises over: the Aurora engines
//! whose clusters this front door creates, each with the port its engine publishes and a
//! database name of its own.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with.

use std::time::{Duration, Instant};

use tokio::time::sleep;

use crate::db_instances::{call, emulator, expect_ok, optional_text, text};

/// How long a poll loop is given before this suite calls a transition unobservable.
///
/// Generous next to the span a transition this service owes becomes due after, so a loaded
/// test machine cannot fail the suite, and still far short of a client's patience.
const POLL_DEADLINE: Duration = Duration::from_secs(20);

/// The gap between two polls of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The status a DB cluster and a DB instance both settle on once they are usable.
const AVAILABLE: &str = "available";

/// One row: an Aurora engine, the class a member of its clusters is created with, and the
/// database the cluster is created with.
struct Row {
    case: &'static str,
    engine: &'static str,
    /// An instance class the orderable-options report offers for a cluster of this engine.
    class: &'static str,
    database: &'static str,
}

/// The Aurora engines whose data plane is addressed through the CLUSTER's endpoint, which is
/// what this sequence is about.
const ROWS: &[Row] = &[
    Row {
        case: "an Aurora PostgreSQL cluster",
        engine: "aurora-postgresql",
        class: "db.r5.large",
        database: "enginetruth",
    },
    Row {
        case: "an Aurora MySQL cluster",
        engine: "aurora-mysql",
        class: "db.r5.large",
        database: "truthdb",
    },
];

/// Poll `read` until it answers 200 with `tag` reading [`AVAILABLE`], and answer the body it
/// settled on; panics with the last answer when [`POLL_DEADLINE`] passes first.
///
/// This is the shape of the loop a real client writes: `read` makes a REPORTING call and
/// nothing else, so only a transition this service's clock owes can end it.
async fn poll_until_available<Read, Fut>(case: &str, what: &str, tag: &str, read: Read) -> String
where
    Read: Fn() -> Fut,
    Fut: std::future::Future<Output = (u16, String)>,
{
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = read().await;
        if status == 200 && text(&body, tag) == AVAILABLE {
            return body;
        }
        last = format!("HTTP {status}: {body}");
        sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: {what} never reached {AVAILABLE:?} in {POLL_DEADLINE:?}; last answer: {last}");
}

#[tokio::test]
async fn an_aurora_cluster_and_its_writer_converge_and_can_then_be_torn_down() {
    let router = emulator();

    for row in ROWS {
        let case = row.case;
        let cluster = format!("emu-aurora-seq-{}", row.engine.replace('-', ""));
        let writer = format!("{cluster}-w");

        let created = expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", cluster.as_str()),
                    ("Engine", row.engine),
                    ("MasterUsername", "sim2real"),
                    ("MasterUserPassword", "Sim2RealPassw0rd"),
                    ("DatabaseName", row.database),
                    ("BackupRetentionPeriod", "1"),
                ],
            )
            .await,
        );
        // The create answers the transient status it stored, which is what a client polls
        // out of.
        let status = text(&created, "Status");
        assert_eq!(
            status, "creating",
            "case {case:?}: a cluster this call created reports creating, got {status:?}"
        );

        expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", writer.as_str()),
                    ("DBClusterIdentifier", cluster.as_str()),
                    ("Engine", row.engine),
                    ("DBInstanceClass", row.class),
                    ("PubliclyAccessible", "false"),
                ],
            )
            .await,
        );

        // Both converge on their own: the poll loop makes reporting calls only.
        let cluster_params = [("DBClusterIdentifier", cluster.clone())];
        let writer_params = [("DBInstanceIdentifier", writer.clone())];
        let settled = poll_until_available(case, "the cluster", "Status", || {
            call(&router, "DescribeDBClusters", &cluster_params)
        })
        .await;
        poll_until_available(case, "the writer", "DBInstanceStatus", || {
            call(&router, "DescribeDBInstances", &writer_params)
        })
        .await;

        // The settled cluster publishes where its engine accepts connections: the address
        // and port its own engine listener answers on, which the core assigned (the port
        // the request named, or the engine's own default, is the CONFIGURED port and is not
        // what a client can reach here — see `state::data_plane::cluster_endpoint`).
        let endpoint = optional_text(&settled, "Endpoint").unwrap_or_default();
        assert!(
            !endpoint.is_empty(),
            "case {case:?}: an available cluster publishes an Endpoint, got {settled}"
        );
        let reader = optional_text(&settled, "ReaderEndpoint").unwrap_or_default();
        assert!(
            !reader.is_empty(),
            "case {case:?}: an available cluster publishes a ReaderEndpoint, got {settled}"
        );
        let port = text(&settled, "Port");
        assert!(
            port.parse::<u16>().is_ok_and(|port| port != 0),
            "case {case:?}: expected the port the cluster's engine listener answers on, got \
             {port:?}"
        );
        // The master user the cluster was created with is reported back, which is the user a
        // session on that endpoint logs in as.
        let master = text(&settled, "MasterUsername");
        assert_eq!(
            master, "sim2real",
            "case {case:?}: expected the master user the create named, got {master:?}"
        );
        let database = text(&settled, "DatabaseName");
        assert_eq!(
            database, row.database,
            "case {case:?}: expected the database the create named, got {database:?}"
        );

        // The teardown, in the order the cluster's own rule forces: a cluster that still has
        // members refuses its delete, so the writer goes first and the cluster follows once
        // the member is gone. Each call is retried, as a client's teardown retries.
        let refused = call(
            &router,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", cluster.as_str()),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
        assert_eq!(
            refused.0, 400,
            "case {case:?}: a cluster that still has a member refuses its delete, got {:?}",
            refused
        );

        expect_ok(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", writer.as_str()),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
        // The member leaves the store on the clock, so the cluster's delete is retried until
        // it is accepted — the loop a teardown script writes.
        let deadline = Instant::now() + POLL_DEADLINE;
        let mut last = String::new();
        let mut accepted = false;
        while Instant::now() < deadline {
            let (status, body) = call(
                &router,
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", cluster.as_str()),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await;
            if status == 200 {
                accepted = true;
                break;
            }
            last = format!("HTTP {status}: {body}");
            sleep(POLL_INTERVAL).await;
        }
        assert!(
            accepted,
            "case {case:?}: the cluster's delete was never accepted in {POLL_DEADLINE:?}; \
             last answer: {last}"
        );
    }
}
