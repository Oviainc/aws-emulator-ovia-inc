// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE three families of not-found refusal carry: the DB
//! subnet group a create PLACES A DB INSTANCE in, the DB instance the modification report
//! and the tenant-database create resolve, and the automated backup of a DB cluster.
//!
//! The rule under test: a not-found refusal answers the sentence Amazon RDS words for that
//! operation's fault — its fixed text exactly, with the resource type spelled the way the
//! modelled shape spells it and only the variable parts the service puts in it, taken from
//! the request — rather than one house-style `<type> <id> not found.` template. Concretely,
//! over the families this suite spans:
//!
//! * the calls that PLACE a DB instance in a DB subnet group (`CreateDBInstance`,
//!   `CreateDBInstanceReadReplica`, `RestoreDBInstanceFromDBSnapshot`) answer
//!   `DBSubnetGroupNotFoundFault` (404, the `awsQueryError` code of the modelled fault —
//!   `smithy_query(query="operation", operation="CreateDBInstance")`) with
//!   `DBSubnetGroup '<name>' not found.`, while the REPORT of the same condition
//!   (`DescribeDBSubnetGroups`) answers its own sentence for it.
//! * `DescribeValidDBInstanceModifications` and `CreateTenantDatabase` answer
//!   `DBInstanceNotFound` (404) with `DBInstance not found: <identifier>` — the fixed
//!   phrase first, then the identifier, and no terminal period — while
//!   `DescribeDBInstances` answers `DBInstance <identifier> not found.` for the same miss.
//! * `DescribeDBClusterAutomatedBackups` answers `DBClusterAutomatedBackupNotFoundFault`
//!   (404) as a statement about the BACKUP — `DBClusterAutomatedBackup for DBCluster
//!   <value> not found.` — naming the value the request narrowed the report by, whichever
//!   of the two members carried it.
//!
//! Each table spans what the rule generalises over: the shape of a name nothing holds (a
//! fresh one, a near miss of a stored one, a single letter, mixed case, the punctuation and
//! spaces the member admits, one at the published length bound), how it came to hold
//! nothing (never created, created and then deleted, held only by another region), which
//! member carried it, and — for one row — a value this test never spells out, taken from
//! what the emulator itself minted.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML this emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};

/// A second region, so a resource that exists but is out of the signed region's scope is a
/// row of each table.
const OTHER_REGION: &str = "ap-south-1";

/// The two subnets the smallest DB subnet group is made of.
const TWO_SUBNETS: [&str; 2] = ["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"];

/// The DB subnet group this region holds, so a near miss of its name is a row of the
/// placement table and no refusal there passes merely because the store is empty.
const STORED_GROUP: &str = "emu-place-subnets";

/// The source a read replica below is built from: a MySQL instance that retains automated
/// backups, which is what `CreateDBInstanceReadReplica` requires of a source.
const SOURCE_INSTANCE: &str = "emu-place-source";

/// The snapshot a restore below comes back from.
const SOURCE_SNAPSHOT: &str = "emu-place-snap";

/// The Oracle container-database engine: the configuration tenant databases live in
/// (`CreateDBInstance$MultiTenant` "only applies to RDS for Oracle container database (CDB)
/// engines").
const CDB_ENGINE: &str = "oracle-ee-cdb";

/// The error code every DB-instance not-found refusal below carries.
const DB_INSTANCE_NOT_FOUND: &str = "DBInstanceNotFound";

/// The sentence the calls that PLACE a DB instance in a DB subnet group answer for a group
/// they cannot resolve.
fn placement_wording(folded_name: &str) -> String {
    format!("DBSubnetGroup '{folded_name}' not found.")
}

/// The sentence `DescribeDBSubnetGroups` — the REPORT — answers for the same miss.
fn report_wording(folded_name: &str) -> String {
    format!("DB Subnet group '{folded_name}' not found.")
}

/// The sentence `DescribeValidDBInstanceModifications` and `CreateTenantDatabase` answer
/// for a DB instance they cannot resolve.
fn instance_not_found_wording(named: &str) -> String {
    format!("DBInstance not found: {named}")
}

/// The sentence `DescribeDBInstances` — the report — answers for the same miss.
fn instance_report_wording(named: &str) -> String {
    format!("DBInstance {named} not found.")
}

/// The sentence `DescribeDBClusterAutomatedBackups` answers for a value that resolves to no
/// automated backup.
fn automated_backup_wording(requested: &str) -> String {
    format!("DBClusterAutomatedBackup for DBCluster {requested} not found.")
}

/// A `DBSubnetGroupName` of exactly the 255 characters its member admits ("Must contain no
/// more than 255 letters, numbers, periods, underscores, spaces, or hyphens. First character
/// must be a letter"), so the published bound is one of the rows.
fn group_name_at_the_published_bound() -> String {
    let mut name = String::from("e");
    name.extend(std::iter::repeat_n('a', 254));
    assert_eq!(
        name.chars().count(),
        255,
        "the row at the published bound must be 255 characters, got {}",
        name.chars().count()
    );
    name
}

/// A `DBInstanceIdentifier` of exactly the 63 characters its member admits ("Must contain
/// from 1 to 63 letters, numbers, or hyphens"), so the published bound is one of the rows.
fn instance_identifier_at_the_published_bound() -> String {
    let mut identifier = String::from("e");
    identifier.extend(std::iter::repeat_n('a', 62));
    assert_eq!(
        identifier.chars().count(),
        63,
        "the row at the published bound must be 63 characters, got {}",
        identifier.chars().count()
    );
    identifier
}

/// `CreateDBSubnetGroup` in `region`, over the two subnets a group must name.
async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) {
    expect_ok(
        &format!("create the subnet group {name} in {region}"),
        call_in(
            router,
            region,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", name),
                ("DBSubnetGroupDescription", "the placement suite"),
                ("SubnetIds.SubnetIdentifier.1", TWO_SUBNETS[0]),
                ("SubnetIds.SubnetIdentifier.2", TWO_SUBNETS[1]),
            ],
        )
        .await,
    );
}

/// `CreateDBInstance` in `region`, waited past the span the create owes so the instance is
/// `available` when the calls below name it.
async fn create_available_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) {
    let mut params = vec![
        ("DBInstanceIdentifier", identifier),
        ("Engine", engine),
        ("DBInstanceClass", "db.t3.micro"),
        ("AllocatedStorage", "20"),
    ];
    params.extend_from_slice(extra);
    expect_ok(
        &format!("create the instance {identifier} in {region}"),
        call_in(router, region, "CreateDBInstance", &params).await,
    );
    wait_for_settle();
}

/// One create that places the DB instance it makes in a DB subnet group.
struct PlacingCreate {
    action: &'static str,
    /// The members the call carries beyond `DBInstanceIdentifier` and `DBSubnetGroupName`.
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation of this service that places a DB INSTANCE in a DB subnet group, which is
/// the class the placement wording holds over.
const PLACING_CREATES: &[PlacingCreate] = &[
    PlacingCreate {
        action: "CreateDBInstance",
        extra: &[
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.t3.micro"),
            ("AllocatedStorage", "20"),
        ],
    },
    PlacingCreate {
        action: "CreateDBInstanceReadReplica",
        extra: &[("SourceDBInstanceIdentifier", SOURCE_INSTANCE)],
    },
    PlacingCreate {
        action: "RestoreDBInstanceFromDBSnapshot",
        extra: &[("DBSnapshotIdentifier", SOURCE_SNAPSHOT)],
    },
];

#[tokio::test]
async fn the_creates_that_place_an_instance_word_the_subnet_group_fault_their_own_way() {
    let router = emulator();

    // A group this region holds, one only the OTHER region holds, and one this region held
    // until its delete ran.
    create_group(&router, REGION, STORED_GROUP).await;
    create_group(&router, OTHER_REGION, "emu-place-elsewhere").await;
    create_group(&router, REGION, "emu-place-deleted").await;
    expect_ok(
        "the delete of a group that exists",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", "emu-place-deleted")],
        )
        .await,
    );

    // The source a read replica is built from and the snapshot a restore comes back from:
    // both creates resolve their source BEFORE the group, so every row below reaches the
    // group resolution.
    create_available_instance(
        &router,
        REGION,
        SOURCE_INSTANCE,
        "mysql",
        &[("BackupRetentionPeriod", "7")],
    )
    .await;
    expect_ok(
        "the snapshot a restore comes back from",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", SOURCE_INSTANCE),
                ("DBSnapshotIdentifier", SOURCE_SNAPSHOT),
            ],
        )
        .await,
    );
    wait_for_settle();

    let at_the_bound = group_name_at_the_published_bound();
    let rows: Vec<(&str, &str)> = vec![
        ("a name nothing was created under", "emu-place-ghost"),
        ("a near miss of a stored name", "emu-place-subnets-2"),
        ("a single letter", "e"),
        (
            "a name in mixed case, folded on the way in",
            "EMU-Place-Ghost-Mixed",
        ),
        (
            "the periods, underscores and spaces the member admits",
            "emu.place_ghost 3",
        ),
        (
            "a name at the published 255-character bound",
            at_the_bound.as_str(),
        ),
        ("a group only another region holds", "emu-place-elsewhere"),
        ("a group whose delete has already run", "emu-place-deleted"),
    ];

    for (index, (case, name)) in rows.iter().enumerate() {
        let folded = name.to_ascii_lowercase();
        let expected = placement_wording(&folded);

        for (op_index, op) in PLACING_CREATES.iter().enumerate() {
            let case = format!("{case} named by {}", op.action);
            let target = format!("emu-place-t{index}-{op_index}");
            let mut params: Vec<(&str, String)> = vec![
                ("DBInstanceIdentifier", target.clone()),
                ("DBSubnetGroupName", (*name).to_string()),
            ];
            params.extend(
                op.extra
                    .iter()
                    .map(|(member, value)| (*member, (*value).to_string())),
            );
            let refused = expect_error(
                &case,
                call(&router, op.action, &params).await,
                404,
                "DBSubnetGroupNotFoundFault",
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the placement wording {expected:?}, got {message:?}"
            );

            // A refused create left no instance behind for a later call to find.
            expect_error(
                &case,
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", target.as_str())],
                )
                .await,
                404,
                DB_INSTANCE_NOT_FOUND,
            );
        }

        // The REPORT of the same condition answers its own sentence: the contrast is what
        // makes the placement wording a rule rather than one house style.
        let reported = expect_error(
            case,
            call(
                &router,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", *name)],
            )
            .await,
            404,
            "DBSubnetGroupNotFoundFault",
        );
        let reported = text(&reported, "Message");
        assert_eq!(
            reported,
            report_wording(&folded),
            "case {case:?}: expected the report's own wording, got {reported:?}"
        );
        assert_ne!(
            reported, expected,
            "case {case:?}: the placement and the report must not answer one \
             undifferentiated sentence"
        );
    }

    // Each of the three creates IS served for the group this region holds, so the refusals
    // above are about resolving the group rather than about the operation — and the
    // placement is read back through the report.
    for (op_index, op) in PLACING_CREATES.iter().enumerate() {
        let case = format!("{} naming the stored group", op.action);
        let target = format!("emu-place-served-{op_index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", target.clone()),
            ("DBSubnetGroupName", STORED_GROUP.to_string()),
        ];
        params.extend(
            op.extra
                .iter()
                .map(|(member, value)| (*member, (*value).to_string())),
        );
        expect_ok(&case, call(&router, op.action, &params).await);
        let reported = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", target.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&reported, "DBSubnetGroupName"),
            STORED_GROUP,
            "case {case:?}: expected the instance to report the group it was placed in, \
             got {reported}"
        );
    }
}

#[tokio::test]
async fn the_instance_resolving_calls_put_the_phrase_before_the_identifier() {
    let router = emulator();

    // An instance this region holds (a container database, so the tenant create below is
    // served for it), one only the OTHER region holds, and one this region held until its
    // delete ran.
    create_available_instance(&router, REGION, "emu-vmod-cdb", CDB_ENGINE, &[]).await;
    create_available_instance(&router, OTHER_REGION, "emu-vmod-elsewhere", "mysql", &[]).await;
    create_available_instance(&router, REGION, "emu-vmod-deleted", "mysql", &[]).await;
    expect_ok(
        "the delete of an instance that exists",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-vmod-deleted"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let at_the_bound = instance_identifier_at_the_published_bound();
    let rows: Vec<(&str, &str)> = vec![
        ("an identifier nothing was created under", "emu-vmod-ghost"),
        ("a near miss of a stored identifier", "emu-vmod-cdb-2"),
        ("a single letter", "e"),
        (
            "an identifier at the published 63-character bound",
            at_the_bound.as_str(),
        ),
        (
            "an instance only another region holds",
            "emu-vmod-elsewhere",
        ),
        (
            "an instance whose delete has already run",
            "emu-vmod-deleted",
        ),
    ];

    for (index, (case, identifier)) in rows.iter().enumerate() {
        let expected = instance_not_found_wording(identifier);

        let refused = expect_error(
            case,
            call(
                &router,
                "DescribeValidDBInstanceModifications",
                &[("DBInstanceIdentifier", *identifier)],
            )
            .await,
            404,
            DB_INSTANCE_NOT_FOUND,
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected DescribeValidDBInstanceModifications to answer \
             {expected:?}, got {message:?}"
        );

        let refused = expect_error(
            case,
            call(
                &router,
                "CreateTenantDatabase",
                &[
                    ("DBInstanceIdentifier", *identifier),
                    ("TenantDBName", &format!("tenant{index}")),
                    ("MasterUsername", "tenantadmin"),
                    ("MasterUserPassword", "a-secret-password"),
                ],
            )
            .await,
            404,
            DB_INSTANCE_NOT_FOUND,
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected CreateTenantDatabase to answer {expected:?}, \
             got {message:?}"
        );

        // The REPORT of the same miss answers the other phrase order, which is the contrast
        // that makes the wording a per-operation rule rather than one house style.
        let reported = expect_error(
            case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", *identifier)],
            )
            .await,
            404,
            DB_INSTANCE_NOT_FOUND,
        );
        let reported = text(&reported, "Message");
        assert_eq!(
            reported,
            instance_report_wording(identifier),
            "case {case:?}: expected the report's own wording, got {reported:?}"
        );
        assert_ne!(
            reported, expected,
            "case {case:?}: the two phrase orders must not collapse into one sentence"
        );
    }

    // Each call names the identifier in the spelling it resolves the member in:
    // `DescribeValidDBInstanceModifications` takes "the customer identifier or the ARN of
    // your DB instance", so it names the value the caller sent, while
    // `CreateTenantDatabase` resolves a stored identifier, which is folded ("stored as a
    // lowercase string").
    let mixed = "EMU-Vmod-Ghost-Mixed";
    let refused = expect_error(
        "a mixed-case identifier nothing holds",
        call(
            &router,
            "DescribeValidDBInstanceModifications",
            &[("DBInstanceIdentifier", mixed)],
        )
        .await,
        404,
        DB_INSTANCE_NOT_FOUND,
    );
    let message = text(&refused, "Message");
    assert_eq!(
        message,
        instance_not_found_wording(mixed),
        "case {mixed:?}: expected the value the caller sent, got {message:?}"
    );
    let refused = expect_error(
        "a mixed-case identifier nothing holds",
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", mixed),
                ("TenantDBName", "tenantmixed"),
                ("MasterUsername", "tenantadmin"),
                ("MasterUserPassword", "a-secret-password"),
            ],
        )
        .await,
        404,
        DB_INSTANCE_NOT_FOUND,
    );
    let message = text(&refused, "Message");
    assert_eq!(
        message,
        instance_not_found_wording(&mixed.to_ascii_lowercase()),
        "case {mixed:?}: expected the folded spelling the identifier is stored under, \
         got {message:?}"
    );

    // An ARN of another region names nothing addressable here, and the sentence carries the
    // ARN the caller sent.
    let remote_arn = format!(
        "arn:aws:rds:{OTHER_REGION}:000000000000:db:emu-vmod-elsewhere"
    );
    let refused = expect_error(
        "the ARN of an instance only another region holds",
        call(
            &router,
            "DescribeValidDBInstanceModifications",
            &[("DBInstanceIdentifier", remote_arn.as_str())],
        )
        .await,
        404,
        DB_INSTANCE_NOT_FOUND,
    );
    let message = text(&refused, "Message");
    assert_eq!(
        message,
        instance_not_found_wording(&remote_arn),
        "expected the ARN the caller sent, got {message:?}"
    );

    // Both calls ARE served for the instance this region holds, so the refusals above are
    // about resolving the identifier rather than about the operation.
    let reported = expect_ok(
        "the instance this region holds",
        call(
            &router,
            "DescribeValidDBInstanceModifications",
            &[("DBInstanceIdentifier", "emu-vmod-cdb")],
        )
        .await,
    );
    assert!(
        !texts(&reported, "StorageType").is_empty(),
        "expected the report to carry the storage a modification can choose, got {reported}"
    );
    let created = expect_ok(
        "the tenant database of the instance this region holds",
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-vmod-cdb"),
                ("TenantDBName", "tenantserved"),
                ("MasterUsername", "tenantadmin"),
                ("MasterUserPassword", "a-secret-password"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "TenantDBName"),
        "tenantserved",
        "expected the create to report the tenant database it made, got {created}"
    );
    wait_for_settle();
    let reported = expect_ok(
        "the tenant database read back",
        call(
            &router,
            "DescribeTenantDatabases",
            &[("DBInstanceIdentifier", "emu-vmod-cdb")],
        )
        .await,
    );
    assert_eq!(
        text(&reported, "TenantDBName"),
        "tenantserved",
        "expected the report to read the created tenant database back, got {reported}"
    );
}

#[tokio::test]
async fn the_automated_backup_report_names_the_cluster_it_was_asked_about() {
    let router = emulator();

    // A cluster this region holds, so a near miss of its identifier is a row, and one only
    // the OTHER region holds — whose minted resource id is a row this test never spells out.
    for (region, identifier) in [
        (REGION, "emu-backup-cluster"),
        (OTHER_REGION, "emu-backup-elsewhere"),
    ] {
        expect_ok(
            &format!("create the cluster {identifier} in {region}"),
            call_in(
                &router,
                region,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier),
                    ("Engine", "aurora-mysql"),
                ],
            )
            .await,
        );
    }
    wait_for_settle();
    let elsewhere = expect_ok(
        "the other region's cluster",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-backup-elsewhere")],
        )
        .await,
    );
    let remote_resource_id = text(&elsewhere, "DbClusterResourceId");
    assert!(
        !remote_resource_id.is_empty(),
        "the cluster must report the resource id it was minted with, got {elsewhere}"
    );

    // `(case, the member that narrows the report, the value it carries)`.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "an identifier nothing was created under",
            "DBClusterIdentifier",
            "emu-backup-ghost",
        ),
        (
            "a near miss of a stored identifier",
            "DBClusterIdentifier",
            "emu-backup-cluster-2",
        ),
        ("a single letter", "DBClusterIdentifier", "e"),
        (
            "an identifier in mixed case that folds to nothing",
            "DBClusterIdentifier",
            "EMU-Backup-Ghost",
        ),
        (
            "a cluster only another region holds",
            "DBClusterIdentifier",
            "emu-backup-elsewhere",
        ),
        (
            "a resource id of the published form that names nothing",
            "DbClusterResourceId",
            "cluster-ZZZZZZZZZZZZZZZZZZZZZZZZZZ",
        ),
        (
            "the resource id of a cluster only another region holds",
            "DbClusterResourceId",
            remote_resource_id.as_str(),
        ),
    ];

    for (case, member, value) in rows {
        let refused = expect_error(
            case,
            call(
                &router,
                "DescribeDBClusterAutomatedBackups",
                &[(member, value)],
            )
            .await,
            404,
            "DBClusterAutomatedBackupNotFoundFault",
        );
        let message = text(&refused, "Message");
        let expected = automated_backup_wording(value);
        assert_eq!(
            message, expected,
            "case {case:?} in {member}: expected {expected:?}, got {message:?}"
        );
    }

    // The backup of the cluster this region DOES hold is reported, by either member, so the
    // refusals above are about resolving the cluster rather than about the operation.
    let local = expect_ok(
        "this region's cluster",
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-backup-cluster")],
        )
        .await,
    );
    let resource_id = text(&local, "DbClusterResourceId");
    for (case, member, value) in [
        (
            "the stored cluster's identifier",
            "DBClusterIdentifier",
            "emu-backup-cluster",
        ),
        (
            "the stored cluster's resource id",
            "DbClusterResourceId",
            resource_id.as_str(),
        ),
    ] {
        let reported = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterAutomatedBackups",
                &[(member, value)],
            )
            .await,
        );
        assert_eq!(
            text(&reported, "DBClusterIdentifier"),
            "emu-backup-cluster",
            "case {case:?}: expected the backup of the stored cluster, got {reported}"
        );
    }
}
