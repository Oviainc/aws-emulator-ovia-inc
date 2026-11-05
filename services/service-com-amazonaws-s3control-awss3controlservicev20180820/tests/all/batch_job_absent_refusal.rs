// SPDX-License-Identifier: CC0-1.0

//! What the S3 Batch Operations job surface says when it refuses a request,
//! driven end to end over HTTP.
//!
//! Two rules, one per test:
//!
//! 1. a refusal for a job id the account owns no job for is answered, by EVERY
//!    operation that addresses a job by id, in the fixed words the service
//!    answers that class of failure with — `Job not found`, with no variable
//!    part, so the id the caller asked about is not interpolated into the
//!    sentence;
//! 2. a refusal of a job request over a value the model does not admit is the
//!    ONE refusal this surface answers a request it will not carry out with,
//!    whatever the member and whatever the value: the fixed `Request invalid`,
//!    rather than a sentence restating the value and the rule it broke.
//!
//! Both travel at HTTP 400 with code `InvalidRequest`, and both name the
//! response they are rather than the request's values, through the `RequestId`
//! and `HostId` the error envelope carries beside the message and the
//! `x-amz-request-id` / `x-amz-id-2` headers that mirror them.
//!
//! The tables below vary what the rules generalise over: the operation that
//! addresses the job (all six the model binds a `JobId` into — a read, two
//! updates and the three tagging calls, across GET, POST, PUT and DELETE), the
//! shape of the absent id (both published bounds of the modelled `JobId`, the
//! character classes its pattern admits, an id one character away from a job
//! that really is there, and the 36-character hyphenated form a client is most
//! likely to send), whether the id names no job at all or a job owned by a
//! DIFFERENT account, and — for the second rule — which member carries the
//! value the model does not admit and which kind of rule it breaks (an enum's
//! value set, including a value differing only in case; a range, at one past
//! each published bound; a length; and a pattern).
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder and the create call are the sibling suites', reused rather
//! than copied.

use crate::access_grants_refusal_identities::assert_names_the_request;
use crate::access_point_lifecycle::{
    xml_escape, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, XMLNS,
};
use crate::batch_job_lifecycle::{
    create_job, describe_job, JobCase, ACTION_PUT_OBJECT_COPY, MANIFEST_CSV,
};

/// The fixed sentence S3 Batch Operations answers for a job id the account owns
/// no job for. No variable part: the id that was looked up is the caller's own,
/// carried by the request it sent.
const JOB_NOT_FOUND: &str = "Job not found";

/// The wire code every refusal below carries, and the status it travels at.
const INVALID_REQUEST: &str = "InvalidRequest";
const REFUSED: u16 = 400;

/// A priority the modelled `JobPriority` range admits, so a priority update is
/// refused over the job id and nothing else.
const ADMITTED_PRIORITY: &str = "10";

/// A value the modelled `RequestedJobStatus` enum carries, so a status update is
/// refused over the job id and nothing else.
const ADMITTED_STATUS: &str = "Cancelled";

/// The published bounds of the modelled `JobId`: `length 5..36`, pattern
/// `^[a-zA-Z0-9\-\_]+$` (`smithy_query(query="shape", shape="JobId")`).
const JOB_ID_LEN: std::ops::RangeInclusive<usize> = 5..=36;

/// Every operation that addresses a Batch Operations job by its id — the six
/// the model binds a `JobId` into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobCall {
    Describe,
    UpdatePriority,
    UpdateStatus,
    GetTagging,
    PutTagging,
    DeleteTagging,
}

impl JobCall {
    /// Every call of the surface, as the table below runs them.
    const ALL: [Self; 6] = [
        Self::Describe,
        Self::UpdatePriority,
        Self::UpdateStatus,
        Self::GetTagging,
        Self::PutTagging,
        Self::DeleteTagging,
    ];

    /// The operation name, for the assertion messages.
    fn name(self) -> &'static str {
        match self {
            Self::Describe => "DescribeJob",
            Self::UpdatePriority => "UpdateJobPriority",
            Self::UpdateStatus => "UpdateJobStatus",
            Self::GetTagging => "GetJobTagging",
            Self::PutTagging => "PutJobTagging",
            Self::DeleteTagging => "DeleteJobTagging",
        }
    }

    /// Address `job_id` under `account_id` through this operation, with every
    /// other member of the request a value the model admits.
    async fn issue(self, emulator: &Emulator, account_id: &str, job_id: &str) -> Reply {
        match self {
            Self::Describe => describe_job(emulator, account_id, job_id).await,
            Self::UpdatePriority => {
                emulator
                    .call(
                        "POST",
                        &format!("/v20180820/jobs/{job_id}/priority?priority={ADMITTED_PRIORITY}"),
                        account_id,
                        "",
                    )
                    .await
            }
            Self::UpdateStatus => {
                emulator
                    .call(
                        "POST",
                        &format!(
                            "/v20180820/jobs/{job_id}/status?requestedJobStatus={ADMITTED_STATUS}"
                        ),
                        account_id,
                        "",
                    )
                    .await
            }
            Self::GetTagging => {
                emulator
                    .call(
                        "GET",
                        &format!("/v20180820/jobs/{job_id}/tagging"),
                        account_id,
                        "",
                    )
                    .await
            }
            Self::PutTagging => {
                let body = format!(
                    "<PutJobTaggingRequest xmlns=\"{XMLNS}\"><Tags><member><Key>{}</Key>\
                     <Value>{}</Value></member></Tags></PutJobTaggingRequest>",
                    xml_escape("stage"),
                    xml_escape("archived")
                );
                emulator
                    .call(
                        "PUT",
                        &format!("/v20180820/jobs/{job_id}/tagging"),
                        account_id,
                        &body,
                    )
                    .await
            }
            Self::DeleteTagging => {
                emulator
                    .call(
                        "DELETE",
                        &format!("/v20180820/jobs/{job_id}/tagging"),
                        account_id,
                        "",
                    )
                    .await
            }
        }
    }
}

/// One row: the case name, the id the request names and the account it names it
/// under.
struct AbsentJob {
    case: String,
    job_id: String,
    account_id: &'static str,
}

/// Assert `reply` is the refusal this surface answers `message` with: the status
/// and code the whole surface answers, the message as FIXED text, and the
/// correlation pair that names the response, in the envelope and in the two
/// headers that mirror it (asserted by the sibling suite's reader, which every
/// refusal of this service is held to).
fn assert_refusal(case: &str, reply: &Reply, message: &str) {
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
        answered, message,
        "case {case:?}: expected the fixed message {message:?}, got {answered:?} in {}",
        reply.body
    );
    assert_names_the_request(case, "the refusal", reply);
}

/// Create one job under `account_id` and answer the id the service assigned.
async fn existing_job(emulator: &Emulator, account_id: &str, token: &str) -> String {
    let case = JobCase::new("a job that is there", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV);
    let created = create_job(emulator, account_id, &case, token).await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: expected CreateJob to answer 200, got {} with body {}",
        created.status, created.body
    );
    created
        .text("JobId")
        .unwrap_or_else(|| panic!("case {token:?}: CreateJob answers a JobId: {}", created.body))
}

/// The same id with its last character changed, so a row can name an id one
/// character away from a job that really is there.
fn one_character_off(job_id: &str) -> String {
    let mut characters: Vec<char> = job_id.chars().collect();
    let last = characters.len() - 1;
    characters[last] = if characters[last] == 'z' { 'y' } else { 'z' };
    characters.into_iter().collect()
}

/// The ids every call of the surface is run over: each is a value the modelled
/// `JobId` admits, and none of them names a job of the account the row names.
fn absent_jobs(owned_elsewhere: &str, nearly_owned: &str) -> Vec<AbsentJob> {
    let row = |case: &str, job_id: String, account_id: &'static str| AbsentJob {
        case: case.to_string(),
        job_id,
        account_id,
    };
    vec![
        row(
            "the 36-character hyphenated form, naming no job",
            "11111111-2222-3333-4444-555555555555".to_string(),
            ACCOUNT,
        ),
        row(
            "the shortest id the model admits",
            "a".repeat(*JOB_ID_LEN.start()),
            ACCOUNT,
        ),
        row(
            "the longest id the model admits",
            "Z".repeat(*JOB_ID_LEN.end()),
            ACCOUNT,
        ),
        row(
            "an id carrying the underscores and mixed case the pattern admits",
            "Job_Id_0001".to_string(),
            ACCOUNT,
        ),
        row(
            "one character away from a job the account owns",
            one_character_off(nearly_owned),
            ACCOUNT,
        ),
        row(
            "a job id another account owns",
            owned_elsewhere.to_string(),
            ACCOUNT,
        ),
        row(
            "a job id of the first account, asked for by the second",
            nearly_owned.to_string(),
            OTHER_ACCOUNT,
        ),
    ]
}

#[tokio::test]
async fn every_job_call_refuses_an_absent_job_id_in_the_services_fixed_words() {
    let emulator = Emulator::start();

    // Two jobs that really are there, one per account: the rows below are about
    // the id the request names, not about an empty store.
    let owned_here = existing_job(&emulator, ACCOUNT, "owned-here").await;
    let owned_elsewhere = existing_job(&emulator, OTHER_ACCOUNT, "owned-elsewhere").await;

    for row in absent_jobs(&owned_elsewhere, &owned_here) {
        for call in JobCall::ALL {
            let case = format!("{} of {}: {}", call.name(), row.job_id, row.case);
            let refused = call.issue(&emulator, row.account_id, &row.job_id).await;
            assert_refusal(&case, &refused, JOB_NOT_FOUND);
            assert!(
                !refused
                    .text("Message")
                    .unwrap_or_default()
                    .contains(&row.job_id),
                "case {case:?}: expected the message to carry no job id, got body {}",
                refused.body
            );
        }
    }

    // Two refusals name two responses: the pair is per-response, not a constant.
    let first = describe_job(&emulator, ACCOUNT, "absent-job-one").await;
    let second = describe_job(&emulator, ACCOUNT, "absent-job-two").await;
    assert_ne!(
        first.text("RequestId"),
        second.text("RequestId"),
        "expected two refusals to carry two RequestIds, got {} and {}",
        first.body,
        second.body
    );

    // And every job that really is there is still readable: a refusal changed
    // nothing on either account's jobs.
    for (account_id, job_id) in [(ACCOUNT, &owned_here), (OTHER_ACCOUNT, &owned_elsewhere)] {
        let described = describe_job(&emulator, account_id, job_id).await;
        assert_eq!(
            described.status, 200,
            "expected the job {job_id:?} of {account_id} to stay readable, got {} with body {}",
            described.status, described.body
        );
    }
}

/// The published bounds of the members the rows below drive one past, as the
/// model declares them: `JobPriority` range 0..2147483647 and
/// `JobStatusUpdateReason` length 1..256
/// (`smithy_query(query="operation", operation="UpdateJobStatus")` and
/// `operation="UpdateJobPriority"`).
const PRIORITY_RANGE: std::ops::RangeInclusive<i64> = 0..=2_147_483_647;
const REASON_LEN: std::ops::RangeInclusive<usize> = 1..=256;

/// The fixed sentence this surface answers a request it will not carry out
/// with. No variable part: neither the value nor the rule it broke is restated
/// to the caller that sent it.
const REQUEST_INVALID: &str = "Request invalid";

/// One row of the second table: the case name, and the request that carries a
/// value the model does not admit — its method, its URI over the job that
/// really is there, and the document it sends.
struct OutsideTheModel {
    case: &'static str,
    method: &'static str,
    /// The request URI, built from the id of a job the account really owns, so
    /// the refusal is about the VALUE and not about the id.
    uri: Box<dyn Fn(&str) -> String>,
    /// The request body, empty for the routes that carry their values in the
    /// URI.
    body: String,
}

/// The IAM role the `CreateJob` row below submits, satisfying the modelled
/// `IAMRoleArn` pattern (`^arn:[^:]+:iam::\d{12}:role/`) so that create is
/// refused over the ONE value the model does not admit.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3BatchOperations";

/// Every way a job request can carry a value the model does not admit: an
/// enum's value set, a range at one past each published bound, a length, a
/// pattern, and a value that is not of its member's type at all — over the
/// collection route a job is created on as well as the per-job routes.
fn outside_the_model() -> Vec<OutsideTheModel> {
    let row = |case: &'static str, method: &'static str, uri: Box<dyn Fn(&str) -> String>| {
        OutsideTheModel {
            case,
            method,
            uri,
            body: String::new(),
        }
    };
    vec![
        row(
            "a requested status the RequestedJobStatus enum does not carry",
            "POST",
            Box::new(|job_id| format!("/v20180820/jobs/{job_id}/status?requestedJobStatus=Complete")),
        ),
        row(
            "a requested status differing from an admitted one only in case",
            "POST",
            Box::new(|job_id| {
                format!("/v20180820/jobs/{job_id}/status?requestedJobStatus=cancelled")
            }),
        ),
        row(
            "a status update reason one character past the longest admitted",
            "POST",
            Box::new(|job_id| {
                format!(
                    "/v20180820/jobs/{job_id}/status?requestedJobStatus=Cancelled\
                     &statusUpdateReason={}",
                    "r".repeat(REASON_LEN.end() + 1)
                )
            }),
        ),
        row(
            "a priority one below the lowest admitted",
            "POST",
            Box::new(|job_id| {
                format!(
                    "/v20180820/jobs/{job_id}/priority?priority={}",
                    PRIORITY_RANGE.start() - 1
                )
            }),
        ),
        row(
            "a priority one above the highest admitted",
            "POST",
            Box::new(|job_id| {
                format!(
                    "/v20180820/jobs/{job_id}/priority?priority={}",
                    PRIORITY_RANGE.end() + 1
                )
            }),
        ),
        row(
            "a priority that is not a number at all",
            "POST",
            Box::new(|job_id| format!("/v20180820/jobs/{job_id}/priority?priority=soon")),
        ),
        row(
            "a job id shorter than the modelled JobId admits",
            "GET",
            Box::new(|_| "/v20180820/jobs/ab".to_string()),
        ),
        row(
            "a job id carrying a character the modelled JobId pattern excludes",
            "GET",
            Box::new(|_| "/v20180820/jobs/job%20id".to_string()),
        ),
        // The collection route a job is CREATED on is in the same path space,
        // so a create carrying a priority past the modelled range is refused in
        // the same words.
        OutsideTheModel {
            case: "a create whose priority is one above the highest admitted",
            method: "POST",
            uri: Box::new(|_| "/v20180820/jobs".to_string()),
            body: format!(
                "<CreateJobRequest xmlns=\"{XMLNS}\"><ClientRequestToken>outside</ClientRequestToken>\
                 <Operation>{ACTION_PUT_OBJECT_COPY}</Operation>\
                 <Report><Enabled>false</Enabled></Report>{MANIFEST_CSV}\
                 <Priority>{}</Priority><RoleArn>{ROLE_ARN}</RoleArn></CreateJobRequest>",
                PRIORITY_RANGE.end() + 1
            ),
        },
    ]
}

#[tokio::test]
async fn a_job_request_outside_the_model_is_refused_in_the_services_own_words() {
    let emulator = Emulator::start();

    // A job that really is there, so each row below is refused over the VALUE
    // it carries rather than over the job it names.
    let job_id = existing_job(&emulator, ACCOUNT, "outside-the-model").await;
    let before = describe_job(&emulator, ACCOUNT, &job_id).await;

    let mut refused_creates = 0;
    for OutsideTheModel {
        case,
        method,
        uri,
        body,
    } in outside_the_model()
    {
        let uri = uri(&job_id);
        let refused = emulator.call(method, &uri, ACCOUNT, &body).await;
        let case = format!("{method} {uri}: {case}");
        assert_refusal(&case, &refused, REQUEST_INVALID);
        assert!(
            refused.text("JobId").is_none(),
            "case {case:?}: expected a refused request to record no job, got body {}",
            refused.body
        );
        refused_creates += usize::from(method == "POST" && uri == "/v20180820/jobs");
    }
    assert_eq!(
        refused_creates, 1,
        "expected the table to drive the create route too, so the widened scope is covered"
    );

    // Nothing the refused requests named was changed: the job is readable and
    // still carries the priority its create recorded, with no reason and no
    // termination recorded against it. The STATUS is not compared, because the
    // clock owes this job its own `New` -> `Ready` move whatever these requests
    // did.
    let described = describe_job(&emulator, ACCOUNT, &job_id).await;
    assert_eq!(
        described.status, 200,
        "expected the job to stay readable, got {} with body {}",
        described.status, described.body
    );
    assert_eq!(
        described.text("Priority"),
        before.text("Priority"),
        "expected every refused update to leave the stored priority at {:?}, got body {}",
        before.text("Priority"),
        described.body
    );
    assert!(
        described.text("StatusUpdateReason").is_none(),
        "expected every refused update to record no reason, got body {}",
        described.body
    );
    assert!(
        described.text("TerminationDate").is_none(),
        "expected every refused update to leave the job unterminated, got body {}",
        described.body
    );
}
