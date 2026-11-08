// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RemoveFromGlobalCluster`: detaching a member DB cluster from an
//! Aurora global database.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (through `db_instances::emulator`, the one mounting
//! helper this test target has), and each request is a real awsQuery form POST carrying the
//! SigV4 credential scope the router keys on. The assertions read the XML the emulator
//! answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test:
//!
//! * a detach writes BOTH sides of the membership and every later call sees them: the
//!   global database stops reporting the member, and the member stops reporting the global
//!   database and the data it received from it — "the cluster becomes a standalone cluster
//!   with read-write capability instead of being read-only and receiving data from a
//!   primary cluster in a different Region" — so a global database this call emptied is
//!   deletable and the detached cluster can be adopted by another global database;
//! * a detach is in force when it answers: nothing is provisioned for it, so the global
//!   database stays ready and the next call against it is accepted;
//! * a detach this service refuses persists nothing: a global database nothing created is
//!   the modelled 404, a DB cluster nothing created is the modelled
//!   `DBClusterNotFoundFault`, a cluster that is not a member of the global database named
//!   is the modelled `InvalidDBClusterStateFault`, and a global database whose own change
//!   is still being applied — or whose primary is asked for while secondaries are still
//!   attached — is the modelled `InvalidGlobalClusterStateFault`.
//!
//! The tables below span what those rules generalise over: both engines
//! `CreateGlobalClusterMessage$Engine` publishes, a global database created EMPTY and
//! joined by its cluster's own create beside one created AROUND an existing cluster, a
//! membership of one cluster and one of two, a member in the region the request is signed
//! for and a secondary in ANOTHER region, and each form `DbClusterIdentifier` takes — the
//! documented ARN, the bare identifier, and the identifier in another case.

use service_com_amazonaws_rds_amazonrdsv19::state::GLOBAL_CLUSTER_STATUS_AVAILABLE;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, text, texts, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, so a secondary cluster of ANOTHER region is detachable — which is the
/// case the documented ARN form of `DbClusterIdentifier` exists for.
const OTHER_REGION: &str = "eu-west-1";

/// A global cluster's ARN, which carries NO region: it is an account-scoped resource.
fn global_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds::{ACCOUNT}:global-cluster:{identifier}")
}

/// A DB cluster's ARN, which carries the region the cluster lives in.
fn db_cluster_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:cluster:{identifier}")
}

/// How a row SPELLS the member it detaches. `DbClusterIdentifier` is documented as "the
/// Amazon Resource Name (ARN) identifying the cluster", and every cluster-naming member of
/// this service accepts "the user-supplied DB cluster identifier or the Amazon Resource
/// Name (ARN)", case-insensitively — so all three forms name one and the same cluster.
#[derive(Debug, Clone, Copy)]
enum NameForm {
    Arn,
    Identifier,
    MixedCaseIdentifier,
}

impl NameForm {
    /// What the detach sends for the cluster `identifier` of `region`.
    fn spell(self, region: &str, identifier: &str) -> String {
        match self {
            NameForm::Arn => db_cluster_arn(region, identifier),
            NameForm::Identifier => identifier.to_string(),
            NameForm::MixedCaseIdentifier => identifier.to_ascii_uppercase(),
        }
    }
}

/// `CreateGlobalCluster`, optionally around the DB cluster it adopts as its primary.
async fn create_global(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    version: &str,
    source: Option<&str>,
) -> String {
    let mut params: Vec<(&str, String)> =
        vec![("GlobalClusterIdentifier", identifier.to_string())];
    match source {
        // "Amazon Aurora uses the values from the specified source DB cluster", so a
        // create that adopts one carries neither engine nor version.
        Some(source) => params.push(("SourceDBClusterIdentifier", source.to_string())),
        None => {
            params.push(("Engine", engine.to_string()));
            params.push(("EngineVersion", version.to_string()));
        }
    }
    expect_ok(case, call(router, "CreateGlobalCluster", &params).await)
}

/// `CreateDBCluster` in `region`, joining `global` when one is named.
async fn create_cluster(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    identifier: &str,
    engine: &str,
    version: &str,
    global: Option<&str>,
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("EngineVersion", version.to_string()),
    ];
    if let Some(global) = global {
        params.push(("GlobalClusterIdentifier", global.to_string()));
    }
    expect_ok(case, call_in(router, region, "CreateDBCluster", &params).await)
}

/// `RemoveFromGlobalCluster` as a row spells the member it detaches.
async fn detach(
    router: &MultiServiceRouter,
    global: &str,
    db_cluster: &str,
) -> (u16, String) {
    call(
        router,
        "RemoveFromGlobalCluster",
        &[
            ("GlobalClusterIdentifier", global),
            ("DbClusterIdentifier", db_cluster),
        ],
    )
    .await
}

/// The member ARNs `DescribeGlobalClusters` reports for `identifier`, in the order the
/// global database holds them — the order that decides which member is the writer.
async fn described_members(router: &MultiServiceRouter, identifier: &str) -> Vec<String> {
    let body = expect_ok(
        "read the global database back",
        call(
            router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", identifier)],
        )
        .await,
    );
    texts(&body, "DBClusterArn")
}

/// The `GlobalClusterIdentifier` `DescribeDBClusters` reports for a cluster of `region`, or
/// `None` when the cluster belongs to no global database.
async fn cluster_global(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> Option<String> {
    let body = expect_ok(
        "read the member cluster back",
        call_in(
            router,
            region,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", identifier)],
        )
        .await,
    );
    optional_text(&body, "GlobalClusterIdentifier")
}

/// One row of the detach round trip.
struct DetachCase {
    case: &'static str,
    engine: &'static str,
    /// A version of the published engine-version catalogue.
    version: &'static str,
    /// The `GlobalClusterIdentifier` the create names.
    global: &'static str,
    /// What that identifier is stored as: "this parameter is stored as a lowercase string".
    stored_global: &'static str,
    /// How the detach SPELLS the global database: "this parameter isn't case-sensitive".
    global_named_as: &'static str,
    /// The primary cluster's identifier.
    primary: &'static str,
    /// Whether the global database is created AROUND the primary
    /// (`SourceDBClusterIdentifier`) rather than created empty and joined by the cluster's
    /// own `CreateDBCluster$GlobalClusterIdentifier`.
    adopts_primary: bool,
    /// The secondary cluster this row attaches, if any: its identifier and its region.
    secondary: Option<(&'static str, &'static str)>,
    /// How the detach names each member it removes.
    named_as: NameForm,
}

/// The memberships this suite drives.
///
/// The rows differ along the engine, how the global database came to hold its primary,
/// how many members it holds and in which regions, and the form the detach names a member
/// by.
const DETACH_CASES: &[DetachCase] = &[
    DetachCase {
        case: "aurora-mysql, a global database joined by its own primary, named by ARN",
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.10.3",
        global: "emu-rfgc-mysql",
        stored_global: "emu-rfgc-mysql",
        global_named_as: "emu-rfgc-mysql",
        primary: "emu-rfgc-mysql-primary",
        adopts_primary: false,
        secondary: None,
        named_as: NameForm::Arn,
    },
    DetachCase {
        case: "aurora-postgresql, created around its primary, named by bare identifier",
        engine: "aurora-postgresql",
        version: "15.8",
        global: "EMU-RFGC-PG",
        stored_global: "emu-rfgc-pg",
        global_named_as: "emu-rfgc-pg",
        primary: "emu-rfgc-pg-primary",
        adopts_primary: true,
        secondary: None,
        named_as: NameForm::Identifier,
    },
    DetachCase {
        case: "aurora-mysql, a secondary in the same region, named in another case",
        engine: "aurora-mysql",
        version: "8.0.mysql_aurora.3.08.0",
        global: "emu-rfgc-pair",
        stored_global: "emu-rfgc-pair",
        global_named_as: "EMU-RFGC-Pair",
        primary: "emu-rfgc-pair-primary",
        adopts_primary: false,
        secondary: Some(("emu-rfgc-pair-secondary", REGION)),
        named_as: NameForm::MixedCaseIdentifier,
    },
    DetachCase {
        case: "aurora-postgresql, a secondary in ANOTHER region, named by ARN",
        engine: "aurora-postgresql",
        version: "16.6",
        global: "emu-rfgc-cross",
        stored_global: "emu-rfgc-cross",
        global_named_as: "emu-rfgc-cross",
        primary: "emu-rfgc-cross-primary",
        adopts_primary: true,
        secondary: Some(("emu-rfgc-cross-secondary", OTHER_REGION)),
        named_as: NameForm::Arn,
    },
];

#[tokio::test]
async fn a_detach_writes_both_sides_of_the_membership_and_every_later_call_sees_it() {
    for row in DETACH_CASES {
        let router = emulator();
        // The global database and its primary, built the way the row asks for.
        if row.adopts_primary {
            create_cluster(
                &router,
                row.case,
                REGION,
                row.primary,
                row.engine,
                row.version,
                None,
            )
            .await;
            create_global(
                &router,
                row.case,
                row.global,
                row.engine,
                row.version,
                Some(row.primary),
            )
            .await;
        } else {
            create_global(&router, row.case, row.global, row.engine, row.version, None).await;
            create_cluster(
                &router,
                row.case,
                REGION,
                row.primary,
                row.engine,
                row.version,
                Some(row.global),
            )
            .await;
        }
        let primary_arn = db_cluster_arn(REGION, row.primary);
        let mut expected_members = vec![primary_arn.clone()];
        if let Some((secondary, secondary_region)) = row.secondary {
            create_cluster(
                &router,
                row.case,
                secondary_region,
                secondary,
                row.engine,
                row.version,
                Some(row.global),
            )
            .await;
            expected_members.push(db_cluster_arn(secondary_region, secondary));
        }
        let actual = described_members(&router, row.stored_global).await;
        assert_eq!(
            actual, expected_members,
            "case {:?}: expected the global database to hold {expected_members:?}, got \
             {actual:?}",
            row.case
        );

        // A row with a secondary: the PRIMARY cannot be detached while the secondary is
        // still attached — it feeds it — and that refusal changes nothing.
        if let Some((secondary, secondary_region)) = row.secondary {
            let refused = expect_error(
                row.case,
                detach(
                    &router,
                    row.global_named_as,
                    &row.named_as.spell(REGION, row.primary),
                )
                .await,
                400,
                "InvalidGlobalClusterStateFault",
            );
            // The refusal names the cluster, the global database and how many secondaries
            // are in the way — every part taken from the request and the stored records.
            let expected_message = format!(
                "DB cluster {} is the primary cluster of global cluster {} and can't be \
                 detached while it still has 1 secondary cluster(s)",
                row.primary, row.stored_global
            );
            let actual_message = text(&refused, "Message");
            assert_eq!(
                actual_message, expected_message,
                "case {:?}: expected the refusal {expected_message:?}, got {actual_message:?}",
                row.case
            );
            let actual = described_members(&router, row.stored_global).await;
            assert_eq!(
                actual, expected_members,
                "case {:?}: a refused detach must leave the membership as it was, got \
                 {actual:?}",
                row.case
            );

            // The secondary detaches, and both records agree about it afterwards.
            let detached = expect_ok(
                row.case,
                detach(
                    &router,
                    row.global_named_as,
                    &row.named_as.spell(secondary_region, secondary),
                )
                .await,
            );
            let answered = texts(&detached, "DBClusterArn");
            assert_eq!(
                answered,
                vec![primary_arn.clone()],
                "case {:?}: the answer must report the global database without the member it \
                 detached, got {detached}",
                row.case
            );
            let actual = described_members(&router, row.stored_global).await;
            assert_eq!(
                actual,
                vec![primary_arn.clone()],
                "case {:?}: the detach must persist, got {actual:?}",
                row.case
            );
            let joined = cluster_global(&router, secondary_region, secondary).await;
            assert_eq!(
                joined, None,
                "case {:?}: the detached secondary must belong to no global database, got \
                 {joined:?}",
                row.case
            );
            // A member that is no longer one cannot be detached twice.
            expect_error(
                row.case,
                detach(
                    &router,
                    row.global_named_as,
                    &row.named_as.spell(secondary_region, secondary),
                )
                .await,
                400,
                "InvalidDBClusterStateFault",
            );
        }

        // The last member — the primary — detaches too, and the global database is left
        // empty, ready, and reporting the identity it has always had.
        let detached = expect_ok(
            row.case,
            detach(
                &router,
                row.global_named_as,
                &row.named_as.spell(REGION, row.primary),
            )
            .await,
        );
        assert!(
            texts(&detached, "DBClusterArn").is_empty(),
            "case {:?}: an emptied global database must report no member, got {detached}",
            row.case
        );
        for (member, expected) in [
            ("GlobalClusterIdentifier", row.stored_global.to_string()),
            ("GlobalClusterArn", global_cluster_arn(row.stored_global)),
            ("Status", GLOBAL_CLUSTER_STATUS_AVAILABLE.to_string()),
            ("Engine", row.engine.to_string()),
            ("EngineVersion", row.version.to_string()),
        ] {
            let actual = text(&detached, member);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {member} {expected:?}, got {actual:?}",
                row.case
            );
        }
        let actual = described_members(&router, row.stored_global).await;
        assert!(
            actual.is_empty(),
            "case {:?}: the emptied membership must persist, got {actual:?}",
            row.case
        );
        let joined = cluster_global(&router, REGION, row.primary).await;
        assert_eq!(
            joined, None,
            "case {:?}: the detached primary must belong to no global database, got {joined:?}",
            row.case
        );

        // What the detach makes possible: a global database that holds no member is
        // deletable, and a cluster that belongs to none can be adopted by another one.
        expect_ok(
            row.case,
            call(
                &router,
                "DeleteGlobalCluster",
                &[("GlobalClusterIdentifier", row.global_named_as)],
            )
            .await,
        );
        let readopted = create_global(
            &router,
            row.case,
            "emu-rfgc-readopted",
            row.engine,
            row.version,
            Some(row.primary),
        )
        .await;
        assert_eq!(
            texts(&readopted, "DBClusterArn"),
            vec![primary_arn.clone()],
            "case {:?}: the detached cluster must be adoptable by another global database, \
             got {readopted}",
            row.case
        );
    }
}

/// One row of the refusal table: the case, the global database the detach names, the member
/// it names, the HTTP status and wire error code it is refused with, and the fault's whole
/// sentence — whose variable parts are the value the request carried and the stored records.
type RefusalRow = (
    &'static str,
    &'static str,
    &'static str,
    u16,
    &'static str,
    &'static str,
);

#[tokio::test]
async fn a_detach_this_service_refuses_persists_nothing() {
    let router = emulator();
    create_global(
        &router,
        "the global database under test",
        "emu-rfgc-refused",
        "aurora-mysql",
        "8.0.mysql_aurora.3.10.3",
        None,
    )
    .await;
    create_cluster(
        &router,
        "its member",
        REGION,
        "emu-rfgc-refused-member",
        "aurora-mysql",
        "8.0.mysql_aurora.3.10.3",
        Some("emu-rfgc-refused"),
    )
    .await;
    // A cluster that belongs to no global database at all, and one that belongs to ANOTHER:
    // neither is a member of the global database the refusals below name.
    create_cluster(
        &router,
        "a standalone cluster",
        REGION,
        "emu-rfgc-standalone",
        "aurora-mysql",
        "8.0.mysql_aurora.3.10.3",
        None,
    )
    .await;
    create_global(
        &router,
        "another global database",
        "emu-rfgc-other",
        "aurora-mysql",
        "8.0.mysql_aurora.3.10.3",
        None,
    )
    .await;
    create_cluster(
        &router,
        "the other global database's member",
        REGION,
        "emu-rfgc-other-member",
        "aurora-mysql",
        "8.0.mysql_aurora.3.10.3",
        Some("emu-rfgc-other"),
    )
    .await;

    // (case, the global database named, the member named, status, code, the fault's sentence)
    let refusals: &[RefusalRow] = &[
        (
            "a global database nothing created",
            "emu-rfgc-absent",
            "emu-rfgc-refused-member",
            404,
            "GlobalClusterNotFoundFault",
            "Global cluster 'emu-rfgc-absent' not found",
        ),
        (
            "a DB cluster nothing created",
            "emu-rfgc-refused",
            "emu-rfgc-absent-cluster",
            404,
            "DBClusterNotFoundFault",
            "DBCluster emu-rfgc-absent-cluster not found.",
        ),
        (
            "a DB cluster ARN of a region this service holds no cluster in",
            "emu-rfgc-refused",
            "arn:aws:rds:ap-south-1:000000000000:cluster:emu-rfgc-refused-member",
            404,
            "DBClusterNotFoundFault",
            "DBCluster arn:aws:rds:ap-south-1:000000000000:cluster:emu-rfgc-refused-member \
             not found.",
        ),
        (
            "a cluster that belongs to no global database",
            "emu-rfgc-refused",
            "emu-rfgc-standalone",
            400,
            "InvalidDBClusterStateFault",
            "DB cluster emu-rfgc-standalone is not a member of global cluster \
             emu-rfgc-refused",
        ),
        (
            "a cluster that is a member of another global database",
            "emu-rfgc-refused",
            "emu-rfgc-other-member",
            400,
            "InvalidDBClusterStateFault",
            "DB cluster emu-rfgc-other-member is not a member of global cluster \
             emu-rfgc-refused",
        ),
    ];

    for &(case, global, db_cluster, status, code, message) in refusals {
        let refused = expect_error(case, detach(&router, global, db_cluster).await, status, code);
        let actual_message = text(&refused, "Message");
        assert_eq!(
            actual_message, message,
            "case {case:?}: expected the refusal {message:?}, got {actual_message:?}"
        );
        // Every membership this service holds is exactly as it was.
        let actual = described_members(&router, "emu-rfgc-refused").await;
        assert_eq!(
            actual,
            vec![db_cluster_arn(REGION, "emu-rfgc-refused-member")],
            "case {case:?}: a refused detach must leave the membership as it was, got {actual:?}"
        );
        let joined = cluster_global(&router, REGION, "emu-rfgc-other-member").await;
        assert_eq!(
            joined,
            Some("emu-rfgc-other".to_string()),
            "case {case:?}: a refused detach must leave the other global database's member \
             joined, got {joined:?}"
        );
    }
}

#[tokio::test]
async fn a_detach_waits_for_a_change_the_global_database_is_still_applying() {
    let router = emulator();
    create_global(
        &router,
        "the upgrading global database",
        "emu-rfgc-upgrade",
        "aurora-mysql",
        "8.0.mysql_aurora.3.05.2",
        None,
    )
    .await;
    create_cluster(
        &router,
        "its member",
        REGION,
        "emu-rfgc-upgrade-member",
        "aurora-mysql",
        "8.0.mysql_aurora.3.05.2",
        Some("emu-rfgc-upgrade"),
    )
    .await;
    // An engine upgrade is work the service runs, so the global database is in a transient
    // status while it runs — and a detach is not a request that status takes.
    expect_ok(
        "the upgrade",
        call(
            &router,
            "ModifyGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-rfgc-upgrade"),
                ("EngineVersion", "8.0.mysql_aurora.3.08.0"),
            ],
        )
        .await,
    );
    expect_error(
        "a detach while the upgrade runs",
        detach(&router, "emu-rfgc-upgrade", "emu-rfgc-upgrade-member").await,
        400,
        "InvalidGlobalClusterStateFault",
    );
    let actual = described_members(&router, "emu-rfgc-upgrade").await;
    assert_eq!(
        actual,
        vec![db_cluster_arn(REGION, "emu-rfgc-upgrade-member")],
        "a refused detach must leave the membership as it was, got {actual:?}"
    );

    // Once the span the clock owes has passed, the same detach is accepted.
    crate::db_instances::wait_for_settle();
    let detached = expect_ok(
        "the detach after the upgrade settled",
        detach(&router, "emu-rfgc-upgrade", "emu-rfgc-upgrade-member").await,
    );
    assert!(
        texts(&detached, "DBClusterArn").is_empty(),
        "the detach must empty the global database, got {detached}"
    );
}
