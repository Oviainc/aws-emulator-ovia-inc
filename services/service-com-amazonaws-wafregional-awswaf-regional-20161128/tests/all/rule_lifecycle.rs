// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `Rule` lifecycle, end to end over the wire:
//! `GetChangeToken`, `CreateRule`, `GetRule`.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.
//!
//! The rule under test for `MetricName` is the member's own documentation on
//! `CreateRule`: "The name can contain only alphanumeric characters (A-Z, a-z, 0-9),
//! with maximum length 128 and minimum length one. It can't contain whitespace or
//! metric names reserved for AWS WAF, including "All" and "Default_Action.""

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    ComAmazonawsWafregionalAwswafRegional20161128State, MAX_RULES, MAX_TAGS_PER_RESOURCE,
    MESSAGE_DISALLOWED_NAME, MESSAGE_LIMITS_EXCEEDED, MESSAGE_NONEXISTENT_ITEM, MESSAGE_STALE_DATA,
};

use crate::wire::{
    call, change_token, error_message, expect_error, is_resource_id, ok_call, router, router_on,
    token_status,
};

/// An id this service never minted, used wherever a test needs one it holds nothing
/// for.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Create a rule and return the parsed `CreateRule` response.
async fn create_rule(router: &MultiServiceRouter, name: &str, metric_name: &str) -> Value {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRule",
        &json!({"Name": name, "MetricName": metric_name, "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        body["ChangeToken"].as_str(),
        Some(token.as_str()),
        "CreateRule must report the change token it consumed: {body}"
    );
    body
}

/// The `RuleId` of a `CreateRule` response.
fn rule_id(body: &Value) -> String {
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRule returned no RuleId: {body}"))
        .to_string()
}

/// `CreateRule` stores a new, predicate-free `Rule` under a minted id, and `GetRule`
/// reads that record back — across a table of names and metric names differing in
/// length, case and the characters they use. `ResourceName` carries no uniqueness
/// constraint, so the last two rows share a `Name` and must still be two distinct
/// resources.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_rule() {
    let router = router();
    let max_length = "m".repeat(128);
    let cases: &[(&str, &str, &str)] = &[
        // (label, Name, MetricName) — `ResourceName` and `MetricName` are both
        // len 1..128, pattern \S; a metric name is alphanumeric.
        ("a plain name", "bootstrap-rule", "bootstrapRule"),
        ("one character (the minimum length)", "x", "x"),
        ("128 characters (the maximum length)", &max_length, &max_length),
        ("digits only", "0123456789", "0123456789"),
        ("mixed case and digits", "Rule_42.v2", "Rule42Metric"),
        ("whitespace inside the resource name", "two words", "twoWords"),
        ("a repeated name", "shared-name", "sharedNameOne"),
        ("the same name again", "shared-name", "sharedNameTwo"),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (label, name, metric_name) in cases {
        let created = create_rule(&router, name, metric_name).await;
        let id = rule_id(&created);
        assert!(
            is_resource_id(&id),
            "case {label}: CreateRule must mint a RuleId, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {label}: every CreateRule call must mint a distinct RuleId, {id:?} repeats"
        );
        assert_eq!(
            created["Rule"]["Name"].as_str(),
            Some(*name),
            "case {label}: the created Rule must carry the stored Name, got {}",
            created["Rule"]
        );
        assert_eq!(
            created["Rule"]["MetricName"].as_str(),
            Some(*metric_name),
            "case {label}: the created Rule must carry the stored MetricName, got {}",
            created["Rule"]
        );
        assert_eq!(
            created["Rule"]["Predicates"].as_array().map(Vec::len),
            Some(0),
            "case {label}: a new Rule holds no predicates, got {}",
            created["Rule"]
        );

        // The round trip: GetRule reports the stored record, not the request.
        let fetched = ok_call(&router, "GetRule", &json!({"RuleId": id})).await;
        assert_eq!(
            fetched["Rule"], created["Rule"],
            "case {label}: GetRule must read back the record CreateRule stored, got {} after {}",
            fetched["Rule"], created["Rule"]
        );
    }
}

/// A `MetricName` outside the documented character set, or one AWS WAF reserves, is
/// refused with the modelled `WAFDisallowedNameException` — and the refused request
/// changes nothing: no rule is stored and the change token it carried is still usable.
#[tokio::test]
async fn create_refuses_metric_names_outside_the_documented_rule() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());

    let cases: &[(&str, &str)] = &[
        // (label, MetricName) — none of these is "only alphanumeric characters
        // (A-Z, a-z, 0-9)" that is not a reserved AWS WAF metric name.
        ("whitespace inside the name", "metric name"),
        ("a trailing space", "metricName "),
        ("a tab", "metric\tname"),
        ("a hyphen", "metric-name"),
        ("an underscore", "metric_name"),
        ("a dot", "metric.name"),
        ("a non-ASCII letter", "métricName"),
        // The reserved names the member's documentation lists by name.
        ("the reserved name All", "All"),
        ("the reserved name Default_Action", "Default_Action"),
    ];

    for (label, metric_name) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "CreateRule",
            &json!({"Name": "disallowed-probe", "MetricName": metric_name, "ChangeToken": token}),
            "WAFDisallowedNameException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_DISALLOWED_NAME,
            "case {label}: the refusal carries WAF Classic's fixed disallowed-name \
             sentence, got {:?}",
            error_message(&body)
        );
        // Nothing changed, so the token is untouched and a later call may spend it.
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused CreateRule must not consume the change token"
        );
        assert!(
            engine
                .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
                .lock()
                .rules
                .is_empty(),
            "case {label}: a refused CreateRule must persist no Rule"
        );
    }

    // And the token the last refusal left alone still creates a rule.
    let token = change_token(&router).await;
    let created = ok_call(
        &router,
        "CreateRule",
        &json!({"Name": "after-refusal", "MetricName": "afterRefusal", "ChangeToken": token}),
    )
    .await;
    assert!(
        is_resource_id(&rule_id(&created)),
        "a token no refused call consumed must still create a rule: {created}"
    );
}

/// Every metric name the documented rule admits is accepted: alphanumeric, any case,
/// any length the model allows, including names that merely START with a reserved one.
#[tokio::test]
async fn create_accepts_every_metric_name_the_rule_admits() {
    let router = router();
    let max_length = "M".repeat(128);
    let cases: &[(&str, &str)] = &[
        ("one lower-case letter", "a"),
        ("one upper-case letter", "Z"),
        ("one digit", "0"),
        ("letters and digits", "Rule42Metric"),
        ("128 characters (the maximum length)", &max_length),
        // Only the reserved names themselves are reserved, not every name
        // containing one.
        ("a name beginning with the reserved All", "AllRequests"),
        ("the reserved name without its underscore", "DefaultAction"),
        ("the reserved name in another case", "all"),
    ];

    for (label, metric_name) in cases {
        let created = create_rule(&router, "accepted-metric-name", metric_name).await;
        assert_eq!(
            created["Rule"]["MetricName"].as_str(),
            Some(*metric_name),
            "case {label}: CreateRule must accept this metric name and store it, got {}",
            created["Rule"]
        );
    }
}

/// A change token no `GetChangeToken` minted, and one a mutating call already spent,
/// are both stale data: "each create, update, or delete request must use a unique
/// change token".
#[tokio::test]
async fn create_refuses_a_stale_or_unknown_change_token() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());

    let spent = change_token(&router).await;
    ok_call(
        &router,
        "CreateRule",
        &json!({"Name": "token-probe", "MetricName": "tokenProbe", "ChangeToken": spent}),
    )
    .await;

    for (label, token) in [
        ("already consumed", spent.clone()),
        ("never minted", UNKNOWN_ID.to_string()),
    ] {
        let body = expect_error(
            &router,
            "CreateRule",
            &json!({"Name": "stale-token-probe", "MetricName": "staleProbe", "ChangeToken": token}),
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
    }

    assert_eq!(
        engine
            .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
            .lock()
            .rules
            .len(),
        1,
        "only the call with a fresh token may store a Rule"
    );
}

/// The tag set one rule may carry is bounded: a request at the bound is stored whole,
/// and one past it is refused with the modelled `WAFLimitsExceededException`, leaving
/// the change token unspent and nothing persisted.
#[tokio::test]
async fn create_refuses_more_tags_than_a_resource_may_carry() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());

    /// `Tags` carrying `count` distinct keys.
    fn tags(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"Key": format!("key{index}"), "Value": format!("value{index}")}))
                .collect(),
        )
    }

    // (label, tag count, whether the request is accepted) — the bound, one below it,
    // and one past it.
    let cases: &[(&str, usize, bool)] = &[
        ("one tag", 1, true),
        ("one below the bound", MAX_TAGS_PER_RESOURCE - 1, true),
        ("exactly the bound", MAX_TAGS_PER_RESOURCE, true),
        ("one past the bound", MAX_TAGS_PER_RESOURCE + 1, false),
        ("far past the bound", MAX_TAGS_PER_RESOURCE * 4, false),
    ];

    for (label, count, accepted) in cases {
        let token = change_token(&router).await;
        let request = json!({
            "Name": "tag-bound-probe",
            "MetricName": "tagBoundProbe",
            "ChangeToken": token,
            "Tags": tags(*count),
        });
        let rules_before = engine
            .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
            .lock()
            .rules
            .len();

        if *accepted {
            let created = ok_call(&router, "CreateRule", &request).await;
            let id = rule_id(&created);
            let stored = engine
                .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
                .lock()
                .rules
                .get(&id)
                .map(|entry| entry.tags.len());
            assert_eq!(
                stored,
                Some(*count),
                "case {label}: a request at or below the bound stores every tag, got {stored:?}"
            );
        } else {
            let body = expect_error(
                &router,
                "CreateRule",
                &request,
                "WAFLimitsExceededException",
                label,
            )
            .await;
            assert_eq!(
                error_message(&body),
                MESSAGE_LIMITS_EXCEEDED,
                "case {label}: the refusal carries WAF Classic's fixed limit sentence, \
                 got {:?}",
                error_message(&body)
            );
            assert_eq!(
                token_status(&router, &token).await,
                "PROVISIONED",
                "case {label}: a refused CreateRule must not consume the change token"
            );
            assert_eq!(
                engine
                    .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
                    .lock()
                    .rules
                    .len(),
                rules_before,
                "case {label}: a refused CreateRule must persist no Rule"
            );
        }
    }
}

/// `GetRule` for an id this service holds nothing for is the modelled not-found — for a
/// never-created id and for one whose rule was never created under this engine alike.
#[tokio::test]
async fn an_unknown_rule_id_is_a_modelled_not_found() {
    let router = router();
    let created = create_rule(&router, "present-rule", "presentRule").await;
    let present = rule_id(&created);

    let cases: &[(&str, &str)] = &[
        ("a UUID-shaped id that was never minted", UNKNOWN_ID),
        ("the zero id", "00000000-0000-0000-0000-000000000000"),
        ("a one-character id", "x"),
    ];
    for (label, id) in cases {
        let body = expect_error(
            &router,
            "GetRule",
            &json!({"RuleId": id}),
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
    }

    // The id that does exist still reads back: the refusals above changed nothing.
    let fetched = ok_call(&router, "GetRule", &json!({"RuleId": present})).await;
    assert_eq!(
        fetched["Rule"], created["Rule"],
        "the stored rule must survive every not-found refusal, got {}",
        fetched["Rule"]
    );
}

/// Every `Rule` is a stored record, so `CreateRule` refuses past the account's quota
/// with the modelled `WAFLimitsExceededException`, and that refusal leaves the change
/// token unspent.
#[tokio::test]
async fn create_refuses_past_the_rule_quota() {
    let router = router();
    for index in 0..MAX_RULES {
        let created = create_rule(&router, &format!("rule-{index}"), &format!("rule{index}")).await;
        assert!(
            is_resource_id(&rule_id(&created)),
            "case rule {index}: every rule up to the quota must be created"
        );
    }

    let token = change_token(&router).await;
    let body = expect_error(
        &router,
        "CreateRule",
        &json!({"Name": "over-quota", "MetricName": "overQuota", "ChangeToken": token}),
        "WAFLimitsExceededException",
        "one rule past the quota",
    )
    .await;
    assert_eq!(
        error_message(&body),
        MESSAGE_LIMITS_EXCEEDED,
        "case one rule past the quota: the refusal carries WAF Classic's fixed limit \
         sentence, got {:?}",
        error_message(&body)
    );
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "a refused CreateRule must not consume the change token"
    );
}

/// The `Tags` a `CreateRule` request carries are persisted on the rule it creates. A
/// tag set is keyed by `Key`, so a key repeated in one request keeps the last value
/// given for it.
#[tokio::test]
async fn tags_given_at_create_are_persisted_on_the_rule() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());

    /// One row: a label, the `Tags` member as sent, and the tag set the created rule
    /// must carry.
    type TagCase<'a> = (&'a str, Value, &'a [(&'a str, &'a str)]);

    let cases: &[TagCase<'_>] = &[
        ("no Tags member at all", Value::Null, &[]),
        (
            "one tag",
            json!([{"Key": "env", "Value": "prod"}]),
            &[("env", "prod")],
        ),
        (
            "several tags, out of order",
            json!([{"Key": "team", "Value": "waf"}, {"Key": "env", "Value": "dev"}]),
            &[("env", "dev"), ("team", "waf")],
        ),
        (
            "an empty tag value",
            json!([{"Key": "owner", "Value": ""}]),
            &[("owner", "")],
        ),
        (
            "a key repeated in one request",
            json!([{"Key": "env", "Value": "first"}, {"Key": "env", "Value": "last"}]),
            &[("env", "last")],
        ),
    ];

    for (label, tags, expected) in cases {
        let token = change_token(&router).await;
        let mut request = json!({
            "Name": "tagged-rule",
            "MetricName": "taggedRule",
            "ChangeToken": token,
        });
        if !tags.is_null() {
            request["Tags"] = tags.clone();
        }
        let (status, created) = call(&router, "CreateRule", &request).await;
        assert_eq!(
            status, 200,
            "case {label}: CreateRule must accept the tag set, got {status}: {created}"
        );

        let id = rule_id(&created);
        let expected: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let stored = engine
            .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
            .lock()
            .rules
            .get(&id)
            .map(|entry| entry.tags.clone());
        assert_eq!(
            stored.as_ref(),
            Some(&expected),
            "case {label}: the created rule must carry the request's tag set, got {stored:?}"
        );
    }
}
