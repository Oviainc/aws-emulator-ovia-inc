// SPDX-License-Identifier: CC0-1.0

//! The Call Analytics category lifecycle, end to end over the wire:
//! `CreateCallAnalyticsCategory` -> `GetCallAnalyticsCategory` ->
//! `DeleteCallAnalyticsCategory`.
//!
//! The rules these tests pin, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. A create RECORDS the category the request describes — its rules, its input type
//!    (`POST_CALL` when the request names none) and its tags — and answers the stored
//!    record, which the read then reports unchanged.
//! 2. A read REPORTS and never mutates: two reads of one category answer the same
//!    properties, timestamps included.
//! 3. A delete REMOVES the named category and only that one, so a read of that name is
//!    refused afterwards while every other category is still reported.
//! 4. A name this account holds no category under is refused — on the read, and on the
//!    delete, which is not an already-satisfied success.
//! 5. A name this account already holds a category under is refused on the create, and
//!    the stored category is left as it was. Category names are CASE SENSITIVE, so
//!    names differing only in case are different categories.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator, so these
//! tests exercise the awsJson1_1 envelope and the boundary constraint validation too.

use serde_json::{json, Value};

use crate::call_analytics_rule_negate::as_recorded;
use crate::transcription_jobs::{Reply, Transcribe};

/// The modelled client error this family's refusals carry — a category it does not hold
/// and a category name it already holds alike (`crate::call_analytics_category_refusals`)
/// — and the status its awsJson1_1 wire protocol puts on every client error.
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// One `NonTalkTimeFilter` rule, from the union's own member list
/// (`smithy_query(query="closure", operation="CreateCallAnalyticsCategory")`).
/// `Threshold` is a `TimestampMilliseconds`, modelled `@range(min: 0, max: 14400000)`.
///
/// `pub(crate)`, like the three rule builders below it: the listing suite
/// (`crate::call_analytics_category_listing`) builds its categories from the same four
/// members of the `Rule` union rather than spelling a second set.
pub(crate) fn non_talk_time_rule(threshold: i64) -> Value {
    json!({
        "NonTalkTimeFilter": {
            "Threshold": threshold,
            "RelativeTimeRange": {"First": 30},
            "Negate": false
        }
    })
}

/// One `InterruptionFilter` rule. `ParticipantRole` is the modelled enum
/// `AGENT | CUSTOMER`.
pub(crate) fn interruption_rule(threshold: i64) -> Value {
    json!({
        "InterruptionFilter": {
            "Threshold": threshold,
            "ParticipantRole": "AGENT",
            "AbsoluteTimeRange": {"First": 60_000},
            "Negate": false
        }
    })
}

/// One `TranscriptFilter` rule. `TranscriptFilterType` is the modelled enum whose only
/// value is `EXACT`, and `Targets` is required with `@length(min: 1)`.
pub(crate) fn transcript_rule(target: &str) -> Value {
    json!({
        "TranscriptFilter": {
            "TranscriptFilterType": "EXACT",
            "Targets": [target],
            "ParticipantRole": "CUSTOMER",
            "Negate": true
        }
    })
}

/// One `SentimentFilter` rule. `SentimentValueList` is modelled
/// `@length(min: 1, max: 1)`, so exactly one of `POSITIVE | NEGATIVE | NEUTRAL | MIXED`.
pub(crate) fn sentiment_rule(sentiment: &str) -> Value {
    json!({
        "SentimentFilter": {
            "Sentiments": [sentiment],
            "RelativeTimeRange": {"StartPercentage": 0, "EndPercentage": 50},
            "ParticipantRole": "CUSTOMER"
        }
    })
}

/// A rule list at the modelled `RuleList` ceiling (`@length(min: 1, max: 20)`), cycling
/// through all four members of the `Rule` union.
fn twenty_rules() -> Vec<Value> {
    (0..20)
        .map(|index| match index % 4 {
            0 => non_talk_time_rule(1_000 * index),
            1 => interruption_rule(500 * index),
            2 => transcript_rule(&format!("phrase {index}")),
            _ => sentiment_rule("NEGATIVE"),
        })
        .collect()
}

/// One category a row of the table creates.
struct Row {
    case: &'static str,
    name: String,
    /// The `InputType` the create sends, or `None` to send none at all.
    input_type: Option<&'static str>,
    rules: Vec<Value>,
    /// The tags the create sends, or `None` to send no `Tags` member.
    tags: Option<Vec<(&'static str, &'static str)>>,
    /// The `InputType` the service must report for the created category.
    expected_input_type: &'static str,
}

impl Row {
    /// This row's `CreateCallAnalyticsCategory` request body.
    fn create_request(&self) -> Value {
        let mut request = json!({"CategoryName": self.name, "Rules": self.rules});
        if let Some(input_type) = self.input_type {
            request["InputType"] = json!(input_type);
        }
        if let Some(tags) = &self.tags {
            request["Tags"] = tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect();
        }
        request
    }

    /// The `Tags` list this row's category must report, or `None` when it carries no
    /// tags — `TagList` is modelled `@length(min: 1, max: 200)`, so a category with no
    /// tags leaves the member out rather than reporting an empty list.
    fn expected_tags(&self) -> Option<Value> {
        self.tags.as_ref().map(|tags| {
            tags.iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect()
        })
    }
}

/// The categories the table spans, one row per axis the rules generalise over: the
/// form of the name (the shortest the member's `@length(1..200)` /
/// `@pattern(^[0-9a-zA-Z._-]+$)` allows, every allowed punctuation character, a name at
/// the 200-character ceiling), which member of the `Rule` union the rules use, how many
/// rules there are (one, two, and the modelled ceiling of twenty), whether the request
/// names an `InputType` (and which of `POST_CALL | REAL_TIME`), and whether it carries
/// tags.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one non-talk-time rule, no InputType, no tags",
            name: "a".to_string(),
            input_type: None,
            rules: vec![non_talk_time_rule(10_000)],
            tags: None,
            // "If you do not include InputType, your category is created as a post-call
            // category by default" (model doc for
            // `CreateCallAnalyticsCategoryRequest$InputType`).
            expected_input_type: "POST_CALL",
        },
        Row {
            case: "an interruption rule, POST_CALL, one tag",
            name: "a.b_c-d.0_9".to_string(),
            input_type: Some("POST_CALL"),
            rules: vec![interruption_rule(4_000)],
            tags: Some(vec![("team", "analytics")]),
            expected_input_type: "POST_CALL",
        },
        Row {
            case: "a transcript rule, REAL_TIME, two tags",
            name: "c".repeat(200),
            input_type: Some("REAL_TIME"),
            rules: vec![transcript_rule("please hold")],
            tags: Some(vec![("team", "analytics"), ("env", "test")]),
            expected_input_type: "REAL_TIME",
        },
        Row {
            case: "a sentiment rule and a non-talk-time rule, no InputType",
            name: "sentiment-and-silence".to_string(),
            input_type: None,
            rules: vec![sentiment_rule("POSITIVE"), non_talk_time_rule(0)],
            tags: None,
            expected_input_type: "POST_CALL",
        },
        Row {
            case: "twenty rules, the modelled RuleList ceiling, REAL_TIME",
            name: "twenty.rules".to_string(),
            input_type: Some("REAL_TIME"),
            rules: twenty_rules(),
            tags: Some(vec![("env", "")]),
            expected_input_type: "REAL_TIME",
        },
    ]
}

/// The `CategoryProperties` of a successful answer.
fn properties<'a>(reply: &'a Reply, case: &str) -> &'a Value {
    let body = reply.ok(case);
    body.get("CategoryProperties").unwrap_or_else(|| {
        panic!("case {case:?}: expected CategoryProperties in the answer, got {body:?}")
    })
}

/// Assert `properties` describes the category `row` asked for.
fn describes(found: &Value, row: &Row) {
    let case = row.case;
    assert_eq!(
        found.get("CategoryName"),
        Some(&json!(row.name)),
        "case {case:?}: the category must report the name it was created under, got {found:?}"
    );
    assert_eq!(
        found.get("InputType"),
        Some(&json!(row.expected_input_type)),
        "case {case:?}: expected InputType {:?}, got {found:?}",
        row.expected_input_type
    );
    assert_eq!(
        found.get("Rules"),
        Some(&as_recorded(&Value::Array(row.rules.clone()))),
        "case {case:?}: the category must report the rules it was created with, as it \
         records them, got {found:?}"
    );
    assert_eq!(
        found.get("Tags").cloned(),
        row.expected_tags(),
        "case {case:?}: expected the tags the create carried, got {found:?}"
    );
    let create_time = found.get("CreateTime").and_then(Value::as_f64);
    let last_update_time = found.get("LastUpdateTime").and_then(Value::as_f64);
    assert!(
        create_time.is_some_and(|time| time > 0.0),
        "case {case:?}: expected a CreateTime, got {found:?}"
    );
    assert_eq!(
        create_time, last_update_time,
        "case {case:?}: a category never updated was last updated when it was created, \
         got {found:?}"
    );
}

/// RULES 1 and 2: a create records the category the request describes and answers the
/// stored record; the read reports that same record, and reading it twice changes
/// nothing about it.
#[tokio::test]
async fn a_created_category_is_reported_by_the_read() {
    let transcribe = Transcribe::new();

    for row in rows() {
        let created = transcribe
            .call("CreateCallAnalyticsCategory", row.create_request())
            .await;
        let created = properties(&created, row.case).clone();
        describes(&created, &row);

        let read = transcribe
            .call(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": row.name}),
            )
            .await;
        let read = properties(&read, row.case).clone();
        describes(&read, &row);
        assert_eq!(
            read, created,
            "case {:?}: the read must report what the create stored",
            row.case
        );

        // RULE 2: the read is reporting, not mutating — a second one answers the same
        // properties, timestamps included.
        let again = transcribe
            .call(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": row.name}),
            )
            .await;
        assert_eq!(
            properties(&again, row.case),
            &read,
            "case {:?}: reading a category must not change it",
            row.case
        );
    }
}

/// RULE 3: a delete removes the named category and ONLY that one — every category it
/// did not name is still reported afterwards, and the deleted name is not.
#[tokio::test]
async fn a_delete_removes_only_the_named_category() {
    let transcribe = Transcribe::new();
    let rows = rows();

    for row in &rows {
        transcribe
            .call("CreateCallAnalyticsCategory", row.create_request())
            .await
            .ok(row.case);
    }

    for (index, row) in rows.iter().enumerate() {
        let deleted = transcribe
            .call(
                "DeleteCallAnalyticsCategory",
                json!({"CategoryName": row.name}),
            )
            .await;
        deleted.ok_status(row.case, 200);
        let empty = match &deleted.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        };
        assert!(
            empty,
            "case {:?}: a delete must answer an empty body, got {:?}",
            row.case, deleted.body
        );

        for (other, state) in rows.iter().enumerate() {
            let read = transcribe
                .call(
                    "GetCallAnalyticsCategory",
                    json!({"CategoryName": state.name}),
                )
                .await;
            if other <= index {
                read.refused(state.case, CLIENT_ERROR_STATUS, ABSENT_CODE);
            } else {
                describes(properties(&read, state.case), state);
            }
        }
    }
}

/// The names a read and a delete are probed with for a category that was never
/// created, one row per form the member's own `@length(1..200)` /
/// `@pattern(^[0-9a-zA-Z._-]+$)` constraint allows
/// (`smithy_query(query="operation", operation="DeleteCallAnalyticsCategory")`).
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest allowed name", "z".to_string()),
        ("every allowed punctuation character", "z.y_x-w.1_8".to_string()),
        ("a plainly named category", "never-created-at-all".to_string()),
        ("a name at the 200-character ceiling", "n".repeat(200)),
    ]
}

/// RULE 4: a name this account holds no category under is refused — by the read, and
/// by the delete, both before anything was ever created and after the category of that
/// name was deleted.
#[tokio::test]
async fn a_category_this_account_does_not_hold_is_refused() {
    let transcribe = Transcribe::new();

    for (name_case, name) in absent_names() {
        for operation in ["GetCallAnalyticsCategory", "DeleteCallAnalyticsCategory"] {
            let case = format!("{operation} / {name_case}");
            let reply = transcribe
                .call(operation, json!({"CategoryName": name}))
                .await;
            reply.refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        }
    }

    // The same refusal after a real create and delete of the name: a repeated delete is
    // refused rather than answered as already satisfied.
    for row in rows() {
        transcribe
            .call("CreateCallAnalyticsCategory", row.create_request())
            .await
            .ok(row.case);
        transcribe
            .call(
                "DeleteCallAnalyticsCategory",
                json!({"CategoryName": row.name}),
            )
            .await
            .ok_status(row.case, 200);

        for operation in ["GetCallAnalyticsCategory", "DeleteCallAnalyticsCategory"] {
            let case = format!("{operation} / {} after its delete", row.case);
            transcribe
                .call(operation, json!({"CategoryName": row.name}))
                .await
                .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        }
    }
}

/// RULE 5: a create naming a category this account already holds is refused with the
/// shape this family refuses a taken name with — `BadRequestException`, as its lookups
/// refuse with too, not the `ConflictException` the operation also models and the
/// vocabulary and job families answer (`crate::call_analytics_category_refusals` pins
/// the shape and the wording) — and the stored category keeps the rules, input type and
/// tags of the create that won.
#[tokio::test]
async fn creating_a_category_whose_name_is_taken_is_refused() {
    let transcribe = Transcribe::new();

    for row in rows() {
        transcribe
            .call("CreateCallAnalyticsCategory", row.create_request())
            .await
            .ok(row.case);

        // The second create differs from the first in every member it may differ in, so
        // the assertion below cannot pass by accident.
        let mut second = json!({
            "CategoryName": row.name,
            "Rules": [transcript_rule("a different rule")],
            "InputType": "REAL_TIME",
            "Tags": [{"Key": "second", "Value": "create"}]
        });
        if row.expected_input_type == "REAL_TIME" {
            second["InputType"] = json!("POST_CALL");
        }
        let conflict = transcribe
            .call("CreateCallAnalyticsCategory", second)
            .await;
        conflict.refused(row.case, CLIENT_ERROR_STATUS, ABSENT_CODE);

        let read = transcribe
            .call(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": row.name}),
            )
            .await;
        describes(properties(&read, row.case), &row);
    }
}

/// RULE 5, the case-sensitivity axis: "Category names are case sensitive", so names
/// differing only in case are different categories — each created, read and deleted on
/// its own.
#[tokio::test]
async fn category_names_are_case_sensitive() {
    let transcribe = Transcribe::new();
    let case = "names differing only in case";

    for name in ["Sentiment.Positive", "sentiment.positive", "SENTIMENT.POSITIVE"] {
        transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({
                    "CategoryName": name,
                    "Rules": [transcript_rule(name)],
                    "InputType": "POST_CALL"
                }),
            )
            .await
            .ok(case);
    }

    // Each name holds its OWN category: the rules report the name they were created
    // with.
    for name in ["Sentiment.Positive", "sentiment.positive", "SENTIMENT.POSITIVE"] {
        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        assert_eq!(
            properties(&read, case).get("Rules"),
            Some(&as_recorded(&json!([transcript_rule(name)]))),
            "case {case:?}: {name} must report its own rules, got {:?}",
            read.body
        );
    }

    transcribe
        .call(
            "DeleteCallAnalyticsCategory",
            json!({"CategoryName": "sentiment.positive"}),
        )
        .await
        .ok_status(case, 200);
    transcribe
        .call(
            "GetCallAnalyticsCategory",
            json!({"CategoryName": "sentiment.positive"}),
        )
        .await
        .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    for survivor in ["Sentiment.Positive", "SENTIMENT.POSITIVE"] {
        transcribe
            .call(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": survivor}),
            )
            .await
            .ok(case);
    }
}
