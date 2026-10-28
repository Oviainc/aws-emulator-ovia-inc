// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the FORM a queue reference takes on a resource projection.
//!
//! The rule every case below is one row of: every queue reference a resource projection
//! reports is the referenced queue's ARN —
//! `arn:aws:mediaconvert:<region>:<account>:queues/<name>` — built from the queue's name
//! and the region of the request doing the reading, whichever of the two forms a queue
//! reference may be WRITTEN in (bare name or ARN) the request that stored it used. It
//! holds for every member of every projection that references a queue:
//! `JobTemplate$Queue`, `Job$Queue` and `HopDestination$Queue` inside both of them.
//!
//! From the model
//! (`smithy_query(query="shape",shape="JobTemplate,HopDestination",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! `JobTemplate$Queue` is "Optional. The queue that jobs created from this template are
//! assigned to", `HopDestination$Queue` is "Optional unless the job is submitted on the
//! default queue … you can specify a destination queue", and `Queue$Arn` is "An identifier
//! for this resource that is unique within all of AWS" — so the identifier a projection
//! reports for a referenced queue is the one the queue itself reports, which is what lets a
//! client match a template or a job against a queue ARN it holds. A hop destination that
//! references NO queue reports none: there is no reference to report.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness
//! (`job_lifecycle`), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{
    queue_arn_in, settings as job_settings, Emulator, Reply, REGION, ROLE,
};
use super::job_template_crud::settings as template_settings;
use super::resource_tagging::encode_label;

/// The two forms a queue reference may be written in. The rule generalises over which one
/// the request used, so every row is driven in both.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// The queue's bare `Name`.
    Name,
    /// The queue's ARN.
    Arn,
}

impl Form {
    /// The reference a request carries for `queue` in this form.
    fn reference(self, queue: &str) -> String {
        match self {
            Form::Name => queue.to_string(),
            Form::Arn => queue_arn_in(REGION, queue),
        }
    }

    /// The other form, so a row can write its two references one each way.
    fn flipped(self) -> Self {
        match self {
            Form::Name => Form::Arn,
            Form::Arn => Form::Name,
        }
    }
}

/// One row: a queue this account holds, and the form the request references it in.
#[derive(Clone, Copy, Debug)]
struct Row {
    case: &'static str,
    /// The queue the template is assigned to and the job is submitted to.
    queue: &'static str,
    /// False for the built-in queue nobody creates.
    create_queue: bool,
    /// The form the row's `Queue` member is written in; its hop destination uses the other
    /// one.
    form: Form,
}

/// The rows span what the rule generalises over: the form the reference is written in, and
/// which queue is referenced — one a caller created, the built-in `Default` queue nobody
/// created, and names carrying characters a reference has to survive (dots, digits and
/// surrounding space, which a queue's `Name` keeps verbatim).
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for form in [Form::Name, Form::Arn] {
        rows.push(Row {
            case: "a queue the caller created",
            queue: "projection-queue",
            create_queue: true,
            form,
        });
        rows.push(Row {
            case: "the built-in queue nobody created",
            queue: "Default",
            create_queue: false,
            form,
        });
        rows.push(Row {
            case: "a queue whose name carries dots and digits",
            queue: "projection.queue-2",
            create_queue: true,
            form,
        });
        rows.push(Row {
            case: "a queue whose name carries surrounding space",
            queue: " projection queue ",
            create_queue: true,
            form,
        });
    }
    rows
}

/// The queue every row's hop destination moves to — a queue of its own, because a job
/// cannot hop to the queue it was submitted to.
const HOP_QUEUE: &str = "projection-hop-queue";

/// The `hopDestinations` list a row sends: one entry naming [`HOP_QUEUE`] in the opposite
/// form to the row's own, and — unless the row's own queue is the default one, which such
/// an entry would hop to — one naming no queue at all.
fn hop_destinations(form: Form, with_unreferenced: bool) -> Value {
    let mut hops = vec![json!({ "queue": form.flipped().reference(HOP_QUEUE), "waitMinutes": 15 })];
    if with_unreferenced {
        hops.push(json!({ "priority": -10, "waitMinutes": 1 }));
    }
    Value::Array(hops)
}

/// The queue each hop destination of [`hop_destinations`] references, in order: the hop
/// queue, then — when the list carries it — the entry that references none.
fn expected_hops(with_unreferenced: bool) -> Vec<Option<&'static str>> {
    if with_unreferenced {
        vec![Some(HOP_QUEUE), None]
    } else {
        vec![Some(HOP_QUEUE)]
    }
}

/// Assert `view` reports `queue` as that queue's ARN in `region`, and reports each of its
/// hop destinations' queues the same way — `None` for an entry that references no queue.
fn assert_queue_arns(
    case: &str,
    label: &str,
    view: &Value,
    queue: &str,
    hops: &[Option<&str>],
    region: &str,
) {
    let expected = queue_arn_in(region, queue);
    let got = view.pointer("/queue").cloned().unwrap_or(Value::Null);
    assert_eq!(
        got,
        json!(expected),
        "case {case:?} at {label}: expected the queue member to be {expected:?}, got {got:?} \
         in {view}"
    );
    let reported = view
        .pointer("/hopDestinations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reported.len(),
        hops.len(),
        "case {case:?} at {label}: expected {} hop destinations, got {reported:?} in {view}",
        hops.len()
    );
    for (index, want) in hops.iter().enumerate() {
        let got = reported[index]
            .pointer("/queue")
            .cloned()
            .unwrap_or(Value::Null);
        let expected = want.map_or(Value::Null, |name| json!(queue_arn_in(region, name)));
        assert_eq!(
            got, expected,
            "case {case:?} at {label}: expected hop destination {index} to report queue \
             {expected:?}, got {got:?} in {view}"
        );
    }
}

/// The element of a listing whose `member` equals `value`, or `Value::Null`.
fn element(reply: &Reply, list: &str, member: &str, value: &str) -> Value {
    reply
        .at(list)
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| {
                    entry.pointer(&format!("/{member}")).and_then(Value::as_str) == Some(value)
                })
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// Every door onto a job template reports its queue reference — and its hop destinations'
/// — as the referenced queue's ARN, whichever form the request wrote them in.
#[tokio::test]
async fn every_job_template_projection_reports_its_queue_as_an_arn() {
    let emulator = Emulator::new();
    seed_queue(&emulator, HOP_QUEUE).await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        if row.create_queue {
            seed_queue(&emulator, row.queue).await;
        }
        let name = format!("projected-{index}");
        // A template carries no source queue for a hop to collide with, so every row's
        // list carries the entry that references no queue.
        let hops = hop_destinations(row.form, true);
        let expected = expected_hops(true);

        let created = emulator
            .create_job_template(json!({
                "name": &name,
                "queue": row.form.reference(row.queue),
                "hopDestinations": hops,
                "settings": template_settings(1, 1),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?} ({:?}): expected HTTP 201 from CreateJobTemplate, got {} with \
             body {}",
            row.form, created.status, created.body
        );
        assert_queue_arns(
            case,
            "CreateJobTemplate",
            created.template(),
            row.queue,
            &expected,
            REGION,
        );

        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from GetJobTemplate, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_queue_arns(
            case,
            "GetJobTemplate",
            fetched.template(),
            row.queue,
            &expected,
            REGION,
        );

        let listed = emulator.list_job_templates("").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListJobTemplates, got {} with body {}",
            listed.status, listed.body
        );
        let entry = element(&listed, "/jobTemplates", "name", &name);
        assert_queue_arns(
            case,
            "ListJobTemplates",
            &entry,
            row.queue,
            &expected,
            REGION,
        );

        // An update that names no queue leaves the stored reference alone, and still
        // reports it in the one form.
        let updated = emulator
            .update_job_template(&name, json!({ "description": "projected" }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateJobTemplate, got {} with body {}",
            updated.status, updated.body
        );
        assert_queue_arns(
            case,
            "UpdateJobTemplate",
            updated.template(),
            row.queue,
            &expected,
            REGION,
        );

        // And an update that RESTATES both references in the other form reports the same
        // ARNs: the form a reference arrives in never reaches a reader.
        let restated = emulator
            .update_job_template(
                &name,
                json!({
                    "queue": row.form.flipped().reference(row.queue),
                    "hopDestinations": hop_destinations(row.form.flipped(), true),
                }),
            )
            .await;
        assert_eq!(
            restated.status, 200,
            "case {case:?}: expected HTTP 200 from the restating update, got {} with body {}",
            restated.status, restated.body
        );
        assert_queue_arns(
            case,
            "UpdateJobTemplate restating the references",
            restated.template(),
            row.queue,
            &expected,
            REGION,
        );

        // The ARN a projection reports IS the referenced queue's own identifier, which is
        // what lets a client match the two.
        let queue = emulator.get_queue(&encode_label(row.queue)).await;
        assert_eq!(
            queue.at("/queue/arn"),
            restated.template().pointer("/queue").unwrap_or(&Value::Null),
            "case {case:?}: the template must report the queue's own ARN, got {} against {}",
            restated.body,
            queue.body
        );
    }
}

/// The same rule on the job projection: `Job$Queue` and a job's hop destinations report the
/// referenced queue's ARN, from every door onto a job.
#[tokio::test]
async fn every_job_projection_reports_its_queue_as_an_arn() {
    let emulator = Emulator::new();
    seed_queue(&emulator, HOP_QUEUE).await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        if row.create_queue {
            seed_queue(&emulator, row.queue).await;
        }
        // A hop that references no queue moves the job to the default queue, so a job
        // submitted ON that queue cannot carry one.
        let with_unreferenced = row.queue != "Default";
        let hops = hop_destinations(row.form, with_unreferenced);
        let expected = expected_hops(with_unreferenced);

        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": row.form.reference(row.queue),
                "hopDestinations": hops,
                "settings": job_settings(
                    &format!("s3://example-bucket/in/projected-{index}.mp4"),
                    "_p",
                ),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?} ({:?}): expected HTTP 201 from CreateJob, got {} with body {}",
            row.form, created.status, created.body
        );
        assert_queue_arns(
            case,
            "CreateJob",
            created.at("/job"),
            row.queue,
            &expected,
            REGION,
        );

        let id = created.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from GetJob, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_queue_arns(
            case,
            "GetJob",
            fetched.at("/job"),
            row.queue,
            &expected,
            REGION,
        );

        let listed = emulator.send("GET", "/2017-08-29/jobs", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListJobs, got {} with body {}",
            listed.status, listed.body
        );
        let entry = element(&listed, "/jobs", "id", &id);
        assert_queue_arns(case, "ListJobs", &entry, row.queue, &expected, REGION);
    }
}

/// An ARN names the region of the request that READ it, so a projected queue reference does
/// too — the queue is the same resource whichever region a caller signs for.
#[tokio::test]
async fn a_projected_queue_arn_names_the_reading_requests_region() {
    let emulator = Emulator::new();
    seed_queue(&emulator, HOP_QUEUE).await;
    seed_queue(&emulator, "cross-region-queue").await;

    let created = emulator
        .create_job_template(json!({
            "name": "cross-region",
            "queue": "cross-region-queue",
            "hopDestinations": hop_destinations(Form::Name, true),
            "settings": template_settings(1, 1),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created, got {} with body {}",
        created.status, created.body
    );

    // The region the fixture was created in, and two that created nothing.
    for region in [REGION, "eu-central-1", "ap-south-1"] {
        let fetched = emulator
            .send_in(region, "GET", "/2017-08-29/jobTemplates/cross-region", None)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {region:?}: expected HTTP 200 from GetJobTemplate, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_queue_arns(
            region,
            "GetJobTemplate",
            fetched.template(),
            "cross-region-queue",
            &expected_hops(true),
            region,
        );
    }
}

/// Create the queue `name`, which every row needs before it can reference it.
async fn seed_queue(emulator: &Emulator, name: &str) {
    let created = emulator.create_queue(json!({ "name": name })).await;
    assert_eq!(
        created.status, 201,
        "the fixture queue {name:?} must be created, got {} with body {}",
        created.status, created.body
    );
}
