// SPDX-License-Identifier: CC0-1.0

//! `DeleteStorageConfiguration` over the wire.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call. The rest of the storage-configuration surface is covered by
//! `tests/all/storage_configurations.rs`; this suite is about the delete.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A DELETE REMOVES PERSISTED STATE. "Deletes the storage configuration for the
//!    specified ARN": the delete answers the modelled empty success (HTTP 200), after
//!    which the ARN names nothing — `GetStorageConfiguration` answers the modelled
//!    `ResourceNotFoundException` (HTTP 404) and `ListStorageConfigurations` no longer
//!    reports it — and every OTHER configuration of the account is left exactly as it was.
//! 2. A DELETE OF AN ARN THIS ACCOUNT HOLDS NO CONFIGURATION FOR IS REFUSED with the
//!    modelled `ResourceNotFoundException` (HTTP 404): one never created, one already
//!    deleted (so a delete is not idempotently accepted a second time), and one of
//!    ANOTHER region, which stays readable through the region that holds it.
//! 3. A CONFIGURATION A COMPOSITION USES CANNOT BE DELETED. "If you try to delete a
//!    storage configuration that is used by a Composition, you will get an error (409
//!    ConflictException). To avoid this, for all Compositions that reference the storage
//!    configuration, first use StopComposition and wait for it to complete": while such a
//!    composition exists the delete is the modelled `ConflictException` and the
//!    configuration stays exactly as a read reports it; once the stop has completed the
//!    same delete succeeds.
//! 4. A COMPOSITION THAT RECORDS SOMEWHERE ELSE, AND A STAGE THAT MERELY REFERENCES THE
//!    CONFIGURATION, HOLD NOTHING BACK: a Composition is what the model names as the
//!    blocker, so the delete succeeds in every other case.

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

/// The region every test that does not vary it signs for.
const REGION: &str = "us-east-1";

/// A second region, for the rule that a configuration belongs to the region it was
/// created through.
const OTHER_REGION: &str = "eu-west-1";

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
    /// scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{region}.amazonaws.com/{operation}"
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

    /// [`Self::call_in`] against [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Create a storage configuration in `region` from `body` and answer its ARN.
    async fn storage_configuration_in(&self, case: &str, region: &str, body: &Value) -> String {
        let (status, response) = self
            .call_in(region, "CreateStorageConfiguration", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {body} in {region} must answer \
             HTTP 200, got {status} {response}"
        );
        member(&response["storageConfiguration"], "arn")
    }

    /// Create a storage configuration in [`REGION`] recording to `bucket`.
    async fn storage_configuration(&self, case: &str, bucket: &str) -> String {
        self.storage_configuration_in(case, REGION, &json!({ "s3": { "bucketName": bucket } }))
            .await
    }

    /// Create an encoder configuration and answer its ARN: an S3 composition destination
    /// needs one beside the storage configuration it records to.
    async fn encoder_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": "encoder" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["encoderConfiguration"], "arn")
    }

    /// Create a stage from `body` and answer its ARN.
    async fn stage(&self, case: &str, body: &Value) -> String {
        let (status, response) = self.call("CreateStage", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {body} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
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

    /// `GetStorageConfiguration` through `region`'s endpoint, without asserting the
    /// answer: this suite reads a configuration both while it is in use and after it is
    /// gone.
    async fn read_in(&self, region: &str, arn: &str) -> (u16, Value) {
        self.call_in(region, "GetStorageConfiguration", &json!({ "arn": arn }))
            .await
    }

    /// [`Self::read_in`] against [`REGION`].
    async fn read(&self, arn: &str) -> (u16, Value) {
        self.read_in(REGION, arn).await
    }

    /// `DeleteStorageConfiguration` through `region`'s endpoint, without asserting the
    /// answer.
    async fn delete_in(&self, region: &str, arn: &str) -> (u16, Value) {
        self.call_in(region, "DeleteStorageConfiguration", &json!({ "arn": arn }))
            .await
    }

    /// [`Self::delete_in`] against [`REGION`].
    async fn delete(&self, arn: &str) -> (u16, Value) {
        self.delete_in(REGION, arn).await
    }

    /// Every ARN `ListStorageConfigurations` reports for [`REGION`], in page order.
    async fn listed(&self, case: &str) -> Vec<String> {
        let (status, response) = self.call("ListStorageConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListStorageConfigurations must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfigurations"]
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| member(summary, "arn"))
                    .collect()
            })
            .unwrap_or_default()
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

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// Assert that `response` is the modelled failure `status`/`code`.
fn assert_failure(case: &str, what: &str, expected: (u16, &str), actual: &(u16, Value)) {
    let (status, response) = actual;
    assert_eq!(
        *status, expected.0,
        "case {case:?}: {what} must answer HTTP {}, got {status} {response}",
        expected.0
    );
    assert_eq!(
        error_code(response),
        expected.1,
        "case {case:?}: {what} must be the modelled {}, got {response}",
        expected.1
    );
}

/// RULE 1: the delete removes the configuration the request names and nothing else.
///
/// One row per KIND of configuration the create surface admits — whether a name is given,
/// how many tags it carries, and the form of the bucket name (`S3BucketName` is
/// `@length(3..63)` with `@pattern ^[a-z0-9-.]+$`, from
/// `smithy_query(query="shape",shape="S3StorageConfiguration")`, so the published minimum
/// length, the published maximum and both allowed punctuation characters appear below) —
/// since the delete is keyed by the ARN alone and so must not depend on any of them.
#[tokio::test]
async fn a_storage_configuration_this_account_holds_is_deleted() {
    let cases: [(&str, Value); 5] = [
        (
            "the smallest request the model allows",
            json!({ "s3": { "bucketName": "abc" } }),
        ),
        (
            "every optional member supplied",
            json!({
                "s3": { "bucketName": "recordings-bucket" },
                "name": "studio-recordings",
                "tags": { "env": "test", "team": "media" },
            }),
        ),
        (
            "a bucket name at the modelled 63-character maximum, with dots and dashes",
            json!({ "s3": { "bucketName": format!("a.b-{}", "c".repeat(59)) } }),
        ),
        (
            "an explicitly empty name",
            json!({ "s3": { "bucketName": "empty-name-bucket" }, "name": "" }),
        ),
        (
            "an explicitly empty tag map",
            json!({ "s3": { "bucketName": "untagged-bucket" }, "tags": {} }),
        ),
    ];

    for (case, body) in cases {
        // One emulator per row, so each row decides against a store holding only its own
        // configurations.
        let service = RealTime::new();
        let target = service.storage_configuration_in(case, REGION, &body).await;
        let other = service.storage_configuration(case, "left-alone").await;
        let other_before = service.read(&other).await;

        let (status, response) = service.delete(&target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting {target} must answer HTTP 200, got {status} {response}"
        );
        assert!(
            response.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteStorageConfigurationResponse declares no members, so the \
             answer must carry none, got {response}"
        );

        let read = service.read(&target).await;
        assert_failure(
            case,
            &format!("a read of the deleted {target}"),
            (404, "ResourceNotFoundException"),
            &read,
        );

        let listed = service.listed(case).await;
        assert!(
            !listed.contains(&target),
            "case {case:?}: the listing must not report the deleted {target}, got {listed:?}"
        );
        assert!(
            listed.contains(&other),
            "case {case:?}: the listing must still report {other}, got {listed:?}"
        );
        let other_after = service.read(&other).await;
        assert_eq!(
            other_after, other_before,
            "case {case:?}: the delete must change nothing about {other}, got {:?} against \
             {:?}",
            other_after, other_before
        );
    }
}

/// RULE 2: an ARN this account view holds no configuration for is the modelled
/// not-found, and a delete refused that way changes nothing.
///
/// One row per WAY an ARN can name no configuration of the calling region's view: never
/// created, created and already deleted, and created through another region's endpoint.
#[tokio::test]
async fn a_delete_of_an_arn_this_account_holds_no_configuration_for_is_refused() {
    /// How a row produces the ARN it deletes, and the region it deletes through.
    type Target = fn(&str) -> (String, &'static str);

    let cases: [(&str, bool, Target); 3] = [
        (
            "an ARN no create ever minted",
            false,
            |_| (arn(REGION, "storage-configuration", "neverCreated"), REGION),
        ),
        (
            "an ARN whose configuration this request already deleted",
            true,
            |created| (created.to_string(), REGION),
        ),
        (
            "an ARN of a configuration another region's endpoint created",
            false,
            |created| (created.to_string(), OTHER_REGION),
        ),
    ];

    for (case, delete_first, target) in cases {
        let service = RealTime::new();
        let created = service.storage_configuration(case, "a-live-bucket").await;
        let (arn_under_test, region) = target(&created);
        if delete_first {
            let (status, response) = service.delete(&arn_under_test).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first delete of {arn_under_test} must answer HTTP 200, \
                 got {status} {response}"
            );
        }

        let refused = service.delete_in(region, &arn_under_test).await;
        assert_failure(
            case,
            &format!("deleting {arn_under_test} through {region}"),
            (404, "ResourceNotFoundException"),
            &refused,
        );

        // The configuration the row did not delete is still exactly where it was, read
        // through the region that holds it.
        if !delete_first {
            let (read_status, read) = service.read_in(REGION, &created).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: the refused delete must leave {created} readable in \
                 {REGION}, got {read_status} {read}"
            );
        }
    }
}

/// The resources one composition row is started against.
struct Referenced {
    stage_arn: String,
    encoder_configuration_arn: String,
    /// The storage configuration whose delete is under test.
    target_arn: String,
    /// A second storage configuration no test deletes, so a row can record elsewhere.
    other_arn: String,
}

impl Referenced {
    async fn resolve(service: &RealTime, case: &str) -> Self {
        Self {
            stage_arn: service.stage(case, &json!({ "name": "composited" })).await,
            encoder_configuration_arn: service.encoder_configuration(case).await,
            target_arn: service.storage_configuration(case, "target-bucket").await,
            other_arn: service.storage_configuration(case, "other-bucket").await,
        }
    }
}

/// How one table row builds the destinations its composition is started with.
type Destinations = fn(&Referenced) -> Value;

/// [`Destinations`] for a row that may start no composition at all.
type OptionalDestinations = fn(&Referenced) -> Option<Value>;

/// The `StartComposition` request for `destinations` against the stage of `referenced`.
fn against(referenced: &Referenced, destinations: Value) -> Value {
    json!({ "stageArn": referenced.stage_arn, "destinations": destinations })
}

/// Assert that deleting `arn` is refused as the modelled conflict and that the
/// configuration is still readable, unchanged, afterwards.
async fn assert_delete_refused(service: &RealTime, case: &str, arn: &str, expected_state: &str) {
    let before = service.read(arn).await;
    let refused = service.delete(arn).await;
    assert_failure(
        case,
        &format!("deleting {arn} while a {expected_state} composition uses it"),
        (409, "ConflictException"),
        &refused,
    );
    // The refusal names the resource the request named and nothing else — the same
    // sentence `DeleteEncoderConfiguration` states for a resource a composition holds.
    assert_eq!(
        member(&refused.1, "message"),
        format!("Resource: {arn} is attached to a composition"),
        "case {case:?}: the refusal must state that {arn} is attached to a composition, \
         got {:?}",
        refused.1
    );
    let after = service.read(arn).await;
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

/// RULE 3: while a composition records to the storage configuration the delete is the
/// modelled conflict, in every state on this side of a finished stop, and the
/// configuration stays readable; once the stop the model tells the caller to wait for has
/// completed, the same delete succeeds and the ARN names nothing.
///
/// One row per WAY a composition can record to it — how many destinations the composition
/// has, which of them carries the reference and whether they carry names — since that is
/// what the rule generalises over. A composition carries at most one destination of each
/// kind (`smithy_query(query="shape",shape="Composition")`, member `destinations`), so a
/// two-destination row is one `channel` and one `s3`.
#[tokio::test]
async fn a_configuration_a_composition_uses_cannot_be_deleted_until_that_composition_stops() {
    let cases: [(&str, Destinations); 3] = [
        ("one S3 destination recording to it", |referenced| {
            json!([{ "s3": {
                "storageConfigurationArn": referenced.target_arn,
                "encoderConfigurationArns": [referenced.encoder_configuration_arn],
            } }])
        }),
        (
            "a named S3 destination recording to it, beside a channel destination",
            |referenced| {
                json!([
                    { "name": "broadcast", "channel": {
                        "channelArn": arn(REGION, "channel", "live"),
                        "encoderConfigurationArn": referenced.encoder_configuration_arn,
                    } },
                    { "name": "archive", "s3": {
                        "storageConfigurationArn": referenced.target_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                    } },
                ])
            },
        ),
        (
            "a channel destination first, with the S3 one recording to it",
            |referenced| {
                json!([
                    { "channel": { "channelArn": arn(REGION, "channel", "live") } },
                    { "s3": {
                        "storageConfigurationArn": referenced.target_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                    } },
                ])
            },
        ),
    ];

    for (case, destinations) in cases {
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
        let (status, response) = service.delete(&referenced.target_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the composition's stop has completed the delete must \
             answer HTTP 200, got {status} {response}"
        );
        let read = service.read(&referenced.target_arn).await;
        assert_failure(
            case,
            &format!("a read of the deleted {}", referenced.target_arn),
            (404, "ResourceNotFoundException"),
            &read,
        );
    }
}

/// RULE 4: a Composition is the one blocker the model names, so everything else holds the
/// delete back not at all — a composition recording somewhere else, a composition with no
/// S3 destination, no composition whatsoever, and a stage whose
/// `autoParticipantRecordingConfiguration` names the configuration.
#[tokio::test]
async fn a_configuration_no_composition_uses_is_deleted() {
    let cases: [(&str, bool, OptionalDestinations); 4] = [
        ("no composition at all", false, |_| None),
        (
            "an S3 destination recording to another configuration",
            false,
            |referenced| {
                Some(json!([{ "s3": {
                    "storageConfigurationArn": referenced.other_arn,
                    "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                } }]))
            },
        ),
        (
            "a composition with no S3 destination at all",
            false,
            |referenced| {
                Some(json!([{ "channel": {
                    "channelArn": arn(REGION, "channel", "live"),
                    "encoderConfigurationArn": referenced.encoder_configuration_arn,
                } }]))
            },
        ),
        (
            "a stage recording individual participants to it, and no composition",
            true,
            |_| None,
        ),
    ];

    for (case, referencing_stage, destinations) in cases {
        let service = RealTime::new();
        let referenced = Referenced::resolve(&service, case).await;
        let composition_arn = match destinations(&referenced) {
            Some(destinations) => {
                let started = service.start(case, &against(&referenced, destinations)).await;
                Some(member(&started, "arn"))
            }
            None => None,
        };
        let recording_stage = if referencing_stage {
            Some(
                service
                    .stage(
                        case,
                        &json!({
                            "name": "recorded",
                            "autoParticipantRecordingConfiguration": {
                                "storageConfigurationArn": referenced.target_arn,
                            },
                        }),
                    )
                    .await,
            )
        } else {
            None
        };

        let (status, response) = service.delete(&referenced.target_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: no composition uses the configuration, so the delete must \
             answer HTTP 200, got {status} {response}"
        );
        let read = service.read(&referenced.target_arn).await;
        assert_failure(
            case,
            &format!("a read of the deleted {}", referenced.target_arn),
            (404, "ResourceNotFoundException"),
            &read,
        );

        // Nothing else moved: the configuration the row did not delete is still there, the
        // composition still reports the destinations it was started with, and the stage is
        // still reported.
        let (other_status, other) = service.read(&referenced.other_arn).await;
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
        if let Some(stage_arn) = recording_stage {
            let (stage_status, stage) = service
                .call("GetStage", &json!({ "arn": stage_arn }))
                .await;
            assert_eq!(
                stage_status, 200,
                "case {case:?}: the stage must still be reported, got {stage_status} {stage}"
            );
        }
    }
}
