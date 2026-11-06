// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the operations that act on a DB CLUSTER itself —
//! `StopDBCluster`, `FailoverDBCluster` and `CreateDBClusterSnapshot` — and for the
//! wording this front door answers `DBClusterNotFoundFault` with.
//!
//! Two rules are pinned here.
//!
//! The first is about the refusal: a request naming a DB cluster the service does not hold
//! is the modelled `DBClusterNotFoundFault` (HTTP 404, `text/xml`, `Error/Type` `Sender`)
//! whose `<Message>` is the template THAT operation's API uses — this front door words the
//! one fault by the role the cluster plays in the call — and, where that template has a
//! variable part, names the value the REQUEST carried, in the spelling it was written in,
//! rather than the key the store folded it to. The modification path's template has no
//! variable part at all.
//! It is asserted over a TABLE spanning the operations that name a cluster and, for each,
//! the three forms the members that name one accept ("the user-supplied DB cluster
//! identifier or the Amazon Resource Name (ARN)", "this parameter isn't case-sensitive"),
//! with another cluster stored throughout so no 404 can come from an empty store. The tag
//! operations, which address their resource by `ResourceName` instead, are asserted the
//! same way across several resource families.
//!
//! The second is about what the three operations DO: a stop moves the cluster and its
//! members to `stopped` through the transient status, a failover moves which member
//! `DBClusterMembers` reports `IsClusterWriter` for, and a snapshot of a cluster is read
//! back through `DescribeDBClusterSnapshots` carrying that cluster's settings. Each is
//! refused with the modelled state fault in a status that cannot take it.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::describe_one;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, subtrees, text, texts, wait_for_settle, REGION,
};
use crate::resource_tags::TAG_ACTIONS;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// An identifier no call below ever creates a cluster for.
const ABSENT: &str = "emu-absent-cluster";

/// The one cluster the refusal tables keep stored, so a 404 is a decision about the
/// identifier the request named and never "this service holds no cluster at all".
const PRESENT: &str = "emu-present-cluster";

/// The ARN a stored DB cluster carries, which is also the other form every member that
/// names a cluster accepts.
pub(crate) fn cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

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

/// Create a cluster and wait until the clock has made it `available`.
///
/// `pub(crate)` so a sibling suite of this one test target stands a cluster and its members
/// up the same way rather than growing a second copy of these calls.
pub(crate) async fn create_available_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) {
    expect_ok(
        &format!("create {identifier}"),
        create_cluster(router, identifier, engine, extra).await,
    );
    wait_for_settle();
}

/// `CreateDBInstance` for an instance that joins `cluster`.
///
/// `pub(crate)` for the same reason [`create_available_cluster`] is.
pub(crate) async fn create_member(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    cluster: &str,
    zone: &str,
) {
    expect_ok(
        &format!("create member {identifier}"),
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", engine),
                ("DBInstanceClass", "db.serverless"),
                ("DBClusterIdentifier", cluster),
                ("AvailabilityZone", zone),
            ],
        )
        .await,
    );
}

/// "DBCluster &lt;named&gt; not found." — the wording the calls that REPORT a cluster, or
/// stop it, answer the fault with.
fn reported_not_found(named: &str) -> String {
    format!("DBCluster {named} not found.")
}

/// The wording the call that MODIFIES the cluster's configuration answers the same fault
/// with: ONE fixed sentence, so the identifier the request named does not appear in it at
/// all and every identifier gets the same text.
fn modified_not_found(_named: &str) -> String {
    "DBCluster not found".to_string()
}

/// The wording the calls that act on the cluster ITSELF — its delete, and the failover
/// that moves its primary — answer the same fault with.
fn subject_not_found(named: &str) -> String {
    format!("The source cluster could not be found or cannot be accessed: {named}")
}

/// The wording the call that takes a SNAPSHOT of the cluster answers it with.
fn snapshot_source_not_found(named: &str) -> String {
    format!("DBCluster not found: {named}")
}

/// One operation that names a DB cluster: its `Action`, the other members the model
/// requires of it, and the message its own `DBClusterNotFoundFault` reads.
struct ClusterOperation {
    action: &'static str,
    extra: &'static [(&'static str, &'static str)],
    wording: fn(&str) -> String,
}

/// Every implemented operation whose `DBClusterIdentifier` names the cluster the call is
/// about, with the wording each answers a cluster this service does not hold with.
const CLUSTER_OPERATIONS: &[ClusterOperation] = &[
    ClusterOperation {
        action: "DescribeDBClusters",
        extra: &[],
        wording: reported_not_found,
    },
    ClusterOperation {
        action: "ModifyDBCluster",
        extra: &[],
        wording: modified_not_found,
    },
    ClusterOperation {
        action: "StopDBCluster",
        extra: &[],
        wording: reported_not_found,
    },
    ClusterOperation {
        action: "DeleteDBCluster",
        extra: &[("SkipFinalSnapshot", "true")],
        wording: subject_not_found,
    },
    ClusterOperation {
        action: "FailoverDBCluster",
        extra: &[],
        wording: subject_not_found,
    },
    ClusterOperation {
        action: "CreateDBClusterSnapshot",
        extra: &[("DBClusterSnapshotIdentifier", "emu-absent-cluster-snapshot")],
        wording: snapshot_source_not_found,
    },
];

#[tokio::test]
async fn every_operation_that_names_a_db_cluster_words_its_own_not_found_fault() {
    let router = emulator();
    create_available_cluster(&router, PRESENT, "aurora-postgresql", &[]).await;

    // The three forms a member that names a DB cluster accepts, all naming nothing.
    let forms: [(&str, String); 3] = [
        ("the identifier as written", ABSENT.to_string()),
        (
            "the same identifier upper-cased",
            ABSENT.to_ascii_uppercase(),
        ),
        ("the cluster's ARN form", cluster_arn(ABSENT)),
    ];

    for row in CLUSTER_OPERATIONS {
        for (form, named) in &forms {
            let case = format!("{} with {form}", row.action);
            let mut params: Vec<(&str, String)> =
                vec![("DBClusterIdentifier", named.to_string())];
            params.extend(
                row.extra
                    .iter()
                    .map(|(name, value)| (*name, (*value).to_string())),
            );
            let body = expect_error(
                &case,
                call(&router, row.action, &params).await,
                404,
                "DBClusterNotFoundFault",
            );
            let message = text(&body, "Message");
            let expected = (row.wording)(named);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            let kind = text(&body, "Type");
            assert_eq!(
                kind, "Sender",
                "case {case:?}: expected a Sender fault, got {kind:?} in {body}"
            );
        }
    }

    // The cluster that IS stored was reported throughout, so none of the refusals above
    // came from an empty store — and none of them changed it.
    let present = describe_one(&router, "the stored cluster", PRESENT).await;
    let status = text(&present, "Status");
    assert_eq!(
        status, "available",
        "expected the stored cluster untouched by the refusals, got {status:?}"
    );
}

/// One row of the tag-refusal table: the family, a `ResourceName` of it that names nothing
/// stored, the family's wire error `<Code>` and the noun phrase its message reads with.
type TagFamily = (&'static str, String, &'static str, &'static str);

#[tokio::test]
async fn a_tag_operation_names_the_resource_name_it_could_not_resolve() {
    let router = emulator();
    create_available_cluster(&router, PRESENT, "aurora-postgresql", &[]).await;

    // Several families, because the rule is about how a tag operation addresses its
    // resource — by `ResourceName`, "an Amazon Resource Name (ARN)" — and not about one
    // resource type. An ARN whose identifier is upper-cased names nothing either: the
    // resolver compares the ARN each stored record RENDERS.
    let families: [TagFamily; 4] = [
        (
            "a DB cluster",
            cluster_arn(ABSENT),
            "DBClusterNotFoundFault",
            "a DB Cluster",
        ),
        (
            "a DB cluster named by an upper-cased ARN",
            cluster_arn(&PRESENT.to_ascii_uppercase()),
            "DBClusterNotFoundFault",
            "a DB Cluster",
        ),
        (
            "a DB instance",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:emu-absent-instance"),
            "DBInstanceNotFound",
            "a DB Instance",
        ),
        (
            "an option group",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:og:emu-absent-option-group"),
            "OptionGroupNotFoundFault",
            "an Option Group",
        ),
    ];

    for (family, resource_name, code, phrase) in &families {
        for (action, extra) in TAG_ACTIONS {
            let case = format!("{action} on {family}");
            let mut params: Vec<(&str, String)> =
                vec![("ResourceName", resource_name.to_string())];
            params.extend(
                extra
                    .iter()
                    .map(|(name, value)| (*name, (*value).to_string())),
            );
            let body = expect_error(&case, call(&router, action, &params).await, 404, code);
            let message = text(&body, "Message");
            let expected =
                format!("Could not find {phrase} matching the resource name: {resource_name}");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }
}

/// One row of the snapshot table: the engine the source cluster runs and the identifiers
/// the calls use.
struct SnapshotCase {
    case: &'static str,
    cluster: &'static str,
    engine: &'static str,
    /// The `DBClusterSnapshotIdentifier` the request names, which is "stored as a
    /// lowercase string".
    snapshot: &'static str,
    /// Whether the source cluster has a DB INSTANCE in it when the snapshot is taken: the
    /// storage a snapshot holds and the port it was taken on are facts about the cluster's
    /// instances, so a cluster with none reports zero for both.
    with_instance: bool,
}

/// The instance class a cluster member is created with here — one the orderable-options
/// report offers for every engine this front door serves a cluster for.
const CLUSTER_MEMBER_CLASS: &str = "db.r5.large";

#[tokio::test]
async fn a_snapshot_of_a_stored_cluster_is_read_back_through_the_describe() {
    let router = emulator();

    // The engines this front door serves, which is what the snapshot's own `Engine`,
    // `Port` and `EngineVersion` are read off, plus both spellings of the identifier the
    // snapshot is stored under, and both sides of the rule the snapshot's storage and port
    // follow: a source cluster that has a DB instance in it, and one that has none.
    let table: &[SnapshotCase] = &[
        SnapshotCase {
            case: "an Aurora PostgreSQL cluster with a member instance",
            cluster: "emu-snap-aurora-pg",
            engine: "aurora-postgresql",
            snapshot: "emu-snap-aurora-pg-1",
            with_instance: true,
        },
        SnapshotCase {
            case: "an Aurora MySQL cluster, snapshot named in upper case",
            cluster: "emu-snap-aurora-mysql",
            engine: "aurora-mysql",
            snapshot: "EMU-SNAP-AURORA-MYSQL-1",
            with_instance: false,
        },
        SnapshotCase {
            case: "a DocumentDB cluster",
            cluster: "emu-snap-docdb",
            engine: "docdb",
            snapshot: "emu-snap-docdb-1",
            with_instance: false,
        },
        SnapshotCase {
            case: "a Neptune cluster with a member instance",
            cluster: "emu-snap-neptune",
            engine: "neptune",
            snapshot: "emu-snap-neptune-1",
            with_instance: true,
        },
    ];

    for row in table {
        create_available_cluster(&router, row.cluster, row.engine, &[]).await;
        if row.with_instance {
            let member = format!("{}-m0", row.cluster);
            expect_ok(
                row.case,
                call(
                    &router,
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", member.as_str()),
                        ("Engine", row.engine),
                        ("DBInstanceClass", CLUSTER_MEMBER_CLASS),
                        ("DBClusterIdentifier", row.cluster),
                    ],
                )
                .await,
            );
        }
        // The snapshot holds the CLUSTER's settings as they stand now, so what it must
        // report is read off the cluster itself rather than restated here.
        let source = describe_one(&router, row.case, row.cluster).await;
        let taken = expect_ok(
            row.case,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", row.cluster),
                    ("DBClusterSnapshotIdentifier", row.snapshot),
                ],
            )
            .await,
        );
        // A cluster with no DB instance holds no data and nothing was listening on its
        // port, so its snapshot reports zero for both; one with an instance reports the
        // cluster's own.
        let zero_without_instances = |member: &str| {
            if row.with_instance {
                text(&source, member)
            } else {
                "0".to_string()
            }
        };
        let folded = row.snapshot.to_ascii_lowercase();
        for (member, expected) in [
            ("DBClusterSnapshotIdentifier", folded.clone()),
            ("DBClusterIdentifier", row.cluster.to_string()),
            ("Engine", row.engine.to_string()),
            ("Port", zero_without_instances("Port")),
            // The snapshot's own size is not known while it is still being made, so it
            // reports the storage of the cluster it is being taken from.
            ("AllocatedStorage", text(&source, "AllocatedStorage")),
            ("EngineVersion", text(&source, "EngineVersion")),
            ("DbClusterResourceId", text(&source, "DbClusterResourceId")),
            ("SnapshotType", "manual".to_string()),
            // The snapshot is not restorable the instant the call returns: it reports the
            // transient status and the clock owes it `available`.
            ("Status", "creating".to_string()),
            ("PercentProgress", "0".to_string()),
            (
                "DBClusterSnapshotArn",
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:{folded}"),
            ),
        ] {
            let reported = text(&taken, member);
            assert_eq!(
                reported, expected,
                "case {:?}: expected {member} {expected:?}, got {reported:?} in {taken}",
                row.case
            );
        }

        // A second snapshot under the same identifier would overwrite the first.
        expect_error(
            &format!("{}: taken twice", row.case),
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", row.cluster),
                    ("DBClusterSnapshotIdentifier", row.snapshot),
                ],
            )
            .await,
            400,
            "DBClusterSnapshotAlreadyExistsFault",
        );

        // The snapshot is read back through the describe — by its own identifier, and as
        // one of the snapshots of the cluster it was taken from — and the clock has made
        // it available by then.
        wait_for_settle();
        let read_back = expect_ok(
            &format!("{}: read back", row.case),
            call(
                &router,
                "DescribeDBClusterSnapshots",
                &[("DBClusterSnapshotIdentifier", &folded)],
            )
            .await,
        );
        for (member, expected) in [("Status", "available"), ("PercentProgress", "100")] {
            let reported = text(&read_back, member);
            assert_eq!(
                reported, expected,
                "case {:?}: expected the settled snapshot's {member} {expected:?}, got \
                 {reported:?}",
                row.case
            );
        }
        let of_the_cluster = expect_ok(
            &format!("{}: the cluster's snapshots", row.case),
            call(
                &router,
                "DescribeDBClusterSnapshots",
                &[("DBClusterIdentifier", row.cluster)],
            )
            .await,
        );
        let reported = texts(&of_the_cluster, "DBClusterSnapshotIdentifier");
        assert_eq!(
            reported,
            vec![folded.clone()],
            "case {:?}: expected the cluster to report the snapshot taken of it, got \
             {reported:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_snapshot_carries_the_clusters_tags_only_when_the_cluster_asks_for_it() {
    let router = emulator();

    // `CopyTagsToSnapshot` — "indicates whether tags from the DB cluster are copied to
    // snapshots of the DB cluster" — is the whole rule, so both of its values are rows.
    for (case, cluster, copy_tags, expected) in [
        (
            "a cluster that copies its tags",
            "emu-snap-tags-copied",
            "true",
            vec![
                ("owner".to_string(), "platform".to_string()),
                ("taken-by".to_string(), "the-test".to_string()),
            ],
        ),
        (
            "a cluster that does not",
            "emu-snap-tags-kept",
            "false",
            vec![("taken-by".to_string(), "the-test".to_string())],
        ),
    ] {
        create_available_cluster(
            &router,
            cluster,
            "aurora-postgresql",
            &[
                ("CopyTagsToSnapshot", copy_tags),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "platform"),
            ],
        )
        .await;
        let taken = expect_ok(
            case,
            call(
                &router,
                "CreateDBClusterSnapshot",
                &[
                    ("DBClusterIdentifier", cluster),
                    ("DBClusterSnapshotIdentifier", &format!("{cluster}-1")),
                    ("Tags.Tag.1.Key", "taken-by"),
                    ("Tags.Tag.1.Value", "the-test"),
                ],
            )
            .await,
        );
        let reported: Vec<(String, String)> = subtrees(&taken, "Tag")
            .iter()
            .map(|tag| (text(tag, "Key"), text(tag, "Value")))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the snapshot to carry {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_stop_moves_the_cluster_and_its_members_through_the_transient_status() {
    let router = emulator();

    // A cluster the clock has not settled yet is not running, so there is nothing to stop:
    // the modelled state fault, reported with the status that refused it.
    expect_ok(
        "create the cluster",
        create_cluster(&router, "emu-stop-cluster", "aurora-mysql", &[]).await,
    );
    let refused = expect_error(
        "stop a cluster that is still creating",
        call(
            &router,
            "StopDBCluster",
            &[("DBClusterIdentifier", "emu-stop-cluster")],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&refused, "Message");
    assert!(
        message.contains("creating"),
        "the refusal must name the status that refused the stop, got {message:?}"
    );

    wait_for_settle();
    create_member(
        &router,
        "emu-stop-member",
        "aurora-mysql",
        "emu-stop-cluster",
        &format!("{REGION}a"),
    )
    .await;
    wait_for_settle();

    // The stop is accepted with the cluster in the transient status, which is also what a
    // read inside the span it owes reports.
    let stopped = expect_ok(
        "stop the cluster",
        call(
            &router,
            "StopDBCluster",
            &[("DBClusterIdentifier", "EMU-Stop-Cluster")],
        )
        .await,
    );
    let status = text(&stopped, "Status");
    assert_eq!(
        status, "stopping",
        "expected the accepted stop to report the transient status, got {status:?} in \
         {stopped}"
    );
    let reading = describe_one(&router, "inside the span the stop owes", "emu-stop-cluster").await;
    let status = text(&reading, "Status");
    assert_eq!(
        status, "stopping",
        "expected the read inside the span to report the transient status, got {status:?}"
    );
    // A cluster that is stopping is not in a state another stop, a failover or a snapshot
    // can be taken in.
    for (case, action, extra) in [
        ("another stop", "StopDBCluster", Vec::new()),
        ("a failover", "FailoverDBCluster", Vec::new()),
        (
            "a snapshot",
            "CreateDBClusterSnapshot",
            vec![("DBClusterSnapshotIdentifier", "emu-stop-cluster-1")],
        ),
    ] {
        let mut params: Vec<(&str, &str)> = vec![("DBClusterIdentifier", "emu-stop-cluster")];
        params.extend(extra);
        expect_error(
            case,
            call(&router, action, &params).await,
            400,
            "InvalidDBClusterStateFault",
        );
    }

    wait_for_settle();
    let settled = describe_one(&router, "after the stop settled", "emu-stop-cluster").await;
    let status = text(&settled, "Status");
    assert_eq!(
        status, "stopped",
        "expected the cluster to reach stopped on its own, got {status:?}"
    );
    // "When you stop a DB cluster, its DB instances are also stopped": the member reports
    // the stop too, rather than claiming a database a client could still connect to.
    let member = expect_ok(
        "the member instance",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "emu-stop-member")],
        )
        .await,
    );
    let status = text(&member, "DBInstanceStatus");
    assert_eq!(
        status, "stopped",
        "expected the cluster's member to be stopped with it, got {status:?}"
    );
}

/// The `(identifier, is the cluster writer)` pairs a cluster reports, in the order it
/// reports them.
///
/// `pub(crate)` for the same reason [`create_available_cluster`] is.
pub(crate) async fn reported_members(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) -> Vec<(String, String)> {
    let reported = describe_one(router, case, cluster).await;
    subtrees(&reported, "DBClusterMember")
        .iter()
        .map(|member| {
            (
                text(member, "DBInstanceIdentifier"),
                text(member, "IsClusterWriter"),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_failover_moves_which_member_is_the_cluster_writer() {
    let router = emulator();
    create_available_cluster(&router, "emu-failover-cluster", "aurora-postgresql", &[]).await;
    // The writer is the instance created first, and every later one is a replica.
    create_member(
        &router,
        "emu-failover-writer",
        "aurora-postgresql",
        "emu-failover-cluster",
        &format!("{REGION}a"),
    )
    .await;
    create_member(
        &router,
        "emu-failover-reader",
        "aurora-postgresql",
        "emu-failover-cluster",
        &format!("{REGION}b"),
    )
    .await;
    wait_for_settle();
    let members = reported_members(&router, "before the failover", "emu-failover-cluster").await;
    assert_eq!(
        members,
        vec![
            ("emu-failover-writer".to_string(), "true".to_string()),
            ("emu-failover-reader".to_string(), "false".to_string()),
        ],
        "expected the first instance created in the cluster to be its writer, got {members:?}"
    );

    // A target that is no member of this cluster cannot be promoted to its primary.
    expect_error(
        "a target that is not a member",
        call(
            &router,
            "FailoverDBCluster",
            &[
                ("DBClusterIdentifier", "emu-failover-cluster"),
                ("TargetDBInstanceIdentifier", "emu-failover-stranger"),
            ],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );

    // Each row is one failover and the member it must leave as the writer: with no target
    // the cluster's replica is promoted, and with one the named member is.
    for (case, target, writer, reader) in [
        (
            "no target names the replica",
            None,
            "emu-failover-reader",
            "emu-failover-writer",
        ),
        (
            "a named target is promoted",
            Some("emu-failover-writer"),
            "emu-failover-writer",
            "emu-failover-reader",
        ),
        (
            "the target may be named by its ARN form",
            Some("arn:aws:rds:us-west-2:000000000000:db:emu-failover-reader"),
            "emu-failover-reader",
            "emu-failover-writer",
        ),
    ] {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", "emu-failover-cluster")];
        if let Some(target) = target {
            params.push(("TargetDBInstanceIdentifier", target));
        }
        let answered = expect_ok(case, call(&router, "FailoverDBCluster", &params).await);
        let status = text(&answered, "Status");
        assert_eq!(
            status, "failing-over",
            "case {case:?}: expected the accepted failover to report the transient status, \
             got {status:?}"
        );
        let members = reported_members(&router, case, "emu-failover-cluster").await;
        assert_eq!(
            members,
            vec![
                (writer.to_string(), "true".to_string()),
                (reader.to_string(), "false".to_string()),
            ],
            "case {case:?}: expected the promoted member to be the cluster writer, got \
             {members:?}"
        );
        wait_for_settle();
        let settled = describe_one(&router, case, "emu-failover-cluster").await;
        let status = text(&settled, "Status");
        assert_eq!(
            status, "available",
            "case {case:?}: expected the cluster to reach available on its own, got \
             {status:?}"
        );
        let members = reported_members(&router, case, "emu-failover-cluster").await;
        assert_eq!(
            members.first().map(|(member, _)| member.as_str()),
            Some(writer),
            "case {case:?}: expected the promoted member to stay the writer once the \
             failover settled, got {members:?}"
        );
    }

    // A cluster holding no instance has no primary to move.
    create_available_cluster(&router, "emu-failover-empty", "aurora-postgresql", &[]).await;
    let refused = expect_error(
        "a cluster with no members",
        call(
            &router,
            "FailoverDBCluster",
            &[("DBClusterIdentifier", "emu-failover-empty")],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&refused, "Message");
    assert!(
        message.contains("emu-failover-empty"),
        "the refusal must name the cluster it could not fail over, got {message:?}"
    );
}

/// One row of the empty-page table: the case, the `Action`, the selector the request
/// carries, and the element the report would hold a match in.
type EmptyPageRow<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>, &'a str);

#[tokio::test]
async fn a_report_narrowed_to_a_cluster_that_does_not_exist_is_an_empty_page() {
    let router = emulator();
    create_available_cluster(&router, PRESENT, "aurora-postgresql", &[]).await;
    expect_ok(
        "a snapshot of the stored cluster",
        call(
            &router,
            "CreateDBClusterSnapshot",
            &[
                ("DBClusterIdentifier", PRESENT),
                ("DBClusterSnapshotIdentifier", "emu-present-cluster-1"),
            ],
        )
        .await,
    );

    // Neither selector models a cluster-not-found error: a value naming no cluster
    // matches no record, which is an EMPTY page and not a refusal — the one answer that
    // lets a caller tell "nothing matches this filter" from "this call cannot be served".
    let absent_arn = cluster_arn(ABSENT);
    let reports: &[EmptyPageRow<'_>] = &[
        (
            "cluster snapshots of a cluster that does not exist",
            "DescribeDBClusterSnapshots",
            vec![("DBClusterIdentifier", ABSENT)],
            "DBClusterSnapshot",
        ),
        (
            "cluster snapshots selected by a filter naming it",
            "DescribeDBClusterSnapshots",
            vec![
                ("Filters.Filter.1.Name", "db-cluster-id"),
                ("Filters.Filter.1.Values.Value.1", ABSENT),
            ],
            "DBClusterSnapshot",
        ),
        (
            "pending maintenance actions of a resource that does not exist",
            "DescribePendingMaintenanceActions",
            vec![("ResourceIdentifier", absent_arn.as_str())],
            "ResourcePendingMaintenanceActions",
        ),
        (
            "pending maintenance actions selected by a filter naming it",
            "DescribePendingMaintenanceActions",
            vec![
                ("Filters.Filter.1.Name", "db-cluster-id"),
                ("Filters.Filter.1.Values.Value.1", ABSENT),
            ],
            "ResourcePendingMaintenanceActions",
        ),
        (
            // Nothing in this service schedules a maintenance action, so even the cluster
            // that IS stored has none pending: the report holds only the resources that
            // have at least one, which makes the whole region's report empty too.
            "pending maintenance actions over the whole region",
            "DescribePendingMaintenanceActions",
            Vec::new(),
            "ResourcePendingMaintenanceActions",
        ),
    ];

    for (case, action, params, element) in reports {
        let body = expect_ok(case, call(&router, action, params).await);
        let reported = subtrees(&body, element);
        assert!(
            reported.is_empty(),
            "case {case:?}: expected an empty page, got {reported:?} in {body}"
        );
    }

    // The snapshot the stored cluster DOES have is still reported, so the empty pages
    // above are decisions about the selector and not an empty store.
    let held = expect_ok(
        "the stored cluster's snapshots",
        call(
            &router,
            "DescribeDBClusterSnapshots",
            &[("DBClusterIdentifier", PRESENT)],
        )
        .await,
    );
    let reported = texts(&held, "DBClusterSnapshotIdentifier");
    assert_eq!(
        reported,
        vec!["emu-present-cluster-1".to_string()],
        "expected the stored cluster's own snapshot to be reported, got {reported:?}"
    );
}
