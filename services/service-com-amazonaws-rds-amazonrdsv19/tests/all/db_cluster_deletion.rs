// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB-cluster DELETE lifecycle: `DeleteDBCluster` and the
//! `DescribeDBClusters` / `DescribeDBInstances` / `DescribeGlobalClusters` reads that
//! follow it.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on.
//!
//! The rule under test: a delete RETIRES the resource it names. `DeleteDBCluster` answers
//! HTTP 200 reporting the cluster in the transient `deleting` status, the record then
//! leaves the service, and every later read of that identifier is the modelled
//! `DBClusterNotFoundFault` (HTTP 404) — so the delete-then-wait-for-gone idiom every AWS
//! waiter is written around terminates. A cluster other records still depend on, or whose
//! delete is already running, is refused with the modelled state fault and left exactly as
//! it was.
//!
//! The table below spans what the rule generalises over: both cluster TYPES
//! `CreateDBCluster` documents (Aurora DB clusters and Multi-AZ DB clusters) across five
//! of the engines `Engine` publishes, the status the cluster is in when the delete arrives
//! (a provisioning status the clock has yet to settle, and a settled one), and each form
//! the delete may name the cluster by (its own spelling, another case — "this parameter
//! isn't case-sensitive" — and its ARN).

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// How long a test waits before expecting a change this service owes to have settled: the
/// span itself plus slack for a loaded test machine.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// How the delete NAMES the cluster it retires.
#[derive(Debug, Clone, Copy)]
enum NamedBy {
    /// The identifier exactly as the create spelled it.
    AsCreated,
    /// The identifier in another case: "this parameter isn't case-sensitive".
    OtherCase,
    /// The cluster's ARN, the other form the members that name a DB cluster accept.
    Arn,
}

/// The status the cluster is in when the delete arrives, and how the test gets it there.
/// Every one of them must accept the delete.
#[derive(Debug, Clone, Copy)]
enum StartingStatus {
    /// Straight after `CreateDBCluster`, before the clock has settled the cluster: the
    /// status the recorded teardown deleted from.
    Creating,
    /// After the create has settled.
    Available,
}

impl StartingStatus {
    /// The `Status` the cluster reports at this point.
    fn reported(self) -> &'static str {
        match self {
            StartingStatus::Creating => "creating",
            StartingStatus::Available => "available",
        }
    }
}

/// One row of the delete table.
struct DeleteCase {
    case: &'static str,
    /// The `DBClusterIdentifier` the create names, which is stored lowercased.
    identifier: &'static str,
    engine: &'static str,
    /// The parameters this row adds beyond `DBClusterIdentifier` and `Engine`.
    extra: &'static [(&'static str, &'static str)],
    status: StartingStatus,
    named_by: NamedBy,
}

/// The settings a Multi-AZ DB cluster requires, as `CreateDBCluster` documents them for
/// that cluster type ("This setting is required to create a Multi-AZ DB cluster").
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "100"),
    ("DBClusterInstanceClass", "db.m6gd.xlarge"),
    ("StorageType", "io1"),
    ("Iops", "1000"),
];

/// `DeleteDBCluster`'s own `Status` for an accepted delete, and the one a read inside the
/// span it owes reports.
const DELETING: &str = "deleting";

/// `CreateDBCluster` with its two required members plus `extra`.
async fn create_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
    ];
    request.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBCluster", &request).await
}

/// The `Status` `DescribeDBClusters` reports for `identifier`, or `None` when the read is
/// the modelled 404.
async fn reported_status(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeDBClusters",
        &[("DBClusterIdentifier", identifier)],
    )
    .await;
    if status == 404 {
        return None;
    }
    assert_eq!(
        status, 200,
        "reading {identifier} must be a 200 or the modelled 404, got {status} with body {body}"
    );
    let clusters = subtrees(&body, "DBCluster");
    assert_eq!(
        clusters.len(),
        1,
        "reading {identifier} must report exactly one DBCluster, got {}",
        clusters.len()
    );
    Some(text(&clusters[0], "Status"))
}

/// `DeleteDBCluster` for one cluster, skipping the final snapshot.
async fn delete_cluster(router: &MultiServiceRouter, named: &str) -> (u16, String) {
    call(
        router,
        "DeleteDBCluster",
        &[
            ("DBClusterIdentifier", named),
            ("SkipFinalSnapshot", "true"),
        ],
    )
    .await
}

#[tokio::test]
async fn a_delete_retires_the_cluster_and_the_read_then_answers_the_modelled_not_found() {
    let table: &[DeleteCase] = &[
        DeleteCase {
            case: "aurora-postgresql, still creating, named as created",
            identifier: "emu-dc-pg-creating",
            engine: "aurora-postgresql",
            extra: &[("EnableHttpEndpoint", "true")],
            status: StartingStatus::Creating,
            named_by: NamedBy::AsCreated,
        },
        DeleteCase {
            case: "aurora-mysql, available, named in another case",
            identifier: "emu-dc-mysql-available",
            engine: "aurora-mysql",
            extra: &[],
            status: StartingStatus::Available,
            named_by: NamedBy::OtherCase,
        },
        DeleteCase {
            case: "docdb, available, named by its ARN",
            identifier: "emu-dc-docdb-arn",
            engine: "docdb",
            extra: &[],
            status: StartingStatus::Available,
            named_by: NamedBy::Arn,
        },
        DeleteCase {
            case: "neptune, still creating, named as created",
            identifier: "emu-dc-neptune-creating",
            engine: "neptune",
            extra: &[],
            status: StartingStatus::Creating,
            named_by: NamedBy::AsCreated,
        },
        DeleteCase {
            case: "mysql: a Multi-AZ DB cluster, still creating",
            identifier: "emu-dc-multiaz-creating",
            engine: "mysql",
            extra: MULTI_AZ_SETTINGS,
            status: StartingStatus::Creating,
            named_by: NamedBy::AsCreated,
        },
        DeleteCase {
            case: "postgres: a Multi-AZ DB cluster, available, named by its ARN",
            identifier: "emu-dc-multiaz-available",
            engine: "postgres",
            extra: MULTI_AZ_SETTINGS,
            status: StartingStatus::Available,
            named_by: NamedBy::Arn,
        },
    ];

    for row in table {
        // Each row gets its own emulator, so the timing of one row's transitions cannot
        // carry into the next.
        let router = emulator();
        expect_ok(
            row.case,
            create_cluster(&router, row.identifier, row.engine, row.extra).await,
        );
        if matches!(row.status, StartingStatus::Available) {
            wait_for_settle();
        }
        let before = reported_status(&router, row.identifier).await;
        assert_eq!(
            before.as_deref(),
            Some(row.status.reported()),
            "case {:?}: the cluster must be deleted from status {:?}, got {before:?}",
            row.case,
            row.status.reported()
        );

        let named = match row.named_by {
            NamedBy::AsCreated => row.identifier.to_string(),
            NamedBy::OtherCase => row.identifier.to_ascii_uppercase(),
            NamedBy::Arn => {
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{}", row.identifier)
            }
        };
        let accepted = expect_ok(row.case, delete_cluster(&router, &named).await);
        let reported = subtrees(&accepted, "DBCluster");
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the delete must report the cluster it accepted, got {accepted}",
            row.case
        );
        assert_eq!(
            text(&reported[0], "DBClusterIdentifier"),
            row.identifier,
            "case {:?}: the delete must report the stored identifier, got {accepted}",
            row.case
        );
        let status_in_response = text(&reported[0], "Status");
        assert_eq!(
            status_in_response, DELETING,
            "case {:?}: an accepted delete must report {DELETING:?}, got {status_in_response:?}",
            row.case
        );

        // Inside the span the clock owes, the cluster is still readable and reports the
        // same transient status the delete answered with.
        let during = reported_status(&router, row.identifier).await;
        assert_eq!(
            during.as_deref(),
            Some(DELETING),
            "case {:?}: a read inside the deleting window must report {DELETING:?}, got \
             {during:?}",
            row.case
        );

        wait_for_settle();

        // And then it is gone: the targeted read, the unfiltered report and a second
        // delete all agree the identifier names nothing.
        let after = reported_status(&router, row.identifier).await;
        assert!(
            after.is_none(),
            "case {:?}: the cluster must be gone once the delete has run, got {after:?}",
            row.case
        );
        let not_found = expect_error(
            row.case,
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", row.identifier)],
            )
            .await,
            404,
            "DBClusterNotFoundFault",
        );
        assert!(
            text(&not_found, "Message").contains(row.identifier),
            "case {:?}: the not-found must name the identifier, got {:?}",
            row.case,
            text(&not_found, "Message")
        );
        assert!(
            subtrees(&not_found, "DBCluster").is_empty(),
            "case {:?}: the not-found must carry no DBCluster, got {not_found}",
            row.case
        );
        let listed = expect_ok(
            row.case,
            call(&router, "DescribeDBClusters", &[] as &[(&str, String)]).await,
        );
        let identifiers = texts(&listed, "DBClusterIdentifier");
        assert!(
            !identifiers.contains(&row.identifier.to_string()),
            "case {:?}: the deleted cluster must be gone from the unfiltered report, got \
             {identifiers:?}",
            row.case
        );
        expect_error(
            row.case,
            delete_cluster(&router, &named).await,
            404,
            "DBClusterNotFoundFault",
        );
    }
}

#[tokio::test]
async fn the_documented_refusals_leave_the_cluster_exactly_as_it_was() {
    let router = emulator();
    expect_ok(
        "create the protected cluster",
        create_cluster(
            &router,
            "emu-dc-protected",
            "aurora-postgresql",
            &[("DeletionProtection", "true")],
        )
        .await,
    );
    expect_ok(
        "create the cluster deleted twice",
        create_cluster(&router, "emu-dc-twice", "aurora-mysql", &[]).await,
    );
    wait_for_settle();

    // "The database can't be deleted when deletion protection is enabled." Deletion
    // protection is a setting the request conflicts with, not a state the cluster is
    // passing through, so the refusal is the Query API's parameter-combination code and the
    // sentence the service words that class with — spelled out here, and naming the
    // resource KIND rather than the identifier the request sent.
    let refused = expect_error(
        "deletion protection",
        delete_cluster(&router, "emu-dc-protected").await,
        400,
        "InvalidParameterCombination",
    );
    assert_eq!(
        text(&refused, "Message"),
        "Cannot delete protected Cluster, please disable deletion protection and try again.",
        "the refusal must carry the sentence this class is worded with, got {:?}",
        text(&refused, "Message")
    );
    let untouched = reported_status(&router, "emu-dc-protected").await;
    assert_eq!(
        untouched.as_deref(),
        Some("available"),
        "a refused delete must leave the cluster as it was, got {untouched:?}"
    );

    // A delete that is already running cannot be started again: it "can't be canceled or
    // reverted after it begins".
    expect_ok(
        "the first delete",
        delete_cluster(&router, "emu-dc-twice").await,
    );
    let twice = expect_error(
        "a second delete inside the deleting window",
        delete_cluster(&router, "emu-dc-twice").await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&twice, "Message");
    assert!(
        message.contains("emu-dc-twice") && message.contains(DELETING),
        "the refusal must say the delete is already running, got {message:?}"
    );
    let during = reported_status(&router, "emu-dc-twice").await;
    assert_eq!(
        during.as_deref(),
        Some(DELETING),
        "the refused second delete must leave the first one running, got {during:?}"
    );
}

#[tokio::test]
async fn an_aurora_cluster_that_still_has_db_instances_is_refused_until_they_are_gone() {
    let router = emulator();
    expect_ok(
        "create the cluster",
        create_cluster(&router, "emu-dc-members", "aurora-mysql", &[]).await,
    );
    for identifier in ["emu-dc-writer", "emu-dc-reader"] {
        expect_ok(
            identifier,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "aurora-mysql"),
                    ("DBInstanceClass", "db.serverless"),
                    ("DBClusterIdentifier", "emu-dc-members"),
                ],
            )
            .await,
        );
    }
    wait_for_settle();

    let refused = expect_error(
        "an Aurora cluster that still has members",
        delete_cluster(&router, "emu-dc-members").await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&refused, "Message");
    for member in ["emu-dc-writer", "emu-dc-reader"] {
        assert!(
            message.contains(member),
            "the refusal must name the member {member} holding the cluster, got {message:?}"
        );
    }
    let untouched = reported_status(&router, "emu-dc-members").await;
    assert_eq!(
        untouched.as_deref(),
        Some("available"),
        "a refused delete must leave the cluster as it was, got {untouched:?}"
    );

    // Deleting the members is what releases the cluster, which is the order the API
    // documents for an Aurora DB cluster.
    for member in ["emu-dc-writer", "emu-dc-reader"] {
        expect_ok(
            member,
            call(
                &router,
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", member),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
    }
    wait_for_settle();
    let accepted = expect_ok(
        "the cluster once its members are gone",
        delete_cluster(&router, "emu-dc-members").await,
    );
    assert_eq!(
        text(&accepted, "Status"),
        DELETING,
        "the delete must be accepted once the members are gone, got {accepted}"
    );
    wait_for_settle();
    let after = reported_status(&router, "emu-dc-members").await;
    assert!(
        after.is_none(),
        "the cluster must be gone once the delete has run, got {after:?}"
    );
}

#[tokio::test]
async fn a_multi_az_cluster_delete_terminates_its_members() {
    let router = emulator();
    let mut settings: Vec<(&str, &str)> = MULTI_AZ_SETTINGS.to_vec();
    settings.push(("DBSubnetGroupName", "default"));
    expect_ok(
        "create the Multi-AZ DB cluster",
        create_cluster(&router, "emu-dc-multiaz-members", "mysql", &settings).await,
    );
    expect_ok(
        "create its member instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-dc-multiaz-member"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m6gd.xlarge"),
                ("DBClusterIdentifier", "emu-dc-multiaz-members"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // "If you're deleting a Multi-AZ DB cluster with read replicas, all cluster members
    // are terminated": the delete is accepted and the member goes with the cluster.
    let accepted = expect_ok(
        "the Multi-AZ DB cluster delete",
        delete_cluster(&router, "emu-dc-multiaz-members").await,
    );
    assert_eq!(
        text(&accepted, "Status"),
        DELETING,
        "the delete must be accepted for a Multi-AZ DB cluster with members, got {accepted}"
    );
    let member = expect_ok(
        "the member inside the deleting window",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-dc-multiaz-member")],
        )
        .await,
    );
    assert_eq!(
        text(&member, "DBInstanceStatus"),
        DELETING,
        "the cluster's member must be terminated with it, got {member}"
    );

    wait_for_settle();
    let after = reported_status(&router, "emu-dc-multiaz-members").await;
    assert!(
        after.is_none(),
        "the cluster must be gone once the delete has run, got {after:?}"
    );
    expect_error(
        "the member once the delete has run",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-dc-multiaz-member")],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );
}

#[tokio::test]
async fn a_replica_of_a_deleted_cluster_stops_naming_it() {
    let router = emulator();
    expect_ok(
        "create the source cluster",
        create_cluster(&router, "emu-dc-source", "aurora-mysql", &[]).await,
    );
    expect_ok(
        "create the replica cluster",
        create_cluster(
            &router,
            "emu-dc-replica",
            "aurora-mysql",
            &[("ReplicationSourceIdentifier", "emu-dc-source")],
        )
        .await,
    );
    wait_for_settle();
    let replica = expect_ok(
        "the replica before the source is deleted",
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-dc-replica")],
        )
        .await,
    );
    assert_eq!(
        optional_text(&replica, "ReplicationSourceIdentifier").as_deref(),
        Some(format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-dc-source").as_str()),
        "expected the replica to name the source it was created from, got {replica}"
    );

    expect_ok(
        "delete the source cluster",
        delete_cluster(&router, "emu-dc-source").await,
    );
    wait_for_settle();

    // No stored record may name a resource the service no longer holds: the replica is
    // standalone from here.
    let promoted = expect_ok(
        "the replica once the source is gone",
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-dc-replica")],
        )
        .await,
    );
    assert_eq!(
        optional_text(&promoted, "ReplicationSourceIdentifier"),
        None,
        "expected the replica of a deleted cluster to be standalone, got {promoted}"
    );
}

#[tokio::test]
async fn a_global_databases_members_are_deleted_secondaries_first() {
    let router = emulator();
    expect_ok(
        "create the global cluster",
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-dc-global"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    // The first member is the global database's primary (writer); the second is a
    // secondary.
    for identifier in ["emu-dc-global-primary", "emu-dc-global-secondary"] {
        expect_ok(
            identifier,
            create_cluster(
                &router,
                identifier,
                "aurora-mysql",
                &[("GlobalClusterIdentifier", "emu-dc-global")],
            )
            .await,
        );
    }
    wait_for_settle();

    // The primary can't go while the global database still has a secondary.
    let refused = expect_error(
        "the primary of a global database that still has a secondary",
        delete_cluster(&router, "emu-dc-global-primary").await,
        400,
        "InvalidGlobalClusterStateFault",
    );
    assert!(
        text(&refused, "Message").contains("emu-dc-global"),
        "the refusal must name the global database, got {:?}",
        text(&refused, "Message")
    );

    // The secondary goes first, and the global database stops reporting it as a member.
    expect_ok(
        "delete the secondary",
        delete_cluster(&router, "emu-dc-global-secondary").await,
    );
    wait_for_settle();
    let global = expect_ok(
        "the global database once the secondary is gone",
        call(
            &router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-dc-global")],
        )
        .await,
    );
    assert_eq!(
        texts(&global, "DBClusterArn"),
        vec![format!(
            "arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-dc-global-primary"
        )],
        "expected only the primary to remain a member, got {global}"
    );

    // And then the primary itself, which leaves the global database empty.
    expect_ok(
        "delete the primary",
        delete_cluster(&router, "emu-dc-global-primary").await,
    );
    wait_for_settle();
    let empty = expect_ok(
        "the global database once both members are gone",
        call(
            &router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-dc-global")],
        )
        .await,
    );
    assert!(
        texts(&empty, "DBClusterArn").is_empty(),
        "expected the global database to hold no member, got {empty}"
    );
    expect_ok(
        "delete the now-empty global database",
        call(
            &router,
            "DeleteGlobalCluster",
            &[("GlobalClusterIdentifier", "emu-dc-global")],
        )
        .await,
    );
}

#[tokio::test]
async fn the_final_snapshot_members_are_refused_as_the_api_documents_them() {
    let router = emulator();
    expect_ok(
        "create the cluster",
        create_cluster(&router, "emu-dc-snapshot", "aurora-postgresql", &[]).await,
    );
    wait_for_settle();

    // "If SkipFinalSnapshot is disabled, you must specify a value for the
    // FinalDBSnapshotIdentifier parameter", and naming both "results in an error":
    // neither combination holds together, which is the code RDS answers that with.
    for (case, params) in [
        (
            "neither member: the default asks for a final snapshot with no name",
            vec![("DBClusterIdentifier", "emu-dc-snapshot")],
        ),
        (
            "both members: a skipped snapshot cannot carry a name",
            vec![
                ("DBClusterIdentifier", "emu-dc-snapshot"),
                ("SkipFinalSnapshot", "true"),
                ("FinalDBSnapshotIdentifier", "emu-dc-snapshot-final"),
            ],
        ),
    ] {
        expect_error(
            case,
            call(&router, "DeleteDBCluster", &params).await,
            400,
            "InvalidParameterCombination",
        );
    }

    // "Constraints: Must be 1 to 255 letters, numbers, or hyphens. First character must be
    // a letter. Can't end with a hyphen or contain two consecutive hyphens."
    for (case, snapshot) in [
        ("a name that is not in the alphabet", "emu_dc_final"),
        ("a name that does not start with a letter", "1-emu-dc-final"),
        ("a name that ends with a hyphen", "emu-dc-final-"),
        ("a name with two consecutive hyphens", "emu--dc-final"),
    ] {
        let refused = expect_error(
            case,
            call(
                &router,
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", "emu-dc-snapshot"),
                    ("FinalDBSnapshotIdentifier", snapshot),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        assert!(
            text(&refused, "Message").contains("FinalDBSnapshotIdentifier"),
            "case {case:?}: the refusal must name the member, got {:?}",
            text(&refused, "Message")
        );
    }

    // A well-formed request for a final DB cluster snapshot asks for a snapshot this
    // operation does not take yet — that path is its own stub, while the snapshot a caller
    // asks for directly is served by `CreateDBClusterSnapshot` — so it is answered through
    // the 501 channel rather than accepted with the snapshot quietly dropped.
    expect_error(
        "a well-formed final DB cluster snapshot",
        call(
            &router,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "emu-dc-snapshot"),
                ("FinalDBSnapshotIdentifier", "emu-dc-snapshot-final"),
            ],
        )
        .await,
        501,
        "NotImplemented",
    );

    // Every refusal above left the cluster exactly as it was.
    let untouched = reported_status(&router, "emu-dc-snapshot").await;
    assert_eq!(
        untouched.as_deref(),
        Some("available"),
        "a refused delete must leave the cluster as it was, got {untouched:?}"
    );
}
