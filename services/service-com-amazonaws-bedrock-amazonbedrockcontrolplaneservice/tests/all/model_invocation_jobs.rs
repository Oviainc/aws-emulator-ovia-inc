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
//! 5. **`status` is non-terminal first and settles on the CLOCK**: `Submitted` with no
//!    `endTime` straight after the create, `InProgress` once
//!    `MODEL_INVOCATION_JOB_BEGINS_AFTER` has passed, `Completed` with an `endTime` once
//!    `MODEL_INVOCATION_JOB_COMPLETE_AFTER` has passed on top of it — the same status a
//!    listing reports and `statusEquals` filters on, and reading twice reports the same
//!    instants.
//! 6. **A replayed `clientRequestToken` submits nothing**: the replay answers with the job
//!    the first call minted and stores no second job, while a fresh token submits another.
//! 7. **The listing orders, filters and pages** the stored jobs: creation order in both
//!    `sortOrder` directions, `nameContains` and the two submit-time bounds, pages that
//!    partition the listing through `nextToken`, a `nextToken` from nowhere refused, and no
//!    job of another region listed.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateModelInvocationJob,GetModelInvocationJob,ListModelInvocationJobs")`
//! / `query="shape"`), or from the published foundation model catalog this service reports
//! through `ListFoundationModels`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::support::{
    date_time_seconds, emulator, foundation_model_arn, label, Emulator, Reply, ACCOUNT, REGION,
};

type State = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState;

/// The published foundation model catalog this service serves: the rows
/// `ListFoundationModels` reports (`state::FOUNDATION_MODELS`), so these rows follow the
/// catalog the service serves rather than a spelling it may add to or drop.
const CATALOG: &[service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FoundationModel] =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS;

/// How long after it was submitted a job is owed its move to `InProgress`.
const BEGINS_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_BEGINS_AFTER;

/// How long after it began a job is owed its move to `Completed`.
const COMPLETE_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_INVOCATION_JOB_COMPLETE_AFTER;

/// A margin on top of a settle span, so a read lands after the instant the move was owed
/// rather than exactly on it.
const MARGIN: Duration = Duration::from_millis(150);

/// `ModelInvocationJobStatus` values this emulator's lifecycle passes through.
const SUBMITTED: &str = "Submitted";
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

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
// 5. The status is non-terminal first and settles on the clock.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_status_settles_from_submitted_through_in_progress_to_completed() {
    let emu = emulator();

    let arn = submit(
        &emu,
        "the settling job",
        create_body("batch-settling", catalog_model(0)),
    )
    .await;

    // Straight after the create, a read and a listing both report it queued: the caller
    // observes the transient status the real service reports.
    let read = get(&emu, &arn).await;
    assert_eq!(
        text("straight after the create", &read.body, "status"),
        SUBMITTED,
        "the read must report the queued status"
    );
    let submitted = instant("straight after the create", &read.body, "submitTime");
    let rows = listed("straight after the create", &list(&emu, "").await);
    assert_eq!(
        text("straight after the create", &rows[0], "status"),
        SUBMITTED,
        "the listing must report the same status the read does"
    );

    // The first move is owed by the CLOCK: once the span has passed the next call that
    // touches the record reports the job running, stamped at the instant it was owed.
    tokio::time::sleep(BEGINS_AFTER + MARGIN).await;
    let running = get(&emu, &arn).await;
    assert_eq!(
        text("after the begin span", &running.body, "status"),
        IN_PROGRESS,
        "the read must report the job running once the begin span has passed"
    );
    let began = instant("after the begin span", &running.body, "lastModifiedTime");
    assert!(
        began >= submitted + BEGINS_AFTER.as_secs_f64() - 0.001,
        "the begin must be stamped at the instant it was owed ({}), got {began} against a \
         submission at {submitted}",
        BEGINS_AFTER.as_secs_f64()
    );
    assert_eq!(
        running.body.get("endTime"),
        None,
        "a running job has no endTime, got {:?}",
        running.body.get("endTime")
    );
    assert_eq!(
        instant("after the begin span", &running.body, "submitTime"),
        submitted,
        "a settle must leave the submission instant alone"
    );

    // The second move is owed the same way, and it is the job's end.
    tokio::time::sleep(COMPLETE_AFTER + MARGIN).await;
    let done = get(&emu, &arn).await;
    assert_eq!(
        text("after the complete span", &done.body, "status"),
        COMPLETED,
        "the read must report the job completed once the run span has passed"
    );
    let ended = instant("after the complete span", &done.body, "endTime");
    assert!(
        ended >= began + COMPLETE_AFTER.as_secs_f64() - 0.001,
        "the completion must be stamped at the instant it was owed ({}), got {ended} against a \
         begin at {began}",
        COMPLETE_AFTER.as_secs_f64()
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

    // And `statusEquals` filters on the status each job holds NOW.
    assert_eq!(
        listed_arns(
            "statusEquals=Completed after the settle",
            &list(&emu, &format!("statusEquals={COMPLETED}")).await
        ),
        vec![arn.clone()],
        "the completed filter must report the settled job"
    );
    for gone in [SUBMITTED, IN_PROGRESS] {
        let reply = list(&emu, &format!("statusEquals={gone}")).await;
        assert!(
            listed_arns("a status the job has left", &reply).is_empty(),
            "nothing is {gone} once the job completed, got {:?}",
            reply.body
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
