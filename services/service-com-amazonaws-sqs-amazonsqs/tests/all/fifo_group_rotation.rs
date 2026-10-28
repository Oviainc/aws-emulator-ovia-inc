// SPDX-License-Identifier: CC0-1.0

//! How a FIFO receive chooses WHICH message group to serve, driven over the wire
//! (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: a FIFO receive serves the HEADS of the eligible message groups and
//! rotates over them, so every group that is not blocked is reached within as many
//! single-message receives as the queue has groups. Order is defined WITHIN a message group
//! only — the cross-group interleaving is the queue's to choose — but reachability is not
//! optional: a consumer that hands back each message it is not ready for
//! (`ChangeMessageVisibility` with `VisibilityTimeout: 0`, the normal way to implement
//! selective consumption) must still make progress past the group it was handed first.
//! Serving the globally oldest visible message on every call satisfies the per-group order
//! and still starves every group behind the first one, because the message just handed back
//! is the oldest again.
//!
//! The tables span what the rule generalises over: the number of groups (2, 5 and 20) and
//! the number of messages each carries, which group is held in flight, and whether the
//! poller asks for one message or ten. A standard queue is included where the rule's
//! counterpart holds — it orders nothing, so it has no group to rotate over and must still
//! hand everything over.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::support::{create_ok, emulator, fifo_attributes, messages, ok};
use smithy_server_runtime::MultiServiceRouter;

/// The `VisibilityTimeout` a row's queue is created with: long enough that nothing a row
/// holds in flight comes back on its own, so what a poller sees is the rotation and not a
/// lapsing timeout.
const HELD_FOR_SECONDS: &str = "600";

/// `(group, body)` for `groups` groups of `per_group` messages each, interleaved so
/// consecutive sends never share a group — the arrival order that tells a rotation apart
/// from a walk over the arrival order.
fn interleaved(groups: usize, per_group: usize) -> Vec<(String, String)> {
    let mut sends = Vec::with_capacity(groups * per_group);
    for index in 0..per_group {
        for group in 0..groups {
            sends.push((format!("g{group:02}"), format!("g{group:02}-m{index:02}")));
        }
    }
    sends
}

/// Send `sends` to the FIFO queue at `url`, one message at a time, in order.
async fn send_each(router: &MultiServiceRouter, url: &str, sends: &[(String, String)]) {
    for (group, body) in sends {
        ok(
            router,
            "SendMessage",
            json!({
                "QueueUrl": url,
                "MessageBody": body,
                "MessageGroupId": group,
                "MessageDeduplicationId": body,
            }),
        )
        .await;
    }
}

/// Receive up to `max` messages, selecting the group each one belongs to, and answer
/// `(group, body, receipt handle)` per message.
async fn receive(
    router: &MultiServiceRouter,
    url: &str,
    max: i64,
) -> Vec<(String, String, String)> {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": max,
            "MessageSystemAttributeNames": ["MessageGroupId"],
        }),
    )
    .await;
    messages(&reply)
        .iter()
        .map(|message| {
            (
                message
                    .get("Attributes")
                    .and_then(|attributes| attributes.get("MessageGroupId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                message["Body"].as_str().unwrap_or_default().to_string(),
                message["ReceiptHandle"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Hand a message back to the queue at once, which is how a consumer declines one it is
/// not ready for.
async fn hand_back(router: &MultiServiceRouter, url: &str, receipt_handle: &str) {
    ok(
        router,
        "ChangeMessageVisibility",
        json!({
            "QueueUrl": url,
            "ReceiptHandle": receipt_handle,
            "VisibilityTimeout": 0,
        }),
    )
    .await;
}

/// With one group's head held in flight, a poller that hands every other message straight
/// back reaches EVERY remaining group, and does so within as many receives as the queue has
/// groups per group it is looking for.
#[tokio::test]
async fn a_poller_handing_messages_back_reaches_every_unblocked_group() {
    // (case, groups, messages per group, how many messages one receive asks for)
    let rows: &[(&str, usize, usize, i64)] = &[
        ("two groups of one", 2, 1, 1),
        ("five groups of three", 5, 3, 1),
        ("twenty groups of five", 20, 5, 1),
        // The same rule when a receive may take a whole batch: the groups it rotates over
        // are the eligible ones either way.
        ("five groups of three, ten at a time", 5, 3, 10),
    ];

    for (case, groups, per_group, max) in rows.iter().copied() {
        let router = emulator();
        let url = create_ok(
            &router,
            &format!("rotation-{groups}x{per_group}-{max}.fifo"),
            &fifo_attributes(&[("VisibilityTimeout", HELD_FOR_SECONDS)]),
        )
        .await;
        send_each(&router, &url, &interleaved(groups, per_group)).await;

        // The first group's head is held in flight by a consumer that keeps it, which is
        // what blocks exactly that group.
        let held = receive(&router, &url, 1).await;
        assert_eq!(
            held.len(),
            1,
            "case {case}: expected the first receive to hand one message over, got {held:?}"
        );
        let blocked = held[0].0.clone();

        // Every other group has to be reachable. The poller hands back everything, so
        // nothing is consumed and only the GROUP SELECTION can make progress.
        let wanted: BTreeSet<String> = (0..groups)
            .map(|group| format!("g{group:02}"))
            .filter(|group| *group != blocked)
            .collect();
        // One receive per group is the least a rotation can need; the bound here is two
        // passes over the groups, which a rotation always meets and a queue serving the
        // globally oldest message never does.
        let budget = wanted.len() * 2;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for _ in 0..budget {
            if seen.len() == wanted.len() {
                break;
            }
            for (group, _, receipt_handle) in receive(&router, &url, max).await {
                seen.insert(group);
                hand_back(&router, &url, &receipt_handle).await;
            }
        }
        assert_eq!(
            seen, wanted,
            "case {case}: {budget} receives must reach every unblocked group, and the \
             blocked one ({blocked}) must stay blocked"
        );
    }
}

/// Rotating over the groups does not reorder any of them: a full drain hands every group's
/// messages over in send order, and the group whose head is in flight hands over nothing
/// more until that head is released.
#[tokio::test]
async fn rotation_keeps_each_group_in_send_order() {
    const GROUPS: usize = 5;
    const PER_GROUP: usize = 4;

    let router = emulator();
    let url = create_ok(
        &router,
        "rotation-order.fifo",
        &fifo_attributes(&[("VisibilityTimeout", HELD_FOR_SECONDS)]),
    )
    .await;
    let sends = interleaved(GROUPS, PER_GROUP);
    send_each(&router, &url, &sends).await;

    // Drain the queue one message at a time, deleting each one so the group behind it
    // becomes eligible.
    let mut handed: Vec<(String, String)> = Vec::with_capacity(sends.len());
    for _ in 0..sends.len() {
        let delivered = receive(&router, &url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case the drain: expected one message per receive, got {delivered:?}"
        );
        let (group, body, receipt_handle) = delivered[0].clone();
        handed.push((group, body));
        ok(
            &router,
            "DeleteMessage",
            json!({"QueueUrl": url, "ReceiptHandle": receipt_handle}),
        )
        .await;
    }
    assert_eq!(
        handed.len(),
        sends.len(),
        "case the drain: every message must be handed over exactly once"
    );

    // Per group, the order handed over is the order sent.
    let mut expected: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (group, body) in &sends {
        expected.entry(group.clone()).or_default().push(body.clone());
    }
    let mut actual: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (group, body) in &handed {
        actual.entry(group.clone()).or_default().push(body.clone());
    }
    assert_eq!(
        actual, expected,
        "case the drain: each group's messages must come over in send order"
    );

    // The rotation also spreads the first messages over the groups rather than draining
    // one group first: the first `GROUPS` messages handed over name `GROUPS` groups.
    let first_groups: BTreeSet<&str> = handed
        .iter()
        .take(GROUPS)
        .map(|(group, _)| group.as_str())
        .collect();
    assert_eq!(
        first_groups.len(),
        GROUPS,
        "case the drain: the first {GROUPS} deliveries must come from {GROUPS} different \
         groups, got {handed:?}"
    );
}

/// A standard queue has no group order to keep, so holding one of its messages in flight
/// blocks nothing: the rest are handed over whatever group they name.
#[tokio::test]
async fn a_standard_queue_hands_over_every_visible_message_whatever_group_it_names() {
    let router = emulator();
    let url = create_ok(
        &router,
        "rotation-standard",
        &json!({ "VisibilityTimeout": HELD_FOR_SECONDS }),
    )
    .await;
    let sends = interleaved(3, 2);
    for (group, body) in &sends {
        ok(
            &router,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": body, "MessageGroupId": group}),
        )
        .await;
    }

    // One message in flight, held.
    let held = receive(&router, &url, 1).await;
    assert_eq!(
        held.len(),
        1,
        "case the held message: expected one, got {held:?}"
    );
    // Everything else comes over, including the rest of the held message's group.
    let rest = receive(&router, &url, 10).await;
    let bodies: BTreeSet<String> = rest.iter().map(|(_, body, _)| body.clone()).collect();
    let expected: BTreeSet<String> = sends
        .iter()
        .map(|(_, body)| body.clone())
        .filter(|body| *body != held[0].1)
        .collect();
    assert_eq!(
        bodies, expected,
        "case a standard queue: every message but the one in flight must be handed over"
    );
}
