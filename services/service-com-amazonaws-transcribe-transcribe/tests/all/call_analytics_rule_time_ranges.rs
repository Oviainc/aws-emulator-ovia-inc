// SPDX-License-Identifier: CC0-1.0

//! The co-requirement the time-range shapes of a Call Analytics rule document, end to
//! end over the wire.
//!
//! THE RULE UNDER TEST: a time range bounds a custom segment with a PAIR of members
//! that are co-required in BOTH directions — "If you include StartTime in your request,
//! you must also include EndTime" and the converse, with the shape doc repeating "You
//! must use StartTime and EndTime as a set; that is, if you include one, you must
//! include both", and the same two sentences for StartPercentage / EndPercentage
//! (`smithy_query(query="shape", shape="AbsoluteTimeRange,RelativeTimeRange")`) — so a
//! request whose rules carry one end of a segment and not the other is refused with the
//! modelled `BadRequestException` (HTTP 400) and stores nothing, while a request whose
//! ranges are whole (or open-ended through `First` / `Last`, which "you can use
//! independently of each other") is accepted and reads its rules back unchanged.
//!
//! Smithy cannot express a conditional co-requirement, so this is the handler's own
//! refusal rather than boundary constraint validation — and it holds for every member of
//! the `Rule` union and for both operations that accept rules
//! (`CreateCallAnalyticsCategory` and `UpdateCallAnalyticsCategory`, which REPLACES the
//! stored rules).
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::call_analytics_rule_negate::as_recorded;
use crate::transcription_jobs::Transcribe;

/// The modelled client error a half-specified range is refused with, and the status
/// this service's awsJson1_1 wire protocol puts on every client error.
const REFUSAL_CODE: &str = "BadRequestException";
/// See [`REFUSAL_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// The published ceiling of `TimestampMilliseconds`, the target of every
/// `AbsoluteTimeRange` member (`@range(min: 0, max: 14400000)`).
const MAX_MILLISECONDS: i64 = 14_400_000;
/// The published ceiling of `Percentage`, the target of every `RelativeTimeRange`
/// member (`@range(min: 0, max: 100)`).
const MAX_PERCENTAGE: i64 = 100;
/// The published ceiling of `RuleList` (`@length(min: 1, max: 20)`).
const MAX_RULES: usize = 20;

/// How this service words "the member you included needs one you left out": the member
/// path the caller sent, and the companion the model documents it as co-required with.
///
/// Spelled out here rather than read from the handler's own formatter, so the expected
/// wording is this test's and not the code's; `pub(crate)` so the update-side suite
/// (`crate::call_analytics_category_update_rule_validation`) asserts the same one
/// string rather than a second copy of it.
pub(crate) fn missing_companion_message(member: &str, companion: &str) -> String {
    format!(
        "{companion} is required when {member} is specified. Correct your request and \
         try again."
    )
}

/// One member of the `Rule` union, with the members its filter models `@required` —
/// the whole union, from `smithy_query(query="shape", shape="Rule")`, so a rule the
/// refusal is not applied to fails here.
struct Filter {
    /// The union member name, which is also the rule's single JSON key.
    member: &'static str,
    /// The filter's required members, which every row carries whatever time range it
    /// adds.
    required: fn() -> Value,
}

fn filters() -> Vec<Filter> {
    vec![
        Filter {
            // `NonTalkTimeFilter` models no required member; `Threshold` is a
            // `TimestampMilliseconds`.
            member: "NonTalkTimeFilter",
            required: || json!({"Threshold": 30_000}),
        },
        Filter {
            // `InterruptionFilter` models no required member either; `ParticipantRole`
            // is the modelled enum `AGENT | CUSTOMER`.
            member: "InterruptionFilter",
            required: || json!({"Threshold": 4_000, "ParticipantRole": "AGENT"}),
        },
        Filter {
            // `TranscriptFilter$TranscriptFilterType` (enum `EXACT`) and `$Targets`
            // (`@length(min: 1)`) are both `@required`.
            member: "TranscriptFilter",
            required: || json!({"TranscriptFilterType": "EXACT", "Targets": ["speak to a manager"]}),
        },
        Filter {
            // `SentimentFilter$Sentiments` is `@required` and `@length(min: 1, max: 1)`.
            member: "SentimentFilter",
            required: || json!({"Sentiments": ["POSITIVE"]}),
        },
    ]
}

/// One rule: `filter`'s required members plus the `members` a row adds (a time range,
/// or nothing at all).
fn rule(filter: &Filter, members: Value) -> Value {
    let mut body = (filter.required)();
    let body_members = body.as_object_mut().expect("a filter is a JSON object");
    let added = match members {
        Value::Object(added) => added,
        other => panic!("a row's members must be a JSON object, got {other:?}"),
    };
    for (key, value) in added {
        body_members.insert(key, value);
    }
    json!({filter.member: body})
}

/// The time ranges that bound a segment with only ONE of its two co-required ends: one
/// row per direction of each pair, at each published bound of the member's own range and
/// at a value inside it, and rows where the half-specified pair sits beside the
/// open-ended `First` / `Last` members or beside the other shape's whole pair.
fn half_specified_ranges() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an absolute segment that starts and never ends",
            json!({"AbsoluteTimeRange": {"StartTime": 10_000}}),
        ),
        (
            "an absolute segment that ends and never starts",
            json!({"AbsoluteTimeRange": {"EndTime": 50_000}}),
        ),
        (
            "an absolute start at the published floor, no end",
            json!({"AbsoluteTimeRange": {"StartTime": 0}}),
        ),
        (
            "an absolute end at the published ceiling, no start",
            json!({"AbsoluteTimeRange": {"EndTime": MAX_MILLISECONDS}}),
        ),
        (
            "an absolute start beside the open-ended Last",
            json!({"AbsoluteTimeRange": {"StartTime": 10_000, "Last": 20_000}}),
        ),
        (
            "an absolute end beside the open-ended First",
            json!({"AbsoluteTimeRange": {"EndTime": 10_000, "First": 20_000}}),
        ),
        (
            "a relative segment that starts and never ends",
            json!({"RelativeTimeRange": {"StartPercentage": 10}}),
        ),
        (
            "a relative segment that ends and never starts",
            json!({"RelativeTimeRange": {"EndPercentage": 50}}),
        ),
        (
            "a relative start at the published floor, no end",
            json!({"RelativeTimeRange": {"StartPercentage": 0}}),
        ),
        (
            "a relative end at the published ceiling, no start",
            json!({"RelativeTimeRange": {"EndPercentage": MAX_PERCENTAGE}}),
        ),
        (
            "a relative start beside the open-ended First",
            json!({"RelativeTimeRange": {"StartPercentage": 10, "First": 20}}),
        ),
        (
            "a half relative segment beside a whole absolute one",
            json!({
                "AbsoluteTimeRange": {"StartTime": 0, "EndTime": 1_000},
                "RelativeTimeRange": {"EndPercentage": 50}
            }),
        ),
        (
            "a half absolute segment beside a whole relative one",
            json!({
                "AbsoluteTimeRange": {"StartTime": 1_000},
                "RelativeTimeRange": {"StartPercentage": 0, "EndPercentage": 50}
            }),
        ),
    ]
}

/// The time ranges whose pairs hold together: both ends given (at the published bounds
/// and inside them), the open-ended members alone or together, both shapes whole at
/// once, and no time range at all.
fn whole_ranges() -> Vec<(&'static str, Value)> {
    vec![
        ("no time range at all", json!({})),
        (
            "a bounded absolute segment",
            json!({"AbsoluteTimeRange": {"StartTime": 10_000, "EndTime": 50_000}}),
        ),
        (
            "an absolute segment spanning both published bounds",
            json!({"AbsoluteTimeRange": {"StartTime": 0, "EndTime": MAX_MILLISECONDS}}),
        ),
        (
            "the open-ended First alone",
            json!({"AbsoluteTimeRange": {"First": 60_000}}),
        ),
        (
            "the open-ended Last alone",
            json!({"AbsoluteTimeRange": {"Last": 60_000}}),
        ),
        (
            "First and Last together, used independently of each other",
            json!({"AbsoluteTimeRange": {"First": 10_000, "Last": 20_000}}),
        ),
        (
            "a bounded relative segment",
            json!({"RelativeTimeRange": {"StartPercentage": 10, "EndPercentage": 50}}),
        ),
        (
            "a relative segment spanning both published bounds",
            json!({"RelativeTimeRange": {"StartPercentage": 0, "EndPercentage": MAX_PERCENTAGE}}),
        ),
        (
            "the open-ended relative First alone",
            json!({"RelativeTimeRange": {"First": 30}}),
        ),
        (
            "both shapes whole at once",
            json!({
                "AbsoluteTimeRange": {"StartTime": 0, "EndTime": 1_000},
                "RelativeTimeRange": {"StartPercentage": 0, "EndPercentage": 50}
            }),
        ),
    ]
}

/// A whole rule every list in this suite pads itself with, so a row's refusal cannot be
/// explained by the rest of the list.
///
/// `pub(crate)` for the update-side suite
/// (`crate::call_analytics_category_update_rule_validation`), which stocks its categories
/// with the same whole rule.
pub(crate) fn whole_rule() -> Value {
    json!({
        "SentimentFilter": {
            "Sentiments": ["NEGATIVE"],
            "AbsoluteTimeRange": {"StartTime": 0, "EndTime": 30_000}
        }
    })
}

/// The rule lists a half-specified range is sent in: alone, first of two, and last of
/// two — so the refusal does not depend on where in the list the malformed rule sits.
fn placements(malformed: &Value) -> Vec<(&'static str, Vec<Value>)> {
    vec![
        ("the only rule", vec![malformed.clone()]),
        (
            "the first of two rules",
            vec![malformed.clone(), whole_rule()],
        ),
        (
            "the last of two rules",
            vec![whole_rule(), malformed.clone()],
        ),
        ("the last of the modelled maximum of twenty", {
            let mut rules: Vec<Value> = (0..MAX_RULES - 1).map(|_| whole_rule()).collect();
            rules.push(malformed.clone());
            rules
        }),
    ]
}

/// RULE: a create whose rules carry one end of a segment and not the other is refused,
/// and no category is created — the name it asked for is unknown to the read afterwards.
#[tokio::test]
async fn a_create_with_a_half_specified_rule_time_range_is_refused() {
    let transcribe = Transcribe::new();
    let mut index = 0;

    for filter in filters() {
        for (range_case, range) in half_specified_ranges() {
            let malformed = rule(&filter, range);
            for (placement, rules) in placements(&malformed) {
                let case = format!("{} / {range_case} / {placement}", filter.member);
                let name = format!("half-range-{index}");
                index += 1;

                transcribe
                    .call(
                        "CreateCallAnalyticsCategory",
                        json!({"CategoryName": name, "Rules": rules}),
                    )
                    .await
                    .refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);

                // A refused create stores nothing, so the name it asked for is unknown.
                transcribe
                    .call(
                        "GetCallAnalyticsCategory",
                        json!({"CategoryName": name}),
                    )
                    .await
                    .refused(&case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
            }
        }
    }
}

/// RULE, the other side: a create whose ranges all hold together is accepted, and the
/// read reports exactly the rules it was created with.
#[tokio::test]
async fn a_create_whose_rule_time_ranges_are_whole_is_accepted_and_read_back() {
    let transcribe = Transcribe::new();
    let mut index = 0;

    for filter in filters() {
        for (range_case, range) in whole_ranges() {
            let case = format!("{} / {range_case}", filter.member);
            let name = format!("whole-range-{index}");
            index += 1;
            let rules = vec![rule(&filter, range), whole_rule()];

            transcribe
                .call(
                    "CreateCallAnalyticsCategory",
                    json!({"CategoryName": name, "Rules": rules}),
                )
                .await
                .ok(&case);

            let read = transcribe
                .call(
                    "GetCallAnalyticsCategory",
                    json!({"CategoryName": name}),
                )
                .await;
            let found = read.ok(&case)["CategoryProperties"]["Rules"].clone();
            assert_eq!(
                found,
                as_recorded(&Value::Array(rules)),
                "case {case:?}: the category must report the rules it was created with, \
                 as it records them, got {found:?}"
            );
        }
    }
}

/// RULE, on the operation that REPLACES a category's rules: an update carrying a
/// half-specified range is refused and the stored rules are left exactly as they were,
/// while an update whose ranges are whole replaces them.
#[tokio::test]
async fn an_update_with_a_half_specified_rule_time_range_is_refused_and_changes_nothing() {
    let transcribe = Transcribe::new();
    let mut index = 0;

    for filter in filters() {
        for (range_case, range) in half_specified_ranges() {
            let case = format!("{} / {range_case}", filter.member);
            let name = format!("update-range-{index}");
            index += 1;

            // The category starts out with a whole range, which every refused update
            // below must leave untouched.
            let stored = vec![rule(&filter, json!({"AbsoluteTimeRange": {"First": 5_000}}))];
            let created = transcribe
                .call(
                    "CreateCallAnalyticsCategory",
                    json!({"CategoryName": name, "Rules": stored}),
                )
                .await;
            let before = created.ok(&case)["CategoryProperties"].clone();

            let malformed = rule(&filter, range);
            for (placement, rules) in placements(&malformed) {
                let placed = format!("{case} / {placement}");
                transcribe
                    .call(
                        "UpdateCallAnalyticsCategory",
                        json!({"CategoryName": name, "Rules": rules}),
                    )
                    .await
                    .refused(&placed, CLIENT_ERROR_STATUS, REFUSAL_CODE);

                let read = transcribe
                    .call(
                        "GetCallAnalyticsCategory",
                        json!({"CategoryName": name}),
                    )
                    .await;
                let found = read.ok(&placed)["CategoryProperties"].clone();
                assert_eq!(
                    found, before,
                    "case {placed:?}: a refused update must leave the stored category as \
                     it was, got {found:?}"
                );
            }

            // The same rules with the pair made whole are accepted and replace the
            // stored ones.
            let replacement = vec![rule(
                &filter,
                json!({"AbsoluteTimeRange": {"StartTime": 1_000, "EndTime": 2_000}}),
            )];
            let updated = transcribe
                .call(
                    "UpdateCallAnalyticsCategory",
                    json!({"CategoryName": name, "Rules": replacement}),
                )
                .await;
            let found = updated.ok(&case)["CategoryProperties"]["Rules"].clone();
            assert_eq!(
                found,
                as_recorded(&Value::Array(replacement)),
                "case {case:?}: an accepted update must replace the stored rules, got \
                 {found:?}"
            );
        }
    }
}

/// The refusal names the member PATH the caller sent and the companion it needs — a
/// validation message built from the request, carrying no other request value.
#[tokio::test]
async fn the_refusal_names_the_missing_companion_member() {
    let transcribe = Transcribe::new();

    // (the rules sent, the member path the refusal must name, the companion it needs)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "an interruption rule whose absolute segment never ends",
            json!([{"InterruptionFilter": {"AbsoluteTimeRange": {"StartTime": 10_000}}}]),
            "Rules[0].InterruptionFilter.AbsoluteTimeRange.StartTime",
            "Rules[0].InterruptionFilter.AbsoluteTimeRange.EndTime",
        ),
        (
            "a sentiment rule whose relative segment never starts",
            json!([
                {"SentimentFilter": {
                    "Sentiments": ["POSITIVE"],
                    "RelativeTimeRange": {"EndPercentage": 50}
                }}
            ]),
            "Rules[0].SentimentFilter.RelativeTimeRange.EndPercentage",
            "Rules[0].SentimentFilter.RelativeTimeRange.StartPercentage",
        ),
        (
            "the SECOND rule's relative segment never ends",
            json!([
                whole_rule(),
                {"NonTalkTimeFilter": {"RelativeTimeRange": {"StartPercentage": 10}}}
            ]),
            "Rules[1].NonTalkTimeFilter.RelativeTimeRange.StartPercentage",
            "Rules[1].NonTalkTimeFilter.RelativeTimeRange.EndPercentage",
        ),
    ];

    for (index, (case, rules, member, companion)) in rows.iter().enumerate() {
        let name = format!("named-member-{index}");
        let refused = transcribe
            .call(
                "CreateCallAnalyticsCategory",
                json!({"CategoryName": name, "Rules": rules}),
            )
            .await;
        refused.refused(case, CLIENT_ERROR_STATUS, REFUSAL_CODE);
        let message = refused.error_message(case);
        assert_eq!(
            message,
            missing_companion_message(member, companion),
            "case {case:?}: the refusal must name the member the caller sent and the \
             companion it needs, got {message:?}"
        );
    }
}
