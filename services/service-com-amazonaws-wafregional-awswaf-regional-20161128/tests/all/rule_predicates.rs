// SPDX-License-Identifier: CC0-1.0

//! `UpdateRule`, end to end over the wire, and the reference it creates: `CreateRule`,
//! `UpdateRule`, `GetRule`, `DeleteIPSet`, `DeleteXssMatchSet`.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.
//!
//! The rules under test are `UpdateRule`'s own documentation — "Inserts or deletes
//! Predicate objects in a Rule", each predicate identifying "a predicate, such as a
//! ByteMatchSet or an IPSet", whose `DataId` is "a unique identifier for a predicate in
//! a Rule, such as ByteMatchSetId or IPSetId \[…\] returned by the corresponding Create
//! or List command" — and the sentence every condition kind's delete carries: "You can't
//! delete an IPSet if it's still used in any Rules or if it still includes any IP
//! addresses."

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_RULE_PREDICATES, MAX_RULE_UPDATES_PER_REQUEST, MESSAGE_INVALID_OPERATION,
    MESSAGE_LIMITS_EXCEEDED, MESSAGE_NONEXISTENT_CONTAINER, MESSAGE_NONEXISTENT_ITEM,
    MESSAGE_REFERENCED_ITEM, MESSAGE_STALE_DATA,
};

use crate::wire::{call, change_token, error_message, expect_error, ok_call, router, token_status};

/// A UUID-shaped id this service never minted.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Create a rule and return its `RuleId`.
async fn create_rule(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRule",
        &json!({"Name": name, "MetricName": "rulePredicates", "ChangeToken": token}),
    )
    .await;
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRule returned no RuleId: {body}"))
        .to_string()
}

/// Create an `IPSet` and return its `IPSetId`.
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

/// Create a `SqlInjectionMatchSet` and return its `SqlInjectionMatchSetId`.
async fn create_sql_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateSqlInjectionMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["SqlInjectionMatchSet"]["SqlInjectionMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSqlInjectionMatchSet returned no id: {body}"))
        .to_string()
}

/// Create an `XssMatchSet` and return its `XssMatchSetId`.
async fn create_xss_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateXssMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["XssMatchSet"]["XssMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateXssMatchSet returned no XssMatchSetId: {body}"))
        .to_string()
}

/// Create a `SizeConstraintSet` and return its `SizeConstraintSetId`.
async fn create_size_constraint_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateSizeConstraintSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["SizeConstraintSet"]["SizeConstraintSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSizeConstraintSet returned no id: {body}"))
        .to_string()
}

/// Create a `ByteMatchSet` and return its `ByteMatchSetId`.
async fn create_byte_match_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateByteMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["ByteMatchSet"]["ByteMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateByteMatchSet returned no ByteMatchSetId: {body}"))
        .to_string()
}

/// Create a `GeoMatchSet` and return its `GeoMatchSetId`.
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

/// Create a `RegexMatchSet` and return its `RegexMatchSetId`.
async fn create_regex_match_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRegexMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["RegexMatchSet"]["RegexMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRegexMatchSet returned no RegexMatchSetId: {body}"))
        .to_string()
}

/// One `RuleUpdate` on the wire.
fn update(action: &str, negated: bool, kind: &str, data_id: &str) -> Value {
    json!({
        "Action": action,
        "Predicate": {"Negated": negated, "Type": kind, "DataId": data_id},
    })
}

/// The modelled `Predicate` a `GetRule` answer carries for `update`'s predicate.
fn predicate(negated: bool, kind: &str, data_id: &str) -> Value {
    json!({"Negated": negated, "Type": kind, "DataId": data_id})
}

/// Apply `updates` to `rule_id` with a fresh change token, answering
/// `(http status, body, the token the request carried)`.
async fn apply_updates(
    router: &MultiServiceRouter,
    rule_id: &str,
    updates: Vec<Value>,
) -> (u16, Value, String) {
    let token = change_token(router).await;
    let (status, body) = call(
        router,
        "UpdateRule",
        &json!({"RuleId": rule_id, "ChangeToken": token, "Updates": updates}),
    )
    .await;
    (status, body, token)
}

/// The `Predicates` `GetRule` reports for `rule_id`, in stored order.
async fn predicates(router: &MultiServiceRouter, rule_id: &str) -> Vec<Value> {
    let body = ok_call(router, "GetRule", &json!({"RuleId": rule_id})).await;
    body["Rule"]["Predicates"]
        .as_array()
        .unwrap_or_else(|| panic!("GetRule returned no Predicates: {body}"))
        .clone()
}

/// An `INSERT` adds the named predicate to the stored rule and a `DELETE` removes it,
/// across every condition kind this service stores and both `Negated` settings — the
/// round trip read back through `GetRule`, which reports the stored record.
#[tokio::test]
async fn insert_then_delete_round_trips_a_predicate_of_every_condition_kind() {
    let router = router();
    let ip_set = create_ip_set(&router, "predicate-ip-set").await;
    let sql_set = create_sql_set(&router, "predicate-sql-set").await;
    let xss_set = create_xss_set(&router, "predicate-xss-set").await;
    let size_set = create_size_constraint_set(&router, "predicate-size-set").await;
    let byte_set = create_byte_match_set(&router, "predicate-byte-set").await;
    let geo_set = create_geo_match_set(&router, "predicate-geo-set").await;
    let regex_set = create_regex_match_set(&router, "predicate-regex-set").await;

    // (label, PredicateType, the id of a stored set of that kind, Negated) — the
    // `PredicateType` values this service holds a condition store for, each with and
    // without negation.
    let cases: &[(&str, &str, &str, bool)] = &[
        ("an IPSet predicate", "IPMatch", ip_set.as_str(), false),
        ("a negated IPSet predicate", "IPMatch", ip_set.as_str(), true),
        (
            "a SqlInjectionMatchSet predicate",
            "SqlInjectionMatch",
            sql_set.as_str(),
            false,
        ),
        (
            "a negated SqlInjectionMatchSet predicate",
            "SqlInjectionMatch",
            sql_set.as_str(),
            true,
        ),
        ("an XssMatchSet predicate", "XssMatch", xss_set.as_str(), false),
        (
            "a negated XssMatchSet predicate",
            "XssMatch",
            xss_set.as_str(),
            true,
        ),
        (
            "a SizeConstraintSet predicate",
            "SizeConstraint",
            size_set.as_str(),
            false,
        ),
        (
            "a negated SizeConstraintSet predicate",
            "SizeConstraint",
            size_set.as_str(),
            true,
        ),
        (
            "a ByteMatchSet predicate",
            "ByteMatch",
            byte_set.as_str(),
            false,
        ),
        (
            "a negated ByteMatchSet predicate",
            "ByteMatch",
            byte_set.as_str(),
            true,
        ),
        ("a GeoMatchSet predicate", "GeoMatch", geo_set.as_str(), false),
        (
            "a negated GeoMatchSet predicate",
            "GeoMatch",
            geo_set.as_str(),
            true,
        ),
        (
            "a RegexMatchSet predicate",
            "RegexMatch",
            regex_set.as_str(),
            false,
        ),
        (
            "a negated RegexMatchSet predicate",
            "RegexMatch",
            regex_set.as_str(),
            true,
        ),
    ];

    for (label, kind, data_id, negated) in cases {
        let rule = create_rule(&router, "predicate-round-trip").await;
        let (status, body, token) =
            apply_updates(&router, &rule, vec![update("INSERT", *negated, kind, data_id)]).await;
        assert_eq!(
            status, 200,
            "case {label}: UpdateRule must accept the insert, got {status}: {body}"
        );
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: UpdateRule must report the change token it consumed, got {body}"
        );
        assert_eq!(
            predicates(&router, &rule).await,
            vec![predicate(*negated, kind, data_id)],
            "case {label}: GetRule must report the inserted predicate"
        );

        let (status, body, _) =
            apply_updates(&router, &rule, vec![update("DELETE", *negated, kind, data_id)]).await;
        assert_eq!(
            status, 200,
            "case {label}: UpdateRule must accept the delete, got {status}: {body}"
        );
        let remaining = predicates(&router, &rule).await;
        assert!(
            remaining.is_empty(),
            "case {label}: the deleted predicate must be gone, got {remaining:?}"
        );
    }
}

/// One request carries several updates and applies them in order: the predicates a rule
/// holds are the ones inserted, in insertion order, and a later request deleting some of
/// them leaves the rest in that order.
#[tokio::test]
async fn one_request_applies_several_updates_in_order() {
    let router = router();
    let first_ip = create_ip_set(&router, "batch-ip-one").await;
    let second_ip = create_ip_set(&router, "batch-ip-two").await;
    let sql_set = create_sql_set(&router, "batch-sql").await;
    let xss_set = create_xss_set(&router, "batch-xss").await;
    let size_set = create_size_constraint_set(&router, "batch-size").await;
    let rule = create_rule(&router, "batch-rule").await;

    let inserts = vec![
        update("INSERT", false, "IPMatch", &first_ip),
        update("INSERT", true, "IPMatch", &second_ip),
        update("INSERT", false, "SqlInjectionMatch", &sql_set),
        update("INSERT", true, "XssMatch", &xss_set),
        update("INSERT", false, "SizeConstraint", &size_set),
    ];
    let (status, body, _) = apply_updates(&router, &rule, inserts).await;
    assert_eq!(
        status, 200,
        "a request carrying five inserts must be accepted, got {status}: {body}"
    );
    assert_eq!(
        predicates(&router, &rule).await,
        vec![
            predicate(false, "IPMatch", &first_ip),
            predicate(true, "IPMatch", &second_ip),
            predicate(false, "SqlInjectionMatch", &sql_set),
            predicate(true, "XssMatch", &xss_set),
            predicate(false, "SizeConstraint", &size_set),
        ],
        "GetRule must report every inserted predicate, in insertion order"
    );

    // A mixed request: delete two of them and insert one back under the other
    // `Negated` setting.
    let mixed = vec![
        update("DELETE", false, "IPMatch", &first_ip),
        update("DELETE", false, "SqlInjectionMatch", &sql_set),
        update("INSERT", true, "SqlInjectionMatch", &sql_set),
    ];
    let (status, body, _) = apply_updates(&router, &rule, mixed).await;
    assert_eq!(
        status, 200,
        "a mixed delete/insert request must be accepted, got {status}: {body}"
    );
    assert_eq!(
        predicates(&router, &rule).await,
        vec![
            predicate(true, "IPMatch", &second_ip),
            predicate(true, "XssMatch", &xss_set),
            predicate(false, "SizeConstraint", &size_set),
            predicate(true, "SqlInjectionMatch", &sql_set),
        ],
        "the surviving predicates keep their order and the re-inserted one goes last"
    );
}

/// An inserted predicate's `DataId` must resolve to a stored condition set OF ITS OWN
/// `Type`; anything else is the modelled "referenced object doesn't exist", and the
/// refused request changes nothing.
#[tokio::test]
async fn insert_refuses_a_predicate_whose_condition_set_does_not_exist() {
    let router = router();
    let ip_set = create_ip_set(&router, "resolution-ip-set").await;
    let xss_set = create_xss_set(&router, "resolution-xss-set").await;
    let byte_set = create_byte_match_set(&router, "resolution-byte-set").await;
    let rule = create_rule(&router, "resolution-rule").await;

    // (label, PredicateType, DataId) — every `PredicateType` comes from the model's
    // `PREDICATE_TYPE_VALUES` (IPMatch, ByteMatch, SqlInjectionMatch, GeoMatch,
    // SizeConstraint, XssMatch, RegexMatch).
    let cases: &[(&str, &str, &str)] = &[
        ("an IPSet id that was never minted", "IPMatch", UNKNOWN_ID),
        (
            "a SqlInjectionMatchSet id that was never minted",
            "SqlInjectionMatch",
            UNKNOWN_ID,
        ),
        (
            "an XssMatchSet id that was never minted",
            "XssMatch",
            UNKNOWN_ID,
        ),
        (
            "a SizeConstraintSet id that was never minted",
            "SizeConstraint",
            UNKNOWN_ID,
        ),
        (
            "a ByteMatchSet id that was never minted",
            "ByteMatch",
            UNKNOWN_ID,
        ),
        (
            "a GeoMatchSet id that was never minted",
            "GeoMatch",
            UNKNOWN_ID,
        ),
        ("the zero id", "IPMatch", "00000000-0000-0000-0000-000000000000"),
        ("a one-character id", "XssMatch", "x"),
        // A stored id of ANOTHER condition kind: the type decides which store the id
        // is resolved against.
        ("an XssMatchSet id used as an IPSet", "IPMatch", xss_set.as_str()),
        (
            "an IPSet id used as an XssMatchSet",
            "XssMatch",
            ip_set.as_str(),
        ),
        (
            "an IPSet id used as a SizeConstraintSet",
            "SizeConstraint",
            ip_set.as_str(),
        ),
        (
            "an IPSet id used as a GeoMatchSet",
            "GeoMatch",
            ip_set.as_str(),
        ),
        (
            "a ByteMatchSet id used as an XssMatchSet",
            "XssMatch",
            byte_set.as_str(),
        ),
        // A `RegexMatch` `DataId` resolves against the `RegexMatchSet`s, which
        // `CreateRegexMatchSet` stores: an id no regex match set was created under does
        // not resolve — not even one a stored set of another kind carries.
        ("a RegexMatchSet id that was never minted", "RegexMatch", UNKNOWN_ID),
        (
            "a stored ByteMatchSet id used as a RegexMatchSet",
            "RegexMatch",
            byte_set.as_str(),
        ),
    ];

    for (label, kind, data_id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRule",
            &json!({
                "RuleId": rule,
                "ChangeToken": token,
                "Updates": [update("INSERT", false, kind, data_id)],
            }),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: the refusal carries WAF Classic's fixed not-found sentence, \
             got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRule must not consume the change token"
        );
        let stored = predicates(&router, &rule).await;
        assert!(
            stored.is_empty(),
            "case {label}: a refused UpdateRule must persist no predicate, got {stored:?}"
        );
    }
}

/// An update that would leave the rule exactly as it was is the modelled "there was
/// nothing to do": an `INSERT` of a predicate the rule already holds, and a `DELETE` of
/// one it does not — `Negated`, `Type` and `DataId` together being the predicate's
/// identity.
#[tokio::test]
async fn an_update_that_changes_nothing_is_refused() {
    let router = router();
    let ip_set = create_ip_set(&router, "identity-ip-set").await;
    let xss_set = create_xss_set(&router, "identity-xss-set").await;
    let rule = create_rule(&router, "identity-rule").await;
    let held = update("INSERT", false, "IPMatch", &ip_set);
    let (status, body, _) = apply_updates(&router, &rule, vec![held]).await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    // (label, the update that must change nothing)
    let cases: &[(&str, Value)] = &[
        (
            "inserting the predicate the rule already holds",
            update("INSERT", false, "IPMatch", &ip_set),
        ),
        (
            "deleting it with the other Negated setting",
            update("DELETE", true, "IPMatch", &ip_set),
        ),
        (
            "deleting a predicate of another type for a stored set",
            update("DELETE", false, "XssMatch", &xss_set),
        ),
        (
            "deleting a predicate for an id the rule never held",
            update("DELETE", false, "IPMatch", UNKNOWN_ID),
        ),
    ];

    for (label, candidate) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRule",
            &json!({"RuleId": rule, "ChangeToken": token, "Updates": [candidate]}),
            "WAFInvalidOperationException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_INVALID_OPERATION,
            "case {label}: the refusal carries WAF Classic's fixed nothing-to-do \
             sentence, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRule must not consume the change token"
        );
        assert_eq!(
            predicates(&router, &rule).await,
            vec![predicate(false, "IPMatch", &ip_set)],
            "case {label}: the stored rule must be unchanged by the refusal"
        );
    }
}

/// A request is applied whole or not at all: when a later update is refused, the earlier
/// ones in the same request are not persisted and the change token is still unspent.
#[tokio::test]
async fn a_request_refused_part_way_through_persists_nothing() {
    let router = router();
    let ip_set = create_ip_set(&router, "atomic-ip-set").await;
    let xss_set = create_xss_set(&router, "atomic-xss-set").await;
    let rule = create_rule(&router, "atomic-rule").await;

    // (label, the updates one request carries) — each row's first update is applicable
    // and a later one is not.
    let cases: &[(&str, Vec<Value>)] = &[
        (
            "a second update naming a condition set that does not exist",
            vec![
                update("INSERT", false, "IPMatch", &ip_set),
                update("INSERT", false, "XssMatch", UNKNOWN_ID),
            ],
        ),
        (
            "a second update that changes nothing",
            vec![
                update("INSERT", false, "IPMatch", &ip_set),
                update("DELETE", false, "XssMatch", &xss_set),
            ],
        ),
        (
            "an update repeated inside one request",
            vec![
                update("INSERT", true, "XssMatch", &xss_set),
                update("INSERT", true, "XssMatch", &xss_set),
            ],
        ),
    ];

    for (label, updates) in cases {
        let (status, body, token) = apply_updates(&router, &rule, updates.clone()).await;
        assert_eq!(
            status, 400,
            "case {label}: the request must be refused, got {status}: {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRule must not consume the change token"
        );
        let stored = predicates(&router, &rule).await;
        assert!(
            stored.is_empty(),
            "case {label}: no update of a refused request may be persisted, got {stored:?}"
        );
    }
}

/// `UpdateRule` for a rule this service holds nothing for is the modelled
/// nonexistent-container refusal: the rule is the container the predicates go into.
#[tokio::test]
async fn an_unknown_rule_id_is_a_modelled_nonexistent_container() {
    let router = router();
    let ip_set = create_ip_set(&router, "container-ip-set").await;

    let cases: &[(&str, &str)] = &[
        ("a UUID-shaped id that was never minted", UNKNOWN_ID),
        ("the zero id", "00000000-0000-0000-0000-000000000000"),
        ("a one-character id", "x"),
        // An id this service minted for ANOTHER resource kind is not a RuleId.
        ("the id of a stored IPSet", ip_set.as_str()),
    ];

    for (label, rule_id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRule",
            &json!({
                "RuleId": rule_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", false, "IPMatch", &ip_set)],
            }),
            "WAFNonexistentContainerException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_CONTAINER,
            "case {label}: the refusal carries WAF Classic's fixed container sentence, \
             got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRule must not consume the change token"
        );
    }
}

/// "Each create, update, or delete request must use a unique change token": a token
/// already spent and one no `GetChangeToken` minted are both stale data, and such a
/// request changes nothing.
#[tokio::test]
async fn update_refuses_a_stale_or_unknown_change_token() {
    let router = router();
    let ip_set = create_ip_set(&router, "token-ip-set").await;
    let other_ip_set = create_ip_set(&router, "token-ip-set-two").await;
    let rule = create_rule(&router, "token-rule").await;
    let (status, body, spent) =
        apply_updates(&router, &rule, vec![update("INSERT", false, "IPMatch", &ip_set)]).await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    for (label, token) in [
        ("a token this UpdateRule already spent", spent.as_str()),
        ("a token no GetChangeToken minted", UNKNOWN_ID),
    ] {
        let body = expect_error(
            &router,
            "UpdateRule",
            &json!({
                "RuleId": rule,
                "ChangeToken": token,
                "Updates": [update("INSERT", false, "IPMatch", &other_ip_set)],
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_STALE_DATA,
            "case {label}: the refusal carries WAF Classic's fixed stale-token sentence, \
             got {:?}",
            error_message(&body)
        );
        assert_eq!(
            predicates(&router, &rule).await,
            vec![predicate(false, "IPMatch", &ip_set)],
            "case {label}: a refused UpdateRule must persist nothing"
        );
    }
}

/// The predicate state one caller can grow is bounded: inserts are accepted up to the
/// per-rule predicate quota and refused past it, and a request carrying more updates
/// than one request may carry is refused before any of them is applied.
#[tokio::test]
async fn update_refuses_past_the_predicate_quota() {
    let router = router();
    let rule = create_rule(&router, "quota-rule").await;

    // One IPSet per predicate the rule may hold, plus one more to insert past the
    // quota.
    let mut ip_sets = Vec::new();
    for index in 0..=MAX_RULE_PREDICATES {
        ip_sets.push(create_ip_set(&router, &format!("quota-ip-set-{index}")).await);
    }

    for (index, ip_set) in ip_sets.iter().take(MAX_RULE_PREDICATES).enumerate() {
        let (status, body, _) =
            apply_updates(&router, &rule, vec![update("INSERT", false, "IPMatch", ip_set)]).await;
        assert_eq!(
            status, 200,
            "case predicate {index}: every insert up to the quota must be accepted, \
             got {status}: {body}"
        );
    }
    assert_eq!(
        predicates(&router, &rule).await.len(),
        MAX_RULE_PREDICATES,
        "the rule must hold every predicate up to the quota"
    );

    // (label, the updates one request carries) — one insert past the per-rule quota,
    // and one request past the per-request update bound.
    let past_quota = vec![update(
        "INSERT",
        false,
        "IPMatch",
        &ip_sets[MAX_RULE_PREDICATES],
    )];
    let past_request_bound: Vec<Value> = (0..=MAX_RULE_UPDATES_PER_REQUEST)
        .map(|index| {
            update(
                "DELETE",
                false,
                "IPMatch",
                &ip_sets[index % ip_sets.len()],
            )
        })
        .collect();
    for (label, updates) in [
        ("one predicate past the per-rule quota", past_quota),
        (
            "one update past what a single request may carry",
            past_request_bound,
        ),
    ] {
        let (status, body, token) = apply_updates(&router, &rule, updates).await;
        assert_eq!(
            status, 400,
            "case {label}: the request must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_message(&body),
            MESSAGE_LIMITS_EXCEEDED,
            "case {label}: the refusal carries WAF Classic's fixed limit sentence, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRule must not consume the change token"
        );
        assert_eq!(
            predicates(&router, &rule).await.len(),
            MAX_RULE_PREDICATES,
            "case {label}: the stored rule must be unchanged by the refusal"
        );
    }
}

/// While a rule's predicate points at a condition set, deleting that set is refused as a
/// referenced item — for every condition kind whose delete this service implements — and
/// the delete succeeds once `UpdateRule` has removed the predicate.
#[tokio::test]
async fn a_condition_set_a_rule_references_cannot_be_deleted() {
    let router = router();

    // (label, PredicateType, the Delete operation, its id member, a constructor for a
    // stored set of that kind)
    let ip_set = create_ip_set(&router, "referenced-ip-set").await;
    let xss_set = create_xss_set(&router, "referenced-xss-set").await;
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "an IPSet a rule's predicate points at",
            "IPMatch",
            "DeleteIPSet",
            "IPSetId",
            ip_set.as_str(),
        ),
        (
            "an XssMatchSet a rule's predicate points at",
            "XssMatch",
            "DeleteXssMatchSet",
            "XssMatchSetId",
            xss_set.as_str(),
        ),
    ];

    for (label, kind, operation, id_member, set_id) in cases {
        let rule = create_rule(&router, "referencing-rule").await;
        let (status, body, _) =
            apply_updates(&router, &rule, vec![update("INSERT", false, kind, set_id)]).await;
        assert_eq!(
            status, 200,
            "case {label}: the setup insert must be accepted, got {status}: {body}"
        );

        let token = change_token(&router).await;
        let mut request = json!({"ChangeToken": token});
        request[*id_member] = json!(set_id);
        let body = expect_error(
            &router,
            operation,
            &request,
            "WAFReferencedItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_REFERENCED_ITEM,
            "case {label}: the refusal carries WAF Classic's fixed in-use sentence, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused delete must not consume the change token"
        );

        // Once the predicate is gone, nothing references the set and the delete lands.
        let (status, body, _) =
            apply_updates(&router, &rule, vec![update("DELETE", false, kind, set_id)]).await;
        assert_eq!(
            status, 200,
            "case {label}: removing the predicate must be accepted, got {status}: {body}"
        );
        let token = change_token(&router).await;
        let mut request = json!({"ChangeToken": token});
        request[*id_member] = json!(set_id);
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {label}: a set no rule references must be deletable, got {status}: {body}"
        );
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: the delete must report the change token it consumed, got {body}"
        );
    }
}
