// SPDX-License-Identifier: CC0-1.0

//! Deleting a resource a composition is using, over the wire.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call. The rest of the encoder-configuration surface is covered by
//! `tests/all/encoder_configurations.rs`; this suite is about the one refusal that
//! depends on what the compositions of this store refer to.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A DELETE OF A RESOURCE A COMPOSITION IS USING IS REFUSED. While a composition this
//!    store holds refers to an encoder configuration — through any destination member
//!    that names one — `DeleteEncoderConfiguration` answers the modelled
//!    `ConflictException` (HTTP 409) and the configuration stays exactly as
//!    `GetEncoderConfiguration` reports it.
//! 2. THE REFUSAL LASTS AS LONG AS THE COMPOSITION DOES, in every state on this side of a
//!    finished stop (`STARTING`, `ACTIVE` and `STOPPING`), and ends when the stop the
//!    clock owes has completed: the delete then succeeds and the ARN names nothing.
//! 3. A COMPOSITION THAT DOES NOT REFER TO THE CONFIGURATION HOLDS NOTHING BACK: the
//!    delete succeeds while compositions of this store refer to OTHER configurations, or
//!    to none, and those compositions and configurations are left untouched.

use std::sync::Arc;
use std::time::Duration;

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

// The `CompositionState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";
const STATE_STOPPING: &str = "STOPPING";

/// Added to every span this suite waits out, so the transition the clock owes has
/// certainly fallen due whatever scheduling delay the test host adds.
const SLACK: Duration = Duration::from_millis(400);

/// Shorter than either span, so a read or a delete made straight after a call still sees
/// the transient state that call put the composition in.
const WITHIN_A_SPAN: Duration = Duration::from_millis(1);

/// Long enough that the `STARTING` -> `ACTIVE` transition the emulator owes has fallen
/// due, read from the crate so this suite cannot drift from the span it enforces.
fn past_the_activation_span() -> Duration {
    state::COMPOSITION_ACTIVATION_SPAN + SLACK
}

/// Long enough that the whole stop the emulator owes has finished — `STOPPING` ->
/// `STOPPED` and then the deletion of the ephemeral resource, one span each.
fn past_the_whole_stop() -> Duration {
    state::COMPOSITION_STOP_SPAN * 2 + SLACK
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
        let (status, response) = self.call("CreateStage", &json!({ "name": "composited" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an encoder configuration named `name` and answer its ARN.
    async fn encoder_configuration(&self, case: &str, name: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration {name:?} must answer HTTP 200, \
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

    /// `StopComposition`, asserting the modelled empty success.
    async fn stop(&self, case: &str, arn: &str) {
        let (status, response) = self.call("StopComposition", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopComposition {arn} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// `GetComposition` as a client makes it, without asserting the answer.
    async fn read_composition(&self, arn: &str) -> (u16, Value) {
        self.call("GetComposition", &json!({ "arn": arn })).await
    }

    /// `GetEncoderConfiguration` as a client makes it, without asserting the answer: this
    /// suite reads a configuration both while it is in use and after it is gone.
    async fn read_encoder_configuration(&self, arn: &str) -> (u16, Value) {
        self.call("GetEncoderConfiguration", &json!({ "arn": arn }))
            .await
    }

    /// `DeleteEncoderConfiguration` as a client makes it, without asserting the answer.
    async fn delete_encoder_configuration(&self, arn: &str) -> (u16, Value) {
        self.call("DeleteEncoderConfiguration", &json!({ "arn": arn }))
            .await
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

/// The resources a composition of this suite is started against: the stage it composites,
/// the encoder configuration whose delete is under test, a second encoder configuration no
/// test deletes, and the storage configuration an S3 destination records to.
struct Referenced {
    stage_arn: String,
    target_arn: String,
    other_arn: String,
    storage_configuration_arn: String,
}

impl Referenced {
    async fn resolve(service: &RealTime, case: &str) -> Self {
        Self {
            stage_arn: service.stage(case).await,
            target_arn: service.encoder_configuration(case, "target").await,
            other_arn: service.encoder_configuration(case, "other").await,
            storage_configuration_arn: service.storage_configuration(case).await,
        }
    }
}

/// How one table row builds the destinations its composition is started with, out of the
/// resources that row was given.
type Destinations = fn(&Referenced) -> Value;

/// [`Destinations`] for a row that may start no composition at all.
type OptionalDestinations = fn(&Referenced) -> Option<Value>;

/// The `StartComposition` request for `destinations` against the stage of `referenced`.
fn against(referenced: &Referenced, destinations: Value) -> Value {
    json!({ "stageArn": referenced.stage_arn, "destinations": destinations })
}

/// Assert that deleting `arn` is refused as the modelled conflict and that the
/// configuration is still readable afterwards.
async fn assert_delete_refused(service: &RealTime, case: &str, arn: &str, expected_state: &str) {
    let before = service.read_encoder_configuration(arn).await;
    let (status, response) = service.delete_encoder_configuration(arn).await;
    assert_eq!(
        status, 409,
        "case {case:?}: deleting {arn} while a {expected_state} composition uses it must \
         answer HTTP 409, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "case {case:?}: the refusal must be the modelled ConflictException, got {response}"
    );
    // The refusal names the resource the request named and nothing else: not the
    // composition holding it, and not that composition's state.
    assert_eq!(
        member(&response, "message"),
        format!("Resource: {arn} is attached to a composition"),
        "case {case:?}: the refusal must state that {arn} is attached to a composition, \
         got {response}"
    );
    let after = service.read_encoder_configuration(arn).await;
    assert_eq!(
        after.0, 200,
        "case {case:?}: a refused delete must leave {arn} readable, got {} {}",
        after.0, after.1
    );
    assert_eq!(
        after, before,
        "case {case:?}: a refused delete must change nothing about {arn}, got {:?} against \
         {:?}",
        after, before
    );
}

/// RULE 1 and RULE 2: while a composition refers to an encoder configuration the delete
/// is the modelled conflict, in every state on this side of a finished stop, and the
/// configuration stays readable; once the stop the clock owes has completed, the same
/// delete succeeds and the ARN names nothing.
///
/// One row per WAY a composition can refer to the configuration — the destination kind
/// that names it, how many destinations the composition has, which of them carries the
/// reference and whether they carry names — since that is what the rule generalises over.
/// A composition carries at most one destination of each kind
/// (`smithy_query(query="shape",shape="Composition")`, member `destinations`), so a
/// two-destination row is one `channel` and one `s3`.
#[tokio::test]
async fn a_configuration_a_composition_uses_cannot_be_deleted_until_that_composition_stops() {
    let cases: [(&str, Destinations); 5] = [
        ("a channel destination naming it", |referenced| {
            json!([{ "channel": {
                "channelArn": arn("channel", "live"),
                "encoderConfigurationArn": referenced.target_arn,
            } }])
        }),
        ("an S3 destination naming it", |referenced| {
            json!([{ "s3": {
                "storageConfigurationArn": referenced.storage_configuration_arn,
                "encoderConfigurationArns": [referenced.target_arn],
            } }])
        }),
        (
            "two destinations, only the channel one naming it",
            |referenced| {
                json!([
                    { "channel": {
                        "channelArn": arn("channel", "live"),
                        "encoderConfigurationArn": referenced.target_arn,
                    } },
                    { "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.other_arn],
                    } },
                ])
            },
        ),
        (
            "two destinations, only the S3 one naming it",
            |referenced| {
                json!([
                    { "channel": {
                        "channelArn": arn("channel", "live"),
                        "encoderConfigurationArn": referenced.other_arn,
                    } },
                    { "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.target_arn],
                    } },
                ])
            },
        ),
        (
            "a named channel destination naming it, beside a named S3 destination naming \
             another",
            |referenced| {
                json!([
                    { "name": "broadcast", "channel": {
                        "channelArn": arn("channel", "live"),
                        "encoderConfigurationArn": referenced.target_arn,
                    } },
                    { "name": "archive", "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.other_arn],
                    } },
                ])
            },
        ),
    ];

    for (case, destinations) in cases {
        // One emulator per row, so each row decides against a store holding only its own
        // composition.
        let service = RealTime::new();
        let referenced = Referenced::resolve(&service, case).await;
        let started = service
            .start(case, &against(&referenced, destinations(&referenced)))
            .await;
        let composition_arn = member(&started, "arn");

        // STARTING: the composition exists from the moment the start returns, so it is
        // already using what its destinations name.
        tokio::time::sleep(WITHIN_A_SPAN).await;
        assert_delete_refused(&service, case, &referenced.target_arn, STATE_STARTING).await;

        // ACTIVE: the clock has made the transition due; the refusal is unchanged.
        tokio::time::sleep(past_the_activation_span()).await;
        let composing = service.read_composition(&composition_arn).await;
        assert_eq!(
            member(&composing.1["composition"], "state"),
            STATE_ACTIVE,
            "case {case:?}: the composition must be {STATE_ACTIVE} once the clock has made \
             the transition due, got {} {}",
            composing.0,
            composing.1
        );
        assert_delete_refused(&service, case, &referenced.target_arn, STATE_ACTIVE).await;

        // STOPPING: the stop has begun but not completed, and the model tells the caller
        // to WAIT for it to complete — so the delete is still refused.
        service.stop(case, &composition_arn).await;
        tokio::time::sleep(WITHIN_A_SPAN).await;
        let stopping = service.read_composition(&composition_arn).await;
        assert_eq!(
            member(&stopping.1["composition"], "state"),
            STATE_STOPPING,
            "case {case:?}: a read inside the stop span must still report \
             {STATE_STOPPING}, got {} {}",
            stopping.0,
            stopping.1
        );
        assert_delete_refused(&service, case, &referenced.target_arn, STATE_STOPPING).await;

        // The stop has completed and the ephemeral resource is gone: nothing uses the
        // configuration any more, so the delete succeeds and the ARN names nothing.
        tokio::time::sleep(past_the_whole_stop()).await;
        let (status, response) = service
            .delete_encoder_configuration(&referenced.target_arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the composition's stop has completed the delete must \
             answer HTTP 200, got {status} {response}"
        );
        let (read_status, read) = service
            .read_encoder_configuration(&referenced.target_arn)
            .await;
        assert_eq!(
            read_status, 404,
            "case {case:?}: the deleted configuration must name nothing, \
             got {read_status} {read}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: the read of a deleted configuration must be the modelled \
             ResourceNotFoundException, got {read}"
        );
    }
}

/// RULE 3: a composition that does not refer to the configuration holds nothing back —
/// the delete succeeds, and both the composition and the configurations it DOES refer to
/// are left exactly as they were.
///
/// One row per way a composition can fail to refer to it: a different configuration named
/// by each destination kind, no encoder configuration named at all, two destinations
/// naming neither, and no composition in the store whatsoever.
#[tokio::test]
async fn a_configuration_no_composition_uses_is_deleted() {
    let cases: [(&str, OptionalDestinations); 5] = [
        ("no composition at all", |_| None),
        (
            "a channel destination naming another configuration",
            |referenced| {
                Some(json!([{ "channel": {
                    "channelArn": arn("channel", "live"),
                    "encoderConfigurationArn": referenced.other_arn,
                } }]))
            },
        ),
        (
            "an S3 destination naming another configuration",
            |referenced| {
                Some(json!([{ "s3": {
                    "storageConfigurationArn": referenced.storage_configuration_arn,
                    "encoderConfigurationArns": [referenced.other_arn],
                } }]))
            },
        ),
        (
            "a channel destination naming no encoder configuration",
            |_| Some(json!([{ "channel": { "channelArn": arn("channel", "live") } }])),
        ),
        (
            "two destinations, neither naming it",
            |referenced| {
                Some(json!([
                    { "channel": { "channelArn": arn("channel", "live") } },
                    { "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.other_arn],
                    } },
                ]))
            },
        ),
    ];

    for (case, destinations) in cases {
        let service = RealTime::new();
        let referenced = Referenced::resolve(&service, case).await;
        let composition_arn = match destinations(&referenced) {
            Some(destinations) => {
                let started = service.start(case, &against(&referenced, destinations)).await;
                Some(member(&started, "arn"))
            }
            None => None,
        };

        let (status, response) = service
            .delete_encoder_configuration(&referenced.target_arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: no composition uses the configuration, so the delete must \
             answer HTTP 200, got {status} {response}"
        );
        assert!(
            response.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteEncoderConfigurationResponse declares no members, so the \
             answer must carry none, got {response}"
        );

        let (read_status, read) = service
            .read_encoder_configuration(&referenced.target_arn)
            .await;
        assert_eq!(
            read_status, 404,
            "case {case:?}: the deleted configuration must name nothing, \
             got {read_status} {read}"
        );

        // Nothing else moved: the configuration the composition refers to is still there,
        // and the composition still reports the destinations it was started with.
        let (other_status, other) = service
            .read_encoder_configuration(&referenced.other_arn)
            .await;
        assert_eq!(
            other_status, 200,
            "case {case:?}: the configuration the delete did not name must be unchanged, \
             got {other_status} {other}"
        );
        if let Some(composition_arn) = composition_arn {
            let (composition_status, composition) =
                service.read_composition(&composition_arn).await;
            assert_eq!(
                composition_status, 200,
                "case {case:?}: the composition must still be reported, \
                 got {composition_status} {composition}"
            );
            assert!(
                !member(&composition["composition"], "state").is_empty(),
                "case {case:?}: the reported composition must carry its state, \
                 got {composition}"
            );
        }
    }
}
