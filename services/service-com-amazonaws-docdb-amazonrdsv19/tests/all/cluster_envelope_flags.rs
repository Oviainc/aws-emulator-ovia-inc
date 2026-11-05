// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members Amazon DocumentDB's cluster envelope ALWAYS carries
//! beyond what the shipped model shape declares: `AutoMinorVersionUpgrade`,
//! `ActivityStreamStatus`, `DBActivityStreamStatus`, `DocdbAnalyticsEnabled`,
//! `CustomDataGroupPresent`, `VPCNetworkingEnabled` and `InternetAccessGatewayEnabled`.
//!
//! The rule: every member the cluster envelope always carries is projected
//! unconditionally, out of the cluster record, by the one projection every path shares
//! (`crate::state::cluster_to_shape`) — and where no input member of this service can set
//! one, the record is born at the value the service reports for a cluster created without
//! it: automatic minor version upgrade on, the DocumentDB-only capability flags off, VPC
//! networking on (a cluster is reachable only from inside a virtual private cloud) with no
//! internet access gateway, and the database activity stream in its stopped steady state.
//! The constants in `src/state.rs` cite each one.
//!
//! The rule generalises over the cluster's inputs (none of them touches these seven
//! members) and over the operation whose envelope carries the cluster, so the table below
//! spans BOTH: each row is a differently shaped `CreateDBCluster`, and every row is read
//! through the create, describe, modify and delete envelopes in turn.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production router uses, with the wire harness this target shares
//! (`tests/all/cluster_lifecycle.rs`).

use crate::cluster_lifecycle::{count, let_the_clock_catch_up, opt_text, text, texts, Docdb};

/// The members and the value each must carry for a cluster created without any input that
/// could change it. Source: the `DBCluster` members of this service's on-disk model (added
/// there from the recorded AWS responses) and the defaults cited on
/// `state::DEFAULT_CLUSTER_AUTO_MINOR_VERSION_UPGRADE`, `state::ACTIVITY_STREAM_STOPPED`,
/// `state::DEFAULT_DOCDB_ANALYTICS_ENABLED`, `state::DEFAULT_CUSTOM_DATA_GROUP_PRESENT`,
/// `state::DEFAULT_VPC_NETWORKING_ENABLED` and
/// `state::DEFAULT_INTERNET_ACCESS_GATEWAY_ENABLED`.
const ENVELOPE_FLAGS: [(&str, &str); 7] = [
    ("AutoMinorVersionUpgrade", "true"),
    ("ActivityStreamStatus", "stopped"),
    ("DBActivityStreamStatus", "stopped"),
    ("DocdbAnalyticsEnabled", "false"),
    ("CustomDataGroupPresent", "false"),
    ("VPCNetworkingEnabled", "true"),
    ("InternetAccessGatewayEnabled", "false"),
];

/// Assert that `xml` carries every element of [`ENVELOPE_FLAGS`] with its value — asserted
/// on the raw XML, so a member the serializer omits fails rather than reading as a default.
fn assert_envelope_flags(case: &str, xml: &str) {
    for (member, expected) in ENVELOPE_FLAGS {
        let actual = opt_text(xml, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: the cluster envelope must carry {member}={expected}, \
             got {actual:?} in {xml}"
        );
    }
}

/// One row of the table: a case name and the extra parameters its `CreateDBCluster` sends
/// beyond the identifier and the engine.
struct CreateCase {
    label: &'static str,
    identifier: &'static str,
    params: Vec<(&'static str, &'static str)>,
}

/// The table: clusters differing in the inputs `CreateDBCluster` takes — credentials,
/// port, retention, encryption, protection, logs exports, storage type, scaling, network
/// type, tags and availability zones. None of them is an input to the members under test,
/// which is exactly the rule.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the bare minimum",
            identifier: "flags-minimal",
            params: vec![],
        },
        CreateCase {
            label: "with master credentials",
            identifier: "flags-credentials",
            params: vec![
                ("MasterUsername", "admin"),
                ("MasterUserPassword", "secret99"),
            ],
        },
        CreateCase {
            label: "encrypted, with a key and a long retention",
            identifier: "flags-encrypted",
            params: vec![
                ("StorageEncrypted", "true"),
                ("KmsKeyId", "alias/aws/rds"),
                ("BackupRetentionPeriod", "35"),
            ],
        },
        CreateCase {
            label: "protected from deletion, on a chosen port",
            identifier: "flags-protected",
            params: vec![("DeletionProtection", "true"), ("Port", "27018")],
        },
        CreateCase {
            label: "exporting logs, copying tags, on the iopt1 storage type",
            identifier: "flags-logs",
            params: vec![
                ("EnableCloudwatchLogsExports.member.1", "audit"),
                ("CopyTagsToSnapshot", "true"),
                ("StorageType", "iopt1"),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "tests"),
            ],
        },
        CreateCase {
            label: "serverless, in two availability zones, on a chosen network type",
            identifier: "flags-serverless",
            params: vec![
                ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
                ("ServerlessV2ScalingConfiguration.MaxCapacity", "8"),
                ("AvailabilityZones.AvailabilityZone.1", "us-east-1a"),
                ("AvailabilityZones.AvailabilityZone.2", "us-east-1b"),
                ("NetworkType", "IPV4"),
            ],
        },
    ]
}

#[tokio::test]
async fn every_cluster_envelope_carries_the_added_members_whatever_the_create_asked_for() {
    let docdb = Docdb::new();

    for case in create_cases() {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", case.identifier), ("Engine", "docdb")];
        params.extend(case.params.iter().copied());

        // 1. The create envelope.
        let created = docdb.ok(case.label, "CreateDBCluster", &params).await;
        assert_envelope_flags(&format!("{} / create", case.label), &created);

        let_the_clock_catch_up();

        // 2. The describe envelope, which reads the record back out of state.
        let described = docdb
            .ok(
                case.label,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", case.identifier)],
            )
            .await;
        assert_envelope_flags(&format!("{} / describe", case.label), &described);

        // 3. The modify envelope — and a modify that changes something else must leave
        //    every one of them alone.
        let modified = docdb
            .ok(
                case.label,
                "ModifyDBCluster",
                &[
                    ("DBClusterIdentifier", case.identifier),
                    ("BackupRetentionPeriod", "7"),
                    ("DeletionProtection", "false"),
                ],
            )
            .await;
        assert_eq!(
            text(&modified, "BackupRetentionPeriod"),
            "7",
            "case {:?}: the modify must really have been applied, got {modified}",
            case.label
        );
        assert_envelope_flags(&format!("{} / modify", case.label), &modified);

        // 4. The delete envelope, the last path that reports a cluster.
        let deleted = docdb
            .ok(
                case.label,
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", case.identifier),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await;
        assert_eq!(
            text(&deleted, "Status"),
            "deleting",
            "case {:?}: the delete must report the deleting status, got {deleted}",
            case.label
        );
        assert_envelope_flags(&format!("{} / delete", case.label), &deleted);
    }
}

#[tokio::test]
async fn a_multi_cluster_page_carries_the_flags_on_every_cluster_it_lists() {
    let docdb = Docdb::new();
    let identifiers = ["page-one", "page-two", "page-three"];
    for identifier in identifiers {
        docdb
            .ok(
                identifier,
                "CreateDBCluster",
                &[("DBClusterIdentifier", identifier), ("Engine", "docdb")],
            )
            .await;
    }
    let_the_clock_catch_up();

    let page = docdb.ok("whole page", "DescribeDBClusters", &[]).await;
    assert_eq!(
        count(&page, "DBCluster"),
        identifiers.len(),
        "case \"whole page\": expected every cluster, got {page}"
    );
    for (member, expected) in ENVELOPE_FLAGS {
        assert_eq!(
            count(&page, member),
            identifiers.len(),
            "case \"whole page\": every listed cluster must carry one {member}, got {page}"
        );
        for (index, value) in texts(&page, member).into_iter().enumerate() {
            assert_eq!(
                value, expected,
                "case \"whole page\" (cluster {index}): expected {member}={expected}, \
                 got {value:?} in {page}"
            );
        }
    }
}

#[tokio::test]
async fn an_instance_keeps_its_own_documented_minor_version_upgrade_default() {
    // The cluster-level flag is a different member from the instance-level one:
    // `CreateDBInstanceMessage$AutoMinorVersionUpgrade` documents "Default: false" and is
    // a real input, so an instance created without it reports false while its cluster
    // reports true.
    let docdb = Docdb::new();
    docdb
        .ok(
            "host cluster",
            "CreateDBCluster",
            &[("DBClusterIdentifier", "flags-host"), ("Engine", "docdb")],
        )
        .await;
    for (case, identifier, sent, expected) in [
        ("no input", "flags-member-default", None, "false"),
        (
            "explicit false",
            "flags-member-off",
            Some("false"),
            "false",
        ),
        ("explicit true", "flags-member-on", Some("true"), "true"),
    ] {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBInstanceIdentifier", identifier),
            ("DBInstanceClass", "db.r5.large"),
            ("Engine", "docdb"),
            ("DBClusterIdentifier", "flags-host"),
        ];
        if let Some(sent) = sent {
            params.push(("AutoMinorVersionUpgrade", sent));
        }
        let created = docdb.ok(case, "CreateDBInstance", &params).await;
        assert_eq!(
            opt_text(&created, "AutoMinorVersionUpgrade").as_deref(),
            Some(expected),
            "case {case:?}: an instance reports the flag it was created with, got {created}"
        );
    }

    let_the_clock_catch_up();
    let cluster = docdb
        .ok(
            "host cluster describe",
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "flags-host")],
        )
        .await;
    assert_envelope_flags("host cluster describe", &cluster);
}
