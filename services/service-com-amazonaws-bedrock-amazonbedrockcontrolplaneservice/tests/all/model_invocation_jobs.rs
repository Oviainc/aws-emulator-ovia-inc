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
//!    that sets one, and is absent for a job that set none.
//! 3. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException`, over rows spanning a never-minted id, a well-formed ARN
//!    that was never minted and a live job of another region.
//! 4. **`modelId` is resolved against the published catalog**: an identifier that names no
//!    model of this account is 404 `ResourceNotFoundException`, while every model the
//!    catalog publishes is accepted.
//! 5. **`status` walks the documented progression on the CLOCK**: `Submitted` with no
//!    `endTime` straight after the create, then `Validating`, then `Scheduled`, then
//!    `InProgress`, each stamped at the instant its own span fell due — the same status a
//!    listing reports and `statusEquals` filters on — and `Completed` with an `endTime` only
//!    once the run span has elapsed on top of them. The whole timeline is asserted at each
//!    instant it turns on, and over the intervals a client polls a job at: at every one of
//!    them the first status outside `{Submitted, Validating, Scheduled}` a poller reads is
//!    the running one, and reading twice reports the same instants.
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

use std::time::Duration;

use serde_json::{json, Value};

use crate::support::{
    date_time_seconds, emulator, foundation_model_arn, label, wall_clock_seconds, Emulator, Reply,
    ACCOUNT, REGION,
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

/// How long after its submission a job is owed its start: the three pre-start spans end to
/// end.
fn pre_start_span() -> Duration {
    VALIDATES_AFTER + SCHEDULES_AFTER + BEGINS_AFTER
}

/// The whole span a job nobody stops takes from its submission to `Completed`.
fn lifespan() -> Duration {
    pre_start_span() + COMPLETE_AFTER
}

/// The stored record of one batch inference job, cloned out of the store the handlers share.
///
/// Cloned rather than borrowed so a case can settle it at an instant of its own choosing:
/// the clock rule is about spans of wall-clock time measured in minutes, which no test can
/// sleep through.
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
    let emu = emulator();

    // EVERY row the catalog publishes is accepted, by both spellings its `modelId` admits —
    // so the suite follows the published catalog rather than the one model a recording used.
    for (index, model) in CATALOG.iter().enumerate() {
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

    // And an identifier naming no model of this account is reported absent.
    let unknown = [
        ("a bare id no catalog row carries", "nosuchmodel".to_string()),
        (
            "a foundation model ARN no catalog row carries",
            foundation_model_arn(REGION, "nosuch.model-v1"),
        ),
        (
            "a custom model ARN that was never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/nosuch.model-v1/cccccccccccc"),
        ),
        (
            "an inference profile ARN that was never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:application-inference-profile/dddddddddddd"),
        ),
    ];
    for (case, model_id) in unknown {
        let reply = create(&emu, create_body("batch-unknown-model", &model_id)).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: {model_id} names no model, so the create must answer 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: the create must answer ResourceNotFoundException, got {:?}",
            reply.error
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. The status walks the documented progression, one stage at a time, on the clock.
// ---------------------------------------------------------------------------------------

/// Every status a stored job's lifecycle passes through, so a case can assert that the
/// filter reports the job under the status it holds and under no other.
const LIFECYCLE: &[&str] = &[SUBMITTED, VALIDATING, SCHEDULED, IN_PROGRESS, COMPLETED];

#[tokio::test]
async fn the_status_walks_every_pre_start_stage_before_it_reports_the_job_running() {
    let emu = emulator();

    let started = ::std::time::Instant::now();
    let arn = submit(
        &emu,
        "the settling job",
        create_body("batch-settling", catalog_model(0)),
    )
    .await;

    // One row per stage of the progression: how much of the job's timeline has elapsed, and
    // the status the job must then hold. Each pre-start status the model documents is a
    // status a caller READS the job in, so none of them may be skipped.
    let stages: &[(&str, Duration, &str)] = &[
        ("straight after the create", Duration::ZERO, SUBMITTED),
        (
            "once the validation span has passed",
            VALIDATES_AFTER,
            VALIDATING,
        ),
        (
            "once the scheduling span has passed",
            VALIDATES_AFTER + SCHEDULES_AFTER,
            SCHEDULED,
        ),
        ("once the begin span has passed", pre_start_span(), IN_PROGRESS),
    ];

    let mut first_submit_time: Option<f64> = None;
    for (case, elapsed, expected) in stages {
        if !elapsed.is_zero() {
            tokio::time::sleep(
                elapsed
                    .saturating_add(MARGIN)
                    .saturating_sub(started.elapsed()),
            )
            .await;
        }

        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the read must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            text(case, &read.body, "status"),
            *expected,
            "case {case}: the read must report {expected}, got {:?}",
            read.body.get("status")
        );

        // A settle leaves the submission instant alone, and stamps the move at the instant
        // the CLOCK owed it rather than at the instant of the read.
        let submitted = instant(case, &read.body, "submitTime");
        let first = *first_submit_time.get_or_insert(submitted);
        assert_eq!(
            submitted, first,
            "case {case}: a settle must leave the submission instant alone, got {submitted} \
             against {first}"
        );
        let modified = instant(case, &read.body, "lastModifiedTime");
        let owed = submitted + elapsed.as_secs_f64();
        assert!(
            (modified - owed).abs() < INSTANT_TOLERANCE,
            "case {case}: the move must be stamped at the instant it was owed ({owed}), got \
             {modified}"
        );
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
            *expected,
            "case {case}: the listing must report the same status the read does"
        );
        for status in LIFECYCLE {
            let reply = list(&emu, &format!("statusEquals={status}")).await;
            let expected_arns = if status == expected {
                vec![arn.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                listed_arns(case, &reply),
                expected_arns,
                "case {case}: statusEquals={status} must report {} while the job is {expected}",
                if status == expected {
                    "the job"
                } else {
                    "nothing"
                }
            );
        }
    }
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

    // A tick before the run span is up the job is still RUNNING, stamped at the instant it
    // began. Ageing the stored record brings the same elapsed time a wall clock would, which
    // is how a run span measured in minutes is asserted without sleeping through one.
    age_job(&emu, &arn, lifespan.saturating_sub(tick));
    let running = get(&emu, &arn).await;
    let case = "a tick before the run span is up";
    assert_eq!(
        text(case, &running.body, "status"),
        IN_PROGRESS,
        "case {case}: the job must still be running, got {:?}",
        running.body.get("status")
    );
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
        (began - pre_start).abs() < INSTANT_TOLERANCE,
        "case {case}: the begin must be stamped {pre_start} s after the submission, got {began} s"
    );

    // Once it is up the job is finished, stamped at the instant the completion was owed.
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
        (ended - owed).abs() < INSTANT_TOLERANCE,
        "case {case}: the completion must be stamped at the instant it was owed ({owed}), got \
         {ended}"
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
async fn the_clock_reports_the_stage_it_has_reached_over_the_whole_timeline() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the clocked job",
        create_body("batch-clocked", catalog_model(0)),
    )
    .await;
    let pristine = stored_job(&emu, &arn);
    let submit_time = pristine.submit_time;

    // The submission arms every transition at once, each a span after the one before it, so
    // no later call arms a move it can then spend at the same instant.
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

    let validating_at = VALIDATES_AFTER;
    let scheduled_at = validating_at.saturating_add(SCHEDULES_AFTER);
    let running_at = scheduled_at.saturating_add(BEGINS_AFTER);
    let completed_at = running_at.saturating_add(COMPLETE_AFTER);
    let tick = Duration::from_millis(1);

    // One row per instant the timeline turns on: each span's own bound, a tick before it,
    // the middle of the run, and an instant far past the end of the whole timeline.
    let rows: &[(&str, Duration, &str, Option<Duration>)] = &[
        ("the submission instant", Duration::ZERO, SUBMITTED, None),
        (
            "a tick before the validation is due",
            validating_at.saturating_sub(tick),
            SUBMITTED,
            None,
        ),
        (
            "the instant the validation is due",
            validating_at,
            VALIDATING,
            None,
        ),
        (
            "a tick before the schedule is due",
            scheduled_at.saturating_sub(tick),
            VALIDATING,
            None,
        ),
        (
            "the instant the schedule is due",
            scheduled_at,
            SCHEDULED,
            None,
        ),
        (
            "a tick before the start is due",
            running_at.saturating_sub(tick),
            SCHEDULED,
            None,
        ),
        ("the instant the start is due", running_at, IN_PROGRESS, None),
        (
            "the middle of the run",
            running_at.saturating_add(COMPLETE_AFTER / 2),
            IN_PROGRESS,
            None,
        ),
        (
            "a tick before the completion is due",
            completed_at.saturating_sub(tick),
            IN_PROGRESS,
            None,
        ),
        (
            "the instant the completion is due",
            completed_at,
            COMPLETED,
            Some(completed_at),
        ),
        (
            "an hour past the completion",
            completed_at.saturating_add(Duration::from_secs(60 * 60)),
            COMPLETED,
            Some(completed_at),
        ),
    ];

    for (case, elapsed, expected, ended) in rows {
        let mut record = pristine.clone();
        record.settle(submit_time + *elapsed);
        assert_eq!(
            record.status, *expected,
            "case {case}: the job must report {expected}, got {}",
            record.status
        );
        assert_eq!(
            record.ended_at,
            ended.map(|at| submit_time + at),
            "case {case}: the end instant must be the one the completion was owed at, got {:?}",
            record.ended_at
        );

        // The stamp is the due instant of the stage the clock has reached, never the instant
        // of the call that materialised it.
        let owed = match record.status {
            SUBMITTED => Duration::ZERO,
            VALIDATING => validating_at,
            SCHEDULED => scheduled_at,
            IN_PROGRESS => running_at,
            _ => completed_at,
        };
        assert_eq!(
            record.last_modified_at,
            submit_time + owed,
            "case {case}: the stage must be stamped at the instant it was owed ({owed:?} after \
             the submission), got {:?}",
            record.last_modified_at
        );

        // Settling again at the same instant moves nothing: what the job reports is what the
        // clock reached, not what a read did to it.
        let mut again = record.clone();
        again.settle(submit_time + *elapsed);
        assert_eq!(
            again.status, record.status,
            "case {case}: a second settle must report the same status"
        );
        assert_eq!(
            again.last_modified_at, record.last_modified_at,
            "case {case}: a second settle must report the same last modification"
        );
        assert_eq!(
            again.ended_at, record.ended_at,
            "case {case}: a second settle must report the same end instant"
        );
    }
}

#[tokio::test]
async fn a_client_polling_at_any_interval_up_to_the_run_span_observes_the_job_running() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the polled job",
        create_body("batch-polled", catalog_model(0)),
    )
    .await;
    let pristine = stored_job(&emu, &arn);
    let submit_time = pristine.submit_time;
    let lifespan = lifespan();

    // The intervals a client polls a job at, from a tight loop up to the run span itself: at
    // every one of them the job must be observable while it is RUNNING, because that is the
    // state a caller acts on — watching progress, stopping a job that has not finished.
    for interval in [
        Duration::from_millis(250),
        Duration::from_secs(1),
        Duration::from_secs(5),
        Duration::from_secs(10),
        Duration::from_secs(30),
        COMPLETE_AFTER,
    ] {
        let mut record = pristine.clone();
        let mut seen: Vec<&'static str> = Vec::new();
        let mut elapsed = Duration::ZERO;
        while elapsed <= lifespan.saturating_add(interval) {
            record.settle(submit_time + elapsed);
            seen.push(record.status);
            elapsed = elapsed.saturating_add(interval);
        }

        assert!(
            seen.contains(&IN_PROGRESS),
            "case a poll every {interval:?}: a client polling from the submission must read the \
             job running at least once, saw {seen:?}"
        );
        let settled = seen
            .iter()
            .copied()
            .find(|status| !PRE_START.contains(status))
            .unwrap_or_else(|| {
                panic!(
                    "case a poll every {interval:?}: the job must leave the pre-start statuses, \
                     saw {seen:?}"
                )
            });
        assert_eq!(
            settled, IN_PROGRESS,
            "case a poll every {interval:?}: the first status outside the pre-start set must be \
             the running one, saw {seen:?}"
        );
        assert_eq!(
            seen.last().copied(),
            Some(COMPLETED),
            "case a poll every {interval:?}: a poller that outlasts the whole timeline must read \
             the job finished, saw {seen:?}"
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

/// One row of the stop table: how long after the submission the stop is asked for, the
/// status the job therefore holds when it arrives, and which `ModelInvocationJobIdentifier`
/// spelling it is addressed by.
struct StopRow {
    case: &'static str,
    /// How long after the submission the stop is asked for.
    after: Duration,
    /// The status the job holds when the stop arrives.
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
            after: Duration::ZERO,
            status_at_stop: SUBMITTED,
            by_arn: true,
        },
        StopRow {
            case: "stopped while queued, by its minted id",
            after: Duration::ZERO,
            status_at_stop: SUBMITTED,
            by_arn: false,
        },
        StopRow {
            case: "stopped while being validated, by its ARN",
            after: VALIDATES_AFTER + MARGIN,
            status_at_stop: VALIDATING,
            by_arn: true,
        },
        StopRow {
            case: "stopped while queued for its turn, by its minted id",
            after: VALIDATES_AFTER + SCHEDULES_AFTER + MARGIN,
            status_at_stop: SCHEDULED,
            by_arn: false,
        },
        StopRow {
            case: "stopped once it had begun, by its ARN",
            after: pre_start_span() + MARGIN,
            status_at_stop: IN_PROGRESS,
            by_arn: true,
        },
        StopRow {
            case: "stopped once it had begun, by its minted id",
            after: pre_start_span() + MARGIN,
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
        if !row.after.is_zero() {
            tokio::time::sleep(row.after).await;
        }

        // The row's premise: the job holds the unfinished status this row stops it from.
        let before = get(&emu, &arn).await;
        assert_eq!(
            text(case, &before.body, "status"),
            row.status_at_stop,
            "case {case}: the job must hold the status this row stops it from, got {:?}",
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
        // takes is measured in minutes, which no test can sleep through.
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
                // The run span an unstopped job takes is measured in minutes, so the elapsed
                // time is brought by ageing the stored record rather than slept through.
                age_job(&emu, &arn, lifespan() + MARGIN);
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
