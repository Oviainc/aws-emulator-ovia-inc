// SPDX-License-Identifier: CC0-1.0

//! The update side of the Call Analytics time-range co-requirement: what
//! `UpdateCallAnalyticsCategory` alone can get wrong, because its rules REPLACE the
//! stored ones.
//!
//! THE RULE UNDER TEST is the one the suite beside this one
//! (`crate::call_analytics_rule_time_ranges`) states and drives over both operations that
//! accept rules: a time range bounds a custom segment with a PAIR of members co-required
//! in BOTH directions, so a request carrying one end and not the other is refused with
//! the modelled `BadRequestException` (HTTP 400) and stores nothing
//! (`smithy_query(query="shape", shape="AbsoluteTimeRange,RelativeTimeRange")`).
//!
//! This suite adds only what is specific to the UPDATE, whose rules "overwrite all
//! existing rules contained in the specified category":
//!
//! 1. the refusal is worded on the update exactly as on the create — the member PATH the
//!    caller sent, at the index the malformed rule sits at, and the companion it needs —
//!    and the category's previously valid rules survive it untouched, timestamps and all;
//! 2. the malformed set is never stored even when the request gets something ELSE wrong
//!    too: a category this account holds no record of, or an `InputType` contradicting
//!    the stored one.
//!
//! The rows, the padding rule and the expected wording come from that suite
//! (`whole_rule`, `missing_companion_message`), and the emulator and the answer
//! assertions from the transcription-job harness (`crate::transcription_jobs`), so
//! neither is stood up a second time.

use serde_json::{json, Value};

use crate::call_analytics_rule_negate::as_recorded;
use crate::call_analytics_rule_time_ranges::{missing_companion_message, whole_rule};
use crate::transcription_jobs::Transcribe;

/// The modelled client error every refusal here carries (`@httpError 400`), and the
/// status this service's awsJson1_1 wire protocol puts on every client error.
const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// How this service words "no such category" — the refusal an update naming a category
/// this account holds no record of earns instead of the one under test.
///
/// Read from the family's own refusal suite
/// (`crate::call_analytics_category_refusals`), which pins that wording for every
/// operation of the family, so the sentence has one home in these tests.
fn not_found_message() -> String {
    crate::call_analytics_category_refusals::ABSENT_CATEGORY_MESSAGE.to_string()
}

/// How the update words "that is not the input type this category was created with":
/// "The input type you specify must match the input type specified when the category was
/// created" (the model doc for `UpdateCallAnalyticsCategory$InputType`).
fn input_type_message(requested: &str, stored: &str) -> String {
    format!(
        "The InputType {requested} in your request doesn't match the {stored} input type \
         this category was created with. Specify {stored} and try your request again."
    )
}

/// One malformed replacement set: the rules the update sends, and the two member paths
/// the refusal must name.
///
/// The rows span both directions of each co-required pair, all four members of the `Rule`
/// union (`smithy_query(query="shape", shape="Rule")`), values inside each member's
/// published range and at its bounds (`TimestampMilliseconds` is `@range(min: 0, max:
/// 14400000)`, `Percentage` `@range(min: 0, max: 100)`), the open-ended `First` / `Last`
/// sitting beside a half pair (they "can be used independently of each other", so neither
/// completes one), the other shape whole beside a half one, and a malformed rule at the
/// last index of a list at `RuleList`'s published ceiling of 20.
fn rows() -> Vec<(&'static str, Value, &'static str, &'static str)> {
    let twenty = |malformed: Value| {
        let mut rules: Vec<Value> = (0..19).map(|_| whole_rule()).collect();
        rules.push(malformed);
        Value::Array(rules)
    };
    vec![
        (
            "a non-talk-time segment that starts and never ends",
            json!([{"NonTalkTimeFilter": {
                "Threshold": 30_000,
                "AbsoluteTimeRange": {"StartTime": 10_000}
            }}]),
            "Rules[0].NonTalkTimeFilter.AbsoluteTimeRange.StartTime",
            "Rules[0].NonTalkTimeFilter.AbsoluteTimeRange.EndTime",
        ),
        (
            "a non-talk-time segment that ends at the published ceiling and never starts",
            json!([{"NonTalkTimeFilter": {"AbsoluteTimeRange": {"EndTime": 14_400_000}}}]),
            "Rules[0].NonTalkTimeFilter.AbsoluteTimeRange.EndTime",
            "Rules[0].NonTalkTimeFilter.AbsoluteTimeRange.StartTime",
        ),
        (
            "an interruption segment that starts at the published floor and never ends",
            json!([{"InterruptionFilter": {
                "Threshold": 4_000,
                "ParticipantRole": "AGENT",
                "RelativeTimeRange": {"StartPercentage": 0}
            }}]),
            "Rules[0].InterruptionFilter.RelativeTimeRange.StartPercentage",
            "Rules[0].InterruptionFilter.RelativeTimeRange.EndPercentage",
        ),
        (
            "an interruption segment that ends and never starts, beside the open-ended Last",
            json!([{"InterruptionFilter": {
                "AbsoluteTimeRange": {"EndTime": 50_000, "Last": 20_000}
            }}]),
            "Rules[0].InterruptionFilter.AbsoluteTimeRange.EndTime",
            "Rules[0].InterruptionFilter.AbsoluteTimeRange.StartTime",
        ),
        (
            "a transcript segment that ends at the published ceiling and never starts",
            json!([{"TranscriptFilter": {
                "TranscriptFilterType": "EXACT",
                "Targets": ["speak to a manager"],
                "RelativeTimeRange": {"EndPercentage": 100}
            }}]),
            "Rules[0].TranscriptFilter.RelativeTimeRange.EndPercentage",
            "Rules[0].TranscriptFilter.RelativeTimeRange.StartPercentage",
        ),
        (
            "a sentiment rule whose half absolute segment sits beside a whole relative one",
            json!([{"SentimentFilter": {
                "Sentiments": ["MIXED"],
                "AbsoluteTimeRange": {"StartTime": 1_000},
                "RelativeTimeRange": {"StartPercentage": 0, "EndPercentage": 50}
            }}]),
            "Rules[0].SentimentFilter.AbsoluteTimeRange.StartTime",
            "Rules[0].SentimentFilter.AbsoluteTimeRange.EndTime",
        ),
        (
            "the last rule of a list at the modelled ceiling of twenty",
            twenty(json!({"SentimentFilter": {
                "Sentiments": ["POSITIVE"],
                "RelativeTimeRange": {"StartPercentage": 10, "First": 20}
            }})),
            "Rules[19].SentimentFilter.RelativeTimeRange.StartPercentage",
            "Rules[19].SentimentFilter.RelativeTimeRange.EndPercentage",
        ),
    ]
}

/// The rules a category under test is created with: whole, and what every refused update
/// must leave it reporting.
fn stored_rules() -> Value {
    json!([whole_rule()])
}

/// The `CategoryProperties` of a successful answer.
fn properties(reply: &crate::transcription_jobs::Reply, case: &str) -> Value {
    reply.ok(case)["CategoryProperties"].clone()
}

/// RULE 1: an update whose rules half-specify a time range is refused by name, and the
/// category keeps the rules it already had — the destructive overwrite never lands.
#[tokio::test]
async fn an_update_refusal_names_the_missing_companion_and_keeps_the_stored_rules() {
    let transcribe = Transcribe::new();

    for (index, (case, rules, member, companion)) in rows().into_iter().enumerate() {
        let name = format!("update-half-range-{index}");
        let created = transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": stored_rules()}),
            )
            .await;
        let before = properties(&created, case);

        let refused = transcribe
            .call(
                "UpdateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": rules}),
            )
            .await;
        refused.refused(case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
        let message = refused.error_message(case);
        assert_eq!(
            message,
            missing_companion_message(member, companion),
            "case {case:?}: the update must be refused naming the member the caller sent \
             and the companion it needs, got {message:?}"
        );

        // The refusal left the category exactly as it was — rules, CreateTime and
        // LastUpdateTime all unchanged.
        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        let found = properties(&read, case);
        assert_eq!(
            found, before,
            "case {case:?}: a refused update must leave the stored category as it was, \
             got {found:?}"
        );

        // The other side of the rule on the same category: a whole replacement is served
        // and becomes what the store holds.
        let replacement = json!([
            {"InterruptionFilter": {
                "Threshold": 2_000,
                "AbsoluteTimeRange": {"StartTime": 1_000, "EndTime": 2_000}
            }},
            whole_rule()
        ]);
        let updated = transcribe
            .call(
                "UpdateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": replacement}),
            )
            .await;
        updated.ok_status(case, 200);
        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        let found = properties(&read, case)["Rules"].clone();
        assert_eq!(
            found,
            as_recorded(&replacement),
            "case {case:?}: an accepted update must replace the stored rules, got \
             {found:?}"
        );
    }
}

/// RULE 2: a half-specified replacement set is never stored, whatever ELSE the request
/// gets wrong — a category this account holds no record of, or an `InputType`
/// contradicting the stored one. Which of the two refusals such a request earns first is
/// not documented, so either is accepted; what is asserted is the status, the modelled
/// error, the wording of whichever refusal came back, and that nothing was stored.
#[tokio::test]
async fn a_half_specified_update_is_refused_whatever_else_the_request_gets_wrong() {
    let transcribe = Transcribe::new();

    let malformed = json!([{"InterruptionFilter": {"AbsoluteTimeRange": {"EndTime": 50_000}}}]);
    let companion = missing_companion_message(
        "Rules[0].InterruptionFilter.AbsoluteTimeRange.EndTime",
        "Rules[0].InterruptionFilter.AbsoluteTimeRange.StartTime",
    );

    // A category that was never created: refused, and the refusal created nothing.
    let case = "a half-specified update of a category this account does not hold";
    let absent = not_found_message();
    let refused = transcribe
        .call(
            "UpdateCallAnalyticsCategory",
            json!({"CategoryName": "never-created", "Rules": malformed}),
        )
        .await;
    refused.refused(case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
    let message = refused.error_message(case);
    assert!(
        message == companion || message == absent,
        "case {case:?}: expected one of the two refusals this request earns \
         ({companion:?} or {absent:?}), got {message:?}"
    );
    transcribe
        .call(
            "GetCallAnalyticsCategory",
            json!({"CategoryName": "never-created"}),
        )
        .await
        .refused(case, CLIENT_ERROR_STATUS, REFUSAL_CODE);

    // Both directions of the modelled `InputType` (POST_CALL | REAL_TIME), each
    // contradicted by an update that also half-specifies a range.
    for (stored_type, other_type) in [("POST_CALL", "REAL_TIME"), ("REAL_TIME", "POST_CALL")] {
        let case = format!("a category stored {stored_type}, updated as {other_type}");
        let case = case.as_str();
        let name = format!("input-type-{stored_type}");
        let created = transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({
                    "CategoryName": name,
                    "Rules": stored_rules(),
                    "InputType": stored_type
                }),
            )
            .await;
        let before = properties(&created, case);

        let refused = transcribe
            .call(
                "UpdateCallAnalyticsCategory",
                json!({
                    "CategoryName": name,
                    "InputType": other_type,
                    "Rules": malformed
                }),
            )
            .await;
        refused.refused(case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
        let message = refused.error_message(case);
        let mismatch = input_type_message(other_type, stored_type);
        assert!(
            message == companion || message == mismatch,
            "case {case:?}: expected one of the two refusals this request earns \
             ({companion:?} or {mismatch:?}), got {message:?}"
        );

        let read = transcribe
            .call("GetCallAnalyticsCategory", json!({"CategoryName": name}))
            .await;
        let found = properties(&read, case);
        assert_eq!(
            found, before,
            "case {case:?}: a refused update must leave the stored category as it was, \
             got {found:?}"
        );
    }
}
