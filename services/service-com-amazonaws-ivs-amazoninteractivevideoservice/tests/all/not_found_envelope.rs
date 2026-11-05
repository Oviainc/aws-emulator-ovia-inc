// SPDX-License-Identifier: CC0-1.0

//! What IVS answers a request that names a resource this store does not hold, driven
//! over the wire.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws ivs` call arrives — so what it asserts is
//! the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an operation
//! handed a well-formed ARN no resource of this store answers to refuses the call with
//! HTTP 404 and an error document a client can read the failure out of — the error's code
//! under `Code` (and in the restJson1 `__type` discriminator), the text
//! `Resource: <the ARN the request named> not found` under `Message` and under the
//! `message` key an SDK error parser reads, and the id of the request it answers under
//! `RequestID`. It holds for every operation that declares
//! `ResourceNotFoundException` (`smithy_query(query="operation",
//! operation="GetChannel,UpdateChannel,DeleteChannel,GetAdConfiguration,\
//! DeleteAdConfiguration,GetPlaybackKeyPair,DeletePlaybackKeyPair,\
//! GetRecordingConfiguration,DeleteRecordingConfiguration,GetStreamKey")), whichever
//! member the ARN travels in.
//!
//! The rows differ along what the rule generalises over: the operation, the member
//! carrying the ARN, the resource type, the shape of the resource id, the ARN's region
//! and account, and whether the named resource never existed or was minted by this store
//! and then deleted.

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
use serde_json::{json, Value};

use crate::channel_lifecycle::{text, Ivs, Reply};
use crate::error_envelope::is_request_id_shaped;

/// The wire error code the model declares for a request that names a resource the
/// service does not hold (`@httpError 404`).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The three members of the error document a client of this service reads a failure out
/// of, plus the key an SDK error parser reads the text under.
const CODE_KEY: &str = "Code";
const MESSAGE_KEY: &str = "Message";
const REQUEST_ID_KEY: &str = "RequestID";
const SDK_MESSAGE_KEY: &str = "message";

/// The region and account `Ivs` signs for, and so the ones every ARN this store mints
/// carries.
const REGION: &str = "us-east-1";
const ACCOUNT: &str = "000000000000";

/// An IVS ARN of `resource_type` for `id`, in `region` and for `account`.
///
/// Every modelled IVS ARN pattern pins the region to `[a-z0-9-]+`, the account to
/// `[0-9]+` and the id to `[a-zA-Z0-9-]+`, so a row may vary all three and still send a
/// request that reaches the handler rather than the constraint walk.
fn arn_in(region: &str, account: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{account}:{resource_type}/{id}")
}

/// An ARN of `resource_type` for `id`, in the region and account this suite signs for.
fn absent_arn(resource_type: &str, id: &str) -> String {
    arn_in(REGION, ACCOUNT, resource_type, id)
}

/// This service's one not-found sentence: fixed text around the ARN the request named,
/// which is its only variable part.
fn not_found(arn: &str) -> String {
    format!("Resource: {arn} not found")
}

/// A freshly generated public key in the `PUBLIC KEY` PEM block an import carries, so
/// what the suite imports is a real public key rather than a hand-built byte string.
fn public_key_pem() -> String {
    let der = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .expect("the test key pair is generated")
        .public_key_der();
    pem::encode(&pem::Pem::new("PUBLIC KEY", der))
}

/// The rule over ONE answer, returning the id the response named the request by so a
/// caller can compare it with the other rows'.
fn assert_not_found_envelope(case: &str, reply: &Reply, arn: &str) -> String {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        text(&reply.body, CODE_KEY),
        NOT_FOUND_CODE,
        "case {case:?}: expected the document's {CODE_KEY} to be {NOT_FOUND_CODE:?}, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_code, NOT_FOUND_CODE,
        "case {case:?}: expected the restJson1 discriminator to be {NOT_FOUND_CODE:?}, got {}",
        reply.body
    );
    assert_eq!(
        text(&reply.body, MESSAGE_KEY),
        not_found(arn),
        "case {case:?}: expected the document's {MESSAGE_KEY} to name the ARN the request \
         sent, got {}",
        reply.body
    );
    assert_eq!(
        text(&reply.body, SDK_MESSAGE_KEY),
        not_found(arn),
        "case {case:?}: expected the SDK {SDK_MESSAGE_KEY} key to carry the same text, got {}",
        reply.body
    );
    let request_id = text(&reply.body, REQUEST_ID_KEY);
    assert!(
        is_request_id_shaped(&request_id),
        "case {case:?}: expected the document's {REQUEST_ID_KEY} to take the 8-4-4-4-12 \
         lowercase-hex form, got {request_id:?} in {}",
        reply.body
    );
    // The model's own detail member is not what a client reads: the text travels under
    // the keys above, so the modelled `exceptionMessage` is nowhere in the body.
    assert!(
        reply.body.get("exceptionMessage").is_none(),
        "case {case:?}: the text travels under the document's keys, not the model's own \
         member name, got {}",
        reply.body
    );
    request_id
}

/// No two of `named` name the same request.
fn assert_each_names_one_request(named: &[(String, String)]) {
    for (left, (case, id)) in named.iter().enumerate() {
        for (other_case, other_id) in named.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "case {case:?} and case {other_case:?}: two responses must not name the \
                 same request, both named {id:?}"
            );
        }
    }
}

/// One row: the case, the operation, the request to send, and the ARN its refusal must
/// name.
type Row = (&'static str, &'static str, Value, String);

/// The rule over every operation that declares `ResourceNotFoundException`, for an ARN
/// this store never minted.
#[tokio::test]
async fn an_arn_this_store_never_minted_is_refused_with_the_not_found_envelope() {
    let ivs = Ivs::new();

    let channel = absent_arn("channel", "neverCreated");
    let renamed_channel = absent_arn("channel", "never-created-either");
    let dropped_channel = absent_arn("channel", "ffffffffffff");
    let other_region_channel = arn_in("eu-west-1", ACCOUNT, "channel", "abcdef123456");
    let other_account_channel = arn_in(REGION, "123456789012", "channel", "abcdef123456");
    let key_source = absent_arn("channel", "noKeysHere");
    let listed_channel = absent_arn("channel", "nothingToList");
    let revoked_channel = absent_arn("channel", "noViewersHere");
    let read_channel = absent_arn("channel", "notIngesting");
    let stopped_channel = absent_arn("channel", "nothingToStop");
    let annotated_channel = absent_arn("channel", "noMetadataHere");
    let ad_configuration = absent_arn("ad-configuration", "does-not-exist");
    // A single-character id: the modelled `[a-zA-Z0-9-]+` admits one, and the ARN still
    // reaches the handler.
    let deleted_ad_configuration = absent_arn("ad-configuration", "A");
    let key_pair = absent_arn("playback-key", "neverImported");
    let deleted_key_pair = absent_arn("playback-key", "0123456789ab");
    let recording_configuration = absent_arn("recording-configuration", "does-not-exist");
    let deleted_recording_configuration = absent_arn("recording-configuration", "r");
    let stream_key = absent_arn("stream-key", "aaaaaaaaaaaa");
    let deleted_stream_key = absent_arn("stream-key", "bb-cc-dd");

    let rows: Vec<Row> = vec![
        (
            "a channel read",
            "GetChannel",
            json!({ "arn": channel }),
            channel.clone(),
        ),
        (
            "a channel updated",
            "UpdateChannel",
            json!({ "arn": renamed_channel, "name": "renamed" }),
            renamed_channel.clone(),
        ),
        (
            "a channel deleted",
            "DeleteChannel",
            json!({ "arn": dropped_channel }),
            dropped_channel.clone(),
        ),
        (
            "a channel of another region",
            "GetChannel",
            json!({ "arn": other_region_channel }),
            other_region_channel.clone(),
        ),
        (
            "a channel of another account",
            "DeleteChannel",
            json!({ "arn": other_account_channel }),
            other_account_channel.clone(),
        ),
        (
            "an ad configuration read",
            "GetAdConfiguration",
            json!({ "arn": ad_configuration }),
            ad_configuration.clone(),
        ),
        (
            "an ad configuration deleted",
            "DeleteAdConfiguration",
            json!({ "arn": deleted_ad_configuration }),
            deleted_ad_configuration.clone(),
        ),
        (
            "a playback key pair read",
            "GetPlaybackKeyPair",
            json!({ "arn": key_pair }),
            key_pair.clone(),
        ),
        (
            "a playback key pair deleted",
            "DeletePlaybackKeyPair",
            json!({ "arn": deleted_key_pair }),
            deleted_key_pair.clone(),
        ),
        (
            "a recording configuration read",
            "GetRecordingConfiguration",
            json!({ "arn": recording_configuration }),
            recording_configuration.clone(),
        ),
        (
            "a recording configuration deleted",
            "DeleteRecordingConfiguration",
            json!({ "arn": deleted_recording_configuration }),
            deleted_recording_configuration.clone(),
        ),
        (
            "a stream key read",
            "GetStreamKey",
            json!({ "arn": stream_key }),
            stream_key.clone(),
        ),
        (
            "a stream key deleted",
            "DeleteStreamKey",
            json!({ "arn": deleted_stream_key }),
            deleted_stream_key.clone(),
        ),
        // The same rule where the ARN travels in `channelArn` rather than `arn`: the
        // refusal names the ARN the request sent, whatever the member it sent it in.
        (
            "a stream key minted on an absent channel",
            "CreateStreamKey",
            json!({ "channelArn": key_source }),
            key_source.clone(),
        ),
        (
            "an absent channel's stream keys listed",
            "ListStreamKeys",
            json!({ "channelArn": listed_channel }),
            listed_channel.clone(),
        ),
        (
            "a viewer session revoked on an absent channel",
            "StartViewerSessionRevocation",
            json!({ "channelArn": revoked_channel, "viewerId": "viewer-1" }),
            revoked_channel.clone(),
        ),
        (
            "an absent channel's stream read",
            "GetStream",
            json!({ "channelArn": read_channel }),
            read_channel.clone(),
        ),
        (
            "an absent channel's stream stopped",
            "StopStream",
            json!({ "channelArn": stopped_channel }),
            stopped_channel.clone(),
        ),
        (
            "metadata put on an absent channel",
            "PutMetadata",
            json!({ "channelArn": annotated_channel, "metadata": "cue" }),
            annotated_channel.clone(),
        ),
    ];

    let mut named: Vec<(String, String)> = Vec::new();
    for (case, operation, request, arn) in rows {
        let reply = ivs.call(operation, &request).await;
        let request_id = assert_not_found_envelope(case, &reply, &arn);
        named.push((case.to_string(), request_id));
    }
    assert_each_names_one_request(&named);
}

/// The same rule for an ARN this store DID mint and then dropped: every operation that
/// names a deleted resource answers the same envelope as one that never existed, for
/// every resource type this service holds.
#[tokio::test]
async fn a_deleted_resource_is_refused_with_the_not_found_envelope() {
    let ivs = Ivs::new();

    // One channel, with the stream key the create minted for it. The key is deleted
    // first, so the row that reads it back is a resource that existed on its own.
    let created = ivs.create(json!({ "name": "envelope-channel" })).await;
    let channel = text(&created["channel"], "arn");
    let stream_key = text(&created["streamKey"], "arn");

    let ad_configuration = text(
        &ivs.ok(
            "CreateAdConfiguration",
            json!({ "name": "envelope-ads",
                    "mediaTailorPlaybackConfigurations": [{
                        "playbackConfigurationArn":
                            format!("arn:aws:mediatailor:{REGION}:{ACCOUNT}:playbackConfiguration/live"),
                    }] }),
        )
        .await["adConfiguration"],
        "arn",
    );
    let recording_configuration = text(
        &ivs.ok(
            "CreateRecordingConfiguration",
            json!({ "name": "envelope-recording",
                    "destinationConfiguration": { "s3": { "bucketName": "envelope-bucket" } } }),
        )
        .await["recordingConfiguration"],
        "arn",
    );
    let key_pair = text(
        &ivs.ok(
            "ImportPlaybackKeyPair",
            json!({ "name": "envelope-key", "publicKeyMaterial": public_key_pem() }),
        )
        .await["keyPair"],
        "arn",
    );

    // Each resource type: the delete that drops it, then every operation that names it
    // afterwards. The deletes are the modelled ones, so what the rows read back is state
    // this emulator really dropped.
    let dropped: Vec<(&str, &str, Value, &[&str], String)> = vec![
        (
            "the stream key the channel's create minted",
            "DeleteStreamKey",
            json!({ "arn": stream_key }),
            &["GetStreamKey", "DeleteStreamKey"],
            stream_key.clone(),
        ),
        (
            "an imported playback key pair",
            "DeletePlaybackKeyPair",
            json!({ "arn": key_pair }),
            &["GetPlaybackKeyPair", "DeletePlaybackKeyPair"],
            key_pair.clone(),
        ),
        (
            "a created ad configuration",
            "DeleteAdConfiguration",
            json!({ "arn": ad_configuration }),
            &["GetAdConfiguration", "DeleteAdConfiguration"],
            ad_configuration.clone(),
        ),
        (
            "a created recording configuration",
            "DeleteRecordingConfiguration",
            json!({ "arn": recording_configuration }),
            &["GetRecordingConfiguration", "DeleteRecordingConfiguration"],
            recording_configuration.clone(),
        ),
        (
            "a created channel",
            "DeleteChannel",
            json!({ "arn": channel }),
            &["GetChannel", "DeleteChannel"],
            channel.clone(),
        ),
    ];

    let mut named: Vec<(String, String)> = Vec::new();
    for (case, delete, request, readers, arn) in dropped {
        let dropped = ivs.call(delete, &request).await;
        assert!(
            dropped.status == 200 || dropped.status == 204,
            "case {case:?}: expected {delete} to succeed, got {} {}",
            dropped.status,
            dropped.body
        );
        for reader in readers {
            let reply = ivs.call(reader, &json!({ "arn": arn })).await;
            let row = format!("{case} read back through {reader}");
            let request_id = assert_not_found_envelope(&row, &reply, &arn);
            named.push((row, request_id));
        }
    }

    // The channel is gone, so the operations that reach a channel by `channelArn` answer
    // the same envelope for an ARN that WAS live a moment ago.
    for (reader, request) in [
        ("UpdateChannel", json!({ "arn": channel, "name": "renamed" })),
        ("CreateStreamKey", json!({ "channelArn": channel })),
        ("ListStreamKeys", json!({ "channelArn": channel })),
        ("GetStream", json!({ "channelArn": channel })),
        ("StopStream", json!({ "channelArn": channel })),
        (
            "PutMetadata",
            json!({ "channelArn": channel, "metadata": "cue" }),
        ),
        (
            "StartViewerSessionRevocation",
            json!({ "channelArn": channel, "viewerId": "viewer-1" }),
        ),
    ] {
        let reply = ivs.call(reader, &request).await;
        let row = format!("a deleted channel named by {reader}");
        let request_id = assert_not_found_envelope(&row, &reply, &channel);
        named.push((row, request_id));
    }

    assert_each_names_one_request(&named);
}
