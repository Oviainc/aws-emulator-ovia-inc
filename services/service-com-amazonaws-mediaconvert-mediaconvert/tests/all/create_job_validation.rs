// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what `CreateJob` refuses: job settings that describe no
//! transcode, and a resource reference that resolves to nothing.
//!
//! The two rules every case below is one row of:
//!
//! * a submitted job's EFFECTIVE settings graph — the request's own `Settings` plus
//!   whatever the job template it references supplies — must carry at least one `Inputs`
//!   entry (the source the transcode reads) and at least one `OutputGroups` entry (the
//!   outputs it writes), or the request is refused with `BadRequestException` (400) and
//!   nothing is stored;
//! * every member of the request that references another MediaConvert resource by name or
//!   by ARN — the job's `Queue`, its `JobTemplate`, the `Queue` of each of its hop
//!   destinations — must name a resource the store holds, or the request is refused with
//!   `NotFoundException` (404) naming the member the reference came from, and nothing is
//!   stored.
//!
//! The members and their documented purpose come from the model:
//! `smithy_query(query="shape",shape="JobSettings",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! and `smithy_query(query="operation",operation="CreateJob", …)` for the two errors.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call.

use serde_json::{json, Value};

use super::job_lifecycle::{arn, settings as job_settings, Emulator, Reply, ROLE};
use super::job_template_crud::settings as template_settings;

/// A queue the fixtures create, so a hop destination can reference a real queue.
const FIXTURE_QUEUE: &str = "hop-queue";

/// One output group, the same shape the job fixture builds, so a row that needs outputs
/// without inputs does not hand-roll a second graph.
fn one_output_group() -> Value {
    job_settings("s3://example-bucket/in/ignored.mp4", "_a")["outputGroups"].clone()
}

/// How many jobs the store holds, read back through `ListJobs`.
///
/// A refusal must leave the store as it was, and this is how every refusal row checks it:
/// through the API, never by reaching into the state struct.
async fn stored_jobs(emulator: &Emulator) -> usize {
    let listed = emulator.send("GET", "/2017-08-29/jobs", None).await;
    assert_eq!(
        listed.status, 200,
        "ListJobs must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed.at("/jobs").as_array().map_or(usize::MAX, Vec::len)
}

/// The `message` of an error body.
fn message(reply: &Reply) -> String {
    reply.string("/message")
}

/// Assert a reply is the modelled refusal `status`/`error_type` names.
fn assert_refusal(case: &str, reply: &Reply, status: u16, error_type: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(error_type),
        "case {case:?}: expected {error_type}, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
}

/// One row of the settings table: a `Settings` graph that describes no transcode, and the
/// settings member the refusal must name as the one with no entry.
struct NoTranscode {
    case: &'static str,
    settings: Value,
    names: &'static str,
}

/// A job whose settings describe neither a source nor outputs is a bad request, and no job
/// is stored.
///
/// The table spans the class along what the rule generalises over: which of the two lists
/// is missing, whether it is absent or present-but-empty, how many entries the other list
/// has, and a graph that carries other `JobSettings` members but neither end of the
/// transcode.
#[tokio::test]
async fn job_settings_that_describe_no_transcode_are_a_bad_request() {
    let rows = vec![
        NoTranscode {
            case: "settings with no member at all",
            settings: json!({}),
            names: "Inputs",
        },
        NoTranscode {
            case: "an empty inputs list",
            settings: json!({ "inputs": [] }),
            names: "Inputs",
        },
        NoTranscode {
            case: "both lists present and empty",
            settings: json!({ "inputs": [], "outputGroups": [] }),
            names: "Inputs",
        },
        NoTranscode {
            case: "one output group but no inputs",
            settings: json!({ "outputGroups": one_output_group() }),
            names: "Inputs",
        },
        NoTranscode {
            case: "settings carrying only members that describe neither end",
            settings: json!({
                "adAvailOffset": 100,
                "timecodeConfig": { "source": "ZEROBASED" },
                "followSource": 1
            }),
            names: "Inputs",
        },
        NoTranscode {
            case: "one input but no output groups",
            settings: json!({ "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }] }),
            names: "OutputGroups",
        },
        NoTranscode {
            case: "two inputs and an empty output group list",
            settings: json!({
                "inputs": [
                    { "fileInput": "s3://example-bucket/in/a.mp4" },
                    { "fileInput": "s3://example-bucket/in/b.mov" }
                ],
                "outputGroups": []
            }),
            names: "OutputGroups",
        },
        NoTranscode {
            case: "a generated-video input with no output groups",
            settings: json!({ "inputs": [{ "videoGenerator": { "duration": 5_000 } }] }),
            names: "OutputGroups",
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        let NoTranscode {
            case,
            settings,
            names,
        } = row;
        let reply = emulator
            .create_job(json!({ "role": ROLE, "settings": settings }))
            .await;
        assert_refusal(case, &reply, 400, "BadRequestException");
        let message = message(&reply);
        assert!(
            message.contains(names),
            "case {case:?}: the refusal must name the settings member with no entry \
             ({names}), got message {message:?}"
        );
    }

    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored, 0,
        "after {} refused submissions the store must hold no job, got {stored}",
        rows.len()
    );
}

/// One row of the accepted-settings table: a graph that DOES describe a transcode.
struct Accepted {
    case: &'static str,
    /// Merged into the request on top of `Role`.
    request: Value,
    /// The number of `Inputs` entries the stored job must report.
    inputs: usize,
}

/// A job whose settings describe a source and outputs is accepted, whatever form each end
/// takes, and reads back through `GetJob`.
///
/// The positive control of the rule above, spanning the same axes: the number of inputs and
/// of output groups, the kind of source (a file input, a generated video), and where the
/// outputs come from (the request, or the job template the request references).
#[tokio::test]
async fn job_settings_that_describe_a_transcode_are_accepted() {
    let emulator = Emulator::new();
    // The template the last row inherits its output groups from: one input template and
    // one output group.
    let template = emulator
        .create_job_template(json!({ "name": "outputs-from-here", "settings": template_settings(1, 1) }))
        .await;
    assert_eq!(
        template.status, 201,
        "the fixture job template must be created: got {} with body {}",
        template.status, template.body
    );

    let rows = vec![
        Accepted {
            case: "one file input and one output group",
            request: json!({ "settings": job_settings("s3://example-bucket/in/a.mp4", "_a") }),
            inputs: 1,
        },
        Accepted {
            case: "two file inputs and two output groups",
            request: json!({
                "settings": {
                    "inputs": [
                        { "fileInput": "s3://example-bucket/in/a.mp4" },
                        { "fileInput": "s3://example-bucket/in/b.mov" }
                    ],
                    "outputGroups": [
                        one_output_group()[0].clone(),
                        one_output_group()[0].clone()
                    ]
                }
            }),
            inputs: 2,
        },
        Accepted {
            case: "a generated-video input and one output group",
            request: json!({
                "settings": {
                    "inputs": [{ "videoGenerator": { "duration": 5_000 } }],
                    "outputGroups": one_output_group()
                }
            }),
            inputs: 1,
        },
        Accepted {
            case: "inputs from the request, output groups from the referenced template",
            request: json!({
                "jobTemplate": "outputs-from-here",
                "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/c.mkv" }] }
            }),
            inputs: 1,
        },
    ];

    for row in &rows {
        let Accepted {
            case,
            request,
            inputs,
        } = row;
        let mut body = json!({ "role": ROLE });
        if let (Some(body), Some(members)) = (body.as_object_mut(), request.as_object()) {
            for (member, value) in members {
                body.insert(member.clone(), value.clone());
            }
        }
        let created = emulator.create_job(body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let counted = created
            .at("/job/settings/inputs")
            .as_array()
            .map_or(usize::MAX, Vec::len);
        assert_eq!(
            counted, *inputs,
            "case {case:?}: expected {inputs} stored inputs, got {counted} in {}",
            created.body
        );

        // Round trip: the stored job reads back through `GetJob` exactly as created.
        let id = created.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJob HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/job"),
            created.at("/job"),
            "case {case:?}: GetJob must return what CreateJob persisted, got {}",
            fetched.body
        );
    }

    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored,
        rows.len(),
        "every accepted submission must be stored: expected {}, got {stored}",
        rows.len()
    );
}

/// One row of the reference table: the value of a reference member, and what the service
/// answers for it.
struct Reference {
    case: &'static str,
    /// The members merged into an otherwise valid `CreateJob` request.
    extras: Value,
    status: u16,
    error_type: &'static str,
    /// The `jobTemplate` the stored job must report, for an accepted row that names one.
    job_template: Option<&'static str>,
}

/// A `JobTemplate` reference resolves against the job-template store: a template the store
/// holds is accepted in either form it can be named in, and one it does not hold is not
/// found — exactly as an unknown `Queue` already is.
///
/// The table spans the class: the two forms a reference comes in (name, ARN), a built-in
/// template as well as a caller-created one, a name nothing ever held, an ARN of one, a
/// template that was deleted, and the empty reference.
#[tokio::test]
async fn a_job_template_reference_must_resolve_to_a_stored_template() {
    let emulator = Emulator::new();
    let created = emulator
        .create_job_template(json!({ "name": "live-template", "settings": template_settings(1, 1) }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job template must be created: got {} with body {}",
        created.status, created.body
    );

    // A template that existed and was then deleted: its name resolves to nothing, which is
    // the same refusal as a name nothing ever held.
    let doomed = emulator
        .create_job_template(json!({ "name": "gone-template", "settings": template_settings(1, 1) }))
        .await;
    assert_eq!(
        doomed.status, 201,
        "the deleted-template fixture must be created: got {} with body {}",
        doomed.status, doomed.body
    );
    let deleted = emulator.delete_job_template("gone-template").await;
    assert_eq!(
        deleted.status, 202,
        "the deleted-template fixture must be deleted: got {} with body {}",
        deleted.status, deleted.body
    );

    // The built-in templates come from the store's own catalogue, never a hard-coded name.
    let listed = emulator.list_job_templates("listBy=SYSTEM").await;
    assert_eq!(
        listed.status, 200,
        "ListJobTemplates must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    let system = listed.string("/jobTemplates/0/name");
    assert!(
        !system.is_empty(),
        "the store must hold at least one built-in template, got body {}",
        listed.body
    );

    let rows = vec![
        Reference {
            case: "a stored template by name",
            extras: json!({ "jobTemplate": "live-template" }),
            status: 201,
            error_type: "",
            job_template: Some("live-template"),
        },
        Reference {
            case: "the same template by ARN",
            extras: json!({ "jobTemplate": arn("jobTemplates/live-template") }),
            status: 201,
            error_type: "",
            // A reference resolves to the template's NAME, which is the store's key for it.
            job_template: Some("live-template"),
        },
        Reference {
            case: "no template at all",
            extras: json!({}),
            status: 201,
            error_type: "",
            job_template: None,
        },
        Reference {
            case: "a name no template holds",
            extras: json!({ "jobTemplate": "no-such-template" }),
            status: 404,
            error_type: "NotFoundException",
            job_template: None,
        },
        Reference {
            case: "an ARN no template holds",
            extras: json!({ "jobTemplate": arn("jobTemplates/no-such-template") }),
            status: 404,
            error_type: "NotFoundException",
            job_template: None,
        },
        Reference {
            case: "a template that was deleted",
            extras: json!({ "jobTemplate": "gone-template" }),
            status: 404,
            error_type: "NotFoundException",
            job_template: None,
        },
        Reference {
            case: "an empty reference",
            extras: json!({ "jobTemplate": "" }),
            status: 400,
            error_type: "BadRequestException",
            job_template: None,
        },
    ];

    let mut accepted = 0;
    for row in &rows {
        run_reference_row(&emulator, row).await;
        if row.status == 201 {
            accepted += 1;
        }
    }

    // The built-in catalogue resolves too, under whatever name the store published.
    let built_in = Reference {
        case: "a built-in template, by the name the store published",
        extras: json!({ "jobTemplate": system.clone() }),
        status: 201,
        error_type: "",
        job_template: None,
    };
    let reply = emulator.create_job(reference_request(&built_in)).await;
    assert_eq!(
        reply.status, 201,
        "case {:?}: expected HTTP 201, got {} with body {}",
        built_in.case, reply.status, reply.body
    );
    assert_eq!(
        reply.string("/job/jobTemplate"),
        system,
        "case {:?}: the stored job must name the template it was created from, got {}",
        built_in.case,
        reply.body
    );
    accepted += 1;

    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored, accepted,
        "only the accepted submissions may be stored: expected {accepted}, got {stored}"
    );
}

/// An otherwise valid `CreateJob` request carrying a row's reference members.
fn reference_request(row: &Reference) -> Value {
    let mut body = json!({
        "role": ROLE,
        "settings": job_settings("s3://example-bucket/in/a.mp4", "_a")
    });
    if let (Some(body), Some(members)) = (body.as_object_mut(), row.extras.as_object()) {
        for (member, value) in members {
            body.insert(member.clone(), value.clone());
        }
    }
    body
}

/// Submit one reference row and assert what the service answered.
async fn run_reference_row(emulator: &Emulator, row: &Reference) {
    let case = row.case;
    let reply = emulator.create_job(reference_request(row)).await;
    if row.status == 201 {
        assert_eq!(
            reply.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        let expected = row.job_template.map(Value::from).unwrap_or(Value::Null);
        assert_eq!(
            reply.at("/job/jobTemplate"),
            &expected,
            "case {case:?}: expected jobTemplate {expected}, got {}",
            reply.body
        );
        // Round trip: the reference the store resolved is the one a reader sees.
        let fetched = emulator.get_job(&reply.string("/job/id")).await;
        assert_eq!(
            fetched.at("/job/jobTemplate"),
            &expected,
            "case {case:?}: GetJob must report the stored jobTemplate, got {}",
            fetched.body
        );
        return;
    }
    assert_refusal(case, &reply, row.status, row.error_type);
}

/// One row of the hop-destination table: the `hopDestinations` list a request carries, and
/// what the service answers for it.
struct Hop {
    case: &'static str,
    hops: Value,
    status: u16,
    error_type: &'static str,
}

/// The rows every operation that accepts hop destinations is driven through.
fn hop_rows() -> Vec<Hop> {
    vec![
        Hop {
            case: "a hop to a stored queue by name",
            hops: json!([{ "queue": FIXTURE_QUEUE, "priority": 10, "waitMinutes": 15 }]),
            status: 0,
            error_type: "",
        },
        Hop {
            case: "a hop to the same queue by ARN",
            hops: json!([{ "queue": arn(&format!("queues/{FIXTURE_QUEUE}")), "waitMinutes": 30 }]),
            status: 0,
            error_type: "",
        },
        Hop {
            case: "a hop that names no queue at all",
            hops: json!([{ "priority": -10, "waitMinutes": 4_320 }]),
            status: 0,
            error_type: "",
        },
        Hop {
            case: "an empty hop list",
            hops: json!([]),
            status: 0,
            error_type: "",
        },
        Hop {
            case: "a hop to a queue name nothing holds",
            hops: json!([{ "queue": "no-such-queue", "waitMinutes": 10 }]),
            status: 404,
            error_type: "NotFoundException",
        },
        Hop {
            case: "a hop to a queue ARN nothing holds",
            hops: json!([{ "queue": arn("queues/no-such-queue"), "waitMinutes": 10 }]),
            status: 404,
            error_type: "NotFoundException",
        },
        Hop {
            case: "a second hop to a queue nothing holds, after a resolvable one",
            hops: json!([
                { "queue": FIXTURE_QUEUE, "waitMinutes": 10 },
                { "queue": "no-such-queue", "waitMinutes": 20 }
            ]),
            status: 404,
            error_type: "NotFoundException",
        },
    ]
}

/// A hop destination's `Queue` is a reference like any other: every operation that accepts
/// one refuses a hop to a queue the store does not hold, and accepts one it does.
///
/// The same rows are driven through all three operations that carry `HopDestinations` —
/// `CreateJob`, `CreateJobTemplate` and `UpdateJobTemplate` — so the rule is asserted
/// wherever it holds, not only on the operation the issue named.
#[tokio::test]
async fn a_hop_destination_queue_must_resolve_to_a_stored_queue() {
    let emulator = Emulator::new();
    let queue = emulator
        .create_queue(json!({ "name": FIXTURE_QUEUE }))
        .await;
    assert_eq!(
        queue.status, 201,
        "the fixture queue must be created: got {} with body {}",
        queue.status, queue.body
    );
    let target = emulator
        .create_job_template(json!({ "name": "hops-here", "settings": template_settings(1, 1) }))
        .await;
    assert_eq!(
        target.status, 201,
        "the fixture job template must be created: got {} with body {}",
        target.status, target.body
    );

    for row in hop_rows() {
        let Hop {
            case,
            hops,
            status,
            error_type,
        } = row;

        // What the addressed template carries BEFORE the update, so a refused update can
        // be shown to have changed nothing whatever an earlier row left there.
        let before = emulator.get_job_template("hops-here").await;
        let before_hops = before.at("/jobTemplate/hopDestinations").clone();

        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "hopDestinations": hops.clone(),
                "settings": job_settings("s3://example-bucket/in/a.mp4", "_a")
            }))
            .await;
        let template = emulator
            .create_job_template(json!({
                "name": format!("hopping-{case}"),
                "hopDestinations": hops.clone(),
                "settings": template_settings(1, 1)
            }))
            .await;
        let updated = emulator
            .update_job_template("hops-here", json!({ "hopDestinations": hops.clone() }))
            .await;

        for (operation, reply, accepted) in [
            ("CreateJob", &job, 201),
            ("CreateJobTemplate", &template, 201),
            ("UpdateJobTemplate", &updated, 200),
        ] {
            let labelled = format!("{operation}: {case}");
            if status == 0 {
                assert_eq!(
                    reply.status, accepted,
                    "case {labelled:?}: expected HTTP {accepted}, got {} with body {}",
                    reply.status, reply.body
                );
            } else {
                assert_refusal(&labelled, reply, status, error_type);
            }
        }

        let after = emulator.get_job_template("hops-here").await;
        let after_hops = after.at("/jobTemplate/hopDestinations");
        if status == 0 {
            assert_eq!(
                after_hops, &hops,
                "case {case:?}: an accepted UpdateJobTemplate must store the hop list it \
                 was given, got {}",
                after.body
            );
        } else {
            assert_eq!(
                after_hops, &before_hops,
                "case {case:?}: a refused UpdateJobTemplate must change nothing, got {}",
                after.body
            );
        }
    }
}
