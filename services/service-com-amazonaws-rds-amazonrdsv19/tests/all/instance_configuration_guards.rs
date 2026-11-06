// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB instance guards and flows that follow from the instance's
//! own CONFIGURATION rather than from its lifecycle state.
//!
//! The rules under test:
//!
//! * deletion protection is a SETTING of the resource a delete names, so a delete of a
//!   protected resource is refused as a parameter combination (HTTP 400,
//!   `InvalidParameterCombination`) and not as a lifecycle state fault — the refusal says
//!   what to change, because no amount of waiting clears it;
//! * every operation that takes a `MasterUserPassword` refuses one outside the engine's
//!   published policy, with the refusal naming the member and the constraint;
//! * an instance that keeps continuous backups reports a `LatestRestorableTime` that
//!   advances with the clock, and one with automated backups off reports none;
//! * `RestoreDBInstanceFromDBSnapshot` checks the target identifier BEFORE it reads the
//!   snapshot, and a restore that is accepted returns the rows the source instance held
//!   when the snapshot was taken.
//!
//! Each table spans what its rule generalises over: the three resource families that carry
//! deletion protection, the two operations that set a master password and the published
//! bounds of each engine's policy, the retention settings that do and do not keep a
//! recovery window, and the restore cases (a free identifier, a live one, a snapshot that
//! does not exist).

use crate::db_instance_engine_endpoint::{available_instance, postgres_login, Transport};
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, wait_for_settle,
};

/// The master user the instances here are created with, and its test password.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";
/// The `DBName` the instances here are created with.
const DATABASE: &str = "appdb";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The `awsQueryError` code RDS answers a request whose members cannot hold together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// The `awsQueryError` code RDS answers a member value it will not take.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

#[tokio::test]
async fn a_delete_of_a_protected_resource_names_the_setting_that_has_to_change() {
    let router = emulator();
    // One row per resource family that carries `DeletionProtection`, because the rule is
    // the setting's and not any one resource's: RDS words the refusal with the resource's
    // own kind and the identifier the request named.
    expect_ok(
        "the protected instance",
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-prot-instance",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
                "DeletionProtection" => true,
            ],
        )
        .await,
    );
    expect_ok(
        "the protected cluster",
        call(
            &router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-prot-cluster",
                "Engine" => "aurora-postgresql",
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
                "DeletionProtection" => true,
            ],
        )
        .await,
    );
    expect_ok(
        "the protected global cluster",
        call(
            &router,
            "CreateGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-prot-global",
                "Engine" => "aurora-postgresql",
                "DeletionProtection" => true,
            ],
        )
        .await,
    );
    wait_for_settle();

    for (case, operation, member, identifier, kind, extra) in [
        (
            "a DB instance",
            "DeleteDBInstance",
            "DBInstanceIdentifier",
            "emu-prot-instance",
            // The resource KIND as Amazon RDS names it in this sentence; the recorded
            // answers carry no identifier in it.
            "DB Instance",
            Some(("SkipFinalSnapshot", "true")),
        ),
        (
            "a DB cluster",
            "DeleteDBCluster",
            "DBClusterIdentifier",
            "emu-prot-cluster",
            "Cluster",
            Some(("SkipFinalSnapshot", "true")),
        ),
        (
            "a global cluster",
            "DeleteGlobalCluster",
            "GlobalClusterIdentifier",
            "emu-prot-global",
            "Global Cluster",
            None,
        ),
    ] {
        let mut request = vec![(member, identifier.to_string())];
        if let Some((name, value)) = extra {
            request.push((name, value.to_string()));
        }
        let body = expect_error(
            case,
            call(&router, operation, &request).await,
            400,
            INVALID_PARAMETER_COMBINATION,
        );
        assert_eq!(
            text(&body, "Message"),
            format!(
                "Cannot delete protected {kind}, please disable deletion protection and \
                 try again."
            ),
            "case {case:?}: the refusal must carry the sentence this class is worded \
             with, naming the resource kind, got {:?}",
            text(&body, "Message")
        );
    }

    // And with the setting turned off the delete is accepted: the refusal was the
    // configuration's, so changing it is what clears it.
    expect_ok(
        "turn deletion protection off",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-prot-instance",
                "DeletionProtection" => false,
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "the delete once the setting is off",
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-prot-instance",
                "SkipFinalSnapshot" => true,
            ],
        )
        .await,
    );
}

#[tokio::test]
async fn a_master_user_password_outside_the_engines_policy_is_refused_on_every_path() {
    let router = emulator();
    // The bound is the ENGINE's: `MasterUserPassword` publishes one length line per
    // engine, and every one of them has the same lower bound of 8. The table spans the
    // engines whose published upper bound differs and the values at and just outside each
    // published bound, on both operations that set a password.
    for (case, engine, longest) in [
        ("a MySQL instance", "mysql", 41usize),
        ("a MariaDB instance", "mariadb", 41),
        ("a PostgreSQL instance", "postgres", 128),
    ] {
        let identifier = format!("emu-pw-{engine}");
        expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => identifier,
                    "Engine" => engine,
                    "DBInstanceClass" => "db.t3.micro",
                    "AllocatedStorage" => 20,
                    "MasterUsername" => MASTER_USER,
                    "MasterUserPassword" => MASTER_PASSWORD,
                ],
            )
            .await,
        );
        wait_for_settle();
        let rows: [(&str, String, Option<String>); 6] = [
            (
                "one character under the lower bound",
                "p".repeat(7),
                Some(
                    "The parameter MasterUserPassword is not a valid password because it \
                     is shorter than 8 characters."
                        .to_string(),
                ),
            ),
            ("the lower bound itself", "p".repeat(8), None),
            ("the upper bound itself", "p".repeat(longest), None),
            (
                "one character over the upper bound",
                "p".repeat(longest + 1),
                Some(format!(
                    "The parameter MasterUserPassword is not a valid password because it \
                     is longer than {longest} characters."
                )),
            ),
            (
                "a forbidden character",
                "pw1-Secret@".to_string(),
                Some(
                    "The parameter MasterUserPassword is not a valid password. Only \
                     printable ASCII characters besides '/', '@', '\"', ' ' may be used."
                        .to_string(),
                ),
            ),
            ("a password well inside the bounds", "pw1-Secret".to_string(), None),
        ];
        for (row, password, refusal) in rows {
            let answer = call(
                &router,
                "ModifyDBInstance",
                &params![
                    "DBInstanceIdentifier" => identifier,
                    "MasterUserPassword" => password,
                    "ApplyImmediately" => true,
                ],
            )
            .await;
            match &refusal {
                Some(message) => {
                    let body = expect_error(
                        &format!("{case}: {row}"),
                        answer,
                        400,
                        INVALID_PARAMETER_VALUE,
                    );
                    assert_eq!(
                        text(&body, "Message"),
                        *message,
                        "case {case:?} / {row:?}: the refusal must name the member and the \
                         constraint, got {:?}",
                        text(&body, "Message")
                    );
                }
                None => {
                    expect_ok(&format!("{case}: {row}"), answer);
                }
            }
            wait_for_settle();
        }
    }
}

#[tokio::test]
async fn an_instance_with_continuous_backups_reports_an_advancing_recovery_point() {
    let router = emulator();
    // `BackupRetentionPeriod`: "To disable automated backups, set this value to 0" — so
    // the table spans the retention values that do and do not keep a recovery window.
    for (case, retention, reports) in [
        ("automated backups off", 0, false),
        ("the smallest retention that keeps them", 1, true),
        ("a longer retention", 7, true),
    ] {
        let identifier = format!("emu-pitr-{retention}");
        expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => identifier,
                    "Engine" => "postgres",
                    "DBInstanceClass" => "db.t3.micro",
                    "AllocatedStorage" => 20,
                    "MasterUsername" => MASTER_USER,
                    "MasterUserPassword" => MASTER_PASSWORD,
                    "BackupRetentionPeriod" => retention,
                ],
            )
            .await,
        );
        wait_for_settle();
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => identifier],
            )
            .await,
        );
        let first = optional_text(&described, "LatestRestorableTime");
        assert_eq!(
            first.is_some(),
            reports,
            "case {case:?}: a recovery point must be reported exactly while continuous \
             backups are kept, got {first:?}"
        );
        if !reports {
            continue;
        }
        // The recovery point ADVANCES: a second read a moment later is not behind the
        // first, and neither predates the instance.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let again = expect_ok(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => identifier],
            )
            .await,
        );
        let second = optional_text(&again, "LatestRestorableTime").unwrap_or_default();
        let first = first.unwrap_or_default();
        assert!(
            second >= first,
            "case {case:?}: the recovery point must not go backwards, got {second:?} \
             after {first:?}"
        );
        assert!(
            first >= text(&described, "InstanceCreateTime"),
            "case {case:?}: the recovery point must not predate the instance, got \
             {first:?} against {:?}",
            text(&described, "InstanceCreateTime")
        );
    }
}

#[tokio::test]
async fn a_restore_from_a_snapshot_returns_the_rows_the_source_held() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-restore-source",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    // The rows the snapshot has to carry, written through the source instance's own
    // engine endpoint.
    let mut source = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted on the source");
    for statement in [
        "CREATE TABLE ledger (id int PRIMARY KEY, note text)",
        "INSERT INTO ledger VALUES (1, 'before the snapshot')",
    ] {
        source
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    expect_ok(
        "the snapshot",
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-restore-source",
                "DBSnapshotIdentifier" => "emu-restore-snap",
            ],
        )
        .await,
    );
    wait_for_settle();
    // Written AFTER the snapshot: a restore must not return it, because a snapshot is the
    // database as it stood when it was taken.
    source
        .query("INSERT INTO ledger VALUES (2, 'after the snapshot')")
        .await
        .expect("the later write runs");

    // The identifier collision comes first, whatever the snapshot is: a restore onto a
    // live instance is refused for the name.
    let collision = expect_error(
        "a restore onto a live instance",
        call(
            &router,
            "RestoreDBInstanceFromDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-restore-source",
                "DBSnapshotIdentifier" => "emu-restore-snap",
            ],
        )
        .await,
        400,
        "DBInstanceAlreadyExists",
    );
    assert_eq!(
        text(&collision, "Message"),
        "DB instance already exists",
        "the refusal must be RDS's own sentence, got {:?}",
        text(&collision, "Message")
    );
    // A snapshot that does not exist is a not-found, under a free identifier.
    expect_error(
        "a snapshot that does not exist",
        call(
            &router,
            "RestoreDBInstanceFromDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-restore-target",
                "DBSnapshotIdentifier" => "emu-restore-absent",
            ],
        )
        .await,
        404,
        "DBSnapshotNotFound",
    );

    // The restore itself: the new instance carries the source's settings and its data.
    let restored = expect_ok(
        "the restore",
        call(
            &router,
            "RestoreDBInstanceFromDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-restore-target",
                "DBSnapshotIdentifier" => "emu-restore-snap",
            ],
        )
        .await,
    );
    assert_eq!(
        text(&restored, "DBInstanceStatus"),
        "creating",
        "a restored instance comes up like any other create, got {restored}"
    );
    assert_eq!(
        (
            text(&restored, "Engine").as_str(),
            text(&restored, "MasterUsername").as_str(),
            text(&restored, "DBInstanceClass").as_str(),
        ),
        ("postgres", MASTER_USER, "db.t3.micro"),
        "the restored instance must carry the source's configuration, got {restored}"
    );
    wait_for_settle();
    let described = expect_ok(
        "the restored instance",
        call(
            &router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => "emu-restore-target"],
        )
        .await,
    );
    assert_eq!(
        text(&described, "DBInstanceStatus"),
        "available",
        "the restored instance must settle to available, got {described}"
    );
    let endpoint = crate::db_instances::subtrees(&described, "Endpoint")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the restored instance must publish an endpoint: {described}"));
    let restored_port: u16 = text(&endpoint, "Port")
        .parse()
        .unwrap_or_else(|_| panic!("the endpoint must publish a port: {endpoint}"));
    assert_ne!(
        text(&endpoint, "Address").as_str(),
        "",
        "the restored instance must publish an address, got {endpoint}"
    );
    // The restored instance's master credential is not the source's: a restore installs
    // no password, so the endpoint is reachable and admits nobody until one is set.
    expect_ok(
        "set the restored instance's master password",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-restore-target",
                "MasterUserPassword" => MASTER_PASSWORD,
                "ApplyImmediately" => true,
            ],
        )
        .await,
    );
    wait_for_settle();
    let mut target = postgres_login(
        &text(&endpoint, "Address"),
        restored_port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the restored instance must admit its master user");
    let read_back = target
        .query("SELECT id, note FROM ledger ORDER BY id")
        .await
        .expect("the restored data must be readable");
    assert_eq!(
        read_back.rows,
        vec![vec![
            Some("1".to_string()),
            Some("before the snapshot".to_string())
        ]],
        "the restore must return the rows the source held when the snapshot was taken, \
         and only those, got {read_back:?}"
    );
}
