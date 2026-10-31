// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHICH ARNs a resource reference of this service resolves.
//!
//! The rule every case below is one row of: a member that references a resource of this
//! service accepts the resource's own name (or id) and the ARN of a MEDIACONVERT resource
//! of the kind that member references, and nothing else — so a value whose service
//! namespace is another service's, whose ARN fields are too few to be an ARN, or whose
//! resource kind is not the kind the member references, is read as the literal name the
//! caller sent and resolves a resource only if this account holds one under it.
//!
//! The members are the ones the model documents as taking either form —
//! `smithy_query(query="operation",operation="CreateJob,CreateJobTemplate,UpdateJobTemplate,CreateResourceShare,SearchJobs",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`:
//! `CreateJob$Queue` and `CreateJob$JobTemplate`, the `Queue` of both job-template doors,
//! the nested `HopDestination$Queue`, `CreateResourceShare$JobId` ("Specify MediaConvert
//! Job ID or ARN to share"), `SearchJobs$Queue` ("Provide a queue name, or a queue ARN")
//! and a jobs query's `queue` filter key ("Your Queue's name or ARN"). One suite, so the
//! rule is asserted wherever it holds rather than at the one door it was noticed at.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message;

use super::job_lifecycle::{
    arn, queue_arn, settings as job_settings, Emulator, Reply, REGION, ROLE,
};
use super::job_search::job_ids;
use super::job_template_crud::settings as template_settings;
// Starting a jobs query, pinning its completion span and waiting past it are the jobs
// query suite's own helpers, reused here rather than copied.
use super::jobs_query::{pin_query_span, run_query, SHORT_QUERY};
use super::resource_tagging::encode_label;

/// The queue every reference case points at, created by the fixture.
const FIXTURE_QUEUE: &str = "reference-queue";

/// The queue a hop destination's rows are submitted FROM, so a hop to [`FIXTURE_QUEUE`]
/// is a hop to another queue rather than to the job's own.
const HOP_SOURCE_QUEUE: &str = "hop-source-queue";

/// The job template every job-template reference case points at.
const FIXTURE_TEMPLATE: &str = "reference-template";

/// One value that is NOT an ARN of this service naming the referenced kind of resource,
/// and the label the assertions name it by.
struct Impostor {
    label: String,
    value: String,
}

/// Every impostor form, built around `resource_id` — the `<plural type>/<name>` resource
/// id of a resource this account DOES hold.
///
/// So each row is a value a reader that skipped the service namespace, the ARN's field
/// count or the resource kind would resolve to that stored resource, while a reader held
/// to the rule resolves none of them. The rows differ along what the rule generalises
/// over: the service namespace (another service's, one this service's name is a prefix
/// of, one that ends with it), the partition, and how many ARN fields the value carries.
/// The resource-kind row is added per door by [`kind_impostor`], since the kind that is
/// wrong depends on the kind the member references.
fn namespace_impostors(resource_id: &str) -> Vec<Impostor> {
    let account = engine::emulator_account_id();
    vec![
        Impostor {
            label: "an ARN of another service, written with no region or account".to_string(),
            value: format!("arn:aws:s3:::{resource_id}"),
        },
        Impostor {
            label: "a fully qualified ARN of another service".to_string(),
            value: engine::emulator_arn("medialive", REGION, resource_id),
        },
        Impostor {
            label: "an ARN whose service namespace this service's is a prefix of".to_string(),
            value: engine::emulator_arn("mediaconvert-internal", REGION, resource_id),
        },
        Impostor {
            label: "an ARN whose service namespace ends with this service's".to_string(),
            value: engine::emulator_arn("aws-mediaconvert", REGION, resource_id),
        },
        Impostor {
            label: "another partition, another service".to_string(),
            value: format!("arn:aws-cn:s3:cn-north-1:{account}:{resource_id}"),
        },
        Impostor {
            label: "a value with too few ARN fields to be an ARN".to_string(),
            value: format!("arn:aws:{resource_id}"),
        },
        Impostor {
            label: "a value carrying nothing but the arn prefix".to_string(),
            value: format!("arn:{resource_id}"),
        },
    ]
}

/// The resource-kind row: THIS service's ARN, naming `other_kind` instead of the kind the
/// member references.
///
/// The four kinds a MediaConvert resource id names are `queues`, `jobs`, `presets` and
/// `jobTemplates` (the resource kinds `smithy_query(query="shape",shape="TagResourceRequest")`'s
/// `Arn` addresses, as the tagging suite's table spells them), so a door that references
/// one of them is handed the ARN of another.
fn kind_impostor(other_kind: &str, name: &str) -> Impostor {
    Impostor {
        label: format!("this service's ARN naming a {other_kind} resource instead"),
        value: arn(&format!("{other_kind}/{name}")),
    }
}

/// Every impostor a queue reference is driven through: the namespace and field-count rows
/// around the fixture queue's resource id, plus this service's ARN for a resource of
/// another kind that carries the same name.
fn queue_impostors() -> Vec<Impostor> {
    let mut rows = namespace_impostors(&format!("queues/{FIXTURE_QUEUE}"));
    rows.push(kind_impostor("presets", FIXTURE_QUEUE));
    rows.push(kind_impostor("jobTemplates", FIXTURE_QUEUE));
    rows
}

/// Assert `reply` is the modelled 404 for a reference that resolves to nothing, worded
/// with the member the reference arrived in.
fn assert_not_found(case: &str, member: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected NotFoundException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        not_found_message(member),
        "case {case:?}: expected the refusal to name the {member} member, got {}",
        reply.body
    );
}

/// Create the queues, the job template and the stored job every case below references.
async fn seed(emulator: &Emulator) -> String {
    for name in [FIXTURE_QUEUE, HOP_SOURCE_QUEUE] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }
    let created = emulator
        .create_job_template(json!({
            "name": FIXTURE_TEMPLATE,
            "queue": FIXTURE_QUEUE,
            "settings": template_settings(1, 1),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job template must be created: got {} with body {}",
        created.status, created.body
    );

    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": FIXTURE_QUEUE,
            "settings": job_settings("s3://example-bucket/in/a.mp4", "_seed"),
        }))
        .await;
    assert_eq!(
        job.status, 201,
        "the fixture job must be created: got {} with body {}",
        job.status, job.body
    );
    job.string("/job/id")
}

/// A `CreateJob` body sending a job to `queue`.
fn job_on_queue(queue: &str) -> Value {
    json!({
        "role": ROLE,
        "queue": queue,
        "settings": job_settings("s3://example-bucket/in/a.mp4", "_out"),
    })
}

/// Every door that takes a top-level or nested queue reference refuses a value that is
/// not this service's ARN for a queue, and accepts the two forms that are.
#[tokio::test]
async fn a_queue_reference_resolves_only_this_services_queue_arn() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let stored_arn = queue_arn(FIXTURE_QUEUE);

    for (index, Impostor { label, value }) in queue_impostors().into_iter().enumerate() {
        // `CreateJob$Queue`.
        assert_not_found(
            &format!("CreateJob queue given {label} ({value:?})"),
            "queue",
            &emulator.create_job(job_on_queue(&value)).await,
        );

        // `HopDestination$Queue`, nested in the same operation: the job itself goes to a
        // real queue, so only the hop's reference can refuse it.
        let mut hopping = job_on_queue(HOP_SOURCE_QUEUE);
        hopping["hopDestinations"] = json!([{ "queue": value, "waitMinutes": 20 }]);
        assert_not_found(
            &format!("a hop destination given {label} ({value:?})"),
            "hopDestinations",
            &emulator.create_job(hopping).await,
        );

        // `CreateJobTemplate$Queue`, which must persist nothing when it refuses.
        let template = format!("refused-{index}");
        assert_not_found(
            &format!("CreateJobTemplate queue given {label} ({value:?})"),
            "queue",
            &emulator
                .create_job_template(json!({
                    "name": template,
                    "queue": value,
                    "settings": template_settings(1, 1),
                }))
                .await,
        );
        let fetched = emulator.get_job_template(&template).await;
        assert_eq!(
            fetched.status, 404,
            "case {label:?}: a refused create must persist no template, got {} with body {}",
            fetched.status, fetched.body
        );

        // `UpdateJobTemplate$Queue`, on the fixture template, which must keep the queue it
        // already resolved.
        assert_not_found(
            &format!("UpdateJobTemplate queue given {label} ({value:?})"),
            "queue",
            &emulator
                .update_job_template(FIXTURE_TEMPLATE, json!({ "queue": value }))
                .await,
        );
        let unchanged = emulator.get_job_template(FIXTURE_TEMPLATE).await;
        assert_eq!(
            unchanged.string("/jobTemplate/queue"),
            stored_arn,
            "case {label:?}: a refused update must leave the stored queue, got {}",
            unchanged.body
        );
    }

    // The two forms the member DOES offer still resolve the stored queue, at every door.
    for (form, reference) in [
        ("the queue's name", FIXTURE_QUEUE.to_string()),
        ("the queue's own ARN", stored_arn.clone()),
    ] {
        let job = emulator.create_job(job_on_queue(&reference)).await;
        assert_eq!(
            job.status, 201,
            "case {form:?}: expected CreateJob to accept the reference, got {} with body {}",
            job.status, job.body
        );
        assert_eq!(
            job.string("/job/queue"),
            stored_arn,
            "case {form:?}: expected the job on the queue the reference named, got {}",
            job.body
        );
        // Round trip: the queue the store resolved is the one a later reader sees.
        let fetched = emulator.get_job(&job.string("/job/id")).await;
        assert_eq!(
            fetched.string("/job/queue"),
            stored_arn,
            "case {form:?}: GetJob must report the stored queue, got {}",
            fetched.body
        );

        let mut hopping = job_on_queue(HOP_SOURCE_QUEUE);
        hopping["hopDestinations"] = json!([{ "queue": reference, "waitMinutes": 20 }]);
        let hopped = emulator.create_job(hopping).await;
        assert_eq!(
            hopped.status, 201,
            "case {form:?}: expected a hop to the stored queue to be accepted, got {} with \
             body {}",
            hopped.status, hopped.body
        );
        assert_eq!(
            hopped.string("/job/hopDestinations/0/queue"),
            stored_arn,
            "case {form:?}: expected the hop to report the queue it resolved, got {}",
            hopped.body
        );

        let updated = emulator
            .update_job_template(FIXTURE_TEMPLATE, json!({ "queue": reference }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {form:?}: expected UpdateJobTemplate to accept the reference, got {} with \
             body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("/jobTemplate/queue"),
            stored_arn,
            "case {form:?}: expected the update to store the queue it resolved, got {}",
            updated.body
        );
    }
}

/// The job and job-template references take the same rule: `CreateJob$JobTemplate` and
/// `CreateResourceShare$JobId` resolve this service's ARN for the kind they reference, and
/// read anything else as the literal name or id it is.
#[tokio::test]
async fn a_job_and_template_reference_resolve_only_this_services_arn() {
    let emulator = Emulator::new();
    let job_id = seed(&emulator).await;

    let mut template_rows = namespace_impostors(&format!("jobTemplates/{FIXTURE_TEMPLATE}"));
    template_rows.push(kind_impostor("queues", FIXTURE_TEMPLATE));
    template_rows.push(kind_impostor("presets", FIXTURE_TEMPLATE));
    for Impostor { label, value } in template_rows {
        assert_not_found(
            &format!("CreateJob jobTemplate given {label} ({value:?})"),
            "jobTemplate",
            &emulator
                .create_job(json!({
                    "role": ROLE,
                    "jobTemplate": value,
                    "settings": json!({ "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }] }),
                }))
                .await,
        );
    }

    let mut job_rows = namespace_impostors(&format!("jobs/{job_id}"));
    job_rows.push(kind_impostor("queues", &job_id));
    job_rows.push(kind_impostor("jobTemplates", &job_id));
    for Impostor { label, value } in job_rows {
        assert_not_found(
            &format!("CreateResourceShare jobId given {label} ({value:?})"),
            "jobId",
            &emulator
                .send(
                    "POST",
                    "/2017-08-29/resourceShares",
                    Some(json!({ "jobId": value, "supportCaseId": "case-404" })),
                )
                .await,
        );
    }

    // No refusal above shared the stored job: nothing has asked to share it.
    let read = emulator.get_job(&job_id).await;
    assert_eq!(
        read.at("/job/lastShareDetails"),
        &Value::Null,
        "a refused share must record nothing on any job, got {}",
        read.body
    );

    // And the forms each member DOES offer still resolve: the template by name and by its
    // own ARN, the job by id and by its own ARN.
    for (form, reference) in [
        ("the template's name", FIXTURE_TEMPLATE.to_string()),
        (
            "the template's own ARN",
            arn(&format!("jobTemplates/{FIXTURE_TEMPLATE}")),
        ),
    ] {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "jobTemplate": reference,
                "settings": json!({ "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }] }),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {form:?}: expected CreateJob to accept the reference, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.string("/job/jobTemplate"),
            FIXTURE_TEMPLATE,
            "case {form:?}: expected the job to name the template it resolved, got {}",
            created.body
        );
    }

    for (form, reference, support_case_id) in [
        ("the job's id", job_id.clone(), "case-by-id"),
        (
            "the job's own ARN",
            arn(&format!("jobs/{job_id}")),
            "case-by-arn",
        ),
    ] {
        let shared = emulator
            .send(
                "POST",
                "/2017-08-29/resourceShares",
                Some(json!({ "jobId": reference, "supportCaseId": support_case_id })),
            )
            .await;
        assert_eq!(
            shared.status, 202,
            "case {form:?}: expected CreateResourceShare to accept the reference, got {} with \
             body {}",
            shared.status, shared.body
        );
        let read = emulator.get_job(&job_id).await;
        let details = read.string("/job/lastShareDetails");
        assert!(
            details.contains(support_case_id),
            "case {form:?}: expected the share recorded against the job it resolved to name \
             {support_case_id:?}, got {details:?} from body {}",
            read.body
        );
    }
}

/// The readers take the same rule: a `queue` filter narrows by the queue this service's
/// queue ARN names, and a value that is not one narrows to the jobs of the queue literally
/// named by the whole value — of which this account holds none.
#[tokio::test]
async fn a_queue_filter_resolves_only_this_services_queue_arn() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let job_id = seed(&emulator).await;
    let on_fixture = BTreeSet::from([job_id.clone()]);
    let none: BTreeSet<String> = BTreeSet::new();

    for Impostor { label, value } in queue_impostors() {
        let searched = emulator
            .send(
                "GET",
                &format!("/2017-08-29/search?queue={}", encode_label(&value)),
                None,
            )
            .await;
        assert_eq!(
            searched.status, 200,
            "case {label:?}: expected HTTP 200 from SearchJobs, got {} with body {}",
            searched.status, searched.body
        );
        let reported: BTreeSet<String> = job_ids(&searched).into_iter().collect();
        assert_eq!(
            reported, none,
            "case {label:?} ({value:?}): SearchJobs must report no job, got {reported:?} from \
             body {}",
            searched.body
        );

        let queried = run_query(
            &emulator,
            json!({ "filterList": [{ "key": "queue", "values": [value] }] }),
        )
        .await;
        let reported: BTreeSet<String> = job_ids(&queried).into_iter().collect();
        assert_eq!(
            reported, none,
            "case {label:?} ({value:?}): a jobs query must report no job, got {reported:?} \
             from body {}",
            queried.body
        );
    }

    // The forms the filters DO offer still select the fixture job.
    for (form, reference) in [
        ("the queue's name", FIXTURE_QUEUE.to_string()),
        ("the queue's own ARN", queue_arn(FIXTURE_QUEUE)),
    ] {
        let searched = emulator
            .send(
                "GET",
                &format!("/2017-08-29/search?queue={}", encode_label(&reference)),
                None,
            )
            .await;
        let reported: BTreeSet<String> = job_ids(&searched).into_iter().collect();
        assert_eq!(
            reported, on_fixture,
            "case {form:?}: SearchJobs must report the job on that queue, got {reported:?} \
             from body {}",
            searched.body
        );

        let queried = run_query(
            &emulator,
            json!({ "filterList": [{ "key": "queue", "values": [reference] }] }),
        )
        .await;
        let reported: BTreeSet<String> = job_ids(&queried).into_iter().collect();
        assert_eq!(
            reported, on_fixture,
            "case {form:?}: a jobs query must report the job on that queue, got {reported:?} \
             from body {}",
            queried.body
        );
    }
}

/// The other direction of the same rule: a value that is not this service's ARN is the
/// NAME it looks like, so a queue created under such a name is reached by it.
///
/// The sharpest row of the class — a queue literally named the ARN another service would
/// write for a MediaConvert queue — which a reader that stripped any `:queues/` it found
/// could never reach, because it would look for the queue `Default` instead.
#[tokio::test]
async fn a_queue_named_like_another_services_arn_is_reached_by_that_name() {
    let emulator = Emulator::new();
    let impostor_name = "arn:aws:s3:::queues/Default".to_string();
    let created = emulator
        .create_queue(json!({ "name": impostor_name }))
        .await;
    assert_eq!(
        created.status, 201,
        "the ARN-named fixture queue must be created: got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.string("/queue/name"),
        impostor_name,
        "the queue's name must be stored byte for byte, got {}",
        created.body
    );

    let job = emulator.create_job(job_on_queue(&impostor_name)).await;
    assert_eq!(
        job.status, 201,
        "case {impostor_name:?}: expected CreateJob to send the job to the queue NAMED by \
         the value, got {} with body {}",
        job.status, job.body
    );
    assert_eq!(
        job.string("/job/queue"),
        queue_arn(&impostor_name),
        "case {impostor_name:?}: expected the job on the queue that whole value names, got {}",
        job.body
    );

    // And not on the built-in queue the value's resource id spells.
    assert_ne!(
        job.string("/job/queue"),
        queue_arn("Default"),
        "case {impostor_name:?}: the job must not land on the queue the resource id spells, \
         got {}",
        job.body
    );
}

