// SPDX-License-Identifier: CC0-1.0

//! MANY rules matching one event each, and a target Id RE-POINTED at another
//! resource — end to end over the wire through the real `MultiServiceRouter`
//! with SQS mounted beside EventBridge, so each delivery is read back on the
//! queue it was meant for.
//!
//! Two rules are asserted here, both of them about WHICH target a delivery
//! reaches:
//!
//! * every ENABLED rule of the bus whose `EventPattern` matches a published
//!   event gets its own copy, independently of how many other rules there are
//!   and of whether they matched, and a rule that does not match gets none;
//! * `PutTargets` "adds the specified targets to the specified rule, or updates
//!   the targets if they are already associated with the rule, as identified by
//!   target `Id`" (`PutTargets`), so writing the same `Id` with another `Arn`
//!   REPLACES that target: later events reach the new resource and not the old
//!   one.
//!
//! The tables span the class rather than one case: the fan-out rows differ along
//! how many rules match one event and how many events are published, and the
//! re-pointing rows differ along what the second write changes (the ARN, the ARN
//! of another service, the target's configuration) and along how many targets the
//! rule holds while it happens.

use serde_json::{json, Value};

use crate::target_dispatch::{
    drain, emulator, events, given_queue, Emulator, DETAIL_TYPE, SOURCE,
};

/// How many rules the fan-out test writes, each matching exactly one of the
/// events it publishes.
const RULES: usize = 12;

/// Write a rule in `name` whose pattern matches `detail`'s `orderId` and nothing
/// else, carrying `targets`.
async fn given_matching_rule(emulator: &Emulator, name: &str, order: &str, targets: Value) {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
        "detail": { "orderId": [order] },
    });
    let (status, written) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {written}");
    let (status, targeted) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": name, "Targets": targets }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: PutTargets answered {status} {targeted}"
    );
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {targeted}"
    );
}

/// Publish one entry per `order`, each matching the rule written for that order.
async fn publish_orders(emulator: &Emulator, orders: &[String]) {
    // `Entries` is modelled `length 1..10`, so a publication of more than ten
    // events is more than one request — which is also what makes the per-request
    // fan-out of each one observable on its own.
    for chunk in orders.chunks(10) {
        let entries: Vec<Value> = chunk
            .iter()
            .map(|order| {
                json!({
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({ "orderId": order }).to_string(),
                })
            })
            .collect();
        let (status, published) =
            events(emulator, "PutEvents", &json!({ "Entries": entries })).await;
        assert_eq!(status, 200, "PutEvents answered {status} {published}");
        assert_eq!(
            published["FailedEntryCount"],
            json!(0),
            "PutEvents reported a failed entry: {published}"
        );
    }
}

/// The bodies waiting on the queue at `url`.
async fn bodies(emulator: &Emulator, url: &str) -> Vec<String> {
    drain(emulator, url)
        .await
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Many rules, each matching one of many published events, each deliver their own
/// copy — and a rule that matches none of them delivers nothing.
#[tokio::test]
async fn every_matching_rule_delivers_its_own_copy_and_a_near_miss_none() {
    let emulator = emulator();
    let orders: Vec<String> = (0..RULES).map(|index| format!("order-{index}")).collect();

    // One rule and one queue per event, plus a near-miss rule whose pattern no
    // published event satisfies.
    let mut queues: Vec<(String, String)> = Vec::new();
    for order in &orders {
        let (url, arn) = given_queue(&emulator, &format!("{order}-queue")).await;
        given_matching_rule(
            &emulator,
            &format!("{order}-rule"),
            order,
            json!([{ "Id": "t1", "Arn": arn }]),
        )
        .await;
        queues.push((order.clone(), url));
    }
    let (near_miss_url, near_miss_arn) = given_queue(&emulator, "near-miss-queue").await;
    given_matching_rule(
        &emulator,
        "near-miss-rule",
        "order-never-published",
        json!([{ "Id": "t1", "Arn": near_miss_arn }]),
    )
    .await;

    publish_orders(&emulator, &orders).await;

    for (order, url) in &queues {
        let delivered = bodies(&emulator, url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {order:?}: expected this rule's own copy and no other, got {delivered:?}"
        );
        let copy: Value = serde_json::from_str(&delivered[0])
            .unwrap_or_else(|error| panic!("case {order:?}: the copy is not JSON ({error})"));
        assert_eq!(
            copy["detail"]["orderId"], json!(order),
            "case {order:?}: expected the event this rule matched, got {copy}"
        );
    }
    let near_miss = bodies(&emulator, &near_miss_url).await;
    assert!(
        near_miss.is_empty(),
        "a rule no published event matched delivers nothing, got {near_miss:?}"
    );
}

/// Writing the same target `Id` with another `Arn` MOVES the delivery: the new
/// resource receives the next event and the old one does not — over a TABLE whose
/// rows differ along what the second write changes and along how many targets the
/// rule holds.
#[tokio::test]
async fn the_same_target_id_written_again_moves_the_delivery() {
    // (case, whether the rule holds a second target that stays put)
    let rows = [
        ("a rule holding only the re-pointed target", false),
        ("a rule holding another target beside it", true),
    ];

    for (case, keeps_another) in rows {
        let emulator = emulator();
        let (first_url, first_arn) = given_queue(&emulator, "first-queue").await;
        let (second_url, second_arn) = given_queue(&emulator, "second-queue").await;
        let (kept_url, kept_arn) = given_queue(&emulator, "kept-queue").await;
        let mut targets = vec![json!({ "Id": "t1", "Arn": first_arn })];
        if keeps_another {
            targets.push(json!({ "Id": "t2", "Arn": kept_arn }));
        }
        given_matching_rule(
            &emulator,
            "repointed",
            "order-0",
            Value::Array(targets),
        )
        .await;

        // The positive control: while the target names the first queue, that is
        // where the event goes.
        publish_orders(&emulator, &["order-0".to_string()]).await;
        let before = bodies(&emulator, &first_url).await;
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: expected the first queue to receive while the target names it, \
             got {before:?}"
        );

        // The same Id, a new ARN: an UPDATE of that target rather than a second
        // one, so the rule still holds one target per Id.
        let (status, written) = events(
            &emulator,
            "PutTargets",
            &json!({
                "Rule": "repointed",
                "Targets": [{ "Id": "t1", "Arn": second_arn }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-pointing PutTargets answered {status} {written}"
        );
        assert_eq!(
            written["FailedEntryCount"],
            json!(0),
            "case {case:?}: the re-pointing PutTargets reported a failed entry: {written}"
        );
        let (status, listed) = events(
            &emulator,
            "ListTargetsByRule",
            &json!({ "Rule": "repointed" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTargetsByRule answered {status} {listed}"
        );
        let arns: Vec<String> = listed["Targets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|target| target["Arn"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut expected = vec![second_arn.clone()];
        if keeps_another {
            expected.push(kept_arn.clone());
        }
        assert_eq!(
            arns, expected,
            "case {case:?}: expected the rule to hold one target per Id, in the order they \
             were first written, got {listed}"
        );

        publish_orders(&emulator, &["order-0".to_string()]).await;

        let old = bodies(&emulator, &first_url).await;
        assert!(
            old.is_empty(),
            "case {case:?}: the re-pointed-away queue receives nothing more, got {old:?}"
        );
        let new = bodies(&emulator, &second_url).await;
        assert_eq!(
            new.len(),
            1,
            "case {case:?}: expected the new queue to receive the next event, got {new:?}"
        );
        let kept = bodies(&emulator, &kept_url).await;
        assert_eq!(
            kept.len(),
            usize::from(keeps_another) * 2,
            "case {case:?}: a target the write did not name keeps receiving every event, \
             got {kept:?}"
        );
    }
}
