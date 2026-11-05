// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a cluster envelope reports about the cluster's NETWORKING and
//! about the window it can be restored inside.
//!
//! Three rules, each asserted over a table that spans what the rule generalises over, and
//! each read back through every operation whose envelope carries a cluster — so a value
//! the create invented rather than stored cannot pass:
//!
//! 1. Every security group the request names is a membership of the cluster: the cluster
//!    reports one `VpcSecurityGroupMembership` per requested `VpcSecurityGroupIds` entry,
//!    in the order requested, each carrying that id and the active membership status
//!    (`DBCluster$VpcSecurityGroups`: "Provides a list of virtual private cloud (VPC)
//!    security groups that the cluster belongs to"). The table spans the COUNT and the
//!    FORM of the ids, and the membership list is read back after a `ModifyDBCluster`
//!    replaces it and off the cluster's member instances, which belong to the same groups.
//! 2. Every cluster in a Region reports the one Route 53 hosted zone its endpoints live
//!    in (`DBCluster$HostedZoneId`, `Endpoint$HostedZoneId`: "Specifies the ID that Amazon
//!    Route 53 assigns when you create a hosted zone"), stable across reads and identical
//!    for every cluster and every instance endpoint in the Region.
//! 3. A cluster whose continuous backup has started reports the window a point-in-time
//!    restore may name (`RestoreDBClusterToPointInTimeMessage$RestoreToTime` "must be
//!    before the latest restorable time"): `EarliestRestorableTime` at the start of the
//!    retained backups — never before the cluster existed — and `LatestRestorableTime` at
//!    the instant the window is read. A cluster still provisioning reports neither. The
//!    table spans the `BackupRetentionPeriod` that sets the window's width, including both
//!    published bounds ("Must be a value from 1 to 35").
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the wire harness this target shares (`tests/all/cluster_lifecycle.rs`).

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{let_the_clock_catch_up, opt_text, text, texts, Docdb, REGION};

/// `VpcSecurityGroupMembership$Status` for a security group attached to a live cluster.
const ACTIVE: &str = "active";

/// The inner XML of the first element named `tag`, or `None` when the response carries
/// none.
///
/// Needed because the members under test share element NAMES with members elsewhere in the
/// envelope — a membership's `<Status>` is also the cluster's own `<Status>` — so a
/// membership is read out of the `VpcSecurityGroups` element rather than out of the whole
/// document. Same reader discipline as the shared harness's `texts`: an empty element
/// carries no inner XML, and no element nests another of its own name.
fn inner(xml: &str, tag: &str) -> Option<String> {
    let needle = format!("<{tag}");
    let close = format!("</{tag}>");
    let at = xml.find(needle.as_str())?;
    let after_name = &xml[at + needle.len()..];
    let gt = after_name
        .find('>')
        .unwrap_or_else(|| panic!("unterminated <{tag} in {xml}"));
    if after_name[..gt].ends_with('/') {
        return Some(String::new());
    }
    let body = &after_name[gt + 1..];
    let end = body
        .find(close.as_str())
        .unwrap_or_else(|| panic!("unclosed <{tag}> in {xml}"));
    Some(body[..end].to_string())
}

/// The cluster's VPC security group memberships, as (id, status) pairs in document order.
fn memberships(xml: &str) -> Vec<(String, String)> {
    let Some(block) = inner(xml, "VpcSecurityGroups") else {
        return Vec::new();
    };
    texts(&block, "VpcSecurityGroupId")
        .into_iter()
        .zip(texts(&block, "Status"))
        .collect()
}

/// Assert that `xml` reports exactly `expected` as the cluster's memberships, each active.
fn assert_memberships(case: &str, xml: &str, expected: &[&str]) {
    let actual = memberships(xml);
    let wanted: Vec<(String, String)> = expected
        .iter()
        .map(|id| ((*id).to_string(), ACTIVE.to_string()))
        .collect();
    assert_eq!(
        actual, wanted,
        "case {case:?}: expected one active membership per requested security group \
         ({expected:?}), got {actual:?} in {xml}"
    );
}

/// One row of the security-group table: the ids the create names, under a case name.
struct GroupCase {
    label: &'static str,
    identifier: &'static str,
    groups: &'static [&'static str],
}

/// The table: clusters differing in HOW MANY security groups they belong to and in the FORM
/// of the ids, which is what the rule generalises over. The ids are opaque to this service
/// (`VpcSecurityGroupIds` constrains nothing), so the rows span the shapes a caller sends.
const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        label: "no security group at all",
        identifier: "sg-none",
        groups: &[],
    },
    GroupCase {
        label: "one security group",
        identifier: "sg-one",
        groups: &["sg-0123456789abcdef0"],
    },
    GroupCase {
        label: "three security groups, in the order requested",
        identifier: "sg-three",
        groups: &["sg-ccc33333", "sg-aaa11111", "sg-bbb22222"],
    },
    GroupCase {
        label: "ids of other forms: short, long and mixed case",
        identifier: "sg-forms",
        groups: &["sg-1", "SG-MixedCase0001", "sg-00000000000000000000000000000001"],
    },
];

/// The form parameters that name `groups` as `VpcSecurityGroupIds`.
fn group_params(groups: &[&str]) -> Vec<(String, String)> {
    groups
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                format!("VpcSecurityGroupIds.VpcSecurityGroupId.{}", index + 1),
                (*id).to_string(),
            )
        })
        .collect()
}

/// Borrow an owned parameter list as the harness's `&[(&str, &str)]`.
fn as_params(owned: &[(String, String)]) -> Vec<(&str, &str)> {
    owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

#[tokio::test]
async fn every_requested_security_group_comes_back_as_an_active_membership() {
    let docdb = Docdb::new();

    for case in GROUP_CASES {
        let owned = group_params(case.groups);
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", case.identifier),
            ("Engine", "docdb"),
        ];
        params.extend(as_params(&owned));

        // 1. The create envelope echoes the membership it just stored.
        let created = docdb.ok(case.label, "CreateDBCluster", &params).await;
        assert_memberships(&format!("{} / create", case.label), &created, case.groups);

        let_the_clock_catch_up();

        // 2. The describe envelope reads it back out of state.
        let described = docdb
            .ok(
                case.label,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", case.identifier)],
            )
            .await;
        assert_memberships(
            &format!("{} / describe", case.label),
            &described,
            case.groups,
        );

        // 3. A modify that changes something else leaves the membership alone.
        let modified = docdb
            .ok(
                case.label,
                "ModifyDBCluster",
                &[
                    ("DBClusterIdentifier", case.identifier),
                    ("BackupRetentionPeriod", "7"),
                ],
            )
            .await;
        assert_memberships(&format!("{} / modify", case.label), &modified, case.groups);

        // 4. And the delete envelope, the last path that reports the cluster.
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
        assert_memberships(&format!("{} / delete", case.label), &deleted, case.groups);
    }
}

#[tokio::test]
async fn a_modify_replaces_the_membership_list_and_the_describe_reads_the_new_one() {
    let docdb = Docdb::new();
    let first = group_params(&["sg-aaa11111", "sg-bbb22222"]);
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", "sg-replaced"),
        ("Engine", "docdb"),
    ];
    params.extend(as_params(&first));
    docdb.ok("created", "CreateDBCluster", &params).await;
    let_the_clock_catch_up();

    // Each row replaces the WHOLE list, as `ModifyDBClusterMessage$VpcSecurityGroupIds`
    // ("A list of VPC security groups that the cluster will belong to") does.
    for (case, groups) in [
        ("shrink to one", &["sg-ccc33333"][..]),
        ("grow to three", &["sg-ddd44444", "sg-eee55555", "sg-fff66666"][..]),
        ("replace with the original two", &["sg-aaa11111", "sg-bbb22222"][..]),
    ] {
        let owned = group_params(groups);
        let mut params: Vec<(&str, &str)> = vec![("DBClusterIdentifier", "sg-replaced")];
        params.extend(as_params(&owned));
        let modified = docdb.ok(case, "ModifyDBCluster", &params).await;
        assert_memberships(&format!("{case} / modify"), &modified, groups);

        let described = docdb
            .ok(
                case,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", "sg-replaced")],
            )
            .await;
        assert_memberships(&format!("{case} / describe"), &described, groups);
    }
}

#[tokio::test]
async fn a_member_instance_belongs_to_its_cluster_s_security_groups_and_hosted_zone() {
    let docdb = Docdb::new();
    let groups = ["sg-aaa11111", "sg-bbb22222"];
    let owned = group_params(&groups);
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", "sg-host"),
        ("Engine", "docdb"),
    ];
    params.extend(as_params(&owned));
    docdb.ok("host cluster", "CreateDBCluster", &params).await;

    let created_instance = docdb
        .ok(
            "member instance",
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "sg-host-member"),
                ("DBInstanceClass", "db.r5.large"),
                ("Engine", "docdb"),
                ("DBClusterIdentifier", "sg-host"),
            ],
        )
        .await;
    assert_memberships("member instance / create", &created_instance, &groups);

    let_the_clock_catch_up();

    let described = docdb
        .ok(
            "member instance",
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "sg-host-member")],
        )
        .await;
    assert_memberships("member instance / describe", &described, &groups);
    // The instance endpoint sits in the same Route 53 hosted zone as its cluster's.
    let endpoint = inner(&described, "Endpoint").expect("the instance reports an Endpoint");
    assert_eq!(
        opt_text(&endpoint, "HostedZoneId").as_deref(),
        Some(state::hosted_zone_id(REGION).as_str()),
        "case \"member instance / describe\": the instance endpoint must sit in the \
         Region's hosted zone, got {described}"
    );
}

#[tokio::test]
async fn every_cluster_in_the_region_reports_the_one_regional_hosted_zone() {
    let docdb = Docdb::new();
    // The id the service derives for this Region; the projection must report the stored
    // one, so the test reads it from the crate rather than restating a literal.
    let expected = state::hosted_zone_id(REGION);
    // `Z` plus thirteen uppercase alphanumerics, the form of the fourteen-character zone
    // ids AWS publishes for its endpoint hosted zones.
    let suffix: Vec<char> = expected.chars().skip(1).collect();
    assert!(
        expected.starts_with('Z')
            && suffix.len() == 13
            && suffix
                .iter()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit()),
        "the Region's hosted zone id must be a Z-prefixed, 14-character, uppercase \
         alphanumeric id, got {expected:?}"
    );

    for (case, identifier, extra) in [
        ("the bare minimum", "zone-minimal", Vec::new()),
        (
            "with security groups",
            "zone-groups",
            vec![(
                "VpcSecurityGroupIds.VpcSecurityGroupId.1",
                "sg-0123456789abcdef0",
            )],
        ),
        (
            "encrypted, on a chosen port",
            "zone-encrypted",
            vec![("StorageEncrypted", "true"), ("Port", "27018")],
        ),
    ] {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", identifier), ("Engine", "docdb")];
        params.extend(extra);
        let created = docdb.ok(case, "CreateDBCluster", &params).await;
        assert_eq!(
            opt_text(&created, "HostedZoneId").as_deref(),
            Some(expected.as_str()),
            "case {case:?} / create: expected the Region's hosted zone, got {created}"
        );
    }

    let_the_clock_catch_up();

    // Every cluster in the page reports the same id, and reads report it again: the id is
    // per-Region and stable, not per-call.
    let page = docdb.ok("whole page", "DescribeDBClusters", &[]).await;
    let zones = texts(&page, "HostedZoneId");
    assert_eq!(
        zones.len(),
        3,
        "case \"whole page\": every listed cluster must carry one HostedZoneId, got {page}"
    );
    for (index, zone) in zones.iter().enumerate() {
        assert_eq!(
            zone, &expected,
            "case \"whole page\" (cluster {index}): expected {expected}, got {zone:?} in {page}"
        );
    }
    let again = docdb.ok("whole page again", "DescribeDBClusters", &[]).await;
    assert_eq!(
        texts(&again, "HostedZoneId"),
        zones,
        "case \"whole page again\": the hosted zone id must not change between reads, \
         got {again}"
    );
}

/// Parse one of the envelope's timestamps, which the awsQuery serializer writes in the
/// ISO-8601 date-time form (`RestoreDBClusterToPointInTimeMessage$RestoreToTime`:
/// "Example: 2015-03-07T23:45:00Z").
fn instant(case: &str, xml: &str, tag: &str) -> f64 {
    let raw = text(xml, tag);
    DateTime::from_str(&raw, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: <{tag}> must be an ISO-8601 instant, got {raw:?}: {error}")
        })
        .as_secs_f64()
}

#[tokio::test]
async fn a_provisioned_cluster_reports_the_window_it_can_be_restored_inside() {
    let docdb = Docdb::new();

    // `BackupRetentionPeriod`: "Default: 1 Constraints: Must be a value from 1 to 35." The
    // rows span the published range, both bounds included, plus the absent-input default.
    for (case, identifier, retention) in [
        ("the default retention", "window-default", None),
        ("the minimum retention", "window-min", Some("1")),
        ("a retention inside the range", "window-mid", Some("7")),
        ("the maximum retention", "window-max", Some("35")),
    ] {
        let mut params: Vec<(&str, &str)> =
            vec![("DBClusterIdentifier", identifier), ("Engine", "docdb")];
        if let Some(retention) = retention {
            params.push(("BackupRetentionPeriod", retention));
        }

        // A cluster that is still provisioning has no continuous backup yet, so it reports
        // no window at all.
        let created = docdb.ok(case, "CreateDBCluster", &params).await;
        assert_eq!(
            text(&created, "Status"),
            "creating",
            "case {case:?}: a fresh cluster is still provisioning, got {created}"
        );
        for tag in ["EarliestRestorableTime", "LatestRestorableTime"] {
            assert_eq!(
                opt_text(&created, tag),
                None,
                "case {case:?}: a cluster that is still provisioning has nothing to \
                 restore from, so it must carry no {tag}, got {created}"
            );
        }

        let_the_clock_catch_up();

        let described = docdb
            .ok(
                case,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier)],
            )
            .await;
        assert_eq!(
            text(&described, "Status"),
            "available",
            "case {case:?}: the clock owes the cluster its available status, got {described}"
        );
        let created_at = instant(case, &described, "ClusterCreateTime");
        let earliest = instant(case, &described, "EarliestRestorableTime");
        let latest = instant(case, &described, "LatestRestorableTime");
        // The retained window reaches back `BackupRetentionPeriod` days, but never before
        // the cluster itself existed — and this cluster was created moments ago, so the
        // earliest restorable instant is its creation instant, whatever the retention.
        assert!(
            (earliest - created_at).abs() < 1.0,
            "case {case:?}: expected the earliest restorable instant at the cluster's \
             creation instant {created_at}, got {earliest} in {described}"
        );
        // Continuous backup covers everything up to the read, so the window's upper end is
        // at or after its lower end and no older than the cluster.
        assert!(
            latest >= earliest && latest >= created_at,
            "case {case:?}: expected a window that ends at or after it starts \
             (earliest {earliest}, created {created_at}), got {latest} in {described}"
        );
        assert!(
            latest - earliest < 60.0,
            "case {case:?}: expected the window of a cluster created moments ago to end \
             at about now, got {} seconds of window in {described}",
            latest - earliest
        );

        // The upper end tracks the clock: a later read never reports an older one.
        let_the_clock_catch_up();
        let again = docdb
            .ok(
                case,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier)],
            )
            .await;
        let later = instant(case, &again, "LatestRestorableTime");
        assert!(
            later >= latest,
            "case {case:?}: the latest restorable instant must never move backwards, \
             got {later} after {latest} in {again}"
        );
        assert!(
            (instant(case, &again, "EarliestRestorableTime") - earliest).abs() < 1.0,
            "case {case:?}: the window's lower end is the cluster's creation instant and \
             must not move, got {again}"
        );
    }
}
