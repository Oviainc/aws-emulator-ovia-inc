// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RemoveNotificationChannel`'s target resolution.
//!
//! The RULE these tests pin: a deletion addressed by the identifier of a stored
//! resource acts on the resource that identifier names, so an identifier naming no
//! stored resource is refused with the modelled `ResourceNotFoundException` (HTTP 404)
//! carrying that identifier and its resource type, and changes nothing — only a call
//! that really deleted the resource answers HTTP 200 with the empty output. For a
//! notification channel the identifier is the `NotificationChannelId`
//! `AddNotificationChannel` minted, so the FIRST removal of a channel succeeds and
//! every later removal of that id, like a removal of an id this account never minted,
//! is refused.
//!
//! The rows differ along what the rule generalises over: where the requested id comes
//! from (minted and live, minted and already removed, never minted, one hex digit away
//! from a live one), how many removals are issued, and how many other channels exist
//! alongside — the issue report's "remove the same id twice" is one row.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same `MultiServiceRouter` the emulator boots — through the harness the
//! configuration-lifecycle suite already defines
//! ([`crate::configuration_lifecycle::Emulator`]).

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::NOTIFICATION_CHANNEL_RESOURCE_TYPE;

use crate::configuration_lifecycle::{Emulator, Reply};
use crate::mutation_idempotency_and_tenancy::{channel_config, listed_channel_ids, minted_id};

/// Where the id a row removes comes from.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// The id of the channel this row minted, so the first removal deletes it.
    Minted,
    /// A well-formed `NotificationChannelId` this account never minted.
    NeverMinted(&'static str),
    /// The minted channel's id with its last hex digit changed: well formed, one
    /// character away from a live channel, and naming none.
    OneDigitOff,
}

struct Case {
    name: &'static str,
    /// Channels minted beside the row's target, which every refusal must leave alone.
    bystanders: usize,
    /// Whether the row mints the channel its target is derived from.
    mint: bool,
    target: Target,
    /// How many removals the row issues for its target.
    removals: usize,
}

/// A `NotificationChannelId` that is well formed (36 chars, lowercase hex, the model's
/// `NotificationChannelId` pattern) and that this account never minted.
const UNMINTED_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Another of those, so a row can name an id unrelated to the first.
const OTHER_UNMINTED_ID: &str = "ffffffff-ffff-4fff-bfff-ffffffffffff";

/// The target id with its last hex digit swapped for a different one, keeping the
/// `NotificationChannelId` pattern.
fn one_digit_off(id: &str) -> String {
    let mut altered = id.to_string();
    let last = altered.pop().unwrap_or('0');
    altered.push(if last == '0' { '1' } else { '0' });
    altered
}

/// The `ResourceNotFoundException` a refused removal must carry, about `id`.
fn assert_refused(case: &str, attempt: usize, id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: removal {attempt} of {id:?} must be refused with 404, got status {} body {}",
        reply.status, reply.raw
    );
    assert!(
        reply.names_error("ResourceNotFoundException"),
        "case {case:?}: removal {attempt} of {id:?} must name a ResourceNotFoundException, got {}",
        reply.raw
    );
    let resource_id = reply.at("/ResourceId").and_then(Value::as_str);
    assert_eq!(
        resource_id,
        Some(id),
        "case {case:?}: removal {attempt} must report ResourceId {id:?}, got {resource_id:?}"
    );
    let resource_type = reply.at("/ResourceType").and_then(Value::as_str);
    assert_eq!(
        resource_type,
        Some(NOTIFICATION_CHANNEL_RESOURCE_TYPE),
        "case {case:?}: removal {attempt} must report ResourceType \
         {NOTIFICATION_CHANNEL_RESOURCE_TYPE:?}, got {resource_type:?}"
    );
    let message = reply
        .at("/Message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains(id),
        "case {case:?}: removal {attempt} must name {id:?} in its message, got {message:?}"
    );
}

/// RULE: a removal addressed by a resource identifier acts on the resource that
/// identifier names — the first removal of a channel succeeds with the empty output, and
/// an id that names no stored channel (never minted, or already removed) is refused with
/// `ResourceNotFoundException` (404) naming the id and the channel resource type,
/// leaving every other channel exactly as it was.
#[tokio::test]
async fn removing_an_id_that_names_no_channel_is_refused_as_not_found() {
    let cases = [
        Case {
            name: "an id that was never minted, over an empty store",
            bystanders: 0,
            mint: false,
            target: Target::NeverMinted(UNMINTED_ID),
            removals: 1,
        },
        Case {
            name: "a minted id removed twice",
            bystanders: 0,
            mint: true,
            target: Target::Minted,
            removals: 2,
        },
        Case {
            name: "a minted id removed four times",
            bystanders: 0,
            mint: true,
            target: Target::Minted,
            removals: 4,
        },
        Case {
            name: "an id never minted, removed three times beside two channels",
            bystanders: 2,
            mint: false,
            target: Target::NeverMinted(OTHER_UNMINTED_ID),
            removals: 3,
        },
        Case {
            name: "a minted id removed twice beside another channel",
            bystanders: 1,
            mint: true,
            target: Target::Minted,
            removals: 2,
        },
        Case {
            name: "an id one hex digit away from a live channel",
            bystanders: 1,
            mint: true,
            target: Target::OneDigitOff,
            removals: 2,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();

        let mut survivors: Vec<String> = Vec::new();
        for bystander in 0..case.bystanders {
            let reply = emulator
                .add_notification_channel(channel_config(
                    &format!("arn:aws:sns:us-east-1:000000000000:guru-keep-me-{bystander}"),
                    &json!(null),
                ))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the bystander add {bystander} must succeed, got {}",
                case.name, reply.raw
            );
            survivors.push(minted_id(&reply));
        }

        let minted = if case.mint {
            let reply = emulator
                .add_notification_channel(channel_config(
                    "arn:aws:sns:us-east-1:000000000000:guru-doomed",
                    &json!(null),
                ))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the add must succeed, got {}",
                case.name, reply.raw
            );
            Some(minted_id(&reply))
        } else {
            None
        };

        let id = match case.target {
            Target::Minted => minted.clone().expect("a Minted target mints its channel"),
            Target::NeverMinted(id) => id.to_string(),
            Target::OneDigitOff => {
                one_digit_off(minted.as_deref().expect("a OneDigitOff target mints a channel"))
            }
        };
        // Only a live channel's own id names something to delete; that is the single
        // removal that succeeds.
        let deletes_a_channel = matches!(case.target, Target::Minted);

        for attempt in 0..case.removals {
            let before = listed_channel_ids(&emulator).await;
            let reply = emulator.remove_notification_channel(&id).await;
            if attempt == 0 && deletes_a_channel {
                assert_eq!(
                    reply.status, 200,
                    "case {:?}: the first removal of a live channel must succeed, got status {} body {}",
                    case.name, reply.status, reply.raw
                );
                assert_eq!(
                    reply.body,
                    json!({}),
                    "case {:?}: a successful removal must report the empty output, got {}",
                    case.name,
                    reply.raw
                );
            } else {
                assert_refused(case.name, attempt, &id, &reply);
                let after = listed_channel_ids(&emulator).await;
                assert_eq!(
                    after, before,
                    "case {:?}: removal {attempt} was refused, so the store must be unchanged: expected {before:?}, got {after:?}",
                    case.name
                );
            }
        }

        let remaining = listed_channel_ids(&emulator).await;
        let mut expected = survivors.clone();
        if let Some(minted) = minted.as_ref() {
            if !deletes_a_channel {
                expected.push(minted.clone());
            }
        }
        expected.sort();
        let mut actual = remaining.clone();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?} to remain, got {actual:?}",
            case.name
        );
    }
}

/// RULE (the same rule, from the other side): a removal that DID delete a channel frees
/// the id's resource, so the id then names nothing and the topic may be registered
/// again — the re-added topic gets a fresh id, and the removed id stays refused.
///
/// The rows differ in the form of `Config.Filters` the registration carries, which the
/// rule generalises over.
#[tokio::test]
async fn a_removed_id_stays_refused_while_its_topic_can_be_registered_again() {
    let cases: [(&str, Value); 4] = [
        ("no filters", json!(null)),
        ("severities only", json!({ "Severities": ["HIGH"] })),
        (
            "message types only",
            json!({ "MessageTypes": ["NEW_INSIGHT"] }),
        ),
        (
            "both filter lists",
            json!({ "Severities": ["LOW"], "MessageTypes": ["CLOSED_INSIGHT"] }),
        ),
    ];

    for (name, filters) in cases {
        let emulator = Emulator::new();
        let topic_arn = "arn:aws:sns:us-east-1:000000000000:guru-recycled";
        let config = channel_config(topic_arn, &filters);

        let added = emulator.add_notification_channel(config.clone()).await;
        assert_eq!(
            added.status, 200,
            "case {name:?}: the add must succeed, got {}",
            added.raw
        );
        let id = minted_id(&added);

        let removed = emulator.remove_notification_channel(&id).await;
        assert_eq!(
            removed.status, 200,
            "case {name:?}: the removal must succeed, got {}",
            removed.raw
        );

        let reused = emulator.add_notification_channel(config.clone()).await;
        assert_eq!(
            reused.status, 200,
            "case {name:?}: re-adding the freed topic must succeed, got {}",
            reused.raw
        );
        let reused_id = minted_id(&reused);
        assert_ne!(
            reused_id, id,
            "case {name:?}: a re-added topic must get a fresh id, got the removed {id:?}"
        );

        // The removed id names nothing even though its topic backs a channel again.
        let again = emulator.remove_notification_channel(&id).await;
        assert_refused(name, 1, &id, &again);

        let remaining = listed_channel_ids(&emulator).await;
        assert_eq!(
            remaining,
            vec![reused_id.clone()],
            "case {name:?}: expected only {reused_id:?} to remain, got {remaining:?}"
        );

        // And the fresh id does name a channel, so removing it succeeds exactly once.
        let fresh = emulator.remove_notification_channel(&reused_id).await;
        assert_eq!(
            fresh.status, 200,
            "case {name:?}: removing the re-added channel must succeed, got {}",
            fresh.raw
        );
        let fresh_again = emulator.remove_notification_channel(&reused_id).await;
        assert_refused(name, 1, &reused_id, &fresh_again);
    }
}
