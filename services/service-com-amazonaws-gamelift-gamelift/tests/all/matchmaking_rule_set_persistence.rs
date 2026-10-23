// SPDX-License-Identifier: CC0-1.0

//! `CreateMatchmakingRuleSet`, over the wire: what a create persists is what every later
//! read of the rule set reports.
//!
//! The rule these suites pin: a create under a name no rule set holds stores ONE record —
//! the requested name, the ARN `matchmakingruleset/<name>` for that name, the rule set body
//! exactly as supplied and a creation time — and reports that stored record, which
//! `DescribeMatchmakingRuleSets` then serves unchanged, selected by either the name or the
//! ARN; a create the service refuses (a name a rule set already holds, a tag set that is
//! not a key-value map, a member outside its modelled constraint) stores nothing.
//!
//! Each table row differs along what the rule generalises over — the form and length of the
//! name, the formatting, element set and size of the body, and the number of tags assigned —
//! so the recorded case (a short name with a compact three-element body and no tags) is one
//! row among many.
//!
//! The harness (`GameLift`, `gamelift_arn`) is the one `hosting_loop` drives the production
//! dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::{gamelift_arn, GameLift};

/// The ARN a rule set named `name` is assigned: `MatchmakingRuleSet$RuleSetArn`'s doc —
/// "Format is `arn:aws:gamelift:<region>::matchmakingruleset/<ruleset name>`[;] the resource
/// ID matches the RuleSetName value."
fn rule_set_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingruleset/{name}"))
}

/// The modelled maximum length of `CreateMatchmakingRuleSetInput$Name`
/// (`smithy_query shape=MatchmakingIdStringModel`: length 0..128).
const MAX_RULE_SET_NAME_LENGTH: usize = 128;

/// The modelled maximum length of `CreateMatchmakingRuleSetInput$RuleSetBody`
/// (`smithy_query shape=RuleSetBody`: length 1..65535).
const MAX_RULE_SET_BODY_LENGTH: usize = 65_535;

/// The modelled maximum number of tags a create may assign
/// (`smithy_query shape=TagList`: length 0..200).
const MAX_TAGS: usize = 200;

/// The smallest body that conforms to the FlexMatch rule language: the version it is
/// written in, and the one team a match is built from.
fn compact_body() -> String {
    json!({
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// The same rule set, written out with newlines and indentation — a body's FORMATTING is
/// part of the string the caller supplied, so it must survive the round trip.
fn pretty_body() -> String {
    serde_json::to_string_pretty(&json!({
        "name": "pretty",
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 4}],
    }))
    .expect("render an indented rule set body")
}

/// A conforming body of `team_count` teams, each with its own player counts.
fn body_with_teams(team_count: usize) -> String {
    let teams: Vec<Value> = (0..team_count)
        .map(|index| json!({"name": format!("team-{index}"), "minPlayers": 1, "maxPlayers": 4}))
        .collect();
    json!({"ruleLanguageVersion": "1.0", "teams": teams}).to_string()
}

/// A conforming body padded with an unknown top-level element until it is exactly
/// `total_length` characters long, so a test can sit on the modelled `RuleSetBody` bounds.
/// The padding element is one the rule language does not define, which the service stores
/// and reports verbatim rather than interpreting.
fn padded_body(total_length: usize) -> String {
    let skeleton = json!({
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "maxPlayers": 2}],
        "description": "",
    })
    .to_string();
    let padding = total_length
        .checked_sub(skeleton.len())
        .expect("the padded body must be at least as long as its skeleton");
    let body = json!({
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "maxPlayers": 2}],
        "description": "x".repeat(padding),
    })
    .to_string();
    assert_eq!(
        body.len(),
        total_length,
        "the padded body must be exactly {total_length} characters long"
    );
    body
}

/// `count` distinct tags, as `Tags` carries them.
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect(),
    )
}

/// One create request: the members it sets, and the name it takes.
struct Case {
    label: &'static str,
    name: String,
    body: String,
    /// The `Tags` member the request carries, or `None` when it sets none at all.
    tags: Option<Value>,
}

/// The rows every accepted create is asserted over: the name form and length, the body's
/// formatting, element set and size, and the tag count — including each member's published
/// bound.
fn accepted_cases() -> Vec<Case> {
    vec![
        Case {
            label: "a plain name, the smallest conforming body, no tags at all",
            name: "solo".to_string(),
            body: compact_body(),
            tags: None,
        },
        Case {
            label: "a name carrying dots and digits, an indented body, an empty tag list",
            name: "Rules.v1.0".to_string(),
            body: pretty_body(),
            tags: Some(json!([])),
        },
        Case {
            label: "a hyphenated name, eight teams, one tag",
            name: "team-rules-2".to_string(),
            body: body_with_teams(8),
            tags: Some(tag_list(1)),
        },
        Case {
            label: "a one-character name and three tags",
            name: "0".to_string(),
            body: compact_body(),
            tags: Some(tag_list(3)),
        },
        Case {
            label: "a name at the modelled maximum length",
            name: "n".repeat(MAX_RULE_SET_NAME_LENGTH),
            body: body_with_teams(1),
            tags: None,
        },
        Case {
            label: "a body at the modelled maximum length",
            name: "longest.body-1".to_string(),
            body: padded_body(MAX_RULE_SET_BODY_LENGTH),
            tags: None,
        },
        Case {
            label: "a body of exactly one element past what the language requires",
            name: "extra-elements".to_string(),
            body: padded_body(200),
            tags: Some(tag_list(2)),
        },
        Case {
            label: "the maximum number of tags a create may assign",
            name: "fully.tagged".to_string(),
            body: compact_body(),
            tags: Some(tag_list(MAX_TAGS)),
        },
    ]
}

/// The create request `case` describes.
fn create_request(case: &Case) -> Value {
    let mut request = json!({"Name": &case.name, "RuleSetBody": &case.body});
    if let Some(tags) = &case.tags {
        request["Tags"] = tags.clone();
    }
    request
}

/// Every accepted create persists one record and reports it, and
/// `DescribeMatchmakingRuleSets` serves that same record back — selected by the name or by
/// the ARN the create minted, and unchanged by being read.
#[tokio::test]
async fn a_created_rule_set_reads_back_whatever_form_its_name_body_and_tags_take() {
    let cases = accepted_cases();
    let gamelift = GameLift::new();

    for case in &cases {
        let label = case.label;
        let created = gamelift
            .ok("CreateMatchmakingRuleSet", create_request(case))
            .await;
        let reported = created["RuleSet"].clone();

        assert_eq!(
            reported["RuleSetName"],
            json!(&case.name),
            "case {label:?}: the created rule set must carry the requested name, got {reported}"
        );
        assert_eq!(
            reported["RuleSetArn"],
            json!(rule_set_arn(&case.name)),
            "case {label:?}: the created rule set's ARN must name it, got {reported}"
        );
        assert_eq!(
            reported["RuleSetBody"],
            json!(&case.body),
            "case {label:?}: the body must be reported exactly as supplied, got {}",
            reported["RuleSetBody"]
        );
        assert!(
            reported["CreationTime"].is_number(),
            "case {label:?}: the created rule set must carry a numeric CreationTime, got {reported}"
        );

        // The describe serves the STORED record, by either identifier, and twice over: a
        // read cannot change what the next reader sees.
        for selector in [case.name.clone(), rule_set_arn(&case.name)] {
            for attempt in 1..=2 {
                let described = gamelift
                    .ok("DescribeMatchmakingRuleSets", json!({"Names": [&selector]}))
                    .await;
                assert_eq!(
                    described["RuleSets"],
                    json!([reported.clone()]),
                    "case {label:?}: describe {attempt} naming {selector:?} must report the \
                     created record, got {described}"
                );
                assert_eq!(
                    described["NextToken"],
                    Value::Null,
                    "case {label:?}: a single-name describe has no further page, got {described}"
                );
            }
        }
    }

    // "To request all existing rule sets, leave this parameter empty": every row's record
    // is in the store, exactly once.
    let all = gamelift
        .ok("DescribeMatchmakingRuleSets", json!({}))
        .await;
    let listed = all["RuleSets"]
        .as_array()
        .expect("DescribeMatchmakingRuleSets must report a RuleSets list")
        .clone();
    assert_eq!(
        listed.len(),
        cases.len(),
        "the store must hold one record per accepted create, got {} records",
        listed.len()
    );
    for case in &cases {
        let matching: Vec<&Value> = listed
            .iter()
            .filter(|rule_set| rule_set["RuleSetName"] == json!(&case.name))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {:?}: the unfiltered describe must report its rule set exactly once, got {} \
             matches",
            case.label,
            matching.len()
        );
    }
}

/// A rule set name identifies ONE rule set: a second create under a name a rule set already
/// holds is refused with the modelled `InvalidRequestException`, and the stored record is
/// left exactly as the first create wrote it — whatever the second request asks for.
///
/// The rows differ along what the second request changes: the body, the tags, both, or
/// nothing at all.
#[tokio::test]
async fn a_rule_set_name_identifies_one_rule_set() {
    let gamelift = GameLift::new();
    let name = "taken.name-1";
    let first = gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": name, "RuleSetBody": compact_body(), "Tags": tag_list(1)}),
        )
        .await;
    let stored = first["RuleSet"].clone();

    let retries: Vec<(&str, Value)> = vec![
        (
            "the same request again",
            json!({"Name": name, "RuleSetBody": compact_body(), "Tags": tag_list(1)}),
        ),
        (
            "a different body",
            json!({"Name": name, "RuleSetBody": body_with_teams(3)}),
        ),
        (
            "a different, longer body and more tags",
            json!({"Name": name, "RuleSetBody": padded_body(4_096), "Tags": tag_list(5)}),
        ),
        (
            "an indented body and no tags",
            json!({"Name": name, "RuleSetBody": pretty_body()}),
        ),
    ];

    for (label, request) in &retries {
        let (status, code) = gamelift
            .err("CreateMatchmakingRuleSet", request.clone())
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: a create under a name a rule set holds must be refused, got {status} \
             {code}"
        );
        let described = gamelift
            .ok("DescribeMatchmakingRuleSets", json!({"Names": [name]}))
            .await;
        assert_eq!(
            described["RuleSets"],
            json!([stored.clone()]),
            "case {label:?}: the refused create must leave the stored rule set untouched, got \
             {described}"
        );
    }

    // A name no rule set holds is still free, and the two records stay distinct.
    let other = gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "free.name-2", "RuleSetBody": body_with_teams(2)}),
        )
        .await;
    assert_ne!(
        other["RuleSet"]["RuleSetArn"], stored["RuleSetArn"],
        "two rule sets must carry distinct ARNs, got {other} and {stored}"
    );
}

/// Tags are a key-value MAP, so a create assigning one key twice is refused with the
/// modelled `TaggingFailedException` ("This may be due to invalid tag format") and stores
/// nothing; a tag list longer than the modelled maximum is refused one step earlier, by the
/// boundary walk.
///
/// The rows differ along where the repeated key sits and how many tags surround it, and along
/// which published bound the list sits on.
#[tokio::test]
async fn a_tag_set_that_is_not_a_key_value_map_is_refused_and_stores_nothing() {
    struct TagCase {
        label: &'static str,
        tags: Value,
        /// The wire error code the create must answer with, or `None` when it is accepted.
        expected: Option<&'static str>,
    }

    let mut repeated_among_many: Vec<Value> = (0..MAX_TAGS - 1)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
        .collect();
    repeated_among_many.push(json!({"Key": "key-0", "Value": "again"}));

    let cases = [
        TagCase {
            label: "no tags at all, the low bound of the modelled list",
            tags: json!([]),
            expected: None,
        },
        TagCase {
            label: "one tag",
            tags: tag_list(1),
            expected: None,
        },
        TagCase {
            label: "a tag whose value is empty, which the model allows",
            tags: json!([{"Key": "empty-value", "Value": ""}]),
            expected: None,
        },
        TagCase {
            label: "the maximum number of tags the model allows",
            tags: tag_list(MAX_TAGS),
            expected: None,
        },
        TagCase {
            label: "one key assigned two different values",
            tags: json!([{"Key": "stage", "Value": "test"}, {"Key": "stage", "Value": "prod"}]),
            expected: Some("TaggingFailedException"),
        },
        TagCase {
            label: "one key assigned the same value twice",
            tags: json!([{"Key": "stage", "Value": "test"}, {"Key": "stage", "Value": "test"}]),
            expected: Some("TaggingFailedException"),
        },
        TagCase {
            label: "a repeated key at the end of a full tag list",
            tags: Value::Array(repeated_among_many),
            expected: Some("TaggingFailedException"),
        },
        TagCase {
            label: "one tag past the modelled maximum",
            tags: tag_list(MAX_TAGS + 1),
            expected: Some("ValidationException"),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("tagged-row-{index}");
        let request = json!({
            "Name": &name,
            "RuleSetBody": compact_body(),
            "Tags": case.tags.clone(),
        });
        match case.expected {
            None => {
                let created = gamelift.ok("CreateMatchmakingRuleSet", request).await;
                assert_eq!(
                    created["RuleSet"]["RuleSetName"],
                    json!(&name),
                    "case {label:?}: the create must be served, got {created}"
                );
            }
            Some(expected) => {
                let (status, code) = gamelift.err("CreateMatchmakingRuleSet", request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {label:?}: the create must be refused with {expected}, got {status} \
                     {code}"
                );
                // Nothing was stored, so the name still names no rule set.
                let (status, code) = gamelift
                    .err("DescribeMatchmakingRuleSets", json!({"Names": [&name]}))
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "InvalidRequestException"),
                    "case {label:?}: a refused create must store nothing, got {status} {code}"
                );
            }
        }
    }
}

/// A member outside its modelled constraint is refused by the boundary walk as
/// `ValidationException`, and stores nothing: each published bound of `Name`
/// (`MatchmakingIdStringModel`, length 0..128, pattern `^[a-zA-Z0-9-\.]*$`) and `RuleSetBody`
/// (`RuleSetBody`, length 1..65535) is accepted, and one past it refused.
#[tokio::test]
async fn a_member_past_its_modelled_bound_is_refused_and_stores_nothing() {
    let gamelift = GameLift::new();

    // On the bounds: the longest name and the longest body are served.
    let longest_name = "a".repeat(MAX_RULE_SET_NAME_LENGTH);
    let created = gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": &longest_name, "RuleSetBody": padded_body(MAX_RULE_SET_BODY_LENGTH)}),
        )
        .await;
    assert_eq!(
        created["RuleSet"]["RuleSetName"],
        json!(&longest_name),
        "a name at the modelled maximum must be stored whole, got {created}"
    );

    // One past each bound.
    let past_bounds: Vec<(&str, Value)> = vec![
        (
            "a name one character past the modelled maximum",
            json!({
                "Name": "b".repeat(MAX_RULE_SET_NAME_LENGTH + 1),
                "RuleSetBody": compact_body(),
            }),
        ),
        (
            "a body one character past the modelled maximum",
            json!({
                "Name": "past.body-bound",
                "RuleSetBody": padded_body(MAX_RULE_SET_BODY_LENGTH + 1),
            }),
        ),
        (
            "a body one character short of the modelled minimum",
            json!({"Name": "empty-body", "RuleSetBody": ""}),
        ),
        (
            "a name carrying a character the modelled pattern forbids",
            json!({"Name": "under_score", "RuleSetBody": compact_body()}),
        ),
        (
            "a name carrying a space",
            json!({"Name": "two words", "RuleSetBody": compact_body()}),
        ),
    ];

    for (label, request) in &past_bounds {
        let (status, code) = gamelift
            .err("CreateMatchmakingRuleSet", request.clone())
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: a member past its modelled bound must be refused by the boundary \
             walk, got {status} {code}"
        );
    }

    // Only the one accepted record is in the store.
    let all = gamelift
        .ok("DescribeMatchmakingRuleSets", json!({}))
        .await;
    assert_eq!(
        all["RuleSets"].as_array().map(Vec::len),
        Some(1),
        "a refused create must store nothing, got {all}"
    );
}
