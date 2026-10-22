// SPDX-License-Identifier: CC0-1.0

//! This crate's model does not contend for the `events` key: it answers under its own
//! sdkId-derived one, out of its own store.
//!
//! This model and the EventBridge model are one AWS service on the wire. Both declare
//! service shape `AWSEvents` version `2015-10-07` on `awsJson1_1`, both route `POST /` by
//! `X-Amz-Target: AWSEvents.<OperationName>`, and both publish signing name `events`,
//! endpoint prefix `events` and ARN namespace `events` (each crate's `OPERATIONS.md`,
//! "Wire protocol"); this model's 51 operations are a subset of the EventBridge model's
//! 57, which adds the Endpoints surface and `UpdateEventBus`. Nothing in a request
//! distinguishes them, so while both claimed the `events` key the router had two
//! claimants for every such request and refused all of them with `500 AmbiguousService`
//! — leaving the whole surface unreachable.
//!
//! The `events` key is therefore owned by the EventBridge crate (the endpoint's current
//! name, and the superset model) and this model answers under its own key
//! (`src/service.rs`'s `OWN_SDK_ID_KEY`). What THIS suite pins, with only this model
//! mounted, is both halves of that from this crate's side: a request signed for its own
//! key is served here and lands in THIS store, and a request signed for `events` is
//! claimed by nothing here at all — so it can never be a second claimant of that key.
//! The complementary claim, that every `events`-signed request is served by exactly one
//! of the models that publish this wire identity and lands in THAT model's store, needs
//! every claimant mounted and lives in
//! `core/router/tests/all/events_store_ownership.rs`.

use serde_json::json;
use service_com_amazonaws_cloudwatchevents_awsevents::state::ComAmazonawsCloudwatcheventsAwseventsState;

use crate::support::{emulator, Emulator, DEFAULT_BUS, SIGNED_FOR_EVENTS, SIGNED_FOR_LEGACY};

/// The rules this model's own store holds on the default bus, in name order.
fn stored_rules(emulator: &Emulator) -> Vec<String> {
    let state = emulator
        .engine
        .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
    let store = state.lock();
    store
        .bus(DEFAULT_BUS)
        .map(|bus| bus.rules.keys().cloned().collect())
        .unwrap_or_default()
}

/// A request signed for THIS model's own key is served by it, out of its own store; the
/// rows differ along the rule each call writes, and each is read back through this
/// model's own reporting operation under the same key.
#[tokio::test]
async fn its_own_key_is_served_out_of_its_own_store() {
    let emulator = emulator();
    let rules: [(&str, &str); 3] = [
        ("legacy-pattern-rule", "my.app"),
        ("legacy.second-rule", "other.app"),
        ("legacy_third_rule", "third.app"),
    ];

    for (rule, source) in rules {
        let pattern = json!({ "source": [source] }).to_string();
        let answer = emulator
            .call_signed(
                SIGNED_FOR_LEGACY,
                "PutRule",
                json!({ "Name": rule, "EventPattern": pattern }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (200, ""),
            "case {rule:?}: this model must serve its own key, got {} {:?}",
            answer.status,
            answer.body
        );
        let described = emulator
            .call_signed(SIGNED_FOR_LEGACY, "DescribeRule", json!({ "Name": rule }))
            .await;
        assert_eq!(
            (described.status, described.body["EventPattern"].clone()),
            (200, json!(pattern)),
            "case {rule:?}: the rule must read back as stored, got {} {:?}",
            described.status,
            described.body
        );
    }

    let mut expected: Vec<String> = rules.iter().map(|(rule, _)| (*rule).to_string()).collect();
    expected.sort();
    assert_eq!(
        stored_rules(&emulator),
        expected,
        "every rule written under this model's own key must be in ITS store"
    );
}

/// A request signed for the `events` key is claimed by NOTHING here: with this model the
/// only service mounted, the router answers its own `404 UnknownService` and this store
/// stays empty. That is what stops this model being a second claimant of a key the
/// EventBridge model owns — the refusal the issue reported (`500 AmbiguousService`) is
/// exactly what two claimants produce.
///
/// The rows span this model's own surface: an operation it implements, a read, the data
/// plane, and one it leaves scaffolded — the claim is about the KEY, so none of them may
/// be answered here.
#[tokio::test]
async fn the_events_key_is_claimed_by_nothing_here() {
    let emulator = emulator();
    let rows: [(&str, serde_json::Value); 5] = [
        (
            "PutRule",
            json!({ "Name": "events-key-rule", "EventPattern": r#"{"source":["my.app"]}"# }),
        ),
        ("DescribeRule", json!({ "Name": "events-key-rule" })),
        ("ListRules", json!({})),
        (
            "PutEvents",
            json!({ "Entries": [{ "Source": "my.app", "Detail": "{}" }] }),
        ),
        ("CreateEventBus", json!({ "Name": "events-key-bus" })),
    ];

    for (operation, body) in rows {
        let answer = emulator
            .call_signed(SIGNED_FOR_EVENTS, operation, body)
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "UnknownService"),
            "case {operation:?}: this model must not claim the `events` key, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    assert_eq!(
        stored_rules(&emulator),
        Vec::<String>::new(),
        "a request this model never claimed cannot have written to its store"
    );
}
