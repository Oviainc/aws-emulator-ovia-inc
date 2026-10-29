// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ONE status step every `Subscription` transition takes,
//! driven through each operation that takes it.
//!
//! The RULE under test, asserted over a table rather than one recorded sequence: moving a
//! subscription to a status writes that status, stamps the instant of the transition on
//! the member the status owns, retains the stamp the opposite status owns, and moves
//! `lastUpdatedAt` to the same instant — one step, whichever operation takes it.
//! `ActivateSubscription`, `DeactivateSubscription` and the `DeleteDomain` that takes the
//! subscriptions down with their domain therefore agree about every byte of the record
//! they leave behind, and this suite drives all three. Whether a call that finds the
//! record ALREADY in the target status takes the step is the operation's own rule, not
//! the record's, so the `@idempotent` transition writes nothing on a repeat while the
//! other records that it ran.
//!
//! The rows differ along what the rule generalises over: which transition is taken, the
//! status and stamps the record holds when the call arrives, how long the sequence of
//! transitions is and in which order, how many subscriptions the domain holds, which of
//! them is targeted, and the Region the request is signed for.
//!
//! Sources for every value written by hand here:
//!
//! * `smithy_query(query="shape",shape="SubscriptionStatus,SubscriptionDescription",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — the statuses are `ACTIVE | INACTIVE | DELETED`, and the description carries
//!   `activatedAt` and `deactivatedAt` and no stamp member for `DELETED`, which is why
//!   that step moves the status and `lastUpdatedAt` only.
//! * `smithy_query(query="operation",operation="ActivateSubscription,DeactivateSubscription,GetSubscription,DeleteDomain",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `POST /domains/{domainId}/subscriptions/{subscriptionId}/activate` and
//!   `.../deactivate` answering 200, `GET /domains/{domainId}/subscriptions/{subscriptionId}`
//!   answering 200 with the modelled `ResourceNotFoundException` (404), `DELETE
//!   /domain/{domainId}` answering 200, and `"idempotent": true` for
//!   `DeactivateSubscription` (and `DeleteDomain`) only.
//!
//! The harness (`Emulator`, `string_at`, `error_code`, `web_app_setup`) is the sibling
//! suites': one way of driving this service, not two.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{self, SubscriptionStatus};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// ONE transition operation a client addresses, and what the step it takes must leave on
/// the record.
struct Step {
    what: &'static str,
    /// The last segment of the operation's URI.
    segment: &'static str,
    /// The `SubscriptionStatus` value this step moves the record to.
    target: &'static str,
    /// The stamp member the target status owns, which this step writes.
    stamp: &'static str,
    /// The stamp member the OPPOSITE status owns, which this step must retain.
    opposite_stamp: &'static str,
    /// Whether the model declares this operation `@idempotent`, which is what decides
    /// whether a call that finds the record already in `target` takes the step at all.
    idempotent: bool,
}

const ACTIVATE: Step = Step {
    what: "ActivateSubscription",
    segment: "activate",
    target: "ACTIVE",
    stamp: "activatedAt",
    opposite_stamp: "deactivatedAt",
    idempotent: false,
};

const DEACTIVATE: Step = Step {
    what: "DeactivateSubscription",
    segment: "deactivate",
    target: "INACTIVE",
    stamp: "deactivatedAt",
    opposite_stamp: "activatedAt",
    idempotent: true,
};

/// Now, as epoch seconds — the unit this protocol serialises a modelled `Timestamp` in,
/// so the window a call was served in can be compared with the stamp it wrote.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// A timestamp member of a response, as epoch seconds.
fn seconds_at(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a number in {body:?}"))
}

/// Create a domain (owning a web application when `web_app`) and `count` subscriptions
/// under it, answering the domain id and the subscription ids in creation order.
async fn domain_with_subscriptions(
    emulator: &Emulator,
    case: &str,
    name: &str,
    web_app: bool,
    count: usize,
) -> (String, Vec<String>) {
    let mut request = json!({ "name": name });
    if web_app {
        request["webAppSetupConfiguration"] = web_app_setup();
    }
    let domain_id = string_at(&emulator.create_domain(&request).await, "domainId");
    let mut ids = Vec::new();
    for index in 0..count {
        let (status, created) = emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateSubscription #{index} got {status} with {created:?}"
        );
        ids.push(string_at(&created, "subscriptionId"));
    }
    (domain_id, ids)
}

/// `GetSubscription`, asserting the modelled 200, and the stored record it reports.
async fn stored(emulator: &Emulator, case: &str, domain_id: &str, subscription_id: &str) -> Value {
    let (status, body) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSubscription got {status} with {body:?}"
    );
    body.get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {body:?}"))
}

/// Take one step through its own operation and assert the record it left is the one the
/// shared step writes. Answers the record the call reported.
async fn take_step(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    subscription_id: &str,
    step: &Step,
) -> Value {
    let before_record = stored(emulator, case, domain_id, subscription_id).await;
    let held_target = before_record.get("status").and_then(Value::as_str) == Some(step.target);

    let path = format!("/domains/{domain_id}/subscriptions/{subscription_id}/{}", step.segment);
    let before = epoch_now();
    let (status, body) = emulator.send("POST", &path, Some(&json!({}))).await;
    let after = epoch_now();
    assert_eq!(
        status, 200,
        "case {case:?}: {} got {status} with {body:?}",
        step.what
    );
    let answered = body
        .get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {body:?}"));

    if step.idempotent && held_target {
        // The operation's own rule, not the record's: the step is not taken at all, so
        // the record stands exactly as it did.
        assert_eq!(
            answered, before_record,
            "case {case:?}: a repeat of the idempotent {} must leave the record as it \
             stood, got {answered:?} against {before_record:?}",
            step.what
        );
    } else {
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some(step.target),
            "case {case:?}: {} must move the record to {}, got {answered:?}",
            step.what,
            step.target
        );
        // The stamp the target status owns records the instant of THIS transition, and
        // `lastUpdatedAt` moves to the same instant.
        let stamp = seconds_at(case, &answered, step.stamp);
        assert!(
            (before..=after).contains(&stamp),
            "case {case:?}: {} must stamp {} with the instant it was served, in \
             [{before}, {after}], got {stamp}",
            step.what,
            step.stamp
        );
        assert_eq!(
            seconds_at(case, &answered, "lastUpdatedAt"),
            stamp,
            "case {case:?}: {} must move lastUpdatedAt to the instant of its own \
             transition, got {answered:?}",
            step.what
        );
        // The opposite transition's stamp is history: retained exactly as it stood.
        assert_eq!(
            answered.get(step.opposite_stamp).cloned(),
            before_record.get(step.opposite_stamp).cloned(),
            "case {case:?}: {} must retain {}, got {answered:?} against {before_record:?}",
            step.what,
            step.opposite_stamp
        );
    }
    // Nothing else about the record is a transition's to touch, and the write is
    // PERSISTED: the read path reports what the call answered.
    assert_eq!(
        (
            answered.get("domainId").cloned(),
            answered.get("subscriptionId").cloned(),
            answered.get("arn").cloned(),
            answered.get("createdAt").cloned(),
        ),
        (
            before_record.get("domainId").cloned(),
            before_record.get("subscriptionId").cloned(),
            before_record.get("arn").cloned(),
            before_record.get("createdAt").cloned(),
        ),
        "case {case:?}: {} must not re-identify the subscription or move createdAt, \
         got {answered:?} against {before_record:?}",
        step.what
    );
    assert_eq!(
        stored(emulator, case, domain_id, subscription_id).await,
        answered,
        "case {case:?}: GetSubscription must report the record {} wrote",
        step.what
    );
    answered
}

#[tokio::test]
async fn every_subscription_transition_takes_the_one_shared_step() {
    // Each row: the sequence of transitions taken on the targeted subscription, how many
    // subscriptions the domain holds, which one is targeted, and the signed Region.
    let rows: [(&str, &[&Step], usize, usize, &str); 6] = [
        ("one activation", &[&ACTIVATE], 1, 0, "us-west-2"),
        ("one deactivation", &[&DEACTIVATE], 1, 0, "us-east-1"),
        (
            "deactivate then activate",
            &[&DEACTIVATE, &ACTIVATE],
            1,
            0,
            "eu-west-1",
        ),
        (
            "an idempotent repeat in the middle of a sequence",
            &[&DEACTIVATE, &DEACTIVATE, &ACTIVATE, &DEACTIVATE],
            2,
            1,
            "eu-central-1",
        ),
        (
            "four alternating steps, a middle subscription targeted",
            &[&ACTIVATE, &DEACTIVATE, &ACTIVATE, &DEACTIVATE],
            3,
            1,
            "ap-south-1",
        ),
        (
            "three activations in a row, the last subscription targeted",
            &[&ACTIVATE, &ACTIVATE, &ACTIVATE],
            3,
            2,
            "ca-central-1",
        ),
    ];

    for (case, sequence, count, target_index, region) in rows {
        let emulator = Emulator::new(region);
        let (domain_id, ids) =
            domain_with_subscriptions(&emulator, case, "clinic-step", false, count).await;
        let target = ids[target_index].clone();

        // Every neighbour's record, so a step can be shown to touch one resource only.
        let mut neighbours = Vec::new();
        for id in ids.iter().filter(|id| **id != target) {
            neighbours.push((id.clone(), stored(&emulator, case, &domain_id, id).await));
        }

        for (index, step) in sequence.iter().enumerate() {
            let case = format!("{case} (step {index}: {})", step.what);
            take_step(&emulator, &case, &domain_id, &target, step).await;
            for (id, snapshot) in &neighbours {
                assert_eq!(
                    &stored(&emulator, &case, &domain_id, id).await,
                    snapshot,
                    "case {case:?}: a transition of {target} must leave {id} untouched"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_domain_takes_the_same_step_for_every_subscription() {
    // Each row: how many subscriptions the domain holds, the transitions run on each of
    // them before the delete (so the status and stamps they carry when it arrives), and
    // the signed Region.
    let rows: [(&str, usize, &[&Step], &str); 4] = [
        ("one untouched subscription", 1, &[], "us-west-2"),
        (
            "three subscriptions, each deactivated",
            3,
            &[&DEACTIVATE],
            "us-east-1",
        ),
        (
            "two subscriptions, re-activated after a deactivation",
            2,
            &[&DEACTIVATE, &ACTIVATE],
            "eu-west-1",
        ),
        (
            "four subscriptions, each activated",
            4,
            &[&ACTIVATE],
            "ap-south-1",
        ),
    ];

    for (case, count, prior, region) in rows {
        let emulator = Emulator::new(region);
        // A domain owning a web application is the one whose record OUTLIVES the delete
        // call — it passes through the non-terminal DELETING — so the subscriptions the
        // delete stepped are still in the store to be read.
        let (domain_id, ids) =
            domain_with_subscriptions(&emulator, case, "clinic-delete-step", true, count).await;
        for id in &ids {
            for step in prior {
                take_step(&emulator, case, &domain_id, id, step).await;
            }
        }

        // The stamps and the instant each record carries when the delete arrives.
        let snapshots: Vec<(String, Option<SystemTime>, Option<SystemTime>, SystemTime)> = {
            let store =
                state::service_state(&**emulator.engine()).expect("this service's own store");
            let store = store.lock();
            let domain = store
                .held_domain(emulator.region(), &domain_id)
                .unwrap_or_else(|| panic!("case {case:?}: the created domain is held"));
            ids.iter()
                .map(|id| {
                    let subscription = domain
                        .subscriptions
                        .get(id)
                        .unwrap_or_else(|| panic!("case {case:?}: {id} is held under its domain"));
                    (
                        id.clone(),
                        subscription.activated_at,
                        subscription.deactivated_at,
                        subscription.last_updated_at,
                    )
                })
                .collect()
        };

        let before = SystemTime::now();
        let (status, deleted) = emulator
            .send("DELETE", &format!("/domain/{domain_id}"), None)
            .await;
        let after = SystemTime::now();
        assert_eq!(
            (status, string_at(&deleted, "status").as_str()),
            (200, "DELETING"),
            "case {case:?}: DeleteDomain of a web-app domain got {status} with {deleted:?}"
        );

        {
            let store =
                state::service_state(&**emulator.engine()).expect("this service's own store");
            let store = store.lock();
            let domain = store
                .held_domain(emulator.region(), &domain_id)
                .unwrap_or_else(|| panic!("case {case:?}: a DELETING domain's record is held"));
            for (id, activated_at, deactivated_at, previous_updated) in &snapshots {
                let subscription = domain
                    .subscriptions
                    .get(id)
                    .unwrap_or_else(|| panic!("case {case:?}: {id} is still held"));
                assert_eq!(
                    subscription.status,
                    SubscriptionStatus::Deleted,
                    "case {case:?}: deleting the domain must move {id} to DELETED, got \
                     {:?}",
                    subscription.status
                );
                assert!(
                    (before..=after).contains(&subscription.last_updated_at),
                    "case {case:?}: the step must move {id}'s lastUpdatedAt to the instant \
                     the delete was served, in [{before:?}, {after:?}], got {:?} (it stood \
                     at {previous_updated:?})",
                    subscription.last_updated_at
                );
                // `DELETED` owns no stamp member of its own, and the step clears neither
                // of the stamps the other statuses own.
                assert_eq!(
                    (subscription.activated_at, subscription.deactivated_at),
                    (*activated_at, *deactivated_at),
                    "case {case:?}: the step must retain {id}'s transition stamps, got \
                     {:?} against {:?}",
                    (subscription.activated_at, subscription.deactivated_at),
                    (*activated_at, *deactivated_at)
                );
            }
        }

        // And the status the step wrote is what every read path answers from: a DELETED
        // subscription is no longer live, so its reader reports the modelled 404.
        for id in &ids {
            let (status, body) = emulator
                .send(
                    "GET",
                    &format!("/domains/{domain_id}/subscriptions/{id}"),
                    None,
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: GetSubscription of the deleted {id} got {status} with \
                 {body:?}"
            );
        }
    }
}
