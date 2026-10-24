// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `GeoMatchSet` create-and-read path, end to end over
//! the wire: `GetChangeToken`, `CreateGeoMatchSet`, `GetGeoMatchSet`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`, awsJson1_1) via this crate's shared
//! `wire` harness, so what these tests assert is what an SDK client sees: each response
//! is built from the persisted record, not from the request.
//!
//! `UpdateGeoMatchSet`, `ListGeoMatchSets` and `DeleteGeoMatchSet` are still scaffolds
//! (HTTP 501), so nothing here calls them.

use std::collections::BTreeSet;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::MAX_GEO_MATCH_SETS;

// The wire harness is shared with the other suites in this crate.
use crate::wire::{
    call, change_token, error_code, expect_error, is_resource_id, ok_call, router, token_status,
};

/// Create a GeoMatchSet named `name` and return its `GeoMatchSetId`.
async fn create_geo_match_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateGeoMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["GeoMatchSet"]["GeoMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGeoMatchSet returned no GeoMatchSetId: {body}"))
        .to_string()
}

/// Create an IPSet named `name` and return its `IPSetId`. The GeoMatchSet tests below
/// use it to show that the two resource families are separate stores.
async fn create_ip_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateIPSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {body}"))
        .to_string()
}

/// `CreateGeoMatchSet` stores a new, empty GeoMatchSet under a minted id, and
/// `GetGeoMatchSet` reads that record back — across a table of names differing in
/// length, case and the characters they use. `ResourceName` carries no uniqueness
/// constraint, so the last two rows share a name and must still be two distinct
/// resources.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_geo_match_set() {
    let router = router();
    let max_length_name = "g".repeat(128);
    let cases: &[(&str, &str)] = &[
        // (label, Name) — `ResourceName` is len 1..128, pattern \S, per the model's
        // CreateGeoMatchSet input.
        ("a plain name", "blocked-countries"),
        ("one character (the minimum length)", "g"),
        ("128 characters (the maximum length)", max_length_name.as_str()),
        ("mixed case, digits, separators", "Prod_Geo-2"),
        ("an interior space", "eu office regions"),
        ("a name shared with the next row", "shared-name"),
        ("the same name again", "shared-name"),
    ];

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for (label, name) in cases {
        let token = change_token(&router).await;
        let created = ok_call(
            &router,
            "CreateGeoMatchSet",
            &json!({"Name": name, "ChangeToken": token}),
        )
        .await;

        let geo_match_set_id = created["GeoMatchSet"]["GeoMatchSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: no GeoMatchSetId was returned: {created}"))
            .to_string();
        assert!(
            is_resource_id(&geo_match_set_id),
            "case {label}: expected a ResourceId, got {geo_match_set_id:?}"
        );
        assert!(
            seen_ids.insert(geo_match_set_id.clone()),
            "case {label}: CreateGeoMatchSet reused the id {geo_match_set_id}"
        );
        assert_eq!(
            created["GeoMatchSet"]["Name"].as_str(),
            Some(*name),
            "case {label}: CreateGeoMatchSet must report the stored name: {created}"
        );
        // "The GeoMatchSet contains no GeoMatchConstraints" — CreateGeoMatchSet's own
        // documented output.
        assert_eq!(
            created["GeoMatchSet"]["GeoMatchConstraints"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {label}: a new GeoMatchSet holds no constraints: {created}"
        );
        // "The ChangeToken that you used to submit the CreateGeoMatchSet request."
        assert_eq!(
            created["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: CreateGeoMatchSet must echo the consumed change token: {created}"
        );
        // A mutating call consumed the token, so it is propagating, never still
        // PROVISIONED.
        assert_eq!(
            token_status(&router, &token).await,
            "PENDING",
            "case {label}: CreateGeoMatchSet must consume the change token"
        );

        let fetched = ok_call(
            &router,
            "GetGeoMatchSet",
            &json!({"GeoMatchSetId": geo_match_set_id}),
        )
        .await;
        assert_eq!(
            fetched["GeoMatchSet"]["GeoMatchSetId"].as_str(),
            Some(geo_match_set_id.as_str()),
            "case {label}: GetGeoMatchSet must answer for the id it was given: {fetched}"
        );
        assert_eq!(
            fetched["GeoMatchSet"]["Name"].as_str(),
            Some(*name),
            "case {label}: GetGeoMatchSet must read the stored name back: {fetched}"
        );
        assert_eq!(
            fetched["GeoMatchSet"]["GeoMatchConstraints"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {label}: GetGeoMatchSet must read back an empty constraint list: {fetched}"
        );
    }

    // Reading is repeatable: the second read of each stored record answers exactly
    // what the first one did, so no read changed what a later caller sees.
    for id in &seen_ids {
        let first = ok_call(&router, "GetGeoMatchSet", &json!({"GeoMatchSetId": id})).await;
        let second = ok_call(&router, "GetGeoMatchSet", &json!({"GeoMatchSetId": id})).await;
        assert_eq!(
            first, second,
            "case {id}: two reads of one GeoMatchSet must answer the same record"
        );
    }
}

/// `CreateGeoMatchSet` follows the change-token protocol: "each create, update, or
/// delete request must use a unique change token", so a token no `GetChangeToken`
/// minted, and one another mutating call already consumed, are both
/// `WAFStaleDataException` — and the refused call stores nothing.
#[tokio::test]
async fn create_refuses_a_change_token_it_cannot_use() {
    let router = router();

    // A token this service minted and CreateGeoMatchSet already consumed.
    let used = change_token(&router).await;
    ok_call(
        &router,
        "CreateGeoMatchSet",
        &json!({"Name": "first-use", "ChangeToken": used}),
    )
    .await;
    // A token another resource family's mutating call consumed: one change-token
    // store serves every operation.
    let used_elsewhere = change_token(&router).await;
    ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "other-family", "ChangeToken": used_elsewhere}),
    )
    .await;

    let cases: &[(&str, &str)] = &[
        // (label, ChangeToken) — `ChangeToken` is len 1..128, pattern \S, so each row
        // satisfies the model and is refused by the protocol, not by validation.
        ("already consumed by this operation", used.as_str()),
        (
            "already consumed by another operation",
            used_elsewhere.as_str(),
        ),
        ("never minted", "00000000-0000-4000-8000-000000000000"),
        ("not even token-shaped", "x"),
    ];
    for (label, token) in cases {
        let body = expect_error(
            &router,
            "CreateGeoMatchSet",
            &json!({"Name": "refused", "ChangeToken": token}),
            "WAFStaleDataException",
            label,
        )
        .await;
        assert_eq!(
            body["message"].as_str(),
            Some("The input token is no longer current."),
            "case {label}: the refusal carries this exception type's fixed sentence, which \
             says nothing about the token it refused: {body}"
        );
    }
}

/// `GetGeoMatchSet` answers its modelled `WAFNonexistentItemException` for an id no
/// GeoMatchSet is stored under — never a 501 and never a hollow 200 — and a
/// GeoMatchSet is not reachable by another resource family's id.
#[tokio::test]
async fn an_unknown_geo_match_set_id_is_a_modelled_not_found() {
    let router = router();
    let stored = create_geo_match_set(&router, "the-one-that-exists").await;
    // The IPSet store is a different store: its ids are not GeoMatchSet ids.
    let ip_set_id = create_ip_set(&router, "the-one-that-exists").await;

    let max_length_id = "y".repeat(128);
    let cases: &[(&str, &str)] = &[
        (
            "a well-formed id nothing is stored under",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("another resource family's id", ip_set_id.as_str()),
        ("a one-character id (the minimum length)", "x"),
        (
            "a 128-character id (the maximum length)",
            max_length_id.as_str(),
        ),
    ];
    for (label, id) in cases {
        let body = expect_error(
            &router,
            "GetGeoMatchSet",
            &json!({"GeoMatchSetId": id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            body["message"].as_str(),
            Some("The referenced item does not exist."),
            "case {label}: the refusal carries this exception type's fixed sentence, which \
             says nothing about the id it did not find: {body}"
        );
    }

    // The stored record is still readable: a refused read changed nothing.
    let fetched = ok_call(
        &router,
        "GetGeoMatchSet",
        &json!({"GeoMatchSetId": stored}),
    )
    .await;
    assert_eq!(
        fetched["GeoMatchSet"]["GeoMatchSetId"].as_str(),
        Some(stored.as_str()),
        "case after the refusals: the stored GeoMatchSet must still read back: {fetched}"
    );
}

/// The store a caller can grow is bounded: at the account's GeoMatchSet quota
/// `CreateGeoMatchSet` answers its modelled `WAFLimitsExceededException` and consumes
/// no change token, while the IPSet quota is counted separately.
#[tokio::test]
async fn create_refuses_past_the_geo_match_set_quota() {
    let router = router();
    for n in 0..MAX_GEO_MATCH_SETS {
        create_geo_match_set(&router, &format!("quota-probe-{n}")).await;
    }

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateGeoMatchSet",
        &json!({"Name": "one-too-many", "ChangeToken": token}),
        "WAFLimitsExceededException",
        "at the quota",
    )
    .await;
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "case at the quota: a refused create must not consume the change token"
    );

    // Each resource family has its own quota, so a full GeoMatchSet store does not
    // block an IPSet.
    let token = change_token(&router).await;
    let (status, body) = call(
        &router,
        "CreateIPSet",
        &json!({"Name": "a-different-family", "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case at the GeoMatchSet quota: CreateIPSet must still succeed, got {status} \
         {} ({body})",
        error_code(&body)
    );
}
