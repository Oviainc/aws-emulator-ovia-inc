// SPDX-License-Identifier: CC0-1.0

//! How many destinations of ONE kind a composition may publish to, over the wire.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rule under test, exercised over a SET of inputs rather than on one request:
//! a list whose documented shape admits at most one element per alternative refuses a
//! request that names one alternative twice, however the two elements differ.
//! `Composition$destinations` says it for this list — "A Composition can contain either
//! one destination (`channel` or `s3`) or two (one `channel` and one `s3`)"
//! (`smithy_query(query="shape",shape="Composition")`, member `destinations`) — so:
//!
//! 1. TWO ELEMENTS OF ONE KIND ARE REFUSED with the modelled `ValidationException`
//!    (HTTP 400), whichever kind repeats and whichever member makes the two elements
//!    distinct, and the refused request starts no composition.
//! 2. THE SHAPES THE DOCUMENTATION ADMITS STILL START: one `channel`, one `s3`, and one
//!    of each in either order, each read back by `GetComposition` exactly as started.
//! 3. A REFUSED START CONSUMES NOTHING: the `@idempotencyToken` it carried is still free
//!    for a later request, and the resources its destinations named are still deletable.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every test in this suite signs for.
const REGION: &str = "us-east-1";

/// The operation under test, as the refusals name it.
const OPERATION: &str = "StartComposition";

/// The list member the rule bounds, as the model names it.
const DESTINATIONS: &str = "destinations";

// The two members of `DestinationConfiguration` that select WHERE one destination
// publishes, from `smithy_query(query="shape",shape="DestinationConfiguration")`:
// `channel` ("An IVS channel to be used for broadcasting") and `s3` ("An S3 storage
// configuration to be used for recording video data").
const CHANNEL: &str = "channel";
const S3: &str = "s3";

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

    /// Create a storage configuration recording into `bucket` and answer its ARN.
    async fn storage_configuration(&self, case: &str, bucket: &str) -> String {
        let (status, response) = self
            .call(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": bucket } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {bucket:?} must answer HTTP 200, \
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

    /// `GetComposition`, asserting the modelled success, and answering with the
    /// `composition` the service reported.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetComposition", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetComposition {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["composition"].clone()
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
    composition[DESTINATIONS]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The resources the requests of this suite refer to, resolved against this store: the
/// stage being composited, two encoder configurations and two storage configurations, so
/// two destinations of one kind can differ in what they name.
struct Referenced {
    stage_arn: String,
    encoder_arn: String,
    other_encoder_arn: String,
    storage_arn: String,
    other_storage_arn: String,
}

impl Referenced {
    async fn resolve(service: &RealTime, case: &str) -> Self {
        Self {
            stage_arn: service.stage(case).await,
            encoder_arn: service.encoder_configuration(case, "primary").await,
            other_encoder_arn: service.encoder_configuration(case, "secondary").await,
            storage_arn: service
                .storage_configuration(case, "composition-recordings")
                .await,
            other_storage_arn: service
                .storage_configuration(case, "composition-archive")
                .await,
        }
    }

    /// One `s3` destination recording to `storage` through the primary encoder
    /// configuration.
    fn s3_to(&self, storage: &str) -> Value {
        json!({
            "s3": {
                "storageConfigurationArn": storage,
                "encoderConfigurationArns": [self.encoder_arn],
            },
        })
    }
}

/// The `StartComposition` request for `destinations` against the stage of `referenced`.
fn against(referenced: &Referenced, destinations: Value) -> Value {
    json!({ "stageArn": referenced.stage_arn, "destinations": destinations })
}

// ---------------------------------------------------------------------------
// Two destinations of one kind
// ---------------------------------------------------------------------------

/// A request whose two destinations select the SAME member is refused with the modelled
/// `ValidationException`, whichever member repeats and whatever makes the two elements
/// distinct — and the refused request starts no composition.
#[tokio::test]
async fn two_destinations_of_one_kind_are_refused() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "one kind twice").await;
    let channel = arn("channel", "live");
    let other_channel = arn("channel", "backup");

    // (case, the two destinations, the member the request named twice; `None` where a
    // modelled constraint of the list refuses the request before the rule is reached)
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "two channel destinations naming two different channels",
            json!([
                { "channel": { "channelArn": channel } },
                { "channel": { "channelArn": other_channel } },
            ]),
            Some(CHANNEL),
        ),
        (
            "two channel destinations naming one channel, told apart by their names",
            json!([
                { "name": "primary", "channel": { "channelArn": channel } },
                { "name": "backup", "channel": { "channelArn": channel } },
            ]),
            Some(CHANNEL),
        ),
        (
            "two channel destinations differing only in the encoder configuration",
            json!([
                { "channel": {
                    "channelArn": channel,
                    "encoderConfigurationArn": referenced.encoder_arn,
                }},
                { "channel": {
                    "channelArn": channel,
                    "encoderConfigurationArn": referenced.other_encoder_arn,
                }},
            ]),
            Some(CHANNEL),
        ),
        (
            "two S3 destinations naming two different storage configurations",
            json!([
                referenced.s3_to(&referenced.storage_arn),
                referenced.s3_to(&referenced.other_storage_arn),
            ]),
            Some(S3),
        ),
        (
            "two S3 destinations naming one storage configuration, told apart by \
             their names",
            json!([
                { "name": "archive", "s3": {
                    "storageConfigurationArn": referenced.storage_arn,
                    "encoderConfigurationArns": [referenced.encoder_arn],
                }},
                { "name": "mirror", "s3": {
                    "storageConfigurationArn": referenced.storage_arn,
                    "encoderConfigurationArns": [referenced.encoder_arn],
                }},
            ]),
            Some(S3),
        ),
        (
            "two S3 destinations differing only in the recording configuration",
            json!([
                { "s3": {
                    "storageConfigurationArn": referenced.storage_arn,
                    "encoderConfigurationArns": [referenced.encoder_arn],
                    "recordingConfiguration": {
                        "format": "HLS",
                        "hlsConfiguration": { "targetSegmentDurationSeconds": 2 },
                    },
                }},
                { "s3": {
                    "storageConfigurationArn": referenced.storage_arn,
                    "encoderConfigurationArns": [referenced.encoder_arn],
                    "recordingConfiguration": {
                        "format": "HLS",
                        "hlsConfiguration": { "targetSegmentDurationSeconds": 6 },
                    },
                }},
            ]),
            Some(S3),
        ),
        (
            // `destinations` is `@uniqueItems`, so two IDENTICAL elements are refused by
            // the modelled constraint rather than by the rule above — the same status and
            // error code, stated by the constraint walk.
            "two identical channel destinations",
            json!([
                { "channel": { "channelArn": channel } },
                { "channel": { "channelArn": channel } },
            ]),
            None,
        ),
    ];

    // Every refused request carries an `@idempotencyToken`, so the last assertion of this
    // test can show the refusal consumed none of them.
    let mut refused_tokens: Vec<(&str, String)> = Vec::new();
    for (index, (case, destination_list, repeated)) in cases.into_iter().enumerate() {
        let token = format!("refused-{index}");
        let mut request = against(&referenced, destination_list);
        request["idempotencyToken"] = json!(token);

        let (status, response) = service.call("StartComposition", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: StartComposition {request} must answer HTTP 400, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the wire error code ValidationException, got {response}"
        );
        match repeated {
            Some(repeated) => {
                let expected = state::at_most_one_element_per_choice_message(
                    OPERATION,
                    DESTINATIONS,
                    repeated,
                    2,
                );
                assert_eq!(
                    member(&response, "message"),
                    expected,
                    "case {case:?}: expected the refusal {expected:?}, got {response}"
                );
            }
            None => assert!(
                !member(&response, "message").is_empty(),
                "case {case:?}: the refusal must state why, got {response}"
            ),
        }
        refused_tokens.push((case, token));
    }

    // A refused start consumed nothing. The token it carried is still free: offering it
    // with a request the rule admits starts a composition rather than replaying one or
    // colliding with one.
    let mut started: Vec<String> = Vec::new();
    for (case, token) in &refused_tokens {
        let mut request = against(
            &referenced,
            json!([{ "channel": { "channelArn": channel } }]),
        );
        request["idempotencyToken"] = json!(token);
        let composition = service.start(case, &request).await;
        let composition_arn = member(&composition, "arn");
        assert!(
            !composition_arn.is_empty() && !started.contains(&composition_arn),
            "case {case:?}: the token a refused start carried must still be free, \
             got {composition_arn:?} beside {started:?}"
        );
        assert_eq!(
            destinations(&composition).len(),
            1,
            "case {case:?}: the admitted request must start a one-destination \
             composition, got {composition}"
        );
        started.push(composition_arn);
    }
}

/// Deleting a resource only a REFUSED start's destinations named succeeds: the refusal
/// left no composition using it.
#[tokio::test]
async fn a_refused_start_leaves_its_resources_deletable() {
    let case = "two channel destinations naming one encoder configuration each";
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, case).await;
    let channel = arn("channel", "live");

    let request = against(
        &referenced,
        json!([
            { "channel": {
                "channelArn": channel,
                "encoderConfigurationArn": referenced.encoder_arn,
            }},
            { "channel": {
                "channelArn": arn("channel", "backup"),
                "encoderConfigurationArn": referenced.other_encoder_arn,
            }},
        ]),
    );
    let (status, response) = service.call("StartComposition", &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: StartComposition {request} must answer HTTP 400, \
         got {status} {response}"
    );

    // `DeleteEncoderConfiguration` is the modelled `ConflictException` while a
    // composition of this store refers to the configuration, so its success here is the
    // store's own answer that the refused start persisted nothing.
    for configuration in [&referenced.encoder_arn, &referenced.other_encoder_arn] {
        let (status, response) = service
            .call("DeleteEncoderConfiguration", &json!({ "arn": configuration }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEncoderConfiguration {configuration} must succeed after \
             a refused start, got {status} {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// The shapes the documentation admits
// ---------------------------------------------------------------------------

/// The destination lists the documented shape admits — one `channel`, one `s3`, and one of
/// each in either order — still start, and `GetComposition` reports each of them exactly
/// as started.
#[tokio::test]
async fn the_documented_destination_shapes_still_start() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "admitted shapes").await;
    let channel = arn("channel", "live");

    let cases: Vec<(&str, Value)> = vec![
        (
            "one channel destination",
            json!([{ "channel": { "channelArn": channel } }]),
        ),
        (
            "one S3 destination",
            json!([referenced.s3_to(&referenced.storage_arn)]),
        ),
        (
            "one channel and one S3 destination, the channel first",
            json!([
                { "channel": { "channelArn": channel } },
                referenced.s3_to(&referenced.storage_arn),
            ]),
        ),
        (
            "one channel and one S3 destination, the S3 one first",
            json!([
                referenced.s3_to(&referenced.storage_arn),
                { "channel": { "channelArn": channel } },
            ]),
        ),
        (
            "one named channel and one named S3 destination",
            json!([
                { "name": "broadcast", "channel": {
                    "channelArn": channel,
                    "encoderConfigurationArn": referenced.other_encoder_arn,
                }},
                { "name": "archive", "s3": {
                    "storageConfigurationArn": referenced.other_storage_arn,
                    "encoderConfigurationArns": [referenced.encoder_arn],
                }},
            ]),
        ),
    ];

    for (case, destination_list) in cases {
        let request = against(&referenced, destination_list.clone());
        let started = service.start(case, &request).await;

        let requested = destination_list.as_array().cloned().unwrap_or_default();
        let reported = destinations(&started);
        assert_eq!(
            reported.len(),
            requested.len(),
            "case {case:?}: one destination must be reported per requested destination, \
             got {started}"
        );
        for (index, (asked, answered)) in requested.iter().zip(reported.iter()).enumerate() {
            assert_eq!(
                &answered["configuration"], asked,
                "case {case:?}: destination {index} must report the configuration it was \
                 created from, got {answered}"
            );
        }

        // The composition is PERSISTED, not just answered: a read reports the same ARN,
        // the same stage and the same destination configurations in the same order. Its
        // state is left out, since the clock owes this composition `ACTIVE` and may have
        // made that transition due between the two calls.
        let read = service.get(case, &member(&started, "arn")).await;
        assert_eq!(
            member(&read, "arn"),
            member(&started, "arn"),
            "case {case:?}: GetComposition must report the composition the start answered \
             with, got {read}"
        );
        assert_eq!(
            member(&read, "stageArn"),
            referenced.stage_arn,
            "case {case:?}: the composition must name the stage it composites, got {read}"
        );
        let read_destinations = destinations(&read);
        assert_eq!(
            read_destinations.len(),
            requested.len(),
            "case {case:?}: the read must report one destination per requested \
             destination, got {read}"
        );
        for (index, (asked, answered)) in requested.iter().zip(read_destinations.iter()).enumerate()
        {
            assert_eq!(
                &answered["configuration"], asked,
                "case {case:?}: destination {index} must read back as the configuration it \
                 was created from, got {answered}"
            );
            assert_eq!(
                member(answered, "id"),
                member(&reported[index], "id"),
                "case {case:?}: destination {index} must keep the id the start minted, \
                 got {answered}"
            );
        }
    }
}
