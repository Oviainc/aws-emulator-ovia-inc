// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CopySnapshot`: "makes a copy of an existing snapshot".
//!
//! Every assertion goes over the wire through the same [`Emulator`] harness the
//! cluster and snapshot suites use: an `awsQuery` form-urlencoded request is
//! dispatched through the production router and only the HTTP status and the XML
//! body are inspected. Nothing reaches into the service's state struct, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The rule under test: a copy is a NEW snapshot whose captured content — the
//! source cluster it names, that cluster's configuration as the source snapshot
//! holds it, and the per-node snapshots with the instant they were taken at —
//! is the SOURCE snapshot's, carried over unchanged; only the copy's own
//! identity (`SnapshotName`, `ARN`), its fresh `creating` status, its `manual`
//! provenance and the members the copy request carries (`KmsKeyId`, `Tags`)
//! are the copy's own. The tests are tables whose rows differ along the source
//! snapshot's content and the copy's own members, never a single case.

use crate::cache_cluster_lifecycle::create_available_cluster;
use crate::harness::{params, Emulator, RefusalCase, Reply, PAST_TRANSITION};
use crate::snapshots::{create_snapshot, describe_snapshots, reported_names};

/// Every `Snapshot` member that describes WHAT the snapshot captured, so a copy
/// has to report exactly what its source reports. `KmsKeyId` is deliberately
/// absent: `CopySnapshot$KmsKeyId` is "the ID of the KMS key used to encrypt the
/// target snapshot", so it is the copy's own and is asserted per row.
const SAME_AS_SOURCE: [&str; 11] = [
    "Snapshot/CacheClusterId",
    "Snapshot/Engine",
    "Snapshot/EngineVersion",
    "Snapshot/CacheNodeType",
    "Snapshot/NumCacheNodes",
    "Snapshot/Port",
    "Snapshot/CacheParameterGroupName",
    "Snapshot/PreferredMaintenanceWindow",
    "Snapshot/PreferredAvailabilityZone",
    "Snapshot/CacheClusterCreateTime",
    "Snapshot/SnapshotRetentionLimit",
];

/// The per-node members a copy carries over: the copy holds the same
/// point-in-time data, so copying does not retake the snapshot and does not
/// re-stamp the instant it was taken at.
const NODE_SAME_AS_SOURCE: [&str; 4] = [
    "NodeSnapshot/CacheClusterId",
    "NodeSnapshot/CacheNodeId",
    "NodeSnapshot/CacheNodeCreateTime",
    "NodeSnapshot/SnapshotCreateTime",
];

/// Copy `source` to `target`, with whatever else `extra` names.
async fn copy_snapshot(
    emulator: &Emulator,
    source: &str,
    target: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[
        ("SourceSnapshotName", source),
        ("TargetSnapshotName", target),
    ]);
    pairs.extend(params(extra));
    emulator.call("CopySnapshot", &pairs).await
}

/// One row of the copy table: the source cluster and snapshot to copy, and the
/// copy's own members.
#[derive(Debug, Clone)]
struct CopyCase {
    /// The case name.
    case: &'static str,
    cluster: &'static str,
    /// The form pairs the source cluster is created with.
    cluster_extra: &'static [(&'static str, &'static str)],
    source_name: &'static str,
    /// The form pairs the source snapshot is taken with.
    source_extra: &'static [(&'static str, &'static str)],
    /// The `TargetSnapshotName` the copy is asked for.
    requested_target: &'static str,
    /// The identity the copy is stored and reported under: the name "is stored
    /// as a lowercase string".
    stored_target: &'static str,
    /// The form pairs sent beside the two names.
    copy_extra: &'static [(&'static str, &'static str)],
    /// What the copy must report for `KmsKeyId`: the copy's own key when it
    /// named one, otherwise the source snapshot's.
    kms_key_id: Option<&'static str>,
}

#[tokio::test]
async fn a_copy_is_the_source_snapshot_under_a_new_name() {
    // The axis: the source snapshot's content (the source cluster's engine and
    // its documented defaults, its port, its backup settings, the source
    // snapshot's own KMS key) and the copy's own members (target name case, KMS
    // key, tags).
    let cases = [
        CopyCase {
            case: "a redis snapshot with the documented defaults",
            cluster: "copy-redis",
            cluster_extra: &[("CacheNodeType", "cache.t3.micro")],
            source_name: "redis-origin",
            source_extra: &[],
            requested_target: "redis-copy",
            stored_target: "redis-copy",
            copy_extra: &[],
            kms_key_id: None,
        },
        CopyCase {
            case: "a valkey snapshot with backups configured, copied under a mixed-case name",
            cluster: "copy-valkey",
            cluster_extra: &[
                ("Engine", "valkey"),
                ("Port", "7000"),
                ("SnapshotRetentionLimit", "5"),
                ("SnapshotWindow", "05:00-09:00"),
            ],
            source_name: "valkey-origin",
            source_extra: &[("KmsKeyId", "alias/source-key")],
            requested_target: "Valkey-COPY",
            stored_target: "valkey-copy",
            copy_extra: &[("KmsKeyId", "alias/target-key")],
            // The copy named its own key, so that is the one it reports.
            kms_key_id: Some("alias/target-key"),
        },
        CopyCase {
            case: "a redis 6.2 snapshot whose key the copy does not name",
            cluster: "copy-redis-six",
            cluster_extra: &[("EngineVersion", "6.2"), ("SnapshotRetentionLimit", "1")],
            source_name: "redis-six-origin",
            source_extra: &[("KmsKeyId", "alias/kept-key")],
            requested_target: "20260101-copy-2",
            stored_target: "20260101-copy-2",
            copy_extra: &[
                ("Tags.Tag.1.Key", "team"),
                ("Tags.Tag.1.Value", "cache"),
                ("Tags.Tag.2.Key", "purpose"),
                ("Tags.Tag.2.Value", "retention"),
            ],
            // No key on the copy, so the source snapshot's travels with it.
            kms_key_id: Some("alias/kept-key"),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        create_available_cluster(&emulator, case.cluster, case.cluster_extra).await;
        let created = create_snapshot(
            &emulator,
            case.source_name,
            case.cluster,
            case.source_extra,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: the source snapshot must be created, got {} {}",
            case.case, created.status, created.body
        );
        // The source snapshot can only be copied once the clock owes it
        // `available`.
        std::thread::sleep(PAST_TRANSITION);
        let source = describe_snapshots(&emulator, &[("SnapshotName", case.source_name)]).await;
        assert_eq!(
            source.text("Snapshot/SnapshotStatus"),
            Some("available"),
            "case {}: the source must be available before it is copied, got {}",
            case.case,
            source.body
        );

        let copied = copy_snapshot(
            &emulator,
            case.source_name,
            case.requested_target,
            case.copy_extra,
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {}: expected HTTP 200, got {} {}",
            case.case, copied.status, copied.body
        );
        // A copy is NOT ready: it answers `creating` first.
        assert_eq!(
            copied.text("Snapshot/SnapshotStatus"),
            Some("creating"),
            "case {}: a just-made copy must report creating, got {}",
            case.case,
            copied.body
        );

        std::thread::sleep(PAST_TRANSITION);
        let described =
            describe_snapshots(&emulator, &[("SnapshotName", case.requested_target)]).await;
        assert_eq!(
            described.status, 200,
            "case {}: the copy must be readable back, got {} {}",
            case.case, described.status, described.body
        );
        assert_eq!(
            described.text("Snapshot/SnapshotStatus"),
            Some("available"),
            "case {}: the copy must reach available on the clock, got {}",
            case.case,
            described.body
        );

        let expected_arn = format!(":snapshot:{}", case.stored_target);
        for (phase, reply) in [("copy", &copied), ("describe", &described)] {
            assert_eq!(
                reply.text("Snapshot/SnapshotName"),
                Some(case.stored_target),
                "case {} ({phase}): expected the stored lowercase target name, got {}",
                case.case,
                reply.body
            );
            assert!(
                reply
                    .text("Snapshot/ARN")
                    .is_some_and(|arn| arn.ends_with(&expected_arn)),
                "case {} ({phase}): expected an ARN ending in {expected_arn}, got {:?} in {}",
                case.case,
                reply.text("Snapshot/ARN"),
                reply.body
            );
            // Whatever the source's provenance, a copy is the caller's own
            // manual snapshot.
            assert_eq!(
                reply.text("Snapshot/SnapshotSource"),
                Some("manual"),
                "case {} ({phase}): a copy must report the manual source, got {}",
                case.case,
                reply.body
            );
            assert_eq!(
                reply.text("Snapshot/KmsKeyId"),
                case.kms_key_id,
                "case {} ({phase}): expected KmsKeyId {:?}, got {}",
                case.case,
                case.kms_key_id,
                reply.body
            );
            // Everything the snapshot captured is the source's, element for
            // element.
            for element in SAME_AS_SOURCE.iter().chain(NODE_SAME_AS_SOURCE.iter()) {
                assert_eq!(
                    reply.all(element),
                    source.all(element),
                    "case {} ({phase}): expected {element} = {:?} (the source's), got {}",
                    case.case,
                    source.all(element),
                    reply.body
                );
            }
            // `SnapshotWindow` is part of the captured content too, and is
            // absent on a cluster that has no daily backup window.
            assert_eq!(
                reply.all("Snapshot/SnapshotWindow"),
                source.all("Snapshot/SnapshotWindow"),
                "case {} ({phase}): expected the source's SnapshotWindow {:?}, got {}",
                case.case,
                source.all("Snapshot/SnapshotWindow"),
                reply.body
            );
        }

        // Both snapshots are stored: a copy adds a record beside its source
        // rather than replacing it.
        let mut expected = vec![case.source_name, case.stored_target];
        expected.sort_unstable();
        let all = describe_snapshots(&emulator, &[]).await;
        assert_eq!(
            reported_names(&all),
            expected,
            "case {}: expected both {expected:?} to be stored, got {}",
            case.case,
            all.body
        );
        // The source itself did not move: copying reads it.
        assert_eq!(
            describe_snapshots(&emulator, &[("SnapshotName", case.source_name)])
                .await
                .text("Snapshot/SnapshotStatus"),
            Some("available"),
            "case {}: copying must leave the source available, got {}",
            case.case,
            all.body
        );
    }
}

#[tokio::test]
async fn a_copy_of_a_copy_still_reports_the_original_point_in_time() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "chained", &[("SnapshotRetentionLimit", "2")]).await;
    let created = create_snapshot(&emulator, "origin-snap", "chained", &[]).await;
    assert_eq!(
        created.status, 200,
        "the origin snapshot must be created, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let origin = describe_snapshots(&emulator, &[("SnapshotName", "origin-snap")]).await;

    // Each link copies the previous one, so the second copy's content came
    // through a copy rather than from the cluster.
    for (case, source, target) in [
        ("first link", "origin-snap", "copy-one"),
        ("second link", "copy-one", "copy-two"),
    ] {
        let copied = copy_snapshot(&emulator, source, target, &[]).await;
        assert_eq!(
            copied.status, 200,
            "case {case}: expected HTTP 200, got {} {}",
            copied.status, copied.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_snapshots(&emulator, &[("SnapshotName", target)]).await;
        for element in SAME_AS_SOURCE.iter().chain(NODE_SAME_AS_SOURCE.iter()) {
            assert_eq!(
                described.all(element),
                origin.all(element),
                "case {case}: expected {element} = {:?} (the origin's), got {}",
                origin.all(element),
                described.body
            );
        }
    }

    let all = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&all),
        vec!["copy-one", "copy-two", "origin-snap"],
        "every link of the chain must be stored, got {}",
        all.body
    );
}

#[tokio::test]
async fn the_documented_target_name_form_is_enforced() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "named-source", &[]).await;
    let created = create_snapshot(&emulator, "name-origin", "named-source", &[]).await;
    assert_eq!(
        created.status, 200,
        "the source snapshot must be created, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);

    // The axis: the target name's form. The documented rule is the character
    // set and the 255-character bound a snapshot name shares
    // (`state::is_valid_snapshot_name`), so the row at the bound is accepted and
    // the row one past it is refused; a hyphen's placement, which no
    // ElastiCache documentation constrains for a snapshot name, is accepted.
    let at_bound = "c".repeat(255);
    let past_bound = "c".repeat(256);
    let cases: [(&str, &str, u16, &str); 7] = [
        ("a plain name", "plain-copy", 200, ""),
        ("mixed case is stored lowercase", "Plain-COPY-2", 200, ""),
        ("digits and hyphens", "20260101-copy", 200, ""),
        ("two consecutive hyphens", "ok--copy", 200, ""),
        ("a trailing hyphen", "ok-copy-", 200, ""),
        (
            "a character that is neither alphanumeric nor a hyphen",
            "bad.copy",
            400,
            "InvalidParameterValue",
        ),
        (
            "one character past the 255-character bound",
            past_bound.as_str(),
            400,
            "InvalidParameterValue",
        ),
    ];

    let mut accepted: Vec<String> = vec!["name-origin".to_string()];
    for (case, target, status, code) in cases
        .into_iter()
        .chain([("at the 255-character bound", at_bound.as_str(), 200u16, "")])
    {
        let reply = copy_snapshot(&emulator, "name-origin", target, &[]).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code:?}, got {}",
            reply.body
        );
        if status == 200 {
            accepted.push(target.to_ascii_lowercase());
        }
    }

    accepted.sort();
    let all = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&all),
        accepted,
        "only the accepted target names may be stored, got {}",
        all.body
    );
}

#[tokio::test]
async fn a_copy_the_rule_forbids_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "refused-source", &[]).await;
    let created = create_snapshot(&emulator, "held-snap", "refused-source", &[]).await;
    assert_eq!(
        created.status, 200,
        "the source snapshot must be created, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let copied = copy_snapshot(&emulator, "held-snap", "taken-copy", &[]).await;
    assert_eq!(
        copied.status, 200,
        "the first copy must succeed, got {} {}",
        copied.status, copied.body
    );
    std::thread::sleep(PAST_TRANSITION);

    // Each row: the form pairs that make the request, and the modelled refusal.
    // The axis is the reason the copy cannot be made.
    let cases: [RefusalCase; 7] = [
        (
            "a source snapshot the store does not hold",
            &[
                ("SourceSnapshotName", "no-such-snap"),
                ("TargetSnapshotName", "fresh-copy"),
            ],
            404,
            "SnapshotNotFoundFault",
        ),
        (
            "an export whose source the store does not hold: the missing resource is named",
            &[
                ("SourceSnapshotName", "no-such-snap"),
                ("TargetSnapshotName", "fresh-export"),
                ("TargetBucket", "my-export-bucket"),
            ],
            404,
            "SnapshotNotFoundFault",
        ),
        (
            "a target name already held",
            &[
                ("SourceSnapshotName", "held-snap"),
                ("TargetSnapshotName", "taken-copy"),
            ],
            400,
            "SnapshotAlreadyExistsFault",
        ),
        (
            "a target name already held, in another case",
            &[
                ("SourceSnapshotName", "held-snap"),
                ("TargetSnapshotName", "TAKEN-COPY"),
            ],
            400,
            "SnapshotAlreadyExistsFault",
        ),
        (
            "a snapshot copied onto its own name",
            &[
                ("SourceSnapshotName", "held-snap"),
                ("TargetSnapshotName", "held-snap"),
            ],
            400,
            "SnapshotAlreadyExistsFault",
        ),
        (
            "an export to Amazon S3, which this emulator cannot perform",
            &[
                ("SourceSnapshotName", "held-snap"),
                ("TargetSnapshotName", "exported-copy"),
                ("TargetBucket", "my-export-bucket"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag with no key",
            &[
                ("SourceSnapshotName", "held-snap"),
                ("TargetSnapshotName", "keyless-tag-copy"),
                ("Tags.Tag.1.Value", "no-key"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, pairs, status, code) in cases {
        let reply = emulator.call("CopySnapshot", &params(pairs)).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code}, got {}",
            reply.body
        );
    }

    // More tags than a resource may carry, which the modelled
    // `TagQuotaPerResourceExceeded` refuses: "the maximum number of tags
    // permitted on a resource is 50". Built rather than tabled, because a row
    // of 51 tags is 102 form pairs.
    let mut over_tagged = params(&[
        ("SourceSnapshotName", "held-snap"),
        ("TargetSnapshotName", "over-tagged-copy"),
    ]);
    for index in 1..=51 {
        over_tagged.push((format!("Tags.Tag.{index}.Key"), format!("key-{index}")));
        over_tagged.push((format!("Tags.Tag.{index}.Value"), format!("value-{index}")));
    }
    let over_tagged = emulator.call("CopySnapshot", &over_tagged).await;
    assert_eq!(
        over_tagged.status, 400,
        "case 51 tags: expected HTTP 400, got {} {}",
        over_tagged.status, over_tagged.body
    );
    assert_eq!(
        over_tagged.error_code(),
        "TagQuotaPerResourceExceeded",
        "case 51 tags: expected TagQuotaPerResourceExceeded, got {}",
        over_tagged.body
    );

    // A source that is not settled yet has no finished snapshot to copy: this
    // row is timed rather than tabled, because it has to run inside the
    // transition span the create opened.
    create_available_cluster(&emulator, "unsettled-source", &[]).await;
    let creating = create_snapshot(&emulator, "creating-snap", "unsettled-source", &[]).await;
    assert_eq!(
        creating.status, 200,
        "case a source still creating: the create must succeed, got {}",
        creating.body
    );
    let too_early = copy_snapshot(&emulator, "creating-snap", "early-copy", &[]).await;
    assert_eq!(
        too_early.status, 400,
        "case a source still creating: expected HTTP 400, got {} {}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code(),
        "InvalidSnapshotState",
        "case a source still creating: expected InvalidSnapshotState, got {}",
        too_early.body
    );
    // ... and once the clock owes it `available`, the same copy is accepted.
    std::thread::sleep(PAST_TRANSITION);
    let in_time = copy_snapshot(&emulator, "creating-snap", "early-copy", &[]).await;
    assert_eq!(
        in_time.status, 200,
        "case a settled source: expected HTTP 200, got {} {}",
        in_time.status, in_time.body
    );

    // Not one refusal added a record: the store holds exactly the snapshots the
    // accepted calls made.
    let all = describe_snapshots(&emulator, &[]).await;
    assert_eq!(
        reported_names(&all),
        vec!["creating-snap", "early-copy", "held-snap", "taken-copy"],
        "a refused copy must persist nothing, got {}",
        all.body
    );
}
