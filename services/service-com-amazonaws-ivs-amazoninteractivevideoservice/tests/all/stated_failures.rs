// SPDX-License-Identifier: CC0-1.0

//! How this emulator STATES what it answers, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A modelled failure reaches the client with its text: the body carries it under
//!    the key `message` (the key every SDK error parser reads), beside the error code
//!    and HTTP status the model declares for that shape. The text itself is built from
//!    the request — the ARN it named, the member it sent, the quota it met.
//! 2. A value outside a modelled constraint is refused as `<Operation>.<member path>`
//!    plus the constraint it violated, for a member of the request or of a structure
//!    nested in it, and for every constraint class the model uses.
//! 3. A resource's tags are readable and writable through the tagging surface by the
//!    resource's own ARN, whatever its resource type; an ARN that names no resource is
//!    a not-found there, and one that is not an IVS resource ARN at all is a
//!    validation failure.
//! 4. An operation that acts on a channel's INGEST needs the channel to be ingesting;
//!    one that acts on its playback tokens does not.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::service_state;
use service_com_amazonaws_ivs_amazoninteractivevideoservice::{
    ComAmazonawsIvsAmazoninteractivevideoserviceState, EmulatedService,
};

/// A SigV4 `Authorization` header is how a request names the service and the region it
/// is addressed to, so every call carries one. The signature itself is never checked
/// here: request authentication is off unless a router turns it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The account and region every ARN in this suite is built for: the canonical emulator
/// account id and the region the `AUTHORIZATION` scope above names.
const ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:";

/// One IVS emulator over one shared engine, reached the way the router reaches it.
struct Ivs {
    router: MultiServiceRouter,
    /// The engine the service was built on, kept so this suite can read the store for
    /// state no operation reports back.
    engine: Arc<engine::InMemoryEngine>,
}

/// One answer from the emulator: the status, the wire error code (the restJson1
/// `__type` discriminator), the body's `message` member, and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    message: String,
    body: Value,
}

impl Ivs {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The version threshold this service's store holds for the
    /// `(channelArn, viewerId)` pair, or `None` when no revocation was started for it.
    ///
    /// Read straight from the store, the way a handler reaches it, because IVS models
    /// no operation that reports a started viewer-session revocation back.
    fn revocation_threshold(&self, channel_arn: &str, viewer_id: &str) -> Option<i32> {
        let state = service_state(&*self.engine).expect("the IVS store is reachable");
        let store = state.lock();
        read_threshold(&store, channel_arn, viewer_id)
    }

    /// An operation bound to `POST /<Operation>`, which is every IVS operation except
    /// the three tagging ones.
    async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.send(
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// Call `operation` and require it to have succeeded, returning the body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let reply = self.call(operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {operation} {body}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, which carry the ARN
    /// in the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, query: &str, body: Option<Value>) -> Reply {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(&self, method: &str, path: &str, body: Option<bytes::Bytes>) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivs.us-east-1.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code: member(&parsed, "__type"),
            message: member(&parsed, "message"),
            body: parsed,
        }
    }

    /// Create a channel and return the whole `CreateChannel` output.
    async fn create(&self, request: Value) -> Value {
        self.ok("CreateChannel", request).await
    }

    /// Create a channel and return just its ARN.
    async fn create_arn(&self, request: Value) -> String {
        member(&self.create(request).await["channel"], "arn")
    }

    /// The tags the tagging surface answers for `arn`, as a map.
    async fn tags_of(&self, arn: &str) -> HashMap<String, String> {
        let reply = self.tagging("GET", arn, "", None).await;
        assert_eq!(
            reply.status, 200,
            "case tags of {arn}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        serde_json::from_value(reply.body["tags"].clone()).unwrap_or_default()
    }
}

/// The threshold `store` holds for one `(channelArn, viewerId)` pair.
fn read_threshold(
    store: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    viewer_id: &str,
) -> Option<i32> {
    store
        .viewer_session_revocations
        .get(&(channel_arn.to_string(), viewer_id.to_string()))
        .copied()
}

/// A string member, read as the empty string when absent — which is what a client sees
/// for a member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value, the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment. Every character
/// an IVS ARN can hold is ASCII, so one byte per character.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The not-found text the service states for an ARN it holds no resource for.
fn not_found(arn: &str) -> String {
    format!("Resource: {arn} not found")
}

/// The text the service states for an operation on a channel that is not ingesting.
fn offline(arn: &str) -> String {
    format!("Channel: {arn} is not currently online")
}

/// Rule 1: every modelled failure carries its text under the body key `message`, with
/// the code and status the model declares, and the text is built from the request.
///
/// The rows span the failures this service can answer: a resource that is not there (on
/// each operation that addresses one by ARN, and both for an ARN that never existed and
/// one that was deleted), a channel that is not ingesting, a quota that is full, and a
/// pair of members that cannot hold together.
#[tokio::test]
async fn a_modelled_failure_states_its_message_under_the_key_a_client_reads() {
    let ivs = Ivs::new();
    let live = ivs.create_arn(json!({ "name": "stated" })).await;
    let deleted = ivs.create_arn(json!({ "name": "gone" })).await;
    // `DeleteChannel`'s modelled success is 204, so it is driven through `call`.
    let removal = ivs.call("DeleteChannel", &json!({ "arn": deleted })).await;
    assert_eq!(
        removal.status, 204,
        "the fixture channel must really be deleted, got {} {}",
        removal.status, removal.body
    );
    // A well-formed `ChannelArn` this store never minted.
    let ghost = format!("{ARN_PREFIX}channel/0123456789ab");

    let cases: Vec<(&str, &str, Value, u16, &str, String)> = vec![
        (
            "GetChannel, arn never created",
            "GetChannel",
            json!({ "arn": ghost }),
            404,
            "ResourceNotFoundException",
            not_found(&ghost),
        ),
        (
            "GetChannel, arn deleted",
            "GetChannel",
            json!({ "arn": deleted }),
            404,
            "ResourceNotFoundException",
            not_found(&deleted),
        ),
        (
            "UpdateChannel, arn deleted",
            "UpdateChannel",
            json!({ "arn": deleted, "latencyMode": "NORMAL" }),
            404,
            "ResourceNotFoundException",
            not_found(&deleted),
        ),
        (
            "DeleteChannel, arn never created",
            "DeleteChannel",
            json!({ "arn": ghost }),
            404,
            "ResourceNotFoundException",
            not_found(&ghost),
        ),
        (
            "CreateStreamKey, channelArn never created",
            "CreateStreamKey",
            json!({ "channelArn": ghost }),
            404,
            "ResourceNotFoundException",
            not_found(&ghost),
        ),
        (
            "ListStreamKeys, channelArn deleted",
            "ListStreamKeys",
            json!({ "channelArn": deleted }),
            404,
            "ResourceNotFoundException",
            not_found(&deleted),
        ),
        (
            "StartViewerSessionRevocation, channelArn never created",
            "StartViewerSessionRevocation",
            json!({ "channelArn": ghost, "viewerId": "viewer-1" }),
            404,
            "ResourceNotFoundException",
            not_found(&ghost),
        ),
        (
            "PutMetadata, channel that never went live",
            "PutMetadata",
            json!({ "channelArn": live, "metadata": "cue" }),
            404,
            "ChannelNotBroadcasting",
            offline(&live),
        ),
        (
            "StopStream, channel that never went live",
            "StopStream",
            json!({ "channelArn": live }),
            404,
            "ChannelNotBroadcasting",
            offline(&live),
        ),
        (
            "CreateStreamKey, channel already at its one-key quota",
            "CreateStreamKey",
            json!({ "channelArn": live }),
            402,
            "ServiceQuotaExceededException",
            "Resource: stream-key quota exceeded limit: 1".to_string(),
        ),
        (
            "CreateChannel, multitrack enabled with a TS container",
            "CreateChannel",
            json!({ "name": "mt", "containerFormat": "TS",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "ALLOW",
                                                      "maximumResolution": "HD" } }),
            400,
            "ValidationException",
            "Request member: multitrack_input_configuration.enabled failed to satisfy the \
             constraints: invalid value for multitrack input configuration enabled and \
             container format"
                .to_string(),
        ),
    ];

    for (case, operation, request, status, code, message) in cases {
        let reply = ivs.call(operation, &request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str(), reply.message.as_str()),
            (status, code, message.as_str()),
            "case {case:?}: expected {status} {code} {message:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert!(
            reply.body.get("exceptionMessage").is_none(),
            "case {case:?}: the text travels under `message`, not under the model's own \
             member name, got {}",
            reply.body
        );
    }
}

/// Rule 2: a value outside a modelled constraint is refused as `<Operation>.<member
/// path>` plus the constraint it violated.
///
/// The rows span what the rule generalises over: nine operations, a member of the
/// request and a member of a structure nested two deep in it, and every constraint
/// class the model uses on this service — a `@pattern`, both ends of a `@length` on a
/// string and on a collection, a `@range`, and a `@required` member absent — each bound
/// exercised at the value just past it. Nothing in the service names these members: the
/// refusal is built from the framework's own violations, so a row here passes only
/// because the DERIVATION is right, not because that member was special-cased.
#[tokio::test]
async fn a_constraint_violation_names_the_operation_the_member_and_the_constraint() {
    let ivs = Ivs::new();
    let arn = ivs.create_arn(json!({ "name": "constrained" })).await;
    let too_long = "n".repeat(129);
    let bucket_of = |name: &str| {
        json!({ "name": "rec", "destinationConfiguration": { "s3": { "bucketName": name } } })
    };
    let playback_configuration = json!({ "playbackConfigurationArn": "arn:aws:mediatailor:us-east-1:000000000000:playbackConfiguration/x" });

    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "GetChannel.arn outside the pattern of its ARN shape",
            "GetChannel",
            json!({ "arn": "not-an-arn" }),
            "GetChannel.arn does not match the required pattern \
             ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$"
                .to_string(),
        ),
        (
            "CreateStreamKey without the channelArn it requires",
            "CreateStreamKey",
            json!({}),
            "CreateStreamKey.channelArn is required".to_string(),
        ),
        (
            "ListChannels.maxResults below its range",
            "ListChannels",
            json!({ "maxResults": 0 }),
            "ListChannels.maxResults is required to have a minimum value of 1".to_string(),
        ),
        (
            "StartViewerSessionRevocation.viewerId one character past its maximum",
            "StartViewerSessionRevocation",
            json!({ "channelArn": arn, "viewerId": "v".repeat(41) }),
            "StartViewerSessionRevocation.viewerId is required to have a maximum length \
             of 40, found 41"
                .to_string(),
        ),
        (
            "PutMetadata.metadata below its minimum length",
            "PutMetadata",
            json!({ "channelArn": arn, "metadata": "" }),
            "PutMetadata.metadata is required to have a minimum length of 1, found 0"
                .to_string(),
        ),
        (
            "CreateChannel.name outside its pattern",
            "CreateChannel",
            json!({ "name": "sim2real ivs chan!!" }),
            "CreateChannel.name does not match the required pattern ^[a-zA-Z0-9-_]*$".to_string(),
        ),
        (
            "CreateChannel.name one character past its maximum",
            "CreateChannel",
            json!({ "name": too_long }),
            "CreateChannel.name is required to have a maximum length of 128, found 129"
                .to_string(),
        ),
        (
            "UpdateChannel.name outside its pattern",
            "UpdateChannel",
            json!({ "arn": arn, "name": "not valid" }),
            "UpdateChannel.name does not match the required pattern ^[a-zA-Z0-9-_]*$".to_string(),
        ),
        (
            "ListChannels.filterByName outside its pattern",
            "ListChannels",
            json!({ "filterByName": "not valid" }),
            "ListChannels.filterByName does not match the required pattern ^[a-zA-Z0-9-_]*$"
                .to_string(),
        ),
        (
            "CreateRecordingConfiguration's nested bucketName outside its pattern",
            "CreateRecordingConfiguration",
            bucket_of("Invalid_Bucket_Name_x220e5705608b"),
            "CreateRecordingConfiguration.destinationConfiguration.s3.bucketName does not \
             match the required pattern ^[a-z0-9-.]+$"
                .to_string(),
        ),
        (
            "CreateRecordingConfiguration's nested bucketName one character short",
            "CreateRecordingConfiguration",
            bucket_of("ab"),
            "CreateRecordingConfiguration.destinationConfiguration.s3.bucketName is required \
             to have a minimum length of 3, found 2"
                .to_string(),
        ),
        (
            "CreateAdConfiguration.mediaTailorPlaybackConfigurations below its minimum",
            "CreateAdConfiguration",
            json!({ "name": "ads", "mediaTailorPlaybackConfigurations": [] }),
            "CreateAdConfiguration.mediaTailorPlaybackConfigurations is required to have a \
             minimum length of 1, found 0"
                .to_string(),
        ),
        (
            "CreateAdConfiguration.mediaTailorPlaybackConfigurations past its maximum",
            "CreateAdConfiguration",
            json!({ "name": "ads", "mediaTailorPlaybackConfigurations": [
                playback_configuration, playback_configuration,
                playback_configuration, playback_configuration ] }),
            "CreateAdConfiguration.mediaTailorPlaybackConfigurations is required to have a \
             maximum length of 3, found 4"
                .to_string(),
        ),
    ];

    for (case, operation, request, message) in cases {
        let reply = ivs.call(operation, &request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str(), reply.message.as_str()),
            (400, "ValidationException", message.as_str()),
            "case {case:?}: expected 400 ValidationException {message:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 3, the accepting half: tags round-trip through the tagging surface for a
/// resource of any type this service holds, addressed by its own ARN.
///
/// The rows are the resource types the store can hold — a channel, a stream key and a
/// recording configuration — one created with tags and two without, so the read answers
/// both a tag set a create persisted and an empty one.
#[tokio::test]
async fn tags_round_trip_through_the_tagging_surface_for_every_resource_type() {
    let ivs = Ivs::new();
    let created = ivs
        .create(json!({ "name": "taggable", "tags": { "env": "test" } }))
        .await;
    let channel_arn = member(&created["channel"], "arn");
    let stream_key_arn = member(&created["streamKey"], "arn");
    let recording_configuration = ivs
        .ok(
            "CreateRecordingConfiguration",
            json!({ "name": "rec",
                    "destinationConfiguration": { "s3": { "bucketName": "valid-bucket" } } }),
        )
        .await;
    let recording_configuration_arn =
        member(&recording_configuration["recordingConfiguration"], "arn");

    let cases: Vec<(&str, String, HashMap<String, String>)> = vec![
        (
            "channel, tagged at creation",
            channel_arn,
            HashMap::from([("env".to_string(), "test".to_string())]),
        ),
        ("stream key, untagged", stream_key_arn, HashMap::new()),
        (
            "recording configuration, untagged",
            recording_configuration_arn,
            HashMap::new(),
        ),
    ];

    for (case, arn, at_creation) in cases {
        assert_eq!(
            ivs.tags_of(&arn).await,
            at_creation,
            "case {case:?}: the surface must answer the tags the create persisted"
        );

        // A tag added, and one of the same key replaced.
        let reply = ivs
            .tagging(
                "POST",
                &arn,
                "",
                Some(json!({ "tags": { "team": "video", "env": "prod" } })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: TagResource must be accepted, got {} {}",
            reply.status, reply.body
        );
        let expected = HashMap::from([
            ("team".to_string(), "video".to_string()),
            ("env".to_string(), "prod".to_string()),
        ]);
        assert_eq!(
            ivs.tags_of(&arn).await,
            expected,
            "case {case:?}: a tagged key takes the new value and every other tag is kept"
        );

        // A removal, and the same removal again: the operation is modelled
        // `@idempotent`, so repeating it leaves the same tag set.
        for round in 0..2 {
            let reply = ivs.tagging("DELETE", &arn, "?tagKeys=env", None).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} round {round}: UntagResource must be accepted, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                ivs.tags_of(&arn).await,
                HashMap::from([("team".to_string(), "video".to_string())]),
                "case {case:?} round {round}: the named key is gone and the rest is kept"
            );
        }

        // Several keys at once, including one the resource does not carry.
        let reply = ivs
            .tagging("DELETE", &arn, "?tagKeys=team&tagKeys=absent", None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: removing several keys at once must be accepted, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            ivs.tags_of(&arn).await,
            HashMap::new(),
            "case {case:?}: every named key is gone"
        );
    }
}

/// Rule 3, the refusing half: a well-formed IVS resource ARN that names no resource is
/// a not-found on the tagging surface, and a string that is not an IVS resource ARN is
/// a validation failure naming the member and the pattern.
#[tokio::test]
async fn the_tagging_surface_separates_a_missing_resource_from_a_bad_arn() {
    let ivs = Ivs::new();

    // Rows differ in the resource TYPE named: one this service can create and does not
    // hold, and two whose creates are still scaffold stubs, so no such resource exists.
    let missing = [
        format!("{ARN_PREFIX}channel/0123456789ab"),
        format!("{ARN_PREFIX}playback-key/00000000-0000-0000-0000-000000000000"),
        format!("{ARN_PREFIX}recording-configuration/abc123"),
    ];
    for arn in &missing {
        for (method, query, body) in [
            ("GET", "", None),
            ("POST", "", Some(json!({ "tags": { "team": "video" } }))),
            ("DELETE", "?tagKeys=team", None),
        ] {
            let reply = ivs.tagging(method, arn, query, body).await;
            assert_eq!(
                (
                    reply.status,
                    reply.error_code.as_str(),
                    reply.message.as_str()
                ),
                (404, "NotFoundException", "Resource was not found"),
                "case {method} {arn}: a well-formed arn naming no resource is a not-found, \
                 got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }

    // Rows differ in HOW the string fails to be an IVS resource ARN: not an ARN, another
    // service's ARN, no resource id, an uppercase region.
    let malformed = [
        "not-an-arn",
        "arn:aws:s3:::some-bucket",
        &format!("{ARN_PREFIX}channel"),
        "arn:aws:ivs:US-EAST-1:000000000000:channel/0123456789ab",
    ];
    let pattern = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]+/[a-zA-Z0-9-]+$";
    for arn in malformed {
        let reply = ivs.tagging("GET", arn, "", None).await;
        let expected =
            format!("ListTagsForResource.resourceArn does not match the required pattern {pattern}");
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.message.as_str()
            ),
            (400, "ValidationException", expected.as_str()),
            "case {arn:?}: a string that is not an IVS resource arn is a validation \
             failure, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 1 again, on the quota's other half: the stream-key quota is a bound on what the
/// CHANNEL holds, so deleting the key it holds frees the slot and the next create
/// succeeds — the delete-then-create a client rotates a key with — while a further
/// create is refused again.
#[tokio::test]
async fn the_stream_key_quota_is_freed_by_deleting_the_key() {
    let ivs = Ivs::new();

    // Rows differ in what the replacement key carries, so the freed slot is exercised
    // for more than one shape of create.
    for request in [
        json!({}),
        json!({ "tags": {} }),
        json!({ "tags": { "env": "test", "team": "video" } }),
    ] {
        let created = ivs.create(json!({ "name": "rotating" })).await;
        let channel = member(&created["channel"], "arn");
        let first_key = member(&created["streamKey"], "arn");

        let refused = ivs
            .call("CreateStreamKey", &json!({ "channelArn": channel }))
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {request}: the channel is at its quota before the delete, got {} {} {}",
            refused.status,
            refused.error_code,
            refused.body
        );

        let freed = ivs
            .call("DeleteStreamKey", &json!({ "arn": first_key }))
            .await;
        assert_eq!(
            freed.status, 204,
            "case {request}: DeleteStreamKey must remove the key, got {} {}",
            freed.status, freed.body
        );

        let mut body = request.clone();
        body["channelArn"] = json!(channel);
        let replacement = ivs.call("CreateStreamKey", &body).await;
        assert_eq!(
            replacement.status, 200,
            "case {request}: the freed slot must accept a new key, got {} {}",
            replacement.status, replacement.body
        );
        let replacement_arn = member(&replacement.body["streamKey"], "arn");
        assert_ne!(
            replacement_arn, first_key,
            "case {request}: the replacement key must be a new key, got {}",
            replacement.body
        );

        let refused_again = ivs
            .call("CreateStreamKey", &json!({ "channelArn": channel }))
            .await;
        assert_eq!(
            (refused_again.status, refused_again.error_code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {request}: the channel is at its quota again, got {} {} {}",
            refused_again.status,
            refused_again.error_code,
            refused_again.body
        );

        // The key the delete removed is gone for every reader, and the one that
        // replaced it is there.
        let listed = ivs
            .ok("ListStreamKeys", json!({ "channelArn": channel }))
            .await;
        let arns: Vec<String> = listed["streamKeys"]
            .as_array()
            .expect("streamKeys is a list")
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect();
        assert_eq!(
            arns,
            vec![replacement_arn.clone()],
            "case {request}: the channel holds exactly the replacement key, got {listed}"
        );
        let gone = ivs.call("GetStreamKey", &json!({ "arn": first_key })).await;
        assert_eq!(
            (gone.status, gone.error_code.as_str(), gone.message.as_str()),
            (404, "ResourceNotFoundException", not_found(&first_key).as_str()),
            "case {request}: the deleted key is not found by its own arn, got {} {} {}",
            gone.status,
            gone.error_code,
            gone.body
        );
    }
}

/// Rule 4: `StartViewerSessionRevocation` RECORDS the revocation for the
/// `(channelArn, viewerId)` pair it names — the record the batch form writes too, with
/// the threshold the maximum over the calls — and, because it acts on playback tokens
/// rather than on the ingest, it is answered whatever the channel's broadcast state and
/// changes nothing about the channel itself.
#[tokio::test]
async fn revoking_viewer_sessions_records_the_pair_without_touching_the_channel() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "revocable" })).await;
    let arn = member(&created["channel"], "arn");
    let before = ivs.ok("GetChannel", json!({ "arn": arn })).await;

    // Rows differ along the one member the caller varies — the session version ceiling,
    // absent (the modelled default of 0) or given — and each names the threshold the
    // store must hold for its pair afterwards.
    for (viewer, request, threshold) in [
        (
            "viewer-1",
            json!({ "channelArn": arn, "viewerId": "viewer-1" }),
            0,
        ),
        (
            "v",
            json!({ "channelArn": arn, "viewerId": "v",
                    "viewerSessionVersionsLessThanOrEqualTo": 0 }),
            0,
        ),
        (
            "viewer-2",
            json!({ "channelArn": arn, "viewerId": "viewer-2",
                    "viewerSessionVersionsLessThanOrEqualTo": 7 }),
            7,
        ),
    ] {
        assert_eq!(
            ivs.revocation_threshold(&arn, viewer),
            None,
            "case {request}: no revocation is recorded for this pair before the call"
        );

        let reply = ivs.call("StartViewerSessionRevocation", &request).await;
        assert_eq!(
            reply.status, 200,
            "case {request}: a revocation on an existing channel is accepted, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            ivs.revocation_threshold(&arn, viewer),
            Some(threshold),
            "case {request}: the call must record the pair's version threshold"
        );

        // The threshold is the maximum over the calls, so a later call naming a LOWER
        // version cannot un-revoke what this one revoked, and a repeat of the same
        // version changes nothing; a HIGHER version raises it.
        for (again, expected) in [(threshold, threshold), (0, threshold), (9, 9)] {
            let mut repeat = request.clone();
            repeat["viewerSessionVersionsLessThanOrEqualTo"] = json!(again);
            let reply = ivs.call("StartViewerSessionRevocation", &repeat).await;
            assert_eq!(
                reply.status, 200,
                "case {repeat}: a repeated revocation is accepted, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                ivs.revocation_threshold(&arn, viewer),
                Some(expected),
                "case {repeat}: the stored threshold is the maximum over the calls"
            );
        }

        let after = ivs.ok("GetChannel", json!({ "arn": arn })).await;
        assert_eq!(
            after, before,
            "case {request}: a revocation must leave the channel exactly as it was"
        );
        let stopped = ivs.call("StopStream", &json!({ "channelArn": arn })).await;
        assert_eq!(
            (stopped.status, stopped.error_code.as_str()),
            (404, "ChannelNotBroadcasting"),
            "case {request}: the channel's broadcast state is untouched, so it is still \
             not broadcasting, got {} {} {}",
            stopped.status,
            stopped.error_code,
            stopped.body
        );
    }
}
