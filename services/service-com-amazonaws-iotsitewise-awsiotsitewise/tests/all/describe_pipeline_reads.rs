// SPDX-License-Identifier: CC0-1.0

//! `DescribePipeline` as a REPORTING read, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The sibling `pipeline_lifecycle` suite pins what a created or updated pipeline
//! reads back AS. These suites pin the rule that makes the read a read — the
//! operation is `@readonly` (smithy_query query="operation"
//! operation="DescribePipeline"), so it answers from what the writes persisted and
//! changes nothing a later caller sees:
//!
//! * every read answers from the version the write it names persisted, so two
//!   reads of the same pipeline — at the latest version or at any version a write
//!   produced — agree on every member but the lifecycle status the clock owes, and
//!   `createdAt` is the instant the create recorded whichever version is read;
//! * a read writes NOTHING: the whole stored pipeline map is identical before and
//!   after any number of reads, including the ones that refuse (a pipeline no
//!   workspace holds, a version no write produced), and including reads taken
//!   either side of the instant an owed transition falls due — the status is
//!   DERIVED from the instants the write recorded, never stored and advanced by
//!   whoever reads;
//! * each pipeline's status comes from its OWN last write, so pipelines written at
//!   different instants report independently and reading one never drags another
//!   along.
//!
//! The HTTP harness (`call`, `router_with_engine`) is the asset suite's, and the
//! `CreatePipeline` / `DescribePipeline` wrappers and ARN oracle are the pipeline
//! suite's; this suite reuses both rather than growing a third copy.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ACTIVATION_DELAY, FIRST_PIPELINE_VERSION, STATE_ACTIVE, STATE_CREATING,
    STATE_UPDATING,
};

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::pipeline_lifecycle::{
    create, describe, expected_arn, node, pipeline_uri, without_status,
};

/// The workspace most rows below write in.
const WORKSPACE: &str = "read-only-pipelines";

/// A second workspace, so the table spans more than one pipeline scope.
const OTHER_WORKSPACE: &str = "read-only-pipelines_2";

/// How long past an owed transition's due instant a read is taken, so a settled
/// read is unambiguous.
const SETTLE_MARGIN: Duration = Duration::from_millis(200);

/// Every pipeline the store holds, one line per entry and sorted, so two snapshots
/// compare as a WHOLE: what a read must leave untouched is not one member of one
/// record but the entire map.
///
/// Rendered through `Debug` rather than compared field by field so a record that
/// grows a member is covered without this suite being updated.
fn stored_pipelines(shared: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let state = service_state(&**shared).expect("the iotsitewise store");
    let guard = state.lock();
    let mut rendered: Vec<String> = guard
        .pipelines
        .iter()
        .map(|(key, record)| format!("{key:?} => {record:?}"))
        .collect();
    drop(guard);
    rendered.sort();
    rendered
}

/// How far along its lifecycle a reported state is, so a sequence of reads can be
/// checked for never going backwards. Source: the `ResourceState` values a
/// pipeline's write leaves it in (`CREATING` / `UPDATING`) and the one the owed
/// transition takes it to (`ACTIVE`).
fn rank(state: &str) -> usize {
    match state {
        STATE_CREATING | STATE_UPDATING => 0,
        STATE_ACTIVE => 1,
        other => panic!("DescribePipeline reported a state no pipeline write leaves: {other:?}"),
    }
}

/// The `state` of the `status` a read reported.
fn reported_state(body: &Value) -> &str {
    body["status"]["state"].as_str().unwrap_or("")
}

/// One row of the read table: a pipeline, the workspace it lives in, how many
/// writes it takes and how many times each of its versions is read.
#[derive(Debug, Clone)]
struct ReadCase {
    label: &'static str,
    workspace: &'static str,
    /// How many writes the pipeline takes: the create, then that many updates
    /// less one, so the newest version is `writes`.
    writes: u64,
    reads_per_version: usize,
}

/// The rows differ along what the rule generalises over: how many versions the
/// pipeline holds (one write, two, several), which workspace it lives in, and how
/// many times each version is read — a read is no more stable for a one-version
/// pipeline than for one with a history.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "created-only-read-twice",
            workspace: WORKSPACE,
            writes: 1,
            reads_per_version: 2,
        },
        ReadCase {
            label: "updated-once-read-three-times",
            workspace: WORKSPACE,
            writes: 2,
            reads_per_version: 3,
        },
        ReadCase {
            label: "five-versions-read-twice",
            workspace: WORKSPACE,
            writes: 5,
            reads_per_version: 2,
        },
        ReadCase {
            label: "another-workspace-read-four-times",
            workspace: OTHER_WORKSPACE,
            writes: 3,
            reads_per_version: 4,
        },
    ]
}

/// Write `case.writes` versions of one pipeline and return its name: the create,
/// then an update per further version, each carrying a configuration of its own so
/// a version-addressed read has something to tell apart.
async fn write_versions(router: &MultiServiceRouter, case: &ReadCase) -> String {
    let name = format!("pipeline-{}", case.label);
    let (status, created) = create(
        router,
        case.workspace,
        json!({
            "pipelineName": name,
            "computations": [node("extract", "extract-task", &[])],
            "description": format!("{} at version {FIRST_PIPELINE_VERSION}", case.label),
            "environmentVariables": { "LOG_LEVEL": "info" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {}: CreatePipeline must accept the request, got {status} {created}",
        case.label
    );

    for version in FIRST_PIPELINE_VERSION + 1..=case.writes {
        let (status, updated) = call(
            router,
            "PUT",
            &pipeline_uri(case.workspace, &name),
            Some(json!({
                "computations": [node(&format!("node_{version}"), "extract-task", &[])],
                "description": format!("{} at version {version}", case.label),
                "environmentVariables": { "LOG_LEVEL": "debug", "VERSION": version.to_string() },
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: UpdatePipeline to version {version} must be accepted, got {status} {updated}",
            case.label
        );
        assert_eq!(
            updated["version"].as_str().unwrap_or_default(),
            version.to_string(),
            "case {}: an accepted update answers with the version it wrote, got {updated}",
            case.label
        );
    }
    name
}

#[tokio::test]
async fn every_read_answers_the_persisted_pipeline_and_writes_nothing() {
    let (router, shared) = router_with_engine();

    let mut written: Vec<(ReadCase, String)> = Vec::new();
    for case in read_cases() {
        let name = write_versions(&router, &case).await;
        written.push((case, name));
    }

    // Past every owed transition, so the pipelines are not moving under the reads
    // below and any difference between two snapshots is a read having written.
    tokio::time::sleep(ACTIVATION_DELAY + SETTLE_MARGIN).await;
    let before = stored_pipelines(&shared);
    assert_eq!(
        before.len(),
        written.len(),
        "the store must hold one pipeline per create, expected {}, got {before:?}",
        written.len()
    );

    for (case, name) in &written {
        // The newest version is what a read naming no version answers from.
        let (status, latest) = describe(&router, case.workspace, name, None).await;
        assert_eq!(
            status, 200,
            "case {}: a read naming no version must answer, got {status} {latest}",
            case.label
        );
        assert_eq!(
            latest["version"].as_str().unwrap_or_default(),
            case.writes.to_string(),
            "case {}: a read naming no version answers from the newest one, got {latest}",
            case.label
        );
        assert_eq!(
            latest["pipelineArn"].as_str().unwrap_or_default(),
            expected_arn(case.workspace, name),
            "case {}: unexpected pipelineArn in {latest}",
            case.label
        );

        let created_at = latest["createdAt"].as_f64().unwrap_or_default();
        assert!(
            created_at > 0.0,
            "case {}: createdAt must be the instant the create recorded, got {latest}",
            case.label
        );

        // Every version a write produced, oldest first, plus the read that names
        // no version at all.
        let labels: Vec<Option<String>> = (FIRST_PIPELINE_VERSION..=case.writes)
            .map(|version| Some(version.to_string()))
            .chain([None])
            .collect();
        let mut previous_updated_at = 0.0_f64;
        let mut highest_rank = 0;
        for wanted in &labels {
            let mut answers: Vec<Value> = Vec::new();
            for read in 1..=case.reads_per_version {
                let (status, answer) =
                    describe(&router, case.workspace, name, wanted.as_deref()).await;
                assert_eq!(
                    status, 200,
                    "case {} version {wanted:?} read {read}: a version a write produced must read back, got {status} {answer}",
                    case.label
                );
                answers.push(answer);
            }

            let first = answers.first().expect("every row reads at least once").clone();
            assert_eq!(
                first["version"].as_str().unwrap_or_default(),
                wanted
                    .clone()
                    .unwrap_or_else(|| case.writes.to_string())
                    .as_str(),
                "case {} version {wanted:?}: a read answers from the version it names, got {first}",
                case.label
            );
            assert_eq!(
                first["createdAt"].as_f64().unwrap_or_default(),
                created_at,
                "case {} version {wanted:?}: createdAt is the pipeline's, the same at every version, expected {created_at}, got {first}",
                case.label
            );
            let updated_at = first["updatedAt"].as_f64().unwrap_or_default();
            assert!(
                updated_at >= previous_updated_at,
                "case {} version {wanted:?}: a later version was written no earlier than the one before it, expected at least {previous_updated_at}, got {updated_at}",
                case.label
            );
            assert!(
                updated_at >= created_at,
                "case {} version {wanted:?}: a version was written no earlier than the pipeline was created, expected at least {created_at}, got {updated_at}",
                case.label
            );
            previous_updated_at = updated_at;

            for (at, answer) in answers.iter().enumerate() {
                assert_eq!(
                    without_status(answer),
                    without_status(&first),
                    "case {} version {wanted:?} read {}: every member but the status must be identical on every read, expected {first}, got {answer}",
                    case.label,
                    at + 1
                );
                let state = reported_state(answer);
                assert!(
                    rank(state) >= highest_rank,
                    "case {} version {wanted:?} read {}: a state may only move forwards, got {state}",
                    case.label,
                    at + 1
                );
                highest_rank = rank(state);
            }
        }
    }

    // The refusals write nothing either: a pipeline name no workspace holds, a
    // workspace that holds no pipeline at all, and a version no write produced —
    // one past the newest, and a far one at the published `Version` maximum
    // length (len 1..10).
    let (case, held) = written.first().cloned().expect("at least one row");
    let refusals: Vec<(&str, &str, Option<String>)> = vec![
        ("unknown-pipeline", case.workspace, None),
        ("unknown-workspace", "no-such-workspace", None),
        (
            "version-past-the-newest",
            case.workspace,
            Some((case.writes + 1).to_string()),
        ),
        ("version-zero", case.workspace, Some("0".to_string())),
        (
            "far-version",
            case.workspace,
            Some("9999999999".to_string()),
        ),
    ];
    for (label, workspace, version) in refusals {
        let pipeline = if label == "unknown-pipeline" {
            "never-written"
        } else {
            held.as_str()
        };
        let (status, body) = describe(&router, workspace, pipeline, version.as_deref()).await;
        assert_eq!(
            status, 404,
            "case {label}: a read that resolves to nothing is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: unexpected error code in {body}"
        );
    }

    assert_eq!(
        stored_pipelines(&shared),
        before,
        "no number of reads, refused or served, may change the stored pipelines, expected {before:?}"
    );
}

#[tokio::test]
async fn each_pipeline_reports_the_transition_its_own_write_owes() {
    let (router, shared) = router_with_engine();

    let earlier = write_versions(
        &router,
        &ReadCase {
            label: "earlier",
            workspace: WORKSPACE,
            writes: 1,
            reads_per_version: 1,
        },
    )
    .await;
    // Past the earlier pipeline's owed transition, and before the later one is
    // created at all.
    tokio::time::sleep(ACTIVATION_DELAY + SETTLE_MARGIN).await;
    let later = write_versions(
        &router,
        &ReadCase {
            label: "later",
            workspace: WORKSPACE,
            writes: 1,
            reads_per_version: 1,
        },
    )
    .await;

    // The pipeline whose owed transition has NOT fallen due yet is read first,
    // before any other work, so the assertion does not depend on how long the rest
    // of this suite takes.
    for (label, name, expected) in [
        ("later-still-creating", &later, STATE_CREATING),
        ("earlier-settled", &earlier, STATE_ACTIVE),
    ] {
        let (status, body) = describe(&router, WORKSPACE, name, None).await;
        assert_eq!(
            status, 200,
            "case {label}: DescribePipeline must answer, got {status} {body}"
        );
        assert_eq!(
            reported_state(&body),
            expected,
            "case {label}: each status comes from that pipeline's own last write, expected {expected}, got {body}"
        );
    }

    let before = stored_pipelines(&shared);

    // An accepted update owes a transition of its own, so the pipeline it wrote
    // reports UPDATING while the one nobody touched stays as it was.
    let (status, updated) = call(
        &router,
        "PUT",
        &pipeline_uri(WORKSPACE, &earlier),
        Some(json!({ "computations": [node("reshaped", "extract-task", &[])] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdatePipeline must be accepted, got {status} {updated}"
    );
    let (status, body) = describe(&router, WORKSPACE, &earlier, None).await;
    assert_eq!(status, 200, "the read during the update got {status} {body}");
    assert_eq!(
        reported_state(&body),
        STATE_UPDATING,
        "case updating: a pipeline an accepted update is settling reports {STATE_UPDATING}, got {body}"
    );

    // Past every owed transition: both settle, without any read having moved them.
    tokio::time::sleep(ACTIVATION_DELAY + SETTLE_MARGIN).await;
    for (label, name) in [("earlier", &earlier), ("later", &later)] {
        let (status, body) = describe(&router, WORKSPACE, name, None).await;
        assert_eq!(
            status, 200,
            "case {label}-terminal: DescribePipeline must answer, got {status} {body}"
        );
        assert_eq!(
            reported_state(&body),
            STATE_ACTIVE,
            "case {label}-terminal: every written pipeline settles at {STATE_ACTIVE}, got {body}"
        );
    }

    // The states above moved because the clock did, and because the update wrote;
    // the reads themselves touched nothing. The snapshot was taken before the
    // update, so the record it wrote is excluded by comparing the pipeline nobody
    // updated.
    let after = stored_pipelines(&shared);
    assert_eq!(
        after.len(),
        before.len(),
        "reading must not add or remove a pipeline, expected {} entries, got {after:?}",
        before.len()
    );
    let untouched = |rendered: &[String]| -> String {
        rendered
            .iter()
            .find(|line| line.contains(&later))
            .cloned()
            .unwrap_or_else(|| panic!("the store must hold {later}, got {rendered:?}"))
    };
    assert_eq!(
        untouched(&after),
        untouched(&before),
        "reading a pipeline through every phase must leave its record as its write left it, expected {:?}",
        untouched(&before)
    );
}
