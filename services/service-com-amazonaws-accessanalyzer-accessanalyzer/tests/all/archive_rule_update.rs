// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateArchiveRule`, driven over the wire.
//!
//! The rule under test: an update REPLACES the stored filter of an existing archive
//! rule with the whole `filter` map the request carries, leaves the rule's `createdAt`
//! alone while moving its `updatedAt` forward, creates nothing, and answers the
//! modelled 404 when the analyzer or the rule it names does not exist.
//!
//! The tables span what that rule generalises over — the criterion operator, how many
//! criteria the filter holds, the size of a criterion's value list, and whether the
//! update grows, shrinks or repeats the stored filter — with the recorded
//! `demo`/`rule1`/`resourceType eq AWS::S3::Bucket` case as one row. Every case is read
//! back through `GetArchiveRule` and `ListArchiveRules`; nothing reaches into state.

use serde_json::{json, Value};

use crate::harness::{is_timestamp, listed_rule_names, timestamp_at_least, Emulator};

/// The analyzer every case in this suite hangs its rules off.
const ANALYZER: &str = "demo";

/// One row of the update table: the filter a rule is created with, and the filter an
/// update then replaces it with.
#[derive(Debug)]
struct UpdateCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    rule_name: &'static str,
    created_with: Value,
    updated_to: Value,
}

/// A criterion whose `eq` list holds `count` distinct values.
///
/// `Criterion`'s `eq`/`neq`/`contains` lists carry `length 1..20`, from
/// `smithy_query(query="shape", shape="Criterion")`, so these are the sizes an accepted
/// filter spans.
fn eq_values(count: usize) -> Value {
    let values: Vec<String> = (0..count).map(|index| format!("value-{index}")).collect();
    json!({"eq": values})
}

/// Create the analyzer this suite's rules hang off.
async fn emulator_with_analyzer() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator
        .create_analyzer(json!({"analyzerName": ANALYZER, "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        created.status, 200,
        "the analyzer under test must be created, got {created:?}"
    );
    emulator
}

/// An update replaces the stored criteria with exactly what it carried, over the whole
/// class of filter shapes.
#[tokio::test]
async fn an_update_replaces_the_stored_filter() {
    // The rows differ along the members the rule generalises over: each `Criterion`
    // operator (`eq`, `neq`, `contains`, `exists`), a filter that grows, one that
    // shrinks (so a criterion the update leaves out is dropped, not merged), one whose
    // value list sits on each published bound (1 and 20), and one that repeats the
    // stored filter unchanged.
    let cases = vec![
        UpdateCase {
            case: "the recorded case: one eq criterion replaced by another",
            rule_name: "rule1",
            created_with: json!({"resourceType": {"eq": ["AWS::S3::Bucket"]}}),
            updated_to: json!({"resourceType": {"eq": ["AWS::IAM::Role"]}}),
        },
        UpdateCase {
            case: "eq replaced by neq on the same key",
            rule_name: "operatorSwap",
            created_with: json!({"principal.AWS": {"eq": ["123456789012"]}}),
            updated_to: json!({"principal.AWS": {"neq": ["123456789012"]}}),
        },
        UpdateCase {
            case: "contains replaced by exists",
            rule_name: "containsToExists",
            created_with: json!({"condition.aws:SourceVpc": {"contains": ["vpc-"]}}),
            updated_to: json!({"condition.aws:SourceVpc": {"exists": true}}),
        },
        UpdateCase {
            case: "exists true replaced by exists false",
            rule_name: "existsFlip",
            created_with: json!({"isPublic": {"exists": true}}),
            updated_to: json!({"isPublic": {"exists": false}}),
        },
        UpdateCase {
            case: "one criterion grows to three",
            rule_name: "grows",
            created_with: json!({"isPublic": {"eq": ["true"]}}),
            updated_to: json!({
                "isPublic": {"eq": ["false"]},
                "resourceType": {"eq": ["AWS::S3::Bucket", "AWS::SQS::Queue"]},
                "principal.AWS": {"exists": true},
            }),
        },
        UpdateCase {
            case: "three criteria shrink to one, dropping the others",
            rule_name: "shrinks",
            created_with: json!({
                "isPublic": {"eq": ["true"]},
                "resourceType": {"eq": ["AWS::S3::Bucket"]},
                "principal.AWS": {"neq": ["123456789012"]},
            }),
            updated_to: json!({"resourceType": {"contains": ["Queue"]}}),
        },
        UpdateCase {
            case: "a value list at the minimum length grows to the maximum",
            rule_name: "valueListBounds",
            created_with: json!({"resourceType": eq_values(1)}),
            updated_to: json!({"resourceType": eq_values(20)}),
        },
        UpdateCase {
            case: "a value list at the maximum length shrinks to the minimum",
            rule_name: "valueListShrinks",
            created_with: json!({"resourceType": eq_values(20)}),
            updated_to: json!({"resourceType": eq_values(1)}),
        },
        UpdateCase {
            case: "an update that repeats the stored filter",
            rule_name: "unchanged",
            created_with: json!({"isPublic": {"eq": ["true"]}}),
            updated_to: json!({"isPublic": {"eq": ["true"]}}),
        },
    ];

    let emulator = emulator_with_analyzer().await;

    let mut rule_names: Vec<String> = Vec::new();
    for UpdateCase {
        case,
        rule_name,
        created_with,
        updated_to,
    } in &cases
    {
        let created = emulator
            .create_archive_rule(
                ANALYZER,
                json!({"ruleName": rule_name, "filter": created_with}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the rule must be created first, got {created:?}"
        );
        rule_names.push((*rule_name).to_string());

        let before = emulator.get_archive_rule(ANALYZER, rule_name).await;
        let before_rule = before
            .member("archiveRule")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            before_rule.get("filter"),
            Some(created_with),
            "case {case:?}: the create must store its own filter, got {before:?}"
        );

        let updated = emulator
            .update_archive_rule(ANALYZER, rule_name, json!({"filter": updated_to}))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateArchiveRule must succeed, got {updated:?}"
        );

        let after = emulator.get_archive_rule(ANALYZER, rule_name).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the updated rule must still be readable, got {after:?}"
        );
        let after_rule = after.member("archiveRule").cloned().unwrap_or(Value::Null);

        assert_eq!(
            after_rule.get("ruleName").and_then(Value::as_str),
            Some(*rule_name),
            "case {case:?}: the update must not rename the rule, got {after:?}"
        );
        assert_eq!(
            after_rule.get("filter"),
            Some(updated_to),
            "case {case:?}: the stored filter must be exactly the updated one, got {:?}",
            after_rule.get("filter")
        );
        assert_eq!(
            after_rule.get("createdAt"),
            before_rule.get("createdAt"),
            "case {case:?}: createdAt must survive the update, got {:?}",
            after_rule.get("createdAt")
        );
        assert!(
            is_timestamp(after_rule.get("updatedAt")),
            "case {case:?}: updatedAt must be an instant from this run's clock, got {:?}",
            after_rule.get("updatedAt")
        );
        assert!(
            timestamp_at_least(after_rule.get("updatedAt"), after_rule.get("createdAt")),
            "case {case:?}: updatedAt must not precede createdAt, got {after_rule:?}"
        );
        assert!(
            timestamp_at_least(after_rule.get("updatedAt"), before_rule.get("updatedAt")),
            "case {case:?}: updatedAt must not move backwards, got {after_rule:?}"
        );
    }

    // No update created a rule: the analyzer holds exactly the rules the creates made.
    rule_names.sort();
    let list = emulator.list_archive_rules(ANALYZER, "").await;
    assert_eq!(
        listed_rule_names(&list),
        rule_names,
        "an update must create no rule, got {list:?}"
    );

    // And the listing reports the same updated filter the single read does.
    let listed: Vec<Value> = list
        .member("archiveRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for UpdateCase {
        case,
        rule_name,
        updated_to,
        ..
    } in &cases
    {
        let entry = listed
            .iter()
            .find(|item| item.get("ruleName").and_then(Value::as_str) == Some(*rule_name));
        assert_eq!(
            entry.and_then(|item| item.get("filter")),
            Some(updated_to),
            "case {case:?}: ListArchiveRules must report the updated filter, got {entry:?}"
        );
    }
}

/// An update names an existing rule: a name no create stored is the modelled 404, and
/// nothing is created by the refusal.
#[tokio::test]
async fn an_update_of_a_rule_that_does_not_exist_is_refused() {
    let emulator = emulator_with_analyzer().await;
    let created = emulator
        .create_archive_rule(
            ANALYZER,
            json!({"ruleName": "rule1", "filter": {"isPublic": {"eq": ["true"]}}}),
        )
        .await;
    assert_eq!(created.status, 200, "the rule must be created, got {created:?}");

    // (case, analyzer, rule) — each row names something that does not exist.
    let missing = [
        ("a rule the analyzer does not hold", ANALYZER, "nope"),
        ("an analyzer that does not exist", "missing", "rule1"),
        ("neither exists", "missing", "nope"),
    ];
    for (case, analyzer, rule) in &missing {
        let reply = emulator
            .update_archive_rule(
                analyzer,
                rule,
                json!({"filter": {"resourceType": {"eq": ["AWS::S3::Bucket"]}}}),
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {reply:?}"
        );
        assert!(
            reply.code().contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must be a ResourceNotFoundException, got {reply:?}"
        );

        // The refusal created nothing.
        let read_back = emulator.get_archive_rule(analyzer, rule).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: a refused update must create no rule, got {read_back:?}"
        );
    }

    // The rule that does exist is untouched by those refusals.
    let survivor = emulator.get_archive_rule(ANALYZER, "rule1").await;
    assert_eq!(
        survivor
            .member("archiveRule")
            .and_then(|rule| rule.get("filter")),
        Some(&json!({"isPublic": {"eq": ["true"]}})),
        "a refused update must not touch the stored rule, got {survivor:?}"
    );
}

/// A rule is scoped to the analyzer it was created under: updating the same rule name
/// on another analyzer changes only that analyzer's copy.
#[tokio::test]
async fn an_update_is_scoped_to_its_analyzer() {
    let emulator = Emulator::new();
    for analyzer in ["first", "second"] {
        let created = emulator
            .create_analyzer(json!({"analyzerName": analyzer, "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {analyzer:?}: the analyzer must be created, got {created:?}"
        );
        let rule = emulator
            .create_archive_rule(
                analyzer,
                json!({"ruleName": "shared", "filter": {"isPublic": {"eq": ["true"]}}}),
            )
            .await;
        assert_eq!(
            rule.status, 200,
            "case {analyzer:?}: the rule must be created, got {rule:?}"
        );
    }

    let updated = emulator
        .update_archive_rule(
            "first",
            "shared",
            json!({"filter": {"resourceType": {"eq": ["AWS::SQS::Queue"]}}}),
        )
        .await;
    assert_eq!(updated.status, 200, "the update must succeed, got {updated:?}");

    // (case, analyzer, expected filter)
    let expected = [
        (
            "the updated analyzer",
            "first",
            json!({"resourceType": {"eq": ["AWS::SQS::Queue"]}}),
        ),
        (
            "the untouched analyzer",
            "second",
            json!({"isPublic": {"eq": ["true"]}}),
        ),
    ];
    for (case, analyzer, filter) in &expected {
        let got = emulator.get_archive_rule(analyzer, "shared").await;
        assert_eq!(
            got.member("archiveRule").and_then(|rule| rule.get("filter")),
            Some(filter),
            "case {case:?}: expected filter {filter}, got {got:?}"
        );
    }
}

/// The boundary refuses an update outside the modelled constraints instead of storing
/// it: each published bound of `Name`/`AnalyzerName` and `Criterion`'s value lists, and
/// one value past it.
#[tokio::test]
async fn an_update_outside_the_modelled_constraints_is_refused() {
    // (case, ruleName, filter, expected status). The accepted rows sit ON the published
    // bounds — `Name` is `len 1..255` with pattern `^[A-Za-z][A-Za-z0-9_.-]*$` and a
    // `ValueList` is `len 1..20` — and the refused rows one step past them.
    let at_max_name = format!("n{}", "x".repeat(254));
    let past_max_name = format!("n{}", "x".repeat(255));
    let cases: Vec<(&str, String, Value, u16)> = vec![
        (
            "a rule name at the maximum length",
            at_max_name,
            json!({"isPublic": {"eq": ["true"]}}),
            200,
        ),
        (
            "a rule name of the minimum length",
            "z".to_string(),
            json!({"isPublic": {"eq": ["true"]}}),
            200,
        ),
        (
            "a value list at the maximum length",
            "maxValues".to_string(),
            json!({"resourceType": eq_values(20)}),
            200,
        ),
        (
            "one past the maximum rule-name length",
            past_max_name,
            json!({"isPublic": {"eq": ["true"]}}),
            400,
        ),
        (
            "a rule name the pattern rejects (leading digit)",
            "1rule".to_string(),
            json!({"isPublic": {"eq": ["true"]}}),
            400,
        ),
        (
            "one past the maximum value-list length",
            "tooManyValues".to_string(),
            json!({"resourceType": eq_values(21)}),
            400,
        ),
        (
            "an empty value list is below the minimum",
            "emptyValues".to_string(),
            json!({"resourceType": {"eq": []}}),
            400,
        ),
    ];

    for (case, rule_name, filter, expected) in &cases {
        let emulator = emulator_with_analyzer().await;
        // Only the accepted rows can be created first; a refused row is refused on the
        // update itself, which is what this table is about.
        if *expected == 200 {
            let created = emulator
                .create_archive_rule(
                    ANALYZER,
                    json!({"ruleName": rule_name, "filter": {"isPublic": {"eq": ["true"]}}}),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the rule must be creatable, got {created:?}"
            );
        }

        let reply = emulator
            .update_archive_rule(ANALYZER, rule_name, json!({"filter": filter}))
            .await;
        assert_eq!(
            reply.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {reply:?}"
        );

        if *expected == 400 {
            // A refused update stores nothing: the analyzer still holds no rule.
            let list = emulator.list_archive_rules(ANALYZER, "").await;
            assert_eq!(
                listed_rule_names(&list),
                Vec::<String>::new(),
                "case {case:?}: a refused update must store nothing, got {list:?}"
            );
        }
    }
}

/// Replaying the same update — the way a retry of an `@idempotent` call does, with or
/// without its `clientToken` — leaves the same stored filter and answers success.
#[tokio::test]
async fn replaying_an_update_is_idempotent() {
    let emulator = emulator_with_analyzer().await;
    let created = emulator
        .create_archive_rule(
            ANALYZER,
            json!({"ruleName": "retried", "filter": {"isPublic": {"eq": ["true"]}}}),
        )
        .await;
    assert_eq!(created.status, 200, "the rule must be created, got {created:?}");

    let filter = json!({"resourceType": {"eq": ["AWS::S3::Bucket"]}});
    // (case, request body) — the same update with a token, and without one.
    let bodies = [
        (
            "with a clientToken",
            json!({"filter": filter, "clientToken": "update-token-1"}),
        ),
        ("without a clientToken", json!({"filter": filter})),
    ];
    for attempt in 1..=3 {
        for (case, body) in &bodies {
            let reply = emulator
                .update_archive_rule(ANALYZER, "retried", body.clone())
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} attempt {attempt}: a replayed update must succeed, got {reply:?}"
            );
            let got = emulator.get_archive_rule(ANALYZER, "retried").await;
            assert_eq!(
                got.member("archiveRule").and_then(|rule| rule.get("filter")),
                Some(&filter),
                "case {case:?} attempt {attempt}: the filter must stay the updated one, got {got:?}"
            );
        }
    }

    // Still exactly one rule: a replay creates nothing.
    let list = emulator.list_archive_rules(ANALYZER, "").await;
    assert_eq!(
        listed_rule_names(&list),
        vec!["retried".to_string()],
        "a replayed update must create no rule, got {list:?}"
    );
}
