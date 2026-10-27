// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RemoveNotificationChannel`'s deletion contract.
//!
//! The RULE these tests pin: removing a notification channel is a TERMINAL, idempotent
//! deletion — the request carries nothing but the channel id and the response models no
//! member, so the call's whole contract is that no channel with that id is registered
//! afterwards. A removal therefore answers HTTP 200 with the empty output for EVERY
//! well-formed id, deleting the channel when one is registered under it and changing
//! nothing when none is (an id never minted, or one an earlier removal already freed),
//! and it never touches another channel.
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
    /// Channels minted beside the row's target, which every removal must leave alone.
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

/// What every removal of a well-formed id must look like: HTTP 200, the empty
/// `RemoveNotificationChannelResponse` body, and no error of any kind.
fn assert_empty_success(case: &str, attempt: usize, id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: removal {attempt} of {id:?} must succeed, got status {} body {}",
        reply.status, reply.raw
    );
    assert_eq!(
        reply.body,
        json!({}),
        "case {case:?}: removal {attempt} of {id:?} must report the empty output, got {}",
        reply.raw
    );
    assert!(
        reply.error_name.is_empty(),
        "case {case:?}: removal {attempt} of {id:?} must name no error, got {:?}",
        reply.error_name
    );
}

/// RULE: removing a notification channel is terminal and idempotent — every well-formed
/// id answers HTTP 200 with the empty output, the channel registered under that id (if
/// any) is gone afterwards, and an id naming no channel (never minted, already removed)
/// changes nothing and leaves every other channel exactly as it was.
#[tokio::test]
async fn removing_a_channel_is_an_idempotent_empty_success() {
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
        // Only a live channel's own id names something to delete; every other removal
        // succeeds without changing the store.
        let deletes_a_channel = matches!(case.target, Target::Minted);

        for attempt in 0..case.removals {
            let before = listed_channel_ids(&emulator).await;
            let reply = emulator.remove_notification_channel(&id).await;
            assert_empty_success(case.name, attempt, &id, &reply);

            let after = listed_channel_ids(&emulator).await;
            let expected_after: Vec<String> = if attempt == 0 && deletes_a_channel {
                before.iter().filter(|held| *held != &id).cloned().collect()
            } else {
                // Nothing was registered under this id any more, so the no-op removal
                // must leave the collection byte-for-byte as it was.
                before.clone()
            };
            assert_eq!(
                after, expected_after,
                "case {:?}: after removal {attempt} of {id:?} expected {expected_after:?} to be held, got {after:?}",
                case.name
            );
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
/// the id's resource, so the id then names nothing — removing it again is still an empty
/// success — while the topic may be registered again and gets a fresh id of its own.
///
/// The rows differ in the form of `Config.Filters` the registration carries, which the
/// rule generalises over.
#[tokio::test]
async fn a_removed_id_stays_removable_while_its_topic_can_be_registered_again() {
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
        assert_empty_success(name, 0, &id, &removed);

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

        // The removed id names nothing even though its topic backs a channel again, so
        // removing it again is a no-op success that leaves the new channel alone.
        let again = emulator.remove_notification_channel(&id).await;
        assert_empty_success(name, 1, &id, &again);

        let remaining = listed_channel_ids(&emulator).await;
        assert_eq!(
            remaining,
            vec![reused_id.clone()],
            "case {name:?}: expected only {reused_id:?} to remain, got {remaining:?}"
        );

        // And the fresh id does name a channel, so removing it deletes that channel and
        // every later removal of it is the same empty success.
        let fresh = emulator.remove_notification_channel(&reused_id).await;
        assert_empty_success(name, 0, &reused_id, &fresh);
        let fresh_again = emulator.remove_notification_channel(&reused_id).await;
        assert_empty_success(name, 1, &reused_id, &fresh_again);

        let empty = listed_channel_ids(&emulator).await;
        assert!(
            empty.is_empty(),
            "case {name:?}: expected no channels after both removals, got {empty:?}"
        );
    }
}
