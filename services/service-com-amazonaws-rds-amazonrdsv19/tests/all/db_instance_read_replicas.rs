// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateDBInstanceReadReplica`: the operation that creates a DB
//! instance which replicates another DB instance this service holds.
//!
//! The rules under test:
//!
//! * a replica is built from the SOURCE's automated backups, so a source that keeps none
//!   ("its backup retention period must be greater than 0") cannot be replicated, and the
//!   identical call is served once `ModifyDBInstance` sets a non-zero period;
//! * a replica is a copy of its source — its engine, its version and every setting the
//!   request does not name are the source's — and the two instances report the
//!   relationship from both ends;
//! * a source this operation cannot replicate is refused for the SOURCE: a DB instance that
//!   belongs to a DB cluster, a source and a cluster named at once, neither named, and a
//!   group of an engine the source does not run.
//!
//! The rules are asserted over TABLES spanning what they generalise over — the source's
//! engine, how the source is named (identifier and ARN), and which of the replica's
//! optional members the request sets — rather than over one request.
//!
//! Every call goes over the wire: real awsQuery form POSTs through a `MultiServiceRouter`
//! carrying the SigV4 credential scope the router keys on, and every assertion reads the
//! XML the emulator answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The fixed sentence Amazon RDS refuses a replica of a source with automated backups off
/// with: it names the operation that turns them on and nothing from the request.
///
/// Spelled out here so the wording is pinned by the test rather than by the code under
/// test.
const AUTOMATED_BACKUPS_OFF: &str =
    "Automated backups are not enabled for this database instance. To enable automated \
     backups, use ModifyDBInstance to set the backup retention period to a non-zero value.";

/// `CreateDBInstance` for a source instance, waited past the span the create owes so the
/// instance is `available`.
async fn source_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    retention: &str,
) {
    expect_ok(
        case,
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", engine),
                ("DBInstanceClass", "db.t3.micro"),
                ("AllocatedStorage", "20"),
                ("BackupRetentionPeriod", retention),
            ],
        )
        .await,
    );
    wait_for_settle();
}

/// `DescribeDBInstances` naming one instance.
async fn describe(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    )
}

/// One row of the replica table: the source's engine, how the request names the source, and
/// the replica's own members.
struct ReplicaCase {
    case: &'static str,
    engine: &'static str,
    source: &'static str,
    replica: &'static str,
    /// How this row names the source: `{name}` for the identifier, `{arn}` for its ARN.
    names_source_by: &'static str,
    /// The members beyond the two identifiers.
    extra: &'static [(&'static str, &'static str)],
}

/// The class the replica rule generalises over: three engines of different products, the
/// two forms a source may be named by, and a different set of the replica's optional
/// members in each row (none, the instance class and tag copying, the availability zone and
/// the storage).
const REPLICAS: &[ReplicaCase] = &[
    ReplicaCase {
        case: "a mysql source named by identifier, nothing else set",
        engine: "mysql",
        source: "replica-src-mysql",
        replica: "replica-of-mysql",
        names_source_by: "{name}",
        extra: &[],
    },
    ReplicaCase {
        case: "a postgres source named by ARN, with its own class and tag copying",
        engine: "postgres",
        source: "replica-src-postgres",
        replica: "replica-of-postgres",
        names_source_by: "{arn}",
        extra: &[
            ("DBInstanceClass", "db.t3.small"),
            ("CopyTagsToSnapshot", "true"),
        ],
    },
    ReplicaCase {
        case: "a mariadb source, with its own zone and storage",
        engine: "mariadb",
        source: "replica-src-mariadb",
        replica: "replica-of-mariadb",
        names_source_by: "{name}",
        extra: &[
            ("AvailabilityZone", "us-west-2c"),
            ("AllocatedStorage", "40"),
            ("PubliclyAccessible", "true"),
        ],
    },
];

#[tokio::test]
async fn a_replica_copies_its_source_and_both_report_the_relationship() {
    let router = emulator();

    for row in REPLICAS {
        let case = row.case;
        source_instance(&router, case, row.source, row.engine, "1").await;
        let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:{}", row.source);
        let named = row
            .names_source_by
            .replace("{name}", row.source)
            .replace("{arn}", &source_arn);

        let described_source = describe(&router, case, row.source).await;
        let engine_version = text(&described_source, "EngineVersion");

        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", row.replica.to_string()),
            ("SourceDBInstanceIdentifier", named),
        ];
        for (name, value) in row.extra {
            params.push((name, (*value).to_string()));
        }
        let created = expect_ok(
            case,
            call(&router, "CreateDBInstanceReadReplica", &params).await,
        );

        // The replica runs the SOURCE's engine and version — the operation has no `Engine`
        // member — and reports the source it replicates.
        assert_eq!(
            (
                text(&created, "DBInstanceIdentifier"),
                text(&created, "Engine"),
                text(&created, "EngineVersion"),
                text(&created, "ReadReplicaSourceDBInstanceIdentifier"),
                text(&created, "DBInstanceStatus"),
                text(&created, "DBInstanceArn"),
            ),
            (
                row.replica.to_string(),
                row.engine.to_string(),
                engine_version.clone(),
                row.source.to_string(),
                "creating".to_string(),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:{}", row.replica),
            ),
            "case {case:?}: got {created}"
        );
        // A replica keeps no automated backup of its own until one is turned on for it.
        assert_eq!(
            text(&created, "BackupRetentionPeriod"),
            "0",
            "case {case:?}: got {created}"
        );

        // The members the request named are the replica's own; the ones it did not are the
        // source's.
        for (member, value) in row.extra {
            let reported = match *member {
                "DBInstanceClass" => text(&created, "DBInstanceClass"),
                "CopyTagsToSnapshot" => text(&created, "CopyTagsToSnapshot"),
                "AvailabilityZone" => text(&created, "AvailabilityZone"),
                "AllocatedStorage" => text(&created, "AllocatedStorage"),
                "PubliclyAccessible" => text(&created, "PubliclyAccessible"),
                other => panic!("case {case:?}: no projection asserted for {other}"),
            };
            assert_eq!(
                reported, *value,
                "case {case:?}: the replica must carry the {member} the request named, got \
                 {created}"
            );
        }
        if !row.extra.iter().any(|(member, _)| *member == "DBInstanceClass") {
            assert_eq!(
                text(&created, "DBInstanceClass"),
                text(&described_source, "DBInstanceClass"),
                "case {case:?}: a member the request did not name is the source's, got {created}"
            );
        }

        // The replica reaches `available` on its own, and the read-back is what proves it
        // was persisted rather than echoed.
        wait_for_settle();
        let described_replica = describe(&router, case, row.replica).await;
        assert_eq!(
            (
                text(&described_replica, "DBInstanceStatus"),
                text(&described_replica, "ReadReplicaSourceDBInstanceIdentifier"),
            ),
            ("available".to_string(), row.source.to_string()),
            "case {case:?}: got {described_replica}"
        );
        // The SOURCE reports the replica from its own end.
        let described_source = describe(&router, case, row.source).await;
        assert!(
            texts(&described_source, "ReadReplicaDBInstanceIdentifier")
                .contains(&row.replica.to_string()),
            "case {case:?}: the source must report the replica it has, got {described_source}"
        );

        // Deleting the replica ends the relationship: the source reports no replica it does
        // not have.
        expect_ok(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", row.replica),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
        let described_source = describe(&router, case, row.source).await;
        assert!(
            !texts(&described_source, "ReadReplicaDBInstanceIdentifier")
                .contains(&row.replica.to_string()),
            "case {case:?}: a deleted replica is not one the source has, got {described_source}"
        );
    }
}

#[tokio::test]
async fn a_source_with_automated_backups_off_cannot_be_replicated_until_they_are_on() {
    let router = emulator();
    // The sources differ in engine, so the rule is not one engine's: each is created with
    // `BackupRetentionPeriod` 0, which is what "to disable automated backups, set this
    // value to 0" means.
    for (case, engine, source, replica) in [
        ("mysql", "mysql", "nobackup-mysql", "replica-nobackup-mysql"),
        (
            "postgres",
            "postgres",
            "nobackup-postgres",
            "replica-nobackup-postgres",
        ),
    ] {
        source_instance(&router, case, source, engine, "0").await;

        let refused = expect_error(
            case,
            call(
                &router,
                "CreateDBInstanceReadReplica",
                &[
                    ("DBInstanceIdentifier", replica),
                    ("SourceDBInstanceIdentifier", source),
                ],
            )
            .await,
            400,
            "InvalidDBInstanceState",
        );
        assert_eq!(
            text(&refused, "Message"),
            AUTOMATED_BACKUPS_OFF,
            "case {case:?}: got {refused}"
        );
        // The refused call left no instance behind.
        expect_error(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", replica)],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );

        // Re-enable automated backups, and the IDENTICAL call is served.
        expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", source),
                    ("BackupRetentionPeriod", "7"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        );
        wait_for_settle();
        let created = expect_ok(
            case,
            call(
                &router,
                "CreateDBInstanceReadReplica",
                &[
                    ("DBInstanceIdentifier", replica),
                    ("SourceDBInstanceIdentifier", source),
                ],
            )
            .await,
        );
        assert_eq!(
            text(&created, "ReadReplicaSourceDBInstanceIdentifier"),
            source,
            "case {case:?}: got {created}"
        );
    }
}

#[tokio::test]
async fn a_source_this_operation_cannot_replicate_is_refused_and_stores_nothing() {
    let router = emulator();
    source_instance(&router, "the source", "guard-src-mysql", "mysql", "1").await;
    // An instance that belongs to a DB cluster: its replicas are the CLUSTER's.
    expect_ok(
        "create the cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "guard-cluster"),
                ("Engine", "aurora-mysql"),
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "a-password-9"),
            ],
        )
        .await,
    );
    expect_ok(
        "create the cluster member",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "guard-src-member"),
                ("Engine", "aurora-mysql"),
                ("DBInstanceClass", "db.t3.medium"),
                ("DBClusterIdentifier", "guard-cluster"),
            ],
        )
        .await,
    );
    // A parameter group of another engine's family, and an option group of another engine:
    // neither can be applied to a replica of this source, which runs the source's engine.
    expect_ok(
        "create a parameter group of another family",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "guard-postgres-pg"),
                ("DBParameterGroupFamily", "postgres17"),
                ("Description", "another-engines-family"),
            ],
        )
        .await,
    );
    expect_ok(
        "create an option group of another engine",
        call(
            &router,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", "guard-oracle-og"),
                ("EngineName", "oracle-se2"),
                ("MajorEngineVersion", "19"),
                ("OptionGroupDescription", "another-engines-options"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // One refusal row: the parameters of a replica this operation cannot serve, and the
    // status and wire code it answers with.
    struct Guard {
        case: &'static str,
        params: &'static [(&'static str, &'static str)],
        status: u16,
        code: &'static str,
    }

    let rows: [Guard; 7] = [
        Guard {
            case: "a source identifier nothing created",
            params: &[
                ("DBInstanceIdentifier", "replica-guard-1"),
                ("SourceDBInstanceIdentifier", "no-such-source"),
            ],
            status: 404,
            code: "DBInstanceNotFound",
        },
        Guard {
            case: "a source ARN nothing created",
            params: &[
                ("DBInstanceIdentifier", "replica-guard-2"),
                (
                    "SourceDBInstanceIdentifier",
                    "arn:aws:rds:us-west-2:000000000000:db:no-such-source",
                ),
            ],
            status: 404,
            code: "DBInstanceNotFound",
        },
        Guard {
            case: "neither a source instance nor a source cluster",
            params: &[("DBInstanceIdentifier", "replica-guard-3")],
            status: 400,
            code: "InvalidParameterCombination",
        },
        Guard {
            case: "a source instance and a source cluster at once",
            params: &[
                ("DBInstanceIdentifier", "replica-guard-4"),
                ("SourceDBInstanceIdentifier", "guard-src-mysql"),
                ("SourceDBClusterIdentifier", "guard-cluster"),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
        Guard {
            case: "a source that belongs to a DB cluster",
            params: &[
                ("DBInstanceIdentifier", "replica-guard-5"),
                ("SourceDBInstanceIdentifier", "guard-src-member"),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
        Guard {
            case: "a parameter group of another engine's family",
            params: &[
                ("DBInstanceIdentifier", "replica-guard-6"),
                ("SourceDBInstanceIdentifier", "guard-src-mysql"),
                ("DBParameterGroupName", "guard-postgres-pg"),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
        Guard {
            case: "an option group of another engine",
            params: &[
                ("DBInstanceIdentifier", "replica-guard-7"),
                ("SourceDBInstanceIdentifier", "guard-src-mysql"),
                ("OptionGroupName", "guard-oracle-og"),
            ],
            status: 400,
            code: "InvalidParameterCombination",
        },
    ];

    for row in rows {
        let case = row.case;
        expect_error(
            case,
            call(&router, "CreateDBInstanceReadReplica", row.params).await,
            row.status,
            row.code,
        );
        let identifier = row
            .params
            .iter()
            .find(|(member, _)| *member == "DBInstanceIdentifier")
            .map(|(_, value)| *value)
            .unwrap_or_default();
        expect_error(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier)],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
    }

    // A cluster member's refusal is the one fixed sentence RDS answers it with.
    let refused = expect_error(
        "the cluster member's refusal wording",
        call(
            &router,
            "CreateDBInstanceReadReplica",
            &[
                ("DBInstanceIdentifier", "replica-guard-8"),
                ("SourceDBInstanceIdentifier", "guard-src-member"),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    assert_eq!(
        text(&refused, "Message"),
        "Cannot create a read replica for a DB Instance that belongs to a DB Cluster",
        "case \"the cluster member's refusal wording\": got {refused}"
    );

    // And a group of the SOURCE's own engine is served, which is what makes the two
    // refusals above about the engine rather than about naming a group at all.
    let served = expect_ok(
        "a parameter group of the source's own family",
        call(
            &router,
            "CreateDBInstanceReadReplica",
            &[
                ("DBInstanceIdentifier", "replica-guard-served"),
                ("SourceDBInstanceIdentifier", "guard-src-mysql"),
                ("DBParameterGroupName", "default.mysql8.0"),
            ],
        )
        .await,
    );
    assert_eq!(
        optional_text(&served, "ReadReplicaSourceDBInstanceIdentifier").as_deref(),
        Some("guard-src-mysql"),
        "case \"a parameter group of the source's own family\": got {served}"
    );
}
