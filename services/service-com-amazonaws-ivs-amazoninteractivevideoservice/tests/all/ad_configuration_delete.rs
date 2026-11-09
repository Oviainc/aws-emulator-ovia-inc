// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAdConfiguration`, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! dispatch harness (`Ivs`, `Reply`, `text`) is the one [`crate::channel_lifecycle`]
//! already owns and the page walk is [`crate::ad_configuration_list`]'s, so no suite
//! invents its own way of talking to the emulator.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A delete removes the resource it names from state, so afterwards every reader —
//!    the read and the paginated list alike — answers as if it had never been created,
//!    and the delete itself answers its modelled empty 204.
//! 2. A resource another resource still POINTS AT is not deleted: the call is refused
//!    with the modelled `ConflictException` (409) and state is left exactly as it was,
//!    so the only way through is to clear the reference first and then delete.
//! 3. An ARN the store does not hold names no resource, so the delete is the modelled
//!    `ResourceNotFoundException` (404), whether the store never minted it or held it
//!    until an earlier delete; an ARN outside the modelled `AdConfigurationArn`
//!    pattern never reaches the handler at all and is a 400 `ValidationException`.

use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::{
    resource_not_found_message, resource_referenced_message,
};

use crate::ad_configuration_list::list_all;
use crate::channel_lifecycle::{text, Ivs, Reply};

/// A well-formed `AdConfigurationArn` no create ever minted: it satisfies the modelled
/// pattern `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ad-configuration/[a-zA-Z0-9-]+$`, so the
/// request reaches the handler and is refused on state rather than on its shape.
const ABSENT_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:ad-configuration/ffffffffffff";

/// The same, in another region and for another account: the modelled pattern pins
/// those segments only to `[a-z0-9-]+` and `[0-9]+`, so these too reach the handler
/// and name nothing.
const OTHER_REGION_ARN: &str = "arn:aws:ivs:eu-west-1:000000000000:ad-configuration/abcdef123456";
const OTHER_ACCOUNT_ARN: &str = "arn:aws:ivs:us-east-1:123456789012:ad-configuration/abcdef123456";

/// One MediaTailor integration entry, of the form the modelled
/// `MediaTailorPlaybackConfigurationArn` pattern
/// `^arn:aws:mediatailor:[a-z0-9-]+:[0-9]+:playbackConfiguration/[a-zA-Z0-9-]+$` pins,
/// so every request below satisfies its own constraints.
fn playback_entry(suffix: &str) -> Value {
    json!({
        "playbackConfigurationArn":
            format!("arn:aws:mediatailor:us-east-1:000000000000:playbackConfiguration/{suffix}"),
    })
}

/// The smallest create request there is: just the required integration list.
fn minimal(suffix: &str) -> Value {
    json!({ "mediaTailorPlaybackConfigurations": [playback_entry(suffix)] })
}

/// Create an ad configuration and return its ARN.
async fn create_configuration(ivs: &Ivs, request: Value) -> String {
    text(
        &ivs.ok("CreateAdConfiguration", request).await["adConfiguration"],
        "arn",
    )
}

/// What `GetAdConfiguration` answers for `arn`, success or failure.
async fn get(ivs: &Ivs, arn: &str) -> Reply {
    ivs.call("GetAdConfiguration", &json!({ "arn": arn })).await
}

/// What `DeleteAdConfiguration` answers for `arn`, success or failure.
async fn delete(ivs: &Ivs, arn: &str) -> Reply {
    ivs.call("DeleteAdConfiguration", &json!({ "arn": arn }))
        .await
}

/// Whether the paginated list still answers `arn`.
async fn is_listed(ivs: &Ivs, arn: &str) -> bool {
    list_all(ivs, None)
        .await
        .iter()
        .any(|summary| text(summary, "arn") == arn)
}

/// The text the emulator stated, read from the key every SDK error parser reads.
fn message(reply: &Reply) -> String {
    text(&reply.body, "message")
}

/// Rule 1's second half: `arn` is gone from every reader, and asking again is the same
/// not-found as an ARN that never existed.
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

/// Rule 1, over a table spanning the delete surface: whatever the ad configuration
/// looks like, the delete answers the modelled empty 204 and the resource is observably
/// gone afterwards — while every neighbour, including one a channel references, is
/// untouched.
#[tokio::test]
async fn an_unreferenced_ad_configuration_is_deleted_and_observably_gone() {
    // Rows differ along what the rule generalises over: how many MediaTailor entries
    // the required list carries (the modelled 1..3), whether `name`,
    // `postRollConfiguration` and `tags` are sent at all, and the values they take.
    let cases: Vec<(&str, Value)> = vec![
        ("only the required list, one entry", minimal("delete-one")),
        (
            "named and tagged, two entries",
            json!({
                "name": "delete_two-x",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("two-a"), playback_entry("two-b"),
                ],
                "tags": { "env": "test", "team": "video" },
            }),
        ),
        (
            "three entries (the modelled maximum) and post-roll enabled",
            json!({
                "name": "delete-three",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("three-a"), playback_entry("three-b"), playback_entry("three-c"),
                ],
                "postRollConfiguration": { "durationSeconds": 1, "enabled": true },
            }),
        ),
        (
            "post-roll at the upper bound of the modelled AdDurationSeconds range",
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("four")],
                "postRollConfiguration": { "durationSeconds": 300, "enabled": false },
            }),
        ),
        (
            "a name at the modelled 128-character maximum",
            json!({
                "name": "aZ9-_".repeat(26)[..128].to_string(),
                "mediaTailorPlaybackConfigurations": [playback_entry("five")],
                "tags": {},
            }),
        ),
    ];

    let ivs = Ivs::new();
    // An ad configuration a channel DOES reference, so every row below deletes beside
    // a resource rule 2 protects: what rule 2 refuses is the ARN the request named,
    // never "this store holds channels".
    let referenced = create_configuration(&ivs, minimal("sentinel")).await;
    let sentinel_channel = ivs
        .create_arn(json!({ "name": "sentinel", "adConfigurationArn": referenced }))
        .await;

    for (case, request) in cases {
        let arn = create_configuration(&ivs, request).await;
        assert_present(&ivs, &arn, case).await;

        let removed = delete(&ivs, &arn).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: DeleteAdConfiguration must answer its modelled 204, got {} {}",
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
                "adConfigurationArn"
            ),
            referenced,
            "case {case:?}: an unrelated delete must not touch a channel's reference"
        );
    }
}

/// Rule 2, over a table spanning how a reference comes to exist and how many hold it:
/// while any channel still points at the ad configuration the delete is refused with
/// the modelled 409 and changes nothing, and once the last reference is cleared the
/// delete goes through.
#[tokio::test]
async fn an_ad_configuration_a_channel_still_references_cannot_be_deleted() {
    // Each row is the channels to stand up: the body each is created with, and whether
    // it takes the reference at create time or through a later `UpdateChannel`. Rows
    // differ in that route, in how many channels hold the reference, and in the
    // channels' own shapes.
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
        // A second ad configuration nothing references, to show the refusal is about
        // the ARN the request named rather than about the store holding references.
        let unreferenced = create_configuration(&ivs, minimal("unreferenced")).await;

        let mut channels = Vec::new();
        for (body, via_update) in &channel_specs {
            let mut request = body.clone();
            if !via_update {
                request["adConfigurationArn"] = json!(arn);
            }
            let channel_arn = ivs.create_arn(request).await;
            if *via_update {
                ivs.ok(
                    "UpdateChannel",
                    json!({ "arn": channel_arn, "adConfigurationArn": arn }),
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
                resource_referenced_message(
                    "DeleteAdConfiguration",
                    Some("ad configuration"),
                    &arn
                ),
                "case {case:?} with {} reference(s) left: the refusal states the attachment \
                 rule and names no channel, got {}",
                remaining.len(),
                refused.body
            );
            // The refusal removed nothing.
            assert_present(&ivs, &arn, case).await;

            // Clearing the reference: the modelled `ChannelAdConfigurationArn` pattern
            // `^^$|^arn:...$` admits the empty string, which is its documented
            // "no ad configuration is associated" value.
            let cleared = remaining.remove(0);
            let updated = ivs
                .ok(
                    "UpdateChannel",
                    json!({ "arn": cleared, "adConfigurationArn": "" }),
                )
                .await;
            assert_eq!(
                text(&updated["channel"], "adConfigurationArn"),
                "",
                "case {case:?}: clearing the reference must persist, got {updated}"
            );
        }

        // With the last reference cleared the delete goes through.
        let removed = delete(&ivs, &arn).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: with no reference left the delete answers its modelled 204, got {} {}",
            removed.status, removed.body
        );
        assert_gone(&ivs, &arn, case).await;

        // Every channel survived the refusals and the delete, naming no ad configuration.
        for channel_arn in &channels {
            let channel =
                ivs.ok("GetChannel", json!({ "arn": channel_arn })).await["channel"].clone();
            assert_eq!(
                text(&channel, "adConfigurationArn"),
                "",
                "case {case:?}: {channel_arn} must survive with no ad configuration named, \
                 got {channel}"
            );
        }
        // And the ad configuration nothing ever referenced deletes with no conflict.
        let other = delete(&ivs, &unreferenced).await;
        assert_eq!(
            other.status, 204,
            "case {case:?}: a configuration nothing references deletes straight away, got {} {}",
            other.status, other.body
        );
    }
}

/// Rule 2's ordering half: a resource the store does not hold is a not-found even when
/// a channel names that very ARN, because "does this exist?" is decided before "is
/// anything still pointing at it?".
#[tokio::test]
async fn a_reference_to_an_absent_ad_configuration_is_still_not_found() {
    let ivs = Ivs::new();
    // A channel pointing at a well-formed ARN the store never minted: the create
    // stores the reference as sent, so the delete below meets a reference AND an
    // absent resource at once.
    let channel = ivs
        .create_arn(json!({ "name": "dangling", "adConfigurationArn": ABSENT_ARN }))
        .await;

    let reply = delete(&ivs, ABSENT_ARN).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case a referenced but absent arn: expected 404 ResourceNotFoundException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        message(&reply),
        resource_not_found_message(ABSENT_ARN),
        "case a referenced but absent arn: the refusal states this service's not-found rule, \
         got {}",
        reply.body
    );
    assert_eq!(
        text(
            &ivs.ok("GetChannel", json!({ "arn": channel })).await["channel"],
            "adConfigurationArn"
        ),
        ABSENT_ARN,
        "case a referenced but absent arn: the refusal must not touch the channel"
    );
}

/// Rule 3: an ARN the store does not hold is a 404 whichever way it came to not be
/// held, and an `arn` the modelled pattern rejects is a 400 that never reaches the
/// handler.
#[tokio::test]
async fn deleting_an_ad_configuration_the_store_never_held_is_not_found() {
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
            "a well-formed ARN for another account",
            OTHER_ACCOUNT_ARN.to_string(),
        ),
        (
            "the ARN of an ad configuration already deleted",
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

    // An `arn` outside the modelled `AdConfigurationArn` pattern is refused by the
    // framework's constraint walk before the handler runs: a 400, never a 404.
    let malformed: Vec<(&str, &str)> = vec![
        (
            "a recording configuration ARN",
            "arn:aws:ivs:us-east-1:000000000000:recording-configuration/abcdef123456",
        ),
        (
            "a channel ARN",
            "arn:aws:ivs:us-east-1:000000000000:channel/abcdef123456",
        ),
        (
            "an upper-case region segment",
            "arn:aws:ivs:US-EAST-1:000000000000:ad-configuration/abcdef123456",
        ),
        (
            "an id outside [a-zA-Z0-9-]+",
            "arn:aws:ivs:us-east-1:000000000000:ad-configuration/has_underscore",
        ),
        ("not an ARN at all", "ad-configuration"),
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

    // A missing required member is the same refusal class.
    let absent = ivs.call("DeleteAdConfiguration", &json!({})).await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (400, "ValidationException"),
        "case absent arn: expected 400 ValidationException, got {} {} {}",
        absent.status,
        absent.error_code,
        absent.body
    );

    // None of the refusals touched the resource that does exist.
    assert_present(&ivs, &existing, "the resource that does exist").await;
}
