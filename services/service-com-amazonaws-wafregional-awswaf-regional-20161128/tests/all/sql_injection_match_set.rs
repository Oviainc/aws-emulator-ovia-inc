// SPDX-License-Identifier: CC0-1.0

//! `CreateSqlInjectionMatchSet` and `GetSqlInjectionMatchSet`, end to end over the
//! wire.
//!
//! The rule under test: a create call consumes its change token exactly once, mints a
//! fresh `ResourceId` for the resource, persists the named resource with an EMPTY
//! collection of members, answers with the projection of the record it stored plus the
//! token it consumed, and the matching Get reads that same record back for any id the
//! store holds — refusing every other id with the modelled not-found error.
//!
//! Each row of the tables below is one input along what that rule generalises over
//! (name length, name characters, names shared between two resources, how many sets
//! exist, which id is read), so no assertion depends on the one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::MAX_SQL_INJECTION_MATCH_SETS;

// The wire harness is shared with the other suites in this crate.
use crate::wire::{call, change_token, expect_error, is_resource_id, ok_call, router};

/// Create a SqlInjectionMatchSet named `name` and return `(SqlInjectionMatchSetId, the
/// create response)`.
pub(crate) async fn create_set(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateSqlInjectionMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    let id = body["SqlInjectionMatchSet"]["SqlInjectionMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSqlInjectionMatchSet returned no id: {body}"))
        .to_string();
    (id, body)
}

/// The `SqlInjectionMatchSet` member of `GetSqlInjectionMatchSet`'s answer for `id`.
pub(crate) async fn get_set(router: &MultiServiceRouter, id: &str) -> Value {
    let body = ok_call(
        router,
        "GetSqlInjectionMatchSet",
        &json!({"SqlInjectionMatchSetId": id}),
    )
    .await;
    body["SqlInjectionMatchSet"].clone()
}

/// `CreateSqlInjectionMatchSet` stores a new, empty match set under a minted id, and
/// `GetSqlInjectionMatchSet` reads that record back — over a table of names differing
/// in length, case and the characters they use. `ResourceName` carries no uniqueness
/// constraint (`smithy_query` shows only `len 1..128, pattern \S`), so the last two
/// rows share a name and must still be two distinct resources.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_sql_injection_match_set() {
    let router = router();
    let max_length_name = "n".repeat(128);
    let cases: &[(&str, &str)] = &[
        // (label, Name) — `ResourceName` is len 1..128, pattern \S.
        ("the issue's own name", "sql-injection-roundtrip"),
        ("one character (the minimum length)", "x"),
        ("128 characters (the maximum length)", max_length_name.as_str()),
        ("mixed case, digits, separators", "Prod_SQLi-2"),
        ("an interior space", "corp login forms"),
        ("a name shared with the next row", "shared-name"),
        ("the same name again", "shared-name"),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (label, name) in cases {
        let (id, created) = create_set(&router, name).await;

        assert!(
            is_resource_id(&id),
            "case {label}: the id must be a minted ResourceId, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {label}: every create mints a distinct id, but {id:?} was already \
             returned by an earlier create ({seen_ids:?})"
        );
        seen_ids.push(id.clone());

        assert_eq!(
            created["SqlInjectionMatchSet"]["Name"].as_str(),
            Some(*name),
            "case {label}: the stored name must come back, got {created}"
        );
        assert_eq!(
            created["SqlInjectionMatchSet"]["SqlInjectionMatchTuples"].as_array(),
            Some(&vec![]),
            "case {label}: a newly created set inspects nothing yet, got {created}"
        );

        // The round trip: the response above is the projection of a PERSISTED record,
        // so a separate read of the same id answers the same thing.
        let fetched = get_set(&router, &id).await;
        assert_eq!(
            fetched, created["SqlInjectionMatchSet"],
            "case {label}: GetSqlInjectionMatchSet must read back the created record, \
             got {fetched} after {created}"
        );
    }

    // Every record created above is still readable at its own id: one create never
    // overwrites another, not even when the names are equal.
    for (index, id) in seen_ids.iter().enumerate() {
        let fetched = get_set(&router, id).await;
        assert_eq!(
            fetched["Name"].as_str(),
            Some(cases[index].1),
            "case {}: the record at {id:?} must keep its own name, got {fetched}",
            cases[index].0
        );
    }
}

/// A read reports; it never changes what a later caller sees. Reading one set twice,
/// and reading another set in between, leaves every record exactly as it was.
#[tokio::test]
async fn getting_a_match_set_changes_nothing_for_a_later_caller() {
    let router = router();
    let (first, _) = create_set(&router, "read-probe-one").await;
    let (second, _) = create_set(&router, "read-probe-two").await;

    let before = get_set(&router, &first).await;
    let other = get_set(&router, &second).await;
    let after = get_set(&router, &first).await;

    assert_eq!(
        after, before,
        "case repeated read: GetSqlInjectionMatchSet must be stable, got {after} after \
         {before}"
    );
    assert_eq!(
        other["SqlInjectionMatchSetId"].as_str(),
        Some(second.as_str()),
        "case neighbouring read: the second set must report its own id, got {other}"
    );
}

/// `GetSqlInjectionMatchSet` answers the modelled `WAFNonexistentItemException` for
/// every id the store does not hold — including ids this service really did mint, for
/// a resource of ANOTHER type.
#[tokio::test]
async fn an_unknown_match_set_id_is_a_modelled_not_found() {
    let router = router();

    // A real id of a different resource type: minted by this service, but not a
    // SqlInjectionMatchSetId.
    let token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "other-type", "ChangeToken": token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .expect("CreateIPSet returned an IPSetId")
        .to_string();

    let (known, _) = create_set(&router, "known-set").await;

    let cases: &[(&str, &str)] = &[
        ("a UUID-shaped id nothing was created under", "11111111-2222-4333-8444-555555555555"),
        ("an id of a different resource type", ip_set_id.as_str()),
        ("a one-character id", "x"),
        ("a 128-character id (the maximum length)", &"i".repeat(128)),
    ];
    for (label, id) in cases {
        expect_error(
            &router,
            "GetSqlInjectionMatchSet",
            &json!({"SqlInjectionMatchSetId": id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
    }

    // The refusals above left the one stored record readable.
    let fetched = get_set(&router, &known).await;
    assert_eq!(
        fetched["SqlInjectionMatchSetId"].as_str(),
        Some(known.as_str()),
        "case after the refusals: the stored set must still read back, got {fetched}"
    );
}

/// The change-token protocol, on this operation: the create reports the token it
/// consumed, and no later mutating call may use that token again, since "each create,
/// update, or delete request must use a unique change token".
#[tokio::test]
async fn create_consumes_its_change_token_exactly_once() {
    let router = router();

    let token = change_token(&router).await;
    let created = ok_call(
        &router,
        "CreateSqlInjectionMatchSet",
        &json!({"Name": "token-probe", "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        created["ChangeToken"].as_str(),
        Some(token.as_str()),
        "the create must report the change token it consumed: {created}"
    );

    for (label, candidate) in [
        ("already consumed", token.clone()),
        (
            "never minted",
            "99999999-8888-4777-8666-555555555555".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "CreateSqlInjectionMatchSet",
            &json!({"Name": "stale-token-probe", "ChangeToken": candidate}),
            "WAFStaleDataException",
            label,
        )
        .await;
    }

    // A refused create persisted nothing: the only readable set is the first one.
    let id = created["SqlInjectionMatchSet"]["SqlInjectionMatchSetId"]
        .as_str()
        .expect("the create returned an id");
    let fetched = get_set(&router, id).await;
    assert_eq!(
        fetched["Name"].as_str(),
        Some("token-probe"),
        "the one stored set must be the one the accepted create made, got {fetched}"
    );
}

/// The number of match sets a caller can store is bounded, and the bound is reported
/// as the modelled `WAFLimitsExceededException` — at the quota, not before it.
#[tokio::test]
async fn create_refuses_past_the_match_set_quota() {
    let router = router();

    for index in 0..MAX_SQL_INJECTION_MATCH_SETS {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "CreateSqlInjectionMatchSet",
            &json!({"Name": format!("quota-{index}"), "ChangeToken": token}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case set {index} of {MAX_SQL_INJECTION_MATCH_SETS}: a create inside the \
             quota must be accepted, got {status}: {body}"
        );
    }

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateSqlInjectionMatchSet",
        &json!({"Name": "one-too-many", "ChangeToken": token}),
        "WAFLimitsExceededException",
        "one past the quota",
    )
    .await;
}
