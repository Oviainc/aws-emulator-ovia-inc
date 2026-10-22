// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for DevOps Guru insight feedback: `PutFeedback` (`PUT /feedback`)
//! and the `DescribeFeedback` (`POST /feedback`) read that reports what it stored.
//!
//! The RULE these tests pin: feedback submitted about an insight is stored under the
//! insight id the request names, for any id and any `InsightFeedbackOption` value the
//! model allows and WITHOUT the insight having to exist, one record per insight with
//! the latest submission replacing the previous one; `DescribeFeedback` reports that
//! record for the id it is asked about, or the most recently submitted record when it
//! is asked about none, and reports nothing for an insight no feedback was submitted
//! about. A submission that names no insight, or no option the model declares, is
//! refused and stores nothing.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same router the emulator boots — using the harness the sibling
//! `configuration_lifecycle` suite owns, so the assertions are about wire behaviour.
//!
//! No insight is ever created here, because nothing in DevOps Guru's API creates one:
//! insights come from its own analysis. Every insight id below therefore names an
//! insight this account has never seen, which is exactly the case the issue reports.

use serde_json::{json, Value};

use crate::configuration_lifecycle::{Emulator, Reply};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::shared_types::INSIGHT_FEEDBACK_OPTION_VALUES;
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::MAX_FEEDBACK_INSIGHTS;

/// Insight id forms a caller can submit feedback about, spanning what `InsightId`
/// allows: `smithy_query(query="shape", shape="InsightId")` gives length 1..100 and
/// pattern `^[\w-]*$`, so the forms differ in length (both published bounds) and in
/// which characters of that class they use.
fn insight_id_forms() -> Vec<String> {
    vec![
        // The published minimum length.
        "a".to_string(),
        // Digits only.
        "7".to_string(),
        // The hyphenated, opaque form DevOps Guru's own insight ids take.
        "AJDA_3qHDLhPL0EgXkRYrKJgjJNM6LJm6xMH2QdBsEM".to_string(),
        // Underscores and upper case.
        "INSIGHT_ID_WITH_UNDERSCORES".to_string(),
        // A UUID-shaped id: hyphens between hex groups.
        "4e1d2d6b-0f2f-4b1e-9f2b-93c9a16c0a11".to_string(),
        // The published maximum length.
        "x".repeat(100),
    ]
}

/// Submit feedback with whatever request body the case names.
async fn put_feedback(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("PUT", "/feedback", Some(body)).await
}

/// Submit well-formed feedback about `insight_id`.
async fn put_option(emulator: &Emulator, insight_id: &str, feedback: &str) -> Reply {
    put_feedback(
        emulator,
        &json!({ "InsightFeedback": { "Id": insight_id, "Feedback": feedback } }),
    )
    .await
}

/// Read the feedback back: `Some(id)` asks about one insight, `None` asks for the most
/// recent feedback submitted in the account and Region.
async fn describe_feedback(emulator: &Emulator, insight_id: Option<&str>) -> Reply {
    let body = match insight_id {
        Some(insight_id) => json!({ "InsightId": insight_id }),
        None => json!({}),
    };
    emulator.call("POST", "/feedback", Some(&body)).await
}

/// The `(Id, Feedback)` pair a `DescribeFeedback` reply reports, or `None` when it
/// reports no feedback at all.
fn reported(reply: &Reply) -> Option<(String, String)> {
    let feedback = reply.at("/InsightFeedback")?;
    let id = feedback.get("Id")?.as_str()?.to_string();
    let option = feedback.get("Feedback")?.as_str()?.to_string();
    Some((id, option))
}

/// `PutFeedbackResponse` models no members, so an accepted submission answers 200 with
/// an empty document and nothing else.
fn assert_accepted(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected PutFeedback to be accepted with 200, got {} ({})",
        reply.status, reply.raw
    );
    let empty = match &reply.body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case}: PutFeedbackResponse models no members, so the body must be empty, got {}",
        reply.raw
    );
}

#[tokio::test]
async fn feedback_round_trips_for_every_option_and_insight_id_form() {
    // One emulator per row, so each round trip is judged on its own store.
    for insight_id in insight_id_forms() {
        for feedback in INSIGHT_FEEDBACK_OPTION_VALUES {
            let case = format!("id {insight_id:?} / feedback {feedback:?}");
            let emulator = Emulator::new();

            let put = put_option(&emulator, &insight_id, feedback).await;
            assert_accepted(&put, &case);

            // Read back through the implemented reader: the insight the feedback is
            // about reports exactly the option that was submitted about it.
            let by_id = describe_feedback(&emulator, Some(&insight_id)).await;
            assert_eq!(
                by_id.status, 200,
                "case {case}: expected DescribeFeedback 200, got {} ({})",
                by_id.status, by_id.raw
            );
            assert_eq!(
                reported(&by_id),
                Some((insight_id.clone(), (*feedback).to_string())),
                "case {case}: expected the submitted feedback back, got {}",
                by_id.raw
            );

            // With no InsightId the operation reports the most recent feedback
            // submitted in the account, which is the one submission made here.
            let latest = describe_feedback(&emulator, None).await;
            assert_eq!(
                reported(&latest),
                Some((insight_id.clone(), (*feedback).to_string())),
                "case {case}: expected the only submission to be the most recent one, got {}",
                latest.raw
            );
        }
    }
}

#[tokio::test]
async fn resubmitted_feedback_replaces_that_insights_record() {
    // Every ordered pair of distinct options, over two id forms: whatever was stored
    // first, the second submission is what the insight reports afterwards.
    for insight_id in ["insight-revised", &"z".repeat(100)] {
        for (index, first) in INSIGHT_FEEDBACK_OPTION_VALUES.iter().enumerate() {
            let second = INSIGHT_FEEDBACK_OPTION_VALUES
                [(index + 1) % INSIGHT_FEEDBACK_OPTION_VALUES.len()];
            let case = format!("id {insight_id:?} / {first:?} then {second:?}");
            let emulator = Emulator::new();

            assert_accepted(&put_option(&emulator, insight_id, first).await, &case);
            assert_accepted(&put_option(&emulator, insight_id, second).await, &case);

            let by_id = describe_feedback(&emulator, Some(insight_id)).await;
            assert_eq!(
                reported(&by_id),
                Some((insight_id.to_string(), second.to_string())),
                "case {case}: the later submission must replace the earlier one, got {}",
                by_id.raw
            );
        }
    }
}

#[tokio::test]
async fn describe_without_an_insight_id_reports_the_most_recent_submission() {
    let emulator = Emulator::new();
    // Submissions about several insights, interleaved so the most recent is neither
    // the first stored nor the lowest id: the answer tracks submission order, not the
    // key order of the store.
    let submissions = [
        ("insight-aaa", "VALID_COLLECTION"),
        ("insight-zzz", "DATA_NOISY_ANOMALY"),
        ("insight-mmm", "ALERT_TOO_SENSITIVE"),
        ("insight-aaa", "RECOMMENDATION_USEFUL"),
        ("insight-zzz", "DATA_INCORRECT"),
    ];

    for (step, (insight_id, feedback)) in submissions.iter().enumerate() {
        let case = format!("step {step}: {insight_id} / {feedback}");
        assert_accepted(&put_option(&emulator, insight_id, feedback).await, &case);

        let latest = describe_feedback(&emulator, None).await;
        assert_eq!(
            reported(&latest),
            Some(((*insight_id).to_string(), (*feedback).to_string())),
            "case {case}: expected the submission just made to be the most recent, got {}",
            latest.raw
        );
    }

    // Each insight still reports its OWN latest feedback, not the account's.
    for (insight_id, expected) in [
        ("insight-aaa", "RECOMMENDATION_USEFUL"),
        ("insight-zzz", "DATA_INCORRECT"),
        ("insight-mmm", "ALERT_TOO_SENSITIVE"),
    ] {
        let by_id = describe_feedback(&emulator, Some(insight_id)).await;
        assert_eq!(
            reported(&by_id),
            Some((insight_id.to_string(), expected.to_string())),
            "case {insight_id}: expected that insight's own latest feedback, got {}",
            by_id.raw
        );
    }
}

#[tokio::test]
async fn an_insight_no_feedback_was_submitted_about_reports_none() {
    let emulator = Emulator::new();

    // Nothing submitted yet: neither read reports feedback, and neither fails.
    let empty_latest = describe_feedback(&emulator, None).await;
    assert_eq!(
        empty_latest.status, 200,
        "case empty store / no InsightId: expected 200, got {} ({})",
        empty_latest.status, empty_latest.raw
    );
    assert_eq!(
        reported(&empty_latest),
        None,
        "case empty store / no InsightId: expected no feedback reported, got {}",
        empty_latest.raw
    );

    for insight_id in insight_id_forms() {
        let reply = describe_feedback(&emulator, Some(&insight_id)).await;
        assert_eq!(
            reply.status, 200,
            "case empty store / id {insight_id:?}: expected 200, got {} ({})",
            reply.status, reply.raw
        );
        assert_eq!(
            reported(&reply),
            None,
            "case empty store / id {insight_id:?}: expected no feedback reported, got {}",
            reply.raw
        );
    }

    // One insight has feedback; the others still report none, so an absent answer is
    // about that insight rather than about the store being empty.
    let known = "insight-rated";
    assert_accepted(
        &put_option(&emulator, known, "VALID_COLLECTION").await,
        "seed",
    );
    for insight_id in insight_id_forms() {
        let reply = describe_feedback(&emulator, Some(&insight_id)).await;
        assert_eq!(
            reported(&reply),
            None,
            "case id {insight_id:?} after feedback about {known}: expected no feedback \
             reported for an insight nobody rated, got {}",
            reply.raw
        );
    }
    let rated = describe_feedback(&emulator, Some(known)).await;
    assert_eq!(
        reported(&rated),
        Some((known.to_string(), "VALID_COLLECTION".to_string())),
        "case id {known:?}: expected the rated insight's feedback, got {}",
        rated.raw
    );
}

/// Which refusal path a row expects, because the two word the member differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusedBy {
    /// This handler's own modelled `ValidationException`, which carries the member in
    /// `Fields[0].Name` — asserted exactly.
    Handler,
    /// The boundary validator, which refuses a value outside a declared constraint
    /// before the handler runs and names the member inside its message, in the model's
    /// lowerCamel wire casing.
    Boundary,
}

/// Whether a refusal names `member` as the thing that was wrong, the way `refused_by`
/// says that path words it.
fn names_member(reply: &Reply, member: &str, refused_by: RefusedBy) -> bool {
    match refused_by {
        RefusedBy::Handler => {
            reply.at("/Fields/0/Name").and_then(Value::as_str) == Some(member)
        }
        RefusedBy::Boundary => reply
            .raw
            .to_ascii_lowercase()
            .contains(&member.to_ascii_lowercase()),
    }
}

#[tokio::test]
async fn a_submission_naming_no_insight_or_no_declared_option_is_refused() {
    // Each row: the request body, the member the refusal has to name, and which path
    // refuses it.
    let cases: Vec<(Value, &str, RefusedBy)> = vec![
        // No feedback at all.
        (json!({}), "InsightFeedback", RefusedBy::Handler),
        // Feedback about nothing.
        (
            json!({ "InsightFeedback": {} }),
            "InsightFeedback.Id",
            RefusedBy::Handler,
        ),
        (
            json!({ "InsightFeedback": { "Feedback": "VALID_COLLECTION" } }),
            "InsightFeedback.Id",
            RefusedBy::Handler,
        ),
        // An insight, but no opinion about it.
        (
            json!({ "InsightFeedback": { "Id": "insight-unrated" } }),
            "InsightFeedback.Feedback",
            RefusedBy::Handler,
        ),
        // Opinions outside `InsightFeedbackOption`: the right value in the wrong case,
        // a plausible invention, and an empty one. The model declares the enum, so the
        // boundary validator is what refuses these.
        (
            json!({ "InsightFeedback": { "Id": "insight-unrated", "Feedback": "valid_collection" } }),
            "InsightFeedback.Feedback",
            RefusedBy::Boundary,
        ),
        (
            json!({ "InsightFeedback": { "Id": "insight-unrated", "Feedback": "THUMBS_UP" } }),
            "InsightFeedback.Feedback",
            RefusedBy::Boundary,
        ),
        (
            json!({ "InsightFeedback": { "Id": "insight-unrated", "Feedback": "" } }),
            "InsightFeedback.Feedback",
            RefusedBy::Boundary,
        ),
    ];

    for (body, member, refused_by) in cases {
        let case = body.to_string();
        let emulator = Emulator::new();

        let reply = put_feedback(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected the submission to be refused with 400, got {} ({})",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case}: expected ValidationException, got {}",
            reply.raw
        );
        assert!(
            names_member(&reply, member, refused_by),
            "case {case}: expected the {refused_by:?} refusal to name {member}, got {}",
            reply.raw
        );

        // A refusal stores nothing: the account still reports no feedback.
        let latest = describe_feedback(&emulator, None).await;
        assert_eq!(
            reported(&latest),
            None,
            "case {case}: a refused submission must store nothing, got {}",
            latest.raw
        );
    }
}

#[tokio::test]
async fn an_insight_id_outside_the_models_bounds_is_refused_and_stores_nothing() {
    // `InsightId` is length 1..100, pattern `^[\w-]*$`, so each row breaks one of those
    // published bounds: below the minimum length, one past the maximum, and two
    // characters outside the pattern's class.
    let cases = [
        ("".to_string(), "below the minimum length"),
        ("y".repeat(101), "one past the maximum length"),
        ("insight id".to_string(), "a space, outside the pattern"),
        ("insight!".to_string(), "a bang, outside the pattern"),
    ];

    for (insight_id, why) in cases {
        let case = format!("id {insight_id:?} ({why})");
        let emulator = Emulator::new();

        let reply = put_option(&emulator, &insight_id, "VALID_COLLECTION").await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected a refusal with 400, got {} ({})",
            reply.status, reply.raw
        );

        let latest = describe_feedback(&emulator, None).await;
        assert_eq!(
            reported(&latest),
            None,
            "case {case}: a refused submission must store nothing, got {}",
            latest.raw
        );
    }
}

#[tokio::test]
async fn feedback_about_more_insights_than_the_store_holds_is_refused() {
    // The store bound is this emulator's own resource guard (see
    // `state::MAX_FEEDBACK_INSIGHTS`), not a published AWS quota: nothing withdraws
    // feedback, so the store needs a ceiling. The RULE it pins is that the ceiling
    // counts INSIGHTS, so a resubmission about an insight already in the store is
    // never refused, and a refusal leaves the store exactly as it was.
    let emulator = Emulator::new();
    for index in 0..MAX_FEEDBACK_INSIGHTS {
        let insight_id = format!("insight-{index}");
        let feedback = INSIGHT_FEEDBACK_OPTION_VALUES[index % INSIGHT_FEEDBACK_OPTION_VALUES.len()];
        assert_accepted(
            &put_option(&emulator, &insight_id, feedback).await,
            &format!("fill {index}"),
        );
    }

    // One insight too many.
    let overflow = put_option(&emulator, "insight-overflow", "VALID_COLLECTION").await;
    assert_eq!(
        overflow.status, 429,
        "case overflow: expected the modelled ThrottlingException (429) past the store \
         bound, got {} ({})",
        overflow.status, overflow.raw
    );
    assert!(
        overflow.names_error("ThrottlingException"),
        "case overflow: expected ThrottlingException, got {}",
        overflow.raw
    );
    assert!(
        overflow.at("/QuotaCode").is_some(),
        "case overflow: the refusal must name the quota it hit, got {}",
        overflow.raw
    );
    let refused = describe_feedback(&emulator, Some("insight-overflow")).await;
    assert_eq!(
        reported(&refused),
        None,
        "case overflow: a refused submission must store nothing, got {}",
        refused.raw
    );

    // Changing the feedback on an insight the store already holds is still accepted,
    // because it adds no entry.
    let revised = put_option(&emulator, "insight-0", "DATA_INCORRECT").await;
    assert_accepted(&revised, "revise at the bound");
    let by_id = describe_feedback(&emulator, Some("insight-0")).await;
    assert_eq!(
        reported(&by_id),
        Some(("insight-0".to_string(), "DATA_INCORRECT".to_string())),
        "case revise at the bound: expected the revised feedback, got {}",
        by_id.raw
    );
}

/// The seed the generated cases below are drawn from. Fixed, so a failure reproduces,
/// and named in every assertion message.
const PROPERTY_SEED: u64 = 0x5EED_FEED;

/// How many generated insight ids the property test walks. Bounded: this is a test, not
/// a fuzzer.
const PROPERTY_CASES: usize = 64;

/// The characters `InsightId`'s pattern `^[\w-]*$` admits.
const INSIGHT_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

/// The seeded generator the property test draws its cases from: SplitMix64, the mixing
/// function a generator crate's own `seed_from_u64` uses to expand a seed.
///
/// Written here rather than taken from a generator crate ON PURPOSE: a dev-dependency
/// changes this crate's `Cargo.toml` and the workspace `Cargo.lock`, the two files every
/// other fix in this repository also touches, and this fix has to land against a mainline
/// that moves between submits. Eight lines of arithmetic, nothing on disk, and the seed is
/// a constant every assertion message names.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` — every bound here is tiny, so the modulo bias cannot
    /// affect whether the invariant under test holds.
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

#[tokio::test]
async fn generated_feedback_round_trips_and_the_latest_submission_wins() {
    let mut rng = SplitMix64(PROPERTY_SEED);
    let emulator = Emulator::new();
    // What the store should report per insight, and the most recent submission overall.
    let mut expected: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut most_recent: Option<(String, String)>;

    for case in 0..PROPERTY_CASES {
        // A fresh id, or one already rated — so replacement is exercised as often as
        // first-time submission.
        let insight_id = if !expected.is_empty() && rng.below(2) == 0 {
            let keys: Vec<&String> = expected.keys().collect();
            keys[rng.below(keys.len())].clone()
        } else {
            // `InsightId` is length 1..100, so a generated id spans that whole range.
            let length = 1 + rng.below(100);
            (0..length)
                .map(|_| INSIGHT_ID_ALPHABET[rng.below(INSIGHT_ID_ALPHABET.len())] as char)
                .collect()
        };
        let feedback =
            INSIGHT_FEEDBACK_OPTION_VALUES[rng.below(INSIGHT_FEEDBACK_OPTION_VALUES.len())];
        let label = format!("seed {PROPERTY_SEED:#x} case {case}: id {insight_id:?} / {feedback:?}");

        assert_accepted(&put_option(&emulator, &insight_id, feedback).await, &label);
        expected.insert(insight_id.clone(), feedback.to_string());
        most_recent = Some((insight_id.clone(), feedback.to_string()));

        // INVARIANT 1: the insight just rated reports what was just submitted.
        let by_id = describe_feedback(&emulator, Some(&insight_id)).await;
        assert_eq!(
            reported(&by_id),
            Some((insight_id.clone(), feedback.to_string())),
            "{label}: expected the submitted feedback back, got {}",
            by_id.raw
        );

        // INVARIANT 2: the account's most recent feedback is the submission just made.
        let latest = describe_feedback(&emulator, None).await;
        assert_eq!(
            reported(&latest),
            most_recent,
            "{label}: expected the latest submission to be reported, got {}",
            latest.raw
        );
    }

    // INVARIANT 3: every insight rated along the way still reports its own latest
    // feedback — one record per insight, and no submission disturbed another's.
    for (insight_id, feedback) in &expected {
        let by_id = describe_feedback(&emulator, Some(insight_id)).await;
        assert_eq!(
            reported(&by_id),
            Some((insight_id.clone(), feedback.clone())),
            "seed {PROPERTY_SEED:#x}: expected {insight_id:?} to still report {feedback:?}, got {}",
            by_id.raw
        );
    }
}
