// SPDX-License-Identifier: CC0-1.0

//! A stored harvest job references its origin endpoint's manifests BY NAME, so a write
//! that replaces the endpoint's manifest set is judged against the jobs that reference it,
//! driven over HTTP through the same router the emulator boots.
//!
//! `CreateHarvestJob` refuses a job that names a manifest the endpoint does not publish
//! (400 `ValidationException` / `HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT`), so the rule
//! asserted here is the same reference read from the other side: an `UpdateOriginEndpoint`
//! whose replacement manifest set would stop publishing a manifest a harvest job that has
//! NOT finished is harvesting is refused 409 `ConflictException` / `RESOURCE_IN_USE`
//! (`ConflictExceptionType` from `smithy_query(query="shape",
//! shape="ConflictExceptionType")`) and stores nothing, while an update that keeps those
//! manifests published is accepted. A job in a status it can no longer leave
//! (`CANCELLED`, `FAILED` — `HarvestJobStatus`) is a record of work that is over and pins
//! nothing.
//!
//! Each rule is asserted over a TABLE rather than over one request: the rows differ in
//! which manifest family the job harvests (and in the container type that serves it), in
//! HOW the replacement set treats the harvested manifest (renamed, dropped, the family
//! emptied, kept, kept beside new ones) and in the status the job is in when the update
//! arrives.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::harvested_manifest_in_use;

// The crate's shared test harness: the control-plane client with the error-envelope
// reader (`channel_lifecycle`) and the schedule clock every harvest-job suite dates its
// windows from (`rejection_rule_and_negative_paths`).
use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::epoch_seconds_from_now;

/// The hierarchy every row builds, all matching the model's `ResourceName` pattern
/// `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "hjr-group";
const CHANNEL: &str = "hjr-channel";
const ENDPOINT: &str = "hjr-endpoint";
const JOB: &str = "hjr-job";

/// The manifest the job harvests, and one the endpoint publishes beside it that no job
/// harvests — so a row can drop one without dropping the other.
const HARVESTED: &str = "index";
const SPARE: &str = "spare";

/// The statuses of the model's `HarvestJobStatus` this suite drives a job into.
const QUEUED: &str = "QUEUED";
const IN_PROGRESS: &str = "IN_PROGRESS";
const CANCELLED: &str = "CANCELLED";
const FAILED: &str = "FAILED";

/// How long a row waits for a transition the clock owes a job, and how often it looks.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const POLL_LIMIT: u32 = 1_200;

/// One harvestable manifest family: the member a harvest job names it under, the member
/// the origin endpoint publishes it under, a container type that serves it, and another
/// family of that same container — for the row that empties the harvested family and still
/// has to leave the endpoint publishing a manifest.
///
/// The harvestable families are the members of the model's `HarvestedManifests`
/// (`HlsManifests`, `LowLatencyHlsManifests`, `DashManifests`, from
/// `smithy_query(query="shape", shape="HarvestedManifests")`); which container type serves
/// which family is the model's own matrix (`TS_CONTAINER_TYPE_WITH_DASH_MANIFEST` and the
/// other `ValidationExceptionType` values naming a container/family pair), so a TS
/// endpoint serves the two HLS families and CMAF adds DASH.
#[derive(Debug, Clone, Copy)]
struct Family {
    member: &'static str,
    container: &'static str,
    other_member: &'static str,
}

/// The rows the manifest-set table below is run for, differing in the family the job
/// harvests and in the container type publishing it. `MssManifests` is absent on purpose:
/// `HarvestedManifests` carries no MSS family, so no harvest job can reference one.
const FAMILIES: &[Family] = &[
    Family {
        member: "HlsManifests",
        container: "TS",
        other_member: "LowLatencyHlsManifests",
    },
    Family {
        member: "HlsManifests",
        container: "CMAF",
        other_member: "DashManifests",
    },
    Family {
        member: "LowLatencyHlsManifests",
        container: "CMAF",
        other_member: "HlsManifests",
    },
    Family {
        member: "DashManifests",
        container: "CMAF",
        other_member: "HlsManifests",
    },
];

/// A list of manifest configurations carrying just the names — everything else an origin
/// endpoint's manifest needs the service materialises.
fn manifests(names: &[&str]) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| json!({"ManifestName": name}))
            .collect(),
    )
}

/// A `CreateOriginEndpoint` / `UpdateOriginEndpoint` body publishing `names` in `family`,
/// plus one manifest in the family's sibling when `with_other_family` is set.
fn endpoint_body(family: Family, names: &[&str], with_other_family: bool) -> Value {
    let mut body = json!({"ContainerType": family.container});
    if !names.is_empty() {
        body[family.member] = manifests(names);
    }
    if with_other_family {
        body[family.other_member] = manifests(&["sibling"]);
    }
    body
}

/// A `CreateHarvestJob` body harvesting `names` of `family` over the window
/// `[start, end)` (epoch seconds).
fn job_body(family: Family, names: &[&str], start: f64, end: f64) -> Value {
    let mut harvested = json!({});
    harvested[family.member] = manifests(names);
    json!({
        "HarvestedManifests": harvested,
        "ScheduleConfiguration": {"StartTime": start, "EndTime": end},
        "Destination": {
            "S3Destination": {"BucketName": "harvest-bucket", "DestinationPath": "out/"},
        },
    })
}

/// The harvest-job collection of this suite's endpoint.
fn jobs_path() -> String {
    format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob")
}

/// A fresh emulator whose endpoint publishes [`HARVESTED`] and [`SPARE`] in `family`, and
/// which holds one harvest job over `[start, end)` harvesting [`HARVESTED`] alone.
async fn emulator_with_job(case: &str, family: Family, start: f64, end: f64) -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannel expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            endpoint_body(family, &[HARVESTED, SPARE], false),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateOriginEndpoint expected 200, got {status} {body}"
    );
    let mut job = job_body(family, &[HARVESTED], start, end);
    job["HarvestJobName"] = json!(JOB);
    let (status, body) = emulator.call("POST", &jobs_path(), Some(job)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateHarvestJob expected 200, got {status} {body}"
    );
    assert_eq!(
        body["Status"], json!(QUEUED),
        "case {case:?}: a created job is QUEUED, got {body}"
    );
    emulator
}

/// `GetOriginEndpoint` on this suite's endpoint.
async fn read_endpoint(emulator: &Emulator, case: &str) -> Value {
    let (status, body) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetOriginEndpoint expected 200, got {status} {body}"
    );
    body
}

/// `GetHarvestJob` on this suite's job.
async fn read_job(emulator: &Emulator, case: &str) -> Value {
    let (status, body) = emulator
        .call("GET", &format!("{}/{JOB}", jobs_path()), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetHarvestJob expected 200, got {status} {body}"
    );
    body
}

/// The manifest names a response reports for one family, in the order it lists them.
fn published(body: &Value, member: &str) -> Vec<String> {
    body[member].as_array().map_or_else(Vec::new, |list| {
        list.iter()
            .map(|manifest| {
                manifest["ManifestName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    })
}

/// One replacement manifest set an update can carry for the harvested family.
#[derive(Debug, Clone, Copy)]
struct Replacement {
    label: &'static str,
    /// The names the update leaves the harvested family publishing.
    names: &'static [&'static str],
    /// Whether the update also publishes a manifest in the family's sibling — what lets a
    /// row empty the harvested family and still satisfy `NUM_MANIFESTS_LOW`.
    with_other_family: bool,
    /// The status the update must answer with: 409 when the replacement would stop
    /// publishing [`HARVESTED`], 200 when it keeps publishing it.
    expected_status: u16,
}

/// The replacement sets, differing in whether the harvested manifest survives and in how
/// the rest of the set changes around it.
const REPLACEMENTS: &[Replacement] = &[
    Replacement {
        label: "the harvested manifest is renamed",
        names: &["renamed", SPARE],
        with_other_family: false,
        expected_status: 409,
    },
    Replacement {
        label: "the harvested manifest is dropped and the one no job harvests is kept",
        names: &[SPARE],
        with_other_family: false,
        expected_status: 409,
    },
    Replacement {
        label: "the family the job harvests is emptied, another family taking its place",
        names: &[],
        with_other_family: true,
        expected_status: 409,
    },
    Replacement {
        label: "the harvested manifest is the only one left",
        names: &[HARVESTED],
        with_other_family: false,
        expected_status: 200,
    },
    Replacement {
        label: "the harvested manifest is kept and two more are added beside it",
        names: &[HARVESTED, SPARE, "extra"],
        with_other_family: false,
        expected_status: 200,
    },
    Replacement {
        label: "the harvested manifest is kept and another family is added",
        names: &[HARVESTED],
        with_other_family: true,
        expected_status: 200,
    },
];

/// RULE — a live harvest job's manifests have to stay published: an update that would
/// unpublish one is refused 409 `RESOURCE_IN_USE` and writes nothing; one that keeps them
/// is accepted.
#[tokio::test]
async fn an_update_may_not_unpublish_a_manifest_a_live_harvest_job_harvests() {
    let refusal = harvested_manifest_in_use();
    let expected_message = refusal.message.clone().unwrap_or_default();
    let expected_discriminator = refusal
        .conflict_exception_type
        .clone()
        .unwrap_or_default();
    for family in FAMILIES {
        for row in REPLACEMENTS {
            let case = format!(
                "{} on a {} endpoint publishing {}",
                row.label, family.container, family.member
            );
            let case = case.as_str();
            // A window that opens an hour from now: the job stays QUEUED — a live job —
            // for as long as the row runs, without the row waiting on any clock.
            let now = epoch_seconds_from_now(0.0).floor();
            let emulator = emulator_with_job(case, *family, now + 3600.0, now + 7200.0).await;
            let before = read_endpoint(&emulator, case).await;

            let (status, body) = emulator
                .update_endpoint(
                    GROUP,
                    CHANNEL,
                    ENDPOINT,
                    endpoint_body(*family, row.names, row.with_other_family),
                    &[],
                )
                .await;
            assert_eq!(
                status, row.expected_status,
                "case {case:?}: UpdateOriginEndpoint expected {}, got {status} {body}",
                row.expected_status
            );

            let after = read_endpoint(&emulator, case).await;
            if row.expected_status == 409 {
                assert!(
                    error_type(&body).contains("ConflictException"),
                    "case {case:?}: expected a ConflictException, got {body}"
                );
                assert_eq!(
                    body["ConflictExceptionType"],
                    json!(expected_discriminator),
                    "case {case:?}: expected {expected_discriminator}, got {body}"
                );
                assert_eq!(
                    body["Message"],
                    json!(expected_message),
                    "case {case:?}: expected the service's refusal sentence, got {body}"
                );
                // A refused update writes nothing: the whole endpoint reads back exactly
                // as it was, ETag and ModifiedAt included.
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update must leave the endpoint alone, got \
                     {after} where {before} was stored"
                );
            } else {
                assert_eq!(
                    published(&after, family.member),
                    row.names
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect::<Vec<_>>(),
                    "case {case:?}: the endpoint must publish what the update stored, got \
                     {after}"
                );
                assert_eq!(
                    after["ETag"], body["ETag"],
                    "case {case:?}: the read must report the tag the update answered with, \
                     got {after}"
                );
            }

            // Either way the invariant the create enforces holds again: the job still
            // reports the manifests it harvests, the endpoint still publishes them, and so
            // a second job harvesting exactly the same manifests is accepted rather than
            // refused as naming a manifest that is not published.
            let job = read_job(&emulator, case).await;
            assert_eq!(
                published(&job["HarvestedManifests"], family.member),
                vec![HARVESTED.to_string()],
                "case {case:?}: the job must still report the manifests it harvests, got \
                 {job}"
            );
            assert_eq!(
                job["Status"], json!(QUEUED),
                "case {case:?}: the job's own status is not moved by an update, got {job}"
            );
            let mut second = job_body(*family, &[HARVESTED], now + 3600.0, now + 7200.0);
            second["HarvestJobName"] = json!("hjr-job-again");
            let (status, created) = emulator.call("POST", &jobs_path(), Some(second)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the manifests the stored job harvests must still resolve on \
                 the endpoint, so a second job naming them expected 200, got {status} \
                 {created}"
            );
        }
    }
}

/// One status a harvest job can be in when the update arrives, and how it is reached.
#[derive(Debug, Clone, Copy)]
struct JobStatus {
    label: &'static str,
    /// The scheduled window, as offsets in seconds from the instant the row runs.
    start_offset: f64,
    end_offset: f64,
    /// Whether the row cancels the job once it is created.
    cancel: bool,
    /// The status the job is in when the update is sent.
    status: &'static str,
    /// The status an update that unpublishes the harvested manifest must answer with: a
    /// job that can still harvest pins the manifest, a finished one does not.
    expected_status: u16,
}

/// The statuses, reached through the job's own schedule and through `CancelHarvestJob`.
const JOB_STATUSES: &[JobStatus] = &[
    JobStatus {
        label: "a job whose window has not opened",
        start_offset: 3600.0,
        end_offset: 7200.0,
        cancel: false,
        status: QUEUED,
        expected_status: 409,
    },
    JobStatus {
        label: "a job whose window is open",
        start_offset: -10.0,
        end_offset: 3600.0,
        cancel: false,
        status: IN_PROGRESS,
        expected_status: 409,
    },
    JobStatus {
        label: "a job cancelled before its window opened",
        start_offset: 3600.0,
        end_offset: 7200.0,
        cancel: true,
        status: CANCELLED,
        expected_status: 200,
    },
    JobStatus {
        label: "a job whose window closed before it was created",
        start_offset: -7200.0,
        end_offset: -3600.0,
        cancel: false,
        status: FAILED,
        expected_status: 200,
    },
];

/// RULE, the other half — only a job that can still harvest holds the reference: once the
/// job has reached a status it cannot leave, the same update is accepted.
#[tokio::test]
async fn only_a_harvest_job_that_can_still_harvest_pins_the_endpoints_manifests() {
    // One family is enough here: the test above already varies the family, and what this
    // one varies is the job's status.
    let family = FAMILIES[0];
    for row in JOB_STATUSES {
        let case = row.label;
        let now = epoch_seconds_from_now(0.0).floor();
        let emulator = emulator_with_job(
            case,
            family,
            now + row.start_offset,
            now + row.end_offset,
        )
        .await;
        if row.cancel {
            let (status, cancelled) = emulator
                .call("PUT", &format!("{}/{JOB}", jobs_path()), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CancelHarvestJob expected 200, got {status} {cancelled}"
            );
        }
        // Nobody moves the status but the clock: the row waits for the one its schedule
        // owes the job rather than assuming it has already fallen due.
        await_status(&emulator, case, row.status).await;

        // The update that drops the harvested manifest, the same one every row sends.
        let (status, body) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                endpoint_body(family, &[SPARE], false),
                &[],
            )
            .await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: an update dropping the harvested manifest expected {}, got \
             {status} {body}",
            row.expected_status
        );

        let after = read_endpoint(&emulator, case).await;
        let expected_names = match row.expected_status {
            200 => vec![SPARE.to_string()],
            _ => vec![HARVESTED.to_string(), SPARE.to_string()],
        };
        assert_eq!(
            published(&after, family.member),
            expected_names,
            "case {case:?}: expected the endpoint to publish {expected_names:?}, got {after}"
        );
    }
}

/// Read the job until it reports `target`, so a row judges the status the clock owes
/// rather than racing it.
async fn await_status(emulator: &Emulator, case: &str, target: &str) {
    for _ in 0..POLL_LIMIT {
        let job = read_job(emulator, case).await;
        if job["Status"] == json!(target) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the job never reached {target}");
}
