// SPDX-License-Identifier: CC0-1.0

//! `StopComposition` over the wire: how a composition ends.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call. The start-and-read surface the stop acts on is covered by
//! `tests/all/compositions.rs`; this suite is about what the stop itself does.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A STOP ENDS THE COMPOSITION IT NAMES. `StopComposition` answers the modelled empty
//!    200 and moves the composition — and every destination it broadcasts to — to
//!    `STOPPING` with an end time, leaving `startTime` where it was.
//! 2. THE REST IS OWED BY THE CLOCK, at a span that is not zero: the composition is
//!    `STOPPED` once `crate::state::COMPOSITION_STOP_SPAN` has passed, and the ephemeral
//!    resource is gone one span after that — the model's `StartComposition` doc says a
//!    "Composition stops and the resource is deleted … When StopComposition is called",
//!    so the read that follows is the modelled 404 and not an empty composition.
//! 3. A STOP CHANGES NOTHING ELSE: another composition of the same stage keeps composing,
//!    and the stage and the configurations the composition referred to are untouched.
//! 4. REFUSALS: an ARN this store holds no composition for is the modelled
//!    `ResourceNotFoundException` (HTTP 404); an ARN outside the modelled
//!    `CompositionArn` `@pattern`/`@length` is the modelled `ValidationException`
//!    (HTTP 400), which is a different answer from "not found"; and stopping a
//!    composition that is no longer composing is the modelled `ConflictException`
//!    (HTTP 409).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries, and so the account every ARN
/// built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every test in this suite signs for.
const REGION: &str = "us-east-1";

/// The span the service gives each step of a stop, read from the crate so this suite
/// cannot drift from the span the service actually enforces.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// Added to every sleep, so a span this suite waits out has certainly elapsed whatever
/// scheduling delay the test host adds.
const SLACK: Duration = Duration::from_millis(400);

/// Shorter than a stop span, to pin that the span is NOT zero: a caller acting straight
/// after the stop still sees `STOPPING`.
const WITHIN_THE_STOP_SPAN: Duration = Duration::from_millis(1);

// The `CompositionState` and `DestinationState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState,DestinationState")`:
//   CompositionState  STARTING | ACTIVE | STOPPING | FAILED | STOPPED
//   DestinationState  STARTING | ACTIVE | STOPPING | RECONNECTING | FAILED | STOPPED
const STATE_ACTIVE: &str = "ACTIVE";
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";

/// The states a stopped composition passes through, in order: `STOPPING` until one
/// [`STOP_SPAN`] has elapsed, `STOPPED` until a second one has, and then no state at all
/// — the resource is deleted, so the read is a 404.
const STOP_PHASES: [Option<&str>; 3] = [Some(STATE_STOPPING), Some(STATE_STOPPED), None];

/// Which of [`STOP_PHASES`] a composition stopped `elapsed` ago is in.
fn stop_phase(elapsed: Duration) -> usize {
    if elapsed < STOP_SPAN {
        0
    } else if elapsed < STOP_SPAN * 2 {
        1
    } else {
        2
    }
}

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a stage and answer its ARN.
    async fn stage(&self, case: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an encoder configuration and answer its ARN.
    async fn encoder_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": "enc" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["encoderConfiguration"], "arn")
    }

    /// Create a storage configuration and answer its ARN.
    async fn storage_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": "composition-recordings" } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["storageConfiguration"], "arn")
    }

    /// `StartComposition`, asserting the modelled success, and answering with the
    /// `composition` the service reported.
    async fn start(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("StartComposition", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["composition"].clone()
    }

    /// `GetComposition` without asserting the read succeeded — the stopped composition is
    /// read through its whole life here, and the last read of it is a 404.
    async fn read(&self, arn: &str) -> (u16, Value) {
        self.call("GetComposition", &json!({ "arn": arn })).await
    }

    /// `GetComposition`, asserting the modelled success.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.read(arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetComposition {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["composition"].clone()
    }

    /// `StopComposition`, asserting the modelled empty success, and answering with the
    /// instant the stop was made.
    async fn stop(&self, case: &str, arn: &str) -> Instant {
        let made = Instant::now();
        let (status, response) = self.call("StopComposition", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopComposition {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        assert!(
            response.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: StopCompositionResponse declares no members, so the answer \
             must carry none, got {response}"
        );
        made
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The destinations of a composition, as the wire reports them.
fn destinations(composition: &Value) -> Vec<Value> {
    composition["destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Assert a read issued between `before` and `after` of the stop found the composition in
/// one of the states that window admits — one state where the read cannot have straddled
/// a span boundary, and either side of it where it can.
fn assert_stop_phase(case: &str, what: &str, window: (Duration, Duration), read: (u16, &Value)) {
    let (before, after) = window;
    let (status, response) = read;
    let observed: Option<String> = (status == 200)
        .then(|| member(&response["composition"], "state"))
        .filter(|state| !state.is_empty());
    let admissible = &STOP_PHASES[stop_phase(before)..=stop_phase(after)];
    assert!(
        admissible
            .iter()
            .any(|state| state.map(str::to_string) == observed),
        "case {case:?}: {what}, read between {before:?} and {after:?} after the stop, \
         must be one of {admissible:?} (and reported exactly when that state is Some), \
         got HTTP {status} {response}"
    );
}

/// The destination lists this suite stops a composition over: the rows differ in how many
/// destinations the composition has and of which kind, which is what the rule generalises
/// over — every destination follows the composition it belongs to, whatever it publishes
/// to. `destinations` is `@length(1..2)`, so both published bounds appear.
fn destination_rows(encoder: &str, storage: &str) -> Vec<(&'static str, Value)> {
    let channel = arn("channel", "live-channel");
    let s3 = json!({
        "s3": {
            "storageConfigurationArn": storage,
            "encoderConfigurationArns": [encoder],
        },
    });
    vec![
        (
            "one channel destination",
            json!([{ "channel": { "channelArn": channel } }]),
        ),
        ("one S3 recording destination", json!([s3])),
        (
            "both destinations the model allows at once",
            json!([{ "channel": { "channelArn": channel } }, s3]),
        ),
    ]
}

/// A stop ends the composition it names: `STOPPING` with an end time straight away, then
/// `STOPPED`, then gone — each step owed by the clock, and no other composition moved.
#[tokio::test]
async fn a_stop_walks_the_composition_it_names_through_the_states_the_clock_owes() {
    let service = RealTime::new();
    let stage = service.stage("stop").await;
    let encoder = service.encoder_configuration("stop").await;
    let storage = service.storage_configuration("stop").await;
    let channel = arn("channel", "live-channel");

    for (case, destination_list) in destination_rows(&encoder, &storage) {
        // A second composition of the same stage, which this stop must leave composing.
        let bystander = member(
            &service
                .start(
                    case,
                    &json!({
                        "stageArn": stage,
                        "destinations": [{ "channel": { "channelArn": channel } }],
                    }),
                )
                .await,
            "arn",
        );
        let started = service
            .start(
                case,
                &json!({ "stageArn": stage, "destinations": destination_list }),
            )
            .await;
        let composition_arn = member(&started, "arn");

        let stopped = service.stop(case, &composition_arn).await;

        // Straight after the stop, and inside the span: stopping, with an end time, and
        // every destination with it. The span is not zero, so this state is observable.
        tokio::time::sleep(WITHIN_THE_STOP_SPAN).await;
        let before = stopped.elapsed();
        let (status, response) = service.read(&composition_arn).await;
        let window = (before, stopped.elapsed());
        assert_stop_phase(
            case,
            "the stopped composition",
            window,
            (status, &response),
        );
        let stopping = response["composition"].clone();
        assert!(
            stopping.get("endTime").is_some(),
            "case {case:?}: a stopped composition must report when its broadcast ended, \
             got {stopping}"
        );
        assert_eq!(
            stopping["startTime"], started["startTime"],
            "case {case:?}: the stop must not move startTime, got {} against {}",
            stopping["startTime"], started["startTime"]
        );
        assert_eq!(
            destinations(&stopping).len(),
            destinations(&started).len(),
            "case {case:?}: the stop must not change which destinations the composition \
             has, got {stopping}"
        );
        for (index, destination) in destinations(&stopping).iter().enumerate() {
            assert!(
                destination.get("endTime").is_some(),
                "case {case:?}: destination {index} of a stopped composition must report \
                 when its broadcast ended, got {destination}"
            );
            let state = member(destination, "state");
            assert!(
                [STATE_STOPPING, STATE_STOPPED].contains(&state.as_str()),
                "case {case:?}: destination {index} must follow the composition it \
                 belongs to, got {destination}"
            );
        }

        // One span later the stop has finished, and the destinations with it.
        tokio::time::sleep(STOP_SPAN + SLACK).await;
        let before = stopped.elapsed();
        let (status, response) = service.read(&composition_arn).await;
        let window = (before, stopped.elapsed());
        assert_stop_phase(
            case,
            "the composition one span after the stop",
            window,
            (status, &response),
        );
        if status == 200 {
            for (index, destination) in destinations(&response["composition"]).iter().enumerate() {
                assert_eq!(
                    member(destination, "state"),
                    STATE_STOPPED,
                    "case {case:?}: destination {index} must settle with its composition, \
                     got {destination}"
                );
            }
        }

        // One span after that the ephemeral resource is gone: the model says the
        // composition "stops and the resource is deleted", so the read is the modelled
        // 404 rather than an empty composition.
        tokio::time::sleep(STOP_SPAN + SLACK).await;
        let (status, response) = service.read(&composition_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a composition whose stop has finished is deleted, so the read \
             must answer HTTP 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found code, got {response}"
        );
        assert!(
            response.get("composition").is_none(),
            "case {case:?}: the refusal must report no composition, got {response}"
        );

        // Nothing else moved: the composition nobody stopped is still composing, and the
        // stage and the configurations it refers to are untouched.
        let left = service.get(case, &bystander).await;
        assert_eq!(
            member(&left, "state"),
            STATE_ACTIVE,
            "case {case:?}: the composition nobody stopped must still be composing, \
             got {left}"
        );
        assert!(
            left.get("endTime").is_none(),
            "case {case:?}: the composition nobody stopped must carry no end time, \
             got {left}"
        );
        for (operation, resource) in [
            ("GetStage", stage.clone()),
            ("GetEncoderConfiguration", encoder.clone()),
            ("GetStorageConfiguration", storage.clone()),
        ] {
            let (status, response) = service.call(operation, &json!({ "arn": resource })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} on {resource} must still answer HTTP 200 \
                 after a stop, got {status} {response}"
            );
        }
    }
}

/// A stop this service cannot perform is refused with the modelled error for it, and
/// changes nothing.
#[tokio::test]
async fn a_stop_is_refused_when_the_composition_cannot_be_stopped() {
    let service = RealTime::new();
    let stage = service.stage("refusals").await;
    let channel = arn("channel", "live-channel");

    // Each row is a different way of naming no composition this service could stop, with
    // the status and modelled code it is answered with. A well-formed ARN this store
    // holds nothing for is "not found"; a value outside the member's modelled
    // `@pattern`/`@length` is "malformed", which is a different answer.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a well-formed composition ARN this store holds nothing for",
            json!({ "arn": arn("composition", "cp000000") }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN naming another resource type of this service",
            json!({ "arn": arn("stage", "st000000") }),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another partition",
            json!({ "arn": "arn:aws-cn:ivs:us-east-1:000000000000:composition/cp000000" }),
            400,
            "ValidationException",
        ),
        (
            "not an ARN at all",
            json!({ "arn": "composition-1" }),
            400,
            "ValidationException",
        ),
        (
            "an empty ARN, which the modelled length minimum refuses",
            json!({ "arn": "" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, response) = service.call("StopComposition", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: StopComposition {request} must answer HTTP {expected_status}, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected the wire error code {expected_code}, got {response}"
        );
        assert!(
            !member(&response, "message").is_empty(),
            "case {case:?}: the refusal must state why, got {response}"
        );
    }

    // A composition is stopped once: every later stop is the modelled conflict, whether
    // the composition is still stopping or has finished stopping.
    let composition_arn = member(
        &service
            .start(
                "a second stop",
                &json!({
                    "stageArn": stage,
                    "destinations": [{ "channel": { "channelArn": channel } }],
                }),
            )
            .await,
        "arn",
    );
    let stopped = service.stop("a second stop", &composition_arn).await;
    for case in [
        "while the stop is still running",
        "once the stop has finished",
    ] {
        // The call is bracketed the way every clock assertion in this suite is: a call
        // that straddles the two-span boundary admits either refusal, and one wholly
        // inside a window admits only that window's.
        let before = stopped.elapsed();
        let (status, response) = service
            .call("StopComposition", &json!({ "arn": composition_arn }))
            .await;
        let after = stopped.elapsed();
        // Inside two spans the composition is still there to be refused as a conflict;
        // past them the clock has reclaimed the ephemeral resource and the honest answer
        // is the not-found one.
        let conflict_admitted = before < STOP_SPAN * 2;
        let not_found_admitted = after >= STOP_SPAN * 2;
        assert!(
            (status == 409 && conflict_admitted) || (status == 404 && not_found_admitted),
            "case {case:?}: a second stop between {before:?} and {after:?} after the \
             first must answer {}, got {status} {response}",
            if conflict_admitted && not_found_admitted {
                "HTTP 409 or, once the resource is reclaimed, HTTP 404"
            } else if conflict_admitted {
                "HTTP 409"
            } else {
                "HTTP 404"
            }
        );
        let expected_code = if status == 409 {
            "ConflictException"
        } else {
            "ResourceNotFoundException"
        };
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: an HTTP {status} refusal must carry {expected_code}, \
             got {response}"
        );
        if status == 409 {
            // The refused stop changed nothing: the composition is still the one the
            // first stop left.
            let before = stopped.elapsed();
            let (status, read) = service.read(&composition_arn).await;
            let window = (before, stopped.elapsed());
            assert_stop_phase(
                case,
                "the composition after a refused second stop",
                window,
                (status, &read),
            );
        }
        tokio::time::sleep(STOP_SPAN + SLACK).await;
    }
}
