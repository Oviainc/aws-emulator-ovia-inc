// SPDX-License-Identifier: CC0-1.0

//! The processing scope a submission delimits, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes, and then reads the stored record back out of the
//! shared engine the router hands the service.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * An accepted submission's `inputConfiguration` is retained WHOLE on the job it
//!   mints, so a submission that delimits part of the asset is distinguishable in stored
//!   state from one that asks for the whole asset.
//! * The two timestamps of a delimited segment describe an interval, so the end has to
//!   lie after the start: a segment that does not is refused with `ValidationException`
//!   (400) and mints nothing. The model bounds each member alone and cannot relate one
//!   member to another, so that cross-member rule is the service's.
//! * The per-member bounds the model DOES state (`startTimeMillis` `@range(min: 0)`,
//!   `endTimeMillis` `@range(min: 300000)`) are refused the same way, so the rows below
//!   cover each published bound and one value past it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// A legal `dataAutomationConfiguration.dataAutomationProjectArn` (pattern from
/// `smithy_query` shape `DataAutomationProjectArn`): what tells the service WHAT to
/// extract, which every accepted submission has to name.
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/clips";

/// The published lower bound of `endTimeMillis` (`@range(min: 300000)`, `smithy_query`
/// shape `TimestampSegment`).
const END_TIME_MILLIS_MIN: i64 = 300_000;

/// The published lower bound of `startTimeMillis` (`@range(min: 0)`, same shape).
const START_TIME_MILLIS_MIN: i64 = 0;

/// One row of the acceptance table: the case name, the `inputConfiguration` submitted,
/// whether the minted job must retain an asset-processing configuration, and the
/// `(startTimeMillis, endTimeMillis)` of the segment it must have retained.
type ScopeCase = (&'static str, Value, bool, Option<(i64, i64)>);

/// A router serving only this service, plus the engine it shares with it — the engine is
/// kept so a test can read the record a submission persisted.
///
/// [`PROJECT_ARN`] is arranged in the store of the service that OWNS data-automation
/// projects first: a submission's project reference is resolved against it, so a
/// submission naming a project nobody created is refused before any rule this suite is
/// about can be reached.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "asset-processing-clips");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }

    /// The error message the body carries, or the empty string.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one operation on `router` the way an SDK would.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// A submission carrying `input_configuration`, with every other required member (and
/// the project that says what to extract) at its minimum.
fn submission(input_configuration: Value) -> Value {
    json!({
        "inputConfiguration": input_configuration,
        "outputConfiguration": {"s3Uri": "s3://out-bucket/results/"},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    })
}

/// An `inputConfiguration` delimiting the timestamp segment `start..end`.
fn segment_input(start: i64, end: i64) -> Value {
    json!({
        "s3Uri": "s3://in-bucket/assets/clip.mp4",
        "assetProcessingConfiguration": {
            "video": {
                "segmentConfiguration": {
                    "timestampSegment": {
                        "startTimeMillis": start,
                        "endTimeMillis": end,
                    },
                },
            },
        },
    })
}

/// The `(startTimeMillis, endTimeMillis)` the stored invocation `arn` was submitted with,
/// and whether it retained an asset-processing configuration at all.
fn stored_scope(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    arn: &str,
) -> (bool, Option<(i64, i64)>) {
    let state = bda::state::service_state(&**engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be on the engine"));
    let store = state.lock();
    let invocation = store
        .invocation(arn)
        .unwrap_or_else(|| panic!("case {case:?}: {arn} must name a stored invocation"));
    (
        invocation.asset_processing_configuration.is_some(),
        invocation
            .requested_timestamp_segment()
            .map(|segment| (segment.start_time_millis, segment.end_time_millis)),
    )
}

/// How many invocations the store retains.
fn invocation_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = bda::state::service_state(&**engine).expect("this service's store");
    let store = state.lock();
    store.invocation_count()
}

#[tokio::test]
async fn an_accepted_submission_retains_the_processing_scope_it_asked_for() {
    let (router, engine) = service();

    // Rows differ in the scope the submission delimits: none at all, an empty
    // configuration, a video configuration carrying no segment, and segments at the
    // published bounds and far past them. What each row asserts is what the minted job
    // retained.
    let rows: Vec<ScopeCase> = vec![
        (
            "no asset processing configuration at all: the whole asset",
            json!({"s3Uri": "s3://in-bucket/assets/doc.pdf"}),
            false,
            None,
        ),
        (
            "an asset processing configuration naming no modality",
            json!({
                "s3Uri": "s3://in-bucket/assets/clip.mp4",
                "assetProcessingConfiguration": {},
            }),
            true,
            None,
        ),
        (
            "a video configuration delimiting no segment",
            json!({
                "s3Uri": "s3://in-bucket/assets/clip.mp4",
                "assetProcessingConfiguration": {"video": {}},
            }),
            true,
            None,
        ),
        (
            "a segment starting at the startTimeMillis bound and ending at the endTimeMillis bound",
            segment_input(START_TIME_MILLIS_MIN, END_TIME_MILLIS_MIN),
            true,
            Some((START_TIME_MILLIS_MIN, END_TIME_MILLIS_MIN)),
        ),
        (
            "a segment one millisecond wide at the endTimeMillis bound",
            segment_input(END_TIME_MILLIS_MIN - 1, END_TIME_MILLIS_MIN),
            true,
            Some((END_TIME_MILLIS_MIN - 1, END_TIME_MILLIS_MIN)),
        ),
        (
            "a segment starting past the endTimeMillis bound",
            segment_input(600_000, 900_000),
            true,
            Some((600_000, 900_000)),
        ),
        (
            "a segment near the end of a day-long asset",
            segment_input(86_399_000, 86_400_000),
            true,
            Some((86_399_000, 86_400_000)),
        ),
    ];

    for (case, input_configuration, want_configuration, want_segment) in &rows {
        let answer = call(
            &router,
            "InvokeDataAutomationAsync",
            submission(input_configuration.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
            answer.status, answer.body
        );
        let arn = answer.body["invocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
            .to_string();

        let (has_configuration, segment) = stored_scope(&engine, case, &arn);
        assert_eq!(
            has_configuration, *want_configuration,
            "case {case:?}: expected the stored record to carry an assetProcessingConfiguration = \
             {want_configuration}, got {has_configuration}"
        );
        assert_eq!(
            segment, *want_segment,
            "case {case:?}: expected the stored segment {want_segment:?}, got {segment:?}"
        );

        // The job the submission minted is readable through its own reader, so the
        // retained scope rides on a record the service still resolves.
        let read = call(
            &router,
            "GetDataAutomationStatus",
            json!({"invocationArn": arn}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
            read.status, read.body
        );
        assert_eq!(
            read.body["outputConfiguration"]["s3Uri"].as_str(),
            Some(crate::support::job_metadata_uri("s3://out-bucket/results/", &arn).as_str()),
            "case {case:?}: the reader must report this job's own output location under the \
             submitted prefix, got {}",
            read.body
        );
    }

    assert_eq!(
        invocation_count(&engine),
        rows.len(),
        "every accepted submission must have minted exactly one retained job"
    );
}

#[tokio::test]
async fn a_segment_outside_its_constraints_is_refused_and_mints_nothing() {
    let (router, engine) = service();

    // `true` means the refusal is the service's own cross-member rule (its one message);
    // `false` means a per-member bound the model states, whose wording the framework's
    // boundary walk owns.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "an end before the start",
            segment_input(600_000, END_TIME_MILLIS_MIN),
            true,
        ),
        (
            "an end equal to the start",
            segment_input(END_TIME_MILLIS_MIN, END_TIME_MILLIS_MIN),
            true,
        ),
        (
            "an end one millisecond before the start",
            segment_input(END_TIME_MILLIS_MIN + 1, END_TIME_MILLIS_MIN),
            true,
        ),
        (
            "an end far before a start late in the asset",
            segment_input(86_400_000, END_TIME_MILLIS_MIN),
            true,
        ),
        (
            "an end one millisecond below the endTimeMillis bound",
            segment_input(START_TIME_MILLIS_MIN, END_TIME_MILLIS_MIN - 1),
            false,
        ),
        (
            "a start one millisecond below the startTimeMillis bound",
            segment_input(START_TIME_MILLIS_MIN - 1, END_TIME_MILLIS_MIN),
            false,
        ),
    ];

    for (case, input_configuration, own_rule) in &rows {
        let answer = call(
            &router,
            "InvokeDataAutomationAsync",
            submission(input_configuration.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {}",
            answer.body
        );
        if *own_rule {
            assert_eq!(
                answer.message(),
                bda::state::INCONSISTENT_TIMESTAMP_SEGMENT,
                "case {case:?}: expected this service's cross-member refusal, got {:?}",
                answer.message()
            );
        } else {
            assert!(
                !answer.message().is_empty(),
                "case {case:?}: a refusal must say what failed, got {}",
                answer.body
            );
        }
    }

    // A refused submission is refused BEFORE anything is minted, so the store is
    // untouched — and a later legal submission still works.
    assert_eq!(
        invocation_count(&engine),
        0,
        "a refused submission must leave no invocation behind"
    );
    let accepted = call(
        &router,
        "InvokeDataAutomationAsync",
        submission(segment_input(START_TIME_MILLIS_MIN, END_TIME_MILLIS_MIN)),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a legal segment must still be accepted after the refusals, got {} / {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        invocation_count(&engine),
        1,
        "the accepted submission must be the only retained job"
    );
}
