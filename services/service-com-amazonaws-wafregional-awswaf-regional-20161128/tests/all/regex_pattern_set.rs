// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `RegexPatternSet` creation path, end to end over the
//! wire: `GetChangeToken`, `CreateRegexPatternSet`, `GetRegexPatternSet`.
//!
//! The rule these tests pin: `CreateRegexPatternSet` stores a NEW pattern set record
//! under a freshly minted `RegexPatternSetId` for whatever `ResourceName` the request
//! carries, consuming the caller's change token exactly once, and answers the stored
//! record — which, as the operation documents, "contains no objects" until
//! `UpdateRegexPatternSet` adds patterns; `GetRegexPatternSet` then reports that same
//! stored record for its id, and the modelled `WAFNonexistentItemException` for an id
//! this account holds no pattern set under.
//!
//! The recorded corpus unit creates ONE pattern set; the table below is the class that
//! rule covers, its rows differing in name length (the modelled `ResourceName` bounds
//! `len 1..128`), character form and repetition.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::MAX_REGEX_PATTERN_SETS;

// The wire harness (router, `call`, `ok_call`, the error helpers) is shared with every
// other suite in this crate; see `tests/all/wire.rs`.
use crate::wire::{
    call, change_token, error_code, expect_error, is_resource_id, ok_call, router, token_status,
};

/// The `ResourceName` values one `CreateRegexPatternSet` request may carry, spanning
/// the modelled constraint `ResourceName: len 1..128, pattern \S`
/// (`smithy_query(query="operation",operation="CreateRegexPatternSet")`): both declared
/// length bounds, punctuation and non-ASCII text.
fn name_table() -> Vec<(&'static str, String)> {
    vec![
        ("minimum modelled length", "a".to_string()),
        ("maximum modelled length", "n".repeat(128)),
        ("punctuated name", "Regex-Pattern_Set.1:v2".to_string()),
        ("non-ASCII name", "Ünïcøde-Ωmega-正規表現".to_string()),
        ("one character past the shortest", "ab".to_string()),
    ]
}

/// Create a pattern set named `name` with a fresh change token, and return the parsed
/// `CreateRegexPatternSet` response.
async fn create_pattern_set(router: &MultiServiceRouter, name: &str) -> Value {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRegexPatternSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        body["ChangeToken"].as_str(),
        Some(token.as_str()),
        "case {name:?}: CreateRegexPatternSet must echo the change token it consumed, got {body}"
    );
    body
}

/// The `RegexPatternSetId` of a create response.
fn created_id(body: &Value) -> String {
    body["RegexPatternSet"]["RegexPatternSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRegexPatternSet returned no RegexPatternSetId: {body}"))
        .to_string()
}

/// Assert `set` is the modelled projection of a pattern set named `name` holding no
/// patterns.
fn assert_empty_set(case: &str, set: &Value, name: &str) {
    let id = set["RegexPatternSetId"].as_str().unwrap_or_default();
    assert!(
        is_resource_id(id),
        "case {case:?}: RegexPatternSetId must be a minted ResourceId, got {id:?}"
    );
    assert_eq!(
        set["Name"].as_str(),
        Some(name),
        "case {case:?}: expected Name {name:?}, got {set}"
    );
    // `CreateRegexPatternSet` documents that it answers "a RegexPatternSet that
    // contains no objects": the patterns arrive through `UpdateRegexPatternSet`.
    let patterns = set["RegexPatternStrings"].as_array();
    assert!(
        patterns.is_some_and(|patterns| patterns.is_empty()),
        "case {case:?}: a new RegexPatternSet holds no patterns, got {set}"
    );
}

/// Every name in the table is persisted under its own minted id, and the Get reader
/// reports the record that was stored rather than the request.
#[tokio::test]
async fn create_persists_each_modelled_name_and_get_reads_it_back() {
    let router = router();
    let mut seen_ids = Vec::new();

    for (case, name) in name_table() {
        let created = create_pattern_set(&router, &name).await;
        assert_empty_set(case, &created["RegexPatternSet"], &name);
        let id = created_id(&created);

        // The round trip: an independent read of the persisted record.
        let fetched = ok_call(
            &router,
            "GetRegexPatternSet",
            &json!({"RegexPatternSetId": id}),
        )
        .await;
        assert_empty_set(case, &fetched["RegexPatternSet"], &name);
        assert_eq!(
            fetched["RegexPatternSet"], created["RegexPatternSet"],
            "case {case:?}: GetRegexPatternSet must report the created record, got {fetched}"
        );

        // A read changes nothing a later caller sees.
        let again = ok_call(
            &router,
            "GetRegexPatternSet",
            &json!({"RegexPatternSetId": id}),
        )
        .await;
        assert_eq!(
            again["RegexPatternSet"], fetched["RegexPatternSet"],
            "case {case:?}: a second GetRegexPatternSet must report the same record, got {again}"
        );

        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: each create mints a distinct id, but {id} was handed out twice \
             (already have {seen_ids:?})"
        );
        seen_ids.push(id);
    }

    // Earlier records survive every later create: the store holds them all at once.
    for ((case, name), id) in name_table().into_iter().zip(seen_ids.iter()) {
        let fetched = ok_call(
            &router,
            "GetRegexPatternSet",
            &json!({"RegexPatternSetId": id}),
        )
        .await;
        assert_empty_set(case, &fetched["RegexPatternSet"], &name);
    }
}

/// `ResourceName` carries no uniqueness constraint, so one name used twice is two
/// resources, each under its own id.
#[tokio::test]
async fn two_creates_with_one_name_are_two_pattern_sets() {
    let router = router();
    let name = "shared-name";

    let first = created_id(&create_pattern_set(&router, name).await);
    let second = created_id(&create_pattern_set(&router, name).await);
    assert_ne!(
        first, second,
        "case {name:?}: two creates must mint two ids, got {first} twice"
    );

    for (case, id) in [("first", &first), ("second", &second)] {
        let fetched = ok_call(
            &router,
            "GetRegexPatternSet",
            &json!({"RegexPatternSetId": id}),
        )
        .await;
        assert_empty_set(case, &fetched["RegexPatternSet"], name);
        assert_eq!(
            fetched["RegexPatternSet"]["RegexPatternSetId"].as_str(),
            Some(id.as_str()),
            "case {case}: GetRegexPatternSet must report the id it was asked for, got {fetched}"
        );
    }
}

/// The change-token protocol: "each create, update, or delete request must use a unique
/// change token", so a token already consumed — and a token this service never minted —
/// is stale data, and neither request stores anything.
#[tokio::test]
async fn create_consumes_its_change_token_exactly_once() {
    let router = router();
    let token = change_token(&router).await;

    let created = ok_call(
        &router,
        "CreateRegexPatternSet",
        &json!({"Name": "consumes-its-token", "ChangeToken": token}),
    )
    .await;
    let id = created_id(&created);

    // Rows: the token this create consumed, and one no `GetChangeToken` ever handed out.
    for (case, stale) in [
        ("the token already consumed", token.clone()),
        (
            "a token never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "CreateRegexPatternSet",
            &json!({"Name": "never-stored", "ChangeToken": stale}),
            "WAFStaleDataException",
            case,
        )
        .await;
    }

    // A fresh token still works, and the refusals left the first record alone.
    let other = created_id(&create_pattern_set(&router, "after-the-refusals").await);
    assert_ne!(other, id, "a later create mints its own id");
    let fetched = ok_call(
        &router,
        "GetRegexPatternSet",
        &json!({"RegexPatternSetId": id}),
    )
    .await;
    assert_empty_set("after the refusals", &fetched["RegexPatternSet"], "consumes-its-token");
}

/// An id this account holds no pattern set under is the modelled
/// `WAFNonexistentItemException` — including the id of a DIFFERENT resource type, since
/// each resource type is its own keyspace.
#[tokio::test]
async fn get_refuses_an_id_no_pattern_set_is_stored_under() {
    let router = router();

    // A real, minted id of another resource type: `CreateIPSet` answers an `IPSetId`.
    let ip_set_token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "not-a-pattern-set", "ChangeToken": ip_set_token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {ip_set}"))
        .to_string();

    // One stored pattern set, so the refusals below are about the id, not an empty store.
    let stored = created_id(&create_pattern_set(&router, "stored").await);

    for (case, id) in [
        (
            "a uuid-shaped id nothing was created under",
            "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
        ),
        ("another resource type's id", ip_set_id),
        ("an opaque non-uuid id", "not-an-id".to_string()),
    ] {
        expect_error(
            &router,
            "GetRegexPatternSet",
            &json!({"RegexPatternSetId": id}),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    // The stored record is still readable: a refused read changed nothing.
    let fetched = ok_call(
        &router,
        "GetRegexPatternSet",
        &json!({"RegexPatternSetId": stored}),
    )
    .await;
    assert_empty_set("stored", &fetched["RegexPatternSet"], "stored");
}

/// Every pattern set is a stored record, so creating past the account's bound is the
/// modelled `WAFLimitsExceededException` and nothing further is stored.
#[tokio::test]
async fn create_refuses_past_the_pattern_set_quota() {
    let router = router();
    let mut ids = Vec::new();
    for index in 0..MAX_REGEX_PATTERN_SETS {
        ids.push(created_id(
            &create_pattern_set(&router, &format!("quota-{index}")).await,
        ));
    }

    let token = change_token(&router).await;
    let (status, body) = call(
        &router,
        "CreateRegexPatternSet",
        &json!({"Name": "one-too-many", "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the quota ({MAX_REGEX_PATTERN_SETS}): expected 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("WAFLimitsExceededException"),
        "case one past the quota: expected WAFLimitsExceededException, got {} ({body})",
        error_code(&body)
    );

    // The refusal consumed nothing: the token it carried never reached the mutating
    // step, so `GetChangeTokenStatus` still reports it as minted-but-unused.
    let reported = token_status(&router, &token).await;
    assert_eq!(
        reported, "PROVISIONED",
        "case one past the quota: a refused create must leave its change token \
         unconsumed, got {reported}"
    );

    // Each record created up to the bound is still there.
    for (index, id) in ids.iter().enumerate() {
        let fetched = ok_call(
            &router,
            "GetRegexPatternSet",
            &json!({"RegexPatternSetId": id}),
        )
        .await;
        assert_empty_set(
            &format!("quota-{index}"),
            &fetched["RegexPatternSet"],
            &format!("quota-{index}"),
        );
    }
}
