// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of MediaConvert's error envelope.
//!
//! Every error body this service answers carries, beside `message`, the envelope members
//! AWS wraps a MediaConvert exception in: `errorType` (the error's code), `httpStatus` (the
//! response's own status, repeated in the body), `operation` (the operation that failed),
//! `accountId`, `requestId` and `settingsValidationErrorsJsonBlob` (empty for a failure
//! that is not a settings-validation failure). Only `message` belongs to a modelled shape
//! — `smithy_query(query="shape",shape="NotFoundException")` declares `Message` and
//! nothing else — so the rule under test is about the ENVELOPE, and it holds for every
//! operation and every modelled exception, not just for the 404s a client happens to read
//! first.
//!
//! The rows span what the rule generalises over: the operation (eleven of them, read, write
//! and delete), the error code (`NotFoundException`, `ForbiddenException`,
//! `ConflictException`, `BadRequestException`), the HTTP status (404, 403, 409, 400), the
//! resource plane (jobs, queues, job templates, presets, jobs queries) and the layer that
//! produced the failure (a handler's own refusal, and a framework refusal raised while the
//! operation was being served). Each case drives the real restJson1 wire path through the
//! router — the harness is the one `job_lifecycle` builds — so what is asserted is the
//! bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{settings as job_settings, Emulator, Reply, ROLE};

/// The queue the fixture creates and deletes, so a read of it is a read of a queue that
/// is gone.
const DELETED_QUEUE: &str = "envelope-deleted-queue";

/// The job template the fixture creates and deletes, for the same reason.
const DELETED_TEMPLATE: &str = "envelope-deleted-template";

/// The queue the fixture leaves holding a non-terminal job, so a delete of it conflicts.
const BUSY_QUEUE: &str = "envelope-busy-queue";

/// One failed call: the request to send, and the failure AWS answers it with.
struct Case {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The operation the envelope's `operation` member must name.
    operation: &'static str,
    status: u16,
    /// The error code the envelope's `errorType` member must name.
    error_type: &'static str,
}

/// An emulator seeded so every row below has something real to fail against, and the rows
/// themselves.
///
/// The seeded state is what makes the failures genuine rather than arranged: a queue and a
/// template that were created and then deleted, and a queue still holding a job the clock
/// has not settled.
async fn seeded() -> (Emulator, Vec<Case>) {
    let emulator = Emulator::new();

    for (label, created) in [
        (
            "the queue to delete",
            emulator.create_queue(json!({ "name": DELETED_QUEUE })).await,
        ),
        (
            "the busy queue",
            emulator.create_queue(json!({ "name": BUSY_QUEUE })).await,
        ),
    ] {
        assert_eq!(
            created.status, 201,
            "fixture {label:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
    }

    let template = emulator
        .create_job_template(json!({
            "name": DELETED_TEMPLATE,
            "settings": { "inputs": [{ "timecodeSource": "ZEROBASED" }] }
        }))
        .await;
    assert_eq!(
        template.status, 201,
        "fixture the template to delete: expected HTTP 201, got {} with body {}",
        template.status, template.body
    );

    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": BUSY_QUEUE,
            "settings": job_settings("s3://example-bucket/in/a.mp4", "_a")
        }))
        .await;
    assert_eq!(
        job.status, 201,
        "fixture the non-terminal job: expected HTTP 201, got {} with body {}",
        job.status, job.body
    );

    for (label, removed) in [
        (
            "delete the queue",
            emulator
                .send("DELETE", &format!("/2017-08-29/queues/{DELETED_QUEUE}"), None)
                .await,
        ),
        (
            "delete the template",
            emulator.delete_job_template(DELETED_TEMPLATE).await,
        ),
    ] {
        assert_eq!(
            removed.status, 202,
            "fixture {label:?}: expected HTTP 202, got {} with body {}",
            removed.status, removed.body
        );
    }

    // The built-in template the 403 row addresses comes from the store's own catalogue
    // (`ListJobTemplates?listBy=SYSTEM`), never a hard-coded entry of it.
    let system = emulator.list_job_templates("listBy=SYSTEM").await;
    assert_eq!(
        system.status, 200,
        "fixture the built-in templates: expected HTTP 200, got {} with body {}",
        system.status, system.body
    );
    let system_template = system
        .at("/jobTemplates")
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.pointer("/name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !system_template.is_empty(),
        "fixture the built-in templates: expected at least one, got body {}",
        system.body
    );

    let cases = vec![
        Case {
            label: "GetJob on an id the store does not hold",
            method: "GET",
            uri: "/2017-08-29/jobs/1700000000000-nosuch".to_string(),
            body: None,
            operation: "GetJob",
            status: 404,
            error_type: "NotFoundException",
        },
        Case {
            label: "GetQueue on a deleted queue",
            method: "GET",
            uri: format!("/2017-08-29/queues/{DELETED_QUEUE}"),
            body: None,
            operation: "GetQueue",
            status: 404,
            error_type: "NotFoundException",
        },
        Case {
            label: "GetJobTemplate on a deleted template",
            method: "GET",
            uri: format!("/2017-08-29/jobTemplates/{DELETED_TEMPLATE}"),
            body: None,
            operation: "GetJobTemplate",
            status: 404,
            error_type: "NotFoundException",
        },
        Case {
            // A plane the issue did not cite, to show the envelope is not per-operation
            // code but the service's.
            label: "GetPreset on a name the store does not hold",
            method: "GET",
            uri: "/2017-08-29/presets/envelope-no-such-preset".to_string(),
            body: None,
            operation: "GetPreset",
            status: 404,
            error_type: "NotFoundException",
        },
        Case {
            // The jobs-query plane: reading the results of a jobs query id that is not
            // present, which is the recorded call
            // `issues/.../mediaconvert-getjobsqueryresults-404-omits-the-error-envelope-members.md`
            // was filed about.
            label: "GetJobsQueryResults on an id the store does not hold",
            method: "GET",
            uri: "/2017-08-29/jobsQueries/1700000000000-nosuch".to_string(),
            body: None,
            operation: "GetJobsQueryResults",
            status: 404,
            error_type: "NotFoundException",
        },
        Case {
            // The same plane's write: `nextToken` is this service's own opaque cursor, so
            // a token it never issued is a problem in the request.
            label: "StartJobsQuery with a nextToken this service never issued",
            method: "POST",
            uri: "/2017-08-29/jobsQueries".to_string(),
            body: Some(json!({ "nextToken": "not-a-token-this-service-issued" })),
            operation: "StartJobsQuery",
            status: 400,
            error_type: "BadRequestException",
        },
        Case {
            label: "CancelJob on an id the store does not hold",
            method: "DELETE",
            uri: "/2017-08-29/jobs/1700000000000-nosuch".to_string(),
            body: None,
            operation: "CancelJob",
            status: 404,
            error_type: "NotFoundException",
        },
        Case {
            // A `DELETE` that is refused, not one that is a no-op: a delete of a template
            // that is simply absent is this service's idempotent 202 (see
            // `tests/all/idempotent_repeats.rs`), so the refusal to address here is the
            // built-in template a caller may not delete at all.
            label: "DeleteJobTemplate on a built-in template",
            method: "DELETE",
            uri: format!("/2017-08-29/jobTemplates/{system_template}"),
            body: None,
            operation: "DeleteJobTemplate",
            status: 403,
            error_type: "ForbiddenException",
        },
        Case {
            // A different status and code: a built-in queue cannot be deleted.
            label: "DeleteQueue on the built-in queue",
            method: "DELETE",
            uri: format!(
                "/2017-08-29/queues/{}",
                service_com_amazonaws_mediaconvert_mediaconvert::state::DEFAULT_QUEUE_NAME
            ),
            body: None,
            operation: "DeleteQueue",
            status: 403,
            error_type: "ForbiddenException",
        },
        Case {
            label: "UpdateJobTemplate on a built-in template",
            method: "PUT",
            uri: format!("/2017-08-29/jobTemplates/{system_template}"),
            body: Some(json!({ "description": "no" })),
            operation: "UpdateJobTemplate",
            status: 403,
            error_type: "ForbiddenException",
        },
        Case {
            label: "DeleteQueue on a queue still holding a job",
            method: "DELETE",
            uri: format!("/2017-08-29/queues/{BUSY_QUEUE}"),
            body: None,
            operation: "DeleteQueue",
            status: 409,
            error_type: "ConflictException",
        },
        Case {
            // Not the handler's refusal but the framework's, raised while the operation
            // was being served: `MaxResults` is modelled `@range(min: 1, max: 20)`
            // (`smithy_query(query="operation",operation="ListJobs")`), and this service
            // states such a refusal as its own `BadRequestException`.
            label: "ListJobs with maxResults past its modelled bound",
            method: "GET",
            uri: "/2017-08-29/jobs?maxResults=21".to_string(),
            body: None,
            operation: "ListJobs",
            status: 400,
            error_type: "BadRequestException",
        },
    ];

    (emulator, cases)
}

/// The members of the envelope, asserted against the case that produced them.
fn assert_envelope(case: &Case, reply: &Reply) {
    let label = case.label;
    assert_eq!(
        reply.status, case.status,
        "case {label:?}: expected HTTP {}, got {} with body {}",
        case.status, reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(case.error_type),
        "case {label:?}: expected the {:?} shape on the wire, got {:?}",
        case.error_type,
        reply.error_type
    );
    assert_eq!(
        reply.at("/errorType").as_str(),
        Some(case.error_type),
        "case {label:?}: expected errorType {:?}, got body {}",
        case.error_type,
        reply.body
    );
    assert_eq!(
        reply.at("/httpStatus").as_u64(),
        Some(u64::from(case.status)),
        "case {label:?}: expected httpStatus {} as a number, got body {}",
        case.status,
        reply.body
    );
    assert_eq!(
        reply.at("/operation").as_str(),
        Some(case.operation),
        "case {label:?}: expected operation {:?}, got body {}",
        case.operation,
        reply.body
    );
    assert_eq!(
        reply.at("/accountId").as_str(),
        Some(::engine::emulator_account_id()),
        "case {label:?}: expected the caller's account, got body {}",
        reply.body
    );
    assert_eq!(
        reply.at("/settingsValidationErrorsJsonBlob").as_str(),
        Some(""),
        "case {label:?}: a failure that is not a settings-validation failure carries an \
         empty blob, got body {}",
        reply.body
    );
    // Only the PRESENCE of the request id is a requirement — its value is per-response —
    // so what is asserted is that one is there and that it is not empty.
    let request_id = reply.string("/requestId");
    assert!(
        !request_id.is_empty(),
        "case {label:?}: expected a requestId, got body {}",
        reply.body
    );
    // The envelope completes the error body; it does not replace it.
    assert!(
        !reply.string("/message").is_empty(),
        "case {label:?}: the message must survive the envelope, got body {}",
        reply.body
    );
}

/// Every failed call carries the envelope, whichever operation failed, with whichever
/// code, at whichever status, and whichever layer refused it.
#[tokio::test]
async fn every_error_body_carries_the_service_error_envelope() {
    let (emulator, cases) = seeded().await;

    for case in &cases {
        let reply = emulator
            .send(case.method, &case.uri, case.body.clone())
            .await;
        assert_envelope(case, &reply);
    }
}

/// The request id names ONE request: two identical failed calls carry two different ids,
/// while every other member of the envelope is identical between them.
#[tokio::test]
async fn the_request_id_is_per_response_and_the_rest_of_the_envelope_is_not() {
    let (emulator, cases) = seeded().await;

    for case in &cases {
        let first = emulator
            .send(case.method, &case.uri, case.body.clone())
            .await;
        let second = emulator
            .send(case.method, &case.uri, case.body.clone())
            .await;
        assert_envelope(case, &first);
        assert_envelope(case, &second);

        assert_ne!(
            first.string("/requestId"),
            second.string("/requestId"),
            "case {:?}: two responses must carry two request ids, got {} twice",
            case.label,
            first.body
        );
        for member in [
            "/errorType",
            "/httpStatus",
            "/operation",
            "/accountId",
            "/settingsValidationErrorsJsonBlob",
            "/message",
        ] {
            assert_eq!(
                first.at(member),
                second.at(member),
                "case {:?}: {member} must not change between two identical calls, got {} \
                 then {}",
                case.label,
                first.body,
                second.body
            );
        }
    }
}

/// The envelope belongs to an error body: a successful response carries none of its
/// members, so nothing was stamped on every response indiscriminately.
#[tokio::test]
async fn a_successful_response_carries_no_envelope_members() {
    let (emulator, _) = seeded().await;

    let created = emulator
        .create_queue(json!({ "name": "envelope-success" }))
        .await;
    let read = emulator.get_queue("envelope-success").await;
    let listed = emulator.list_job_templates("listBy=SYSTEM").await;
    // The jobs-query plane's own success, so the operation this suite's 404 row is about
    // is covered on both sides: a query that WAS started reads back as a 200 with no
    // envelope member on it.
    let started = emulator
        .send("POST", "/2017-08-29/jobsQueries", Some(json!({})))
        .await;
    let results = emulator
        .send(
            "GET",
            &format!("/2017-08-29/jobsQueries/{}", started.string("/id")),
            None,
        )
        .await;

    for (label, reply, status) in [
        ("CreateQueue", &created, 201),
        ("GetQueue", &read, 200),
        ("ListJobTemplates", &listed, 200),
        ("StartJobsQuery", &started, 201),
        ("GetJobsQueryResults", &results, 200),
    ] {
        assert_eq!(
            reply.status, status,
            "case {label:?}: expected HTTP {status}, got {} with body {}",
            reply.status, reply.body
        );
        for member in [
            "/errorType",
            "/httpStatus",
            "/accountId",
            "/requestId",
            "/operation",
            "/settingsValidationErrorsJsonBlob",
        ] {
            assert_eq!(
                reply.at(member),
                &Value::Null,
                "case {label:?}: a success must carry no {member}, got body {}",
                reply.body
            );
        }
    }
}
