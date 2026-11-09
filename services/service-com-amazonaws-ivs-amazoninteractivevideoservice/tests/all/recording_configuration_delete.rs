// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteRecordingConfiguration`, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! dispatch harness (`Ivs`, `Reply`, `text`) is the one [`crate::channel_lifecycle`]
//! already owns and the page walk is [`crate::recording_configuration_list`]'s, so no
//! suite invents its own way of talking to the emulator.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A delete removes the resource it names from state, so afterwards every reader
//!    — the read and the list alike — answers as if it had never been created, and
//!    the delete answers its modelled empty 204.
//! 2. A resource another resource still POINTS AT is not deleted: the call is refused
//!    with the modelled `ConflictException` (409) and state is left exactly as it
//!    was, so the only way through is the one the model documents — clear the
//!    reference first, then delete.
//! 3. An ARN the store does not hold names no resource, so the delete is the modelled
//!    `ResourceNotFoundException` (404), whether the store never minted it or held it
//!    until an earlier delete; an ARN outside the modelled pattern never reaches the
//!    handler at all and is a 400 `ValidationException`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::{
    resource_not_found_message, resource_referenced_message, RECORDING_CONFIGURATION_ACTIVATION,
    RECORDING_CONFIGURATION_ACTIVE, RECORDING_CONFIGURATION_CREATING,
};

use crate::channel_lifecycle::{text, Ivs, Reply};
use crate::recording_configuration_list::list_all;

/// A well-formed `RecordingConfigurationArn` no create ever minted: it satisfies the
/// modelled pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:recording-configuration/[a-zA-Z0-9-]+$`, so the
/// request reaches the handler and is refused on state rather than on its shape.
const ABSENT_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:recording-configuration/ffffffffffff";

/// The same, in another region: the modelled pattern pins the region segment only to
/// `[a-z0-9-]+`, so this too reaches the handler and names nothing.
const OTHER_REGION_ARN: &str =
    "arn:aws:ivs:eu-west-1:000000000000:recording-configuration/abcdef123456";

/// An S3 destination naming `bucket`, the only destination type the model declares.
fn destination(bucket: &str) -> Value {
    json!({ "s3": { "bucketName": bucket } })
}

/// The smallest create request there is: just the required destination.
fn minimal(bucket: &str) -> Value {
    json!({ "destinationConfiguration": destination(bucket) })
}

/// Create a recording configuration and return its ARN.
async fn create_configuration(ivs: &Ivs, request: Value) -> String {
    text(
        &ivs.ok("CreateRecordingConfiguration", request).await["recordingConfiguration"],
        "arn",
    )
}

/// What `GetRecordingConfiguration` answers for `arn`, success or failure.
async fn get(ivs: &Ivs, arn: &str) -> Reply {
    ivs.call("GetRecordingConfiguration", &json!({ "arn": arn }))
        .await
}

/// What `DeleteRecordingConfiguration` answers for `arn`, success or failure.
async fn delete(ivs: &Ivs, arn: &str) -> Reply {
    ivs.call("DeleteRecordingConfiguration", &json!({ "arn": arn }))
        .await
}

/// Whether the paginated list still answers `arn`.
async fn is_listed(ivs: &Ivs, arn: &str) -> bool {
    list_all(ivs)
        .await
        .iter()
        .any(|summary| text(summary, "arn") == arn)
}

/// The text the emulator stated, read from the key every SDK error parser reads.
fn message(reply: &Reply) -> String {
    text(&reply.body, "message")
}

/// Rule 1's second half: `arn` is gone from every reader, and asking again is the
/// same not-found as an ARN that never existed.
async fn assert_gone(ivs: &Ivs, arn: &str, case: &str) {
    let read = get(ivs, arn).await;
    assert_eq!(
        (read.status, read.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: after the delete the read must be 404 ResourceNotFoundException, \
         got {} {} {}",
        read.status,
        read.error_code,
        read.body
    );
    assert_eq!(
        message(&read),
        resource_not_found_message(arn),
        "case {case:?}: the read states this service's not-found rule, got {}",
        read.body
    );
    assert!(
        !is_listed(ivs, arn).await,
        "case {case:?}: after the delete the list must not answer {arn}"
    );
}

/// Rule 1's first half: `arn` is still readable and still listed.
async fn assert_present(ivs: &Ivs, arn: &str, case: &str) {
    let read = get(ivs, arn).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: {arn} must still be readable, got {} {}",
        read.status, read.body
    );
    assert!(
        is_listed(ivs, arn).await,
        "case {case:?}: {arn} must still be listed"
    );
}

/// Rule 1, over a table spanning the delete surface: whatever the configuration
/// looks like and whichever point of its own lifecycle it is at, the delete answers
/// the modelled empty 204 and the resource is observably gone afterwards — while
/// every neighbour, including a configuration another channel references, is
/// untouched.
#[tokio::test]
async fn an_unreferenced_recording_configuration_is_deleted_and_observably_gone() {
    // Rows differ along what the rule generalises over: which members the resource
    // was created with, the size of its `tags`, the shape of each nested
    // configuration, and whether the clock's `CREATING` → `ACTIVE` move has fallen
    // due by the time the delete arrives.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "only the required destination, deleted while CREATING",
            minimal("delete-one"),
            false,
        ),
        (
            "named and tagged, deleted once ACTIVE",
            json!({
                "name": "delete_two-x",
                "destinationConfiguration": destination("delete.two"),
                "tags": { "env": "test", "team": "video" },
            }),
            true,
        ),
        (
            "both nested configurations, deleted while CREATING",
            json!({
                "destinationConfiguration": destination("delete-three"),
                "renditionConfiguration": { "renditionSelection": "CUSTOM", "renditions": ["HD", "SD"] },
                "thumbnailConfiguration": { "recordingMode": "INTERVAL", "targetIntervalSeconds": 1 },
            }),
            false,
        ),
        (
            "thumbnails disabled and no renditions, deleted once ACTIVE",
            json!({
                "name": "delete-four",
                "destinationConfiguration": destination("delete-four"),
                "thumbnailConfiguration": { "recordingMode": "DISABLED" },
                "renditionConfiguration": { "renditionSelection": "NONE" },
            }),
            true,
        ),
        (
            "reconnect window at the top of the modelled range",
            json!({
                "destinationConfiguration": destination("delete-five"),
                "recordingReconnectWindowSeconds": 300,
            }),
            false,
        ),
    ];

    let ivs = Ivs::new();
    // A configuration a channel DOES reference, so every row below deletes beside a
    // resource rule 2 protects: what rule 2 refuses is the ARN the request named,
    // never "this store holds channels".
    let referenced = create_configuration(&ivs, minimal("sentinel")).await;
    let sentinel_channel = ivs
        .create_arn(json!({ "name": "sentinel", "recordingConfigurationArn": referenced }))
        .await;

    for (case, request, wait_for_active) in cases {
        let arn = create_configuration(&ivs, request).await;
        let state_before = if wait_for_active {
            tokio::time::sleep(RECORDING_CONFIGURATION_ACTIVATION + Duration::from_millis(250))
                .await;
            RECORDING_CONFIGURATION_ACTIVE
        } else {
            RECORDING_CONFIGURATION_CREATING
        };
        assert_eq!(
            text(&get(&ivs, &arn).await.body["recordingConfiguration"], "state"),
            state_before,
            "case {case:?}: the row deletes a configuration in {state_before}"
        );

        let removed = delete(&ivs, &arn).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: DeleteRecordingConfiguration must answer its modelled 204, got {} {}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.body,
            Value::Null,
            "case {case:?}: the modelled 204 carries an empty body, got {}",
            removed.body
        );

        assert_gone(&ivs, &arn, case).await;
        // Rule 3 on an ARN this store itself held: a second delete names a resource
        // the store no longer holds, so it is a not-found, not another 204.
        let again = delete(&ivs, &arn).await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: deleting it twice is a not-found the second time, got {} {} {}",
            again.status,
            again.error_code,
            again.body
        );
        assert_eq!(
            message(&again),
            resource_not_found_message(&arn),
            "case {case:?}: the second delete states this service's not-found rule, got {}",
            again.body
        );

        // The delete removed exactly the resource it named: the neighbour and the
        // channel that points at it are untouched.
        assert_present(&ivs, &referenced, case).await;
        assert_eq!(
            text(
                &ivs.ok("GetChannel", json!({ "arn": sentinel_channel })).await["channel"],
                "recordingConfigurationArn"
            ),
            referenced,
            "case {case:?}: an unrelated delete must not touch a channel's reference"
        );
    }
}

/// Rule 2, over a table spanning how a reference comes to exist and how many hold
/// it: while any channel still points at the configuration the delete is refused
/// with the modelled 409 and changes nothing, and once the last reference is cleared
/// — the workaround the model documents — the delete goes through.
#[tokio::test]
async fn a_recording_configuration_a_channel_still_references_cannot_be_deleted() {
    // Each row is the channels to stand up: the body each is created with, and
    // whether it takes the reference at create time or through a later
    // `UpdateChannel`. Rows differ in that route, in how many channels hold the
    // reference, and in the channels' own shapes.
    let cases: Vec<(&str, Vec<(Value, bool)>)> = vec![
        (
            "one channel, referencing from its create",
            vec![(json!({ "name": "from-create" }), false)],
        ),
        (
            "one channel, referencing from a later update",
            vec![(json!({ "name": "from-update" }), true)],
        ),
        (
            "two channels, one from each route",
            vec![
                (json!({ "name": "pair-create", "type": "BASIC" }), false),
                (json!({ "name": "pair-update", "type": "STANDARD" }), true),
            ],
        ),
        (
            "three channels of three types",
            vec![
                (json!({ "name": "trio-a", "type": "BASIC" }), false),
                (json!({ "name": "trio-b", "type": "ADVANCED_SD" }), true),
                (
                    json!({ "name": "trio-c", "type": "ADVANCED_HD", "authorized": true }),
                    false,
                ),
            ],
        ),
    ];

    for (case, channel_specs) in cases {
        // A fresh store per row, so "the first channel that still references it" is
        // decided by the row's own channels and nothing else.
        let ivs = Ivs::new();
        let arn = create_configuration(&ivs, minimal("referenced")).await;
        // A second configuration nothing references, to show the refusal is about
        // the ARN the request named rather than about the store holding references.
        let unreferenced = create_configuration(&ivs, minimal("unreferenced")).await;

        let mut channels = Vec::new();
        for (body, via_update) in &channel_specs {
            let mut request = body.clone();
            if !via_update {
                request["recordingConfigurationArn"] = json!(arn);
            }
            let channel_arn = ivs.create_arn(request).await;
            if *via_update {
                ivs.ok(
                    "UpdateChannel",
                    json!({ "arn": channel_arn, "recordingConfigurationArn": arn }),
                )
                .await;
            }
            channels.push(channel_arn);
        }
        // Sorted only so each row clears its references in one fixed order: the
        // refusal names no channel, so which one is cleared first cannot change it.
        channels.sort();

        let mut remaining = channels.clone();
        while !remaining.is_empty() {
            let refused = delete(&ivs, &arn).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (409, "ConflictException"),
                "case {case:?} with {} reference(s) left: expected 409 ConflictException, \
                 got {} {} {}",
                remaining.len(),
                refused.status,
                refused.error_code,
                refused.body
            );
            assert_eq!(
                message(&refused),
                resource_referenced_message("DeleteRecordingConfiguration", None, &arn),
                "case {case:?} with {} reference(s) left: the refusal states the attachment \
                 rule and names no channel, got {}",
                remaining.len(),
                refused.body
            );
            // The refusal removed nothing.
            assert_present(&ivs, &arn, case).await;

            // The workaround the model documents: "use UpdateChannel to set the
            // recordingConfigurationArn field to an empty string".
            let cleared = remaining.remove(0);
            let updated = ivs
                .ok(
                    "UpdateChannel",
                    json!({ "arn": cleared, "recordingConfigurationArn": "" }),
                )
                .await;
            assert_eq!(
                text(&updated["channel"], "recordingConfigurationArn"),
                "",
                "case {case:?}: clearing the reference must persist, got {updated}"
            );
        }

        // With the last reference cleared the delete the model documents goes through.
        let removed = delete(&ivs, &arn).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: with no reference left the delete answers its modelled 204, got {} {}",
            removed.status, removed.body
        );
        assert_gone(&ivs, &arn, case).await;

        // Every channel survived the refusals and the delete, naming no configuration.
        for channel_arn in &channels {
            let channel = ivs.ok("GetChannel", json!({ "arn": channel_arn })).await["channel"].clone();
            assert_eq!(
                text(&channel, "recordingConfigurationArn"),
                "",
                "case {case:?}: {channel_arn} must survive with no configuration named, \
                 got {channel}"
            );
        }
        // And the configuration nothing ever referenced deletes with no conflict at all.
        let other = delete(&ivs, &unreferenced).await;
        assert_eq!(
            other.status, 204,
            "case {case:?}: a configuration nothing references deletes straight away, got {} {}",
            other.status, other.body
        );
    }
}

/// Rule 3: an ARN the store does not hold is a 404 whichever way it came to not be
/// held, and an `arn` the modelled pattern rejects is a 400 that never reaches the
/// handler.
#[tokio::test]
async fn deleting_a_recording_configuration_the_store_never_held_is_not_found() {
    let ivs = Ivs::new();
    // Resources that DO exist, so every refusal below is about the ARN the request
    // named and not about an empty store.
    let existing = create_configuration(&ivs, minimal("existing")).await;
    ivs.create_arn(json!({ "name": "unrelated" })).await;
    let already_deleted = create_configuration(&ivs, minimal("already-deleted")).await;
    assert_eq!(
        delete(&ivs, &already_deleted).await.status,
        204,
        "the setup delete must succeed for this case to be meaningful"
    );

    let cases: Vec<(&str, String)> = vec![
        ("an ARN no create ever minted", ABSENT_ARN.to_string()),
        (
            "a well-formed ARN in another region",
            OTHER_REGION_ARN.to_string(),
        ),
        (
            "the ARN of a configuration already deleted",
            already_deleted.clone(),
        ),
    ];
    for (case, arn) in &cases {
        let reply = delete(&ivs, arn).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            message(&reply),
            resource_not_found_message(arn),
            "case {case:?}: the refusal states this service's not-found rule, got {}",
            reply.body
        );
    }

    // An `arn` outside the modelled `RecordingConfigurationArn` pattern is refused by
    // the framework's constraint walk before the handler runs: a 400, never a 404.
    let malformed: Vec<(&str, &str)> = vec![
        (
            "a channel ARN",
            "arn:aws:ivs:us-east-1:000000000000:channel/abcdef123456",
        ),
        ("not an ARN at all", "recording-configuration"),
        ("the empty string", ""),
    ];
    for (case, arn) in malformed {
        let reply = delete(&ivs, arn).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // None of the refusals touched the resource that does exist.
    assert_present(&ivs, &existing, "the resource that does exist").await;
}
