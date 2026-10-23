// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `UpdateRegexPatternSet` write path, end to end over
//! the wire: `CreateRegexPatternSet`, `UpdateRegexPatternSet`, `GetRegexPatternSet`.
//!
//! The rule these tests pin, from the operation's own documentation ("Inserts or deletes
//! `RegexPatternString` objects in a `RegexPatternSet`"): each `RegexPatternSetUpdate`
//! applies its `ChangeAction` to the named set's stored pattern-string collection —
//! `INSERT` adds the update's `RegexPatternString`, `DELETE` removes it — and every later
//! read of that set reports the resulting collection, which `RegexPatternSet` carries as
//! the always-present `RegexPatternStrings` member (`@required`, `length 0..10`).
//!
//! The recorded unit inserts ONE pattern into one set and reads it back; the tables below
//! are the class that rule covers, their rows differing in pattern length (the modelled
//! `RegexPatternString: len 1..512`), regex form, the number of updates one request
//! carries and the mix of actions in it.
//!
//! The refusals the same rule implies are tables too: an update that would leave the set
//! as it was, a set id nothing is stored under, a reused change token and an insert past
//! the set's own modelled bound each answer their modelled exception and store nothing.
//! (`WAFInvalidRegexPatternException` — the refusal of a string that is not a regular
//! expression at all — needs a regex engine to decide, a caller-driven compile cost and a
//! dependency this change does not take on; the gap is tracked by its own report,
//! `issues/things-that-do-not-work/20261024T000000Z-waf-regional-updateregexpatternset-accepts-a-pattern-string-that-is-not-a-regex.md`.)

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_REGEX_PATTERN_SET_UPDATES_PER_REQUEST, MAX_REGEX_PATTERN_STRINGS,
};

// The wire harness (router, `call`, `ok_call`, the error helpers) is shared with every
// other suite in this crate; see `tests/all/wire.rs`.
use crate::wire::{call, change_token, error_code, expect_error, ok_call, router, token_status};

/// Pattern strings spanning the modelled `RegexPatternString` (`len 1..512`, pattern
/// `.*`): both declared length bounds, character classes with anchors, the alternation
/// form the `RegexPatternSet` shape's own documentation gives as an example
/// (`B[a@]dB[o0]t`), a Unicode property, a non-capturing group, escaped metacharacters and
/// a bounded repeat.
fn pattern_table() -> Vec<(&'static str, String)> {
    vec![
        ("minimum modelled length", "a".to_string()),
        ("maximum modelled length", "x".repeat(512)),
        ("anchored character class", "^admin-[0-9]+$".to_string()),
        ("the model's own example", "B[a@]dB[o0]t".to_string()),
        ("unicode property", r"\p{L}+".to_string()),
        ("non-capturing group", r"(?:bot|crawler)-[a-z]".to_string()),
        ("escaped metacharacters", r"\.\*\+".to_string()),
        ("alternation group", r"(bot|crawler)-\d{1,3}".to_string()),
    ]
}

/// One `RegexPatternSetUpdate`.
fn update(action: &str, pattern: &str) -> Value {
    json!({"Action": action, "RegexPatternString": pattern})
}

/// Create a pattern set and return its `RegexPatternSetId`.
async fn create_pattern_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRegexPatternSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["RegexPatternSet"]["RegexPatternSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRegexPatternSet returned no RegexPatternSetId: {body}"))
        .to_string()
}

/// `UpdateRegexPatternSet` with a fresh change token, expecting 200.
async fn apply_updates(
    router: &MultiServiceRouter,
    set_id: &str,
    updates: &[Value],
) -> (Value, String) {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "UpdateRegexPatternSet",
        &json!({"RegexPatternSetId": set_id, "ChangeToken": token, "Updates": updates}),
    )
    .await;
    (body, token)
}

/// The `RegexPatternStrings` of the stored set, read back through
/// `GetRegexPatternSet` — which must always carry the member, even when empty.
async fn stored_patterns(router: &MultiServiceRouter, set_id: &str) -> Vec<String> {
    let body = ok_call(
        router,
        "GetRegexPatternSet",
        &json!({"RegexPatternSetId": set_id}),
    )
    .await;
    assert_eq!(
        body["RegexPatternSet"]["RegexPatternSetId"].as_str(),
        Some(set_id),
        "GetRegexPatternSet must report the set it was asked for, got {body}"
    );
    body["RegexPatternSet"]["RegexPatternStrings"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("RegexPatternStrings is @required, so it is always present: {body}")
        })
        .iter()
        .map(|pattern| {
            pattern
                .as_str()
                .unwrap_or_else(|| panic!("a RegexPatternString is a string: {body}"))
                .to_string()
        })
        .collect()
}

/// An `INSERT` of each pattern in the table is reported by a later `GetRegexPatternSet`,
/// and a `DELETE` of it is reported as gone.
#[tokio::test]
async fn an_inserted_pattern_is_read_back_and_a_deleted_one_is_gone() {
    let router = router();

    for (case, pattern) in pattern_table() {
        let set_id = create_pattern_set(&router, &format!("insert-{case}")).await;
        assert!(
            stored_patterns(&router, &set_id).await.is_empty(),
            "case {case:?}: a new set holds no patterns"
        );

        let (body, token) = apply_updates(&router, &set_id, &[update("INSERT", &pattern)]).await;
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: UpdateRegexPatternSet must echo the change token it consumed, \
             got {body}"
        );

        let after_insert = stored_patterns(&router, &set_id).await;
        assert_eq!(
            after_insert,
            vec![pattern.clone()],
            "case {case:?}: expected the inserted pattern to be read back, got {after_insert:?}"
        );
        // A read changes nothing a later caller sees.
        let again = stored_patterns(&router, &set_id).await;
        assert_eq!(
            again, after_insert,
            "case {case:?}: a second read must report the same patterns, got {again:?}"
        );

        apply_updates(&router, &set_id, &[update("DELETE", &pattern)]).await;
        let after_delete = stored_patterns(&router, &set_id).await;
        assert!(
            after_delete.is_empty(),
            "case {case:?}: expected the deleted pattern to be gone, got {after_delete:?}"
        );
    }
}

/// One request carries any number of updates, applied in the order given: the set a later
/// read reports is the one those updates leave behind.
#[tokio::test]
async fn a_batch_of_updates_applies_in_the_order_it_is_given() {
    let router = router();
    let first = "^alpha$";
    let second = "beta[0-9]";
    let third = r"gamma\s+";

    // Rows: the batch to apply, and the patterns the set must hold afterwards. Each row
    // starts from the set the previous one left, so the batches differ in count and in
    // their mix of actions.
    let rows: Vec<(&str, Vec<Value>, Vec<&str>)> = vec![
        (
            "three inserts in one request",
            vec![
                update("INSERT", first),
                update("INSERT", second),
                update("INSERT", third),
            ],
            vec![first, second, third],
        ),
        (
            "one delete",
            vec![update("DELETE", second)],
            vec![first, third],
        ),
        (
            "a delete and an insert in one request",
            vec![update("DELETE", first), update("INSERT", second)],
            vec![third, second],
        ),
        (
            "delete and re-insert the same pattern",
            vec![update("DELETE", third), update("INSERT", third)],
            vec![second, third],
        ),
        (
            "empty the set",
            vec![update("DELETE", second), update("DELETE", third)],
            vec![],
        ),
    ];

    let set_id = create_pattern_set(&router, "batched").await;
    for (case, updates, expected) in rows {
        apply_updates(&router, &set_id, &updates).await;
        let actual = stored_patterns(&router, &set_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// "The operation failed because there was nothing to do": an `INSERT` of a pattern the
/// set already holds and a `DELETE` of one it does not are each refused, and neither
/// changes the set or consumes the token it carried.
#[tokio::test]
async fn an_update_that_would_change_nothing_is_refused() {
    let router = router();
    let set_id = create_pattern_set(&router, "nothing-to-do").await;
    let held = "^held-[a-z]+$";
    apply_updates(&router, &set_id, &[update("INSERT", held)]).await;

    for (case, updates) in [
        ("insert a pattern the set holds", vec![update("INSERT", held)]),
        (
            "delete a pattern the set does not hold",
            vec![update("DELETE", "^absent$")],
        ),
        (
            "a batch whose last update changes nothing",
            vec![update("INSERT", "^new$"), update("INSERT", held)],
        ),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateRegexPatternSet",
            &json!({"RegexPatternSetId": set_id, "ChangeToken": token, "Updates": updates}),
            "WAFInvalidOperationException",
            case,
        )
        .await;

        let actual = stored_patterns(&router, &set_id).await;
        assert_eq!(
            actual,
            vec![held.to_string()],
            "case {case:?}: a refused update must leave the set as it was, got {actual:?}"
        );
        let reported = token_status(&router, &token).await;
        assert_eq!(
            reported, "PROVISIONED",
            "case {case:?}: a refused update must leave its change token unconsumed, \
             got {reported}"
        );
    }
}

/// The updates go INTO a set, so a `RegexPatternSetId` nothing is stored under is the
/// modelled `WAFNonexistentContainerException` — including the id of another resource
/// type, since each type is its own keyspace.
#[tokio::test]
async fn an_id_no_pattern_set_is_stored_under_is_refused() {
    let router = router();
    // One stored set, so the refusals below are about the id, not an empty store.
    let stored = create_pattern_set(&router, "stored").await;

    let ip_token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "not-a-pattern-set", "ChangeToken": ip_token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {ip_set}"))
        .to_string();

    for (case, set_id) in [
        (
            "a uuid-shaped id nothing was created under",
            "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
        ),
        ("another resource type's id", ip_set_id),
        ("an opaque non-uuid id", "not-an-id".to_string()),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "UpdateRegexPatternSet",
            &json!({
                "RegexPatternSetId": set_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", "^any$")],
            }),
            "WAFNonexistentContainerException",
            case,
        )
        .await;
    }

    assert!(
        stored_patterns(&router, &stored).await.is_empty(),
        "the refusals must have stored nothing"
    );
}

/// "Each create, update, or delete request must use a unique change token", so a token an
/// update already consumed — and one this service never minted — is stale data, and
/// neither request changes the set.
#[tokio::test]
async fn an_update_consumes_its_change_token_exactly_once() {
    let router = router();
    let set_id = create_pattern_set(&router, "token-protocol").await;
    let held = "^held$";
    let (_, token) = apply_updates(&router, &set_id, &[update("INSERT", held)]).await;

    for (case, stale) in [
        ("the token the update consumed", token.clone()),
        (
            "a token never minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "UpdateRegexPatternSet",
            &json!({
                "RegexPatternSetId": set_id,
                "ChangeToken": stale,
                "Updates": [update("INSERT", "^never-stored$")],
            }),
            "WAFStaleDataException",
            case,
        )
        .await;
    }

    let actual = stored_patterns(&router, &set_id).await;
    assert_eq!(
        actual,
        vec![held.to_string()],
        "a refused update must leave the set as it was, got {actual:?}"
    );
}

/// A set holds at most `MAX_REGEX_PATTERN_STRINGS` patterns — the bound its own modelled
/// shape carries — and one request carries at most
/// `MAX_REGEX_PATTERN_SET_UPDATES_PER_REQUEST` updates: past either, the modelled
/// `WAFLimitsExceededException`, with nothing further stored.
#[tokio::test]
async fn updates_past_the_modelled_bounds_are_refused() {
    let router = router();
    let set_id = create_pattern_set(&router, "bounded").await;

    let filling: Vec<Value> = (0..MAX_REGEX_PATTERN_STRINGS)
        .map(|index| update("INSERT", &format!("^pattern-{index}$")))
        .collect();
    apply_updates(&router, &set_id, &filling).await;
    let full = stored_patterns(&router, &set_id).await;
    assert_eq!(
        full.len(),
        MAX_REGEX_PATTERN_STRINGS,
        "a set must hold every pattern inserted up to its bound, got {full:?}"
    );

    // Rows: one insert past the set's bound, and one update past what a single request
    // may carry.
    let rows: Vec<(String, Vec<Value>)> = vec![
        (
            format!("one past the {MAX_REGEX_PATTERN_STRINGS}-pattern bound"),
            vec![update("INSERT", "^one-too-many$")],
        ),
        (
            format!("one past the {MAX_REGEX_PATTERN_SET_UPDATES_PER_REQUEST} updates a request may carry"),
            (0..=MAX_REGEX_PATTERN_SET_UPDATES_PER_REQUEST)
                .map(|index| update("DELETE", &format!("^pattern-{index}$")))
                .collect(),
        ),
    ];

    for (case, updates) in rows {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateRegexPatternSet",
            &json!({"RegexPatternSetId": set_id, "ChangeToken": token, "Updates": updates}),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
        assert!(
            error_code(&body).contains("WAFLimitsExceededException"),
            "case {case:?}: expected WAFLimitsExceededException, got {} ({body})",
            error_code(&body)
        );

        let actual = stored_patterns(&router, &set_id).await;
        assert_eq!(
            actual, full,
            "case {case:?}: a refused update must leave the set as it was, got {actual:?}"
        );
        let reported = token_status(&router, &token).await;
        assert_eq!(
            reported, "PROVISIONED",
            "case {case:?}: a refused update must leave its change token unconsumed, \
             got {reported}"
        );
    }
}
