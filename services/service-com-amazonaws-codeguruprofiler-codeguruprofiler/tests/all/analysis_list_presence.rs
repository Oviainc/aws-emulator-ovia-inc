// SPDX-License-Identifier: CC0-1.0

//! ONE rule about `GetRecommendations`' two `@required` analysis lists, asserted on both
//! sides of it: `recommendations` and `anomalies` are written to the response body exactly
//! when they hold a finding.
//!
//! The rule has a single implementation — the `req_unless_empty` presence declared for
//! those two members of `GetRecommendationsResponse` — and this suite drives it through
//! both of its branches:
//!
//! 1. end to end over the router this service mounts in production: the handler fills the
//!    two lists empty (no anti-pattern matcher and no anomaly detector runs over a stored
//!    agent profile, so there is nothing to report and nothing is invented), so a 200
//!    carries the response's other three `@required` members and NOTHING else — the whole
//!    key set, which is what distinguishes these rows from the omission cases
//!    `wire_response_shape.rs` and `recommendations_analysis.rs` already drive. The rows
//!    vary what the rule generalises over: whether the group retains a profile, where the
//!    window closes relative to a retained receipt time, and the locale asked for;
//! 2. on the response codec itself, over a table of list LENGTHS — including the lengths
//!    no operation of this emulator can produce yet — so the rule is pinned as "written
//!    when non-empty", not as "these two keys are never written".
//!
//! Member names, their `@required` marks and the members of `Recommendation`/`Anomaly`
//! come from
//! `smithy_query(query="shape", shape="GetRecommendationsResponse,Recommendation,Anomaly,Metric,AnomalyInstance,Pattern,Match")`.
//!
//! The emulator, the request sender and the receipt-time placement are the test target's
//! shared ones — [`crate::profiling_group_lifecycle::Harness`] and
//! [`crate::profile_times::place_receipts`] — so this suite adds no second harness.

use aws_smithy_schema::serde::ShapeSerializer;
use aws_smithy_schema_server::json::JsonShapeSerializer;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::ops::types::{
    Anomaly, AnomalyInstance, GetRecommendationsOutput, Match, Metric, Pattern, Recommendation,
    GET_RECOMMENDATIONS_OUTPUT_SCHEMA,
};

use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::{Answer, Harness};

/// The two analysis lists, as the wire spells them.
const ANALYSIS_MEMBERS: &[&str] = &["recommendations", "anomalies"];

/// The response members that are on the body whatever the analysis found.
const ALWAYS_REPORTED_MEMBERS: &[&str] =
    &["profilingGroupName", "profileStartTime", "profileEndTime"];

/// Create the profiling group `name` through the shared harness.
async fn create(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({"profilingGroupName": name}))
                .expect("a serializable body"),
        )
        .await
}

/// `GetRecommendations` over `[start, end]` epoch seconds, optionally in `locale`.
async fn get_recommendations(
    harness: &Harness,
    name: &str,
    start: i64,
    end: i64,
    locale: Option<&str>,
) -> Answer {
    let mut query = format!("?startTime={}&endTime={}", iso8601(start), iso8601(end));
    if let Some(locale) = locale {
        query.push_str(&format!("&locale={locale}"));
    }
    harness
        .send(
            "GET",
            &format!("/internal/profilingGroups/{name}/recommendations{query}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// An epoch-second instant in the ISO 8601 form the modelled `date-time` query members
/// carry.
fn iso8601(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .expect("a renderable instant")
}

/// One `Recommendation`, every `@required` member filled, distinguishable by `index`.
/// Used only to give the codec a non-empty list; no operation of this emulator produces
/// one, because nothing here analyses a profile.
fn a_recommendation(index: usize) -> Recommendation {
    Recommendation {
        all_matches_count: index as i32,
        all_matches_sum: index as f64,
        pattern: Pattern {
            id: Some(format!("pattern-{index}")),
            name: Some(format!("Pattern {index}")),
            threshold_percent: 10.0,
            ..Pattern::default()
        },
        top_matches: vec![Match {
            target_frames_index: Some(index as i32),
            frame_address: Some(format!("frame-{index}")),
            threshold_breach_value: Some(1.5),
        }],
        start_time: DateTime::from_secs(1_590_969_600),
        end_time: DateTime::from_secs(1_590_973_200),
    }
}

/// One `Anomaly`, every `@required` member filled, distinguishable by `index`. Same
/// purpose as [`a_recommendation`].
fn an_anomaly(index: usize) -> Anomaly {
    Anomaly {
        metric: Metric {
            frame_name: format!("frame-{index}"),
            // The one `MetricType` the model's shape doc documents.
            r#type: "AggregatedRelativeTotalTime".to_string(),
            thread_states: vec!["RUNNABLE".to_string()],
        },
        reason: format!("reason-{index}"),
        instances: vec![AnomalyInstance {
            id: format!("instance-{index}"),
            start_time: DateTime::from_secs(1_590_969_600),
            end_time: None,
            user_feedback: None,
        }],
    }
}

/// `value` through the same JSON codec this service's restJson1 bodies are written with,
/// as parsed JSON.
fn serialize(case: &str, value: &GetRecommendationsOutput) -> Value {
    // restJson1 honours `@jsonName`, so the codec is configured the way the protocol
    // configures it — no member of this shape declares one, so the keys are the model's.
    let mut serializer = JsonShapeSerializer::new_with_config(true);
    serializer
        .write_struct(GET_RECOMMENDATIONS_OUTPUT_SCHEMA, value)
        .unwrap_or_else(|error| panic!("case {case:?}: the response must serialize: {error}"));
    let text = serializer.into_string();
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("case {case:?}: the codec wrote invalid JSON ({error}): {text}")
    })
}

#[tokio::test]
async fn a_served_analysis_with_nothing_to_report_carries_neither_analysis_list() {
    let harness = Harness::new();
    let bare = "unprofiled-group";
    let profiled = "profiled-group";
    for name in [bare, profiled] {
        assert_eq!(
            create(&harness, name).await.status,
            201,
            "creating {name} must answer 201"
        );
    }
    assert_eq!(
        harness
            .post_agent_profile(
                profiled,
                "application/json",
                None,
                br#"{"start":1600000000000,"frames":["A"]}"#.to_vec()
            )
            .await
            .status,
        204,
        "PostAgentProfile must store the profile the analysis would be about"
    );
    let received_at = 1_590_971_400_i64;
    place_receipts(&harness, profiled, &[received_at]);

    // Rows: (case, group, window start, window end, locale). They differ in where the
    // window closes relative to the retained receipt time, in whether the group retains a
    // profile at all, and in the locale the analysis is asked for.
    let cases: Vec<(&str, &str, i64, i64, Option<&str>)> = vec![
        (
            "a window closed exactly on the retained receipt time",
            profiled,
            received_at,
            received_at + 1,
            None,
        ),
        (
            "a sub-hour window just past the retained profile",
            profiled,
            received_at + 1,
            received_at + 1_800,
            None,
        ),
        (
            "an analysis asked for in a documented locale",
            profiled,
            received_at - 600,
            received_at + 600,
            Some("ja-JP"),
        ),
        (
            "an analysis of an unprofiled group in another documented locale",
            bare,
            1_590_969_600,
            1_590_973_200,
            Some("pt-BR"),
        ),
        (
            "an analysis asked for in a documented locale in another casing",
            bare,
            1_590_969_600,
            1_590_969_601,
            Some("EN-GB"),
        ),
    ];

    for (case, name, start, end, locale) in cases {
        let answer = get_recommendations(&harness, name, start, end, locale).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        // The whole key SET, not just the two keys' absence: nothing else of the modelled
        // response is left out either, and nothing the model does not declare is added.
        let mut written: Vec<&str> = body
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {body}"))
            .keys()
            .map(String::as_str)
            .collect();
        written.sort_unstable();
        let mut expected: Vec<&str> = ALWAYS_REPORTED_MEMBERS.to_vec();
        expected.sort_unstable();
        assert_eq!(
            written, expected,
            "case {case:?}: expected exactly {expected:?} on the body while the analysis \
             holds no finding, got {written:?} ({body})"
        );
        for member in ANALYSIS_MEMBERS {
            assert_eq!(
                body.get(member),
                None,
                "case {case:?}: expected {member} absent while the analysis holds no \
                 finding, got {body}"
            );
        }
    }
}

#[test]
fn each_analysis_list_is_written_exactly_when_it_holds_a_finding() {
    // Rows: (case, how many recommendations, how many anomalies). The rule is about each
    // list's own emptiness, so the rows cover every combination of empty and non-empty
    // and more than one entry per list.
    let cases: Vec<(&str, usize, usize)> = vec![
        ("an analysis with nothing to report", 0, 0),
        ("one recommendation and no anomaly", 1, 0),
        ("no recommendation and one anomaly", 0, 1),
        ("findings of both kinds", 2, 3),
        ("several recommendations, no anomaly", 4, 0),
    ];

    for (case, recommendation_count, anomaly_count) in cases {
        let output = GetRecommendationsOutput {
            profiling_group_name: "codec-group".to_string(),
            profile_start_time: DateTime::from_secs(1_590_969_600),
            profile_end_time: DateTime::from_secs(1_590_973_200),
            recommendations: (0..recommendation_count).map(a_recommendation).collect(),
            anomalies: (0..anomaly_count).map(an_anomaly).collect(),
        };
        let body = serialize(case, &output);

        for (member, count) in [
            ("recommendations", recommendation_count),
            ("anomalies", anomaly_count),
        ] {
            let written = body.get(member);
            if count == 0 {
                assert_eq!(
                    written, None,
                    "case {case:?}: expected {member} absent while it holds no finding, \
                     got {body}"
                );
            } else {
                let entries = written
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!(
                        "case {case:?}: expected {member} present as an array of its \
                         {count} findings, got {body}"
                    ));
                assert_eq!(
                    entries.len(),
                    count,
                    "case {case:?}: expected {count} {member}, got {body}"
                );
            }
        }

        for member in ALWAYS_REPORTED_MEMBERS {
            assert!(
                body.get(member).is_some(),
                "case {case:?}: expected {member} present whatever the analysis found, \
                 got {body}"
            );
        }
    }
}
