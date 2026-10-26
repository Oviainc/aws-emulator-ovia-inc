// SPDX-License-Identifier: CC0-1.0

//! `DeleteRuleGroup`, end to end over the wire.
//!
//! `DeleteRuleGroup` "deletes the specified RuleGroup", and the rule asserted here
//! is: the call resolves the rule group named by the `(Scope, Name, Id)` triple and,
//! when that triple resolves, the request's `LockToken` still equals the stored one
//! and no stored rule tree references the group, REMOVES it from persisted state —
//! so every later read, listing and reference resolves against a store that no
//! longer holds it and its `(Scope, Name)` is free to be taken again — while every
//! other request is refused with HTTP 400 and leaves the store exactly as it was: a
//! triple that resolves to nothing is `WAFNonexistentItemException` and a token that
//! is not the stored one is `WAFOptimisticLockException`.
//!
//! Each suite drives that over a TABLE spanning the class rather than over one
//! request: the rows differ in `Scope`, in declared `Capacity`, in rule count and
//! statement shape, and — for the refusals — in WHICH part of the request fails to
//! resolve. The harness is the one in [`crate::web_acl_lifecycle`]: every call goes
//! through the router's front door, so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    NONEXISTENT_ITEM_MESSAGE, OPTIMISTIC_LOCK_MESSAGE,
};

use crate::rule_group_lifecycle::{byte_match_statement, create_rule_group, rule_group_arn};
use crate::web_acl_lifecycle::{
    create_web_acl, is_model_uuid, rule, sqli_statement, visibility, xss_statement, Waf,
};

/// The scope a row's namesake twin is created in, so a delete in one scope can be
/// seen to leave the other scope's rule group of the SAME name alone.
fn other_scope(scope: &str) -> &'static str {
    if scope == "CLOUDFRONT" {
        "REGIONAL"
    } else {
        "CLOUDFRONT"
    }
}

/// A rule at an explicit `priority`, so a row's rules carry distinct ones.
fn rule_at(name: &str, priority: i64, statement: Value) -> Value {
    let mut built = rule(name, statement);
    built["Priority"] = json!(priority);
    built
}

/// A `(Scope, Name, Id, LockToken)` `DeleteRuleGroup` body.
fn delete_body(scope: &str, name: &str, id: &str, lock_token: &str) -> Value {
    json!({ "Scope": scope, "Name": name, "Id": id, "LockToken": lock_token })
}

/// Every rule group name `ListRuleGroups` reports for `scope`, walked to exhaustion
/// through `NextMarker` so no page size can hide one.
async fn listed_names(waf: &Waf, scope: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "Scope": scope });
        if let Some(marker) = marker.as_deref() {
            request["NextMarker"] = json!(marker);
        }
        let page = waf.ok("ListRuleGroups", request).await;
        let summaries = page["RuleGroups"]
            .as_array()
            .unwrap_or_else(|| panic!("scope {scope}: RuleGroups must be a list, got {page}"))
            .clone();
        names.extend(
            summaries
                .iter()
                .filter_map(|summary| summary["Name"].as_str().map(str::to_string)),
        );
        marker = page["NextMarker"].as_str().map(str::to_string);
        if marker.is_none() {
            break;
        }
    }
    names
}

/// The lock token `GetRuleGroup` currently reports for a triple.
async fn current_lock_token(waf: &Waf, scope: &str, name: &str, id: &str) -> String {
    let read = waf
        .ok("GetRuleGroup", json!({ "Scope": scope, "Name": name, "Id": id }))
        .await;
    read["LockToken"]
        .as_str()
        .unwrap_or_else(|| panic!("GetRuleGroup must report a LockToken, got {read}"))
        .to_string()
}

/// The awsJson error body's message member, whichever casing the shape models.
fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One row of the deletion table.
struct DeletionCase {
    case: &'static str,
    scope: &'static str,
    name: &'static str,
    capacity: i64,
    rules: Value,
}

/// The rows: both `Scope` values, capacities from the smallest upwards, and rule
/// lists from empty through several rules of differing statement shape.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            case: "no rules, the minimum capacity, REGIONAL",
            scope: "REGIONAL",
            name: "empty-group",
            capacity: 1,
            rules: json!([]),
        },
        DeletionCase {
            case: "one SQLi rule, REGIONAL",
            scope: "REGIONAL",
            name: "one-rule-group",
            capacity: 20,
            rules: json!([rule_at("sqli", 0, sqli_statement())]),
        },
        DeletionCase {
            case: "three rules of three statement shapes, REGIONAL",
            scope: "REGIONAL",
            name: "three-rule-group",
            capacity: 500,
            rules: json!([
                rule_at("bytes", 0, byte_match_statement()),
                rule_at("sqli", 1, sqli_statement()),
                rule_at("xss", 2, xss_statement()),
            ]),
        },
        DeletionCase {
            case: "no rules, CLOUDFRONT",
            scope: "CLOUDFRONT",
            name: "empty-global-group",
            capacity: 1,
            rules: json!([]),
        },
        DeletionCase {
            case: "a nested logical statement, CLOUDFRONT",
            scope: "CLOUDFRONT",
            name: "nested-global-group",
            capacity: 1500,
            rules: json!([rule_at(
                "nested",
                7,
                json!({
                    "AndStatement": {
                        "Statements": [
                            byte_match_statement(),
                            { "NotStatement": { "Statement": sqli_statement() } },
                        ]
                    }
                }),
            )]),
        },
    ]
}

/// A rule group whose lock token the caller holds, and which nothing references, is
/// deleted: it disappears from every read and listing, its ARN resolves to nothing,
/// a second delete of the same triple finds nothing, and its name is free again.
#[tokio::test]
async fn deleting_a_rule_group_removes_it_from_every_read() {
    for DeletionCase {
        case,
        scope,
        name,
        capacity,
        rules,
    } in deletion_cases()
    {
        let waf = Waf::new();
        let (id, arn, lock_token) =
            create_rule_group(&waf, scope, name, capacity, rules.clone()).await;
        assert_eq!(
            arn,
            rule_group_arn(scope, name, &id),
            "case {case:?}: the created ARN must name the group, got {arn:?}"
        );
        // A second group in the same scope, and a NAMESAKE in the other scope: the
        // delete must take exactly the one its triple names.
        let (sibling_id, _sibling_arn, _sibling_lock) =
            create_rule_group(&waf, scope, "untouched-sibling", 50, json!([])).await;
        let twin_scope = other_scope(scope);
        let (twin_id, _twin_arn, _twin_lock) =
            create_rule_group(&waf, twin_scope, name, capacity, rules).await;
        let sibling_lock_before =
            current_lock_token(&waf, scope, "untouched-sibling", &sibling_id).await;

        let deleted = waf
            .ok("DeleteRuleGroup", delete_body(scope, name, &id, &lock_token))
            .await;
        // `DeleteRuleGroupOutput` models no members, so the success body carries
        // none.
        let members = deleted.as_object().map(|body| body.len()).unwrap_or(0);
        assert_eq!(
            members, 0,
            "case {case:?}: DeleteRuleGroup answers an empty body, got {deleted}"
        );

        // Gone from both addressing forms `GetRuleGroup` accepts.
        waf.refused(
            "GetRuleGroup",
            json!({ "Scope": scope, "Name": name, "Id": &id }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
        waf.refused(
            "GetRuleGroup",
            json!({ "ARN": &arn }),
            "WAFNonexistentItemException",
            case,
        )
        .await;

        // Gone from the scope's listing, while the sibling is still there with the
        // lock token it had: a delete rotates nothing else.
        let listed = listed_names(&waf, scope).await;
        assert!(
            !listed.iter().any(|listed| listed == name),
            "case {case:?}: {name:?} must be gone from ListRuleGroups, got {listed:?}"
        );
        assert!(
            listed.iter().any(|listed| listed == "untouched-sibling"),
            "case {case:?}: the sibling must still be listed, got {listed:?}"
        );
        assert_eq!(
            current_lock_token(&waf, scope, "untouched-sibling", &sibling_id).await,
            sibling_lock_before,
            "case {case:?}: the sibling's lock token must not rotate"
        );
        // The other scope is a namespace of its own, so its namesake survives.
        let twin_listed = listed_names(&waf, twin_scope).await;
        assert!(
            twin_listed.iter().any(|listed| listed == name),
            "case {case:?}: the {twin_scope} namesake must survive, got {twin_listed:?}"
        );
        waf.ok(
            "GetRuleGroup",
            json!({ "Scope": twin_scope, "Name": name, "Id": &twin_id }),
        )
        .await;

        // A rule that would RUN the dead group can no longer be saved: the ARN
        // resolves to nothing.
        let runner = json!([{
            "Name": "run-the-dead-group",
            "Priority": 0,
            "Statement": { "RuleGroupReferenceStatement": { "ARN": &arn } },
            "OverrideAction": { "None": {} },
            "VisibilityConfig": visibility("run-the-dead-group"),
        }]);
        waf.refused(
            "CreateWebACL",
            json!({
                "Name": "late-acl",
                "Scope": scope,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("late-acl"),
                "Rules": runner,
            }),
            "WAFNonexistentItemException",
            case,
        )
        .await;

        // Deleting it again finds nothing: the delete is refused, not silently
        // accepted a second time.
        waf.refused(
            "DeleteRuleGroup",
            delete_body(scope, name, &id, &lock_token),
            "WAFNonexistentItemException",
            case,
        )
        .await;

        // The `(Scope, Name)` the group held is free again, and what takes it is a
        // NEW resource: a fresh id and a fresh lock token.
        let (new_id, new_arn, new_lock) =
            create_rule_group(&waf, scope, name, capacity, json!([])).await;
        assert!(
            is_model_uuid(&new_id) && new_id != id,
            "case {case:?}: the replacement needs a new model-shaped id, got {new_id:?}"
        );
        assert!(
            is_model_uuid(&new_lock) && new_lock != lock_token,
            "case {case:?}: the replacement needs a new lock token, got {new_lock:?}"
        );
        assert_eq!(
            new_arn,
            rule_group_arn(scope, name, &new_id),
            "case {case:?}: the replacement's ARN must name its own id"
        );
        waf.ok(
            "GetRuleGroup",
            json!({ "Scope": scope, "Name": name, "Id": &new_id }),
        )
        .await;
    }
}

/// How a row's `DeleteRuleGroup` request fails to earn the delete.
struct RefusalCase {
    case: &'static str,
    /// The error code the refusal must carry.
    code: &'static str,
    /// The sentence that error code always carries.
    message: &'static str,
    /// Rewrite the well-formed body into the one this row sends.
    spoil: fn(&mut Value),
}

/// An id of the modelled form (`EntityId`) that nothing was ever stored under.
const UNSTORED_ID: &str = "11111111-2222-4333-8444-555555555555";

/// A lock token of the modelled form (`LockToken`) that no stored group holds.
const UNHELD_LOCK_TOKEN: &str = "99999999-8888-4777-8666-555555555555";

/// The rows: every part of the triple that can fail to resolve, plus a token that is
/// not the stored one.
fn refusal_cases() -> &'static [RefusalCase] {
    &[
        RefusalCase {
            case: "an Id nothing is stored under",
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM_MESSAGE,
            spoil: |body| body["Id"] = json!(UNSTORED_ID),
        },
        RefusalCase {
            case: "a Name that is not the stored group's",
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM_MESSAGE,
            spoil: |body| body["Name"] = json!("not-the-stored-name"),
        },
        RefusalCase {
            case: "the other Scope, which holds no such group",
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM_MESSAGE,
            spoil: |body| {
                let scope = body["Scope"].as_str().unwrap_or_default().to_string();
                body["Scope"] = json!(other_scope(&scope));
            },
        },
        RefusalCase {
            case: "a LockToken no stored group holds",
            code: "WAFOptimisticLockException",
            message: OPTIMISTIC_LOCK_MESSAGE,
            spoil: |body| body["LockToken"] = json!(UNHELD_LOCK_TOKEN),
        },
    ]
}

/// A delete whose triple does not resolve, or whose token is not the stored one, is
/// refused and persists nothing: the group is still there, with the token it had,
/// and the delete that does hold the lock still succeeds afterwards.
#[tokio::test]
async fn a_delete_that_misses_the_triple_or_the_lock_changes_nothing() {
    for DeletionCase {
        case,
        scope,
        name,
        capacity,
        rules,
    } in deletion_cases()
    {
        let waf = Waf::new();
        let (id, _arn, lock_token) = create_rule_group(&waf, scope, name, capacity, rules).await;
        let well_formed = delete_body(scope, name, &id, &lock_token);

        for refusal in refusal_cases() {
            let row = format!("{case} / {}", refusal.case);
            let mut body = well_formed.clone();
            (refusal.spoil)(&mut body);
            let refused = waf
                .refused("DeleteRuleGroup", body, refusal.code, &row)
                .await;
            let actual = error_message(&refused);
            assert_eq!(
                actual, refusal.message,
                "case {row:?}: expected the {} sentence, got {actual:?}",
                refusal.code
            );
            // The refusal wrote nothing: the group still reads back under its own
            // triple, with the lock token it was created with.
            assert_eq!(
                current_lock_token(&waf, scope, name, &id).await,
                lock_token,
                "case {row:?}: a refused delete must not rotate the lock token"
            );
        }

        // Nothing the refusals did stops the delete that does hold the lock.
        waf.ok("DeleteRuleGroup", well_formed).await;
        waf.refused(
            "GetRuleGroup",
            json!({ "Scope": scope, "Name": name, "Id": &id }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }
}

/// A rule group a stored rule tree still RUNS is refused for the association, not
/// for the lock, and survives the refusal — while the same group deletes once the
/// holder is gone.
///
/// The row that holds it is a WEB ACL here; `crate::referenced_resource_deletes`
/// crosses the holder kinds. What this adds is that the association check happens
/// AFTER the lock check, so a request that misses both is refused for the lock.
#[tokio::test]
async fn an_association_refusal_does_not_hide_a_stale_lock() {
    let waf = Waf::new();
    let (group_id, group_arn, group_lock) = create_rule_group(
        &waf,
        "REGIONAL",
        "run-by-an-acl",
        200,
        json!([rule_at("sqli", 0, sqli_statement())]),
    )
    .await;
    let runner = json!([{
        "Name": "run-the-group",
        "Priority": 0,
        "Statement": { "RuleGroupReferenceStatement": { "ARN": &group_arn } },
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility("run-the-group"),
    }]);
    let (acl_id, _acl_arn, acl_lock) =
        create_web_acl(&waf, "REGIONAL", "holding-acl", runner).await;

    // Holding the lock but referenced: the association refuses it.
    waf.refused(
        "DeleteRuleGroup",
        delete_body("REGIONAL", "run-by-an-acl", &group_id, &group_lock),
        "WAFAssociatedItemException",
        "referenced, with the stored lock token",
    )
    .await;
    // Referenced AND holding a stale token: the lock is checked first, so this is a
    // lock refusal.
    waf.refused(
        "DeleteRuleGroup",
        delete_body("REGIONAL", "run-by-an-acl", &group_id, UNHELD_LOCK_TOKEN),
        "WAFOptimisticLockException",
        "referenced, with a token no stored group holds",
    )
    .await;
    assert_eq!(
        current_lock_token(&waf, "REGIONAL", "run-by-an-acl", &group_id).await,
        group_lock,
        "a refused delete must leave the group's lock token where it was"
    );

    // Take the holder away and the same request succeeds.
    waf.ok(
        "DeleteWebACL",
        json!({
            "Scope": "REGIONAL",
            "Name": "holding-acl",
            "Id": &acl_id,
            "LockToken": &acl_lock,
        }),
    )
    .await;
    waf.ok(
        "DeleteRuleGroup",
        delete_body("REGIONAL", "run-by-an-acl", &group_id, &group_lock),
    )
    .await;
    waf.refused(
        "GetRuleGroup",
        json!({ "ARN": &group_arn }),
        "WAFNonexistentItemException",
        "the group after its holder and it were deleted",
    )
    .await;
}
