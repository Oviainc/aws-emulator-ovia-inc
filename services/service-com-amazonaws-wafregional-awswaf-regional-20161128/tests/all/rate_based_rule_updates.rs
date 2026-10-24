// SPDX-License-Identifier: CC0-1.0

//! `UpdateRateBasedRule`, end to end over the wire, read back through
//! `GetRateBasedRule`: `CreateRateBasedRule`, `UpdateRateBasedRule`,
//! `GetRateBasedRule`, `CreateRule`, `UpdateRule` and the condition-kind creates and
//! deletes the predicates point at.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.
//!
//! The rule under test is `UpdateRateBasedRule`'s own documentation — "Inserts or
//! deletes Predicate objects in a rule and updates the RateLimit in the rule" — so one
//! accepted call applies every update it carries to the stored rate-based rule AND
//! writes the `RateLimit` it names, and a refused one applies none of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_RULE_PREDICATES, MAX_RULE_UPDATES_PER_REQUEST, MESSAGE_INVALID_OPERATION,
    MESSAGE_LIMITS_EXCEEDED, MESSAGE_NONEXISTENT_CONTAINER, MESSAGE_NONEXISTENT_ITEM,
    MESSAGE_STALE_DATA,
};

use crate::wire::{call, change_token, error_message, expect_error, ok_call, router, token_status};

/// A UUID-shaped id this service never minted.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// `RateLimit`'s modelled `@range`, from
/// `smithy_query(query="operation", operation="UpdateRateBasedRule")`:
/// `RateLimit (RateLimit, range 100..2000000000)`.
const RATE_LIMIT_MIN: i64 = 100;
/// The upper bound of that same `@range`.
const RATE_LIMIT_MAX: i64 = 2_000_000_000;

/// Create a rate-based rule at `rate_limit` and return its `RuleId`.
async fn create_rate_based_rule(
    router: &MultiServiceRouter,
    name: &str,
    rate_limit: i64,
) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRateBasedRule",
        &json!({
            "Name": name,
            "MetricName": "rateBasedRuleUpdates",
            "RateKey": "IP",
            "RateLimit": rate_limit,
            "ChangeToken": token,
        }),
    )
    .await;
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRateBasedRule returned no RuleId: {body}"))
        .to_string()
}

/// Create a condition set of the kind `operation`/`id_member` name and return its id.
async fn create_condition_set(
    router: &MultiServiceRouter,
    operation: &str,
    container: &str,
    id_member: &str,
    name: &str,
) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        operation,
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body[container][id_member]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} returned no {id_member}: {body}"))
        .to_string()
}

/// Every `PredicateType` the model declares, with the create that mints a set of that
/// kind: `smithy_query(query="shape", shape="PredicateType")` →
/// `["IPMatch", "ByteMatch", "SqlInjectionMatch", "GeoMatch", "SizeConstraint",
/// "XssMatch", "RegexMatch"]`.
const CONDITION_KINDS: [(&str, &str, &str, &str); 7] = [
    ("IPMatch", "CreateIPSet", "IPSet", "IPSetId"),
    (
        "ByteMatch",
        "CreateByteMatchSet",
        "ByteMatchSet",
        "ByteMatchSetId",
    ),
    (
        "SqlInjectionMatch",
        "CreateSqlInjectionMatchSet",
        "SqlInjectionMatchSet",
        "SqlInjectionMatchSetId",
    ),
    (
        "GeoMatch",
        "CreateGeoMatchSet",
        "GeoMatchSet",
        "GeoMatchSetId",
    ),
    (
        "SizeConstraint",
        "CreateSizeConstraintSet",
        "SizeConstraintSet",
        "SizeConstraintSetId",
    ),
    ("XssMatch", "CreateXssMatchSet", "XssMatchSet", "XssMatchSetId"),
    (
        "RegexMatch",
        "CreateRegexMatchSet",
        "RegexMatchSet",
        "RegexMatchSetId",
    ),
];

/// One `RuleUpdate`.
fn update(action: &str, negated: bool, kind: &str, data_id: &str) -> Value {
    json!({
        "Action": action,
        "Predicate": {"Negated": negated, "Type": kind, "DataId": data_id},
    })
}

/// The modelled `Predicate` a `GetRateBasedRule` answer carries.
fn predicate(negated: bool, kind: &str, data_id: &str) -> Value {
    json!({"Negated": negated, "Type": kind, "DataId": data_id})
}

/// Apply `updates` and `rate_limit` to `rule_id` with a fresh change token, answering
/// `(http status, body, the token the request carried)`.
async fn apply(
    router: &MultiServiceRouter,
    rule_id: &str,
    rate_limit: i64,
    updates: Vec<Value>,
) -> (u16, Value, String) {
    let token = change_token(router).await;
    let (status, body) = call(
        router,
        "UpdateRateBasedRule",
        &json!({
            "RuleId": rule_id,
            "ChangeToken": token,
            "RateLimit": rate_limit,
            "Updates": updates,
        }),
    )
    .await;
    (status, body, token)
}

/// The stored `(MatchPredicates, RateLimit)` of `rule_id`, as `GetRateBasedRule`
/// reports them.
async fn stored(router: &MultiServiceRouter, rule_id: &str) -> (Vec<Value>, i64) {
    let body = ok_call(router, "GetRateBasedRule", &json!({"RuleId": rule_id})).await;
    let predicates = body["Rule"]["MatchPredicates"]
        .as_array()
        .unwrap_or_else(|| panic!("GetRateBasedRule returned no MatchPredicates: {body}"))
        .clone();
    let rate_limit = body["Rule"]["RateLimit"]
        .as_i64()
        .unwrap_or_else(|| panic!("GetRateBasedRule returned no RateLimit: {body}"));
    (predicates, rate_limit)
}

/// An `INSERT` adds the named predicate to the stored rate-based rule and a `DELETE`
/// removes it, across every condition kind this service stores and both `Negated`
/// settings — and each call writes the `RateLimit` it carries. The round trip is read
/// back through `GetRateBasedRule`, which reports the stored record.
#[tokio::test]
async fn insert_then_delete_round_trips_a_predicate_of_every_condition_kind() {
    let router = router();
    let rule = create_rate_based_rule(&router, "kinds-rule", 1_000).await;

    // (PredicateType, a stored set of that kind, Negated, the RateLimit this row's
    // insert carries, the one its delete carries) — the rate limits walk each published
    // `@range` bound and values between them.
    let mut rows: Vec<(&str, String, bool, i64, i64)> = Vec::new();
    for (index, (kind, create, container, id_member)) in CONDITION_KINDS.iter().enumerate() {
        let set = create_condition_set(
            &router,
            create,
            container,
            id_member,
            &format!("kinds-set-{index}"),
        )
        .await;
        let negated = index % 2 == 1;
        let insert_limit = match index {
            0 => RATE_LIMIT_MIN,
            1 => RATE_LIMIT_MAX,
            other => 100 * (other as i64 + 1),
        };
        // The delete's own rate limit, a different value inside the same `@range`.
        let delete_limit = if insert_limit > RATE_LIMIT_MAX - 500 {
            insert_limit - 500
        } else {
            insert_limit + 500
        };
        rows.push((kind, set, negated, insert_limit, delete_limit));
    }

    for (kind, data_id, negated, insert_limit, delete_limit) in &rows {
        let case = format!("{kind}/negated={negated}/limit={insert_limit}");

        let (status, body, token) = apply(
            &router,
            &rule,
            *insert_limit,
            vec![update("INSERT", *negated, kind, data_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRateBasedRule must accept the insert, got {status}: {body}"
        );
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {case:?}: the response must report the change token it consumed, got {body}"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert_eq!(
            predicates,
            vec![predicate(*negated, kind, data_id)],
            "case {case:?}: the inserted predicate must be the one stored"
        );
        assert_eq!(
            rate_limit, *insert_limit,
            "case {case:?}: the accepted RateLimit must be stored, got {rate_limit}"
        );

        let (status, body, _) = apply(
            &router,
            &rule,
            *delete_limit,
            vec![update("DELETE", *negated, kind, data_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRateBasedRule must accept the delete, got {status}: {body}"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert!(
            predicates.is_empty(),
            "case {case:?}: the deleted predicate must be gone, got {predicates:?}"
        );
        assert_eq!(
            rate_limit, *delete_limit,
            "case {case:?}: a delete writes its RateLimit too, got {rate_limit}"
        );
    }
}

/// One request carries several updates and applies them in order, and a request with no
/// updates at all still writes its `RateLimit` — the operation updates the rate limit
/// whether or not it moves a predicate.
#[tokio::test]
async fn one_request_applies_several_updates_and_the_rate_limit() {
    let router = router();
    let rule = create_rate_based_rule(&router, "batch-rule", 1_000).await;
    let first = create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "batch-ip").await;
    let second =
        create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "batch-ip-two").await;
    let xss = create_condition_set(
        &router,
        "CreateXssMatchSet",
        "XssMatchSet",
        "XssMatchSetId",
        "batch-xss",
    )
    .await;

    let (status, body, _) = apply(
        &router,
        &rule,
        2_500,
        vec![
            update("INSERT", false, "IPMatch", &first),
            update("INSERT", true, "IPMatch", &second),
            update("INSERT", false, "XssMatch", &xss),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case three inserts: UpdateRateBasedRule must accept them, got {status}: {body}"
    );
    let (predicates, rate_limit) = stored(&router, &rule).await;
    assert_eq!(
        predicates,
        vec![
            predicate(false, "IPMatch", &first),
            predicate(true, "IPMatch", &second),
            predicate(false, "XssMatch", &xss),
        ],
        "case three inserts: the rule must hold every inserted predicate, in insertion order"
    );
    assert_eq!(
        rate_limit, 2_500,
        "case three inserts: the accepted RateLimit must be stored, got {rate_limit}"
    );

    // A delete and an insert in the same request, with a new rate limit.
    let (status, body, _) = apply(
        &router,
        &rule,
        RATE_LIMIT_MAX,
        vec![
            update("DELETE", true, "IPMatch", &second),
            update("INSERT", true, "XssMatch", &xss),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case a delete and an insert: UpdateRateBasedRule must accept them, got {status}: {body}"
    );
    let (predicates, rate_limit) = stored(&router, &rule).await;
    assert_eq!(
        predicates,
        vec![
            predicate(false, "IPMatch", &first),
            predicate(false, "XssMatch", &xss),
            predicate(true, "XssMatch", &xss),
        ],
        "case a delete and an insert: the remaining predicates keep their order"
    );
    assert_eq!(
        rate_limit, RATE_LIMIT_MAX,
        "case a delete and an insert: the accepted RateLimit must be stored, got {rate_limit}"
    );

    // No updates at all: the rate limit is still what this call named, and the
    // predicates are untouched.
    for limit in [RATE_LIMIT_MIN, 777, RATE_LIMIT_MAX] {
        let (status, body, _) = apply(&router, &rule, limit, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case RateLimit {limit} alone: UpdateRateBasedRule must accept it, got {status}: {body}"
        );
        let (after, rate_limit) = stored(&router, &rule).await;
        assert_eq!(
            rate_limit, limit,
            "case RateLimit {limit} alone: the accepted RateLimit must be stored, got {rate_limit}"
        );
        assert_eq!(
            after.len(),
            3,
            "case RateLimit {limit} alone: the predicates must be untouched, got {after:?}"
        );
    }
}

/// A `RateLimit` outside the modelled `@range` is refused at the boundary and writes
/// nothing: one past each published bound, and the bounds themselves are accepted.
#[tokio::test]
async fn a_rate_limit_outside_the_modelled_range_is_refused() {
    let router = router();
    let rule = create_rate_based_rule(&router, "range-rule", 1_000).await;

    for limit in [RATE_LIMIT_MIN, RATE_LIMIT_MAX] {
        let (status, body, _) = apply(&router, &rule, limit, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case RateLimit {limit}: a published bound must be accepted, got {status}: {body}"
        );
        let (_, stored_limit) = stored(&router, &rule).await;
        assert_eq!(
            stored_limit, limit,
            "case RateLimit {limit}: the bound must be stored, got {stored_limit}"
        );
    }

    for limit in [RATE_LIMIT_MIN - 1, 0, -1, RATE_LIMIT_MAX + 1] {
        let (status, body, token) = apply(&router, &rule, limit, Vec::new()).await;
        assert_eq!(
            status, 400,
            "case RateLimit {limit}: a value outside the range must be refused, got {status}: {body}"
        );
        let (_, stored_limit) = stored(&router, &rule).await;
        assert_eq!(
            stored_limit, RATE_LIMIT_MAX,
            "case RateLimit {limit}: a refused call must store nothing, got {stored_limit}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case RateLimit {limit}: a refused call must not consume the change token"
        );
    }
}

/// An inserted predicate's `DataId` must resolve to a stored condition set OF ITS OWN
/// `Type`; anything else is the modelled "referenced object doesn't exist", and the
/// refused request changes neither the predicates nor the rate limit.
#[tokio::test]
async fn insert_refuses_a_predicate_whose_condition_set_does_not_exist() {
    let router = router();
    let rule = create_rate_based_rule(&router, "resolution-rule", 1_000).await;
    let ip_set =
        create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "resolution-ip").await;
    let xss_set = create_condition_set(
        &router,
        "CreateXssMatchSet",
        "XssMatchSet",
        "XssMatchSetId",
        "resolution-xss",
    )
    .await;

    // (label, PredicateType, DataId): an id nothing was stored under, the rule's own id,
    // and an id that names a set of a DIFFERENT kind — for each direction.
    let cases: &[(&str, &str, &str)] = &[
        ("an id no Create minted", "IPMatch", UNKNOWN_ID),
        ("the rate-based rule's own id", "IPMatch", rule.as_str()),
        ("an XssMatchSet id as an IPMatch", "IPMatch", xss_set.as_str()),
        ("an IPSet id as an XssMatch", "XssMatch", ip_set.as_str()),
        (
            "an IPSet id as a SizeConstraint",
            "SizeConstraint",
            ip_set.as_str(),
        ),
        ("an IPSet id as a GeoMatch", "GeoMatch", ip_set.as_str()),
    ];

    for (label, kind, data_id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRateBasedRule",
            &json!({
                "RuleId": rule,
                "ChangeToken": token,
                "RateLimit": 4_000,
                "Updates": [update("INSERT", false, kind, data_id)],
            }),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: the modelled refusal message"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused call must not consume the change token"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert!(
            predicates.is_empty(),
            "case {label}: a refused call must persist no predicate, got {predicates:?}"
        );
        assert_eq!(
            rate_limit, 1_000,
            "case {label}: a refused call must not write the RateLimit, got {rate_limit}"
        );
    }
}

/// An update that would leave the rule as it was is "the operation failed because there
/// was nothing to do": an `INSERT` of a predicate the rule already holds and a `DELETE`
/// of one it does not — `Negated`, `Type` and `DataId` together being the predicate's
/// identity — and the `RateLimit` the refused request carried is not written either.
#[tokio::test]
async fn an_update_that_changes_no_predicate_is_refused() {
    let router = router();
    let rule = create_rate_based_rule(&router, "identity-rule", 1_000).await;
    let ip_set =
        create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "identity-ip").await;
    let xss_set = create_condition_set(
        &router,
        "CreateXssMatchSet",
        "XssMatchSet",
        "XssMatchSetId",
        "identity-xss",
    )
    .await;

    let (status, body, _) = apply(
        &router,
        &rule,
        3_000,
        vec![update("INSERT", false, "IPMatch", &ip_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    let cases: &[(&str, Value)] = &[
        (
            "inserting the predicate the rule already holds",
            update("INSERT", false, "IPMatch", &ip_set),
        ),
        (
            "deleting a predicate whose Negated differs",
            update("DELETE", true, "IPMatch", &ip_set),
        ),
        (
            "deleting a predicate whose Type differs",
            update("DELETE", false, "XssMatch", &xss_set),
        ),
        (
            "deleting a predicate whose DataId the rule does not hold",
            update("DELETE", false, "IPMatch", UNKNOWN_ID),
        ),
    ];

    for (label, candidate) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRateBasedRule",
            &json!({
                "RuleId": rule,
                "ChangeToken": token,
                "RateLimit": 9_000,
                "Updates": [candidate],
            }),
            "WAFInvalidOperationException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_INVALID_OPERATION,
            "case {label}: the modelled refusal message"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused call must not consume the change token"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert_eq!(
            predicates,
            vec![predicate(false, "IPMatch", &ip_set)],
            "case {label}: a refused call must persist nothing"
        );
        assert_eq!(
            rate_limit, 3_000,
            "case {label}: a refused call must not write the RateLimit, got {rate_limit}"
        );
    }
}

/// A request is applied whole or not at all: when a later update is refused, the earlier
/// ones in the same request are not persisted, the `RateLimit` is unchanged and the
/// change token is still unspent.
#[tokio::test]
async fn a_request_refused_part_way_through_persists_nothing() {
    let router = router();
    let rule = create_rate_based_rule(&router, "atomic-rule", 1_000).await;
    let ip_set = create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "atomic-ip").await;
    let xss_set = create_condition_set(
        &router,
        "CreateXssMatchSet",
        "XssMatchSet",
        "XssMatchSetId",
        "atomic-xss",
    )
    .await;

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
                update("INSERT", false, "XssMatch", &xss_set),
                update("DELETE", false, "IPMatch", &ip_set),
            ],
        ),
    ];

    for (label, updates) in cases {
        let (status, body, token) = apply(&router, &rule, 7_000, updates.clone()).await;
        assert_eq!(
            status, 400,
            "case {label}: the request must be refused, got {status}: {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused call must not consume the change token"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert!(
            predicates.is_empty(),
            "case {label}: no earlier update may be persisted, got {predicates:?}"
        );
        assert_eq!(
            rate_limit, 1_000,
            "case {label}: a refused call must not write the RateLimit, got {rate_limit}"
        );
    }
}

/// `UpdateRateBasedRule` for a rule this service holds no RATE-BASED rule for is the
/// modelled nonexistent-container refusal — including the id of a plain `Rule`, which is
/// a resource of its own kind.
#[tokio::test]
async fn an_unknown_rule_id_is_a_modelled_nonexistent_container() {
    let router = router();
    let ip_set =
        create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "container-ip").await;
    let rate_based = create_rate_based_rule(&router, "container-rate-rule", 1_000).await;

    // A plain `Rule`: `ListRules` reports these and `ListRateBasedRules` the rate-based
    // ones, so a `Rule` id names no rate-based rule.
    let token = change_token(&router).await;
    let plain = ok_call(
        &router,
        "CreateRule",
        &json!({
            "Name": "container-plain-rule",
            "MetricName": "containerPlainRule",
            "ChangeToken": token,
        }),
    )
    .await;
    let plain_rule = plain["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRule returned no RuleId: {plain}"))
        .to_string();

    let cases: &[(&str, &str)] = &[
        ("an id no Create minted", UNKNOWN_ID),
        ("a condition set's id", ip_set.as_str()),
        ("a plain Rule's id", plain_rule.as_str()),
    ];

    for (label, rule_id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRateBasedRule",
            &json!({
                "RuleId": rule_id,
                "ChangeToken": token,
                "RateLimit": 5_000,
                "Updates": [update("INSERT", false, "IPMatch", &ip_set)],
            }),
            "WAFNonexistentContainerException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_CONTAINER,
            "case {label}: the modelled refusal message"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused call must not consume the change token"
        );
    }

    // The converse: `UpdateRule` does not reach a rate-based rule either, so neither
    // call writes the other kind's store.
    let token = change_token(&router).await;
    expect_error(
        &router,
        "UpdateRule",
        &json!({
            "RuleId": rate_based,
            "ChangeToken": token,
            "Updates": [update("INSERT", false, "IPMatch", &ip_set)],
        }),
        "WAFNonexistentContainerException",
        "a rate-based rule's id given to UpdateRule",
    )
    .await;
    let (predicates, rate_limit) = stored(&router, &rate_based).await;
    assert!(
        predicates.is_empty(),
        "case a rate-based rule's id given to UpdateRule: nothing may be stored, got {predicates:?}"
    );
    assert_eq!(
        rate_limit, 1_000,
        "case a rate-based rule's id given to UpdateRule: the RateLimit is unchanged"
    );
}

/// "Each create, update, or delete request must use a unique change token": a token
/// already spent and one no `GetChangeToken` minted are both stale data, and such a
/// request changes neither the predicates nor the rate limit.
#[tokio::test]
async fn update_refuses_a_stale_or_unknown_change_token() {
    let router = router();
    let rule = create_rate_based_rule(&router, "token-rule", 1_000).await;
    let ip_set = create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "token-ip").await;
    let other = create_condition_set(
        &router,
        "CreateIPSet",
        "IPSet",
        "IPSetId",
        "token-ip-two",
    )
    .await;

    let (status, body, spent) = apply(
        &router,
        &rule,
        6_000,
        vec![update("INSERT", false, "IPMatch", &ip_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    for (label, token) in [
        ("a token this operation already spent", spent.as_str()),
        ("a token no GetChangeToken minted", UNKNOWN_ID),
    ] {
        let body = expect_error(
            &router,
            "UpdateRateBasedRule",
            &json!({
                "RuleId": rule,
                "ChangeToken": token,
                "RateLimit": 8_000,
                "Updates": [update("INSERT", false, "IPMatch", &other)],
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_STALE_DATA,
            "case {label}: the modelled refusal message"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert_eq!(
            predicates,
            vec![predicate(false, "IPMatch", &ip_set)],
            "case {label}: a refused call must persist nothing"
        );
        assert_eq!(
            rate_limit, 6_000,
            "case {label}: a refused call must not write the RateLimit, got {rate_limit}"
        );
    }
}

/// The predicate state one caller can grow through this operation is bounded: inserts
/// are accepted up to the per-rule predicate quota and refused past it, and a request
/// carrying more updates than one request may carry is refused before any of them is
/// applied.
#[tokio::test]
async fn update_refuses_past_the_predicate_quota() {
    let router = router();
    let rule = create_rate_based_rule(&router, "quota-rule", 1_000).await;

    // One IPSet per predicate the rule may hold, plus one more to insert past the quota.
    let mut sets = Vec::new();
    for n in 0..=MAX_RULE_PREDICATES {
        sets.push(
            create_condition_set(
                &router,
                "CreateIPSet",
                "IPSet",
                "IPSetId",
                &format!("quota-ip-{n}"),
            )
            .await,
        );
    }

    let fill: Vec<Value> = sets[..MAX_RULE_PREDICATES]
        .iter()
        .map(|set| update("INSERT", false, "IPMatch", set))
        .collect();
    let (status, body, _) = apply(&router, &rule, 1_500, fill).await;
    assert_eq!(
        status, 200,
        "case filling the rule to the quota: must be accepted, got {status}: {body}"
    );
    let (predicates, _) = stored(&router, &rule).await;
    assert_eq!(
        predicates.len(),
        MAX_RULE_PREDICATES,
        "case filling the rule to the quota: every insert must be stored"
    );

    let past_quota: Vec<Value> = vec![update(
        "INSERT",
        false,
        "IPMatch",
        &sets[MAX_RULE_PREDICATES],
    )];
    let too_many: Vec<Value> = (0..=MAX_RULE_UPDATES_PER_REQUEST)
        .map(|n| update("INSERT", false, "IPMatch", &sets[n % sets.len()]))
        .collect();
    for (label, updates) in [
        ("one predicate past the per-rule quota", past_quota),
        ("more updates than one request may carry", too_many),
    ] {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRateBasedRule",
            &json!({
                "RuleId": rule,
                "ChangeToken": token,
                "RateLimit": 9_500,
                "Updates": updates,
            }),
            "WAFLimitsExceededException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_LIMITS_EXCEEDED,
            "case {label}: the modelled refusal message"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused call must not consume the change token"
        );
        let (predicates, rate_limit) = stored(&router, &rule).await;
        assert_eq!(
            predicates.len(),
            MAX_RULE_PREDICATES,
            "case {label}: exactly the accepted predicates are stored"
        );
        assert_eq!(
            rate_limit, 1_500,
            "case {label}: a refused call must not write the RateLimit, got {rate_limit}"
        );
    }
}

/// While a rate-based rule's predicate points at a condition set, deleting that set is
/// refused as a referenced item, and the delete succeeds once `UpdateRateBasedRule` has
/// removed the predicate.
#[tokio::test]
async fn a_condition_set_a_rate_based_rule_references_cannot_be_deleted() {
    let router = router();
    let rule = create_rate_based_rule(&router, "reference-rule", 1_000).await;
    let ip_set =
        create_condition_set(&router, "CreateIPSet", "IPSet", "IPSetId", "reference-ip").await;

    let (status, body, _) = apply(
        &router,
        &rule,
        1_200,
        vec![update("INSERT", false, "IPMatch", &ip_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    let token = change_token(&router).await;
    expect_error(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": &ip_set, "ChangeToken": token}),
        "WAFReferencedItemException",
        "the IPSet a rate-based rule's predicate points at",
    )
    .await;

    let (status, body, _) = apply(
        &router,
        &rule,
        1_200,
        vec![update("DELETE", false, "IPMatch", &ip_set)],
    )
    .await;
    assert_eq!(
        status, 200,
        "removing the predicate must be accepted, got {status}: {body}"
    );

    let token = change_token(&router).await;
    let (status, body) = call(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": &ip_set, "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the IPSet once the predicate is gone: the delete must be accepted, got {status}: \
         {body}"
    );
}
