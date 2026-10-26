// SPDX-License-Identifier: CC0-1.0

//! The advanced prompt optimization job resource end-to-end:
//! `CreateAdvancedPromptOptimizationJob` writes it, `GetAdvancedPromptOptimizationJob`
//! reads it back and `StopAdvancedPromptOptimizationJob` winds it down, over the same
//! `MultiServiceRouter` the emulator boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created job round-trips**: the create answers the modelled 200 with a `jobArn` of
//!    the form `AdvancedPromptOptimizationJobArn` declares, and every member the read
//!    reports comes back out of the stored record — for requests differing in name length
//!    (both modelled bounds), description, encryption key, tag count, the number of model
//!    configurations (1 and the modelled maximum of 5), the inference parameters those
//!    configurations carry, and the S3 URI forms the input and output configurations admit.
//! 2. **Both identifier spellings address the same job**: the job ARN and the bare minted
//!    id answer identically, over the rows of 1.
//! 3. **`jobStatus` is `InProgress` and a read changes nothing**: the job is reported
//!    running from the create onwards, and two reads of one job answer identically.
//! 4. **A replayed `clientToken` mints nothing**: the replay answers with the job the first
//!    call minted, whatever else its body carries.
//! 5. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException` — including a bare id read in another region, because the
//!    ARN a bare id completes to carries the region, while the job's own ARN addresses it
//!    from any signing region.
//! 6. **A stop moves a job that is in progress to `Stopping`**: it answers the modelled 200
//!    with a body carrying no members, and the stored job is reported `Stopping` from that
//!    call onwards — over jobs differing in region, identifier spelling and configuration.
//! 7. **The `Stopping` status is owed its move to `Stopped` by the CLOCK**: once
//!    `ADVANCED_PROMPT_OPTIMIZATION_JOB_STOPPED_AFTER` has passed the next read reports
//!    `Stopped`, stamped at the instant the move was owed, while a job nobody stopped is
//!    still `InProgress` after that same span.
//! 8. **A job that is not in progress cannot be stopped**, so a stop of a stopping or
//!    stopped job is refused with 400 `ValidationException`, and a stop of an identifier no
//!    stored job answers to with 404 `ResourceNotFoundException`.
//! 9. **More tags than one resource may carry is refused** with 400 `TooManyTagsException`,
//!    walking the bound itself.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateAdvancedPromptOptimizationJob,GetAdvancedPromptOptimizationJob,StopAdvancedPromptOptimizationJob")`
//! / `query="shape"`).

use serde_json::{json, Value};

use crate::support::{emulator, label, wall_clock_seconds, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// `AdvancedPromptOptimizationModelIdentifier` values the model admits
/// (`smithy_query(query="shape",shape="ModelConfiguration")`): a bare `<provider>.<model>`
/// id, and the `::foundation-model/<id>` ARN its pattern also spells.
const MODEL_IDS: &[&str] = &[
    "anthropic.claude-v2",
    "amazon.titan-text-express-v1",
    "meta.llama3-8b-instruct-v1",
    "arn:aws:bedrock:us-west-2::foundation-model/anthropic.claude-v2",
    "mistral.mistral-large-2402",
];

/// `S3Uri` values `AdvancedPromptOptimizationInputConfig$s3Uri` admits (pattern
/// `^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/…)?/?$`).
const INPUT_URIS: &[&str] = &[
    "s3://prompt-optimization-input/templates.jsonl",
    "s3://prompt.optimization.data/runs/2026/input.jsonl",
];

/// `S3UriFolder` values `AdvancedPromptOptimizationOutputConfig$s3Uri` admits — the same
/// expression, ending in `/`.
const OUTPUT_URIS: &[&str] = &[
    "s3://prompt-optimization-output/results/",
    "s3://prompt.optimization.data/runs/2026/",
];

/// A `KmsKeyArn` `encryptionKeyArn` admits (pattern
/// `^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`).
const ENCRYPTION_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555";

/// `AdvancedPromptOptimizationJobStatus` is `[InProgress, Completed, Failed,
/// PartiallyCompleted, Stopping, Stopped, Deleting]`
/// (`smithy_query(query="shape",shape="AdvancedPromptOptimizationJobStatus")`); an accepted
/// job runs, and a stop walks it through the two further values below.
const IN_PROGRESS: &str = "InProgress";
const STOPPING: &str = "Stopping";
const STOPPED: &str = "Stopped";

/// How long after a stop the job's move to [`STOPPED`] is owed, as the handler holds it.
const STOP_SPAN: std::time::Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::ADVANCED_PROMPT_OPTIMIZATION_JOB_STOPPED_AFTER;

/// The most tags one resource of this service may carry, as the handler enforces it.
const MAX_TAGS: usize =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

/// The most model configurations one job may carry: `ModelConfigurations` is
/// `@length(1..5)` (`smithy_query(query="shape",shape="ModelConfigurations")`).
const MAX_MODEL_CONFIGURATIONS: usize = 5;

// ---------------------------------------------------------------------------------------
// Request bodies and the calls that carry them.
// ---------------------------------------------------------------------------------------

fn model_configuration(model_id: &str) -> Value {
    json!({ "modelId": model_id })
}

/// A `ModelConfiguration` carrying the inference parameters and additional request fields
/// the shape admits beside the model id. The float values are exactly representable, so a
/// round trip through the wire's `Float` cannot change their spelling.
fn tuned_model_configuration(model_id: &str) -> Value {
    json!({
        "modelId": model_id,
        "inferenceConfig": {
            "maxTokens": 512,
            "temperature": 0.5,
            "topP": 0.75,
            "stopSequences": ["Human:", "Assistant:"],
        },
        "additionalModelRequestFields": { "top_k": 250 },
    })
}

fn model_configurations(count: usize, tuned: bool) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let model_id = MODEL_IDS[index % MODEL_IDS.len()];
                if tuned {
                    tuned_model_configuration(model_id)
                } else {
                    model_configuration(model_id)
                }
            })
            .collect(),
    )
}

fn tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "key": format!("key-{index}"), "value": format!("value-{index}") }))
            .collect(),
    )
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/advanced-prompt-optimization-jobs", Some(body))
        .await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        "/advanced-prompt-optimization-jobs",
        Some(body),
    )
    .await
}

async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!(
            "/advanced-prompt-optimization-jobs/{}",
            label(identifier)
        ),
        None,
    )
    .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    get_in_region(emu, REGION, identifier).await
}

/// `StopAdvancedPromptOptimizationJob` is bound to
/// `POST /advanced-prompt-optimization-jobs/{jobIdentifier}/stop` and its request carries
/// no member beside that label, so the call sends no body.
async fn stop_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        &format!(
            "/advanced-prompt-optimization-jobs/{}/stop",
            label(identifier)
        ),
        None,
    )
    .await
}

async fn stop(emu: &Emulator, identifier: &str) -> Reply {
    stop_in_region(emu, REGION, identifier).await
}

// ---------------------------------------------------------------------------------------
// Expectations the model fixes.
// ---------------------------------------------------------------------------------------

/// Whether `value` is an `AdvancedPromptOptimizationJobArn` of `region` whose resource id
/// is the `[a-z0-9]{12}` the pattern admits
/// (`^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/[a-z0-9]{12}$`).
fn is_job_arn(value: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:advanced-prompt-optimization-job/");
    let Some(id) = value.strip_prefix(&prefix) else {
        return false;
    };
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The bare minted id inside a job ARN — the OTHER spelling an
/// `AdvancedPromptOptimizationJobIdentifier` admits.
fn job_id_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .expect("a job ARN carries its minted id after the last slash")
        .to_string()
}

/// Whether `body` carries no members: `StopAdvancedPromptOptimizationJobResponse` is
/// modelled with none
/// (`smithy_query(query="shape",shape="StopAdvancedPromptOptimizationJobResponse")`), so
/// the answer is either an empty document or an empty object.
fn carries_no_members(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty)
}

// ---------------------------------------------------------------------------------------
// 1-3. A created job round-trips through a read, under both identifier spellings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_job_round_trips_through_a_read() {
    let emu = emulator();

    // Rows differ along what the rule generalises over: the name lengths the model admits
    // (`AdvancedPromptOptimizationJobName` is `@length(1..100)`), the optional description
    // (both `@length(1..500)` bounds), the optional encryption key, the tag count, the
    // number of model configurations (`ModelConfigurations` is `@length(1..5)`), whether
    // those configurations carry inference parameters, and the two S3 URI forms.
    let long_name = format!("j{}", "x".repeat(99));
    let long_description = "d".repeat(500);
    let rows: Vec<(&str, Value)> = vec![
        (
            "only the required members, shortest name",
            json!({
                "jobName": "a",
                "inputConfig": { "s3Uri": INPUT_URIS[0] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
                "modelConfigurations": model_configurations(1, false),
            }),
        ),
        (
            "every optional member, longest name, the model bound of configurations",
            json!({
                "jobName": long_name,
                "jobDescription": long_description,
                "inputConfig": { "s3Uri": INPUT_URIS[1] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[1] },
                "encryptionKeyArn": ENCRYPTION_KEY_ARN,
                "tags": tags(MAX_TAGS),
                "modelConfigurations": model_configurations(MAX_MODEL_CONFIGURATIONS, true),
            }),
        ),
        (
            "a punctuated name, a one-character description, three configurations",
            json!({
                "jobName": "job.name+one-2",
                "jobDescription": "d",
                "inputConfig": { "s3Uri": INPUT_URIS[1] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
                "tags": tags(1),
                "modelConfigurations": model_configurations(3, false),
            }),
        ),
    ];

    for (case, body) in rows {
        let before = wall_clock_seconds();
        let created = create(&emu, body.clone()).await;
        let after = wall_clock_seconds();
        assert_eq!(
            created.status, 200,
            "case {case}: the create must answer the modelled 200, got {} {:?}",
            created.status, created.body
        );
        let arn = created.text("jobArn");
        assert!(
            is_job_arn(&arn, REGION),
            "case {case}: jobArn must be an AdvancedPromptOptimizationJobArn of {REGION}, got \
             {arn:?}"
        );

        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the read of {arn} must answer 200, got {} {:?}",
            read.status, read.body
        );
        for member in ["jobName", "inputConfig", "outputConfig", "modelConfigurations"] {
            assert_eq!(
                read.get(member),
                body[member],
                "case {case}: the read's {member} must be what the create carried ({:?}), got \
                 {:?}",
                body[member],
                read.get(member)
            );
        }
        for member in ["jobDescription", "encryptionKeyArn"] {
            let expected = body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                read.get(member),
                expected,
                "case {case}: the read's {member} must be what the create carried \
                 ({expected:?}), got {:?}",
                read.get(member)
            );
        }
        assert_eq!(
            read.text("jobArn"),
            arn,
            "case {case}: the read must report the ARN it was addressed by, got {:?}",
            read.get("jobArn")
        );
        assert_eq!(
            read.get("jobStatus"),
            json!(IN_PROGRESS),
            "case {case}: an accepted job is {IN_PROGRESS}, got {:?}",
            read.get("jobStatus")
        );
        assert_eq!(
            read.get("failureMessage"),
            Value::Null,
            "case {case}: a job that has not failed reports no failureMessage, got {:?}",
            read.get("failureMessage")
        );
        let created_at = read.instant("creationTime");
        assert!(
            created_at >= before && created_at <= after,
            "case {case}: creationTime must fall in the span of the create call \
             [{before}, {after}], got {created_at}"
        );
        assert_eq!(
            read.instant("lastModifiedTime"),
            created_at,
            "case {case}: nothing has changed the job, so lastModifiedTime is its \
             creationTime ({created_at}), got {}",
            read.instant("lastModifiedTime")
        );

        // 2. The bare minted id is the other spelling of the identifier, and answers alike.
        let by_id = get(&emu, &job_id_of(&arn)).await;
        assert_eq!(
            by_id.status, 200,
            "case {case}: the bare id must address the same job, got {} {:?}",
            by_id.status, by_id.body
        );
        assert_eq!(
            by_id.body, read.body,
            "case {case}: both identifier spellings must answer identically, got {:?} and \
             {:?}",
            by_id.body, read.body
        );

        // 3. A read changes nothing a later caller sees.
        let again = get(&emu, &arn).await;
        assert_eq!(
            again.body, read.body,
            "case {case}: two reads of one job must answer identically, got {:?} then {:?}",
            read.body, again.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A replayed clientToken mints nothing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_mints_nothing() {
    let emu = emulator();

    // Rows differ in what the REPLAY carries beside the token: the same body, another name,
    // and another set of model configurations — the token decides, not the body.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "the same body",
            "token-one",
            "replay-one",
            model_configurations(1, false),
        ),
        (
            "another job name",
            "token-two",
            "replay-two-other",
            model_configurations(1, false),
        ),
        (
            "another set of model configurations",
            "token-three",
            "replay-three",
            model_configurations(4, true),
        ),
    ];

    for (case, token, replay_name, replay_configurations) in rows {
        let first_name = format!("first-{token}");
        let first = create(
            &emu,
            json!({
                "jobName": first_name,
                "clientToken": token,
                "inputConfig": { "s3Uri": INPUT_URIS[0] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
                "modelConfigurations": model_configurations(2, false),
            }),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case}: the first create must answer 200, got {} {:?}",
            first.status, first.body
        );
        let arn = first.text("jobArn");

        let replay = create(
            &emu,
            json!({
                "jobName": replay_name,
                "clientToken": token,
                "inputConfig": { "s3Uri": INPUT_URIS[1] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[1] },
                "modelConfigurations": replay_configurations,
            }),
        )
        .await;
        assert_eq!(
            replay.status, 200,
            "case {case}: the replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("jobArn"),
            arn,
            "case {case}: the replay must answer with the job the first call minted ({arn}), \
             got {:?}",
            replay.get("jobArn")
        );

        // The replay minted nothing, so the stored job is still the FIRST request's.
        let read = get(&emu, &arn).await;
        assert_eq!(
            read.get("jobName"),
            json!(first_name),
            "case {case}: the stored job is the first request's, so its jobName is \
             {first_name:?}, got {:?}",
            read.get("jobName")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. An identifier no stored job answers to is refused.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_stored_job_answers_to_is_refused() {
    let emu = emulator();

    let created = create(
        &emu,
        json!({
            "jobName": "known-job",
            "inputConfig": { "s3Uri": INPUT_URIS[0] },
            "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
            "modelConfigurations": model_configurations(1, false),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create must answer 200, got {} {:?}",
        created.status, created.body
    );
    let arn = created.text("jobArn");
    let id = job_id_of(&arn);

    // Rows span the ways an identifier can name no stored job: a bare id never minted, the
    // ARN of that id, an ARN whose region field is not the one the job was created in, and
    // a stored job's bare id addressed in another region (the ARN a bare id completes to
    // carries one).
    let other_region = "eu-central-1";
    let unknown_id = "aaaaaaaaaaaa";
    let unknown_arn =
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:advanced-prompt-optimization-job/{unknown_id}");
    let foreign_arn =
        format!("arn:aws:bedrock:{other_region}:{ACCOUNT}:advanced-prompt-optimization-job/{id}");
    let rows: Vec<(&str, &str, String)> = vec![
        ("a bare id never minted", REGION, unknown_id.to_string()),
        ("the ARN of that id", REGION, unknown_arn),
        (
            "an ARN of the stored id in another region",
            REGION,
            foreign_arn,
        ),
        (
            "a stored job's bare id in another region",
            other_region,
            id,
        ),
    ];

    for (case, region, identifier) in rows {
        let read = get_in_region(&emu, region, &identifier).await;
        assert_eq!(
            read.status, 404,
            "case {case}: a read of {identifier} must answer 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: the refusal is ResourceNotFoundException, got {:?}",
            read.error
        );
    }

    // The job's own ARN carries the region the job lives in, so it addresses the job
    // whichever region the request is signed for — that is what makes the bare id the only
    // spelling the signing region has to complete.
    let by_arn_elsewhere = get_in_region(&emu, other_region, &arn).await;
    assert_eq!(
        by_arn_elsewhere.status, 200,
        "case the job ARN read from another region: the ARN addresses the job on its own, got \
         {} {:?}",
        by_arn_elsewhere.status, by_arn_elsewhere.body
    );

    // The stored job is still readable where it lives, so the refusals above are about the
    // identifier and not about the store.
    assert_eq!(
        get(&emu, &arn).await.status,
        200,
        "the stored job must still be readable in {REGION}, got {:?}",
        get(&emu, &arn).await.body
    );
}

// ---------------------------------------------------------------------------------------
// 6. A stop moves a job that is in progress to Stopping.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stop_moves_a_job_in_progress_to_stopping() {
    let emu = emulator();

    // Rows differ along what the rule generalises over: the region the job lives in, the
    // identifier spelling the stop addresses it by, and the configuration it carries.
    let rows: Vec<(&str, &str, bool, usize)> = vec![
        ("the job ARN, in the signing region", REGION, false, 1),
        ("the bare minted id, in the signing region", REGION, true, 3),
        ("the job ARN, in another region", "eu-central-1", false, 5),
        (
            "the bare minted id, in another region",
            "ap-southeast-2",
            true,
            2,
        ),
    ];

    for (case, region, by_bare_id, configurations) in rows {
        let created = create_in_region(
            &emu,
            region,
            json!({
                "jobName": format!("stoppable-{configurations}"),
                "inputConfig": { "s3Uri": INPUT_URIS[0] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
                "modelConfigurations": model_configurations(configurations, false),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: the create must answer 200, got {} {:?}",
            created.status, created.body
        );
        let arn = created.text("jobArn");
        let identifier = if by_bare_id {
            job_id_of(&arn)
        } else {
            arn.clone()
        };

        let created_at = get_in_region(&emu, region, &arn).await.instant("creationTime");
        let before = wall_clock_seconds();
        let stopped = stop_in_region(&emu, region, &identifier).await;
        let after = wall_clock_seconds();
        assert_eq!(
            stopped.status, 200,
            "case {case}: the stop must answer the modelled 200, got {} {:?}",
            stopped.status, stopped.body
        );
        assert!(
            carries_no_members(&stopped.body),
            "case {case}: the stop's response is modelled with no members, got {:?}",
            stopped.body
        );

        let read = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            read.get("jobStatus"),
            json!(STOPPING),
            "case {case}: the job is {STOPPING} from the stop onwards, got {:?}",
            read.get("jobStatus")
        );
        let modified = read.instant("lastModifiedTime");
        assert!(
            modified >= before && modified <= after,
            "case {case}: lastModifiedTime must fall in the span of the stop call \
             [{before}, {after}], got {modified}"
        );
        assert_eq!(
            read.instant("creationTime"),
            created_at,
            "case {case}: a stop does not change creationTime ({created_at}), got {}",
            read.instant("creationTime")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 7. The Stopping status settles to Stopped on the clock.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_stopping_status_settles_to_stopped_on_the_clock() {
    let emu = emulator();

    // Two jobs created the same way: one is stopped, the other is not, so the move below is
    // attributable to the STOP and not to the span passing.
    let mut arns: Vec<String> = Vec::new();
    for name in ["settling-stop", "never-stopped"] {
        let created = create(
            &emu,
            json!({
                "jobName": name,
                "inputConfig": { "s3Uri": INPUT_URIS[0] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
                "modelConfigurations": model_configurations(1, false),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name}: the create must answer 200, got {} {:?}",
            created.status, created.body
        );
        arns.push(created.text("jobArn"));
    }
    let stopped_arn = arns[0].clone();
    let running_arn = arns[1].clone();

    let before = wall_clock_seconds();
    assert_eq!(
        stop(&emu, &stopped_arn).await.status,
        200,
        "case the stop itself: it must answer 200, got {:?}",
        stop(&emu, &stopped_arn).await.body
    );

    // The span is non-zero, so a caller reading the job straight after the stop still sees
    // the transient status.
    let straight_after = get(&emu, &stopped_arn).await;
    assert_eq!(
        straight_after.get("jobStatus"),
        json!(STOPPING),
        "case straight after the stop: the job is {STOPPING}, got {:?}",
        straight_after.get("jobStatus")
    );

    // The move is owed by the CLOCK: once the span has passed the next call that touches
    // the record reports it stopped, stamped at the instant the move was owed.
    tokio::time::sleep(STOP_SPAN + std::time::Duration::from_millis(150)).await;
    let settled = get(&emu, &stopped_arn).await;
    assert_eq!(
        settled.get("jobStatus"),
        json!(STOPPED),
        "case after the stop span: the job is {STOPPED} once the span has passed, got {:?}",
        settled.get("jobStatus")
    );
    let modified = settled.instant("lastModifiedTime");
    let owed_at = before + STOP_SPAN.as_secs_f64();
    assert!(
        modified >= owed_at - 0.05,
        "case after the stop span: lastModifiedTime is the instant the move was owed (about \
         {owed_at}), got {modified}"
    );
    assert!(
        modified <= wall_clock_seconds(),
        "case after the stop span: lastModifiedTime cannot be in the future, got {modified}"
    );

    // A second read reports the same terminal status: the settle happened once.
    assert_eq!(
        get(&emu, &stopped_arn).await.body,
        settled.body,
        "case a second read after the settle: it must answer identically, got {:?}",
        get(&emu, &stopped_arn).await.body
    );

    // The job nobody stopped is untouched by the same span.
    let running = get(&emu, &running_arn).await;
    assert_eq!(
        running.get("jobStatus"),
        json!(IN_PROGRESS),
        "case the job nobody stopped: it is still {IN_PROGRESS} after the span, got {:?}",
        running.get("jobStatus")
    );
}

// ---------------------------------------------------------------------------------------
// 8. A job that is not in progress cannot be stopped.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stop_of_a_job_that_is_not_in_progress_is_refused() {
    let emu = emulator();

    let created = create(
        &emu,
        json!({
            "jobName": "twice-stopped",
            "inputConfig": { "s3Uri": INPUT_URIS[0] },
            "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
            "modelConfigurations": model_configurations(1, false),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create must answer 200, got {} {:?}",
        created.status, created.body
    );
    let arn = created.text("jobArn");
    assert_eq!(
        stop(&emu, &arn).await.status,
        200,
        "the first stop must answer 200, got {:?}",
        stop(&emu, &arn).await.body
    );

    // A stop of the job while it is `Stopping`, and again once the clock has moved it to
    // `Stopped`: neither status is one a stop may be asked for.
    let while_stopping = stop(&emu, &arn).await;
    assert_eq!(
        while_stopping.status, 400,
        "case a stop while the job is {STOPPING}: it must be refused with 400, got {} {:?}",
        while_stopping.status, while_stopping.body
    );
    assert_eq!(
        while_stopping.error.as_deref(),
        Some("ValidationException"),
        "case a stop while the job is {STOPPING}: the refusal is ValidationException, got {:?}",
        while_stopping.error
    );

    tokio::time::sleep(STOP_SPAN + std::time::Duration::from_millis(150)).await;
    assert_eq!(
        get(&emu, &arn).await.get("jobStatus"),
        json!(STOPPED),
        "case the settled job: it is {STOPPED}, got {:?}",
        get(&emu, &arn).await.get("jobStatus")
    );
    let while_stopped = stop(&emu, &job_id_of(&arn)).await;
    assert_eq!(
        while_stopped.status, 400,
        "case a stop of a {STOPPED} job, by its bare id: it must be refused with 400, got {} \
         {:?}",
        while_stopped.status, while_stopped.body
    );
    assert_eq!(
        while_stopped.error.as_deref(),
        Some("ValidationException"),
        "case a stop of a {STOPPED} job: the refusal is ValidationException, got {:?}",
        while_stopped.error
    );

    // And a stop of an identifier no stored job answers to is the read's refusal.
    for (case, identifier) in [
        ("a bare id never minted", "bbbbbbbbbbbb".to_string()),
        (
            "the ARN of that id",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:advanced-prompt-optimization-job/bbbbbbbbbbbb"
            ),
        ),
    ] {
        let refused = stop(&emu, &identifier).await;
        assert_eq!(
            refused.status, 404,
            "case {case}: a stop of {identifier} must answer 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: the refusal is ResourceNotFoundException, got {:?}",
            refused.error
        );
    }
}

// ---------------------------------------------------------------------------------------
// 9. The per-resource tag bound.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn more_tags_than_a_resource_may_carry_is_refused() {
    let emu = emulator();

    // Rows walk the bound: none, one, exactly the bound, and one past it.
    let rows: Vec<(&str, usize, u16)> = vec![
        ("no tags", 0, 200),
        ("one tag", 1, 200),
        ("the bound itself", MAX_TAGS, 200),
        ("one past the bound", MAX_TAGS + 1, 400),
    ];

    for (case, count, expected) in rows {
        let reply = create(
            &emu,
            json!({
                "jobName": format!("tagged-{count}"),
                "inputConfig": { "s3Uri": INPUT_URIS[0] },
                "outputConfig": { "s3Uri": OUTPUT_URIS[0] },
                "tags": tags(count),
                "modelConfigurations": model_configurations(1, false),
            }),
        )
        .await;
        assert_eq!(
            reply.status, expected,
            "case {case}: {count} tags must answer {expected}, got {} {:?}",
            reply.status, reply.body
        );
        if expected == 400 {
            assert_eq!(
                reply.error.as_deref(),
                Some("TooManyTagsException"),
                "case {case}: the refusal is the modelled TooManyTagsException, got {:?}",
                reply.error
            );
            assert_eq!(
                reply.get("jobArn"),
                Value::Null,
                "case {case}: a refused create mints no job, got {:?}",
                reply.body
            );
        } else {
            let arn = reply.text("jobArn");
            assert_eq!(
                get(&emu, &arn).await.status,
                200,
                "case {case}: the minted job must read back, got {:?}",
                get(&emu, &arn).await.body
            );
        }
    }
}
