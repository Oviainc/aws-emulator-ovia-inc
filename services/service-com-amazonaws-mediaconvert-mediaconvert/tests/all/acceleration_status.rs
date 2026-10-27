// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `Job$AccelerationStatus`.
//!
//! The rule under test, which every row below is one case of: a job's acceleration status
//! is derived from the acceleration mode the job ended up with, and is always reported —
//! `NOT_APPLICABLE` when that mode turns acceleration off ("For jobs that have Acceleration
//! (AccelerationMode) set to DISABLED, AccelerationStatus is always NOT_APPLICABLE"), and
//! the initial `IN_PROGRESS` for every mode that asks for accelerated transcoding
//! ("AccelerationStatus is IN_PROGRESS initially, while the service determines whether the
//! input files and job settings are compatible with accelerated transcoding"). The status
//! is a projection of the stored job, so every reader of that job reports the same value,
//! and reading one never changes it.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the one
//! `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::{
    ACCELERATION_MODE_VALUES, ACCELERATION_STATUS_VALUES,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{
    settings as job_settings, Emulator, Reply, PAST_THE_SETTLE, ROLE, SHORT_SETTLE,
};

/// The two statuses a job that nothing has decided about yet can report, per
/// `Job$AccelerationStatus`. Both are members of the model's own `AccelerationStatus` value
/// set ([`ACCELERATION_STATUS_VALUES`]), which every assertion below also checks against,
/// so neither this suite's expectations nor the service's answers can drift from it.
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
const IN_PROGRESS: &str = "IN_PROGRESS";

/// Every `AccelerationMode` paired with the `AccelerationStatus` the model names for a job
/// that has just been submitted with it. The pairing is what this suite asserts; the
/// catalogue it must span is the model's own [`ACCELERATION_MODE_VALUES`], checked by
/// [`assert_the_pairing_spans_the_model`].
const MODE_STATUSES: [(&str, &str); 3] = [
    ("DISABLED", NOT_APPLICABLE),
    ("ENABLED", IN_PROGRESS),
    ("PREFERRED", IN_PROGRESS),
];

/// The status [`MODE_STATUSES`] pairs with `mode`.
fn status_for_mode(mode: &str) -> &'static str {
    MODE_STATUSES
        .iter()
        .find(|(named, _)| *named == mode)
        .map(|(_, status)| *status)
        .unwrap_or_else(|| panic!("the suite must pair the AccelerationMode {mode:?}"))
}

/// Every published `AccelerationMode` is paired, and nothing else is: the suite's class is
/// the model's, so it can neither fall behind the value set nor invent a mode outside it.
fn assert_the_pairing_spans_the_model() {
    let paired: Vec<&str> = MODE_STATUSES.iter().map(|(mode, _)| *mode).collect();
    assert_eq!(
        paired.len(),
        ACCELERATION_MODE_VALUES.len(),
        "the suite pairs {paired:?}, which is not the published AccelerationMode value set \
         ({ACCELERATION_MODE_VALUES:?})"
    );
    for mode in ACCELERATION_MODE_VALUES {
        assert!(
            paired.contains(mode),
            "the suite must pair the AccelerationMode value {mode:?}, and pairs only \
             {paired:?}"
        );
    }
}

/// A jobs-query completion span short enough for a case to wait past it, and a wait
/// comfortably past it.
const SHORT_QUERY: Duration = Duration::from_millis(20);
const PAST_THE_QUERY: Duration = Duration::from_millis(150);

/// One row: the acceleration mode a job ends up with, how the request gets it there, and
/// the status every projection of that job must then report.
struct Row {
    case: &'static str,
    /// The `CreateJobTemplate` body this row needs first, if any.
    template: Option<Value>,
    /// The `CreateJob` body, minus the input the harness makes unique per row.
    job: Value,
    /// The `AccelerationMode` the stored job carries, which the projections also report.
    mode: &'static str,
    /// The `AccelerationStatus` every projection must report.
    status: &'static str,
}

/// A `JobTemplateSettings` with one output group holding one output.
fn template_settings() -> Value {
    json!({
        "outputGroups": [{
            "name": "File Group",
            "outputs": [{
                "nameModifier": "_tpl",
                "containerSettings": { "container": "MP4" }
            }],
            "outputGroupSettings": {
                "type": "FILE_GROUP_SETTINGS",
                "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
            }
        }]
    })
}

/// The rows: the mode stated on the request (each published `AccelerationMode` value), the
/// mode the service applies when the request states none, and the mode inherited from a job
/// template — both inherited and overridden — so the table spans every way a job can reach
/// a mode rather than only the one way the request can.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no accelerationSettings, so the service's own mode",
            template: None,
            job: json!({ "role": ROLE }),
            mode: "DISABLED",
            status: NOT_APPLICABLE,
        },
        Row {
            case: "mode DISABLED",
            template: None,
            job: json!({ "role": ROLE, "accelerationSettings": { "mode": "DISABLED" } }),
            mode: "DISABLED",
            status: NOT_APPLICABLE,
        },
        Row {
            case: "mode ENABLED",
            template: None,
            job: json!({ "role": ROLE, "accelerationSettings": { "mode": "ENABLED" } }),
            mode: "ENABLED",
            status: IN_PROGRESS,
        },
        Row {
            case: "mode PREFERRED",
            template: None,
            job: json!({ "role": ROLE, "accelerationSettings": { "mode": "PREFERRED" } }),
            mode: "PREFERRED",
            status: IN_PROGRESS,
        },
        Row {
            case: "mode PREFERRED inherited from a job template",
            template: Some(json!({
                "name": "accel-preferred",
                "accelerationSettings": { "mode": "PREFERRED" },
                "settings": template_settings(),
            })),
            job: json!({ "role": ROLE, "jobTemplate": "accel-preferred" }),
            mode: "PREFERRED",
            status: IN_PROGRESS,
        },
        Row {
            case: "a template's ENABLED overridden by the request's DISABLED",
            template: Some(json!({
                "name": "accel-enabled",
                "accelerationSettings": { "mode": "ENABLED" },
                "settings": template_settings(),
            })),
            job: json!({
                "role": ROLE,
                "jobTemplate": "accel-enabled",
                "accelerationSettings": { "mode": "DISABLED" },
            }),
            mode: "DISABLED",
            status: NOT_APPLICABLE,
        },
    ]
}

/// The `accelerationStatus` of one `Job` projection, or `Value::Null` when it is absent.
fn status_of(view: &Value) -> &Value {
    view.pointer("/accelerationStatus").unwrap_or(&Value::Null)
}

/// Assert one projection reports `want`, and that the value it reports is a published
/// `AccelerationStatus`.
fn assert_status(case: &str, where_from: &str, view: &Value, want: &str) {
    let got = status_of(view);
    assert_eq!(
        got,
        &json!(want),
        "case {case:?}: expected accelerationStatus {want:?} in the {where_from} \
         projection, got {got} in {view}"
    );
    let got = got.as_str().unwrap_or_default();
    assert!(
        ACCELERATION_STATUS_VALUES.contains(&got),
        "case {case:?}: the {where_from} projection reported {got:?}, which is not an \
         AccelerationStatus value ({ACCELERATION_STATUS_VALUES:?})"
    );
}

/// The projection of the job `id` in a reply carrying a `jobs` list.
fn job_in(case: &str, where_from: &str, reply: &Reply, id: &str) -> Value {
    reply
        .at("/jobs")
        .as_array()
        .into_iter()
        .flatten()
        .find(|job| job.pointer("/id").and_then(Value::as_str) == Some(id))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the {where_from} projection must carry the job {id:?}, got {}",
                reply.body
            )
        })
}

/// Every mode a job can end up with reports the status the model names for it, and every
/// reader of the job — `CreateJob`, `GetJob`, `ListJobs`, `SearchJobs` and a jobs query's
/// results — reports that same status.
#[tokio::test]
async fn every_acceleration_mode_reports_the_status_the_model_names() {
    assert_the_pairing_spans_the_model();
    // The rows span the WHOLE paired catalogue, so the table cannot fall behind the class
    // the rule generalises over.
    for (mode, _) in MODE_STATUSES {
        let covered: Vec<&str> = rows().iter().map(|row| row.mode).collect();
        assert!(
            covered.contains(&mode),
            "the table must cover the AccelerationMode value {mode:?}, and covers \
             only {covered:?}"
        );
    }

    let emulator = Emulator::new();
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .jobs_query_completes_after = SHORT_QUERY;

    for (index, row) in rows().into_iter().enumerate() {
        if let Some(template) = row.template {
            let created = emulator.create_job_template(template.clone()).await;
            assert_eq!(
                created.status, 201,
                "case {:?}: the row's job template must be created, got {} with body {}",
                row.case, created.status, created.body
            );
        }

        let mut body = row.job;
        body["settings"] = job_settings(
            &format!("s3://example-bucket/in/accel-{index}.mp4"),
            &format!("_{index}"),
        );
        let created = emulator.create_job(body).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = created.string("/job/id");
        // The row's expectation is the rule's, not a value picked per row.
        assert_eq!(
            row.status,
            status_for_mode(row.mode),
            "case {:?}: the row expects {:?} for mode {:?}, which is not what the rule \
             gives that mode",
            row.case,
            row.status,
            row.mode
        );

        // The mode the row says the job ended up with really is the stored one, so the
        // status below is asserted against that mode and not against the request's text.
        let mode = created.at("/job/accelerationSettings/mode");
        assert_eq!(
            mode,
            &json!(row.mode),
            "case {:?}: expected mode {:?}, got {mode} in {}",
            row.case,
            row.mode,
            created.body
        );

        let fetched = emulator.get_job(&id).await;
        let listed = emulator.send("GET", "/2017-08-29/jobs", None).await;
        let searched = emulator.send("GET", "/2017-08-29/search", None).await;

        let started = emulator
            .send("POST", "/2017-08-29/jobsQueries", Some(json!({})))
            .await;
        assert_eq!(
            started.status, 201,
            "case {:?}: StartJobsQuery must answer HTTP 201, got {} with body {}",
            row.case, started.status, started.body
        );
        tokio::time::sleep(PAST_THE_QUERY).await;
        let results = emulator
            .send(
                "GET",
                &format!("/2017-08-29/jobsQueries/{}", started.string("/id")),
                None,
            )
            .await;

        for (where_from, view) in [
            ("create", created.at("/job").clone()),
            ("get", fetched.at("/job").clone()),
            ("list", job_in(row.case, "list", &listed, &id)),
            ("search", job_in(row.case, "search", &searched, &id)),
            (
                "jobs query results",
                job_in(row.case, "jobs query results", &results, &id),
            ),
        ] {
            assert_status(row.case, where_from, &view, row.status);
        }
    }
}

/// The status a job reports comes from its mode, not from how far the job has got: the two
/// modes that ask for acceleration still report the initial `IN_PROGRESS` after the job has
/// left `SUBMITTED`, because nothing has decided whether the job is compatible with
/// accelerated transcoding, and repeated reads never move it.
#[tokio::test]
async fn the_status_survives_the_job_leaving_submitted_and_repeated_reads() {
    let emulator = Emulator::with_settle_span(SHORT_SETTLE);

    let mut jobs = Vec::new();
    assert_the_pairing_spans_the_model();
    for (index, (mode, want)) in MODE_STATUSES.into_iter().enumerate() {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "accelerationSettings": { "mode": mode },
                "settings": job_settings(
                    &format!("s3://example-bucket/in/settle-{index}.mp4"),
                    &format!("_{index}"),
                ),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {mode:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_status(mode, "create", created.at("/job"), want);
        jobs.push((mode, want, created.string("/job/id")));
    }

    tokio::time::sleep(PAST_THE_SETTLE).await;

    for (mode, want, id) in &jobs {
        let first = emulator.get_job(id).await;
        // The job really has left SUBMITTED, so this is a reader looking at a job whose own
        // status moved under it.
        let job_status = first.string("/job/status");
        assert_ne!(
            job_status, "SUBMITTED",
            "case {mode:?}: the job must have left SUBMITTED before the status is \
             re-read, got {}",
            first.body
        );
        assert_status(mode, "get past the settle", first.at("/job"), want);
        let second = emulator.get_job(id).await;
        assert_status(mode, "second get", second.at("/job"), want);
        assert_eq!(
            status_of(first.at("/job")),
            status_of(second.at("/job")),
            "case {mode:?}: reading the job must not change its accelerationStatus, got {} \
             then {}",
            first.body,
            second.body
        );
    }
}
