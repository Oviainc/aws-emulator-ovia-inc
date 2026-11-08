// SPDX-License-Identifier: CC0-1.0

//! What the S3 Batch Operations job surface says when it will not carry out a
//! well-formed request, driven end to end over HTTP.
//!
//! The rule under test: every such refusal travels in this service's own XML
//! envelope at HTTP 400 with the wire code `InvalidRequest`, carries the FIXED
//! sentence the service answers that class of failure with — no value from the
//! request interpolated into it — and names the response it is through the
//! `RequestId` and `HostId` beside the message. Which job, token, status, value
//! or tag the request named is the caller's own: it travels in the request the
//! caller sent, and what the store holds afterwards is what a read reports.
//!
//! The table varies what the rule generalises over: the failure class (a job's
//! object list named twice or not at all, an action named twice or not at all, a
//! completion report with nowhere to be stored, an idempotency token reused for
//! a different job, a status move the job's status does not admit, and a tag set
//! that is not a set), the operation and method that carries it (POST on the job
//! collection, POST on a job's status, PUT on a job's tagging), and the values
//! the refused request carried.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder, the create and describe calls, the listing and the
//! correlation-pair reader are the sibling suites', reused rather than copied.

use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_PREPARATION, JOB_STATUS_READY,
};

use crate::access_grants_refusal_identities::assert_names_the_request;
use crate::access_point_lifecycle::{xml_escape, Emulator, Reply, ACCOUNT, XMLNS};
use crate::batch_job_lifecycle::{describe_job, ACTION_PUT_OBJECT_COPY};
use crate::batch_job_list::list_jobs;

/// The wire code and status every refusal below carries.
const INVALID_REQUEST: &str = "InvalidRequest";
const REFUSED: u16 = 400;

/// The fixed sentence this surface answers a create that names neither of the
/// two members a job's object list can come from with.
const SOURCE_REQUIRED: &str = "Either a Manifest or a ManifestGenerator must be provided.";

/// The fixed sentence it answers a create that names BOTH of them with: the
/// same words, closing with the clause that states which half of the one rule
/// the request broke.
const SOURCE_AMBIGUOUS: &str =
    "Either a Manifest or a ManifestGenerator must be provided, but not both";

/// The fixed sentence it answers a `ClientRequestToken` reused for a different
/// job with.
const DUPLICATE_TOKEN: &str = "Duplicate ClientRequestToken";

/// The fixed sentence it answers a status move the job's status does not admit
/// with.
const JOB_STATUS_FORBIDDEN: &str = "Requested job status forbidden";

/// The fixed sentence it answers every other request it will not carry out
/// with, whatever the member and whatever the value.
const REQUEST_INVALID: &str = "Request invalid";

/// The IAM role every create below submits. Satisfies the modelled `IAMRoleArn`
/// pattern (`^arn:[^:]+:iam::\d{12}:role/`), so no create is refused over it.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3BatchRefusals";

/// A `<Manifest>` element the model admits, for the rows that are refused over
/// something else.
const MANIFEST: &str =
    "<Manifest><Spec><Format>S3BatchOperations_CSV_20180820</Format><Fields><member>Bucket\
     </member><member>Key</member></Fields></Spec><Location>\
     <ObjectArn>arn:aws:s3:::refusal-manifests/objects.csv</ObjectArn><ETag>etag-0007</ETag>\
     </Location></Manifest>";

/// A `<ManifestGenerator>` element the model admits, so a row can name the
/// generator BESIDE the manifest above.
const GENERATOR: &str = "<ManifestGenerator><S3JobManifestGenerator>\
     <SourceBucket>arn:aws:s3:::refusal-source</SourceBucket>\
     <EnableManifestOutput>false</EnableManifestOutput></S3JobManifestGenerator>\
     </ManifestGenerator>";

/// A second action, for the row whose `<Operation>` names two.
const ACTION_DELETE_OBJECT_TAGGING: &str = "<S3DeleteObjectTagging></S3DeleteObjectTagging>";

/// One row: the request that is refused, and the words the surface answers it
/// with.
struct Refusal {
    case: String,
    method: &'static str,
    uri: String,
    body: String,
    /// The fixed sentence this class is answered with.
    message: &'static str,
    /// Values the refused REQUEST carried, none of which may appear in the
    /// message the refusal answers.
    carried: Vec<String>,
}

/// The `<Report>` body of a create that asks for no completion report, so a row
/// carrying one is refused over the report and nothing else.
const REPORT_DISABLED: &str = "<Enabled>false</Enabled>";

/// The priority every create below submits: a value the modelled `JobPriority`
/// range (0..2147483647) admits.
const PRIORITY: &str = "7";

/// A `CreateJob` body carrying `token`, `operation`, `source` and `report`
/// verbatim, so a row can name two sources, no action or a report with nowhere
/// to be stored.
fn create_body(token: &str, operation: &str, source: &str, report: &str) -> String {
    create_body_of(token, operation, source, report, PRIORITY, "")
}

/// [`create_body`] carrying `priority` and the `extra` members a row needs, so
/// two creates under ONE token can differ in exactly one member.
fn create_body_of(
    token: &str,
    operation: &str,
    source: &str,
    report: &str,
    priority: &str,
    extra: &str,
) -> String {
    format!(
        "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>{}</ClientRequestToken>\
         <Operation>{operation}</Operation><Report>{report}</Report>{source}{extra}\
         <Priority>{priority}</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>",
        xml_escape(token)
    )
}

/// A create of the job every row that addresses one addresses, under `token`.
fn plain_create_body(token: &str) -> String {
    create_body(token, ACTION_PUT_OBJECT_COPY, MANIFEST, REPORT_DISABLED)
}

/// A `PutJobTagging` body carrying `tags` in the order given, so a row can name
/// one key twice.
fn tagging_body(tags: &[(&str, &str)]) -> String {
    let members: String = tags
        .iter()
        .map(|(key, value)| {
            format!(
                "<member><Key>{}</Key><Value>{}</Value></member>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect();
    format!(
        "<PutJobTaggingRequest xmlns=\"{XMLNS}\"><Tags>{members}</Tags></PutJobTaggingRequest>"
    )
}

/// Assert `reply` is the refusal this surface answers `message` with, whole: the
/// status and code a client branches on, the message as FIXED text, nothing the
/// request carried inside it, and the pair that names the response.
fn assert_refused(row: &Refusal, reply: &Reply) {
    let case = row.case.as_str();
    assert_eq!(
        reply.status, REFUSED,
        "case {case:?}: expected HTTP {REFUSED}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.code(),
        INVALID_REQUEST,
        "case {case:?}: expected code {INVALID_REQUEST}, got body {}",
        reply.body
    );
    let answered = reply.text("Message").unwrap_or_default();
    assert_eq!(
        answered, row.message,
        "case {case:?}: expected the fixed message {:?}, got {answered:?} in {}",
        row.message, reply.body
    );
    for value in &row.carried {
        assert!(
            !answered.contains(value),
            "case {case:?}: expected the message to carry nothing the request named, got \
             {value:?} inside {answered:?}"
        );
    }
    assert_names_the_request(case, "the refusal", reply);
}

/// Create one job under `ACCOUNT` and `token` and answer the id the service
/// assigned.
async fn job_of(emulator: &Emulator, token: &str) -> String {
    let created = emulator
        .call("POST", "/v20180820/jobs", ACCOUNT, &plain_create_body(token))
        .await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: expected CreateJob to answer 200, got {} with body {}",
        created.status, created.body
    );
    created
        .text("JobId")
        .unwrap_or_else(|| panic!("case {token:?}: CreateJob answers a JobId: {}", created.body))
}

#[tokio::test]
async fn every_job_refusal_answers_the_surfaces_fixed_words_and_records_nothing() {
    let emulator = Emulator::start();

    // The jobs the rows below address: one the clock will move on its own, and
    // one that has already ended, so a status row can ask for a move out of a
    // terminal status.
    let running = job_of(&emulator, "token-alpha").await;
    let ended = job_of(&emulator, "token-beta").await;
    let cancelled = emulator
        .call(
            "POST",
            &format!("/v20180820/jobs/{ended}/status?requestedJobStatus=Cancelled"),
            ACCOUNT,
            "",
        )
        .await;
    assert_eq!(
        cancelled.status, 200,
        "expected the first cancellation to answer 200, got {} with body {}",
        cancelled.status, cancelled.body
    );

    // A token that already created a job, so a row can reuse it for a
    // different one.
    let held_token = "token-gamma";
    let token_job = job_of(&emulator, held_token).await;

    // What every surviving job reports before any refusal, so the rows can be
    // held to changing nothing. The clock's own `New` -> `Ready` move is let
    // fall due first, so what the rows are compared against is the job the
    // clock has settled rather than a status that was about to change anyway.
    tokio::time::sleep(JOB_PREPARATION + Duration::from_millis(50)).await;
    let before = [&running, &ended, &token_job];
    let mut described_before = Vec::new();
    for job_id in before {
        let described = describe_job(&emulator, ACCOUNT, job_id).await;
        assert_eq!(
            described.status, 200,
            "case {job_id:?}: expected the job to be readable, got {} with body {}",
            described.status, described.body
        );
        described_before.push(described.body);
    }
    assert!(
        described_before[0].contains(&format!("<Status>{JOB_STATUS_READY}</Status>")),
        "expected the job the rows address to have settled in {JOB_STATUS_READY}, got {}",
        described_before[0]
    );
    let listed_before = list_jobs(&emulator, ACCOUNT, "").await.body;

    let mut rows = vec![
        Refusal {
            case: "a create naming both a manifest and a generator".to_string(),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            body: create_body(
                "token-delta",
                ACTION_PUT_OBJECT_COPY,
                &format!("{MANIFEST}{GENERATOR}"),
                REPORT_DISABLED,
            ),
            message: SOURCE_AMBIGUOUS,
            carried: vec!["token-delta".to_string(), "etag-0007".to_string()],
        },
        Refusal {
            case: "a create naming neither a manifest nor a generator".to_string(),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            body: create_body("token-epsilon", ACTION_PUT_OBJECT_COPY, "", REPORT_DISABLED),
            message: SOURCE_REQUIRED,
            carried: vec!["token-epsilon".to_string(), ROLE_ARN.to_string()],
        },
        Refusal {
            case: "a create whose Operation names no action".to_string(),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            body: create_body("token-zeta", "", MANIFEST, REPORT_DISABLED),
            message: REQUEST_INVALID,
            carried: vec!["token-zeta".to_string(), ROLE_ARN.to_string()],
        },
        Refusal {
            case: "a create whose Operation names two actions".to_string(),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            body: create_body(
                "token-eta",
                &format!("{ACTION_PUT_OBJECT_COPY}{ACTION_DELETE_OBJECT_TAGGING}"),
                MANIFEST,
                REPORT_DISABLED,
            ),
            message: REQUEST_INVALID,
            carried: vec!["token-eta".to_string(), "S3DeleteObjectTagging".to_string()],
        },
        Refusal {
            case: "a create asking for a report with no bucket to store it in".to_string(),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            body: create_body(
                "token-theta",
                ACTION_PUT_OBJECT_COPY,
                MANIFEST,
                "<Enabled>true</Enabled><Format>Report_CSV_20180820</Format>",
            ),
            message: REQUEST_INVALID,
            carried: vec!["token-theta".to_string(), "Report_CSV_20180820".to_string()],
        },
        Refusal {
            case: "a create asking for a report whose bucket is empty".to_string(),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            body: create_body(
                "token-iota",
                ACTION_PUT_OBJECT_COPY,
                MANIFEST,
                "<Enabled>true</Enabled><Bucket></Bucket><Format>Report_CSV_20180820</Format>",
            ),
            message: REQUEST_INVALID,
            carried: vec!["token-iota".to_string()],
        },
        Refusal {
            case: "confirming a job that awaits no confirmation".to_string(),
            method: "POST",
            uri: format!("/v20180820/jobs/{running}/status?requestedJobStatus=Ready"),
            body: String::new(),
            message: JOB_STATUS_FORBIDDEN,
            carried: vec![running.clone(), "Ready".to_string()],
        },
        Refusal {
            case: "cancelling a job that has already ended".to_string(),
            method: "POST",
            uri: format!(
                "/v20180820/jobs/{ended}/status?requestedJobStatus=Cancelled\
                 &statusUpdateReason=a-reason-a-refusal-must-not-echo"
            ),
            body: String::new(),
            message: JOB_STATUS_FORBIDDEN,
            carried: vec![
                ended.clone(),
                "Cancelled".to_string(),
                "a-reason-a-refusal-must-not-echo".to_string(),
            ],
        },
        Refusal {
            case: "a tag set naming one key twice".to_string(),
            method: "PUT",
            uri: format!("/v20180820/jobs/{running}/tagging"),
            body: tagging_body(&[("stage", "archived"), ("stage", "live")]),
            message: REQUEST_INVALID,
            carried: vec![running.clone(), "archived".to_string(), "live".to_string()],
        },
    ];

    // A token that already created a job names THAT job: a request under it
    // asking for a different one is refused, whichever member differs.
    for (variation, priority, extra) in [
        (
            "a different description",
            PRIORITY,
            "<Description>a different job under one token</Description>",
        ),
        (
            "a different tag set",
            PRIORITY,
            "<Tags><member><Key>owner</Key><Value>someone-else</Value></member></Tags>",
        ),
        ("a different priority", "11", ""),
    ] {
        rows.push(Refusal {
            case: format!("a create reusing one token for {variation}"),
            method: "POST",
            uri: "/v20180820/jobs".to_string(),
            // The same create the token already carried, differing in ONE
            // member, so the row is refused over the token rather than over the
            // job it asks for.
            body: create_body_of(
                held_token,
                ACTION_PUT_OBJECT_COPY,
                MANIFEST,
                REPORT_DISABLED,
                priority,
                extra,
            ),
            message: DUPLICATE_TOKEN,
            carried: vec![held_token.to_string(), token_job.clone()],
        });
    }

    for row in &rows {
        let refused = emulator
            .call(row.method, &row.uri, ACCOUNT, &row.body)
            .await;
        assert_refused(row, &refused);
        assert!(
            refused.text("JobId").is_none(),
            "case {:?}: expected a refused request to name no job, got body {}",
            row.case,
            refused.body
        );
    }

    // Two refusals name two responses: the correlation pair is per-response,
    // not a constant this service repeats.
    let first = emulator
        .call("POST", &rows[0].uri, ACCOUNT, &rows[0].body)
        .await;
    let second = emulator
        .call("POST", &rows[1].uri, ACCOUNT, &rows[1].body)
        .await;
    assert_ne!(
        first.text("RequestId"),
        second.text("RequestId"),
        "expected two refusals to carry two RequestIds, got {} and {}",
        first.body,
        second.body
    );

    // Nothing the refused requests asked for was recorded: every job reports
    // what it reported before, and the listing holds the same jobs — no job was
    // created under a refused token, no status moved and no tag set was stored.
    for (job_id, was) in before.iter().zip(described_before) {
        let now = describe_job(&emulator, ACCOUNT, job_id).await;
        assert_eq!(
            now.body, was,
            "case {job_id:?}: expected a refused request to leave the job as it was, got {} \
             from {was}",
            now.body
        );
    }
    let listed_now = list_jobs(&emulator, ACCOUNT, "").await;
    assert_eq!(
        listed_now.body, listed_before,
        "expected the refused creates to add no job to the listing, got {} from {listed_before}",
        listed_now.body
    );
}
