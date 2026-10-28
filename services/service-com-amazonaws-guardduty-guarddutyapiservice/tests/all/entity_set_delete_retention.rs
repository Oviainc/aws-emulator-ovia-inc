// SPDX-License-Identifier: CC0-1.0

//! The bounded retention of DELETED entity-set records, end to end over the wire,
//! for BOTH entity-set families: `DeleteThreatEntitySet` and
//! `DeleteTrustedEntitySet` share one retention helper, so the rule is asserted on
//! each of them.
//!
//! `threat_entity_set_delete.rs` and `trusted_entity_set_delete.rs` own the
//! `DELETE_PENDING` -> `DELETED` state machine, the slot freeing and the refusals;
//! this suite owns the one rule neither pinned: WHICH retained deleted record a
//! delete past the ceiling drops. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization and the
//! handlers are exercised together.
//!
//! The rule under test, asserted over a SET of inputs rather than one call: a delete
//! past `state::DELETED_ENTITY_SETS_RETAINED` drops the record whose delete was
//! ACCEPTED first, and no read can change which one that is — reading a deleted set
//! materialises the `DELETED` the clock owes it, and that must not reorder the
//! retained deletions. The rows differ along the family and along WHICH of the
//! retained deleted records a read settles before the surplus delete arrives (none
//! of them, the oldest deletion, one in the middle, the newest, or every one),
//! because it is exactly those reads that used to move the eviction key.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    not_owned_message, DELETED_ENTITY_SETS_RETAINED, ENTITY_SET_DELETED,
    ENTITY_SET_DELETE_PENDING, ENTITY_SET_TRANSITION_MILLIS, THREAT_ENTITY_SET_ID_MEMBER,
    TRUSTED_ENTITY_SET_ID_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// Long enough for the clock to owe a `DELETE_PENDING` record its `DELETED`, plus
/// slack for the wall clock the handlers read.
fn past_the_transition() -> std::time::Duration {
    let span = u64::try_from(ENTITY_SET_TRANSITION_MILLIS).unwrap_or_default();
    std::time::Duration::from_millis(span + 250)
}

/// One entity-set family: the collection segment of its URI under
/// `/detector/{DetectorId}/`, and the member its create reports the minted id in and
/// a refusal names.
struct Family {
    collection: &'static str,
    id_member: &'static str,
}

/// Both families, so neither can drift from the shared retention rule.
const FAMILIES: [Family; 2] = [
    Family {
        collection: "threatentityset",
        id_member: THREAT_ENTITY_SET_ID_MEMBER,
    },
    Family {
        collection: "trustedentityset",
        id_member: TRUSTED_ENTITY_SET_ID_MEMBER,
    },
];

/// Which of the already-deleted records a read settles before the surplus delete
/// arrives.
#[derive(Debug, Clone, Copy)]
enum Settle {
    /// Nothing is read, so no record has had its `DELETED` materialised.
    Nothing,
    /// One record is read, named by its position in deletion order.
    One(usize),
    /// Every retained deleted record is read, so all of them are materialised.
    All,
}

/// Create one entity set in `family` and accept its deletion straight away,
/// answering the id the create minted.
async fn create_and_delete(
    router: &Router,
    detector_id: &str,
    family: &Family,
    index: usize,
    case: &str,
) -> String {
    let collection = family.collection;
    let (status, created) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{collection}"),
        Some(json!({
            "name": format!("churn-{index}"),
            "format": "TXT",
            "location": format!("s3://guardduty-lists/churn-{index}.txt"),
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of set {index} to answer 200, got {status} {created:?}"
    );
    let id = created[family.id_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} from the create of set {index}, got {created:?}",
                family.id_member
            )
        })
        .to_string();

    let (status, body) = call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete of set {index} to answer 200, got {status} {body:?}"
    );
    id
}

/// Read one set of `family` back.
async fn get_set(router: &Router, detector_id: &str, family: &Family, id: &str) -> (u16, Value) {
    let collection = family.collection;
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        None,
    )
    .await
}

/// Assert a retained deleted record is still reported, in one of the two statuses a
/// delete puts it in.
async fn assert_still_reported(
    router: &Router,
    detector_id: &str,
    family: &Family,
    id: &str,
    case: &str,
    which: &str,
) {
    let (status, got) = get_set(router, detector_id, family, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {which} to still be reported with 200, got {status} {got:?}"
    );
    let reported = got["status"].as_str().unwrap_or_default();
    assert!(
        reported == ENTITY_SET_DELETE_PENDING || reported == ENTITY_SET_DELETED,
        "case {case:?}: expected {which} to report {ENTITY_SET_DELETE_PENDING} or \
         {ENTITY_SET_DELETED}, got {got:?}"
    );
}

/// Assert the record the eviction dropped is refused the way every unresolved
/// minted id is: HTTP 400 under the `InvalidInputException` class, naming the
/// MEMBER, with the caller's value never echoed.
fn assert_evicted(case: &str, status: u16, body: &Value, id_member: &str, id: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the evicted record's read to be refused with 400, got {status} \
         {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message,
        not_owned_message(id_member),
        "case {case:?}: expected the ownership refusal naming {id_member}, got {message:?}"
    );
    assert!(
        !message.contains(id),
        "case {case:?}: a refusal must not echo the caller's value, got {message:?}"
    );
}

/// Churn one entity set at a time, one more time than the retention ceiling: the
/// record whose delete was accepted FIRST is the one dropped, and every later
/// deletion is still reported — whatever reads settled the retained records in
/// between.
#[tokio::test]
async fn the_oldest_accepted_deletion_is_the_one_evicted() {
    let retained = DELETED_ENTITY_SETS_RETAINED;
    assert!(
        retained >= 3,
        "the retention ceiling must leave room for a middle row, got {retained}"
    );

    for family in &FAMILIES {
        for settle in [
            Settle::Nothing,
            Settle::One(0),
            Settle::One(retained / 2),
            Settle::One(retained - 1),
            Settle::All,
        ] {
            let case = format!("{} with {settle:?} settled", family.collection);
            let case = case.as_str();
            let router = new_router();
            let detector_id = enabled_detector(&router).await;

            // Fill the retention allowance exactly: a deleted set occupies none of
            // the detector's live slots, so one set at a time is enough for both
            // families, the trusted one included.
            let mut ids: Vec<String> = Vec::new();
            for index in 0..retained {
                ids.push(create_and_delete(&router, &detector_id, family, index, case).await);
            }

            // Every accepted deletion is now owed its DELETED, so a read of one
            // materialises it — which is what used to rewrite the eviction key.
            tokio::time::sleep(past_the_transition()).await;
            let settled: Vec<usize> = match settle {
                Settle::Nothing => Vec::new(),
                Settle::One(position) => vec![position],
                Settle::All => (0..retained).collect(),
            };
            for position in settled {
                let (status, got) = get_set(&router, &detector_id, family, &ids[position]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the read of deletion {position} to answer 200, got \
                     {status} {got:?}"
                );
                assert_eq!(
                    got["status"], ENTITY_SET_DELETED,
                    "case {case:?}: the clock owed deletion {position} its {ENTITY_SET_DELETED}, \
                     got {got:?}"
                );
            }

            // One more create-and-delete: the allowance is full, so this delete
            // evicts exactly one retained record.
            let surplus = create_and_delete(&router, &detector_id, family, retained, case).await;

            // The oldest ACCEPTED deletion is the one dropped.
            let (status, body) = get_set(&router, &detector_id, family, &ids[0]).await;
            assert_evicted(case, status, &body, family.id_member, &ids[0]);

            // Every later deletion, and the one that triggered the eviction, is
            // still reported: exactly one record left.
            for (position, id) in ids.iter().enumerate().skip(1) {
                assert_still_reported(
                    &router,
                    &detector_id,
                    family,
                    id,
                    case,
                    &format!("deletion {position}"),
                )
                .await;
            }
            assert_still_reported(
                &router,
                &detector_id,
                family,
                &surplus,
                case,
                "the deletion that triggered the eviction",
            )
            .await;
        }
    }
}

/// The retention is a CEILING, not a window: churning well past it leaves exactly
/// `DELETED_ENTITY_SETS_RETAINED` records reported — the most recent deletions —
/// however many cycles a caller sends, and reads along the way do not change which.
#[tokio::test]
async fn churning_past_the_ceiling_retains_the_newest_deletions_only() {
    let retained = DELETED_ENTITY_SETS_RETAINED;
    // The rows differ along the number of create-and-delete cycles: one past the
    // ceiling, twice it, and an odd count in between.
    for cycles in [retained + 1, retained + retained / 2, 2 * retained] {
        for family in &FAMILIES {
            let case = format!("{} churned {cycles} times", family.collection);
            let case = case.as_str();
            let router = new_router();
            let detector_id = enabled_detector(&router).await;

            let mut ids: Vec<String> = Vec::new();
            for index in 0..cycles {
                let id = create_and_delete(&router, &detector_id, family, index, case).await;
                // Read the deletion back on every other cycle: a read settles the
                // record it reads, and must not decide what a later delete drops.
                if index % 2 == 0 {
                    assert_still_reported(
                        &router,
                        &detector_id,
                        family,
                        &id,
                        case,
                        &format!("deletion {index}, read straight after its delete"),
                    )
                    .await;
                }
                ids.push(id);
                // Long enough that the earlier deletions are owed their DELETED
                // while the newest ones are not: the mixture the eviction key must
                // not be sensitive to.
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            }

            let evicted = cycles - retained;
            for (position, id) in ids.iter().enumerate() {
                if position < evicted {
                    let (status, body) = get_set(&router, &detector_id, family, id).await;
                    assert_evicted(case, status, &body, family.id_member, id);
                } else {
                    assert_still_reported(
                        &router,
                        &detector_id,
                        family,
                        id,
                        case,
                        &format!("deletion {position}"),
                    )
                    .await;
                }
            }
        }
    }
}
