// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE sentence this service answers a reference that
//! resolves to nothing with, and of the ONE sentence the tagging doors answer a value that
//! is not a MediaConvert resource ARN with.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from the
//! request's SigV4 scope, the framework deserializes and validates, the handler runs, and
//! the response is serialized — so what is asserted is the bytes a client would see, never
//! an internal call.
//!
//! The rule under test, which every row below is one case of: a request whose reference
//! resolves to no resource is answered with the modelled `NotFoundException` (HTTP 404)
//! stating the service's single lookup sentence, which names the REQUEST MEMBER the
//! reference arrived in — in the model's wire spelling — and never the value the caller
//! supplied. The rows span every door that resolves such a reference, the three kinds of
//! value a member can carry (a name, an id, and the ARN the tagging and certificate doors
//! address their resource by), the members that carry them (`name`, `id`, `jobId`,
//! `queue`, `jobTemplate`, `arn`), and the four resource kinds an ARN can name — so the
//! status, the error shape and the sentence cannot land on one door and miss another.
//!
//! The second test is the other refusal those ARN doors share: a value that is no
//! MediaConvert resource ARN at all is a problem in the REQUEST
//! (`BadRequestException`, HTTP 400), worded once for all three of them.
//!
//! What a resolved reference reads back as is the business of the suites that own each
//! operation; this one is only about the refusal they all share.

use serde_json::json;
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    not_found_message, resource_arn_message,
};

use super::job_lifecycle::{arn, settings, Emulator, Reply, ROLE};
use super::resource_tagging::encode_label;

/// A job id of the shape this service mints, that no `CreateJob` here was answered with.
const ABSENT_JOB_ID: &str = "1700000000000-zzzzzz";

/// Assert `reply` is the modelled 404 whose message is the service's one lookup sentence,
/// naming `member`.
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
        "case {case:?}: expected the member-naming lookup refusal {:?}, got {}",
        not_found_message(member),
        reply.body
    );
}

/// Assert `reply` is the modelled 400 whose message is the one sentence the tagging doors
/// refuse a value that is no MediaConvert resource ARN with.
fn assert_bad_arn(case: &str, value: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        resource_arn_message(value),
        "case {case:?}: expected the one ARN-form refusal {:?}, got {}",
        resource_arn_message(value),
        reply.body
    );
}

/// `ListTagsForResource` on `resource_arn` — `GET /2017-08-29/tags/{Arn}`, so the value
/// travels as one percent-encoded path segment.
async fn list_tags(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/2017-08-29/tags/{}", encode_label(resource_arn)),
            None,
        )
        .await
}

/// `TagResource` on `resource_arn` — `POST /2017-08-29/tags`, the one tagging door that
/// carries its `Arn` in the body (`jsonName` `arn`).
async fn tag(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "POST",
            "/2017-08-29/tags",
            Some(json!({ "arn": resource_arn, "tags": { "env": "prod" } })),
        )
        .await
}

/// `UntagResource` on `resource_arn` — `PUT /2017-08-29/tags/{Arn}`.
async fn untag(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/2017-08-29/tags/{}", encode_label(resource_arn)),
            Some(json!({ "tagKeys": ["env"] })),
        )
        .await
}

/// One resource of each kind this store holds, so every 404 below is about the reference
/// the request carried and not about an empty store.
async fn seed(emulator: &Emulator) {
    for (label, uri, body) in [
        (
            "queue",
            "/2017-08-29/queues",
            json!({ "name": "present-queue" }),
        ),
        (
            "preset",
            "/2017-08-29/presets",
            json!({
                "name": "present-preset",
                "settings": { "containerSettings": { "container": "MP4" } }
            }),
        ),
        (
            "job template",
            "/2017-08-29/jobTemplates",
            json!({
                "name": "present-template",
                "settings": crate::job_template_crud::settings(1, 1)
            }),
        ),
        (
            "job",
            "/2017-08-29/jobs",
            json!({
                "role": ROLE,
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            }),
        ),
    ] {
        let reply = emulator.send("POST", uri, Some(body)).await;
        assert_eq!(
            reply.status, 201,
            "fixture {label}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
    }
}

/// Every door that resolves a reference answers one that resolves to nothing with the
/// same status, the same error shape and the same member-naming sentence.
#[tokio::test]
async fn every_door_words_a_reference_that_resolves_to_nothing_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The sentence itself, anchored once here so this suite is a regression test for the
    // WORDING and not only for which member it names: AWS answers this error class with
    // exactly this text for an `Arn` that resolves to nothing, as the recorded certificate
    // refusal of this service shows.
    assert_eq!(
        not_found_message("arn"),
        "The resource you specified in arn was not found.",
        "case the sentence itself: expected the wording AWS answers this error class with, \
         got {:?}",
        not_found_message("arn")
    );

    // One row per door, carrying the member the model gives the reference it read: the
    // `@httpLabel` members in their wire spelling (`name`, `id`, `arn`) and the body
    // members by their `jsonName` (`jobId`, `queue`, `jobTemplate`) —
    // `smithy_query(query="digest")` for the bindings.
    let rows: Vec<(&str, &str, Reply)> = vec![
        (
            "GetQueue on a name nobody created",
            "name",
            emulator.get_queue("absent-queue").await,
        ),
        (
            "UpdateQueue on a name nobody created",
            "name",
            emulator
                .send(
                    "PUT",
                    "/2017-08-29/queues/absent-queue",
                    Some(json!({ "description": "a queue that is not there" })),
                )
                .await,
        ),
        (
            "GetPreset on a name nobody created",
            "name",
            emulator
                .send("GET", "/2017-08-29/presets/absent-preset", None)
                .await,
        ),
        (
            "UpdatePreset on a name nobody created",
            "name",
            emulator
                .send(
                    "PUT",
                    "/2017-08-29/presets/absent-preset",
                    Some(json!({ "description": "a preset that is not there" })),
                )
                .await,
        ),
        (
            "GetJobTemplate on a name nobody created",
            "name",
            emulator.get_job_template("absent-template").await,
        ),
        (
            "UpdateJobTemplate on a name nobody created",
            "name",
            emulator
                .update_job_template(
                    "absent-template",
                    json!({ "description": "a template that is not there" }),
                )
                .await,
        ),
        (
            "GetJob on an id nobody was given",
            "id",
            emulator.get_job(ABSENT_JOB_ID).await,
        ),
        (
            "CancelJob on an id nobody was given",
            "id",
            emulator.cancel_job(ABSENT_JOB_ID).await,
        ),
        (
            "GetJobsQueryResults on a query nobody started",
            "id",
            emulator
                .send(
                    "GET",
                    &format!("/2017-08-29/jobsQueries/{ABSENT_JOB_ID}"),
                    None,
                )
                .await,
        ),
        (
            "CreateResourceShare for a job nobody was given",
            "jobId",
            emulator
                .send(
                    "POST",
                    "/2017-08-29/resourceShares",
                    Some(json!({
                        "jobId": ABSENT_JOB_ID,
                        "supportCaseId": "case-00000001"
                    })),
                )
                .await,
        ),
        (
            "CreateJob naming a queue nobody created",
            "queue",
            emulator
                .create_job(json!({
                    "role": ROLE,
                    "queue": "absent-queue",
                    "settings": settings("s3://example-bucket/in/a.mp4", "_a")
                }))
                .await,
        ),
        (
            "CreateJob naming a job template nobody created",
            "jobTemplate",
            emulator
                .create_job(json!({
                    "role": ROLE,
                    "jobTemplate": "absent-template",
                    "settings": { "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }] }
                }))
                .await,
        ),
        (
            "CreateJobTemplate naming a queue nobody created",
            "queue",
            emulator
                .create_job_template(json!({
                    "name": "template-of-an-absent-queue",
                    "queue": "absent-queue",
                    "settings": crate::job_template_crud::settings(1, 1)
                }))
                .await,
        ),
        (
            "DisassociateCertificate for an association the account does not hold",
            "arn",
            emulator
                .send(
                    "DELETE",
                    &format!(
                        "/2017-08-29/certificates/{}",
                        encode_label(
                            "arn:aws:acm:us-west-2:123456789012:certificate/\
                             0d1e2f30-4a5b-6c7d-8e9f-0a1b2c3d4e5f"
                        )
                    ),
                    None,
                )
                .await,
        ),
    ];

    for (case, member, reply) in &rows {
        assert_not_found(case, member, reply);
    }

    // The three ARN doors, over each resource kind an ARN can name — the four forms
    // MediaConvert documents as taggable (`queues/`, `jobs/`, `presets/`,
    // `jobTemplates/`): the resource kind in the value changes nothing about the sentence,
    // which names the member the ARN arrived in.
    for (kind, resource_arn) in [
        ("a queue nobody created", arn("queues/absent-queue")),
        ("a job id nobody was given", arn(&format!("jobs/{ABSENT_JOB_ID}"))),
        ("a preset nobody created", arn("presets/absent-preset")),
        (
            "a job template nobody created",
            arn("jobTemplates/absent-template"),
        ),
    ] {
        for (door, reply) in [
            ("ListTagsForResource", list_tags(&emulator, &resource_arn).await),
            ("TagResource", tag(&emulator, &resource_arn).await),
            ("UntagResource", untag(&emulator, &resource_arn).await),
        ] {
            assert_not_found(&format!("{door} on {kind}"), "arn", &reply);
        }
    }

    // The resources that DO exist are untouched by the refusals above: a refused lookup
    // reports nothing and writes nothing.
    let listed = list_tags(&emulator, &arn("queues/present-queue")).await;
    assert_eq!(
        listed.status, 200,
        "case the seeded queue still resolves: expected HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.at("/resourceTags/tags"),
        &json!({}),
        "case the seeded queue still resolves: expected the stored (empty) tag set, got {}",
        listed.body
    );
}

/// A value that is no MediaConvert resource ARN earns the same bad request, worded the
/// same way, from each of the three doors that address their resource by ARN.
#[tokio::test]
async fn every_tagging_door_refuses_a_value_that_is_no_resource_arn_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The sentence itself, anchored once as the not-found suite anchors its own: it names
    // the member, the value the request carried, and the resource kinds the operation
    // accepts (the four `TagResource` documents as taggable).
    assert_eq!(
        resource_arn_message("not-an-arn"),
        "Arn not-an-arn is not the ARN of a MediaConvert queue, preset, job or job template.",
        "case the sentence itself: expected the one ARN-form refusal, got {:?}",
        resource_arn_message("not-an-arn")
    );

    // The rows span the ways a value can fail to be one of the four taggable ARN forms:
    // not an ARN at all, an ARN of another service, a MediaConvert ARN whose resource
    // kind is not taggable, one with no resource kind, and one naming no resource.
    let cases = [
        ("a bare queue name", "present-queue".to_string()),
        ("an ARN of another service", "arn:aws:s3:::my-bucket".to_string()),
        (
            "an ACM certificate ARN, which the certificate doors take",
            "arn:aws:acm:us-west-2:123456789012:certificate/0d1e2f30-4a5b".to_string(),
        ),
        (
            "a MediaConvert ARN with an untaggable resource kind",
            arn("policies/default"),
        ),
        ("a MediaConvert ARN with no resource kind", arn("present-queue")),
        (
            "a MediaConvert ARN naming no resource",
            arn("queues/"),
        ),
        ("a truncated ARN", "arn:aws:mediaconvert".to_string()),
        ("a value that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (case, value) in cases {
        for (door, reply) in [
            ("ListTagsForResource", list_tags(&emulator, &value).await),
            ("TagResource", tag(&emulator, &value).await),
            ("UntagResource", untag(&emulator, &value).await),
        ] {
            assert_bad_arn(&format!("{door} on {case}"), &value, &reply);
        }
    }
}
