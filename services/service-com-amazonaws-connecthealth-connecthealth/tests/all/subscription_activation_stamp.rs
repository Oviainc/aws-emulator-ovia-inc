// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what one `ActivateSubscription` call WRITES.
//!
//! The RULE under test, asserted over a table rather than one recorded sequence: a
//! transition operation the model does not declare `@idempotent` runs its transition on
//! every served call — it stamps the instant of its own transition and advances
//! `lastUpdatedAt` to it, whatever status the stored record already holds — while the
//! opposite transition's stamp is retained, so both boundaries stay readable. The
//! trait is the boundary of the rule: `smithy_query(query="operation",
//! operation="ActivateSubscription,DeactivateSubscription",
//! model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! reports `"idempotent": true` for `DeactivateSubscription` only, so a repeat of THAT
//! one is the call that must write nothing — pinned by the last test here.
//!
//! The rows differ along what the rule generalises over: the transitions the record has
//! already run (so the status it holds when the call arrives, ACTIVE or INACTIVE, and
//! which stamps it already carries), how many subscriptions the domain holds, which of
//! them is targeted, how many times the call is repeated, and the Region the request is
//! signed for.
//!
//! The harness (`Emulator`, `string_at`) is the sibling suites': one way of driving this
//! service through signed restJson1 requests, not two.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{string_at, Emulator};

/// Now, as epoch seconds — the clock the handlers stamp their timestamps from, and the
/// unit the modelled `Timestamp` members are serialised in by this protocol.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// A required timestamp member of a response, as epoch seconds.
fn seconds_at(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a number in {body:?}"))
}

/// Create a domain and `count` subscriptions under it, answering the domain id and the
/// created subscription ids in creation order.
async fn domain_with_subscriptions(
    emulator: &Emulator,
    case: &str,
    name: &str,
    count: usize,
) -> (String, Vec<String>) {
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": name })).await,
        "domainId",
    );
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
            "case {case:?}: CreateSubscription #{index} got {status} {created:?}"
        );
        ids.push(string_at(&created, "subscriptionId"));
    }
    (domain_id, ids)
}

/// `GetSubscription`, asserting the 200, and the stored `SubscriptionDescription`.
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
        "case {case:?}: GetSubscription got {status} {body:?}"
    );
    body.get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {body:?}"))
}

/// One `POST .../{transition}`, asserting the modelled 200, and the record it answered
/// with together with the window the call was served in.
async fn transition(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    subscription_id: &str,
    what: &str,
) -> (Value, f64, f64) {
    let path = format!("/domains/{domain_id}/subscriptions/{subscription_id}/{what}");
    let before = epoch_now();
    let (status, body) = emulator.send("POST", &path, Some(&json!({}))).await;
    let after = epoch_now();
    assert_eq!(status, 200, "case {case:?}: {what} got {status} {body:?}");
    let answered = body
        .get("subscription")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a subscription member in {body:?}"));
    (answered, before, after)
}

#[tokio::test]
async fn every_activation_stamps_the_record_whatever_status_it_held() {
    // Each row: the transitions already run before the measured call, how many
    // subscriptions the domain holds, which one is targeted, and the signed Region. Row 1
    // is the ordinary path — a freshly created subscription, which is ACTIVE without ever
    // having been activated, so nothing about the status distinguishes it from one that
    // was.
    let cases: [(&str, &[&str], usize, usize, &str); 6] = [
        ("a freshly created subscription", &[], 1, 0, "us-west-2"),
        (
            "already activated once",
            &["activate"],
            1,
            0,
            "us-east-1",
        ),
        ("deactivated first", &["deactivate"], 1, 0, "eu-west-1"),
        (
            "deactivated then activated",
            &["deactivate", "activate"],
            1,
            0,
            "eu-central-1",
        ),
        (
            "three subscriptions, the last targeted",
            &[],
            3,
            2,
            "ap-south-1",
        ),
        (
            "five subscriptions, a middle one deactivated and targeted",
            &["deactivate"],
            5,
            2,
            "ca-central-1",
        ),
    ];

    for (case, prior, count, target_index, region) in cases {
        let emulator = Emulator::new(region);
        let (domain_id, ids) =
            domain_with_subscriptions(&emulator, case, "clinic-activation", count).await;
        let target = ids[target_index].clone();

        for what in prior {
            transition(&emulator, case, &domain_id, &target, what).await;
        }

        // The record as it stands when the measured call arrives, and every neighbour's,
        // so the write can be shown to touch this resource only.
        let before_record = stored(&emulator, case, &domain_id, &target).await;
        let mut neighbours = Vec::new();
        for id in ids.iter().filter(|id| **id != target) {
            neighbours.push((id.clone(), stored(&emulator, case, &domain_id, id).await));
        }
        let previous_updated = seconds_at(case, &before_record, "lastUpdatedAt");
        let created_at = seconds_at(case, &before_record, "createdAt");
        let kept_deactivated = before_record.get("deactivatedAt").cloned();

        let (answered, before, after) =
            transition(&emulator, case, &domain_id, &target, "activate").await;

        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: an activated subscription is ACTIVE, got {answered:?}"
        );
        // The stamp this call owns: present, and taken from the instant this call was
        // served rather than left at whatever the record carried before.
        let activated_at = seconds_at(case, &answered, "activatedAt");
        assert!(
            (before..=after).contains(&activated_at),
            "case {case:?}: activatedAt must be the instant this activation was served, \
             in [{before}, {after}], got {activated_at}"
        );
        // `lastUpdatedAt` advances to the same instant: this call changed the record.
        let last_updated_at = seconds_at(case, &answered, "lastUpdatedAt");
        assert_eq!(
            last_updated_at, activated_at,
            "case {case:?}: lastUpdatedAt must advance to the instant of the transition \
             this call ran, got {answered:?}"
        );
        assert!(
            last_updated_at >= previous_updated,
            "case {case:?}: lastUpdatedAt must not go backwards from {previous_updated}, \
             got {last_updated_at}"
        );
        assert!(
            activated_at >= created_at,
            "case {case:?}: activatedAt {activated_at} must not precede createdAt \
             {created_at}"
        );
        // The opposite transition's stamp is a history entry, so it is RETAINED exactly
        // as it stood — present only for a record that has actually been deactivated.
        assert_eq!(
            answered.get("deactivatedAt").cloned(),
            kept_deactivated,
            "case {case:?}: an activation must retain deactivatedAt as it stood, \
             got {answered:?}"
        );
        assert_eq!(
            answered.get("createdAt").cloned(),
            before_record.get("createdAt").cloned(),
            "case {case:?}: an activation must not move createdAt, got {answered:?}"
        );
        assert_eq!(
            (
                answered.get("domainId").cloned(),
                answered.get("subscriptionId").cloned(),
                answered.get("arn").cloned(),
            ),
            (
                before_record.get("domainId").cloned(),
                before_record.get("subscriptionId").cloned(),
                before_record.get("arn").cloned(),
            ),
            "case {case:?}: an activation must not re-identify the subscription, \
             got {answered:?}"
        );

        // The write is PERSISTED: the read paths report exactly what the call answered.
        assert_eq!(
            stored(&emulator, case, &domain_id, &target).await,
            answered,
            "case {case:?}: GetSubscription must report the record the activation wrote"
        );
        let (status, listed) = emulator
            .send(
                "GET",
                &format!("/domains/{domain_id}/subscriptions?maxResults=100"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListSubscriptions got {status} {listed:?}"
        );
        let found = listed
            .get("subscriptions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: a subscriptions list in {listed:?}"))
            .iter()
            .find(|entry| string_at(entry, "subscriptionId") == target)
            .cloned();
        assert_eq!(
            found.as_ref(),
            Some(&answered),
            "case {case:?}: ListSubscriptions must report the record the activation wrote"
        );

        // And it touched one resource: every neighbour reads back unchanged.
        for (id, snapshot) in neighbours {
            assert_eq!(
                stored(&emulator, case, &domain_id, &id).await,
                snapshot,
                "case {case:?}: activating {target} must leave {id} untouched"
            );
        }
    }
}

#[tokio::test]
async fn repeated_activations_each_restamp_the_record() {
    // The rows differ along how many times the call is repeated and the status the
    // record held when the repeats began: the rule is per CALL, so the n-th call stamps
    // just as the first did.
    let cases: [(&str, &[&str], usize); 4] = [
        ("twice from the created state", &[], 2),
        ("four times from the created state", &[], 4),
        ("twice after a deactivation", &["deactivate"], 2),
        ("three times after a deactivation", &["deactivate"], 3),
    ];

    for (case, prior, repeats) in cases {
        let emulator = Emulator::new("us-west-2");
        let (domain_id, ids) =
            domain_with_subscriptions(&emulator, case, "clinic-restamp", 1).await;
        let target = ids[0].clone();
        for what in prior {
            transition(&emulator, case, &domain_id, &target, what).await;
        }

        let mut stamps: Vec<f64> = Vec::new();
        for round in 0..repeats {
            let (answered, before, after) =
                transition(&emulator, case, &domain_id, &target, "activate").await;
            let activated_at = seconds_at(case, &answered, "activatedAt");
            assert!(
                (before..=after).contains(&activated_at),
                "case {case:?} round {round}: activatedAt must be stamped by THIS call, \
                 in [{before}, {after}], got {activated_at}"
            );
            assert_eq!(
                seconds_at(case, &answered, "lastUpdatedAt"),
                activated_at,
                "case {case:?} round {round}: lastUpdatedAt must advance with the stamp, \
                 got {answered:?}"
            );
            if let Some(earlier) = stamps.last() {
                assert!(
                    activated_at >= *earlier,
                    "case {case:?} round {round}: a later activation's stamp must not \
                     precede an earlier one's {earlier}, got {activated_at}"
                );
            }
            stamps.push(activated_at);
            assert_eq!(
                stored(&emulator, case, &domain_id, &target).await,
                answered,
                "case {case:?} round {round}: the stored record must be what the call \
                 answered"
            );
        }
        assert_eq!(
            stamps.len(),
            repeats,
            "case {case:?}: every repeat must have been served and stamped, got {stamps:?}"
        );
    }
}

#[tokio::test]
async fn the_idempotent_sibling_writes_nothing_on_a_repeat() {
    // The boundary of the rule: `DeactivateSubscription` is the transition the model
    // DOES declare `@idempotent`, so a repeat of it must answer 200 and leave the stored
    // record byte for byte as the first call left it. The rows differ along how the
    // record reached INACTIVE and how many repeats follow.
    let cases: [(&str, &[&str], usize); 3] = [
        ("one repeat of a first deactivation", &[], 1),
        ("three repeats of a first deactivation", &[], 3),
        (
            "a repeat after an activate/deactivate cycle",
            &["activate", "deactivate", "activate"],
            2,
        ),
    ];

    for (case, prior, repeats) in cases {
        let emulator = Emulator::new("eu-west-1");
        let (domain_id, ids) =
            domain_with_subscriptions(&emulator, case, "clinic-idem-sibling", 1).await;
        let target = ids[0].clone();
        for what in prior {
            transition(&emulator, case, &domain_id, &target, what).await;
        }

        let (first, _, _) = transition(&emulator, case, &domain_id, &target, "deactivate").await;
        assert_eq!(
            first.get("status").and_then(Value::as_str),
            Some("INACTIVE"),
            "case {case:?}: a deactivated subscription is INACTIVE, got {first:?}"
        );
        assert!(
            first.get("deactivatedAt").is_some(),
            "case {case:?}: the first deactivation stamps deactivatedAt, got {first:?}"
        );
        for round in 0..repeats {
            let (repeated, _, _) =
                transition(&emulator, case, &domain_id, &target, "deactivate").await;
            assert_eq!(
                repeated, first,
                "case {case:?} round {round}: an idempotent repeat must answer exactly as \
                 the first call did, got {repeated:?}"
            );
            assert_eq!(
                stored(&emulator, case, &domain_id, &target).await,
                first,
                "case {case:?} round {round}: an idempotent repeat must write nothing"
            );
        }
    }
}
