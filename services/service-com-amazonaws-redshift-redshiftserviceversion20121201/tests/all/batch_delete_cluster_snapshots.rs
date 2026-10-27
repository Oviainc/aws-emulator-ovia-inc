// SPDX-License-Identifier: CC0-1.0

//! Deleting a set of cluster snapshots in one call, end to end over the awsQuery
//! wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: `BatchDeleteClusterSnapshots`
//! decides EVERY entry of the list on its own — a manual, `available` snapshot no
//! other account may restore is deleted and reported under `Resources`, and every
//! other entry is reported as its own `Errors` record naming the snapshot, the
//! failure code and the reason, without failing the entries beside it — while a
//! list longer than the published limit is a property of the request and refuses
//! the whole call.
//!
//! It is asserted over a TABLE of entries inside one call (a deletable snapshot,
//! an unknown one, one Amazon Redshift took on its own schedule, one still being
//! taken, one another account may restore, an empty identifier, a snapshot named
//! with a cluster it was not taken of, the same snapshot named with the cluster it
//! WAS taken of, and an entry repeating a snapshot the same call already deleted)
//! and over a table of list LENGTHS, including the published limit and one past
//! it.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::copy_cluster_snapshot::seed_automated;

/// The cluster every case takes its snapshots of.
const CLUSTER: &str = "batch-delete-source";

/// A second cluster, so an entry can name a cluster the snapshot it names was not
/// taken of.
const OTHER_CLUSTER: &str = "batch-delete-other";

/// The account an entry's snapshot is authorized for restore by, in the
/// 12-digit form `AccountWithRestoreAccess` takes.
const GRANTEE_ACCOUNT: &str = "123456789012";

/// One entry of a request: the snapshot it names and, where it names one, the
/// cluster that snapshot was taken of.
type Entry<'a> = (&'a str, Option<&'a str>);

/// One row of the list-length table: the case name, the entries it sends, and
/// how many of them must come back deleted and failed.
type LengthCase<'a> = (&'a str, Vec<Entry<'a>>, usize, usize);

/// An emulator holding both clusters and one `available` manual snapshot per name.
///
/// The `creating` -> `available` transition is clock-owed, so the seed waits for
/// it once rather than per snapshot.
async fn seeded_emulator(snapshots: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    for cluster in [CLUSTER, OTHER_CLUSTER] {
        let created = emulator.create_minimal(cluster).await;
        assert_eq!(
            created.status, 200,
            "case seed cluster {cluster}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }
    for snapshot in snapshots {
        let taken = emulator
            .call(
                "CreateClusterSnapshot",
                &[
                    ("SnapshotIdentifier", snapshot),
                    ("ClusterIdentifier", CLUSTER),
                ],
            )
            .await;
        assert_eq!(
            taken.status, 200,
            "case seed snapshot {snapshot}: expected 200, got {} with body {}",
            taken.status, taken.body
        );
    }
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    emulator
}

/// `BatchDeleteClusterSnapshots` naming each `(SnapshotIdentifier,
/// SnapshotClusterIdentifier)` entry in order.
async fn batch_delete(emulator: &Emulator, entries: &[Entry<'_>]) -> Answer {
    // An awsQuery list member is sent as `<List>.<member xmlName>.<index>`, and
    // `DeleteClusterSnapshotMessageList`'s member carries the xmlName
    // `DeleteClusterSnapshotMessage`
    // (`smithy_query(query="shape", shape="DeleteClusterSnapshotMessageList")`).
    let mut params: Vec<(String, String)> = Vec::new();
    for (position, (identifier, cluster)) in entries.iter().enumerate() {
        let prefix = format!("Identifiers.DeleteClusterSnapshotMessage.{}", position + 1);
        params.push((
            format!("{prefix}.SnapshotIdentifier"),
            (*identifier).to_string(),
        ));
        if let Some(cluster) = cluster {
            params.push((
                format!("{prefix}.SnapshotClusterIdentifier"),
                (*cluster).to_string(),
            ));
        }
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("BatchDeleteClusterSnapshots", &borrowed)
        .await
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

/// The `Resources` entries of a batch answer, in the order the body lists them.
///
/// `SnapshotIdentifierList`'s member is serialized as `<String>`, and the only
/// `SnapshotIdentifierList` in this output is `Resources`.
fn resources(answer: &Answer) -> Vec<String> {
    all_text(&answer.body, "String")
}

/// The `Errors` entries of a batch answer as `(SnapshotIdentifier, FailureCode,
/// FailureReason)`, in the order the body lists them.
///
/// The open tag is matched WITHOUT its closing `>`: the serializer puts the
/// service's XML namespace on the first member element of the list, so that one
/// element carries an attribute.
fn errors(answer: &Answer) -> Vec<(String, String, String)> {
    answer
        .body
        .split("<SnapshotErrorMessage")
        .skip(1)
        .filter_map(|block| {
            let block = block.split_once('>')?.1;
            let block = block.split("</SnapshotErrorMessage>").next().unwrap_or("");
            let first = |tag: &str| {
                all_text(block, tag)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| String::from("<absent>"))
            };
            Some((
                first("SnapshotIdentifier"),
                first("FailureCode"),
                first("FailureReason"),
            ))
        })
        .collect()
}

/// Every entry of one list is decided on its own: the deletable snapshots are
/// deleted and reported, and each other entry comes back as its own failure.
#[tokio::test]
async fn every_entry_of_one_list_is_decided_on_its_own() {
    let emulator = seeded_emulator(&["deletable-a", "deletable-b", "shared-one"]).await;
    seed_automated(&emulator, CLUSTER, "auto-one", state::STATUS_AVAILABLE);
    // A snapshot still being taken: its clock-owed transition is not due yet, so
    // the entry naming it meets a snapshot that is not `available`.
    let creating = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "creating-one"),
                ("ClusterIdentifier", CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        creating.status, 200,
        "case seed a creating snapshot: expected 200, got {} with body {}",
        creating.status, creating.body
    );
    let granted = emulator
        .call(
            "AuthorizeSnapshotAccess",
            &[
                ("SnapshotIdentifier", "shared-one"),
                ("AccountWithRestoreAccess", GRANTEE_ACCOUNT),
            ],
        )
        .await;
    assert_eq!(
        granted.status, 200,
        "case seed a shared snapshot: expected 200, got {} with body {}",
        granted.status, granted.body
    );

    let not_found = "ClusterSnapshotNotFoundFault";
    let invalid_state = "InvalidClusterSnapshotStateFault";
    let not_found_reason = "Snapshot does not exist for the customer.";

    // The entries differ along WHAT each one gets wrong (or right), and the
    // request lists them together so the suite sees one entry's outcome not
    // disturbing its neighbours'.
    let entries: Vec<Entry> = vec![
        ("deletable-a", None),
        ("no-such-snapshot", None),
        ("auto-one", None),
        ("creating-one", None),
        ("shared-one", None),
        ("", None),
        ("deletable-b", Some(OTHER_CLUSTER)),
        ("deletable-b", Some(CLUSTER)),
        ("deletable-a", None),
    ];
    let answer = batch_delete(&emulator, &entries).await;
    assert_eq!(
        answer.status, 200,
        "case a mixed list: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    // Only the two entries the rule admits are reported as deleted, in the order
    // the request named them.
    assert_eq!(
        resources(&answer),
        vec!["deletable-a".to_string(), "deletable-b".to_string()],
        "case a mixed list: expected only the deletable snapshots under Resources, got {:?} with body {}",
        resources(&answer),
        answer.body
    );

    // One failure per entry the rule excludes, each naming the snapshot the entry
    // named and the code for what it got wrong.
    let expected: Vec<(&str, &str)> = vec![
        ("no-such-snapshot", not_found),
        ("auto-one", invalid_state),
        ("creating-one", invalid_state),
        ("shared-one", invalid_state),
        ("", not_found),
        ("deletable-b", not_found),
        ("deletable-a", not_found),
    ];
    let reported = errors(&answer);
    assert_eq!(
        reported.len(),
        expected.len(),
        "case a mixed list: expected {} failures, got {reported:?} with body {}",
        expected.len(),
        answer.body
    );
    for (position, (identifier, code)) in expected.iter().enumerate() {
        let (reported_identifier, reported_code, reported_reason) = &reported[position];
        let case = format!("entry {} naming {identifier:?}", position + 1);
        assert_eq!(
            reported_identifier, identifier,
            "case {case}: expected the failure to name {identifier:?}, got {reported_identifier:?}"
        );
        assert_eq!(
            reported_code, code,
            "case {case}: expected failure code {code:?}, got {reported_code:?}"
        );
        if *code == not_found {
            assert_eq!(
                reported_reason, not_found_reason,
                "case {case}: expected the fixed reason {not_found_reason:?}, got {reported_reason:?}"
            );
        } else {
            assert!(
                reported_reason.contains(identifier),
                "case {case}: expected the reason to name the snapshot, got {reported_reason:?}"
            );
        }
    }

    // What the call deleted is gone, and what it refused is untouched.
    for deleted in ["deletable-a", "deletable-b"] {
        let read = describe(&emulator, deleted).await;
        assert_eq!(
            read.status, 404,
            "case {deleted} must be gone: expected 404, got {} with body {}",
            read.status, read.body
        );
    }
    for kept in ["auto-one", "creating-one", "shared-one"] {
        let read = describe(&emulator, kept).await;
        assert_eq!(
            read.status, 200,
            "case {kept} must be kept: expected 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.text("SnapshotIdentifier").as_deref(),
            Some(kept),
            "case {kept} must be kept: expected the snapshot back, got {:?}",
            read.text("SnapshotIdentifier")
        );
    }
}

/// A list the published limit long is decided entry by entry; one entry longer
/// refuses the whole call.
#[tokio::test]
async fn the_published_list_limit_bounds_the_call() {
    let emulator = seeded_emulator(&["one-of-two", "two-of-two"]).await;
    let limit = state::BATCH_SNAPSHOT_IDENTIFIER_LIMIT;

    // The rows differ along the LENGTH of the list: one entry, two, the published
    // limit, and one past it.
    let at_limit: Vec<String> = (0..limit).map(|index| format!("filler-{index}")).collect();
    let past_limit: Vec<String> = (0..=limit).map(|index| format!("filler-{index}")).collect();
    let rows: Vec<LengthCase> = vec![
        (
            "one entry",
            vec![("one-of-two", None)],
            1,
            0,
        ),
        (
            "two entries",
            vec![("two-of-two", None), ("no-such-snapshot", None)],
            1,
            1,
        ),
        (
            "the published limit",
            at_limit
                .iter()
                .map(|identifier| (identifier.as_str(), None))
                .collect(),
            0,
            limit,
        ),
    ];

    for (case, entries, deleted, failed) in rows {
        let answer = batch_delete(&emulator, &entries).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            resources(&answer).len(),
            deleted,
            "case {case}: expected {deleted} deleted, got {:?}",
            resources(&answer)
        );
        assert_eq!(
            errors(&answer).len(),
            failed,
            "case {case}: expected {failed} failures, got {} with body {}",
            errors(&answer).len(),
            answer.body
        );
    }

    let case = "one entry past the published limit";
    let entries: Vec<Entry> = past_limit
        .iter()
        .map(|identifier| (identifier.as_str(), None))
        .collect();
    let answer = batch_delete(&emulator, &entries).await;
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some("BatchDeleteRequestSizeExceeded"),
        "case {case}: expected the published-limit refusal, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}
