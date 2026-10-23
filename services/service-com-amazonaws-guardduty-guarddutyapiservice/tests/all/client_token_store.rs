// SPDX-License-Identifier: CC0-1.0

//! The idempotency-token store, end to end over the wire.
//!
//! The rules under test, asserted over a SET of inputs rather than one call:
//!
//! 1. A `ClientToken` replays to the detector the first `CreateDetector` minted
//!    under it, for any token shape, and only for the endpoint it was recorded
//!    against.
//! 2. The store that holds those mappings is BOUNDED: the key is request-chosen, so
//!    after more distinct tokens than `MAX_CLIENT_TOKENS` the map holds at most that
//!    many mappings. Eviction costs a replay its idempotency and nothing else —
//!    every detector a create minted is still readable afterwards, and the mapping a
//!    caller just recorded still replays.
//! 3. A delete drops the mappings naming the detector it removed, so a replayed
//!    create never answers with an id the store no longer holds.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ComAmazonawsGuarddutyGuarddutyapiserviceState as GuardDutyState, MAX_CLIENT_TOKENS,
};

use super::detector_lifecycle::{call_in, error_code, router_over};

/// How many `(region, token)` mappings the store holds right now.
fn stored_mappings(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<GuardDutyState>().lock().client_tokens.len()
}

async fn create_in(router: &MultiServiceRouter, region: &str, body: Value) -> (u16, Value) {
    call_in(router, region, "POST", "/detector", Some(body)).await
}

fn detector_id(case: &str, body: &Value) -> String {
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a detectorId, got {body:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: a token replays, and only against the endpoint that recorded it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_client_token_replays_to_the_detector_it_minted() {
    // Rows differ along the token's FORM: a word, a UUID, a token at the single
    // character end, and one at the published upper bound of `ClientToken`'s length
    // (64 — the boundary refuses 65).
    let tokens = [
        "a-plain-token",
        "11111111-2222-3333-4444-555555555555",
        "t",
        &"z".repeat(64),
    ];

    for token in tokens {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let case = format!("token {token:?}");

        let (status, first) =
            create_in(&router, "us-east-1", json!({"enable": true, "clientToken": token})).await;
        assert_eq!(status, 200, "case {case}: first create got {status} {first:?}");
        let id = detector_id(&case, &first);

        // The replay returns the same detector rather than refusing as a duplicate.
        let (status, replayed) =
            create_in(&router, "us-east-1", json!({"enable": true, "clientToken": token})).await;
        assert_eq!(
            status, 200,
            "case {case}: the replayed create got {status} {replayed:?}"
        );
        assert_eq!(
            detector_id(&case, &replayed),
            id,
            "case {case}: a replay must return the detector the first create minted, got {replayed:?}"
        );
        assert_eq!(
            stored_mappings(&engine),
            1,
            "case {case}: a replayed token must not record a second mapping"
        );

        // The same token against another Region's endpoint is a different mapping:
        // it mints that Region's own detector.
        let (status, elsewhere) =
            create_in(&router, "eu-central-1", json!({"enable": true, "clientToken": token})).await;
        assert_eq!(
            status, 200,
            "case {case}: the create in eu-central-1 got {status} {elsewhere:?}"
        );
        assert_ne!(
            detector_id(&case, &elsewhere),
            id,
            "case {case}: a token is scoped to the endpoint it was replayed against, got {elsewhere:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a delete drops the mappings naming the detector it removed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_detector_leaves_no_resolvable_mapping_behind() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let region = "us-west-2";
    let token = "a-token-the-delete-outlives";

    let (status, created) =
        create_in(&router, region, json!({"enable": true, "clientToken": token})).await;
    assert_eq!(status, 200, "create got {status} {created:?}");
    let id = detector_id("delete", &created);

    let (status, body) = call_in(
        &router,
        region,
        "DELETE",
        &format!("/detector/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "delete got {status} {body:?}");
    assert_eq!(
        stored_mappings(&engine),
        0,
        "the delete must drop the mapping naming the detector it removed"
    );

    // The replayed create mints a NEW detector rather than reporting the deleted id.
    let (status, replayed) =
        create_in(&router, region, json!({"enable": true, "clientToken": token})).await;
    assert_eq!(status, 200, "the replayed create got {status} {replayed:?}");
    let replayed_id = detector_id("replay after delete", &replayed);
    assert_ne!(
        replayed_id, id,
        "a replay after a delete must not answer with the removed detector id"
    );
    let (status, read) = call_in(&router, region, "GET", &format!("/detector/{id}"), None).await;
    assert_eq!(
        status, 400,
        "the deleted detector must stay deleted, got {status} {read:?}"
    );
    assert_eq!(
        error_code(&read),
        "InvalidInputException",
        "the deleted detector must be refused, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the store is bounded, and eviction costs only idempotency.
// ---------------------------------------------------------------------------

/// Drive more distinct tokens into the store than it retains.
///
/// Each create is signed for its own Region, because a detector is one per account
/// per Region: that is the only way one caller records many mappings that all still
/// name a live detector, which is exactly the case a bound has to survive.
#[tokio::test]
async fn the_idempotency_store_stays_bounded_without_losing_a_detector() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));

    let overshoot = 8;
    let mut minted: Vec<(String, String, String)> = Vec::new();
    for index in 0..MAX_CLIENT_TOKENS + overshoot {
        let region = format!("us-test-{index}");
        let token = format!("token-{index}");
        let (status, created) = create_in(
            &router,
            &region,
            json!({"enable": true, "clientToken": token}),
        )
        .await;
        assert_eq!(
            status, 200,
            "create {index} in {region} got {status} {created:?}"
        );
        minted.push((
            region,
            token,
            detector_id(&format!("create {index}"), &created),
        ));

        assert!(
            stored_mappings(&engine) <= MAX_CLIENT_TOKENS,
            "after {} create(s) the store held {} mapping(s), past the bound of {MAX_CLIENT_TOKENS}",
            index + 1,
            stored_mappings(&engine)
        );
    }

    // Every detector a create minted is still there: the bound evicts mappings, not
    // resources.
    for (region, _, id) in &minted {
        let (status, read) = call_in(&router, region, "GET", &format!("/detector/{id}"), None).await;
        assert_eq!(
            status, 200,
            "the detector of {region} must still be readable, got {status} {read:?}"
        );
    }

    // The most recently recorded mapping still replays...
    let (region, token, id) = minted
        .last()
        .cloned()
        .expect("the loop recorded at least one mapping");
    let (status, replayed) = create_in(
        &router,
        &region,
        json!({"enable": true, "clientToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the newest mapping must still replay, got {status} {replayed:?}"
    );
    assert_eq!(
        detector_id("newest mapping", &replayed),
        id,
        "the newest mapping must replay to its own detector, got {replayed:?}"
    );

    // ...while an evicted one has lost only its idempotency: the create it replays
    // is refused as the duplicate it is, and the detector is untouched.
    let (region, token, id) = minted
        .first()
        .cloned()
        .expect("the loop recorded at least one mapping");
    let (status, refused) = create_in(
        &router,
        &region,
        json!({"enable": true, "clientToken": token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "an evicted mapping replays as a duplicate create, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "an evicted mapping replays as a duplicate create, got {refused:?}"
    );
    let (status, read) = call_in(&router, &region, "GET", &format!("/detector/{id}"), None).await;
    assert_eq!(
        status, 200,
        "eviction must not touch the detector, got {status} {read:?}"
    );
}
