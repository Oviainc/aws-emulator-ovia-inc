// SPDX-License-Identifier: CC0-1.0

//! Which handlers drive the job-lifecycle clock sweep, and which leave it alone.
//!
//! The rule under test: a handler materialises the status changes the clock has made due
//! only when its own request path reads or writes the state that sweep touches — a job's
//! status or timing, a queue projection's job counts, a jobs query, a share attempt or a
//! queue hop. A door onto a resource the clock owes nothing (a preset, a job template, a
//! record's tag set) answers from that resource alone and leaves the job lifecycle
//! exactly as it found it, and no caller can observe the difference, because every door
//! onto job state does settle.
//!
//! Each case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — and then reads the STORE, because what a clock sweep
//! moved is a property of the stored job rather than of the body a non-job door returns.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    DEFAULT_QUEUE_NAME, JOB_STATUS_ERROR, JOB_STATUS_SUBMITTED,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

// The router, the SigV4 scope, the reply decoding, the ARN helper and the request helpers
// are the crate's ONE test harness, exported by `job_lifecycle`: this suite drives the
// same wire path through it rather than keeping a second copy of it.
use super::job_lifecycle::{
    arn, settings as job_settings, Emulator, PAST_THE_SETTLE, ROLE, SHORT_SETTLE,
};
// One `JobTemplateSettings` fixture for every job-template suite, and one `@httpLabel`
// encoding for every suite that addresses a resource by ARN.
use super::job_template_crud::settings as template_settings;
use super::resource_tagging::encode_label;

/// The preset the fixtures create, which the preset and tag doors address.
const FIXTURE_PRESET: &str = "clock-scope-preset";

/// The job template the fixtures create, which `GetJobTemplate` reads.
const FIXTURE_TEMPLATE: &str = "clock-scope-template";

/// A tag key the fixtures put on every resource, so an `UntagResource` case removes a key
/// that is really there.
const FIXTURE_TAG_KEY: &str = "crew";

/// Which stored resource a tag door's `Arn` names: a tag set hangs off a record, and the
/// rule has to hold for a JOB's tag set too — `settle` moves a job's status, never its
/// tags.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    Preset,
    Job,
}

/// A door onto a resource the job-lifecycle clock owes nothing: the creates, reads,
/// updates, deletes and listings of the preset and job-template resources, and the three
/// tag doors, each addressing a preset and a job in turn.
#[derive(Debug, Clone, Copy)]
enum Door {
    CreatePreset,
    GetPreset,
    UpdatePreset,
    ListPresets,
    DeletePreset,
    CreateJobTemplate,
    GetJobTemplate,
    TagResource(Addressed),
    ListTagsForResource(Addressed),
    UntagResource(Addressed),
}

/// One HTTP request, and the status the door answers when it succeeds.
struct Call {
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
}

impl Door {
    /// The request this door is driven with. `job_id` is the seeded job, which a tag door
    /// addressing a job needs for its ARN.
    fn call(self, job_id: &str) -> Call {
        let presets = "/2017-08-29/presets";
        let templates = "/2017-08-29/jobTemplates";
        match self {
            // A name no fixture used, so the create really stores a record.
            Self::CreatePreset => Call {
                method: "POST",
                uri: presets.to_string(),
                body: Some(json!({
                    "name": "clock-scope-created-preset",
                    "settings": { "containerSettings": { "container": "MP4" } }
                })),
                status: 201,
            },
            Self::GetPreset => Call {
                method: "GET",
                uri: format!("{presets}/{FIXTURE_PRESET}"),
                body: None,
                status: 200,
            },
            Self::UpdatePreset => Call {
                method: "PUT",
                uri: format!("{presets}/{FIXTURE_PRESET}"),
                body: Some(json!({ "description": "updated by the clock-scope case" })),
                status: 200,
            },
            Self::ListPresets => Call {
                method: "GET",
                uri: presets.to_string(),
                body: None,
                status: 200,
            },
            Self::DeletePreset => Call {
                method: "DELETE",
                uri: format!("{presets}/{FIXTURE_PRESET}"),
                body: None,
                status: 202,
            },
            Self::CreateJobTemplate => Call {
                method: "POST",
                uri: templates.to_string(),
                body: Some(json!({
                    "name": "clock-scope-created-template",
                    "settings": template_settings(1, 1)
                })),
                status: 201,
            },
            Self::GetJobTemplate => Call {
                method: "GET",
                uri: format!("{templates}/{FIXTURE_TEMPLATE}"),
                body: None,
                status: 200,
            },
            Self::TagResource(addressed) => Call {
                method: "POST",
                uri: "/2017-08-29/tags".to_string(),
                body: Some(json!({
                    "arn": addressed_arn(addressed, job_id),
                    "tags": { "shift": "late" }
                })),
                status: 200,
            },
            Self::ListTagsForResource(addressed) => Call {
                method: "GET",
                uri: format!(
                    "/2017-08-29/tags/{}",
                    encode_label(&addressed_arn(addressed, job_id))
                ),
                body: None,
                status: 200,
            },
            Self::UntagResource(addressed) => Call {
                method: "PUT",
                uri: format!(
                    "/2017-08-29/tags/{}",
                    encode_label(&addressed_arn(addressed, job_id))
                ),
                body: Some(json!({ "tagKeys": [FIXTURE_TAG_KEY] })),
                status: 200,
            },
        }
    }
}

/// The ARN of the resource a tag door addresses.
fn addressed_arn(addressed: Addressed, job_id: &str) -> String {
    match addressed {
        Addressed::Preset => arn(&format!("presets/{FIXTURE_PRESET}")),
        Addressed::Job => arn(&format!("jobs/{job_id}")),
    }
}

/// A job held in `SUBMITTED` with a tag set, and its id.
async fn seeded_job(emulator: &Emulator) -> String {
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": job_settings("s3://example-bucket/in/a.mp4", "_a"),
            "tags": { FIXTURE_TAG_KEY: "alpha" }
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "fixture job: expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");
    assert!(
        !id.is_empty(),
        "fixture job: expected a minted id, got {}",
        created.body
    );
    id
}

/// The preset and the job template every non-job door addresses, each carrying a tag so a
/// tag door has a set to work on.
async fn seeded_resources(emulator: &Emulator) {
    let preset = emulator
        .create_preset(json!({
            "name": FIXTURE_PRESET,
            "settings": { "containerSettings": { "container": "MP4" } },
            "tags": { FIXTURE_TAG_KEY: "alpha" }
        }))
        .await;
    assert_eq!(
        preset.status, 201,
        "fixture preset: expected HTTP 201, got {} with body {}",
        preset.status, preset.body
    );
    let template = emulator
        .create_job_template(json!({
            "name": FIXTURE_TEMPLATE,
            "settings": template_settings(1, 1)
        }))
        .await;
    assert_eq!(
        template.status, 201,
        "fixture job template: expected HTTP 201, got {} with body {}",
        template.status, template.body
    );
}

/// The stored job `id`'s status and whether the clock still owes it its exit from
/// `SUBMITTED`, read out of the one store the handlers write to.
fn stored_job(emulator: &Emulator, case: &str, id: &str) -> (String, bool) {
    let store = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>();
    let guard = store.lock();
    let Some(job) = guard.jobs.get(id) else {
        panic!("case {case:?}: expected the seeded job {id:?} to still be stored");
    };
    (job.status.clone(), job.settles_at.is_some())
}

/// A door onto a resource the job-lifecycle clock owes nothing does not drive the sweep:
/// after the seeded job's exit from `SUBMITTED` has come due, the door answers from its
/// own resource and the stored job is left `SUBMITTED` with that exit still owed.
///
/// The table spans the class: the verb (create, read, update, delete, list, tag, untag),
/// the resource the door is onto (preset, job template, tag set) and — for the tag doors,
/// which reach a tag set hanging off ANY record — the kind of resource addressed, a
/// preset and a job in turn.
#[tokio::test]
async fn a_door_onto_a_resource_the_clock_owes_nothing_leaves_the_job_sweep_alone() {
    let doors = [
        Door::CreatePreset,
        Door::GetPreset,
        Door::UpdatePreset,
        Door::ListPresets,
        Door::DeletePreset,
        Door::CreateJobTemplate,
        Door::GetJobTemplate,
        Door::TagResource(Addressed::Preset),
        Door::TagResource(Addressed::Job),
        Door::ListTagsForResource(Addressed::Preset),
        Door::ListTagsForResource(Addressed::Job),
        Door::UntagResource(Addressed::Preset),
        Door::UntagResource(Addressed::Job),
    ];

    for door in doors {
        let case = format!("{door:?}");
        // The span is state, so the case pins it instead of racing the real clock.
        let emulator = Emulator::with_settle_span(SHORT_SETTLE);
        let job_id = seeded_job(&emulator).await;
        seeded_resources(&emulator).await;
        // Past the span: the job's exit from `SUBMITTED` is now due, so any handler that
        // drove the sweep would move it.
        tokio::time::sleep(PAST_THE_SETTLE).await;

        let call = door.call(&job_id);
        let reply = emulator.send(call.method, &call.uri, call.body).await;
        assert_eq!(
            reply.status, call.status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            call.status, reply.status, reply.body
        );

        let (status, exit_still_owed) = stored_job(&emulator, &case, &job_id);
        assert_eq!(
            status, JOB_STATUS_SUBMITTED,
            "case {case:?}: expected the stored job to be left {JOB_STATUS_SUBMITTED}, got \
             {status:?}"
        );
        assert!(
            exit_still_owed,
            "case {case:?}: expected the job's due exit from {JOB_STATUS_SUBMITTED} to be \
             still owed, got settles_at cleared"
        );

        // And no caller can observe that: the first door onto job state still answers the
        // status the clock had made due, because every such door settles.
        let read = emulator.get_job(&job_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetJob, got {} with body {}",
            read.status, read.body
        );
        let reported = read.string("/job/status");
        assert_eq!(
            reported, JOB_STATUS_ERROR,
            "case {case:?}: expected GetJob to report {JOB_STATUS_ERROR} once the exit was \
             due, got {reported:?} in {}",
            read.body
        );
    }
}

/// The other half of the rule: a door whose own response reads job state DOES drive the
/// sweep, so the change the clock had made due is materialised in the store by the call
/// itself.
///
/// The rows differ in how the door reaches job state: one job by id, the whole job set,
/// the job set narrowed by a search, and a queue projection whose counts are derived from
/// the jobs in it.
#[tokio::test]
async fn a_door_onto_job_state_drives_the_job_sweep() {
    let doors = [
        ("GetJob", "/2017-08-29/jobs/{id}".to_string()),
        ("ListJobs", "/2017-08-29/jobs".to_string()),
        (
            "SearchJobs",
            "/2017-08-29/search?inputFile=.mp4".to_string(),
        ),
        (
            "GetQueue",
            format!("/2017-08-29/queues/{DEFAULT_QUEUE_NAME}"),
        ),
    ];

    for (case, uri) in doors {
        let emulator = Emulator::with_settle_span(SHORT_SETTLE);
        let job_id = seeded_job(&emulator).await;
        tokio::time::sleep(PAST_THE_SETTLE).await;

        let reply = emulator
            .send("GET", &uri.replace("{id}", &job_id), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );

        let (status, exit_still_owed) = stored_job(&emulator, case, &job_id);
        assert_eq!(
            status, JOB_STATUS_ERROR,
            "case {case:?}: expected the due exit from {JOB_STATUS_SUBMITTED} to be \
             materialised as {JOB_STATUS_ERROR}, got {status:?}"
        );
        assert!(
            !exit_still_owed,
            "case {case:?}: expected the settled job to owe the clock nothing further, got \
             settles_at still set"
        );
    }
}
