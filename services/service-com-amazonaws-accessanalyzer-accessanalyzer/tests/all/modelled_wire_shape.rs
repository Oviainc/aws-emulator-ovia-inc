// SPDX-License-Identifier: CC0-1.0

//! The wire shape this service's reads and refusals must have, driven over the wire.
//!
//! Four rules are under test here, each asserted over a TABLE that spans what it
//! generalises over rather than over the one recorded request:
//!
//! 1. A `Timestamp` member whose model declares `@timestampFormat("date-time")` is an
//!    ISO-8601/RFC3339 STRING on the wire, never the restJson1 protocol default of epoch
//!    seconds — in every reader that projects the shape, single-resource and list alike.
//!    The members are `AnalyzerSummary$createdAt`, `AnalyzerSummary$lastResourceAnalyzedAt`
//!    and `ArchiveRuleSummary$createdAt`/`$updatedAt`
//!    (`smithy_query(query="shape",shape="AnalyzerSummary,ArchiveRuleSummary")`).
//! 2. A resource's tag collection always exists: an untagged resource carries the EMPTY
//!    map, so `AnalyzerSummary$tags` (`TagsMap`, `map<String,String>`) is present on every
//!    read and is `{}` when the create carried no tags.
//! 3. A resource removed by its delete is gone: every later read of it answers the
//!    modelled `ResourceNotFoundException` (404), and the listing it belonged to no longer
//!    carries it.
//! 4. A request member outside a modelled constraint is refused with this service's
//!    modelled `ValidationException`, which declares BOTH `message` and `reason`
//!    `@required` — so the 400 carries a `ValidationExceptionReason` value
//!    (`fieldValidationFailed`) and a `fieldList` naming the member, not a message alone.
//!
//! Every case goes through the real router (see `tests/all/harness.rs`), so the restJson1
//! routing, the boundary constraint validation, the handler and the response
//! serialization all run exactly as they do in the emulator binary.

use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use super::harness::{Emulator, Reply};

/// The one `ValidationExceptionReason` value a member-constraint refusal reports.
///
/// From the model's own enumeration: `smithy_query(query="shape",
/// shape="ValidationExceptionReason")` lists `unknownOperation | cannotParse |
/// fieldValidationFailed | other | notSupported`, and AWS's API reference for this
/// service publishes the same lowerCamel wire values — so a field that failed its
/// declared constraint is `fieldValidationFailed`.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `resourceType` an archive-rule-scoped refusal reports.
const ARCHIVE_RULE_RESOURCE_TYPE: &str = "archive-rule";

/// The `resourceType` an analyzer-scoped refusal reports.
const ANALYZER_RESOURCE_TYPE: &str = "analyzer";

/// Seconds since the epoch, as a wire timestamp carries them.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A `date-time` member's instant, or why the value is not one.
///
/// The only accepted encoding is a JSON string that `aws_smithy_types` parses under
/// [`Format::DateTime`] — the format the member's `@timestampFormat` names. A JSON number
/// (the protocol's epoch-seconds default) is reported as the defect it is, with the value
/// it carried.
fn date_time_member(value: Option<&Value>) -> Result<f64, String> {
    match value {
        Some(Value::String(text)) => match DateTime::from_str(text, Format::DateTime) {
            Ok(parsed) => Ok(parsed.as_secs_f64()),
            Err(error) => Err(format!("{text:?} is not an RFC3339 date-time: {error}")),
        },
        Some(Value::Number(number)) => {
            Err(format!("epoch-seconds number {number}, not a date-time string"))
        }
        other => Err(format!("{other:?}, not a date-time string")),
    }
}

/// Assert that `value` is a `date-time` string naming an instant from this run's clock.
///
/// `floor` is a bound taken before the resource was created, so a zero/default stand-in or
/// a value the handler invented is rejected as well as a mis-encoded one.
fn assert_date_time(case: &str, member: &str, value: Option<&Value>, floor: f64) -> f64 {
    match date_time_member(value) {
        Ok(seconds) => {
            let ceiling = now_secs() + 1.0;
            assert!(
                seconds >= floor - 1.0 && seconds <= ceiling,
                "case {case:?}: {member} must name an instant from this run \
                 ({floor}..={ceiling}), got {seconds}"
            );
            seconds
        }
        Err(why) => panic!("case {case:?}: {member} must be a date-time string: {why}"),
    }
}

/// One analyzer, with the archive rules created under it, for the timestamp table.
#[derive(Debug)]
struct ShapeCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    analyzer: &'static str,
    analyzer_type: &'static str,
    /// The `tags` the create carries; `None` when it carries the member at all.
    tags: Option<Value>,
    /// `(ruleName, filter)` for every archive rule created under this analyzer. The
    /// filters span the `Criterion` members the model declares (`eq`, `neq`, `contains`,
    /// `exists` — `smithy_query(query="shape",shape="Criterion")`).
    rules: Vec<(&'static str, Value)>,
}

fn shape_cases() -> Vec<ShapeCase> {
    vec![
        ShapeCase {
            case: "the recorded case: ACCOUNT analyzer, tagged, one eq-filtered rule",
            analyzer: "demo",
            analyzer_type: "ACCOUNT",
            tags: Some(json!({"env": "test"})),
            rules: vec![(
                "ArchiveS3",
                json!({"resourceType": {"eq": ["AWS::S3::Bucket"]}}),
            )],
        },
        ShapeCase {
            case: "unused-access analyzer, no tags member, no rules",
            analyzer: "unused_access_probe",
            analyzer_type: "ACCOUNT_UNUSED_ACCESS",
            tags: None,
            rules: Vec::new(),
        },
        ShapeCase {
            case: "organization analyzer, three rules spanning the criterion members",
            analyzer: "Org.analyzer-9",
            analyzer_type: "ORGANIZATION",
            tags: Some(json!({"owner": "team", "cost-center": "42"})),
            rules: vec![
                (
                    "NotIam",
                    json!({"resourceType": {"neq": ["AWS::IAM::Role"]}}),
                ),
                (
                    "ContainsPublic",
                    json!({"principal.AWS": {"contains": ["*"]}}),
                ),
                ("HasCondition", json!({"condition.aws:SourceVpc": {"exists": true}})),
            ],
        },
    ]
}

/// RULE 1: every `date-time` timestamp member is an RFC3339 string, in the
/// single-resource reader and in the list projection alike, and the same instant is
/// reported by both.
#[tokio::test]
async fn date_time_members_are_rfc3339_strings_in_every_reader() {
    let floor = now_secs();
    let emulator = Emulator::new();
    let cases = shape_cases();

    for ShapeCase {
        case,
        analyzer,
        analyzer_type,
        tags,
        rules,
    } in &cases
    {
        let mut request = json!({"analyzerName": analyzer, "type": analyzer_type});
        if let Some(tags) = tags {
            request["tags"] = tags.clone();
        }
        let create = emulator.create_analyzer(request).await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: CreateAnalyzer must succeed, got {create:?}"
        );

        for (rule, filter) in rules {
            let created = emulator
                .create_archive_rule(analyzer, json!({"ruleName": rule, "filter": filter}))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateArchiveRule({rule}) must succeed, got {created:?}"
            );
        }

        // GetAnalyzer: the single-resource read.
        let get = emulator.get_analyzer(analyzer).await;
        assert_eq!(
            get.status, 200,
            "case {case:?}: GetAnalyzer must succeed, got {get:?}"
        );
        let summary = get
            .member("analyzer")
            .unwrap_or_else(|| panic!("case {case:?}: GetAnalyzer must carry `analyzer`: {get:?}"));
        let created_at = assert_date_time(case, "analyzer.createdAt", summary.get("createdAt"), floor);
        // A fresh analyzer has analyzed nothing, so the optional sibling member is absent
        // rather than mis-encoded; when it IS present it must take the same form.
        if let Some(value) = summary
            .get("lastResourceAnalyzedAt")
            .filter(|value| !value.is_null())
        {
            assert_date_time(
                case,
                "analyzer.lastResourceAnalyzedAt",
                Some(value),
                floor,
            );
        }

        // ListAnalyzers: the same member through the list projection.
        let listed = emulator.list_analyzers("").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListAnalyzers must succeed, got {listed:?}"
        );
        let entry = listed
            .member("analyzers")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("name").and_then(Value::as_str) == Some(*analyzer))
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListAnalyzers must carry {analyzer}: {listed:?}")
            });
        let listed_created_at = assert_date_time(
            case,
            "analyzers[].createdAt",
            entry.get("createdAt"),
            floor,
        );
        assert_eq!(
            listed_created_at, created_at,
            "case {case:?}: the list must report the SAME createdAt as the read, \
             got {listed_created_at} and {created_at}"
        );

        // GetArchiveRule / ListArchiveRules: both archive-rule timestamp members.
        let rule_list = emulator.list_archive_rules(analyzer, "").await;
        assert_eq!(
            rule_list.status, 200,
            "case {case:?}: ListArchiveRules must succeed, got {rule_list:?}"
        );
        let listed_rules = rule_list
            .member("archiveRules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListArchiveRules must carry `archiveRules`: {rule_list:?}")
            })
            .clone();
        assert_eq!(
            listed_rules.len(),
            rules.len(),
            "case {case:?}: ListArchiveRules must report every created rule, got {listed_rules:?}"
        );

        for (rule, _) in rules {
            let got = emulator.get_archive_rule(analyzer, rule).await;
            assert_eq!(
                got.status, 200,
                "case {case:?}: GetArchiveRule({rule}) must succeed, got {got:?}"
            );
            let summary = got.member("archiveRule").unwrap_or_else(|| {
                panic!("case {case:?}: GetArchiveRule({rule}) must carry `archiveRule`: {got:?}")
            });
            let rule_created = assert_date_time(
                case,
                "archiveRule.createdAt",
                summary.get("createdAt"),
                floor,
            );
            let rule_updated = assert_date_time(
                case,
                "archiveRule.updatedAt",
                summary.get("updatedAt"),
                floor,
            );
            assert!(
                rule_updated >= rule_created,
                "case {case:?}: updatedAt must be at or after createdAt, \
                 got {rule_updated} and {rule_created}"
            );

            let entry = listed_rules
                .iter()
                .find(|item| item.get("ruleName").and_then(Value::as_str) == Some(*rule))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: ListArchiveRules must carry {rule}: {listed_rules:?}")
                });
            assert_eq!(
                assert_date_time(
                    case,
                    "archiveRules[].createdAt",
                    entry.get("createdAt"),
                    floor
                ),
                rule_created,
                "case {case:?}: the list must report the SAME rule createdAt as the read"
            );
            assert_eq!(
                assert_date_time(
                    case,
                    "archiveRules[].updatedAt",
                    entry.get("updatedAt"),
                    floor
                ),
                rule_updated,
                "case {case:?}: the list must report the SAME rule updatedAt as the read"
            );
        }
    }
}

/// One row of the tag-projection table: the `tags` a create carries, and what every later
/// read must report for it.
#[derive(Debug)]
struct TagCase {
    case: &'static str,
    analyzer: &'static str,
    /// The `tags` member the create carries, absent when `None`.
    sent: Option<Value>,
    /// What the read must report: the same map, or the empty map when none was sent.
    expected: Value,
}

/// A one-pair tag map built at runtime, for a row whose key or value is not a literal.
fn tag_map(key: String, value: &str) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(key, Value::String(value.to_string()));
    Value::Object(map)
}

/// RULE 2: `tags` is a collection that always exists, so it is present on every read and
/// empty — `{}`, never absent — for a resource created without tags.
#[tokio::test]
async fn an_analyzers_tag_map_is_always_present() {
    // The rows span what the rule generalises over: no `tags` member at all, an
    // explicitly empty map, one pair, several pairs, and the edges `TagsMap`'s
    // documentation publishes for a pair — a 128-character key (its maximum) and a
    // zero-length value (its minimum), from
    // `smithy_query(query="doc",shape="AnalyzerSummary",member="tags")`.
    let cases = vec![
        TagCase {
            case: "no tags member",
            analyzer: "untagged",
            sent: None,
            expected: json!({}),
        },
        TagCase {
            case: "an explicitly empty tags map",
            analyzer: "empty_tags",
            sent: Some(json!({})),
            expected: json!({}),
        },
        TagCase {
            case: "the recorded case: one pair",
            analyzer: "demo",
            sent: Some(json!({"env": "test"})),
            expected: json!({"env": "test"}),
        },
        TagCase {
            case: "three pairs",
            analyzer: "many_tags",
            sent: Some(json!({"owner": "team", "cost-center": "42", "note": "a b"})),
            expected: json!({"owner": "team", "cost-center": "42", "note": "a b"}),
        },
        TagCase {
            case: "a 128-character key and a zero-length value",
            analyzer: "edge_tags",
            sent: Some(tag_map("k".repeat(128), "")),
            expected: tag_map("k".repeat(128), ""),
        },
    ];

    let emulator = Emulator::new();
    for TagCase {
        case,
        analyzer,
        sent,
        expected,
    } in &cases
    {
        let mut request = json!({"analyzerName": analyzer, "type": "ACCOUNT"});
        if let Some(sent) = sent {
            request["tags"] = sent.clone();
        }
        let create = emulator.create_analyzer(request).await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: CreateAnalyzer must succeed, got {create:?}"
        );

        let get = emulator.get_analyzer(analyzer).await;
        let summary = get
            .member("analyzer")
            .unwrap_or_else(|| panic!("case {case:?}: GetAnalyzer must carry `analyzer`: {get:?}"));
        assert_eq!(
            summary.get("tags"),
            Some(expected),
            "case {case:?}: the read must report the tag map (empty when none was sent), got {:?}",
            summary.get("tags")
        );

        let listed = emulator.list_analyzers("").await;
        let entry = listed
            .member("analyzers")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("name").and_then(Value::as_str) == Some(*analyzer))
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListAnalyzers must carry {analyzer}: {listed:?}")
            });
        assert_eq!(
            entry.get("tags"),
            Some(expected),
            "case {case:?}: the list projection must report the same tag map, got {:?}",
            entry.get("tags")
        );
    }
}

/// `ruleName` forms the delete rule has to hold over (`Name`:
/// `^[A-Za-z][A-Za-z0-9_.-]*$`, len 1..255, from
/// `smithy_query(query="operation",operation="DeleteArchiveRule")`): one character, a
/// plain name, every character class the pattern allows, and the 255-character maximum.
fn deletable_rule_names() -> Vec<String> {
    vec![
        "z".to_string(),
        "ArchiveS3".to_string(),
        "Mixed.Case-9_rule".to_string(),
        format!("r{}", "u".repeat(254)),
    ]
}

/// RULE 3: the delete removes the rule from state, so every later read of it is the
/// modelled 404 and the listing no longer carries it.
#[tokio::test]
async fn a_deleted_archive_rule_is_gone_from_every_reader() {
    let emulator = Emulator::new();
    let analyzer = "delete_probe";
    let create = emulator
        .create_analyzer(json!({"analyzerName": analyzer, "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        create.status, 200,
        "CreateAnalyzer must succeed, got {create:?}"
    );

    let names = deletable_rule_names();
    for rule in &names {
        let created = emulator
            .create_archive_rule(
                analyzer,
                json!({"ruleName": rule, "filter": {"resourceType": {"eq": ["AWS::S3::Bucket"]}}}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {rule:?}: CreateArchiveRule must succeed, got {created:?}"
        );
    }

    for (index, rule) in names.iter().enumerate() {
        let case = format!("rule {rule:?}");
        let before = emulator.get_archive_rule(analyzer, rule).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the rule must be readable before the delete, got {before:?}"
        );

        let deleted = emulator.delete_archive_rule(analyzer, rule).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteArchiveRule must succeed, got {deleted:?}"
        );

        let after = emulator.get_archive_rule(analyzer, rule).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: reading a deleted rule must be a 404, got {after:?}"
        );
        assert_eq!(
            after.code(),
            "ResourceNotFoundException",
            "case {case:?}: the refusal must name the modelled error, got {after:?}"
        );
        assert_eq!(
            after.member("resourceType").and_then(Value::as_str),
            Some(ARCHIVE_RULE_RESOURCE_TYPE),
            "case {case:?}: the refusal must report the archive-rule resource type, got {after:?}"
        );
        assert_eq!(
            after.member("resourceId").and_then(Value::as_str),
            Some(rule.as_str()),
            "case {case:?}: the refusal must report the rule asked for, got {after:?}"
        );
        assert!(
            after
                .member("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must carry a message, got {after:?}"
        );

        // Deleting it again finds nothing to delete, and says so with the same modelled
        // refusal rather than reporting a second success.
        let again = emulator.delete_archive_rule(analyzer, rule).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting an absent rule must be a 404, got {again:?}"
        );
        assert_eq!(
            again.code(),
            "ResourceNotFoundException",
            "case {case:?}: the second delete must name the modelled error, got {again:?}"
        );

        // The listing drops exactly the deleted rule and keeps the rest.
        let listed = emulator.list_archive_rules(analyzer, "").await;
        let reported: Vec<String> = listed
            .member("archiveRules")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("ruleName").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<String> = {
            let mut remaining: Vec<String> = names[index + 1..].to_vec();
            remaining.sort();
            remaining
        };
        let mut reported_sorted = reported.clone();
        reported_sorted.sort();
        assert_eq!(
            reported_sorted, expected,
            "case {case:?}: the listing must carry exactly the undeleted rules, got {reported:?}"
        );
    }

    // The analyzer itself is untouched by its rules' deletes, and a delete under an
    // analyzer that does not exist is refused as an ANALYZER miss.
    let analyzer_read = emulator.get_analyzer(analyzer).await;
    assert_eq!(
        analyzer_read.status, 200,
        "the analyzer must survive its rules' deletes, got {analyzer_read:?}"
    );
    let missing_analyzer = emulator.delete_archive_rule("no_such_analyzer", "any").await;
    assert_eq!(
        missing_analyzer.status, 404,
        "a delete under an unknown analyzer must be a 404, got {missing_analyzer:?}"
    );
    assert_eq!(
        missing_analyzer
            .member("resourceType")
            .and_then(Value::as_str),
        Some(ANALYZER_RESOURCE_TYPE),
        "the refusal must report the analyzer resource type, got {missing_analyzer:?}"
    );
}

/// One row of the refusal table: a query string, and the member the refusal must name.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    query: String,
    /// The member whose constraint the value breaks, or `None` for a row that satisfies
    /// every constraint and must therefore NOT be refused.
    member: Option<&'static str>,
}

/// A `principalArn` that satisfies the model's pattern, with a resource segment of
/// `length` characters (`.{1,576}` in `^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$`).
fn principal_arn(entity: &str, length: usize) -> String {
    format!(
        "arn:aws:iam::123456789012:{entity}/{}",
        "p".repeat(length)
    )
}

/// RULE 4: a refusal of a request member carries the modelled `reason` beside its
/// `message`, because the model marks both `@required`.
#[tokio::test]
async fn a_refused_request_member_carries_the_modelled_reason() {
    // The rows span the two constrained members of this operation and, for the pattern,
    // the ways a value can break it (not an ARN at all, the wrong service, a partition or
    // account segment that cannot be there, an entity type the pattern does not list) plus
    // both published bounds of its resource segment and one character past the maximum.
    let cases = vec![
        RefusalCase {
            case: "the recorded case: not an ARN at all",
            query: "?principalArn=not-an-arn".to_string(),
            member: Some("principalArn"),
        },
        RefusalCase {
            case: "an ARN of another service",
            query: "?principalArn=arn:aws:s3:::my-bucket".to_string(),
            member: Some("principalArn"),
        },
        RefusalCase {
            case: "an IAM ARN carrying a region, which the pattern forbids",
            query: "?principalArn=arn:aws:iam:us-east-1:123456789012:role/r".to_string(),
            member: Some("principalArn"),
        },
        RefusalCase {
            case: "an entity type the pattern does not list",
            query: "?principalArn=arn:aws:iam::123456789012:group/admins".to_string(),
            member: Some("principalArn"),
        },
        RefusalCase {
            case: "a role ARN with no resource segment at all (below the 1-character minimum)",
            query: format!("?principalArn={}", principal_arn("role", 0)),
            member: Some("principalArn"),
        },
        RefusalCase {
            case: "a role ARN one character past the 576-character maximum",
            query: format!("?principalArn={}", principal_arn("role", 577)),
            member: Some("principalArn"),
        },
        RefusalCase {
            case: "maxResults below the modelled minimum of 1",
            query: "?maxResults=0".to_string(),
            member: Some("maxResults"),
        },
        RefusalCase {
            case: "a negative maxResults",
            query: "?maxResults=-3".to_string(),
            member: Some("maxResults"),
        },
        RefusalCase {
            case: "a role ARN at the 1-character minimum",
            query: format!("?principalArn={}", principal_arn("role", 1)),
            member: None,
        },
        RefusalCase {
            case: "a user ARN at the 576-character maximum",
            query: format!("?principalArn={}", principal_arn("user", 576)),
            member: None,
        },
        RefusalCase {
            case: "a role ARN with maxResults at the modelled minimum",
            query: format!("?principalArn={}&maxResults=1", principal_arn("role", 8)),
            member: None,
        },
    ];

    let emulator = Emulator::new();
    for RefusalCase {
        case,
        query,
        member,
    } in &cases
    {
        let reply = emulator.list_policy_generations(query).await;
        let Some(member) = member else {
            assert_ne!(
                reply.status, 400,
                "case {case:?}: a request inside every modelled constraint must not be \
                 refused as invalid, got {reply:?}"
            );
            assert_ne!(
                reply.code(),
                "ValidationException",
                "case {case:?}: a well-formed request must not answer ValidationException, \
                 got {reply:?}"
            );
            continue;
        };

        assert_eq!(
            reply.status, 400,
            "case {case:?}: a value outside its modelled constraint must be a 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: the refusal must name the modelled error, got {reply:?}"
        );
        assert_eq!(
            reply.member("reason").and_then(Value::as_str),
            Some(REASON_FIELD_VALIDATION_FAILED),
            "case {case:?}: the refusal must carry the required `reason`, got {reply:?}"
        );
        let message = reply
            .member("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the message must name the member that failed, got {message:?}"
        );
        let named: Vec<&str> = reply
            .member("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            named.contains(member),
            "case {case:?}: fieldList must name {member}, got {named:?}"
        );
    }
}

/// A listing's refusal of its own paging arguments carries the same modelled `reason`:
/// the rule is about every refusal this service raises, not about one operation.
#[tokio::test]
async fn a_paging_refusal_carries_the_modelled_reason_too() {
    let emulator = Emulator::new();
    let create = emulator
        .create_analyzer(json!({"analyzerName": "paging_probe", "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        create.status, 200,
        "CreateAnalyzer must succeed, got {create:?}"
    );

    // Both members a listing refuses, through both listings that page.
    let cases: Vec<(&str, Reply)> = vec![
        (
            "ListAnalyzers with a maxResults of 0",
            emulator.list_analyzers("?maxResults=0").await,
        ),
        (
            "ListAnalyzers with a nextToken this service never issued",
            emulator.list_analyzers("?nextToken=not-a-token").await,
        ),
        (
            "ListArchiveRules with a maxResults of 0",
            emulator.list_archive_rules("paging_probe", "?maxResults=0").await,
        ),
        (
            "ListArchiveRules with a nextToken this service never issued",
            emulator
                .list_archive_rules("paging_probe", "?nextToken=not-a-token")
                .await,
        ),
    ];

    for (case, reply) in &cases {
        assert_eq!(
            reply.status, 400,
            "case {case:?}: the refusal must be a 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: the refusal must name the modelled error, got {reply:?}"
        );
        assert_eq!(
            reply.member("reason").and_then(Value::as_str),
            Some(REASON_FIELD_VALIDATION_FAILED),
            "case {case:?}: the refusal must carry the required `reason`, got {reply:?}"
        );
    }
}
