// SPDX-License-Identifier: CC0-1.0

//! The batch inference job resource end-to-end: `CreateModelInvocationJob` submits it,
//! `GetModelInvocationJob` reads it back and `ListModelInvocationJobs` reports it, over the
//! same `MultiServiceRouter` the emulator boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A submitted job round-trips**: the create answers 200 with a `jobArn` of the form
//!    `ModelInvocationJobArn` declares, and every member `GetModelInvocationJob` reports
//!    comes back out of the stored record — for requests differing in `modelId` spelling,
//!    invocation type, timeout (absent and both modelled bounds), VPC configuration, tag
//!    count, S3 input format and name length (both modelled bounds). Both
//!    `ModelInvocationJobIdentifier` spellings — the job ARN and the minted job id — address
//!    the same job.
//! 2. **`jobExpirationTime` is the submission plus the requested timeout**, over every row
//!    that sets one, and is absent for a job that set none — while an hour count the modelled
//!    `range 24..168` does not admit is refused 400 `ValidationException`, over rows spanning
//!    both bounds a tick outside the range, zero, a negative count and both extremes of the
//!    member's own integer type.
//! 3. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException`, over rows spanning a never-minted id, a well-formed ARN
//!    that was never minted and a live job of another region.
//! 4. **`modelId` is resolved against the published catalog**: an identifier that names no
//!    model of this account is refused as the spelling it IS — 404
//!    `ResourceNotFoundException` for an ARN of one of this service's own resource types,
//!    400 `ValidationException` for a bare model id, which is no ARN to report missing —
//!    while every model the catalog publishes is accepted.
//! 5. **`status` walks the documented progression on the submission's own clock**: `Submitted`
//!    with no `endTime` straight after the create, then `Validating`, then `Scheduled`, then
//!    `InProgress`, each dwelling its own span and each stamped at the instant that span fell
//!    due — the same status a listing reports and `statusEquals` filters on — and `Completed`
//!    with an `endTime` only once the run span has elapsed on top of them. Asserted stage by
//!    stage over the dwell of each one, and over the intervals a client polls a job at, four
//!    orders of magnitude apart: at every one of them the statuses the poller reads are a
//!    subsequence of the progression in its own order, the terminal status comes within the
//!    reads the elapsed time implies rather than within a read per stage, the first status
//!    read outside `{Submitted, Validating, Scheduled}` is the one that interval's own first
//!    poll past the start instant falls in, and reading twice reports the same instants.
//! 6. **A replayed `clientRequestToken` submits nothing**: the replay answers with the job
//!    the first call minted and stores no second job, while a fresh token submits another.
//! 7. **The listing orders, filters and pages** the stored jobs: creation order in both
//!    `sortOrder` directions, `nameContains` and the two submit-time bounds, pages that
//!    partition the listing through `nextToken`, a `nextToken` from nowhere refused, and no
//!    job of another region listed.
//! 8. **`StopModelInvocationJob` winds a job down that has not finished**: whichever
//!    unfinished status the job holds when the stop arrives, it moves to `Stopping`, the
//!    clock owes it the terminal `Stopped`, and it is owed neither its start nor its
//!    completion any more — while a stop of a job that has already left those statuses
//!    changes nothing and is answered the same way, and an identifier no stored job answers
//!    to is refused.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateModelInvocationJob,GetModelInvocationJob,ListModelInvocationJobs")`
//! / `query="shape"`), or from the published foundation model catalog this service reports
//! through `ListFoundationModels`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::support::{
    date_time_seconds, emulator, foundation_model_arn, label, wall_clock_seconds, Emulator, Reply,
    ACCOUNT, REGION,
};

// The instants this service's own spans put each stage of the progression at, derived once
// beside the shared batch inference harness so these suites cannot drift from one another
// when a span changes: `pre_start_span` is when the job is owed its start, `lifespan` the
// whole span one nobody stops takes to its terminal status.
use crate::model_invocation_job_execution::{
    completed_at as lifespan, in_progress_at as pre_start_span,
};

type State = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState;

/// One stored batch inference job, as the create wrote it into [`State`].
type JobRecord = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ModelInvocationJobRecord;

/// The instants a submission owes a job each of its status transitions at.
type JobClock = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ModelInvocationJobClock;

/// The published foundation model catalog this service serves: the rows
/// `ListFoundationModels` reports (`state::FOUNDATION_MODELS`), so these rows follow the
/// catalog the service serves rather than a spelling it may add to or drop.
const CATALOG: &[service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FoundationModel] =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS;

/// How long after it was submitted a job is owed its move to `Validating`.
const VALIDATES_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_VALIDATES_AFTER;

/// How long after it entered validation a job is owed its move to `Scheduled`.
const SCHEDULES_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_SCHEDULES_AFTER;

/// How long after it was scheduled a job is owed its move to `InProgress`.
const BEGINS_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_BEGINS_AFTER;

/// How long after it began a job is owed its move to `Completed`.
const COMPLETE_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_COMPLETE_AFTER;

/// How long after a stop was accepted the job is owed its move to `Stopped`.
const STOPPED_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_STOPPED_AFTER;

/// A margin on top of a settle span, so a read lands after the instant the move was owed
/// rather than exactly on it.
const MARGIN: Duration = Duration::from_millis(150);

/// How far apart two reported instants may be and still count as the same instant: the wire
/// form is an RFC 3339 date-time, so a stamp survives the round trip to the millisecond.
const INSTANT_TOLERANCE: f64 = 0.002;

/// `ModelInvocationJobStatus` values this emulator's lifecycle passes through, in the order
/// the member's documentation lists the progression
/// (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`).
const SUBMITTED: &str = "Submitted";
const VALIDATING: &str = "Validating";
const SCHEDULED: &str = "Scheduled";
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";
const STOPPING: &str = "Stopping";
const STOPPED: &str = "Stopped";

/// The statuses a job holds before it has begun: the set a client polls THROUGH while it
/// waits for a job to start.
const PRE_START: &[&str] = &[SUBMITTED, VALIDATING, SCHEDULED];

/// How long a case polls a job for before it concludes the progression is stuck.
///
/// An absolute bound, generous next to the spans this service arms: room for a slow machine
/// and not room for another order of magnitude.
const POLL_BUDGET: Duration = Duration::from_secs(45);

/// How long a case waits between two reads of a job while it polls it.
const POLL_STEP: Duration = Duration::from_millis(100);

/// Poll `GetModelInvocationJob` the way a client does until the job reports `target`, and
/// hand back the read that first reported it beside the distinct statuses the poll walked
/// through, in order.
///
/// Each stage of the progression dwells its own span on the submission's clock, so a case
/// reaches a stage by polling the way a client does — which is also what proves the stage is
/// one a poller can read the job in rather than one the elapsed time carries it past unseen.
async fn poll_until(emu: &Emulator, case: &str, arn: &str, target: &str) -> (Reply, Vec<String>) {
    let started = ::std::time::Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reply = get(emu, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: the job must stay readable while it is polled, got {} {:?}",
            reply.status, reply.body
        );
        let status = reply.text("status");
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if status == target {
            return (reply, seen);
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case}: a client polling the job must read it {target} inside \
             {POLL_BUDGET:?}, saw {seen:?}"
        );
        tokio::time::sleep(POLL_STEP).await;
    }
}

/// The stored record of one batch inference job, cloned out of the store the handlers share.
///
/// Cloned rather than borrowed so a case can settle it at an instant of its own choosing:
/// the clock rule is about spans of wall-clock time longer than a case wants to sleep
/// through, and ageing or settling a clone brings the same elapsed time.
fn stored_job(emu: &Emulator, arn: &str) -> JobRecord {
    let state =
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::service_state(
            &*emu.engine,
        )
        .expect("the shared engine carries this service's store");
    let store = state.lock();
    store
        .model_invocation_jobs
        .get(arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store must hold the submitted job {arn}"))
}

/// Age the stored job `arn` names by `by`, as though it had been submitted that much
/// earlier: its submission instant and every instant its clock owes a transition at move
/// back together, so the job's own timeline is untouched and only the amount of it that has
/// already elapsed changes.
fn age_job(emu: &Emulator, arn: &str, by: Duration) {
    let state =
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::service_state(
            &*emu.engine,
        )
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let job = store
        .model_invocation_jobs
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store must hold the submitted job {arn}"));
    job.submit_time -= by;
    job.last_modified_at -= by;
    if let Some(expires) = job.job_expiration_time.as_mut() {
        *expires -= by;
    }
    for due in [
        &mut job.validating_due_at,
        &mut job.scheduled_due_at,
        &mut job.in_progress_due_at,
        &mut job.completed_due_at,
        &mut job.stopped_due_at,
    ] {
        if let Some(instant) = due.as_mut() {
            *instant -= by;
        }
    }
}

/// Age the stored job `arn` names so that the completion its clock owes stands `remaining`
/// ahead of now: the elapsed time a client would have waited through, brought without
/// waiting for it.
///
/// The instant is read off the JOB rather than computed from the case's own stopwatch,
/// because the whole progression stands on the submission's own timeline.
fn age_job_to_remaining(emu: &Emulator, arn: &str, remaining: Duration) {
    let due = stored_job(emu, arn)
        .completed_due_at
        .unwrap_or_else(|| panic!("the job {arn} must still owe its completion"));
    let ahead = due
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    age_job(emu, arn, ahead.saturating_sub(remaining));
}

/// The `modelId` of the `index`-th model the published catalog carries.
fn catalog_model(index: usize) -> &'static str {
    CATALOG
        .get(index)
        .map(|model| model.model_id)
        .unwrap_or_else(|| {
            panic!(
                "the catalog must publish at least {} model(s), it publishes {}",
                index + 1,
                CATALOG.len()
            )
        })
}

/// `RoleArn` values the model admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-batch-inference",
    "arn:aws:iam::000000000000:role/service-role/BedrockBatchInferenceExecution",
];

/// `S3InputFormat` — the one value the modelled enum carries
/// (`crate::ops::types::S3_INPUT_FORMAT_VALUES`).
const S3_INPUT_FORMAT: &str = "JSONL";

/// The fixed sentence Amazon Bedrock states for a model identifier it cannot read as a
/// model at all, written out here rather than read off the crate so this suite pins the
/// wording a client reads.
const INVALID_MODEL_IDENTIFIER: &str = "The provided model identifier is invalid.";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// A create body carrying only the members the model marks required.
fn create_body(name: &str, model_id: &str) -> Value {
    json!({
        "jobName": name,
        "roleArn": ROLE_ARNS[0],
        "modelId": model_id,
        "inputDataConfig": {"s3InputDataConfig": {"s3Uri": "s3://batch-inference-in/prompts.jsonl"}},
        "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://batch-inference-out/"}},
    })
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/model-invocation-job", Some(body))
        .await
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    create_in_region(emu, REGION, body).await
}

async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/model-invocation-job/{}", label(identifier)),
        None,
    )
    .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    get_in_region(emu, REGION, identifier).await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/model-invocation-jobs".to_string()
    } else {
        format!("/model-invocation-jobs?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// `StopModelInvocationJob` is bound to `POST /model-invocation-job/{jobIdentifier}/stop`
/// and carries no request body
/// (`smithy_query(query="operation",operation="StopModelInvocationJob")`).
async fn stop_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        &format!("/model-invocation-job/{}/stop", label(identifier)),
        None,
    )
    .await
}

async fn stop(emu: &Emulator, identifier: &str) -> Reply {
    stop_in_region(emu, REGION, identifier).await
}

/// Submit one job and hand back its ARN, failing the case by name when the create is
/// refused.
async fn submit(emu: &Emulator, case: &str, body: Value) -> String {
    let reply = create(emu, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: CreateModelInvocationJob must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("jobArn")
}

// ---------------------------------------------------------------------------------------
// Response helpers.
// ---------------------------------------------------------------------------------------

/// The summaries a listing reported, in the order it reported them.
fn listed(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case}: the listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("invocationJobSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: invocationJobSummaries must be a present array, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The ARNs a listing reported, in order.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    listed(case, reply)
        .iter()
        .map(|row| text(case, row, "jobArn"))
        .collect()
}

fn member(case: &str, value: &Value, name: &str) -> Value {
    value
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the response has no member {name}: {value:?}"))
}

fn text(case: &str, value: &Value, name: &str) -> String {
    let member = member(case, value, name);
    member
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: member {name} must be a string, got {member:?}"))
        .to_string()
}

/// A `Timestamp` member in epoch seconds. Every one of them is modelled
/// `@timestampFormat("date-time")`, so the wire form is an RFC 3339 STRING.
fn instant(case: &str, value: &Value, name: &str) -> f64 {
    let member = member(case, value, name);
    let raw = member.as_str().unwrap_or_else(|| {
        panic!(
            "case {case}: member {name} is modelled @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 string, got {member}"
        )
    });
    date_time_seconds(case, raw)
}

/// `seconds` since the epoch as the RFC 3339 date-time the two submit-time filters carry on
/// the wire, rendered with the same codec the wire uses.
fn date_time_query(seconds: f64) -> String {
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant the date-time format can render")
}

/// Whether `arn` is a `ModelInvocationJobArn`:
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-invocation-job/[a-z0-9]{12}$`.
fn is_model_invocation_job_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:model-invocation-job/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    id.len() == 12
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The minted job id inside a `ModelInvocationJobArn` — the other spelling a
/// `ModelInvocationJobIdentifier` admits.
fn job_id_of(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("{arn} must carry a model-invocation-job/<id> tail"))
        .1
}

// ---------------------------------------------------------------------------------------
// 1 & 2. A submitted job round-trips, under both identifier spellings.
// ---------------------------------------------------------------------------------------

/// One row of the round-trip table: the create body, and what the read must report back.
struct Row {
    case: &'static str,
    body: Value,
    /// `timeoutDurationInHours` the row carried, if any — the hours
    /// `jobExpirationTime` must stand after `submitTime`.
    timeout_hours: Option<i64>,
    /// The `modelInvocationType` the read must report: the one the row named, or the
    /// member's modelled `@default("InvokeModel")` when it named none.
    invocation_type: &'static str,
}

fn round_trip_rows() -> Vec<Row> {
    // `jobName` is `@length(min: 1, max: 63)`, so the rows carry both bounds;
    // `timeoutDurationInHours` is `range 24..168`, so the rows carry both bounds and an
    // absent one; `modelInvocationType` is `[Converse, InvokeModel]` with a modelled
    // default; `modelId` admits a bare model id and a foundation model ARN, so both
    // spellings appear, each over a DIFFERENT catalog row.
    let long_name = "a".repeat(63);
    vec![
        Row {
            case: "required members only",
            body: create_body("batch-minimal", catalog_model(0)),
            timeout_hours: None,
            invocation_type: "InvokeModel",
        },
        Row {
            case: "the model's ARN spelling, the lower timeout bound",
            body: {
                let mut body = create_body(
                    "batch-arn-model",
                    &foundation_model_arn(REGION, catalog_model(1)),
                );
                body["timeoutDurationInHours"] = json!(24);
                body["modelInvocationType"] = json!("Converse");
                body
            },
            timeout_hours: Some(24),
            invocation_type: "Converse",
        },
        Row {
            case: "the upper timeout bound, a VPC, tags and an input format",
            body: {
                let mut body = create_body("batch-vpc-tagged", catalog_model(2));
                body["timeoutDurationInHours"] = json!(168);
                body["modelInvocationType"] = json!("InvokeModel");
                body["vpcConfig"] = json!({
                    "subnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                    "securityGroupIds": ["sg-00000000000000001"],
                });
                body["tags"] = json!([
                    {"key": "team", "value": "inference"},
                    {"key": "cost-centre", "value": ""},
                ]);
                body["inputDataConfig"] = json!({"s3InputDataConfig": {
                    "s3Uri": "s3://batch-inference-in/prompts.jsonl",
                    "s3InputFormat": S3_INPUT_FORMAT,
                    "s3BucketOwner": ACCOUNT,
                }});
                body["outputDataConfig"] = json!({"s3OutputDataConfig": {
                    "s3Uri": "s3://batch-inference-out/run/",
                    "s3BucketOwner": ACCOUNT,
                }});
                body
            },
            timeout_hours: Some(168),
            invocation_type: "InvokeModel",
        },
        Row {
            case: "the shortest name the model admits",
            body: {
                let mut body = create_body("a", catalog_model(3));
                body["roleArn"] = json!(ROLE_ARNS[1]);
                body["timeoutDurationInHours"] = json!(48);
                body
            },
            timeout_hours: Some(48),
            invocation_type: "InvokeModel",
        },
        Row {
            case: "the longest name the model admits",
            body: {
                let mut body = create_body(&long_name, catalog_model(4));
                body["clientRequestToken"] = json!("roundTripToken1");
                body
            },
            timeout_hours: None,
            invocation_type: "InvokeModel",
        },
    ]
}

#[tokio::test]
async fn a_submitted_job_round_trips_through_both_identifier_spellings() {
    let emu = emulator();

    for row in round_trip_rows() {
        let case = row.case;
        let arn = submit(&emu, case, row.body.clone()).await;
        assert!(
            is_model_invocation_job_arn(&arn, REGION),
            "case {case}: the create must answer a ModelInvocationJobArn of the signing region, \
             got {arn:?}"
        );

        // Both spellings of a `ModelInvocationJobIdentifier` address the one job.
        for identifier in [arn.clone(), job_id_of(&arn).to_string()] {
            let read = get(&emu, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case}: GetModelInvocationJob must answer 200 for {identifier}, got {} {:?}",
                read.status, read.body
            );
            let body = &read.body;
            assert_eq!(
                text(case, body, "jobArn"),
                arn,
                "case {case}: the read must report the ARN the create minted"
            );
            for (member_name, request_member) in [
                ("jobName", "jobName"),
                ("roleArn", "roleArn"),
                ("modelId", "modelId"),
                ("inputDataConfig", "inputDataConfig"),
                ("outputDataConfig", "outputDataConfig"),
            ] {
                assert_eq!(
                    member(case, body, member_name),
                    member(case, &row.body, request_member),
                    "case {case}: the read must report {member_name} as the create carried it"
                );
            }
            assert_eq!(
                body.get("vpcConfig").cloned(),
                row.body.get("vpcConfig").cloned(),
                "case {case}: the read must report vpcConfig as the create carried it"
            );
            assert_eq!(
                body.get("clientRequestToken").cloned(),
                row.body.get("clientRequestToken").cloned(),
                "case {case}: the read must report the clientRequestToken the create carried"
            );
            assert_eq!(
                text(case, body, "modelInvocationType"),
                row.invocation_type,
                "case {case}: the read must report the invocation type the model resolves to"
            );

            // `jobExpirationTime` is "the time at which the batch inference job times or
            // timed out": the submission plus the hours the request asked for, and nothing
            // at all for a job that asked for no timeout.
            let submitted = instant(case, body, "submitTime");
            match row.timeout_hours {
                Some(hours) => {
                    assert_eq!(
                        member(case, body, "timeoutDurationInHours").as_i64(),
                        Some(hours),
                        "case {case}: the read must report the timeout the create carried"
                    );
                    let expires = instant(case, body, "jobExpirationTime");
                    let expected = submitted + (hours * 60 * 60) as f64;
                    assert!(
                        (expires - expected).abs() < 1.0,
                        "case {case}: jobExpirationTime must be the submission plus {hours}h \
                         ({expected}), got {expires}"
                    );
                }
                None => {
                    assert_eq!(
                        body.get("timeoutDurationInHours"),
                        None,
                        "case {case}: a job with no timeout reports none, got {:?}",
                        body.get("timeoutDurationInHours")
                    );
                    assert_eq!(
                        body.get("jobExpirationTime"),
                        None,
                        "case {case}: a job with no timeout times out at no instant, got {:?}",
                        body.get("jobExpirationTime")
                    );
                }
            }

            // A job that has not reached a terminal status has no end instant, and its last
            // modification is its submission until the clock moves it on.
            assert_eq!(
                body.get("endTime"),
                None,
                "case {case}: a job that is still running has no endTime, got {:?}",
                body.get("endTime")
            );
            assert_eq!(
                instant(case, body, "lastModifiedTime"),
                submitted,
                "case {case}: an unmoved job was last modified when it was submitted"
            );
        }

        // And the listing reports the same job the read found, with the summary members the
        // two shapes share agreeing.
        let rows = listed(case, &list(&emu, &format!("nameContains={}", "a")).await);
        let read = get(&emu, &arn).await;
        let listed_row = rows
            .iter()
            .find(|row| row.get("jobArn").and_then(Value::as_str) == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case}: the listing must carry the submitted job"));
        for shared in ["jobName", "modelId", "roleArn", "inputDataConfig", "outputDataConfig"] {
            assert_eq!(
                member(case, listed_row, shared),
                member(case, &read.body, shared),
                "case {case}: the listing and the read must agree about {shared}"
            );
        }
    }
}

/// RULE (the arithmetic half of `jobExpirationTime`): the instant a job times out is its
/// submission plus the hours the request asked for, and a value the modelled `range 24..168`
/// does not admit is answered by the modelled refusal — never by a 5xx and never by taking
/// the submission down, whatever integer the request carries.
#[tokio::test]
async fn a_timeout_no_instant_can_stand_after_is_refused_rather_than_served() {
    let emu = emulator();

    // The rows span the integers outside the modelled range: both bounds a tick outside it,
    // zero, a negative count, and the extremes of the modelled `integer`, where the hours
    // are more seconds than any instant of this clock stands after.
    let rows: &[(&str, i64)] = &[
        ("a tick below the lower bound", 23),
        ("a tick above the upper bound", 169),
        ("no hours at all", 0),
        ("a negative count of hours", -1),
        ("the largest value the member's type carries", i64::from(i32::MAX)),
        ("the smallest value the member's type carries", i64::from(i32::MIN)),
    ];

    for (case, hours) in rows {
        let mut body = create_body("batch-timeout-range", catalog_model(0));
        body["timeoutDurationInHours"] = json!(hours);
        let reply = create(&emu, body).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case} ({hours} hours): a timeout outside the modelled range must be refused \
             400 ValidationException, got {} {:?} {:?}",
            reply.status,
            reply.error,
            reply.body
        );
        assert!(
            listed_arns(case, &list(&emu, "").await).is_empty(),
            "case {case} ({hours} hours): a refused create must store no job"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. An identifier no stored job answers to is refused.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_stored_job_answers_to_is_refused() {
    let emu = emulator();

    // A job of ANOTHER region, so its id and its ARN both name nothing in the signing one.
    let elsewhere = "eu-central-1";
    let other_region = create_in_region(
        &emu,
        elsewhere,
        create_body("batch-elsewhere", catalog_model(0)),
    )
    .await;
    assert_eq!(
        other_region.status, 200,
        "the create in {elsewhere} must answer 200, got {} {:?}",
        other_region.status, other_region.body
    );
    let other_arn = other_region.text("jobArn");

    // Every row is a value `ModelInvocationJobIdentifier` admits — its ARN spelling or the
    // bare `[a-z0-9]{12}` id — so each reaches the handler rather than the modelled
    // constraint ahead of it.
    let rows = [
        ("a never-minted id", "aaaaaaaaaaaa".to_string()),
        (
            "a well-formed ARN that was never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-invocation-job/bbbbbbbbbbbb"),
        ),
        (
            "the id of another region's job",
            job_id_of(&other_arn).to_string(),
        ),
    ];
    for (case, identifier) in rows {
        let reply = get(&emu, &identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: {identifier} names no job here, so the read must answer 404, got {} \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: the read must answer ResourceNotFoundException, got {:?}",
            reply.error
        );
    }

    // A `model-invocation-job` ARN of ANOTHER region never reaches the lookup: this service
    // reads such an ARN itself and refuses it ahead of every handler
    // (`state::SELF_VALIDATED_RESOURCE_TYPES`), so the answer is its own refusal rather than
    // "no such job".
    let foreign = get(&emu, &other_arn).await;
    assert_eq!(
        foreign.status, 400,
        "an ARN of another region is refused before the lookup, got {} {:?}",
        foreign.status, foreign.body
    );
    assert_eq!(
        foreign.error.as_deref(),
        Some("ValidationException"),
        "an ARN of another region must answer ValidationException, got {:?}",
        foreign.error
    );

    // The job is still there in the region that submitted it.
    let read = get_in_region(&emu, elsewhere, &other_arn).await;
    assert_eq!(
        read.status, 200,
        "the job must still read back in {elsewhere}, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 4. `modelId` is resolved against the published catalog.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_model_is_resolved_against_the_published_catalog() {
    // EVERY row the catalog publishes is accepted, by both spellings its `modelId` admits —
    // so the suite follows the published catalog rather than the one model a recording used.
    //
    // Each row is submitted against a FRESH account, because the catalog publishes more
    // models than an account may hold batch jobs for, and a quota refusal would say nothing
    // about whether the model resolved.
    for (index, model) in CATALOG.iter().enumerate() {
        let emu = emulator();
        for (spelling, model_id) in [
            ("the bare model id", model.model_id.to_string()),
            (
                "the model's ARN",
                foundation_model_arn(REGION, model.model_id),
            ),
        ] {
            let case = format!("{} as {spelling}", model.model_id);
            let body = create_body(&format!("batch-catalog-{index}-{spelling:.3}"), &model_id);
            let reply = create(&emu, body).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: a model the catalog publishes must be accepted, got {} {:?}",
                reply.status, reply.body
            );
        }
    }

    let emu = emulator();
    // And an identifier naming no model of this account is refused as the spelling it IS:
    // an ARN of one of this service's own resource types reports that resource missing,
    // while the bare model-id spelling is no ARN at all and is un-usable input.
    let custom_model = format!(
        "arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/nosuch.model-v1/cccccccccccc"
    );
    let profile = format!(
        "arn:aws:bedrock:{REGION}:{ACCOUNT}:application-inference-profile/dddddddddddd"
    );
    let unknown: Vec<(&str, String, u16, &str, String)> = vec![
        (
            "a bare id no catalog row carries",
            "nosuchmodel".to_string(),
            400,
            "ValidationException",
            INVALID_MODEL_IDENTIFIER.to_string(),
        ),
        (
            "a foundation model ARN no catalog row carries",
            foundation_model_arn(REGION, "nosuch.model-v1"),
            404,
            "ResourceNotFoundException",
            format!(
                "Could not resolve the model from the provided model identifier: {}",
                foundation_model_arn(REGION, "nosuch.model-v1")
            ),
        ),
        (
            "a custom model ARN that was never minted",
            custom_model,
            404,
            "ResourceNotFoundException",
            "Custom model not found.".to_string(),
        ),
        (
            "an inference profile ARN that was never minted",
            profile,
            404,
            "ResourceNotFoundException",
            "Application inference profile not found".to_string(),
        ),
    ];
    for (case, model_id, status, code, sentence) in unknown {
        let reply = create(&emu, create_body("batch-unknown-model", &model_id)).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (status, Some(code)),
            "case {case}: {model_id} names no model, so the create must answer {status} \
             {code}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.get("message").as_str(),
            Some(sentence.as_str()),
            "case {case}: the refusal states the sentence of its class, got {:?}",
            reply.get("message")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. The status walks the documented progression, stage by stage, on the submission's clock.
// ---------------------------------------------------------------------------------------

/// Every status a stored job's lifecycle passes through, so a case can assert that the
/// filter reports the job under the status it holds and under no other.
const LIFECYCLE: &[&str] = &[SUBMITTED, VALIDATING, SCHEDULED, IN_PROGRESS, COMPLETED];

#[tokio::test]
async fn the_status_walks_every_pre_start_stage_before_it_reports_the_job_running() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the settling job",
        create_body("batch-settling", catalog_model(0)),
    )
    .await;

    // The stages a client polling a submitted job reads it in, in order. Each pre-start
    // status the model documents dwells its own span on the submission's clock, so a client
    // polling tighter than those spans reads the job in every one of them, in order, and in
    // none of them before the stage before it.
    let stages: &[(&str, &str, Duration)] = &[
        ("straight after the create", SUBMITTED, Duration::ZERO),
        ("the read that found it validating", VALIDATING, VALIDATES_AFTER),
        (
            "the read that found it scheduled",
            SCHEDULED,
            VALIDATES_AFTER + SCHEDULES_AFTER,
        ),
        ("the read that found it running", IN_PROGRESS, pre_start_span()),
    ];

    let mut walked: Vec<String> = Vec::new();
    let mut first_submit_time: Option<f64> = None;
    let mut previous_modified = f64::NEG_INFINITY;
    let started = ::std::time::Instant::now();
    loop {
        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "the read must answer 200, got {} {:?}",
            read.status, read.body
        );
        let status = read.text("status");
        if walked.last() == Some(&status) {
            // Still the stage the last read reported: a job dwells in each stage for its own
            // span, so a poller reads it there more than once.
            assert!(
                started.elapsed() < POLL_BUDGET,
                "the job must walk {stages:?} inside {POLL_BUDGET:?}, saw {walked:?}"
            );
            tokio::time::sleep(POLL_STEP).await;
            continue;
        }

        // A stage the poll has just moved into: it must be the NEXT one of the progression,
        // never a later one.
        let position = walked.len();
        let (case, expected, earliest) = *stages.get(position).unwrap_or_else(|| {
            panic!("a job walking to its start reports {stages:?} and nothing further, saw {walked:?} then {status:?}")
        });
        assert_eq!(
            status, expected,
            "case {case}: stage {position} of the progression must be {expected}, got \
             {status:?}, having walked {walked:?}"
        );
        walked.push(status.clone());

        // A settle leaves the submission instant alone, and stamps the move at the instant
        // the CLOCK owed it rather than at the instant of the read — never earlier than the
        // spans before it had elapsed, and never before the stage it left.
        let submitted = instant(case, &read.body, "submitTime");
        let first = *first_submit_time.get_or_insert(submitted);
        assert_eq!(
            submitted, first,
            "case {case}: a settle must leave the submission instant alone, got {submitted} \
             against {first}"
        );
        let modified = instant(case, &read.body, "lastModifiedTime");
        let owed = submitted + earliest.as_secs_f64();
        assert!(
            modified >= owed - INSTANT_TOLERANCE,
            "case {case}: the move must be stamped no earlier than the instant it was owed \
             ({owed}), got {modified}"
        );
        assert!(
            modified >= previous_modified,
            "case {case}: the stages are stamped in the order they were walked, got \
             {modified} after {previous_modified}"
        );
        previous_modified = modified;
        assert_eq!(
            read.body.get("endTime"),
            None,
            "case {case}: a job that has not reached a terminal status has no endTime, got {:?}",
            read.body.get("endTime")
        );

        // The listing agrees, and `statusEquals` filters on the status the job holds NOW.
        let rows = listed(case, &list(&emu, "").await);
        assert_eq!(
            text(case, &rows[0], "status"),
            expected,
            "case {case}: the listing must report the same status the read does"
        );
        for status in LIFECYCLE {
            let reply = list(&emu, &format!("statusEquals={status}")).await;
            let expected_arns = if *status == expected {
                vec![arn.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                listed_arns(case, &reply),
                expected_arns,
                "case {case}: statusEquals={status} must report {} while the job is {expected}",
                if *status == expected {
                    "the job"
                } else {
                    "nothing"
                }
            );
        }

        if expected == IN_PROGRESS {
            break;
        }
        tokio::time::sleep(POLL_STEP).await;
    }

    let walked_stages: Vec<&str> = walked.iter().map(String::as_str).collect();
    let expected_stages: Vec<&str> = stages.iter().map(|(_, status, _)| *status).collect();
    assert_eq!(
        walked_stages, expected_stages,
        "a client polling a submitted job must read it in every stage of the progression, in \
         order"
    );
}

#[tokio::test]
async fn a_job_reports_completed_only_once_its_whole_run_span_has_elapsed() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the completing job",
        create_body("batch-completing", catalog_model(0)),
    )
    .await;
    let lifespan = lifespan();
    let tick = Duration::from_millis(50);

    // The job is polled to the status that says it has begun, which is how a client reaches
    // it; the run span then stands on the submission's own timeline rather than on this read.
    let case = "the read that reported the start";
    let (running, _) = poll_until(&emu, case, &arn, IN_PROGRESS).await;
    assert_eq!(
        running.body.get("endTime"),
        None,
        "case {case}: a running job has no endTime, got {:?}",
        running.body.get("endTime")
    );
    let began = instant(case, &running.body, "lastModifiedTime")
        - instant(case, &running.body, "submitTime");
    let pre_start = pre_start_span().as_secs_f64();
    assert!(
        began >= pre_start - INSTANT_TOLERANCE,
        "case {case}: the begin must be stamped no earlier than the three pre-start spans \
         after the submission ({pre_start} s), got {began} s"
    );

    // A tick before the completion is owed the job is still RUNNING. Ageing the stored record
    // brings the same elapsed time a wall clock would, which is how the whole run span is
    // asserted without sleeping through it — and the instant the completion is owed at is the
    // job's OWN, read off the record, so it is the submission's timeline that is aged towards
    // rather than any instant the poll above established.
    age_job_to_remaining(&emu, &arn, tick);
    let case = "a tick before the completion is owed";
    let still_running = get(&emu, &arn).await;
    assert_eq!(
        text(case, &still_running.body, "status"),
        IN_PROGRESS,
        "case {case}: the job must still be running, got {:?}",
        still_running.body.get("status")
    );
    assert_eq!(
        still_running.body.get("endTime"),
        None,
        "case {case}: a running job has no endTime, got {:?}",
        still_running.body.get("endTime")
    );

    // Once it is up the job is finished, stamped at the instant the completion was owed —
    // which stands the whole lifespan after the submission, the three pre-start spans and
    // then the run span.
    age_job(&emu, &arn, tick.saturating_mul(2));
    let done = get(&emu, &arn).await;
    let case = "once the run span is up";
    assert_eq!(
        text(case, &done.body, "status"),
        COMPLETED,
        "case {case}: the job must be completed, got {:?}",
        done.body.get("status")
    );
    let submitted = instant(case, &done.body, "submitTime");
    let ended = instant(case, &done.body, "endTime");
    let owed = submitted + lifespan.as_secs_f64();
    assert!(
        ended >= owed - INSTANT_TOLERANCE,
        "case {case}: the completion must be stamped no earlier than the instant it was owed \
         ({owed}), got {ended}"
    );
    assert_eq!(
        instant(case, &done.body, "lastModifiedTime"),
        ended,
        "case {case}: the completion is the job's last modification"
    );

    // Reading again reports the SAME instants: the settle is owed by the clock, not by the
    // read, so a second read moves nothing.
    let again = get(&emu, &arn).await;
    for stamped in ["submitTime", "lastModifiedTime", "endTime"] {
        assert_eq!(
            instant("the second read", &again.body, stamped),
            instant("the first read", &done.body, stamped),
            "a second read must report the same {stamped}"
        );
    }

    // And `statusEquals` filters on the status the job holds now.
    for status in LIFECYCLE {
        let reply = list(&emu, &format!("statusEquals={status}")).await;
        let expected_arns = if *status == COMPLETED {
            vec![arn.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_arns("the completed job", &reply),
            expected_arns,
            "statusEquals={status} must report {} once the job completed",
            if *status == COMPLETED {
                "the job"
            } else {
                "nothing"
            }
        );
    }
}

#[tokio::test]
async fn each_stage_of_the_progression_dwells_its_own_span_before_the_next_is_owed() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the clocked job",
        create_body("batch-clocked", catalog_model(0)),
    )
    .await;
    let pristine = stored_job(&emu, &arn);
    let submit_time = pristine.submit_time;

    // The submission arms the earliest instant of every transition, each a span after the one
    // before it, so no later call arms a move it can then spend at the same instant.
    let clock = JobClock::armed_at(submit_time);
    for (case, armed, expected) in [
        ("validating", pristine.validating_due_at, clock.validating_at),
        ("scheduled", pristine.scheduled_due_at, clock.scheduled_at),
        (
            "in progress",
            pristine.in_progress_due_at,
            clock.in_progress_at,
        ),
        ("completed", pristine.completed_due_at, clock.completed_at),
    ] {
        assert_eq!(
            armed,
            Some(expected),
            "case {case}: the submission must arm the move at the instant its own span falls \
             due, got {armed:?}"
        );
    }

    let tick = Duration::from_millis(1);
    // How late the call that materialises a move is: a caller reads the job some time after
    // the move fell due, and what it reads is the instant the move was owed rather than the
    // instant of its own read. It is SHORTER than every span of the progression, so a read
    // this late finds one further move due and not two.
    let lateness = Duration::from_millis(250);

    // One row per transition of the progression: the status the job holds, the span it dwells
    // in that status, and the status that span carries it to.
    let stages: &[(&str, &str, Duration, &str)] = &[
        ("the submission", SUBMITTED, VALIDATES_AFTER, VALIDATING),
        ("the validation", VALIDATING, SCHEDULES_AFTER, SCHEDULED),
        ("the schedule", SCHEDULED, BEGINS_AFTER, IN_PROGRESS),
        ("the start", IN_PROGRESS, COMPLETE_AFTER, COMPLETED),
    ];

    let mut record = pristine.clone();
    // The instant the job entered the status it holds: the submission, and then the instant
    // the clock OWED each move at — never the instant of the call that materialised it,
    // because every span of the progression is counted from the submission's own timeline.
    let mut entered = submit_time;
    for (case, held, span, next) in stages {
        // A tick before the span is up nothing is owed, so a settle changes nothing.
        let before = record.clone();
        record.settle(entered + span.saturating_sub(tick));
        assert_eq!(
            record.status, *held,
            "case {case}: a tick before its span is up the job is still {held}, got {}",
            record.status
        );
        assert_eq!(
            (record.last_modified_at, record.ended_at),
            (before.last_modified_at, before.ended_at),
            "case {case}: a settle that finds nothing due stamps nothing"
        );

        // At the instant the span is up the next stage is owed, and it is stamped at that
        // instant rather than at the instant of the call that materialised it.
        let due = entered + *span;
        let observed_at = due + lateness;
        record.settle(observed_at);
        assert_eq!(
            record.status, *next,
            "case {case}: once its span is up the job must report {next}, got {}",
            record.status
        );
        assert_eq!(
            record.last_modified_at, due,
            "case {case}: the move must be stamped at the instant it was owed, got {:?} \
             against {due:?}",
            record.last_modified_at
        );
        assert_eq!(
            record.ended_at,
            (*next == COMPLETED).then_some(due),
            "case {case}: only a job that reached a terminal status reports an end, got {:?}",
            record.ended_at
        );

        // Settling again moves nothing: what the job reports is what the clock reached, not
        // what a read did to it.
        let mut again = record.clone();
        again.settle(observed_at);
        assert_eq!(
            (again.status, again.last_modified_at, again.ended_at),
            (record.status, record.last_modified_at, record.ended_at),
            "case {case}: a second settle at the same instant must report the same job"
        );

        // The next stage stands its own span after the instant THIS move was owed, so a
        // settle that ran late does not push the rest of the progression back.
        entered = due;
    }

    // And nothing is owed past the end of the progression.
    let case = "an hour past the completion";
    let finished = record.clone();
    record.settle(entered + Duration::from_secs(60 * 60));
    assert_eq!(
        (record.status, record.last_modified_at, record.ended_at),
        (
            finished.status,
            finished.last_modified_at,
            finished.ended_at
        ),
        "case {case}: a finished job is owed nothing further"
    );
}

/// The whole progression an unstopped job walks, in the order the `status` member's
/// documentation lists it: the statuses the clock carries a job through, each of them a
/// status a caller can read it in.
const PROGRESSION: &[&str] = &[SUBMITTED, VALIDATING, SCHEDULED, IN_PROGRESS, COMPLETED];

/// The most reads one row of the polling table makes before it concludes the progression is
/// stuck: a bound on the loop, far above the reads the tightest interval of the table needs.
const MAX_POLLS: usize = 1024;

#[tokio::test]
async fn a_client_polling_at_any_interval_reaches_the_terminal_status_on_the_elapsed_time() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the polled job",
        create_body("batch-polled", catalog_model(0)),
    )
    .await;
    let pristine = stored_job(&emu, &arn);
    let submit_time = pristine.submit_time;

    // The intervals a client polls a job at, from a tight loop to the hour. The job's own
    // progression runs on the time elapsed since its submission, so whatever the interval the
    // statuses a poller reads are a subsequence of the progression, in order, and the job is
    // finished by the first read taken once its whole lifespan has elapsed — a tight loop
    // merely observes more of the intermediate statuses on the way. The rows span four orders
    // of magnitude of interval, from far shorter than every span of the lifecycle to far
    // longer than all of them together.
    for interval in [
        Duration::from_millis(50),
        Duration::from_millis(250),
        COMPLETE_AFTER / 2,
        COMPLETE_AFTER,
        Duration::from_secs(10),
        Duration::from_secs(60),
        Duration::from_secs(10 * 60),
        Duration::from_secs(60 * 60),
    ] {
        let mut record = pristine.clone();
        let mut seen: Vec<&'static str> = Vec::new();
        let mut at = submit_time;
        let mut polls = 0_usize;
        let mut finished_at: Option<Duration> = None;
        for _ in 0..MAX_POLLS {
            record.settle(at);
            polls += 1;
            if seen.last() != Some(&record.status) {
                seen.push(record.status);
            }
            if record.status == COMPLETED {
                finished_at = Some(
                    at.duration_since(submit_time)
                        .expect("a poll stands after the submission"),
                );
                break;
            }
            at += interval;
        }

        let case = format!("a poll every {interval:?}");
        let finished_at = finished_at.unwrap_or_else(|| {
            panic!("case {case}: the job must finish however coarse the polling, saw {seen:?}")
        });

        // Every status the poller read is one of the progression, and they came in the
        // progression's own order with none repeated: a job never goes backwards, and no
        // poller is answered a status the progression does not carry.
        let mut expected = PROGRESSION.iter();
        for status in &seen {
            assert!(
                expected.any(|stage| stage == status),
                "case {case}: the statuses a poller reads must be a subsequence of \
                 {PROGRESSION:?}, in order, saw {seen:?}"
            );
        }
        assert_eq!(
            seen.first().copied(),
            Some(SUBMITTED),
            "case {case}: the read of a job just submitted must answer the queued status, saw \
             {seen:?}"
        );
        assert_eq!(
            seen.last().copied(),
            Some(COMPLETED),
            "case {case}: a poller must read the job in its terminal status, saw {seen:?}"
        );

        // The ELAPSED time decides when the job is finished, not the number of polls: the
        // first poll taken a whole lifespan after the submission reads it terminal, so a
        // poller needs no more reads than its own interval divides that lifespan into.
        let lifespan = lifespan();
        assert!(
            finished_at >= lifespan,
            "case {case}: the job must not finish before its whole lifespan ({lifespan:?}) has \
             elapsed, finished after {finished_at:?}"
        );
        assert!(
            finished_at < lifespan + interval,
            "case {case}: the job must be finished for the first poll taken once its lifespan \
             has elapsed, finished only after {finished_at:?}"
        );
        let polls_needed = lifespan.as_nanos() / interval.as_nanos() + 2;
        assert!(
            u128::try_from(polls).expect("a small poll count") <= polls_needed,
            "case {case}: the progression must be carried by elapsed time rather than by \
             polls, took {polls} reads where the elapsed time needs at most {polls_needed}, \
             saw {seen:?}"
        );

        // A tight loop is the one that observes every intermediate stage; a coarse one is
        // simply answered the stage the elapsed time owes it.
        if interval <= Duration::from_millis(50) {
            assert_eq!(
                seen, PROGRESSION,
                "case {case}: a loop tighter than every span must read the job in every status \
                 of the progression, saw {seen:?}"
            );
        }
        let settled = seen
            .iter()
            .copied()
            .find(|status| !PRE_START.contains(status))
            .unwrap_or_else(|| {
                panic!("case {case}: the job must leave the pre-start statuses, saw {seen:?}")
            });
        // WHICH status that is follows from this row's own interval, because the polls stand
        // at the submission plus a whole number of intervals: the first poll at or after the
        // instant the job begins reads it running, unless that same poll already stands past
        // the instant the completion is owed at — a poller whose interval carried it over the
        // whole run in one step.
        let first_poll_past_the_start = {
            let start = pre_start_span().as_nanos();
            let step = interval.as_nanos();
            start.div_ceil(step) * step
        };
        let expected_settled = if first_poll_past_the_start < lifespan.as_nanos() {
            IN_PROGRESS
        } else {
            COMPLETED
        };
        assert_eq!(
            settled, expected_settled,
            "case {case}: the first poll outside the pre-start set stands {:?} after the \
             submission, so it must read {expected_settled}, saw {seen:?}",
            Duration::from_nanos(
                u64::try_from(first_poll_past_the_start).expect("a span of a few seconds")
            )
        );
    }
}

// ---------------------------------------------------------------------------------------
// 6. A replayed clientRequestToken submits nothing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_submits_nothing() {
    let emu = emulator();

    let mut first = create_body("batch-token-first", catalog_model(0));
    first["clientRequestToken"] = json!("replayedToken1");
    let arn = submit(&emu, "the first submission", first.clone()).await;

    // The replay carries ANOTHER name and another model, and still answers with the job the
    // first call minted — and stores nothing of its own.
    let mut replay = create_body("batch-token-replayed", catalog_model(1));
    replay["clientRequestToken"] = json!("replayedToken1");
    let replayed = submit(&emu, "the replay", replay).await;
    assert_eq!(
        replayed, arn,
        "a replayed clientRequestToken must answer with the job the first call minted"
    );
    assert_eq!(
        listed_arns("after the replay", &list(&emu, "").await),
        vec![arn.clone()],
        "a replay must store no second job"
    );
    let read = get(&emu, &arn).await;
    assert_eq!(
        text("after the replay", &read.body, "jobName"),
        "batch-token-first",
        "the stored job is the one the FIRST call submitted"
    );

    // A fresh token submits another job.
    let mut fresh = create_body("batch-token-fresh", catalog_model(0));
    fresh["clientRequestToken"] = json!("freshToken2");
    let second = submit(&emu, "a fresh token", fresh).await;
    assert_ne!(
        second, arn,
        "a fresh clientRequestToken must submit another job"
    );
    assert_eq!(
        listed_arns("after the fresh token", &list(&emu, "").await).len(),
        2,
        "the account now holds both jobs"
    );
}

// ---------------------------------------------------------------------------------------
// 7. The listing orders, filters and pages the stored jobs.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_orders_filters_and_pages_the_stored_jobs() {
    let emu = emulator();

    assert!(
        listed_arns("an empty account", &list(&emu, "").await).is_empty(),
        "an account that has submitted nothing lists nothing"
    );

    // Two groups, submitted one after the other so the submission order is known, differing
    // in the model they invoke and in their names.
    let first_group = ["alpha-one", "alpha-two", "alpha-three"];
    let second_group = ["beta-one", "beta-two"];
    let mut arns: Vec<String> = Vec::new();
    for name in first_group {
        arns.push(submit(&emu, name, create_body(name, catalog_model(0))).await);
    }
    // A clear gap, so a bound between the two groups is unambiguous.
    tokio::time::sleep(Duration::from_millis(30)).await;
    for name in second_group {
        arns.push(submit(&emu, name, create_body(name, catalog_model(1))).await);
    }

    // Order: `sortBy` admits only `CreationTime`, so the listing is in submission order,
    // ascending by default and reversed by `sortOrder=Descending`.
    assert_eq!(
        listed_arns("the default listing", &list(&emu, "").await),
        arns,
        "the default listing must report every stored job in submission order"
    );
    assert_eq!(
        listed_arns(
            "sortBy=CreationTime&sortOrder=Ascending",
            &list(&emu, "sortBy=CreationTime&sortOrder=Ascending").await
        ),
        arns,
        "sortBy=CreationTime ascending is the order an absent sortBy already takes"
    );
    let mut reversed = arns.clone();
    reversed.reverse();
    assert_eq!(
        listed_arns(
            "sortOrder=Descending",
            &list(&emu, "sortOrder=Descending").await
        ),
        reversed,
        "sortOrder=Descending must report the same listing newest first"
    );

    // `nameContains` reports the jobs whose name carries the fragment, and nothing else.
    assert_eq!(
        listed_arns("nameContains=alpha", &list(&emu, "nameContains=alpha").await),
        arns[..first_group.len()].to_vec(),
        "nameContains must report the jobs whose name carries the fragment"
    );
    assert_eq!(
        listed_arns("nameContains=beta", &list(&emu, "nameContains=beta").await),
        arns[first_group.len()..].to_vec(),
        "nameContains must report the other group for the other fragment"
    );
    assert!(
        listed_arns(
            "nameContains=gamma",
            &list(&emu, "nameContains=gamma").await
        )
        .is_empty(),
        "a fragment no name carries reports nothing"
    );

    // The submit-time bounds are STRICT, as the members word them: a job submitted AFTER
    // the instant, and a job submitted BEFORE it.
    let rows = listed("the default listing", &list(&emu, "").await);
    let last_of_first = instant("the group bound", &rows[first_group.len() - 1], "submitTime");
    let first_of_second = instant("the group bound", &rows[first_group.len()], "submitTime");
    let between = (last_of_first + first_of_second) / 2.0;
    assert_eq!(
        listed_arns(
            "submitTimeAfter between the groups",
            &list(
                &emu,
                &format!("submitTimeAfter={}", date_time_query(between))
            )
            .await
        ),
        arns[first_group.len()..].to_vec(),
        "submitTimeAfter must report only the jobs submitted after the instant"
    );
    assert_eq!(
        listed_arns(
            "submitTimeBefore between the groups",
            &list(
                &emu,
                &format!("submitTimeBefore={}", date_time_query(between))
            )
            .await
        ),
        arns[..first_group.len()].to_vec(),
        "submitTimeBefore must report only the jobs submitted before the instant"
    );
    assert!(
        listed_arns(
            "a window that holds nothing",
            &list(
                &emu,
                &format!(
                    "submitTimeAfter={}&submitTimeBefore={}",
                    date_time_query(first_of_second + 60.0),
                    date_time_query(first_of_second + 120.0)
                )
            )
            .await
        )
        .is_empty(),
        "a window no job falls in reports nothing"
    );

    // Pages: `maxResults` bounds the page and `nextToken` resumes it, so the pages partition
    // the listing with neither a gap nor a repeat, at every page size.
    for page_size in 1..=arns.len() + 1 {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let page = list(&emu, &query).await;
            let page_arns = listed_arns(&case, &page);
            assert!(
                page_arns.len() <= page_size,
                "case {case}: a page must hold at most {page_size} rows, got {}",
                page_arns.len()
            );
            seen.extend(page_arns);
            match page.get("nextToken").as_str().map(str::to_string) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, arns,
            "case {case}: the pages must partition the listing in order"
        );
    }

    // A `nextToken` this service never issued is refused rather than read as some page.
    let bogus = list(&emu, "nextToken=notatokenthisserviceissued").await;
    assert_eq!(
        bogus.status, 400,
        "a nextToken from nowhere must be refused, got {} {:?}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error.as_deref(),
        Some("ValidationException"),
        "a nextToken from nowhere must answer ValidationException, got {:?}",
        bogus.error
    );

    // And a listing is per account-region: the jobs of another region are not these.
    assert!(
        listed_arns(
            "another region's listing",
            &list_in_region(&emu, "eu-central-1", "").await
        )
        .is_empty(),
        "a region that has submitted nothing lists nothing"
    );
}

// ---------------------------------------------------------------------------------------
// The account quota bounds the store.
// ---------------------------------------------------------------------------------------

/// The quota the create enforces, which is also what bounds this part of the store: the
/// submission past it is refused and stores nothing, so no sequence of submissions can grow
/// the store past the cap.
#[tokio::test]
async fn the_account_quota_bounds_the_submissions() {
    let emu = emulator();
    let quota: usize =
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT;

    for index in 0..quota {
        let case = format!("submission {index} of {quota}");
        submit(
            &emu,
            &case,
            create_body(&format!("batch-quota-{index}"), catalog_model(0)),
        )
        .await;
    }

    let refused = create(&emu, create_body("batch-over-quota", catalog_model(0))).await;
    assert_eq!(
        refused.status, 400,
        "the submission past the quota of {quota} must be refused, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ServiceQuotaExceededException"),
        "the submission past the quota must answer ServiceQuotaExceededException, got {:?}",
        refused.error
    );
    assert_eq!(
        listed_arns("after the refused submission", &list(&emu, "").await).len(),
        quota,
        "a refused submission must store nothing"
    );
}

// ---------------------------------------------------------------------------------------
// The wire form of this surface's instants.
// ---------------------------------------------------------------------------------------

/// Every `Timestamp` member of the batch inference surface carries
/// `@timestampFormat("date-time")`
/// (`smithy_query(query="operation",operation="GetModelInvocationJob")` /
/// `query="shape",shape="ModelInvocationJobSummary"`), so each member SCHEMA must report
/// that format — which is what makes the codec write the RFC 3339 string the suites above
/// read rather than the epoch number the protocol default would write.
#[test]
fn every_date_time_member_schema_carries_the_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::{
        GetModelInvocationJobOutput, ModelInvocationJobSummary,
    };

    let date_time = vec![
        (
            "GetModelInvocationJobResponse$submitTime",
            &GetModelInvocationJobOutput::M_SUBMIT_TIME,
        ),
        (
            "GetModelInvocationJobResponse$lastModifiedTime",
            &GetModelInvocationJobOutput::M_LAST_MODIFIED_TIME,
        ),
        (
            "GetModelInvocationJobResponse$endTime",
            &GetModelInvocationJobOutput::M_END_TIME,
        ),
        (
            "GetModelInvocationJobResponse$jobExpirationTime",
            &GetModelInvocationJobOutput::M_JOB_EXPIRATION_TIME,
        ),
        (
            "ModelInvocationJobSummary$submitTime",
            &ModelInvocationJobSummary::M_SUBMIT_TIME,
        ),
        (
            "ModelInvocationJobSummary$lastModifiedTime",
            &ModelInvocationJobSummary::M_LAST_MODIFIED_TIME,
        ),
        (
            "ModelInvocationJobSummary$endTime",
            &ModelInvocationJobSummary::M_END_TIME,
        ),
        (
            "ModelInvocationJobSummary$jobExpirationTime",
            &ModelInvocationJobSummary::M_JOB_EXPIRATION_TIME,
        ),
    ];
    for (case, schema) in date_time {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // The control rows: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema.
    let without = vec![
        (
            "GetModelInvocationJobResponse$status",
            &GetModelInvocationJobOutput::M_STATUS,
        ),
        (
            "ModelInvocationJobSummary$jobArn",
            &ModelInvocationJobSummary::M_JOB_ARN,
        ),
    ];
    for (case, schema) in without {
        assert!(
            schema.timestamp_format().is_none(),
            "case {case}: a member the model gives no @timestampFormat reports none"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The store the three operations share holds one copy of a job.
// ---------------------------------------------------------------------------------------

#[test]
fn the_state_holds_one_batch_inference_job_store() {
    // A fresh store holds no job and has minted no id, so everything a listing reports was
    // put there by a submission.
    let state = State::default();
    assert!(
        state.model_invocation_jobs.is_empty(),
        "a fresh store holds no batch inference job, got {}",
        state.model_invocation_jobs.len()
    );
    assert!(
        state.model_invocation_job_create_tokens.is_empty(),
        "a fresh store holds no idempotency token, got {}",
        state.model_invocation_job_create_tokens.len()
    );
    assert_eq!(
        state.minted_model_invocation_jobs, 0,
        "a fresh store has minted no job id, got {}",
        state.minted_model_invocation_jobs
    );
}

// ---------------------------------------------------------------------------------------
// 8. StopModelInvocationJob winds down a job that has not finished.
// ---------------------------------------------------------------------------------------

/// Whether `body` carries no members: `StopModelInvocationJobResponse` is modelled with
/// none (`smithy_query(query="operation",operation="StopModelInvocationJob")`), so the
/// answer is either an empty document or an empty object.
fn carries_no_members(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty)
}

/// One row of the stop table: the unfinished status the job is polled to before the stop is
/// asked for, and which `ModelInvocationJobIdentifier` spelling it is addressed by.
struct StopRow {
    case: &'static str,
    /// The status the job holds when the stop arrives: the job is POLLED to it, because a
    /// job dwells in each stage for its own span on the submission's clock.
    status_at_stop: &'static str,
    /// Whether the stop addresses the job by its ARN or by the bare minted id.
    by_arn: bool,
}

/// The rows differ along what the rule generalises over: WHICH unfinished status the job
/// holds when the stop arrives — this emulator's lifecycle reaches every one of the four
/// statuses a job that has not finished carries
/// (`state::MODEL_INVOCATION_JOB_STOPPABLE_STATUSES`, read off the `status` doc of
/// `smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`) — and
/// which of the two spellings a `ModelInvocationJobIdentifier` admits addresses the job.
fn stop_rows() -> Vec<StopRow> {
    vec![
        StopRow {
            case: "stopped while queued, by its ARN",
            status_at_stop: SUBMITTED,
            by_arn: true,
        },
        StopRow {
            case: "stopped while queued, by its minted id",
            status_at_stop: SUBMITTED,
            by_arn: false,
        },
        StopRow {
            case: "stopped while being validated, by its ARN",
            status_at_stop: VALIDATING,
            by_arn: true,
        },
        StopRow {
            case: "stopped while queued for its turn, by its minted id",
            status_at_stop: SCHEDULED,
            by_arn: false,
        },
        StopRow {
            case: "stopped once it had begun, by its ARN",
            status_at_stop: IN_PROGRESS,
            by_arn: true,
        },
        StopRow {
            case: "stopped once it had begun, by its minted id",
            status_at_stop: IN_PROGRESS,
            by_arn: false,
        },
    ]
}

#[tokio::test]
async fn a_stop_winds_down_a_job_that_has_not_finished() {
    let emu = emulator();

    for (index, row) in stop_rows().into_iter().enumerate() {
        let case = row.case;
        let body = create_body(&format!("batch-stop-{index}"), catalog_model(index % CATALOG.len()));
        let arn = submit(&emu, case, body).await;

        // The row's premise: the job is polled until it holds the unfinished status this row
        // stops it from — which is how a client reaches that status — and the stop is asked
        // for on the job that read reported.
        let (before, walked) = poll_until(&emu, case, &arn, row.status_at_stop).await;
        assert_eq!(
            text(case, &before.body, "status"),
            row.status_at_stop,
            "case {case}: the job must hold the status this row stops it from, got {:?} after \
             walking {walked:?}",
            before.body.get("status")
        );
        let submitted = instant(case, &before.body, "submitTime");

        let identifier = if row.by_arn {
            arn.clone()
        } else {
            job_id_of(&arn).to_string()
        };
        let asked_at = wall_clock_seconds();
        let stopped = stop(&emu, &identifier).await;
        assert_eq!(
            stopped.status, 200,
            "case {case}: StopModelInvocationJob is bound to @http(code: 200), got {} {:?}",
            stopped.status, stopped.body
        );
        assert_eq!(
            stopped.error, None,
            "case {case}: a stop of an unfinished job is not a failure, got {:?} {:?}",
            stopped.error, stopped.body
        );
        assert!(
            carries_no_members(&stopped.body),
            "case {case}: StopModelInvocationJobResponse is modelled with no members, got {:?}",
            stopped.body
        );

        // The stop is what moves the job, and the move is PERSISTED: stopping is
        // asynchronous, so the caller observes the transient status first.
        let stopping = get(&emu, &arn).await;
        assert_eq!(
            text(case, &stopping.body, "status"),
            STOPPING,
            "case {case}: a stop moves an unfinished job to {STOPPING}, got {:?}",
            stopping.body.get("status")
        );
        assert_eq!(
            stopping.body.get("endTime"),
            None,
            "case {case}: a job that has not reached a terminal status has no endTime, got {:?}",
            stopping.body.get("endTime")
        );
        let asked = instant(case, &stopping.body, "lastModifiedTime");
        assert!(
            asked >= asked_at - 0.001,
            "case {case}: the stop is the job's last change, so it is stamped no earlier than \
             the call at {asked_at}, got {asked}"
        );
        assert_eq!(
            instant(case, &stopping.body, "submitTime"),
            submitted,
            "case {case}: a stop leaves the submission instant alone"
        );
        assert!(
            listed_arns(case, &list(&emu, &format!("statusEquals={STOPPING}")).await)
                .contains(&arn),
            "case {case}: the listing must report the status the read does, so {STOPPING} must \
             carry {arn}"
        );

        // The terminal status is owed by the CLOCK, at the instant it was owed.
        tokio::time::sleep(STOPPED_AFTER + MARGIN).await;
        let done = get(&emu, &arn).await;
        assert_eq!(
            text(case, &done.body, "status"),
            STOPPED,
            "case {case}: the stop settles on {STOPPED} once its span has passed, got {:?}",
            done.body.get("status")
        );
        let ended = instant(case, &done.body, "endTime");
        assert!(
            ended >= asked + STOPPED_AFTER.as_secs_f64() - 0.001,
            "case {case}: the stop must be stamped at the instant it was owed ({}), got {ended} \
             against a stop at {asked}",
            STOPPED_AFTER.as_secs_f64()
        );
        assert_eq!(
            instant(case, &done.body, "lastModifiedTime"),
            ended,
            "case {case}: reaching the terminal status is the job's last change"
        );
        let again = get(&emu, &arn).await;
        for stamped in ["submitTime", "lastModifiedTime", "endTime"] {
            assert_eq!(
                instant(case, &again.body, stamped),
                instant(case, &done.body, stamped),
                "case {case}: a second read must report the same {stamped}"
            );
        }

        // A stopped job is owed NONE of the transitions the submission armed: the record
        // carries no due instant any more, and settling it an hour past the instant an
        // unstopped job would have completed leaves it the job the stop ended, at the instant
        // the stop ended it. Settled on the record because the run span an unstopped job
        // takes is longer than a case wants to sleep through.
        let mut stopped_record = stored_job(&emu, &arn);
        for (field, due) in [
            ("validating", stopped_record.validating_due_at),
            ("scheduled", stopped_record.scheduled_due_at),
            ("in progress", stopped_record.in_progress_due_at),
            ("completed", stopped_record.completed_due_at),
            ("stopped", stopped_record.stopped_due_at),
        ] {
            assert_eq!(
                due, None,
                "case {case}: a job the stop has carried to its end owes no {field} \
                 transition, got {due:?}"
            );
        }
        let stop_ended_at = stopped_record.ended_at;
        let far_future = stopped_record.submit_time + lifespan() + Duration::from_secs(60 * 60);
        stopped_record.settle(far_future);
        assert_eq!(
            stopped_record.status, STOPPED,
            "case {case}: a stopped job is owed no completion, got {}",
            stopped_record.status
        );
        assert_eq!(
            stopped_record.ended_at, stop_ended_at,
            "case {case}: a stopped job keeps the end the stop gave it"
        );
        assert!(
            !listed_arns(case, &list(&emu, &format!("statusEquals={COMPLETED}")).await)
                .contains(&arn),
            "case {case}: a stopped job never reports {COMPLETED}"
        );
        assert!(
            listed_arns(case, &list(&emu, &format!("statusEquals={STOPPED}")).await).contains(&arn),
            "case {case}: {STOPPED} must carry {arn}"
        );
    }
}

/// How a row of the table below drives its job to a status a stop can no longer affect.
enum Reach {
    /// Ask for a stop, so the job is winding down.
    Stopping,
    /// Ask for a stop and let the clock carry it to its end.
    Stopped,
    /// Let the clock carry the job to the end of its own run.
    Completed,
}

#[tokio::test]
async fn a_stop_of_a_job_that_cannot_be_stopped_changes_nothing() {
    let emu = emulator();

    // The rows span the statuses a stop can no longer affect: the two a stop itself
    // produces, and the terminal status a job reaches on its own.
    let rows = [
        (
            "a job a stop has already taken over",
            Reach::Stopping,
            STOPPING,
        ),
        (
            "a job a stop has already carried to its end",
            Reach::Stopped,
            STOPPED,
        ),
        (
            "a job that ran to the end of its own input",
            Reach::Completed,
            COMPLETED,
        ),
    ];

    for (index, (case, reach, expected)) in rows.into_iter().enumerate() {
        let body = create_body(
            &format!("batch-no-op-{index}"),
            catalog_model(index % CATALOG.len()),
        );
        let arn = submit(&emu, case, body).await;
        match reach {
            Reach::Stopping => {
                stop(&emu, &arn).await;
            }
            Reach::Stopped => {
                stop(&emu, &arn).await;
                tokio::time::sleep(STOPPED_AFTER + MARGIN).await;
            }
            Reach::Completed => {
                // A client reaches the terminal status by polling the job until the elapsed
                // time since its submission has carried it there.
                poll_until(&emu, case, &arn, COMPLETED).await;
            }
        }

        let before = get(&emu, &arn).await;
        assert_eq!(
            text(case, &before.body, "status"),
            expected,
            "case {case}: the job must hold the status this row asks a further stop of, got {:?}",
            before.body.get("status")
        );

        let further = stop(&emu, &arn).await;
        assert_eq!(
            further.status, 200,
            "case {case}: a stop of a job with nothing left to wind down is answered as the \
             first stop was, got {} {:?}",
            further.status, further.body
        );
        assert_eq!(
            further.error, None,
            "case {case}: a further stop is not a failure, got {:?} {:?}",
            further.error, further.body
        );
        assert!(
            carries_no_members(&further.body),
            "case {case}: StopModelInvocationJobResponse is modelled with no members, got {:?}",
            further.body
        );

        let after = get(&emu, &arn).await;
        assert_eq!(
            after.body, before.body,
            "case {case}: a stop with nothing left to wind down must change nothing"
        );
    }
}

#[tokio::test]
async fn a_stop_of_an_identifier_no_stored_job_answers_to_is_refused() {
    let emu = emulator();

    // A job of ANOTHER region, so its id names nothing in the signing one.
    let elsewhere = "eu-central-1";
    let other_region = create_in_region(
        &emu,
        elsewhere,
        create_body("batch-stop-elsewhere", catalog_model(0)),
    )
    .await;
    assert_eq!(
        other_region.status, 200,
        "the create in {elsewhere} must answer 200, got {} {:?}",
        other_region.status, other_region.body
    );
    let other_arn = other_region.text("jobArn");

    // Every row is a value `ModelInvocationJobIdentifier` admits — its ARN spelling or the
    // bare `[a-z0-9]{12}` id — so each reaches the handler rather than the modelled
    // constraint ahead of it.
    let rows = [
        ("a never-minted id", "cccccccccccc".to_string()),
        (
            "a well-formed ARN that was never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-invocation-job/dddddddddddd"),
        ),
        (
            "the id of another region's job",
            job_id_of(&other_arn).to_string(),
        ),
    ];
    for (case, identifier) in rows {
        let reply = stop(&emu, &identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: {identifier} names no job here, so the stop must answer 404, got {} \
             {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: the stop must answer ResourceNotFoundException, got {:?}",
            reply.error
        );
    }

    // A `model-invocation-job` ARN of ANOTHER region never reaches the lookup: this service
    // reads such an ARN itself and refuses it ahead of every handler
    // (`state::SELF_VALIDATED_RESOURCE_TYPES`), so the answer is its own refusal rather than
    // "no such job".
    let foreign = stop(&emu, &other_arn).await;
    assert_eq!(
        foreign.status, 400,
        "an ARN of another region is refused before the lookup, got {} {:?}",
        foreign.status, foreign.body
    );
    assert_eq!(
        foreign.error.as_deref(),
        Some("ValidationException"),
        "an ARN of another region must answer ValidationException, got {:?}",
        foreign.error
    );

    // A refused stop wound nothing down: the job of the other region is where it was.
    let read = get_in_region(&emu, elsewhere, &other_arn).await;
    assert_eq!(
        read.status, 200,
        "the job must still read back in {elsewhere}, got {} {:?}",
        read.status, read.body
    );
    let status = text("the job of the other region", &read.body, "status");
    assert!(
        status != STOPPING && status != STOPPED,
        "a refused stop must wind no job down, got {status:?}"
    );
}
