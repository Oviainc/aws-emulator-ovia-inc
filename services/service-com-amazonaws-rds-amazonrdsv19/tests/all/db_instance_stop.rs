// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopDBInstance` and the `DescribeDBInstances` /
//! `DescribeDBSnapshots` reads that follow it.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting
//! helper this test target has, as a real awsQuery form POST carrying the SigV4
//! credential scope the router keys on.
//!
//! The rule under test: `StopDBInstance` stops a stored DB instance that is `available`
//! and that the operation applies to — the instance reports `stopping` at once and
//! `stopped` once the stop has run, and every piece of metadata the operation documents
//! as retained ("Amazon RDS retains the DB instance's metadata, including its endpoint, DB
//! parameter group, and option group membership") reads back unchanged. Asked for a
//! `DBSnapshotIdentifier`, the call also mints that manual snapshot of the instance
//! "immediately before the DB instance is stopped". Every other case is a modelled
//! refusal that leaves the instance exactly as it was: an identifier naming no instance in
//! the region signed for is `DBInstanceNotFoundFault` (HTTP 404); an instance that is not
//! `available`, or whose engine the command "doesn't apply to" (RDS Custom, Aurora MySQL
//! and Aurora PostgreSQL), is `InvalidDBInstanceStateFault` (HTTP 400); one that belongs
//! to a DB cluster, whose stop is `StopDBCluster`'s, is `InvalidDBClusterStateFault`; a
//! `DBSnapshotIdentifier` already in use is `DBSnapshotAlreadyExistsFault`; and one
//! outside the member's documented identifier constraints is a 400 `ValidationException`.
//!
//! The tables below span what the rules generalise over: the engine families and instance
//! classes an instance-level stop applies to, both forms of the snapshot parameter
//! (absent and given) and all three forms of `DetachFromDBCapacityReservation` (absent,
//! `true`, `false` — no stored instance uses a DB capacity reservation, so the parameter
//! "has no effect" for any of them), the statuses a stop can arrive in, each engine family
//! the command does not apply to, and the identifier shapes the snapshot member refuses.

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, texts, wait_for_settle,
    REGION,
};

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The `DBInstance` members `StopDBInstance` documents as RETAINED across a stop, read
/// back from the reported envelope: the endpoint ("including its endpoint"), the DB
/// parameter group and the option group membership ("DB parameter group, and option group
/// membership"), plus the identity and sizing members no status change can touch.
const RETAINED_MEMBERS: [&str; 10] = [
    "Address",
    "DbInstancePort",
    "DBInstanceArn",
    "DbiResourceId",
    "DBInstanceClass",
    "Engine",
    "DBParameterGroupName",
    "OptionGroupName",
    "MasterUsername",
    "InstanceCreateTime",
];

/// One row of the stop table.
struct StopCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
    /// `Some(identifier)` asks for the snapshot taken just before the stop.
    snapshot: Option<&'static str>,
    /// `DetachFromDBCapacityReservation`, left out of the request when `None`.
    detach: Option<bool>,
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

/// The settings every instance below is created with, so each row reports the members
/// [`RETAINED_MEMBERS`] names.
fn create_settings() -> Vec<(&'static str, String)> {
    params!["AllocatedStorage" => 20, "MasterUsername" => "emuadmin"].to_vec()
}

/// The one `<DBInstance>` subtree `DescribeDBInstances` reports for `identifier`, or
/// `None` when the read is the modelled 404.
async fn described(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
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
    Some(instances.into_iter().next().expect("checked just above"))
}

/// The `DBInstanceStatus` `DescribeDBInstances` reports for `identifier`.
async fn reported_status(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
    described(router, identifier)
        .await
        .map(|instance| text(&instance, "DBInstanceStatus"))
}

/// The `StopDBInstance` request for one row.
fn stop_request(row: &StopCase) -> Vec<(&'static str, String)> {
    let mut request: Vec<(&'static str, String)> =
        params!["DBInstanceIdentifier" => row.identifier].to_vec();
    if let Some(snapshot) = row.snapshot {
        request.push(("DBSnapshotIdentifier", snapshot.to_string()));
    }
    if let Some(detach) = row.detach {
        request.push(("DetachFromDBCapacityReservation", detach.to_string()));
    }
    request
}

/// The one `<DBSnapshot>` subtree named `identifier`, out of everything
/// `DescribeDBSnapshots` reports.
async fn snapshot_named(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
    let body = expect_ok(
        "read the snapshots",
        call(router, "DescribeDBSnapshots", &[] as &[(&str, String)]).await,
    );
    subtrees(&body, "DBSnapshot")
        .into_iter()
        .find(|snapshot| text(snapshot, "DBSnapshotIdentifier") == identifier)
}

#[tokio::test]
async fn a_stop_takes_an_available_instance_to_stopped_and_retains_its_metadata() {
    let table: &[StopCase] = &[
        StopCase {
            case: "postgres, no snapshot, no DetachFromDBCapacityReservation",
            identifier: "emu-stop-postgres",
            engine: "postgres",
            class: "db.t3.micro",
            snapshot: None,
            detach: None,
        },
        StopCase {
            case: "mysql, snapshot taken before the stop",
            identifier: "emu-stop-mysql",
            engine: "mysql",
            class: "db.m5.large",
            snapshot: Some("emu-stop-mysql-snap"),
            detach: None,
        },
        StopCase {
            case: "mariadb, snapshot, detaching from the DB capacity reservation",
            identifier: "emu-stop-mariadb",
            engine: "mariadb",
            class: "db.t3.small",
            snapshot: Some("emu-stop-mariadb-snap"),
            detach: Some(true),
        },
        StopCase {
            case: "sqlserver-ex, no snapshot, keeping the DB capacity reservation",
            identifier: "emu-stop-sqlserver",
            engine: "sqlserver-ex",
            class: "db.t3.medium",
            snapshot: None,
            detach: Some(false),
        },
        StopCase {
            case: "oracle-se2, snapshot, keeping the DB capacity reservation",
            identifier: "emu-stop-oracle",
            engine: "oracle-se2",
            class: "db.m5.xlarge",
            snapshot: Some("emu-stop-oracle-snap"),
            detach: Some(false),
        },
        StopCase {
            case: "db2-se, snapshot taken before the stop",
            identifier: "emu-stop-db2",
            engine: "db2-se",
            class: "db.m5.large",
            snapshot: Some("emu-stop-db2-snap"),
            detach: None,
        },
    ];

    for row in table {
        // Each row gets its own emulator, so the timing of one row's transitions cannot
        // carry into the next.
        let router = emulator();
        expect_ok(
            row.case,
            create_instance(
                &router,
                row.identifier,
                row.engine,
                row.class,
                &create_settings(),
            )
            .await,
        );
        wait_for_settle();
        let before = described(&router, row.identifier)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the created instance must be readable", row.case));
        assert_eq!(
            text(&before, "DBInstanceStatus"),
            "available",
            "case {:?}: the stop is issued against an available instance, got {:?}",
            row.case,
            text(&before, "DBInstanceStatus")
        );

        let accepted = expect_ok(
            row.case,
            call(&router, "StopDBInstance", &stop_request(row)).await,
        );
        let reported = subtrees(&accepted, "DBInstance");
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the stop must report the instance it accepted, got {accepted}",
            row.case
        );
        let answered_status = text(&reported[0], "DBInstanceStatus");
        assert_eq!(
            answered_status, "stopping",
            "case {:?}: an accepted stop must report `stopping`, got {answered_status:?}",
            row.case
        );

        // Inside the span the clock owes, the instance still reports the transient status
        // the stop answered with, and the snapshot it was asked for is still being taken.
        let during = reported_status(&router, row.identifier).await;
        assert_eq!(
            during.as_deref(),
            Some("stopping"),
            "case {:?}: a read inside the stopping window must report `stopping`, got {during:?}",
            row.case
        );
        if let Some(snapshot) = row.snapshot {
            let taken = snapshot_named(&router, snapshot).await.unwrap_or_else(|| {
                panic!(
                    "case {:?}: the snapshot {snapshot} must be taken before the stop",
                    row.case
                )
            });
            let status = text(&taken, "Status");
            assert_eq!(
                status, "creating",
                "case {:?}: the snapshot is still being taken inside the window, got {status:?}",
                row.case
            );
        }

        wait_for_settle();

        // And then the instance is down, and stays down: nothing but the clock's own
        // transition moves it, and a second stop of a stopped instance is refused.
        let after = described(&router, row.identifier)
            .await
            .unwrap_or_else(|| panic!("case {:?}: a stopped instance is still stored", row.case));
        let settled = text(&after, "DBInstanceStatus");
        assert_eq!(
            settled, "stopped",
            "case {:?}: the stop must settle on `stopped`, got {settled:?}",
            row.case
        );
        for member in RETAINED_MEMBERS {
            let (was, now) = (text(&before, member), text(&after, member));
            assert_eq!(
                was, now,
                "case {:?}: a stop must retain {member}, was {was:?} and is now {now:?}",
                row.case
            );
        }
        let again = expect_error(
            row.case,
            call(
                &router,
                "StopDBInstance",
                &params!["DBInstanceIdentifier" => row.identifier],
            )
            .await,
            400,
            "InvalidDBInstanceState",
        );
        let message = text(&again, "Message");
        assert!(
            message.contains(row.identifier) && message.contains("stopped"),
            "case {:?}: stopping a stopped instance must say so, got {message:?}",
            row.case
        );

        // The snapshot the stop was asked for outlives the stop and reads back as a
        // manual snapshot of this instance; asking for none stores none.
        let stored = texts(
            &expect_ok(
                row.case,
                call(&router, "DescribeDBSnapshots", &[] as &[(&str, String)]).await,
            ),
            "DBSnapshotIdentifier",
        );
        match row.snapshot {
            Some(snapshot) => {
                assert_eq!(
                    stored,
                    vec![snapshot.to_string()],
                    "case {:?}: the stop must take exactly the snapshot it was asked for, \
                     got {stored:?}",
                    row.case
                );
                let taken = snapshot_named(&router, snapshot)
                    .await
                    .unwrap_or_else(|| panic!("case {:?}: the snapshot is stored", row.case));
                for (member, expected) in [
                    ("DBInstanceIdentifier", row.identifier),
                    ("Engine", row.engine),
                    ("SnapshotType", "manual"),
                    ("Status", "available"),
                ] {
                    let actual = text(&taken, member);
                    assert_eq!(
                        actual, expected,
                        "case {:?}: the snapshot's {member} must be {expected:?}, got {actual:?}",
                        row.case
                    );
                }
            }
            None => assert!(
                stored.is_empty(),
                "case {:?}: a stop asked for no snapshot must store none, got {stored:?}",
                row.case
            ),
        }
    }
}

/// One row of the refusal table: an engine an instance-level stop does not apply to.
struct EngineCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
}

#[tokio::test]
async fn the_engines_the_command_does_not_apply_to_are_refused() {
    // "This command doesn't apply to RDS Custom, Aurora MySQL, and Aurora PostgreSQL. For
    // Aurora clusters, use StopDBCluster instead." The engines are read off
    // `CreateDBInstanceMessage$Engine`'s own list: `aurora-mysql` and `aurora-postgresql`
    // are Aurora, and the `custom-` engines are RDS Custom — one of which, `custom-se2`
    // below, no part of this emulator's own catalogs singles out.
    let table: &[EngineCase] = &[
        EngineCase {
            case: "Aurora MySQL",
            identifier: "emu-stop-aurora-mysql",
            engine: "aurora-mysql",
        },
        EngineCase {
            case: "Aurora PostgreSQL",
            identifier: "emu-stop-aurora-pg",
            engine: "aurora-postgresql",
        },
        EngineCase {
            case: "RDS Custom for Oracle",
            identifier: "emu-stop-custom-oracle",
            engine: "custom-oracle-ee",
        },
        EngineCase {
            case: "RDS Custom for Oracle SE2, a CDB",
            identifier: "emu-stop-custom-se2",
            engine: "custom-oracle-se2-cdb",
        },
        EngineCase {
            case: "RDS Custom for SQL Server",
            identifier: "emu-stop-custom-sqlserver",
            engine: "custom-sqlserver-se",
        },
    ];

    let router = emulator();
    for row in table {
        expect_ok(
            row.case,
            create_instance(
                &router,
                row.identifier,
                row.engine,
                "db.m5.large",
                &create_settings(),
            )
            .await,
        );
    }
    wait_for_settle();

    for row in table {
        let body = expect_error(
            row.case,
            call(
                &router,
                "StopDBInstance",
                &params!["DBInstanceIdentifier" => row.identifier],
            )
            .await,
            400,
            "InvalidDBInstanceState",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(row.identifier) && message.contains(row.engine),
            "case {:?}: the refusal must name the instance and its engine, got {message:?}",
            row.case
        );
        let still = reported_status(&router, row.identifier).await;
        assert_eq!(
            still.as_deref(),
            Some("available"),
            "case {:?}: a refused stop must leave the instance running, got {still:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_cluster_member_is_refused_with_the_cluster_state_fault() {
    let router = emulator();
    // A Multi-AZ DB cluster, with the four settings `CreateDBCluster` requires for one.
    expect_ok(
        "create the cluster",
        call(
            &router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-stop-cluster",
                "Engine" => "mysql",
                "AllocatedStorage" => 100,
                "DBClusterInstanceClass" => "db.m6gd.xlarge",
                "StorageType" => "io1",
                "Iops" => 1000,
            ],
        )
        .await,
    );
    expect_ok(
        "create the member",
        create_instance(
            &router,
            "emu-stop-member",
            "mysql",
            "db.m5.large",
            &params![
                "AllocatedStorage" => 20,
                "MasterUsername" => "emuadmin",
                "DBClusterIdentifier" => "emu-stop-cluster",
            ],
        )
        .await,
    );
    wait_for_settle();

    let body = expect_error(
        "a member of a DB cluster",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-stop-member"],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("emu-stop-member") && message.contains("emu-stop-cluster"),
        "the refusal must name the instance and its cluster, got {message:?}"
    );
    let still = reported_status(&router, "emu-stop-member").await;
    assert_eq!(
        still.as_deref(),
        Some("available"),
        "a refused stop must leave the member running, got {still:?}"
    );
}

#[tokio::test]
async fn only_an_available_instance_in_the_signed_region_can_be_stopped() {
    let router = emulator();

    // An identifier naming nothing is the modelled 404 — before any other member of the
    // request is judged.
    for (case, request) in [
        (
            "an identifier naming no instance",
            params!["DBInstanceIdentifier" => "emu-stop-absent"].to_vec(),
        ),
        (
            "an identifier naming no instance, with a snapshot asked for",
            params![
                "DBInstanceIdentifier" => "emu-stop-absent",
                "DBSnapshotIdentifier" => "emu-stop-absent-snap",
            ]
            .to_vec(),
        ),
    ] {
        let body = expect_error(
            case,
            call(&router, "StopDBInstance", &request).await,
            404,
            "DBInstanceNotFound",
        );
        assert!(
            text(&body, "Message").contains("emu-stop-absent"),
            "case {case:?}: the refusal must name the identifier, got {:?}",
            text(&body, "Message")
        );
    }

    // An instance the clock has not settled yet is not available, so it cannot be
    // stopped: the status the next read reports is the one the stop is judged against.
    expect_ok(
        "create the instance stopped while creating",
        create_instance(
            &router,
            "emu-stop-creating",
            "postgres",
            "db.t3.micro",
            &create_settings(),
        )
        .await,
    );
    let body = expect_error(
        "an instance still creating",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-stop-creating"],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("emu-stop-creating") && message.contains("creating"),
        "the refusal must name the instance and the status it is in, got {message:?}"
    );

    // Nor can one whose stop is already running.
    wait_for_settle();
    expect_ok(
        "the first stop",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-stop-creating"],
        )
        .await,
    );
    let body = expect_error(
        "a second stop inside the stopping window",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-stop-creating"],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("emu-stop-creating") && message.contains("stopping"),
        "the refusal must say the stop is already running, got {message:?}"
    );

    // RDS is regional: an instance is addressable only in the region it was created in,
    // so the same identifier signed for another region names nothing.
    expect_ok(
        "create the regional instance",
        create_instance(
            &router,
            "emu-stop-regional",
            "mysql",
            "db.t3.small",
            &create_settings(),
        )
        .await,
    );
    wait_for_settle();
    for other_region in ["us-east-1", "eu-central-1"] {
        assert_ne!(
            other_region, REGION,
            "the region of this case must differ from the one the instance was created in"
        );
        let body = expect_error(
            other_region,
            call_in(
                &router,
                other_region,
                "StopDBInstance",
                &params!["DBInstanceIdentifier" => "emu-stop-regional"],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
        assert!(
            text(&body, "Message").contains("emu-stop-regional"),
            "case {other_region:?}: the refusal must name the identifier, got {:?}",
            text(&body, "Message")
        );
    }
    let still = reported_status(&router, "emu-stop-regional").await;
    assert_eq!(
        still.as_deref(),
        Some("available"),
        "a stop refused in another region must leave the instance running, got {still:?}"
    );
}

#[tokio::test]
async fn the_snapshot_the_stop_takes_is_held_to_the_members_own_rules() {
    let router = emulator();
    expect_ok(
        "create the instance",
        create_instance(
            &router,
            "emu-stop-snap-rules",
            "postgres",
            "db.t3.micro",
            &create_settings(),
        )
        .await,
    );
    expect_ok(
        "create the instance whose snapshot name is taken",
        create_instance(
            &router,
            "emu-stop-snap-taken",
            "mysql",
            "db.t3.small",
            &create_settings(),
        )
        .await,
    );
    expect_ok(
        "create the instance stopped at the identifier's upper bound",
        create_instance(
            &router,
            "emu-stop-snap-bound",
            "mariadb",
            "db.t3.small",
            &create_settings(),
        )
        .await,
    );
    wait_for_settle();

    // The published upper bound of `DBSnapshotIdentifier` is accepted, and one character
    // past it is not: "Must contain from 1 to 255 letters, numbers, or hyphens."
    let at_the_bound = format!("e{}", "a".repeat(254));
    assert_eq!(
        at_the_bound.chars().count(),
        255,
        "the accepted case must sit exactly on the member's upper bound"
    );
    expect_ok(
        "a snapshot identifier of exactly 255 characters",
        call(
            &router,
            "StopDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-stop-snap-bound",
                "DBSnapshotIdentifier" => at_the_bound,
            ],
        )
        .await,
    );
    assert!(
        snapshot_named(&router, &at_the_bound).await.is_some(),
        "a snapshot identifier on the upper bound must be stored"
    );

    // The identifier shapes `DBSnapshotIdentifier` refuses ("Must contain from 1 to 255
    // letters, numbers, or hyphens. First character must be a letter. Can't end with a
    // hyphen or contain two consecutive hyphens"), one per constraint, plus one character
    // past the length bound.
    let too_long = format!("e{}", "a".repeat(255));
    for (case, snapshot) in [
        ("not starting with a letter", "9emu-stop-snap".to_string()),
        ("ending with a hyphen", "emu-stop-snap-".to_string()),
        ("two consecutive hyphens", "emu--stop-snap".to_string()),
        ("outside the alphabet", "emu_stop_snap".to_string()),
        ("one character past the length bound", too_long),
    ] {
        let body = expect_error(
            case,
            call(
                &router,
                "StopDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-stop-snap-rules",
                    "DBSnapshotIdentifier" => snapshot,
                ],
            )
            .await,
            400,
            "ValidationException",
        );
        assert!(
            text(&body, "Message").contains("DBSnapshotIdentifier"),
            "case {case:?}: the refusal must name the member, got {:?}",
            text(&body, "Message")
        );
        let still = reported_status(&router, "emu-stop-snap-rules").await;
        assert_eq!(
            still.as_deref(),
            Some("available"),
            "case {case:?}: a refused stop must leave the instance running, got {still:?}"
        );
    }

    // A snapshot identifier already in use is the modelled refusal, and the stop it was
    // part of writes nothing at all: the instance is left running and the existing
    // snapshot still belongs to the instance that took it.
    expect_ok(
        "take the snapshot whose name is then reused",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-stop-snap-taken",
                "DBSnapshotIdentifier" => "emu-stop-snap-in-use",
            ],
        )
        .await,
    );
    let body = expect_error(
        "a snapshot identifier already in use",
        call(
            &router,
            "StopDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-stop-snap-rules",
                "DBSnapshotIdentifier" => "emu-stop-snap-in-use",
            ],
        )
        .await,
        400,
        "DBSnapshotAlreadyExists",
    );
    assert!(
        text(&body, "Message").contains("emu-stop-snap-in-use"),
        "the refusal must name the snapshot identifier, got {:?}",
        text(&body, "Message")
    );
    let still = reported_status(&router, "emu-stop-snap-rules").await;
    assert_eq!(
        still.as_deref(),
        Some("available"),
        "a refused stop must leave the instance running, got {still:?}"
    );
    let taken = snapshot_named(&router, "emu-stop-snap-in-use")
        .await
        .expect("the snapshot that was already there is still there");
    assert_eq!(
        text(&taken, "DBInstanceIdentifier"),
        "emu-stop-snap-taken",
        "the existing snapshot must still name the instance it was taken from, got {:?}",
        text(&taken, "DBInstanceIdentifier")
    );

    // The same stop with a free identifier is accepted, which is what makes the refusals
    // above refusals of the snapshot member and not of the stop itself.
    let accepted = expect_ok(
        "a free snapshot identifier",
        call(
            &router,
            "StopDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-stop-snap-rules",
                "DBSnapshotIdentifier" => "emu-stop-snap-free",
            ],
        )
        .await,
    );
    let status = text(&subtrees(&accepted, "DBInstance")[0], "DBInstanceStatus");
    assert_eq!(
        status, "stopping",
        "an accepted stop must report `stopping`, got {status:?}"
    );
    assert!(
        snapshot_named(&router, "emu-stop-snap-free").await.is_some(),
        "the accepted stop must have taken its snapshot"
    );
    assert!(
        STATUS_TRANSITION_DELAY > std::time::Duration::ZERO,
        "the transition this service owes is never instantaneous, so the window above is real"
    );
}
