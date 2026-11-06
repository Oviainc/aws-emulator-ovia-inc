// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Aurora global database lifecycle across the engine versions
//! this service PUBLISHES: `CreateGlobalCluster` -> `CreateDBCluster` (joining it) ->
//! `AddTagsToResource` / `ListTagsForResource` on the global cluster's ARN ->
//! `DeleteDBCluster` -> `DeleteGlobalCluster`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (through `db_instances::emulator`, the one mounting
//! helper this test target has), and each request is a real awsQuery form POST carrying the
//! SigV4 credential scope the router keys on. The assertions read the XML the emulator
//! answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test:
//!
//! * a create this service answers 200 for must be answerable for EVERY engine version its
//!   own catalogue publishes as available — `DescribeDBEngineVersions` is what publishes
//!   which versions a cluster can be created on, so a version in that report and a version
//!   `CreateDBCluster` accepts are the same set, and the cluster reports the version it was
//!   created on;
//! * a resource a create persisted is reachable by every later call that looks it up: the
//!   cluster report, the global database's member list, the tag write and the tag read
//!   addressed by the global cluster's own ARN, and both deletes — and the modelled
//!   not-found fault is answered only for a resource nothing created or one a delete has
//!   already removed.
//!
//! The table is the catalogue's OWN: every version `DescribeDBEngineVersions` reports as
//! available for each engine `CreateGlobalClusterMessage$Engine` publishes, read over the
//! wire, so a version added to or removed from that catalogue is covered without touching
//! this file. The rows therefore differ along the engine, the major version family and the
//! form each engine publishes its builds in.

use service_com_amazonaws_rds_amazonrdsv19::state::GLOBAL_CLUSTER_ENGINES;
use crate::db_engine_versions::all_versions_of;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, subtrees, text, texts, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The `Status` the engine-version report carries for a version a create may name; the
/// value `DescribeDBEngineVersionsMessage$IncludeAll` documents as the one reported by
/// default.
const STATUS_AVAILABLE: &str = "available";

/// The tag every row writes on its global cluster, so the tag read has something of the
/// caller's to report.
const TAG: (&str, &str) = ("fixture", "global-database");

/// A global cluster's ARN, which carries NO region: it is an account-scoped resource.
fn global_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds::{ACCOUNT}:global-cluster:{identifier}")
}

/// A member DB cluster's ARN, which is regional like the cluster itself.
fn cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

/// A name that is unique per row, short enough for every identifier bound these calls hold.
fn identifier(prefix: &str, index: usize) -> String {
    format!("emu-gv-{prefix}-{index}")
}

#[tokio::test]
async fn every_published_version_can_carry_a_global_database_through_its_whole_lifecycle() {
    let router = emulator();

    let mut rows = 0usize;
    for engine in GLOBAL_CLUSTER_ENGINES {
        // The table is the engine's own published report, narrowed to the versions it
        // reports as available — the versions a create may name.
        let published: Vec<String> = all_versions_of(&router, engine)
            .await
            .into_iter()
            .filter(|version| version.status == STATUS_AVAILABLE)
            .map(|version| version.engine_version)
            .collect();
        assert!(
            !published.is_empty(),
            "case {engine:?}: the catalogue must publish an available version for a \
             global-database engine, got none"
        );

        for version in &published {
            rows += 1;
            let case = format!("{engine} {version}");
            let global = identifier("gc", rows);
            let member = identifier("member", rows);

            // 1. The global database, created on this published version.
            let created = expect_ok(
                &case,
                call(
                    &router,
                    "CreateGlobalCluster",
                    &[
                        ("GlobalClusterIdentifier", global.as_str()),
                        ("Engine", engine),
                        ("EngineVersion", version.as_str()),
                    ],
                )
                .await,
            );
            assert_eq!(
                text(&created, "GlobalClusterArn"),
                global_cluster_arn(&global),
                "case {case:?}: expected the global cluster's account-scoped ARN, got \
                 {created}"
            );

            // 2. A DB cluster created ON that version, joining the global database. A
            //    version the report publishes must be one this create accepts, and the
            //    cluster must report the version it was created on.
            let joined = expect_ok(
                &case,
                call(
                    &router,
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", member.as_str()),
                        ("Engine", engine),
                        ("EngineVersion", version.as_str()),
                        ("GlobalClusterIdentifier", global.as_str()),
                    ],
                )
                .await,
            );
            for (member_name, expected) in [
                ("EngineVersion", version.clone()),
                ("GlobalClusterIdentifier", global.clone()),
                ("DBClusterArn", cluster_arn(&member)),
            ] {
                let actual = text(&joined, member_name);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {member_name} {expected:?}, got {actual:?}"
                );
            }

            // 3. Both resources are reachable by the reads that look them up: the cluster
            //    by its own report, and its membership from the global database's record.
            let described = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", member.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&described, "EngineVersion"),
                *version,
                "case {case:?}: the cluster report must carry the version it was created \
                 on, got {described}"
            );
            let reported_global = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", global.as_str())],
                )
                .await,
            );
            assert_eq!(
                texts(&reported_global, "DBClusterArn"),
                vec![cluster_arn(&member)],
                "case {case:?}: expected the global database to report its one member, got \
                 {reported_global}"
            );

            // 4. The global cluster's ARN resolves for the tag calls: a tag written through
            //    one is read back by the other.
            expect_ok(
                &case,
                call(
                    &router,
                    "AddTagsToResource",
                    &[
                        ("ResourceName", global_cluster_arn(&global)),
                        ("Tags.Tag.1.Key", TAG.0.to_string()),
                        ("Tags.Tag.1.Value", TAG.1.to_string()),
                    ],
                )
                .await,
            );
            let listed = expect_ok(
                &case,
                call(
                    &router,
                    "ListTagsForResource",
                    &[("ResourceName", global_cluster_arn(&global))],
                )
                .await,
            );
            let tags: Vec<(String, String)> = subtrees(&listed, "Tag")
                .iter()
                .map(|tag| (text(tag, "Key"), text(tag, "Value")))
                .collect();
            assert_eq!(
                tags,
                vec![(TAG.0.to_string(), TAG.1.to_string())],
                "case {case:?}: expected the tag the write persisted, got {tags:?}"
            );

            // 5. The global database still holds a member, so the delete of it is refused
            //    with the modelled state fault rather than orphaning the member.
            expect_error(
                &case,
                call(
                    &router,
                    "DeleteGlobalCluster",
                    &[("GlobalClusterIdentifier", global.as_str())],
                )
                .await,
                400,
                "InvalidGlobalClusterStateFault",
            );

            // 6. The member is deleted first, which detaches it, and then the global
            //    database itself: both accepted, and each answers with its own envelope.
            let member_deleted = expect_ok(
                &case,
                call(
                    &router,
                    "DeleteDBCluster",
                    &[
                        ("DBClusterIdentifier", member.as_str()),
                        ("SkipFinalSnapshot", "true"),
                    ],
                )
                .await,
            );
            assert_eq!(
                text(&member_deleted, "DBClusterArn"),
                cluster_arn(&member),
                "case {case:?}: the delete must answer with the cluster it retired, got \
                 {member_deleted}"
            );
            let global_deleted = expect_ok(
                &case,
                call(
                    &router,
                    "DeleteGlobalCluster",
                    &[("GlobalClusterIdentifier", global.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&global_deleted, "GlobalClusterArn"),
                global_cluster_arn(&global),
                "case {case:?}: the delete must answer with the global cluster it removed, \
                 got {global_deleted}"
            );

            // 7. Only now is the global cluster not found — by the report, by the tag calls
            //    addressed to its ARN and by a second delete alike.
            expect_error(
                &case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", global.as_str())],
                )
                .await,
                404,
                "GlobalClusterNotFoundFault",
            );
            expect_error(
                &case,
                call(
                    &router,
                    "ListTagsForResource",
                    &[("ResourceName", global_cluster_arn(&global))],
                )
                .await,
                404,
                "GlobalClusterNotFoundFault",
            );
            expect_error(
                &case,
                call(
                    &router,
                    "DeleteGlobalCluster",
                    &[("GlobalClusterIdentifier", global.as_str())],
                )
                .await,
                404,
                "GlobalClusterNotFoundFault",
            );
        }
    }

    assert!(
        rows > 1,
        "expected the published catalogue to give this rule more than one row, got {rows}"
    );
}
