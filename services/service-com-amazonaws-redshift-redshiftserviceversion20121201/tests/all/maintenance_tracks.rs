// SPDX-License-Identifier: CC0-1.0

//! The maintenance-track catalog `DescribeClusterTracks` reports, end to end over
//! the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds, reusing the harness the cluster-lifecycle suite owns
//! rather than keeping a second copy of it.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded request:
//!
//! * the catalog is the service's published track set, each track carrying the
//!   cluster release it applies and, as its `UpdateTargets`, every OTHER published
//!   track with the operations a switch onto it is performed by;
//! * a `MaintenanceTrackName` selects exactly the track it names, and a name the
//!   catalog does not publish is refused with the modelled
//!   `InvalidClusterTrack` (HTTP 400) — on this read and on every operation that
//!   puts a cluster on a track;
//! * the documented `MaxRecords` window and `Marker` cursor, on each published
//!   bound and one past it;
//! * cross-operation agreement: the `MaintenanceTrackName` a cluster reports is
//!   always one the catalog publishes.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::Emulator;

/// One `UpdateTarget` as the wire carries it.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    name: Option<String>,
    database_version: Option<String>,
    supported_operations: Vec<String>,
}

/// One `MaintenanceTrack` as the wire carries it.
#[derive(Debug, PartialEq, Eq)]
struct Track {
    name: Option<String>,
    database_version: Option<String>,
    update_targets: Vec<Target>,
}

/// The INNER XML of every `<tag>` element in an awsQuery body, in document
/// order.
///
/// Like [`crate::cluster_lifecycle::all_text`], a scanner over this one wire
/// format rather than an XML parser from crates.io (which would add an entry to
/// the workspace lockfile), but tolerant of ATTRIBUTES on the open tag: the
/// framework's serializer repeats the service `xmlns` on the first entry of a
/// list, so `<MaintenanceTrack xmlns="…">` and `<MaintenanceTrack>` are the same
/// element. Element names are matched WHOLE — the character after the name must
/// end the tag — so `<MaintenanceTrack>` never matches `<MaintenanceTracks>` or
/// `<MaintenanceTrackName>`.
///
/// `pub(crate)` so a sibling suite reading a nested list reuses this scanner
/// instead of keeping a second copy of it.
pub(crate) fn elements(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let ends_name = after
            .chars()
            .next()
            .is_some_and(|next| next == '>' || next.is_whitespace());
        if !ends_name {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        let inner = &after[gt + 1..];
        let Some(end) = inner.find(&close) else { break };
        found.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    found
}

/// Every `MaintenanceTrack` a `DescribeClusterTracks` body carries, in the order
/// the page lists them.
///
/// `TrackList`'s member carries `xmlName: MaintenanceTrack`, so one element per
/// track; `EligibleTracksToUpdateList`'s carries `xmlName: UpdateTarget` and
/// `SupportedOperationList`'s `xmlName: SupportedOperation`. A structure
/// serializes its members in declaration order, so a track's own name and
/// version are the FIRST inside its element and the rest belong to its update
/// targets.
fn listed_tracks(xml: &str) -> Vec<Track> {
    elements(xml, "MaintenanceTrack")
        .into_iter()
        .map(|track| {
            let update_targets = elements(&track, "UpdateTarget")
                .into_iter()
                .map(|target| Target {
                    name: elements(&target, "MaintenanceTrackName").into_iter().next(),
                    database_version: elements(&target, "DatabaseVersion").into_iter().next(),
                    supported_operations: elements(&target, "OperationName"),
                })
                .collect();
            Track {
                name: elements(&track, "MaintenanceTrackName").into_iter().next(),
                database_version: elements(&track, "DatabaseVersion").into_iter().next(),
                update_targets,
            }
        })
        .collect()
}

/// The track names a body lists, in page order.
fn listed_names(xml: &str) -> Vec<String> {
    listed_tracks(xml)
        .into_iter()
        .filter_map(|track| track.name)
        .collect()
}

/// The catalog this service publishes, written out here rather than read off the
/// production helper so a wrong entry fails this suite.
///
/// Source: `smithy_query(query="shape", shape="MaintenanceTrack")` on
/// `MaintenanceTrackName` — "Possible values are `current` and `trailing`" — and
/// on the structure itself: `current` applies "the most recently certified
/// maintenance release", `trailing` "the previously certified maintenance
/// release". Those are two DIFFERENT releases, so the two tracks report
/// different `DatabaseVersion` values and a client can tell them apart.
const PUBLISHED_TRACKS: [&str; 2] = ["current", "trailing"];

/// The operations a track you can switch to supports, in the order the service
/// reports them and in the hyphenated form `SupportedOperation$OperationName`
/// names an operation by: the model's operations whose input carries a
/// `MaintenanceTrackName` and acts on a cluster that already exists
/// (`RestoreFromClusterSnapshot`, `ModifyCluster`).
const SWITCH_OPERATIONS: [&str; 2] = ["restore-from-cluster-snapshot", "modify-cluster"];

/// Names the catalog does not publish, spanning the ways a name can be wrong:
/// wrong case, a track AWS retired, a name of another shape entirely, one that is
/// only a prefix of a published name, and one that only extends it.
const UNPUBLISHED_TRACKS: [&str; 5] = ["Current", "preview_2023", "no-such-track", "curr", "currents"];

#[tokio::test]
async fn the_published_track_catalog_is_what_a_read_reports() {
    let emulator = Emulator::new();

    // The catalog the store seeds matches the published set, entry for entry.
    let published = state::published_maintenance_tracks();
    let names: Vec<String> = published
        .iter()
        .filter_map(|track| track.maintenance_track_name.clone())
        .collect();
    assert_eq!(
        names,
        PUBLISHED_TRACKS.map(str::to_string).to_vec(),
        "case published catalog: expected the documented track set, got {names:?}"
    );

    // Answered before any cluster exists: the tracks a cluster can be assigned
    // to are catalog data, not something a cluster creates.
    let answer = emulator.call("DescribeClusterTracks", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case read an empty emulator: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    // `current` applies the most recently certified release and `trailing` the
    // one certified before it, so the two carry different releases of the
    // cluster-release stream — and every `UpdateTarget` carries the release of
    // the track it points AT, which is what a client diffing the two tracks
    // reads.
    let releases = state::PUBLISHED_TRACK_RELEASES;
    assert_eq!(
        releases.len(),
        PUBLISHED_TRACKS.len(),
        "case published catalog: expected one published release per track, got {releases:?}"
    );
    for (track, release) in releases {
        // A release is a patch release of the 1.0 cluster-release stream: the
        // engine version, then the patch number the service certified.
        let patch = release.strip_prefix("1.0.").unwrap_or_default();
        assert!(
            !patch.is_empty() && patch.chars().all(|digit| digit.is_ascii_digit()),
            "case release of {track}: expected a 1.0 cluster release, got {release:?}"
        );
    }
    assert_ne!(
        releases[0].1, releases[1].1,
        "case published catalog: the two tracks must apply different releases, got {:?} twice",
        releases[0].1
    );
    let release_of = |track: &str| {
        releases
            .into_iter()
            .find(|(published, _)| *published == track)
            .map(|(_, release)| release.to_string())
            .unwrap_or_else(|| panic!("case release of {track}: the catalog publishes no release"))
    };
    let expected: Vec<Track> = PUBLISHED_TRACKS
        .into_iter()
        .map(|track| Track {
            name: Some(track.to_string()),
            database_version: Some(release_of(track)),
            update_targets: PUBLISHED_TRACKS
                .into_iter()
                .filter(|target| *target != track)
                .map(|target| Target {
                    name: Some(target.to_string()),
                    database_version: Some(release_of(target)),
                    supported_operations: SWITCH_OPERATIONS.map(str::to_string).to_vec(),
                })
                .collect(),
        })
        .collect();
    let listed = listed_tracks(&answer.body);
    assert_eq!(
        listed, expected,
        "case read an empty emulator: expected {expected:?}, got {listed:?} from body {}",
        answer.body
    );

    // A complete page carries no cursor to continue from.
    assert_eq!(
        answer.text("Marker"),
        None,
        "case read an empty emulator: a complete page returns no Marker, got {:?}",
        answer.text("Marker")
    );

    // A read leaves nothing behind: the next caller sees exactly the same answer.
    let again = emulator.call("DescribeClusterTracks", &[]).await;
    assert_eq!(
        (again.status, listed_tracks(&again.body)),
        (answer.status, listed),
        "case read twice: expected an unchanged catalog, got {} with body {}",
        again.status,
        again.body
    );
}

#[tokio::test]
async fn a_track_name_selects_that_track_and_an_unpublished_one_is_refused() {
    let emulator = Emulator::new();

    // Each published name selects exactly its own track.
    for track in PUBLISHED_TRACKS {
        let answer = emulator
            .call("DescribeClusterTracks", &[("MaintenanceTrackName", track)])
            .await;
        assert_eq!(
            answer.status, 200,
            "case select {track}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_names(&answer.body),
            vec![track.to_string()],
            "case select {track}: expected exactly that track, got body {}",
            answer.body
        );
    }

    // A name the catalog does not publish is "not valid" — the modelled
    // `InvalidClusterTrackFault`, HTTP 400 with the awsQuery code
    // `InvalidClusterTrack` — never an empty page.
    for track in UNPUBLISHED_TRACKS {
        let answer = emulator
            .call("DescribeClusterTracks", &[("MaintenanceTrackName", track)])
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidClusterTrack".to_string())),
            "case select unpublished {track}: expected a 400 InvalidClusterTrack, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

#[tokio::test]
async fn the_track_window_and_cursor_follow_the_documented_rules() {
    let emulator = Emulator::new();

    // "Default: 100. Constraints: minimum 20, maximum 100." — the window every
    // paginated Redshift `Describe*` shares. Each published bound, and one past
    // it on either side; only the minimum is enforced.
    for (case, max_records, accepted) in [
        (
            "one below the minimum",
            (state::MAX_RECORDS_MIN - 1).to_string(),
            false,
        ),
        ("at the minimum", state::MAX_RECORDS_MIN.to_string(), true),
        ("at the maximum", state::MAX_RECORDS_MAX.to_string(), true),
        (
            "one above the maximum",
            (state::MAX_RECORDS_MAX + 1).to_string(),
            true,
        ),
    ] {
        let answer = emulator
            .call("DescribeClusterTracks", &[("MaxRecords", &max_records)])
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case MaxRecords {case} ({max_records}): expected accepted={accepted}, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
        if accepted {
            assert_eq!(
                listed_names(&answer.body),
                PUBLISHED_TRACKS.map(str::to_string).to_vec(),
                "case MaxRecords {case} ({max_records}): the catalog fits in one page, got body {}",
                answer.body
            );
            assert_eq!(
                answer.text("Marker"),
                None,
                "case MaxRecords {case} ({max_records}): a complete page returns no Marker, got {:?}",
                answer.text("Marker")
            );
        } else {
            assert_eq!(
                answer.error_code().as_deref(),
                Some("InvalidParameterValue"),
                "case MaxRecords {case} ({max_records}): expected an InvalidParameterValue, got {:?}",
                answer.error_code()
            );
        }
    }

    // A cursor nobody handed out is refused rather than served as an offset,
    // exactly as on the other paginated reads.
    for (case, cursor) in [
        ("not a number", "not-a-cursor"),
        ("an invented offset", "1"),
        ("an offset with a forged tag", "0.00000000"),
    ] {
        let bogus = emulator
            .call("DescribeClusterTracks", &[("Marker", cursor)])
            .await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case marker {case} ({cursor:?}): expected a 400 refusal, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }
}

#[tokio::test]
async fn every_cluster_runs_on_a_track_the_catalog_publishes() {
    let emulator = Emulator::new();

    // Each row creates a cluster and names the track it must report: the request
    // either names a published track or names none and takes the documented
    // default ("If you don't provide a maintenance track name, the cluster is
    // assigned to the current track").
    let mut rows: Vec<(String, String, Option<&str>)> = vec![(
        "no track named".to_string(),
        "track-default".to_string(),
        None,
    )];
    for track in PUBLISHED_TRACKS {
        rows.push((
            format!("the {track} track named"),
            format!("track-{track}"),
            Some(track),
        ));
    }

    for (case, identifier, requested) in rows {
        let created = match requested {
            None => emulator.create_minimal(&identifier).await,
            Some(track) => {
                emulator
                    .call(
                        "CreateCluster",
                        &[
                            ("ClusterIdentifier", &identifier),
                            ("NodeType", "ra3.xlplus"),
                            ("MasterUsername", "admin"),
                            ("MaintenanceTrackName", track),
                        ],
                    )
                    .await
            }
        };
        assert_eq!(
            created.status, 200,
            "case create with {case}: expected 200, got {} with body {}",
            created.status, created.body
        );
        let expected = requested.unwrap_or(state::DEFAULT_MAINTENANCE_TRACK);

        // Read it back through `DescribeClusters`, then look that track up in the
        // catalog: the two reads cannot disagree about which tracks exist.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        let reported = described
            .text("MaintenanceTrackName")
            .unwrap_or_else(|| panic!("case describe with {case}: expected a MaintenanceTrackName"));
        assert_eq!(
            reported, expected,
            "case describe with {case}: expected track {expected}, got {reported}"
        );

        let catalog = emulator
            .call(
                "DescribeClusterTracks",
                &[("MaintenanceTrackName", &reported)],
            )
            .await;
        assert_eq!(
            (catalog.status, listed_names(&catalog.body)),
            (200, vec![reported.clone()]),
            "case catalog lookup with {case}: expected the catalog to publish {reported}, got {} with body {}",
            catalog.status,
            catalog.body
        );
    }

    // A later `ModifyCluster` keeps the rule: switching onto a published track
    // is accepted and reported back, and the catalog still publishes it.
    for track in PUBLISHED_TRACKS {
        let modified = emulator
            .call(
                "ModifyCluster",
                &[
                    ("ClusterIdentifier", "track-default"),
                    ("MaintenanceTrackName", track),
                ],
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case modify onto {track}: expected 200, got {} with body {}",
            modified.status, modified.body
        );
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", "track-default")])
            .await;
        assert_eq!(
            described.text("MaintenanceTrackName").as_deref(),
            Some(track),
            "case modify onto {track}: expected the cluster on {track}, got {:?}",
            described.text("MaintenanceTrackName")
        );
    }
}

#[tokio::test]
async fn a_write_naming_an_unpublished_track_is_refused() {
    let emulator = Emulator::new();

    let seeded = emulator.create_minimal("track-writes").await;
    assert_eq!(
        seeded.status, 200,
        "case seed cluster: expected 200, got {} with body {}",
        seeded.status, seeded.body
    );
    let snapshot = emulator
        .call(
            "CreateClusterSnapshot",
            &[
                ("SnapshotIdentifier", "track-writes-snap"),
                ("ClusterIdentifier", "track-writes"),
            ],
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "case seed snapshot: expected 200, got {} with body {}",
        snapshot.status, snapshot.body
    );

    // Every operation that puts a cluster on a maintenance track resolves the
    // name against the SAME catalog the read reports, so a name outside it is
    // refused with `InvalidClusterTrack` rather than stored on a cluster.
    for track in UNPUBLISHED_TRACKS {
        let writes: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "CreateCluster",
                vec![
                    ("ClusterIdentifier", "track-refused"),
                    ("NodeType", "ra3.xlplus"),
                    ("MasterUsername", "admin"),
                    ("MaintenanceTrackName", track),
                ],
            ),
            (
                "ModifyCluster",
                vec![
                    ("ClusterIdentifier", "track-writes"),
                    ("MaintenanceTrackName", track),
                ],
            ),
            (
                "RestoreFromClusterSnapshot",
                vec![
                    ("ClusterIdentifier", "track-restored"),
                    ("SnapshotIdentifier", "track-writes-snap"),
                    ("MaintenanceTrackName", track),
                ],
            ),
        ];
        for (action, params) in writes {
            let answer = emulator.call(action, &params).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidClusterTrack".to_string())),
                "case {action} with unpublished {track}: expected a 400 InvalidClusterTrack, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // The refusals left nothing behind: neither write created a cluster, and the
    // seeded cluster is still on the track it was created with.
    for identifier in ["track-refused", "track-restored"] {
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
            .await;
        assert_eq!(
            described.status, 404,
            "case refused write left {identifier} behind: expected a 404 not-found, got {} with body {}",
            described.status, described.body
        );
    }
    let described = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", "track-writes")])
        .await;
    assert_eq!(
        described.text("MaintenanceTrackName").as_deref(),
        Some(state::DEFAULT_MAINTENANCE_TRACK),
        "case refused modify: expected the cluster still on the default track, got {:?}",
        described.text("MaintenanceTrackName")
    );
}
