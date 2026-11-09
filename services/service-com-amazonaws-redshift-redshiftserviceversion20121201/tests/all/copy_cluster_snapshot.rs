// SPDX-License-Identifier: CC0-1.0

//! Copying a cluster snapshot into a new manual snapshot, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: `CopyClusterSnapshot`
//! mints a new `manual` snapshot that carries the SOURCE snapshot's
//! data-describing members, reported in the `creating` status the clock moves to
//! `available`, under the manual retention period the request names or the source
//! cluster's own default; and it copies ONLY a source that is a snapshot Amazon
//! Redshift took on its own schedule and that is `available`, refusing anything
//! else without leaving a record behind under the target identifier.
//!
//! It is asserted over a TABLE of retention periods (the documented sentinel,
//! both published bounds, a value inside the range, and the member left off
//! against two clusters with different defaults), over a table of sources the
//! rule excludes (a snapshot a create took, a snapshot an earlier copy took, a
//! snapshot still being taken) and over a table of refusals (an unknown source,
//! an empty source identifier, a source still being taken, a target already in
//! use, four target identifiers outside the documented syntax, each published
//! retention bound one past it, a source cluster the account does not hold and a
//! cluster the source was not taken of).

use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};
use crate::modify_cluster_snapshot::assert_retention;

/// The cluster every case takes its snapshots of.
const CLUSTER: &str = "copy-source-host";

/// A second cluster, created with a manual retention period of its own, so a
/// copy that names no period can be seen to inherit the SOURCE cluster's default
/// rather than one constant.
const CLUSTER_WITH_DEFAULT: &str = "copy-retention-host";

/// The manual retention period [`CLUSTER_WITH_DEFAULT`] is created with — a value
/// inside the published range and not the service-wide sentinel.
const CLUSTER_DEFAULT_RETENTION: i32 = 7;

/// One row of the refusal table: the case name, the request parameters, and the
/// `(status, error code)` the service must answer with.
type RefusalCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// The members a copy carries over from its source, because each of them
/// describes the cluster DATA the snapshot holds rather than the new snapshot's
/// own identity.
const CARRIED_MEMBERS: [&str; 9] = [
    "ClusterIdentifier",
    "NodeType",
    "NumberOfNodes",
    "MasterUsername",
    "DBName",
    "ClusterVersion",
    "AvailabilityZone",
    "Encrypted",
    "ClusterCreateTime",
];

/// An emulator holding both clusters every case draws its snapshots from.
async fn seeded_emulator() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_minimal(CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {CLUSTER}: expected 200, got {} with body {}",
        created.status, created.body
    );
    let retention = CLUSTER_DEFAULT_RETENTION.to_string();
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", CLUSTER_WITH_DEFAULT),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("ManualSnapshotRetentionPeriod", &retention),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {CLUSTER_WITH_DEFAULT}: expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
}

/// Write a snapshot Amazon Redshift took on its OWN schedule straight into the
/// shared store, taken of `cluster` and in `status`.
///
/// Nothing in this slice mints an `automated` record — `CreateClusterSnapshot` is
/// documented to produce a `manual` one — so seeding the same store the handlers
/// read is the only way to give a copy a source at all. The record itself is
/// built by the service's own [`state::snapshot_of`], so a seeded source carries
/// exactly the members a snapshot of that cluster carries.
///
/// `pub(crate)` so the sibling batch-delete suite seeds such a record through
/// this one helper rather than writing a second copy of it.
pub(crate) fn seed_automated(
    emulator: &Emulator,
    cluster: &str,
    identifier: &str,
    status: &str,
) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    let stored_cluster = store
        .cluster(cluster)
        .cloned()
        .expect("the seeded cluster is in the store");
    let created_at = aws_smithy_types::DateTime::from(SystemTime::now());
    let mut snapshot = state::snapshot_of(
        &stored_cluster,
        identifier,
        created_at,
        state::MANUAL_RETENTION_INDEFINITE,
        Vec::new(),
        REGION,
    );
    snapshot.snapshot_type = Some(state::SNAPSHOT_TYPE_AUTOMATED.to_string());
    snapshot.status = Some(status.to_string());
    // How long Amazon Redshift keeps a snapshot it took itself is the cluster's
    // automated retention period, so the record carries no manual one.
    snapshot.manual_snapshot_retention_period = None;
    snapshot.snapshot_retention_start_time = None;
    snapshot.manual_snapshot_remaining_days = None;
    store.snapshots.push(state::StoredSnapshot {
        snapshot,
        // A record seeded as `creating` must stay there for the case that is
        // about it, so its transition is not due inside the test's lifetime; one
        // seeded `available` needs no transition at all.
        available_at: if status == state::STATUS_CREATING {
            Instant::now() + Duration::from_secs(3_600)
        } else {
            Instant::now()
        },
        source: Some(stored_cluster),
        // A seeded record holds no copy of the cluster's databases; what a copy
        // of a snapshot carries over is asserted on the stored records.
        data: Default::default(),
    });
}

/// `CopyClusterSnapshot` with these parameters.
async fn copy(emulator: &Emulator, params: &[(&str, &str)]) -> Answer {
    emulator.call("CopyClusterSnapshot", params).await
}

/// The snapshot `identifier` as `DescribeClusterSnapshots` reports it.
async fn describe(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "DescribeClusterSnapshots",
            &[("SnapshotIdentifier", identifier)],
        )
        .await
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Assert a refused copy materialised NOTHING: the target identifier names no
/// snapshot, which the read reports as the modelled "not found" fault.
async fn assert_no_target(case: &str, emulator: &Emulator, target: &str) {
    let read = describe(emulator, target).await;
    assert_refused(
        &format!("{case} (the target must not exist)"),
        &read,
        404,
        "ClusterSnapshotNotFound",
    );
}

/// An `available` automated snapshot is copied into a new `manual` one that
/// carries the source's data, under every retention period the service admits.
#[tokio::test]
async fn an_available_automated_snapshot_is_copied_into_a_manual_snapshot() {
    let emulator = seeded_emulator().await;
    let (lower, upper) = state::MANUAL_RETENTION_BOUNDS;
    let indefinite = state::MANUAL_RETENTION_INDEFINITE;

    // The rows differ along the retention period the rule generalises over — the
    // documented sentinel, both published bounds, a value inside the range — and
    // along the SOURCE CLUSTER for the two rows that name no period at all, so
    // the inherited default cannot be one constant.
    let rows: Vec<(&str, &str, Option<i32>, i32)> = vec![
        ("no period named, a cluster default of indefinitely", CLUSTER, None, indefinite),
        (
            "no period named, a cluster default inside the range",
            CLUSTER_WITH_DEFAULT,
            None,
            CLUSTER_DEFAULT_RETENTION,
        ),
        ("the documented indefinite sentinel", CLUSTER, Some(indefinite), indefinite),
        ("the documented lower bound", CLUSTER, Some(lower), lower),
        ("the documented upper bound", CLUSTER, Some(upper), upper),
        ("a period inside the published range", CLUSTER, Some(30), 30),
    ];

    let mut copied: Vec<(&str, String)> = Vec::new();
    for (index, (case, cluster, requested, expected)) in rows.iter().enumerate() {
        let source = format!("auto-source-{index}");
        let target = format!("manual-copy-{index}");
        seed_automated(&emulator, cluster, &source, state::STATUS_AVAILABLE);
        let before = describe(&emulator, &source).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected the seeded source to be readable, got {} with body {}",
            before.status, before.body
        );

        let mut params: Vec<(&str, &str)> = vec![
            ("SourceSnapshotIdentifier", source.as_str()),
            ("TargetSnapshotIdentifier", target.as_str()),
        ];
        let period = requested.map(|value| value.to_string());
        if let Some(period) = period.as_deref() {
            params.push(("ManualSnapshotRetentionPeriod", period));
        }
        let answer = copy(&emulator, &params).await;

        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("SnapshotIdentifier").as_deref(),
            Some(target.as_str()),
            "case {case}: expected the new snapshot {target}, got {:?}",
            answer.text("SnapshotIdentifier")
        );
        // "Snapshots created using CreateClusterSnapshot and CopyClusterSnapshot
        // are of type \"manual\"".
        assert_eq!(
            answer.text("SnapshotType").as_deref(),
            Some(state::SNAPSHOT_TYPE_MANUAL),
            "case {case}: expected a {} snapshot, got {:?}",
            state::SNAPSHOT_TYPE_MANUAL,
            answer.text("SnapshotType")
        );
        // "CreateClusterSnapshot and CopyClusterSnapshot returns status as
        // \"creating\"".
        assert_eq!(
            answer.text("Status").as_deref(),
            Some(state::STATUS_CREATING),
            "case {case}: expected the copy to report {}, got {:?}",
            state::STATUS_CREATING,
            answer.text("Status")
        );
        assert_retention(case, &answer, *expected);
        // The ARN names the new snapshot under the cluster the data came from.
        assert_eq!(
            answer.text("SnapshotArn").as_deref(),
            Some(state::snapshot_arn(cluster, &target, REGION).as_str()),
            "case {case}: expected the ARN of the new snapshot, got {:?}",
            answer.text("SnapshotArn")
        );
        // Every member that describes the DATA is the source's.
        for member in CARRIED_MEMBERS {
            assert_eq!(
                answer.text(member),
                before.text(member),
                "case {case}: expected {member} carried over from the source as {:?}, got {:?}",
                before.text(member),
                answer.text(member)
            );
        }
        // Copying reads the source; it does not change it.
        let after = describe(&emulator, &source).await;
        assert_eq!(
            after.text("SnapshotType").as_deref(),
            Some(state::SNAPSHOT_TYPE_AUTOMATED),
            "case {case}: expected the source to stay {}, got {:?}",
            state::SNAPSHOT_TYPE_AUTOMATED,
            after.text("SnapshotType")
        );
        assert_eq!(
            after.text("Status").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the source to stay {}, got {:?}",
            state::STATUS_AVAILABLE,
            after.text("Status")
        );
        copied.push((case, target));
    }

    // The `creating` -> `available` transition is due a span after the call that
    // minted each copy, and nothing sleeps for it inside the handler.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    for (case, target) in &copied {
        let read = describe(&emulator, target).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the copy to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.text("Status").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case}: expected the copy to become {}, got {:?}",
            state::STATUS_AVAILABLE,
            read.text("Status")
        );
        assert_eq!(
            read.text("SnapshotType").as_deref(),
            Some(state::SNAPSHOT_TYPE_MANUAL),
            "case {case}: expected the persisted copy to stay {}, got {:?}",
            state::SNAPSHOT_TYPE_MANUAL,
            read.text("SnapshotType")
        );
    }
}

/// A source that is not a snapshot Amazon Redshift took on its own schedule is
/// refused with the fixed `InvalidParameterValue` message, and nothing is minted
/// under the target identifier.
#[tokio::test]
async fn a_source_that_is_not_an_automated_snapshot_is_refused() {
    let emulator = seeded_emulator().await;

    // A manual snapshot a create took.
    let taken = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "manual-by-create"),
                ("ClusterIdentifier", CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "case seed a manual snapshot: expected 200, got {} with body {}",
        taken.status, taken.body
    );
    // A manual snapshot an earlier COPY took: the copy of an automated snapshot
    // is itself manual, so it cannot be copied again.
    seed_automated(&emulator, CLUSTER, "auto-for-chain", state::STATUS_AVAILABLE);
    let first = copy(
        &emulator,
        &[
            ("SourceSnapshotIdentifier", "auto-for-chain"),
            ("TargetSnapshotIdentifier", "manual-by-copy"),
        ],
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case seed a copied snapshot: expected 200, got {} with body {}",
        first.status, first.body
    );
    // A manual snapshot still being taken: the source's KIND is what this rule is
    // about, whatever state it is in.
    let creating = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "manual-still-creating"),
                ("ClusterIdentifier", CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        creating.status, 200,
        "case seed a creating manual snapshot: expected 200, got {} with body {}",
        creating.status, creating.body
    );

    // The rows differ along HOW the manual source came to be, and along the state
    // it is in.
    let rows: [(&str, &str); 3] = [
        ("a manual snapshot a create took", "manual-by-create"),
        ("a manual snapshot an earlier copy took", "manual-by-copy"),
        ("a manual snapshot still being taken", "manual-still-creating"),
    ];

    for (index, (case, source)) in rows.iter().enumerate() {
        let target = format!("refused-copy-{index}");
        let answer = copy(
            &emulator,
            &[
                ("SourceSnapshotIdentifier", source),
                ("TargetSnapshotIdentifier", target.as_str()),
            ],
        )
        .await;
        assert_refused(case, &answer, 400, "InvalidParameterValue");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some("Only automated snapshots can be copied."),
            "case {case}: expected the fixed refusal message, got {:?} with body {}",
            answer.text("Message"),
            answer.body
        );
        assert_no_target(case, &emulator, &target).await;
    }
}

/// Every other refusal the rule requires, each leaving nothing behind under its
/// target identifier.
#[tokio::test]
async fn the_sources_and_targets_the_rule_excludes_are_refused() {
    let emulator = seeded_emulator().await;
    seed_automated(&emulator, CLUSTER, "auto-ready", state::STATUS_AVAILABLE);
    seed_automated(&emulator, CLUSTER, "auto-creating", state::STATUS_CREATING);
    let taken = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "target-in-use"),
                ("ClusterIdentifier", CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "case seed the snapshot holding the target identifier: expected 200, got {} with body {}",
        taken.status, taken.body
    );

    let (lower, upper) = state::MANUAL_RETENTION_BOUNDS;
    let below = (lower - 1).to_string();
    let above = (upper + 1).to_string();

    // Each row names the source, the target, any extra parameter, and the
    // `(status, code)` the service must answer with. The rows differ along WHICH
    // part of the rule the request breaks.
    let rows: Vec<RefusalCase> = vec![
        (
            "a source snapshot that does not exist",
            vec![
                ("SourceSnapshotIdentifier", "auto-missing"),
                ("TargetSnapshotIdentifier", "from-missing"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "an empty source identifier",
            vec![
                ("SourceSnapshotIdentifier", ""),
                ("TargetSnapshotIdentifier", "from-empty-source"),
            ],
            400,
            "ValidationException",
        ),
        (
            "an automated source still being taken",
            vec![
                ("SourceSnapshotIdentifier", "auto-creating"),
                ("TargetSnapshotIdentifier", "from-creating"),
            ],
            400,
            "InvalidClusterSnapshotState",
        ),
        (
            "a target identifier already in use",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", "target-in-use"),
            ],
            400,
            "ClusterSnapshotAlreadyExists",
        ),
        (
            "a target identifier whose first character is not a letter",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", "9copy"),
            ],
            400,
            "ValidationException",
        ),
        (
            "a target identifier ending in a hyphen",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", "copy-"),
            ],
            400,
            "ValidationException",
        ),
        (
            "a target identifier with two consecutive hyphens",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", "co--py"),
            ],
            400,
            "ValidationException",
        ),
        (
            "an empty target identifier",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", ""),
            ],
            400,
            "ValidationException",
        ),
        (
            "a retention period one day below the published lower bound",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", "from-short-retention"),
                ("ManualSnapshotRetentionPeriod", below.as_str()),
            ],
            400,
            "InvalidRetentionPeriodFault",
        ),
        (
            "a retention period one day above the published upper bound",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("TargetSnapshotIdentifier", "from-long-retention"),
                ("ManualSnapshotRetentionPeriod", above.as_str()),
            ],
            400,
            "InvalidRetentionPeriodFault",
        ),
        (
            "a source cluster the account holds no cluster under",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("SourceSnapshotClusterIdentifier", "no-such-host"),
                ("TargetSnapshotIdentifier", "from-unknown-cluster"),
            ],
            404,
            "ClusterNotFound",
        ),
        (
            "a source cluster the snapshot was not taken of",
            vec![
                ("SourceSnapshotIdentifier", "auto-ready"),
                ("SourceSnapshotClusterIdentifier", CLUSTER_WITH_DEFAULT),
                ("TargetSnapshotIdentifier", "from-other-cluster"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
    ];

    for (case, params, status, code) in rows {
        let answer = copy(&emulator, &params).await;
        assert_refused(case, &answer, status, code);
        let target = params
            .iter()
            .find(|(name, _)| *name == "TargetSnapshotIdentifier")
            .map(|(_, value)| *value)
            .expect("every row names a target identifier");
        if target != "target-in-use" && !target.is_empty() {
            assert_no_target(case, &emulator, target).await;
        }
    }
}

/// The source cluster a request names is honoured when it IS the cluster the
/// source snapshot was taken of, so the pair names the one snapshot.
#[tokio::test]
async fn a_source_named_by_its_cluster_and_identifier_is_copied() {
    let emulator = seeded_emulator().await;
    seed_automated(
        &emulator,
        CLUSTER_WITH_DEFAULT,
        "auto-paired",
        state::STATUS_AVAILABLE,
    );

    let case = "the source named by both its cluster and its identifier";
    let answer = copy(
        &emulator,
        &[
            ("SourceSnapshotIdentifier", "auto-paired"),
            ("SourceSnapshotClusterIdentifier", CLUSTER_WITH_DEFAULT),
            ("TargetSnapshotIdentifier", "paired-copy"),
        ],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.text("ClusterIdentifier").as_deref(),
        Some(CLUSTER_WITH_DEFAULT),
        "case {case}: expected the copy to report the source's cluster, got {:?}",
        answer.text("ClusterIdentifier")
    );
    assert_retention(case, &answer, CLUSTER_DEFAULT_RETENTION);
}
