// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE an `InvalidClusterTrackFault` carries, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The RULE the suite asserts, over a table of names rather than one recorded
//! request: **the fault's status (400) and awsQuery code (`InvalidClusterTrack`)
//! are what the model fixes for every operation that names a
//! `MaintenanceTrackName`, while the sentence it carries belongs to the
//! OPERATION — the operations that would place a cluster on the named track
//! (`CreateCluster`, `ModifyCluster`, `RestoreFromClusterSnapshot`) answer the
//! fixed prefix `Invalid cluster track: `, the requested name copied verbatim
//! from the request, and a terminating period, while the catalog read
//! `DescribeClusterTracks` answers the same prefix and name with no terminator.**
//!
//! The expected sentences are spelled out here rather than read off the crate
//! under test, so a row cannot pass by restating whatever the handler happened to
//! build.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The cluster the modify rows act on, and the snapshot the restore rows restore.
const SEEDED_CLUSTER: &str = "track-wording-cluster";
const SEEDED_SNAPSHOT: &str = "track-wording-snapshot";

/// Which sentence an operation words its invalid-track refusal with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wording {
    /// `Invalid cluster track: <MaintenanceTrackName>.` — the sentence the
    /// operations that place a cluster on a track terminate.
    TrackThenPeriod,
    /// `Invalid cluster track: <MaintenanceTrackName>` — the catalog read's own
    /// unterminated sentence.
    TrackUnterminated,
}

/// The sentence `wording` words about the name a request supplied.
fn sentence(wording: Wording, requested: &str) -> String {
    match wording {
        Wording::TrackThenPeriod => format!("Invalid cluster track: {requested}."),
        Wording::TrackUnterminated => format!("Invalid cluster track: {requested}"),
    }
}

/// Names the catalog does not publish, spanning the ways a name can fall outside
/// it: a plain name nothing publishes, a published name in another case, a
/// published name with a character added and with its last one removed, a name of
/// another shape entirely, one carrying surrounding space, and a long one.
///
/// The published set they are measured against is
/// [`state::PUBLISHED_MAINTENANCE_TRACKS`], the catalog
/// `DescribeClusterTracks` reports (`smithy_query(query="shape",
/// shape="MaintenanceTrack")` on `MaintenanceTrackName`: "Possible values are
/// `current` and `trailing`").
fn unpublished_names() -> Vec<(&'static str, String)> {
    let published = state::PUBLISHED_MAINTENANCE_TRACKS;
    let first = published[0];
    vec![
        ("a name nothing publishes", "no-such-track".to_string()),
        (
            "a published name in upper case",
            first.to_ascii_uppercase(),
        ),
        (
            "a published name with a character added",
            format!("{first}x"),
        ),
        (
            "a published name missing its last character",
            first[..first.len() - 1].to_string(),
        ),
        ("a name of another shape", "track/2026".to_string()),
        ("a name with surrounding space", format!(" {first} ")),
        (
            "a long name",
            std::iter::repeat_n('t', 120).collect::<String>(),
        ),
    ]
}

/// One operation that names a `MaintenanceTrackName`: the case, the action, the
/// request it sends about one track name, and the sentence its refusal carries.
type TrackCase = (
    &'static str,
    &'static str,
    fn(&str) -> Vec<(String, String)>,
    Wording,
);

fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

fn create_request(track: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", "track-wording-created"),
        pair("NodeType", "ra3.xlplus"),
        pair("MasterUsername", "admin"),
        pair("MaintenanceTrackName", track),
    ]
}

fn modify_request(track: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", SEEDED_CLUSTER),
        pair("MaintenanceTrackName", track),
    ]
}

fn restore_request(track: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", "track-wording-restored"),
        pair("SnapshotIdentifier", SEEDED_SNAPSHOT),
        pair("MaintenanceTrackName", track),
    ]
}

fn read_request(track: &str) -> Vec<(String, String)> {
    vec![pair("MaintenanceTrackName", track)]
}

/// Every operation the rule covers: the three cluster writes and the catalog
/// read.
fn track_cases() -> Vec<TrackCase> {
    vec![
        (
            "CreateCluster",
            "CreateCluster",
            create_request as fn(&str) -> Vec<(String, String)>,
            Wording::TrackThenPeriod,
        ),
        (
            "ModifyCluster",
            "ModifyCluster",
            modify_request as fn(&str) -> Vec<(String, String)>,
            Wording::TrackThenPeriod,
        ),
        (
            "RestoreFromClusterSnapshot",
            "RestoreFromClusterSnapshot",
            restore_request as fn(&str) -> Vec<(String, String)>,
            Wording::TrackThenPeriod,
        ),
        (
            "DescribeClusterTracks",
            "DescribeClusterTracks",
            read_request as fn(&str) -> Vec<(String, String)>,
            Wording::TrackUnterminated,
        ),
    ]
}

/// One row of the acceptance table: the case, the action, and the parameters it
/// sends.
type AcceptedCase = (String, &'static str, Vec<(String, String)>);

/// POST one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// Seed the cluster the modify rows name and the snapshot the restore rows
/// restore from.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_minimal(SEEDED_CLUSTER).await;
    assert_eq!(
        created.status, 200,
        "case seed {SEEDED_CLUSTER}: expected 200, got {} with body {}",
        created.status, created.body
    );
    let snapshot = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", SEEDED_SNAPSHOT),
                ("ClusterIdentifier", SEEDED_CLUSTER),
            ],
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "case seed {SEEDED_SNAPSHOT}: expected 200, got {} with body {}",
        snapshot.status, snapshot.body
    );
}

#[tokio::test]
async fn an_invalid_track_refusal_carries_the_sentence_its_operation_words() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for (name_case, requested) in unpublished_names() {
        for (case, action, request, wording) in track_cases() {
            let case = format!("{case} with {name_case} ({requested:?})");
            let refused = call(&emulator, action, &request(&requested)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, Some("InvalidClusterTrack".to_string())),
                "case {case}: expected 400 InvalidClusterTrack, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let expected = sentence(wording, &requested);
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {:?}",
                refused.text("Message")
            );
        }
    }
}

#[tokio::test]
async fn a_write_and_the_catalog_read_word_one_name_two_ways() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The two sentences are DISTINGUISHABLE: the SAME unpublished name asked
    // about through a cluster write and through the catalog read comes back
    // worded two ways rather than collapsed onto one template — and the write's
    // sentence is the read's with the terminator the write adds.
    let requested = "no-such-track";
    let written = call(&emulator, "CreateCluster", &create_request(requested)).await;
    let read = call(&emulator, "DescribeClusterTracks", &read_request(requested)).await;
    let written_message = written.text("Message").unwrap_or_else(|| {
        panic!(
            "case CreateCluster on {requested:?}: expected a refusal message, got body {}",
            written.body
        )
    });
    let read_message = read.text("Message").unwrap_or_else(|| {
        panic!(
            "case DescribeClusterTracks on {requested:?}: expected a refusal message, got body {}",
            read.body
        )
    });
    assert_ne!(
        written_message, read_message,
        "case the two wordings: expected a different sentence per operation for {requested:?}, got {written_message:?} twice"
    );
    assert_eq!(
        written_message,
        format!("{read_message}."),
        "case the two wordings: expected the write's sentence to be the read's terminated, got {written_message:?} against {read_message:?}"
    );
}

/// Wait out the span a cluster and a snapshot are provisioned over, so a write
/// that needs its subject already available is not refused for its state
/// instead of for the track it names.
async fn settle() {
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
}

#[tokio::test]
async fn a_published_track_is_never_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    settle().await;

    // The other half of the rule: every name the catalog DOES publish is served
    // by each of the same operations, so the sentences above answer a name
    // outside the catalog and not every request naming a track.
    for (index, track) in state::PUBLISHED_MAINTENANCE_TRACKS.into_iter().enumerate() {
        let accepted: Vec<AcceptedCase> = vec![
            (
                format!("CreateCluster onto {track}"),
                "CreateCluster",
                vec![
                    pair("ClusterIdentifier", &format!("track-created-{index}")),
                    pair("NodeType", "ra3.xlplus"),
                    pair("MasterUsername", "admin"),
                    pair("MaintenanceTrackName", track),
                ],
            ),
            (
                format!("ModifyCluster onto {track}"),
                "ModifyCluster",
                modify_request(track),
            ),
            (
                format!("RestoreFromClusterSnapshot onto {track}"),
                "RestoreFromClusterSnapshot",
                vec![
                    pair("ClusterIdentifier", &format!("track-restored-{index}")),
                    pair("SnapshotIdentifier", SEEDED_SNAPSHOT),
                    pair("MaintenanceTrackName", track),
                ],
            ),
            (
                format!("DescribeClusterTracks on {track}"),
                "DescribeClusterTracks",
                read_request(track),
            ),
        ];
        for (case, action, params) in accepted {
            let served = call(&emulator, action, &params).await;
            assert_eq!(
                served.status, 200,
                "case {case}: expected 200, got {} / {:?} with body {}",
                served.status,
                served.error_code(),
                served.body
            );
        }
    }
}
