// SPDX-License-Identifier: CC0-1.0

//! Restoring a cluster from a snapshot, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: a restore mints a NEW
//! cluster whose configuration is the one the snapshot captured of the cluster
//! it was taken of, with the members the request names overriding it, the
//! identity-derived values recomputed for the new identifier, and the security
//! and parameter groups reset to the account's defaults. It is asserted over a
//! TABLE of overrides (one per member class: value, bound, flag, count, form),
//! over the ways a snapshot can be named (identifier in either case, ARN), over
//! the refusals, and over the capture rule — the restore still rebuilds the
//! captured configuration after the source cluster is gone.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The Region the shared harness signs every request for, which is what the
/// endpoint hostnames, ARNs and Availability Zones are derived from.
const REGION: &str = "us-west-2";

/// The cluster every case restores from.
const SOURCE: &str = "restore-source";

/// The snapshot taken of [`SOURCE`], deliberately in MIXED case: the identifier
/// a restore names "isn't case sensitive".
const SNAPSHOT: &str = "Restore-Source-Snap";

/// The parameter group the SOURCE cluster joins — not the account's default one,
/// so "the new cluster is created with the default cluster security and
/// parameter groups" is observable.
const SOURCE_GROUP: &str = "restore-source-group";

/// A second group, for the row that names one on the restore request.
const TARGET_GROUP: &str = "restore-target-group";

/// The cluster subnet group the "restored into a VPC" row names. A restore
/// resolves the name against the store, so the seeded emulator creates it.
const SUBNET_GROUP: &str = "restore-subnets";

/// The admin password the source cluster is created with: a restored cluster
/// uses "the admin credentials the cluster had at the time the snapshot was
/// taken", and no response may ever echo it.
const SOURCE_PASSWORD: &str = "Sourc3-Passw0rd";

/// The source cluster's configuration, every value deliberately DIFFERENT from
/// this service's documented default so that inheriting it is observable.
fn source_parameters() -> Vec<(&'static str, &'static str)> {
    vec![
        ("ClusterIdentifier", SOURCE),
        ("NodeType", "ra3.4xlarge"),
        ("MasterUsername", "sourceadmin"),
        ("MasterUserPassword", SOURCE_PASSWORD),
        ("DBName", "analytics"),
        ("ClusterType", "multi-node"),
        ("NumberOfNodes", "4"),
        ("Port", "8191"),
        ("PubliclyAccessible", "true"),
        ("AllowVersionUpgrade", "false"),
        ("EnhancedVpcRouting", "true"),
        ("AutomatedSnapshotRetentionPeriod", "7"),
        ("PreferredMaintenanceWindow", "tue:03:30-tue:04:00"),
        ("IpAddressType", "dualstack"),
        ("MaintenanceTrackName", "trailing"),
        ("ClusterParameterGroupName", SOURCE_GROUP),
    ]
}

/// The values the snapshot captured, which a restore that overrides nothing must
/// report back. `ManualSnapshotRetentionPeriod` is the snapshot's own.
///
/// The captured PORT is not here: a cluster still being restored publishes no
/// connection endpoint at all, so the port it will serve on is asserted once the
/// restored cluster is serving.
fn captured_configuration() -> Vec<(&'static str, &'static str)> {
    vec![
        ("NodeType", "ra3.4xlarge"),
        ("NumberOfNodes", "4"),
        ("MasterUsername", "sourceadmin"),
        ("DBName", "analytics"),
        ("ClusterVersion", "1.0"),
        ("PubliclyAccessible", "true"),
        ("AllowVersionUpgrade", "false"),
        ("EnhancedVpcRouting", "true"),
        ("AutomatedSnapshotRetentionPeriod", "7"),
        ("ManualSnapshotRetentionPeriod", "14"),
        ("PreferredMaintenanceWindow", "tue:03:30-tue:04:00"),
        ("IpAddressType", "dualstack"),
        ("MaintenanceTrackName", "trailing"),
    ]
}

/// One row of the override table: the case name, the cluster identifier it
/// restores into, the extra request parameters, and the `(element, value)` pairs
/// the restored cluster must report.
type OverrideCase = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
);

/// One row of the refusal table: the case name, the request parameters, and the
/// `(status, error code)` the service must answer with.
type RefusalCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// One row of the locator table: the case name, the cluster identifier the row
/// restores into, and the parameters that name the snapshot.
type LocatorCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// An emulator holding [`SOURCE`] and an AVAILABLE snapshot of it, plus the two
/// parameter groups and the one subnet group the cases name. Returns the
/// snapshot's ARN, which one case restores by.
async fn seeded_emulator() -> (Emulator, String) {
    let emulator = Emulator::new();
    let family = state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION);
    for group in [SOURCE_GROUP, TARGET_GROUP] {
        let created = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", group),
                    ("ParameterGroupFamily", &family),
                    ("Description", "a group the restore cases name"),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case seed parameter group {group}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }

    // A restore into a VPC resolves its `ClusterSubnetGroupName` against the
    // store, so the group one case names has to exist before it is named. Its
    // subnet is one EC2 minted for the account: a Redshift subnet group resolves
    // every `SubnetIds` entry against the service that owns subnets.
    let account_subnets = emulator.subnet_ids("seed subnet group", 1).await;
    let subnets = emulator
        .call(
            "CreateClusterSubnetGroup",
            &[
                ("ClusterSubnetGroupName", SUBNET_GROUP),
                ("Description", "the subnet group a restore case names"),
                ("SubnetIds.SubnetIdentifier.1", &account_subnets[0]),
            ],
        )
        .await;
    assert_eq!(
        subnets.status, 200,
        "case seed subnet group {SUBNET_GROUP}: expected 200, got {} with body {}",
        subnets.status, subnets.body
    );

    let created = emulator.call("CreateCluster", &source_parameters()).await;
    assert_eq!(
        created.status, 200,
        "case seed source cluster: expected 200, got {} with body {}",
        created.status, created.body
    );

    let snapshot = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", SNAPSHOT),
                ("ClusterIdentifier", SOURCE),
                ("ManualSnapshotRetentionPeriod", "14"),
            ],
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "case seed snapshot: expected 200, got {} with body {}",
        snapshot.status, snapshot.body
    );
    let snapshot_arn = snapshot
        .text("SnapshotArn")
        .expect("case seed snapshot: the response carries the snapshot ARN");

    // Both records are minted in `creating`; only a snapshot the clock has
    // already made `available` holds the data a cluster is rebuilt from.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    (emulator, snapshot_arn)
}

/// Restore `target` from the seeded snapshot, naming it in lower case (the
/// identifier is not case sensitive), with `extra` request parameters.
async fn restore(emulator: &Emulator, target: &str, extra: &[(&str, &str)]) -> Answer {
    let lowercased = SNAPSHOT.to_ascii_lowercase();
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", target),
        ("SnapshotIdentifier", &lowercased),
    ];
    params.extend_from_slice(extra);
    emulator
        .call("RestoreFromClusterSnapshot", &params)
        .await
}

/// The cluster `identifier` as `DescribeClusters` reports it.
async fn describe(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await
}

#[tokio::test]
async fn a_restore_rebuilds_the_captured_configuration_under_the_new_identifier() {
    let (emulator, _) = seeded_emulator().await;
    let target = "restored-plain";

    let restored = restore(&emulator, target, &[]).await;
    assert_eq!(
        restored.status, 200,
        "case restore {target}: expected 200, got {} with body {}",
        restored.status, restored.body
    );
    assert_eq!(
        restored.text("ClusterIdentifier").as_deref(),
        Some(target),
        "case restore {target}: expected the new identifier back, got {:?}",
        restored.text("ClusterIdentifier")
    );
    // A restored cluster is still being provisioned, and reports the restore it
    // was created by.
    assert_eq!(
        restored.text("ClusterStatus").as_deref(),
        Some("creating"),
        "case restore {target}: expected ClusterStatus=creating, got {:?}",
        restored.text("ClusterStatus")
    );
    // Several `Cluster` members carry a `Status` element of their own (the VPC
    // security group membership, the HSM status), so the restore's own status is
    // asserted over every `Status` the body holds rather than the first one.
    assert!(
        all_text(&restored.body, "Status").contains(&state::RESTORE_STATUS_RESTORING.to_string()),
        "case restore {target}: expected a RestoreStatus={}, got {:?}",
        state::RESTORE_STATUS_RESTORING,
        all_text(&restored.body, "Status")
    );

    for (member, expected) in captured_configuration() {
        assert_eq!(
            restored.text(member).as_deref(),
            Some(expected),
            "case restore {target} member {member}: expected the captured {expected:?}, got {:?}",
            restored.text(member)
        );
    }

    // The documented exception: security and parameter groups are the account's
    // defaults, never the source cluster's.
    let default_group = state::cluster_default_parameter_group();
    assert_eq!(
        restored.text("ParameterGroupName").as_deref(),
        Some(default_group.as_str()),
        "case restore {target}: expected the default parameter group {default_group:?} rather than the source's {SOURCE_GROUP:?}, got {:?}",
        restored.text("ParameterGroupName")
    );

    // Identity-derived values are recomputed for the NEW cluster. The endpoint
    // is not among them yet: a cluster still being restored publishes none, so
    // the hostname it will be reached at is asserted below, once it is serving.
    assert_eq!(
        restored.text("Address"),
        None,
        "case restore {target}: expected no endpoint address while restoring, got {:?}",
        restored.text("Address")
    );
    let namespace = restored
        .text("ClusterNamespaceArn")
        .expect("case restore: the response carries a namespace ARN");
    assert_eq!(
        namespace,
        state::namespace_arn(target, REGION),
        "case restore {target}: expected the namespace ARN of the new identifier, got {namespace:?}"
    );
    // "Default: A random, system-chosen Availability Zone" — this service's own
    // stable choice for the new identifier, not the snapshot's zone.
    let zone = restored
        .text("AvailabilityZone")
        .expect("case restore: the response carries an Availability Zone");
    assert_eq!(
        zone,
        state::default_availability_zone(target, REGION),
        "case restore {target}: expected the system-chosen zone of the new identifier, got {zone:?}"
    );
    assert!(
        state::region_availability_zones(REGION).contains(&zone),
        "case restore {target}: expected a zone of {REGION}, got {zone:?}"
    );
    assert!(
        !restored.body.contains(SOURCE_PASSWORD),
        "case restore {target}: the admin password must never reach the wire, got body {}",
        restored.body
    );

    // ROUND TRIP: the record the restore persisted is what a later read reports.
    let described = describe(&emulator, target).await;
    assert_eq!(
        described.status, 200,
        "case describe {target}: expected 200, got {} with body {}",
        described.status, described.body
    );
    for (member, expected) in captured_configuration() {
        assert_eq!(
            described.text(member).as_deref(),
            Some(expected),
            "case describe {target} member {member}: expected the persisted {expected:?}, got {:?}",
            described.text(member)
        );
    }

    // The source cluster is untouched: a restore reads the snapshot and writes
    // only the new record.
    let source = describe(&emulator, SOURCE).await;
    assert_eq!(
        source.text("ParameterGroupName").as_deref(),
        Some(SOURCE_GROUP),
        "case describe {SOURCE}: expected the source to keep its own parameter group, got {:?}",
        source.text("ParameterGroupName")
    );
    let listed = all_text(
        &emulator.call("DescribeClusters", &[]).await.body,
        "ClusterIdentifier",
    );
    assert!(
        listed.iter().any(|id| id == SOURCE) && listed.iter().any(|id| id == target),
        "case list after restore: expected both {SOURCE:?} and {target:?}, got {listed:?}"
    );

    // The clock owes the restored cluster its `available` state, and the restore
    // is complete exactly when the cluster is.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let settled = describe(&emulator, target).await;
    assert_eq!(
        settled.text("ClusterStatus").as_deref(),
        Some(state::STATUS_AVAILABLE),
        "case settled {target}: expected available, got {:?}",
        settled.text("ClusterStatus")
    );
    assert!(
        all_text(&settled.body, "Status").contains(&state::RESTORE_STATUS_COMPLETED.to_string()),
        "case settled {target}: expected a RestoreStatus={}, got {:?}",
        state::RESTORE_STATUS_COMPLETED,
        all_text(&settled.body, "Status")
    );
    // Serving at last, so the restored cluster publishes its endpoint: the
    // hostname of the NEW identifier, on the port the snapshot captured
    // ("Default: The same port as the original cluster").
    assert_eq!(
        settled.text("Address").as_deref(),
        Some(state::endpoint_address(target, REGION).as_str()),
        "case settled {target}: expected the endpoint host of the new identifier, got {:?}",
        settled.text("Address")
    );
    assert_eq!(
        settled.text("Port").as_deref(),
        Some("8191"),
        "case settled {target}: expected the captured port once serving, got {:?}",
        settled.text("Port")
    );
}

#[tokio::test]
async fn a_request_member_overrides_the_captured_value() {
    let (emulator, _) = seeded_emulator().await;

    // One row per class the rule generalises over: a plain value, each published
    // bound of a range, a flag either way, a node count up and down, a derived
    // form, and the group names the documented exception resets.
    let cases: Vec<OverrideCase> = vec![
        (
            "node type",
            "restored-node-type",
            vec![("NodeType", "ra3.16xlarge")],
            vec![("NodeType", "ra3.16xlarge")],
        ),
        (
            "node count grows the cluster",
            "restored-eight-nodes",
            vec![("NumberOfNodes", "8")],
            vec![("NumberOfNodes", "8")],
        ),
        (
            "node count shrinks to a single node",
            "restored-one-node",
            vec![("NumberOfNodes", "1")],
            vec![("NumberOfNodes", "1")],
        ),
        (
            "availability zone",
            "restored-zone",
            vec![("AvailabilityZone", "us-west-2c")],
            vec![("AvailabilityZone", "us-west-2c")],
        ),
        (
            "automated retention at the documented lower bound",
            "restored-retention-min",
            vec![("AutomatedSnapshotRetentionPeriod", "0")],
            vec![("AutomatedSnapshotRetentionPeriod", "0")],
        ),
        (
            "automated retention at the documented upper bound",
            "restored-retention-max",
            vec![("AutomatedSnapshotRetentionPeriod", "35")],
            vec![("AutomatedSnapshotRetentionPeriod", "35")],
        ),
        (
            "manual retention indefinite",
            "restored-manual-forever",
            vec![("ManualSnapshotRetentionPeriod", "-1")],
            vec![("ManualSnapshotRetentionPeriod", "-1")],
        ),
        (
            "manual retention at the documented upper bound",
            "restored-manual-max",
            vec![("ManualSnapshotRetentionPeriod", "3653")],
            vec![("ManualSnapshotRetentionPeriod", "3653")],
        ),
        (
            "flags both flipped",
            "restored-flags",
            vec![
                ("PubliclyAccessible", "false"),
                ("AllowVersionUpgrade", "true"),
                ("EnhancedVpcRouting", "false"),
            ],
            vec![
                ("PubliclyAccessible", "false"),
                ("AllowVersionUpgrade", "true"),
                ("EnhancedVpcRouting", "false"),
            ],
        ),
        (
            "maintenance window and track",
            "restored-window",
            vec![
                ("PreferredMaintenanceWindow", "sun:23:00-sun:23:30"),
                ("MaintenanceTrackName", "current"),
            ],
            vec![
                ("PreferredMaintenanceWindow", "sun:23:00-sun:23:30"),
                ("MaintenanceTrackName", "current"),
            ],
        ),
        (
            "ip address type and multi-az render their documented words",
            "restored-addressing",
            vec![("IpAddressType", "ipv4"), ("MultiAZ", "true")],
            vec![("IpAddressType", "ipv4"), ("MultiAZ", "Enabled")],
        ),
        (
            "a parameter group the request names",
            "restored-named-group",
            vec![("ClusterParameterGroupName", TARGET_GROUP)],
            vec![("ParameterGroupName", TARGET_GROUP)],
        ),
        (
            "a subnet group puts the restored cluster in a vpc",
            "restored-in-vpc",
            vec![("ClusterSubnetGroupName", SUBNET_GROUP)],
            vec![
                ("ClusterSubnetGroupName", SUBNET_GROUP),
                ("VpcId", "placeholder"),
            ],
        ),
    ];

    for (case, target, extra, expectations) in cases {
        let answer = restore(&emulator, target, &extra).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        // The VPC id is the one the SUBNET GROUP holds — the VPC its subnets are
        // in, as the service that owns subnets answered for them — so it is asked
        // of the group rather than spelled out here.
        let group = emulator
            .call(
                "DescribeClusterSubnetGroups",
                &[("ClusterSubnetGroupName", SUBNET_GROUP)],
            )
            .await;
        let group_vpc = group.text("VpcId").unwrap_or_else(|| {
            panic!("case {case}: the subnet group must report its VPC, got body {}", group.body)
        });
        let described = describe(&emulator, target).await;
        for (member, expected) in expectations {
            let expected = if member == "VpcId" {
                group_vpc.clone()
            } else {
                expected.to_string()
            };
            assert_eq!(
                answer.text(member).as_deref(),
                Some(expected.as_str()),
                "case {case} member {member}: expected {expected:?} in the restore response, got {:?}",
                answer.text(member)
            );
            assert_eq!(
                described.text(member).as_deref(),
                Some(expected.as_str()),
                "case {case} member {member}: expected {expected:?} to be persisted, got {:?}",
                described.text(member)
            );
        }
    }

    // The PORT a request overrides is part of the connection endpoint, which a
    // cluster still being restored publishes none of — so the override is
    // observed once the restored cluster is serving.
    let ported = restore(&emulator, "restored-port", &[("Port", "5455")]).await;
    assert_eq!(
        ported.status, 200,
        "case port: expected 200, got {} with body {}",
        ported.status, ported.body
    );
    assert_eq!(
        ported.text("Port"),
        None,
        "case port: expected no endpoint port while restoring, got {:?}",
        ported.text("Port")
    );
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let serving = describe(&emulator, "restored-port").await;
    assert_eq!(
        serving.text("Port").as_deref(),
        Some("5455"),
        "case port: expected the overriding port once the cluster is serving, got {:?}",
        serving.text("Port")
    );
    assert_eq!(
        serving.text("Address").as_deref(),
        Some(state::endpoint_address("restored-port", REGION).as_str()),
        "case port: expected the endpoint host of the new identifier, got {:?}",
        serving.text("Address")
    );
}

#[tokio::test]
async fn a_snapshot_can_be_named_by_identifier_in_any_case_or_by_arn() {
    let (emulator, snapshot_arn) = seeded_emulator().await;
    let lowercased = SNAPSHOT.to_ascii_lowercase();
    let uppercased = SNAPSHOT.to_ascii_uppercase();

    // The identifier "isn't case sensitive", and the ARN names the same record.
    let cases: Vec<LocatorCase<'_>> = vec![
        (
            "identifier as stored",
            "restored-as-stored",
            vec![("SnapshotIdentifier", SNAPSHOT)],
        ),
        (
            "identifier lowercased",
            "restored-lowercased",
            vec![("SnapshotIdentifier", &lowercased)],
        ),
        (
            "identifier uppercased",
            "restored-uppercased",
            vec![("SnapshotIdentifier", &uppercased)],
        ),
        (
            "snapshot arn",
            "restored-by-arn",
            vec![("SnapshotArn", snapshot_arn.as_str())],
        ),
        (
            "identifier with the source cluster named",
            "restored-with-source",
            vec![
                ("SnapshotIdentifier", SNAPSHOT),
                ("SnapshotClusterIdentifier", SOURCE),
            ],
        ),
    ];

    for (case, target, locator) in cases {
        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", target)];
        params.extend_from_slice(&locator);
        let answer = emulator
            .call("RestoreFromClusterSnapshot", &params)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("NodeType").as_deref(),
            Some("ra3.4xlarge"),
            "case {case}: expected the captured node type, got {:?}",
            answer.text("NodeType")
        );
    }
}

#[tokio::test]
async fn a_restore_the_service_cannot_perform_is_refused_with_its_modelled_fault() {
    let (emulator, snapshot_arn) = seeded_emulator().await;
    let lowercased = SNAPSHOT.to_ascii_lowercase();

    let cases: Vec<RefusalCase<'_>> = vec![
        (
            "neither the identifier nor the ARN",
            vec![("ClusterIdentifier", "restored-nothing-named")],
            400,
            "ValidationException",
        ),
        (
            "both the identifier and the ARN",
            vec![
                ("ClusterIdentifier", "restored-both-named"),
                ("SnapshotIdentifier", &lowercased),
                ("SnapshotArn", snapshot_arn.as_str()),
            ],
            400,
            "ValidationException",
        ),
        (
            "an identifier no snapshot carries",
            vec![
                ("ClusterIdentifier", "restored-unknown-snapshot"),
                ("SnapshotIdentifier", "no-such-snapshot"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "an ARN no snapshot carries",
            vec![
                ("ClusterIdentifier", "restored-unknown-arn"),
                (
                    "SnapshotArn",
                    "arn:aws:redshift:us-west-2:000000000000:snapshot:other/other",
                ),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a source cluster the snapshot was not taken of",
            vec![
                ("ClusterIdentifier", "restored-wrong-source"),
                ("SnapshotIdentifier", &lowercased),
                ("SnapshotClusterIdentifier", "some-other-cluster"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "an identifier that already names a cluster",
            vec![
                ("ClusterIdentifier", SOURCE),
                ("SnapshotIdentifier", &lowercased),
            ],
            400,
            "ClusterAlreadyExists",
        ),
        (
            "a target identifier the documented syntax forbids",
            vec![
                ("ClusterIdentifier", "Restored-Uppercase"),
                ("SnapshotIdentifier", &lowercased),
            ],
            400,
            "ValidationException",
        ),
        (
            "a parameter group no record names",
            vec![
                ("ClusterIdentifier", "restored-unknown-group"),
                ("SnapshotIdentifier", &lowercased),
                ("ClusterParameterGroupName", "no-such-group"),
            ],
            404,
            "ClusterParameterGroupNotFound",
        ),
        (
            "a subnet group no record names",
            vec![
                ("ClusterIdentifier", "restored-unknown-subnets"),
                ("SnapshotIdentifier", &lowercased),
                ("ClusterSubnetGroupName", "no-such-subnet-group"),
            ],
            400,
            "ClusterSubnetGroupNotFoundFault",
        ),
        (
            "a snapshot another account owns",
            vec![
                ("ClusterIdentifier", "restored-foreign-owner"),
                ("SnapshotIdentifier", &lowercased),
                ("OwnerAccount", "999988887777"),
            ],
            400,
            "AccessToSnapshotDenied",
        ),
        (
            "a reserved node this service does not hold",
            vec![
                ("ClusterIdentifier", "restored-reserved-node"),
                ("SnapshotIdentifier", &lowercased),
                ("ReservedNodeId", "reserved-1"),
            ],
            404,
            "ReservedNodeNotFound",
        ),
        (
            "a reserved node offering this service does not hold",
            vec![
                ("ClusterIdentifier", "restored-reserved-offering"),
                ("SnapshotIdentifier", &lowercased),
                ("TargetReservedNodeOfferingId", "offering-1"),
            ],
            404,
            "ReservedNodeOfferingNotFound",
        ),
        (
            "an IAM Identity Center application this service does not hold",
            vec![
                ("ClusterIdentifier", "restored-idc"),
                ("SnapshotIdentifier", &lowercased),
                (
                    "RedshiftIdcApplicationArn",
                    "arn:aws:redshift:us-west-2:000000000000:redshiftidcapplication:app",
                ),
            ],
            404,
            "RedshiftIdcApplicationNotExists",
        ),
        (
            "an automated retention period one past the documented upper bound",
            vec![
                ("ClusterIdentifier", "restored-retention-over"),
                ("SnapshotIdentifier", &lowercased),
                ("AutomatedSnapshotRetentionPeriod", "36"),
            ],
            400,
            "ValidationException",
        ),
        (
            "an automated retention period one past the documented lower bound",
            vec![
                ("ClusterIdentifier", "restored-retention-under"),
                ("SnapshotIdentifier", &lowercased),
                ("AutomatedSnapshotRetentionPeriod", "-1"),
            ],
            400,
            "ValidationException",
        ),
        (
            "a manual retention period one past the documented upper bound",
            vec![
                ("ClusterIdentifier", "restored-manual-over"),
                ("SnapshotIdentifier", &lowercased),
                ("ManualSnapshotRetentionPeriod", "3654"),
            ],
            400,
            "ValidationException",
        ),
        (
            "a manual retention period below the documented lower bound",
            vec![
                ("ClusterIdentifier", "restored-manual-under"),
                ("SnapshotIdentifier", &lowercased),
                ("ManualSnapshotRetentionPeriod", "0"),
            ],
            400,
            "ValidationException",
        ),
        (
            "a node count no cluster can have",
            vec![
                ("ClusterIdentifier", "restored-zero-nodes"),
                ("SnapshotIdentifier", &lowercased),
                ("NumberOfNodes", "0"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, status, code) in cases {
        let answer = emulator
            .call("RestoreFromClusterSnapshot", &params)
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (status, Some(code)),
            "case {case}: expected {status} {code}, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        // A refused restore persists nothing.
        if let Some(target) = params
            .iter()
            .find(|(name, _)| *name == "ClusterIdentifier")
            .map(|(_, value)| *value)
        {
            if target != SOURCE {
                let described = describe(&emulator, target).await;
                assert_eq!(
                    described.error_code().as_deref(),
                    Some("ClusterNotFound"),
                    "case {case}: expected no cluster {target:?} to have been created, got {} / {}",
                    described.status,
                    described.body
                );
            }
        }
    }
}

#[tokio::test]
async fn only_an_available_snapshot_can_be_restored() {
    let emulator = Emulator::new();
    let created = emulator.create_minimal("state-source").await;
    assert_eq!(
        created.status, 200,
        "case seed cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let snapshot = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "state-snap"),
                ("ClusterIdentifier", "state-source"),
            ],
        )
        .await;
    assert_eq!(
        snapshot.text("Status").as_deref(),
        Some("creating"),
        "case seed snapshot: expected a snapshot minted in creating, got {:?}",
        snapshot.text("Status")
    );

    // Before the clock-owed transition the snapshot holds no restorable data.
    let early = emulator
        .call(
            "RestoreFromClusterSnapshot",
            &[
                ("ClusterIdentifier", "restored-too-early"),
                ("SnapshotIdentifier", "state-snap"),
            ],
        )
        .await;
    assert_eq!(
        (early.status, early.error_code().as_deref()),
        (400, Some("InvalidClusterSnapshotState")),
        "case restore while creating: expected 400 InvalidClusterSnapshotState, got {} / {:?} with body {}",
        early.status,
        early.error_code(),
        early.body
    );

    // Once it is available the very same request succeeds.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let late = emulator
        .call(
            "RestoreFromClusterSnapshot",
            &[
                ("ClusterIdentifier", "restored-too-early"),
                ("SnapshotIdentifier", "state-snap"),
            ],
        )
        .await;
    assert_eq!(
        late.status, 200,
        "case restore once available: expected 200, got {} / {:?} with body {}",
        late.status,
        late.error_code(),
        late.body
    );
}

#[tokio::test]
async fn a_snapshot_restores_the_configuration_it_captured_even_once_the_source_is_gone() {
    let (emulator, _) = seeded_emulator().await;

    // The source cluster is modified AFTER the snapshot was taken, and then
    // deleted: a snapshot is a copy of the cluster as of its own instant, so
    // neither change can reach the restore.
    let modified = emulator
        .call(
            "ModifyCluster",
            &[
                ("ClusterIdentifier", SOURCE),
                ("NodeType", "dc2.large"),
                ("ClusterType", "single-node"),
                ("NumberOfNodes", "1"),
            ],
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "case modify the source: expected 200, got {} with body {}",
        modified.status, modified.body
    );
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", SOURCE),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the source: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        describe(&emulator, SOURCE).await.error_code().as_deref(),
        Some("ClusterNotFound"),
        "case delete the source: expected the source cluster to be gone"
    );

    let restored = restore(&emulator, "restored-after-delete", &[]).await;
    assert_eq!(
        restored.status, 200,
        "case restore after delete: expected 200, got {} with body {}",
        restored.status, restored.body
    );
    for (member, expected) in captured_configuration() {
        assert_eq!(
            restored.text(member).as_deref(),
            Some(expected),
            "case restore after delete, member {member}: expected the captured {expected:?}, got {:?}",
            restored.text(member)
        );
    }
}
