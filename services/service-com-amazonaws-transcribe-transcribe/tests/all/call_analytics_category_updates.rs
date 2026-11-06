// SPDX-License-Identifier: CC0-1.0

//! `UpdateCallAnalyticsCategory`, end to end over the wire, on categories created and
//! read back through `CreateCallAnalyticsCategory` / `GetCallAnalyticsCategory`.
//!
//! The rules these tests pin, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. An update OVERWRITES the category's rules: "the UpdateCallAnalyticsCategory
//!    operation overwrites all existing rules contained in the specified category. You
//!    cannot append additional rules onto an existing category." The stored rule list
//!    afterwards is exactly the request's, whatever it replaced.
//! 2. It changes nothing else: the name, the input type and the tags of the category
//!    survive it, `CreateTime` does not move and `LastUpdateTime` moves to the instant
//!    of the call.
//! 3. "The input type you specify must match the input type specified when the category
//!    was created", so an update naming the other one is refused and the stored
//!    category is left as it was; an update naming none keeps the stored one.
//! 4. A name this account holds no category under is refused, and the refused update
//!    creates nothing.
//!
//! The category-lifecycle suite beside this one (`call_analytics_categories`) covers
//! the create, the read and the delete; this suite adds only the update. Every call
//! goes through the same `MultiServiceRouter` the production router dispatches on,
//! reusing the transcription-job suite's harness (`crate::transcription_jobs`) rather
//! than standing up a second emulator, so these tests exercise the awsJson1_1 envelope
//! and the boundary constraint validation too.

use std::time::Duration;

use serde_json::{json, Value};

use crate::call_analytics_rule_negate::as_recorded;
use crate::transcription_jobs::{Reply, Transcribe};

/// The status this service's awsJson1_1 wire protocol puts on every client error.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The modelled client error this service answers for a category it does not hold, the
/// same one its reader and its delete answer.
const ABSENT_CODE: &str = "BadRequestException";

/// Long enough that a timestamp taken after it is strictly later than one taken
/// before, at the precision the awsJson1_1 envelope renders a `DateTime` with.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const A_VISIBLE_MOMENT: Duration = Duration::from_millis(1_100);

/// One rule of each member of the model's `Rule` union, so a rule list can be built
/// from any of them (`smithy_query(query="shape", shape="Rule")`). `which` picks the
/// member; `threshold` makes two rules of the same member distinguishable.
///
/// The enum values are the model's own: `ParticipantRole` is `AGENT | CUSTOMER`,
/// `SentimentValue` is `POSITIVE | NEGATIVE | NEUTRAL | MIXED`, `TranscriptFilterType`
/// is `EXACT`, `Threshold` is `@range(min: 0, max: 14400000)` milliseconds and
/// `Percentage` is `@range(min: 0, max: 100)`.
fn rule(which: usize, threshold: i64) -> Value {
    match which % 4 {
        0 => json!({"NonTalkTimeFilter": {
            "Threshold": threshold,
            "AbsoluteTimeRange": {"First": 50_000},
            "Negate": false
        }}),
        1 => json!({"InterruptionFilter": {
            "Threshold": threshold,
            "ParticipantRole": "AGENT",
            "RelativeTimeRange": {"StartPercentage": 0, "EndPercentage": 50},
            "Negate": true
        }}),
        2 => json!({"TranscriptFilter": {
            "TranscriptFilterType": "EXACT",
            "Targets": ["speak to a manager", "cancel my plan"],
            "ParticipantRole": "CUSTOMER",
            "AbsoluteTimeRange": {"StartTime": threshold, "EndTime": 14_400_000_i64}
        }}),
        _ => json!({"SentimentFilter": {
            "Sentiments": ["NEGATIVE"],
            "ParticipantRole": "CUSTOMER",
            "RelativeTimeRange": {"Last": 100}
        }}),
    }
}

/// `count` rules, cycling through the four union members and giving each its own
/// threshold, so the list's order and its length are both observable in the answer.
/// `RuleList` is modelled `@length(min: 1, max: 20)`.
fn rules(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| rule(index, 1_000 * (index as i64 + 1)))
            .collect(),
    )
}

/// The categories the update is exercised on, one row per axis the rules generalise
/// over: how many rules the category starts with (one, two and the modelled ceiling of
/// twenty), which union member they are, which `InputType` the create named (each of
/// the model's two, plus none at all, whose recorded default is `POST_CALL`), the form
/// of the name, and whether the category carries tags.
///
/// Each row is `(case, create request, the InputType the category must report)`.
fn category_rows() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "one non-talk-time rule, no InputType",
            json!({"CategoryName": "silence", "Rules": rules(1)}),
            "POST_CALL",
        ),
        (
            "two rules of different union members, POST_CALL",
            json!({
                "CategoryName": "silence.and-interruptions_v2",
                "Rules": rules(2),
                "InputType": "POST_CALL"
            }),
            "POST_CALL",
        ),
        (
            "one transcript filter, REAL_TIME, with tags",
            json!({
                "CategoryName": "escalation",
                "Rules": [rule(2, 10_000)],
                "InputType": "REAL_TIME",
                "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]
            }),
            "REAL_TIME",
        ),
        (
            "one sentiment filter, REAL_TIME",
            json!({
                "CategoryName": "unhappy-customer",
                "Rules": [rule(3, 0)],
                "InputType": "REAL_TIME"
            }),
            "REAL_TIME",
        ),
        (
            "the modelled maximum of 20 rules, POST_CALL",
            json!({
                "CategoryName": "twenty.rules",
                "Rules": rules(20),
                "InputType": "POST_CALL"
            }),
            "POST_CALL",
        ),
        (
            "a single-character name and a threshold at the modelled ceiling",
            json!({"CategoryName": "c", "Rules": [rule(1, 14_400_000_i64)]}),
            "POST_CALL",
        ),
    ]
}

/// The `CategoryProperties` of a successful answer.
fn properties(reply: &Reply, case: &str) -> Value {
    reply
        .ok(case)
        .get("CategoryProperties")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CategoryProperties in the answer, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// One member of a `CategoryProperties`.
fn property(properties: &Value, case: &str, name: &str) -> Value {
    properties
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} in {properties:?}"))
        .clone()
}

/// RULES 1 and 2: the update replaces every rule of the category and leaves the rest of
/// it alone, and the replacement is what the store then holds.
#[tokio::test]
async fn an_update_replaces_every_rule_and_keeps_the_rest_of_the_category() {
    let transcribe = Transcribe::new();

    // One row per direction the rule list can change: fewer rules, more rules, the
    // modelled maximum, and a single rule of another union member.
    let replacements: Vec<(&str, Value)> = vec![
        ("replaced by one rule", rules(1)),
        ("replaced by three rules", rules(3)),
        ("replaced by the modelled maximum of 20", rules(20)),
        ("replaced by a single sentiment filter", json!([rule(3, 0)])),
    ];

    for (case, request, expected_input_type) in category_rows() {
        let name = request["CategoryName"].as_str().expect("a name").to_string();
        let created = transcribe
            .call("CreateCallAnalyticsCategory", request.clone())
            .await;
        let created_properties = properties(&created, case);
        let create_time = created_properties["CreateTime"]
            .as_f64()
            .expect("a numeric CreateTime");
        let tags = created_properties.get("Tags").cloned();

        for (replacement_case, replacement) in &replacements {
            let case = format!("{case} / {replacement_case}");
            let case = case.as_str();

            let updated = transcribe
                .call(
                    "UpdateCallAnalyticsCategory",
                    json!({"CategoryName": name, "Rules": replacement}),
                )
                .await;
            updated.ok_status(case, 200);
            let updated_properties = properties(&updated, case);

            assert_eq!(
                property(&updated_properties, case, "Rules"),
                as_recorded(replacement),
                "case {case:?}: the updated rules must be exactly the request's, as the \
                 service records them, got {updated_properties:?}"
            );
            assert_eq!(
                property(&updated_properties, case, "CategoryName"),
                json!(name),
                "case {case:?}: the update must not rename the category, got \
                 {updated_properties:?}"
            );
            assert_eq!(
                property(&updated_properties, case, "InputType"),
                json!(expected_input_type),
                "case {case:?}: an update naming no InputType keeps the stored one \
                 ({expected_input_type}), got {updated_properties:?}"
            );
            assert_eq!(
                updated_properties.get("Tags").cloned(),
                tags,
                "case {case:?}: the update must not change the category's tags, got \
                 {updated_properties:?}"
            );
            assert_eq!(
                property(&updated_properties, case, "CreateTime").as_f64(),
                Some(create_time),
                "case {case:?}: the update must not move CreateTime, got \
                 {updated_properties:?}"
            );
            let last_update = property(&updated_properties, case, "LastUpdateTime")
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: expected a numeric LastUpdateTime"));
            assert!(
                last_update >= create_time,
                "case {case:?}: expected LastUpdateTime at or after CreateTime \
                 {create_time}, got {last_update}"
            );

            // The replacement is what the store now holds, not just what the update
            // answered.
            let read = transcribe
                .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
                .await;
            assert_eq!(
                properties(&read, case),
                updated_properties,
                "case {case:?}: the stored category must be the updated one, got {:?}",
                read.body
            );
        }
    }
}

/// RULE 2, the timestamp axis: the update moves `LastUpdateTime` to the instant of the
/// call and leaves `CreateTime` where it was.
#[tokio::test]
async fn an_update_moves_only_the_last_update_time() {
    let transcribe = Transcribe::new();
    let case = "a category updated a visible moment after it was created";

    let created = transcribe
        .call(
            "CreateCallAnalyticsCategory",
            json!({"CategoryName": "timestamps", "Rules": rules(2)}),
        )
        .await;
    let created_properties = properties(&created, case);
    let create_time = created_properties["CreateTime"]
        .as_f64()
        .expect("a numeric CreateTime");

    std::thread::sleep(A_VISIBLE_MOMENT);

    let updated = transcribe
        .call(
            "UpdateCallAnalyticsCategory",
            json!({"CategoryName": "timestamps", "Rules": rules(1)}),
        )
        .await;
    let updated_properties = properties(&updated, case);
    assert_eq!(
        updated_properties["CreateTime"].as_f64(),
        Some(create_time),
        "case {case:?}: expected CreateTime to stay {create_time}, got {updated_properties:?}"
    );
    let last_update = updated_properties["LastUpdateTime"]
        .as_f64()
        .expect("a numeric LastUpdateTime");
    assert!(
        last_update > create_time,
        "case {case:?}: expected LastUpdateTime after CreateTime {create_time}, got \
         {last_update}"
    );
}

/// RULE 3: an update naming the input type the category was NOT created with is
/// refused and changes nothing; one naming the stored input type is served.
#[tokio::test]
async fn an_update_naming_the_other_input_type_is_refused_and_changes_nothing() {
    let transcribe = Transcribe::new();

    // Both directions, the category created with no InputType at all (whose stored
    // type is the documented POST_CALL default), and the matching case that must be
    // served rather than refused.
    let rows = [
        ("created POST_CALL, updated REAL_TIME", Some("POST_CALL"), "REAL_TIME"),
        ("created REAL_TIME, updated POST_CALL", Some("REAL_TIME"), "POST_CALL"),
        ("created by default, updated REAL_TIME", None, "REAL_TIME"),
        ("created POST_CALL, updated POST_CALL", Some("POST_CALL"), "POST_CALL"),
        ("created REAL_TIME, updated REAL_TIME", Some("REAL_TIME"), "REAL_TIME"),
    ];

    for (index, (case, created_type, requested_type)) in rows.iter().enumerate() {
        let name = format!("input-type-{index}");
        let mut request = json!({"CategoryName": name, "Rules": rules(2)});
        if let Some(created_type) = created_type {
            request["InputType"] = json!(created_type);
        }
        let created = transcribe
            .call("CreateCallAnalyticsCategory", request)
            .await;
        let stored = properties(&created, case);
        let stored_type = stored["InputType"]
            .as_str()
            .expect("an InputType")
            .to_string();

        let updated = transcribe
            .call(
                "UpdateCallAnalyticsCategory",
                json!({
                    "CategoryName": name,
                    "Rules": rules(1),
                    "InputType": requested_type
                }),
            )
            .await;

        if *requested_type == stored_type {
            // The matching input type is no refusal at all: the rules are replaced.
            updated.ok_status(case, 200);
            assert_eq!(
                properties(&updated, case)["Rules"],
                as_recorded(&rules(1)),
                "case {case:?}: an update naming the stored input type must replace the \
                 rules, got {:?}",
                updated.body
            );
            continue;
        }

        updated.refused(case, CLIENT_ERROR_STATUS, "BadRequestException");
        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        assert_eq!(
            properties(&read, case),
            stored,
            "case {case:?}: a refused update must leave the stored category untouched, got \
             {:?}",
            read.body
        );
    }
}

/// RULE 4: a category this account does not hold is refused, and the refused update
/// creates nothing.
#[tokio::test]
async fn an_update_of_a_category_this_account_does_not_hold_is_refused() {
    let transcribe = Transcribe::new();

    // Names no create in this test ever used, one per form the member's own
    // `@pattern(^[0-9a-zA-Z._-]+$)` / `@length(1..200)` allows.
    let names = [
        ("the shortest allowed name", "q".to_string()),
        ("every allowed punctuation character", "q.r_s-t.2_7".to_string()),
        ("a plainly named category", "never-created-at-all".to_string()),
        ("a name at the 200-character ceiling", "u".repeat(200)),
    ];

    for (name_case, name) in names {
        let case = format!("an update of {name_case}");
        let case = case.as_str();

        transcribe
            .call(
                "UpdateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": rules(1)}),
            )
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);

        // The refused update stored nothing: the reader still holds no such category.
        transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }

    // A category name is case sensitive for the update too: the name as created is
    // updated, a name differing only in case is not a category this account holds.
    let case = "an update naming the created category in another case";
    transcribe
        .call(
            "CreateCallAnalyticsCategory",
            json!({"CategoryName": "CaseSensitive", "Rules": rules(2)}),
        )
        .await
        .ok(case);
    transcribe
        .call(
            "UpdateCallAnalyticsCategory",
            json!({"CategoryName": "casesensitive", "Rules": rules(1)}),
        )
        .await
        .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    let read = transcribe
        .call(
            "GetCallAnalyticsCategory",
            json!({"CategoryName": "CaseSensitive"}),
        )
        .await;
    assert_eq!(
        properties(&read, case)["Rules"],
        as_recorded(&rules(2)),
        "case {case:?}: the created category must keep its own rules, got {:?}",
        read.body
    );
}
