// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIdentities`: the identities a call names are GIVEN UP, with
//! everything that named them.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * each named identity is deleted with everything that named it — a read of it is the
//!   modelled `ResourceNotFoundException`,
//!   the pool's listing no longer reports it, and the login the pool had indexed against it
//!   resolves nothing, so presenting that login again mints a NEW identity — while an
//!   identity the batch did not name is untouched;
//! * an id this service holds no identity for has nothing left to delete, so a batch
//!   repeating an id, a batch mixing stored ids with ids nothing ever minted, and a second
//!   call over an overlapping batch all answer success with nothing unprocessed and leave
//!   the same final state;
//! * one call deletes identities of more than one pool, each pool losing exactly the
//!   identities the batch named;
//! * the credentials and OpenID Connect tokens a deleted identity could be issued are no
//!   longer issuable for it, while the pool still issues them to an identity it still holds;
//! * a batch outside a constraint the MODEL states is refused ahead of the operation with a
//!   400 `ValidationException` and deletes nothing.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, member, Emulator};

/// The operation under test, spelled once.
const OPERATION: &str = "DeleteIdentities";

/// The provider every identity below is reached through. One of the names
/// `GetId$Logins` publishes for this service
/// (`smithy_query(query="doc",shape="GetIdInput",member="Logins")`).
const PROVIDER: &str = "graph.facebook.com";

/// Well-formed `IdentityId`s that no `GetId` ever minted: `len 1..55`,
/// `^[\w-]+:[0-9a-f-]+$` (`smithy_query(query="shape",shape="IdentityId")`), so they reach
/// the handler rather than being refused at the boundary.
const ABSENT_IDENTITY_IDS: [&str; 3] = [
    "us-east-1:44444444-4444-4444-4444-444444444444",
    "us-east-1:55555555-5555-5555-5555-555555555555",
    "eu-west-1:0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e",
];

/// The most identities one call may name: `IdentityIdList` is modelled `length 1..60`
/// (`smithy_query(query="shape",shape="IdentityIdList")`).
const MAX_BATCH: usize = 60;

/// A `Logins` map carrying the one federated login of the identity numbered `index` in a
/// case's pool.
fn login_of(index: usize) -> Value {
    json!({ PROVIDER: format!("federated-user-{index}") })
}

/// The `DeleteIdentities` request naming `identity_ids`.
fn delete_request(identity_ids: &[String]) -> Value {
    json!({ "IdentityIdsToDelete": identity_ids })
}

/// Every `IdentityId` the pool's listing reports, read across pages of `page_size` so a
/// listing longer than one page is read the way a client reads it.
async fn listed(emulator: &Emulator, pool_id: &str, page_size: u32) -> Vec<String> {
    let mut identity_ids = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("IdentityPoolId".to_string(), json!(pool_id));
        request.insert("MaxResults".to_string(), json!(page_size));
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), json!(token));
        }
        let page = emulator
            .ok("ListIdentities", &Value::Object(request))
            .await;
        for described in page["Identities"]
            .as_array()
            .unwrap_or_else(|| panic!("ListIdentities answers an Identities list: {page}"))
        {
            identity_ids.push(
                described["IdentityId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a description carries an IdentityId: {page}"))
                    .to_string(),
            );
        }
        next_token = page["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return identity_ids;
        }
    }
}

/// The `UnprocessedIdentityIds` a served call answered, insisting the member is PRESENT:
/// it is what a caller reads to learn what the call could not carry out, so an empty list
/// and a missing member are not the same answer.
fn unprocessed(case: &str, response: &Value) -> Vec<Value> {
    let reported = member(response, "UnprocessedIdentityIds").unwrap_or_else(|| {
        panic!("case {case:?}: a served DeleteIdentities answers UnprocessedIdentityIds: {response}")
    });
    reported
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: UnprocessedIdentityIds is a list, got {reported}")
        })
        .clone()
}

/// Delete `identity_ids` in one call and assert the call was served with nothing left
/// unprocessed — which is what every batch of this account's own ids is.
async fn delete(emulator: &Emulator, case: &str, identity_ids: &[String]) {
    let response = emulator.ok(OPERATION, &delete_request(identity_ids)).await;
    assert!(
        unprocessed(case, &response).is_empty(),
        "case {case:?}: deleting {} of this account's identities leaves nothing unprocessed, \
         got {response}",
        identity_ids.len()
    );
}

/// Mint `count` identities in `pool_id`, each reached through its own federated login.
async fn populate(emulator: &Emulator, pool_id: &str, count: usize) -> Vec<String> {
    let mut identity_ids = Vec::with_capacity(count);
    for index in 0..count {
        identity_ids.push(emulator.identity(pool_id, Some(login_of(index))).await);
    }
    identity_ids
}

/// One row of the deletion table: the case, how many identities the pool holds, and which
/// of them (by index) the batch names — possibly more than once — beside how many ids
/// naming no stored identity it also carries.
struct Case {
    name: &'static str,
    minted: usize,
    named: Vec<usize>,
    absent: usize,
}

/// RULE: every identity a call names is deleted with everything that named it — it is no
/// longer read back, no longer listed, and the login the pool had indexed against it
/// resolves nothing, so presenting that login mints a NEW identity — while an identity the
/// batch did not name is left exactly as it was.
#[tokio::test]
async fn every_named_identity_is_deleted_with_everything_that_named_it() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: the SIZE of the batch (one up
    // to the modelled maximum of 60), how much of the pool it names (one identity, some,
    // every one), whether it repeats an id, and whether it also carries ids naming no
    // stored identity.
    let cases: Vec<Case> = vec![
        Case {
            name: "one identity, the pool's only one",
            minted: 1,
            named: vec![0],
            absent: 0,
        },
        Case {
            name: "one of three identities",
            minted: 3,
            named: vec![1],
            absent: 0,
        },
        Case {
            name: "two of five identities",
            minted: 5,
            named: vec![0, 4],
            absent: 0,
        },
        Case {
            name: "every identity of the pool",
            minted: 4,
            named: vec![0, 1, 2, 3],
            absent: 0,
        },
        Case {
            name: "a batch repeating the same id three times",
            minted: 2,
            named: vec![1, 1, 1],
            absent: 0,
        },
        Case {
            name: "a batch mixing stored ids with ids nothing minted",
            minted: 3,
            named: vec![2],
            absent: ABSENT_IDENTITY_IDS.len(),
        },
        Case {
            name: "a batch naming only ids nothing minted",
            minted: 2,
            named: vec![],
            absent: ABSENT_IDENTITY_IDS.len(),
        },
        Case {
            // The modelled maximum batch: 60 ids, every one of them stored.
            name: "the modelled maximum batch of sixty identities",
            minted: MAX_BATCH,
            named: (0..MAX_BATCH).collect(),
            absent: 0,
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let name = case.name;
        // One pool per row, so the rows cannot interfere through a shared login index.
        let pool_id = emulator.pool(&format!("delete-identities-{row}"), true).await;
        let minted = populate(&emulator, &pool_id, case.minted).await;
        assert_eq!(
            listed(&emulator, &pool_id, 60).await.len(),
            case.minted,
            "case {name:?}: expected the pool to hold {} identities before the delete",
            case.minted
        );

        let mut batch: Vec<String> = case.named.iter().map(|at| minted[*at].clone()).collect();
        batch.extend(
            ABSENT_IDENTITY_IDS[..case.absent]
                .iter()
                .map(|id| (*id).to_string()),
        );
        delete(&emulator, name, &batch).await;

        let deleted: BTreeSet<usize> = case.named.iter().copied().collect();
        let survivors: Vec<String> = minted
            .iter()
            .enumerate()
            .filter(|(index, _)| !deleted.contains(index))
            .map(|(_, identity_id)| identity_id.clone())
            .collect();

        // The pool reports exactly the identities the batch did not name, each still read
        // back on its own.
        let after = listed(&emulator, &pool_id, 60).await;
        assert_eq!(
            after.iter().collect::<BTreeSet<_>>(),
            survivors.iter().collect::<BTreeSet<_>>(),
            "case {name:?}: expected the pool to list {survivors:?} after the delete, got \
             {after:?}"
        );
        for identity_id in &survivors {
            let (status, described) = emulator
                .call("DescribeIdentity", &json!({ "IdentityId": identity_id }))
                .await;
            assert_eq!(
                (status, described["IdentityId"].as_str()),
                (200, Some(identity_id.as_str())),
                "case {name:?}: an identity the batch did not name is still read back, got \
                 {status} with {described}"
            );
        }

        // A deleted identity is read back by nobody, in the words a missing identity is
        // reported with.
        for at in &deleted {
            let identity_id = &minted[*at];
            let (status, body) = emulator
                .call("DescribeIdentity", &json!({ "IdentityId": identity_id }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name:?}: a deleted identity is not found, got {status} with {body}"
            );
            assert_eq!(
                body["message"].as_str().or_else(|| body["Message"].as_str()),
                Some(format!("Identity '{identity_id}' not found.").as_str()),
                "case {name:?}: the not-found wording names the identity, got {body}"
            );
        }

        // The login the pool had indexed against a deleted identity resolves nothing, so
        // presenting it again mints a NEW identity; a surviving identity's login still
        // resolves to it. Read last, because it populates the pool again.
        for (index, identity_id) in minted.iter().enumerate() {
            let resolved = emulator.identity(&pool_id, Some(login_of(index))).await;
            if deleted.contains(&index) {
                assert_ne!(
                    resolved, *identity_id,
                    "case {name:?}: the login of a deleted identity must name a new identity, \
                     got {resolved}"
                );
            } else {
                assert_eq!(
                    resolved, *identity_id,
                    "case {name:?}: the login of a surviving identity still resolves to it, got \
                     {resolved}"
                );
            }
        }
    }
}

/// RULE: an id this service holds no identity for has nothing left to delete, so repeating
/// a batch — the whole of it, or a batch overlapping it — is served and leaves the same
/// final state as the first call.
#[tokio::test]
async fn deleting_the_same_identities_again_leaves_the_same_final_state() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("delete-identities-idempotent", true).await;
    let minted = populate(&emulator, &pool_id, 6).await;

    // (case, the ids the call names) — successive batches that overlap what an earlier one
    // already deleted, in the order a client might send them.
    let batches: Vec<(&str, Vec<String>)> = vec![
        ("the first batch", vec![minted[0].clone(), minted[1].clone()]),
        ("the identical batch again", vec![minted[0].clone(), minted[1].clone()]),
        (
            "a batch overlapping the first, naming one more",
            vec![minted[1].clone(), minted[2].clone()],
        ),
        (
            "a batch of ids every earlier call already deleted",
            vec![minted[0].clone(), minted[1].clone(), minted[2].clone()],
        ),
    ];

    let mut previous: Option<Vec<String>> = None;
    let mut gone: BTreeSet<String> = BTreeSet::new();
    for (case, batch) in &batches {
        let repeats_only_deleted_ids = batch.iter().all(|identity_id| gone.contains(identity_id));
        delete(&emulator, case, batch).await;
        gone.extend(batch.iter().cloned());
        let after = listed(&emulator, &pool_id, 60).await;
        let expected: Vec<String> = minted
            .iter()
            .filter(|identity_id| !gone.contains(*identity_id))
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected the pool to list {expected:?} in mint order, got {after:?}"
        );
        if repeats_only_deleted_ids {
            let previous = previous
                .as_ref()
                .expect("a repeat follows the call that deleted the ids");
            assert_eq!(
                after, *previous,
                "case {case:?}: a batch of ids already deleted changes nothing a later caller \
                 sees, got {after:?}"
            );
        }
        previous = Some(after);
    }
}

/// RULE: a listing read across pages after a delete reports each surviving identity exactly
/// once and no deleted one, whatever the page size.
#[tokio::test]
async fn a_paginated_listing_after_a_delete_reports_each_survivor_once() {
    let emulator = Emulator::new();

    // The rows differ in the page size the listing is read with — one, a size that divides
    // the survivors, one that does not, and one page larger than the pool — and in which
    // identities the delete named.
    let cases: Vec<(&str, u32, Vec<usize>)> = vec![
        ("a page of one", 1, vec![0, 5]),
        ("a page of two", 2, vec![1]),
        ("a page of three", 3, vec![2, 3, 4]),
        ("a page larger than the pool", 60, vec![5]),
    ];

    for (row, (case, page_size, named)) in cases.iter().enumerate() {
        let pool_id = emulator
            .pool(&format!("delete-identities-pages-{row}"), true)
            .await;
        let minted = populate(&emulator, &pool_id, 6).await;
        let batch: Vec<String> = named.iter().map(|at| minted[*at].clone()).collect();
        delete(&emulator, case, &batch).await;

        let after = listed(&emulator, &pool_id, *page_size).await;
        let unique: BTreeSet<&String> = after.iter().collect();
        assert_eq!(
            unique.len(),
            after.len(),
            "case {case:?}: a listing reports each identity once, got {after:?}"
        );
        let expected: Vec<String> = minted
            .iter()
            .enumerate()
            .filter(|(index, _)| !named.contains(index))
            .map(|(_, identity_id)| identity_id.clone())
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected the surviving identities {expected:?} in mint order, got \
             {after:?}"
        );
    }
}

/// RULE: one call deletes identities of more than one pool — an `IdentityId` names no pool,
/// so the batch is not a pool's — and each pool loses exactly the identities the batch
/// named.
#[tokio::test]
async fn one_call_deletes_identities_of_more_than_one_pool() {
    let emulator = Emulator::new();

    // Three pools, so the rule is asserted over more than the two a single pair would
    // show: each holds three identities and loses the ones the one batch names.
    let mut pools = Vec::new();
    for index in 0..3 {
        let pool_id = emulator
            .pool(&format!("delete-identities-across-{index}"), true)
            .await;
        let minted = populate(&emulator, &pool_id, 3).await;
        pools.push((pool_id, minted));
    }

    // One id from the first pool, two from the second, none from the third.
    let batch: Vec<String> = vec![
        pools[0].1[1].clone(),
        pools[1].1[0].clone(),
        pools[1].1[2].clone(),
    ];
    delete(&emulator, "identities of two of three pools", &batch).await;

    for (pool_id, minted) in &pools {
        let expected: Vec<String> = minted
            .iter()
            .filter(|identity_id| !batch.contains(identity_id))
            .cloned()
            .collect();
        let after = listed(&emulator, pool_id, 60).await;
        assert_eq!(
            after, expected,
            "case {pool_id:?}: expected the pool to keep {expected:?}, got {after:?}"
        );
    }
}

/// RULE: a deleted identity can no longer be issued the credentials or the OpenID Connect
/// token its pool issues — what spoke for it speaks for nobody — while an identity the pool
/// still holds is still issued both.
#[tokio::test]
async fn a_deleted_identity_can_no_longer_be_issued_credentials_or_a_token() {
    let emulator = Emulator::new();
    let pool_id = emulator
        .pool_with_roles("delete-identities-credentials", true)
        .await;
    let minted = populate(&emulator, &pool_id, 2).await;
    let (deleted, kept) = (minted[0].clone(), minted[1].clone());

    // Both identities are issued what the pool issues BEFORE the delete, so the refusals
    // below are the delete's work and not a pool that never issued anything.
    for identity_id in [&deleted, &kept] {
        for operation in ["GetCredentialsForIdentity", "GetOpenIdToken"] {
            let (status, body) = emulator
                .call(operation, &json!({ "IdentityId": identity_id }))
                .await;
            assert_eq!(
                status, 200,
                "case {operation:?}: the pool issues to {identity_id} before the delete, got \
                 {status} with {body}"
            );
        }
    }

    delete(
        &emulator,
        "an identity holding credentials",
        std::slice::from_ref(&deleted),
    )
    .await;

    for operation in ["GetCredentialsForIdentity", "GetOpenIdToken"] {
        let (status, body) = emulator
            .call(operation, &json!({ "IdentityId": &deleted }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation:?}: a deleted identity is issued nothing, got {status} with {body}"
        );
        let (status, body) = emulator
            .call(operation, &json!({ "IdentityId": &kept }))
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the identity the batch did not name is still issued to, got \
             {status} with {body}"
        );
    }
}

/// RULE: a batch outside a constraint the MODEL states is refused ahead of the operation
/// with a 400 `ValidationException`, and deletes nothing.
#[tokio::test]
async fn a_batch_outside_the_modelled_constraints_is_refused_and_deletes_nothing() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("delete-identities-refusals", true).await;
    let minted = populate(&emulator, &pool_id, 3).await;

    // `IdentityIdList` is modelled `length 1..60` over an `IdentityId` of `length 1..55,
    // pattern ^[\w-]+:[0-9a-f-]+$`, so these sit on the published bounds and one past each.
    let one_past_max_batch: Vec<String> = (0..=MAX_BATCH)
        .map(|index| format!("us-east-1:{index:0>8}-0000-0000-0000-000000000000"))
        .collect();
    let at_max_length = format!("us-east-1:{}", "a".repeat(45));
    let past_max_length = format!("us-east-1:{}", "a".repeat(46));

    // (case, request, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an empty batch, one short of the modelled minimum",
            delete_request(&[]),
            400,
            "ValidationException",
        ),
        (
            "a batch one past the modelled maximum of sixty",
            delete_request(&one_past_max_batch),
            400,
            "ValidationException",
        ),
        (
            "an absent IdentityIdsToDelete",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "an id outside the modelled pattern",
            delete_request(&["not-an-identity".to_string()]),
            400,
            "ValidationException",
        ),
        (
            "an empty id",
            delete_request(&[String::new()]),
            400,
            "ValidationException",
        ),
        (
            "an id one character past the modelled maximum length",
            delete_request(&[past_max_length]),
            400,
            "ValidationException",
        ),
        (
            "a stored id beside one outside the modelled pattern",
            delete_request(&[minted[0].clone(), "not-an-identity".to_string()]),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in &cases {
        let (status, body) = emulator.call(OPERATION, request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A well-formed id at the modelled maximum length that nothing minted is no refusal at
    // all: it names no identity, so there is nothing left to delete.
    delete(
        &emulator,
        "a well-formed id at the modelled maximum length that nothing minted",
        &[at_max_length],
    )
    .await;

    // Every refusal above deleted nothing: the pool still holds all three identities.
    let after = listed(&emulator, &pool_id, 60).await;
    assert_eq!(
        after, minted,
        "a refused batch deletes nothing, expected {minted:?}, got {after:?}"
    );
}
