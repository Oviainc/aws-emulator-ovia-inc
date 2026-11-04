// SPDX-License-Identifier: CC0-1.0

//! What repeated create-and-delete CHURN leaves behind on both entity-set families,
//! end to end over the wire: `DeleteThreatEntitySet` and `DeleteTrustedEntitySet`
//! share one deletion helper, so the rule is asserted on each of them.
//!
//! `threat_entity_set_delete.rs` and `trusted_entity_set_delete.rs` own one delete
//! each; this suite owns what MANY of them do. A delete takes the set off the
//! detector, so the record the store keeps behind it is a tombstone no operation
//! addresses — and the store bounds how many it keeps
//! (`state::DELETED_ENTITY_SETS_RETAINED`), so the rule a caller can observe is that
//! the bound is INVISIBLE: whether a deleted id was still inside the allowance or
//! has been evicted from it, the answer is the same absence refusal, and no churn
//! ever brings a deleted set back. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization and the
//! handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. Every id a delete took off the detector is refused with the entity-set
//!    families' absence refusal, for every cycle of the churn — the ones still
//!    inside `state::DELETED_ENTITY_SETS_RETAINED` and the ones past it alike.
//! 2. The listing names exactly the sets that are live: the churn leaves no ghost in
//!    the collection, and the set the last cycle created is readable.
//!
//! The rows differ along the family, the number of cycles (one past the retention
//! ceiling, half again, and twice it) and whether a read lands between a delete and
//! the next create.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
use crate::set_delete_terminality::assert_entity_set_absent;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DELETED_ENTITY_SETS_RETAINED, ENTITY_SET_INACTIVE, THREAT_ENTITY_SET_ID_MEMBER,
    TRUSTED_ENTITY_SET_ID_MEMBER,
};

/// One entity-set family: the collection segment of its URI under
/// `/detector/{DetectorId}/`, the member its create reports the minted id in, and the
/// member its listing reports ids in.
struct Family {
    collection: &'static str,
    id_member: &'static str,
    ids_member: &'static str,
}

/// Both families, so neither can drift from the shared deletion rule.
const FAMILIES: [Family; 2] = [
    Family {
        collection: "threatentityset",
        id_member: THREAT_ENTITY_SET_ID_MEMBER,
        ids_member: "threatEntitySetIds",
    },
    Family {
        collection: "trustedentityset",
        id_member: TRUSTED_ENTITY_SET_ID_MEMBER,
        ids_member: "trustedEntitySetIds",
    },
];

/// Create one entity set in `family`, answering the id the create minted.
async fn create_set(
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
    created[family.id_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} from the create of set {index}, got {created:?}",
                family.id_member
            )
        })
        .to_string()
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

/// Delete one set of `family`, failing the test if the call did not succeed.
async fn delete_set(router: &Router, detector_id: &str, family: &Family, id: &str, case: &str) {
    let collection = family.collection;
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete of {id} to answer 200, got {status} {body:?}"
    );
}

/// The ids `family`'s listing names for a detector. The whole collection is bounded
/// well under the modelled page size, so it arrives in one page.
async fn listed(router: &Router, detector_id: &str, family: &Family, case: &str) -> Vec<String> {
    let collection = family.collection;
    let (status, got) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{collection}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing to answer 200, got {status} {got:?}"
    );
    got[family.ids_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {} in {got:?}",
                family.ids_member
            )
        })
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn churn_leaves_no_deleted_entity_set_addressable() {
    let retained = DELETED_ENTITY_SETS_RETAINED;
    assert!(
        retained >= 1,
        "the retention ceiling must keep at least one record, got {retained}"
    );

    // The rows differ along the number of create-and-delete cycles: one past the
    // retention ceiling, half again, and twice it — so each row spans ids the store
    // still holds a tombstone for and ids whose tombstone the bound has dropped.
    for cycles in [retained + 1, retained + retained / 2, 2 * retained] {
        for family in &FAMILIES {
            let case = format!("{} churned {cycles} times", family.collection);
            let case = case.as_str();
            let router = new_router();
            let detector_id = enabled_detector(&router).await;

            let mut deleted_ids: Vec<String> = Vec::new();
            for index in 0..cycles {
                let id = create_set(&router, &detector_id, family, index, case).await;
                // Every id this cycle minted is live until its delete: one set at a
                // time is enough for both families, the one-at-a-time trusted set
                // included.
                let listing = listed(&router, &detector_id, family, case).await;
                assert_eq!(
                    listing,
                    vec![id.clone()],
                    "case {case:?}: cycle {index} must leave exactly its own set listed"
                );
                delete_set(&router, &detector_id, family, &id, case).await;
                // A read between the delete and the next create: it must answer the
                // absence refusal and must not resurrect anything.
                if index % 2 == 0 {
                    let (status, body) = get_set(&router, &detector_id, family, &id).await;
                    assert_entity_set_absent(case, status, &body, &id);
                }
                deleted_ids.push(id);
            }

            // Rule 1: every id the churn deleted is refused, whether the store still
            // holds a tombstone for it or has evicted that tombstone.
            for (index, id) in deleted_ids.iter().enumerate() {
                let (status, body) = get_set(&router, &detector_id, family, id).await;
                assert_entity_set_absent(
                    &format!("{case}: deletion {index}"),
                    status,
                    &body,
                    id,
                );
            }

            // Rule 2: the collection is empty, and a fresh create is served and
            // readable — the churn left the family usable.
            let empty = listed(&router, &detector_id, family, case).await;
            assert!(
                empty.is_empty(),
                "case {case:?}: every set was deleted, so the listing must name none, got \
                 {empty:?}"
            );
            let fresh = create_set(&router, &detector_id, family, cycles, case).await;
            assert!(
                !deleted_ids.contains(&fresh),
                "case {case:?}: a create after the churn must mint an unused id, got {fresh}"
            );
            let (status, body) = get_set(&router, &detector_id, family, &fresh).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fresh set must be readable, got {status} {body:?}"
            );
            assert_eq!(
                body["status"].as_str().unwrap_or_default(),
                ENTITY_SET_INACTIVE,
                "case {case:?}: the fresh set was created unused, got {body:?}"
            );
        }
    }
}
