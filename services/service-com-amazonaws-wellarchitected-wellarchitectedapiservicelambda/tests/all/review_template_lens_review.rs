// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateReviewTemplateLensReview` and
//! `GetReviewTemplateLensReview`: the notes of ONE lens review of ONE review template,
//! written and read back.
//!
//! The rules these pin, over TABLES of inputs rather than one recorded request:
//!
//! * an update REPLACES exactly the note members the request supplies and leaves every
//!   member it omits as stored, and the lens review a read reports is the one the last
//!   update wrote — by either spelling of the lens (`LensAlias` is "either the lens alias
//!   … or the lens ARN");
//! * a `PillarNotes` key must name a pillar the lens under review is organized into
//!   WHERE this emulator holds that lens's pillar set; one that does not is a modelled
//!   400 `ValidationException` naming that member, and nothing is stored. For a lens it
//!   holds no content for there is no pillar set to judge a key against, so the notes
//!   are stored as given and read back;
//! * every other member of the lens review is DERIVED on each read — the lens's own
//!   name, version and status, one pillar summary per pillar of the lens in the lens's
//!   own order, and the answered/unanswered roll-ups of the lens's and each pillar's
//!   questions from the template's stored answers;
//! * a lens review exists exactly while the lens is applied to the template, so a
//!   `TemplateArn` nothing is stored under resolves to a modelled 404 and a lens the
//!   template does not apply is the modelled 400 naming `LensAlias` — from the write as
//!   well as from the read.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the state seeding live in the shared
//! `super::workload_harness`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::workload_harness::{
    answered, assert_field_validation, official_lens_arn, review_template as template, Answer,
    Client,
};

/// The pillars of the AWS Well-Architected Framework lens, in the order the framework
/// presents them: the `PillarId` each is named by on the wire, its `PillarName`, and the
/// number of questions it asks (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11,
/// SUS 1-6). Spelled out here — the members are
/// `smithy_query(query="shape", shape="ReviewTemplatePillarReviewSummary")` — so the
/// tests pin the catalog rather than reading it back out of the code under test.
const FRAMEWORK_PILLARS: [(&str, &str, i64); 6] = [
    ("operationalExcellence", "Operational Excellence", 11),
    ("security", "Security", 11),
    ("reliability", "Reliability", 13),
    ("performance", "Performance Efficiency", 5),
    ("costOptimization", "Cost Optimization", 11),
    ("sustainability", "Sustainability", 6),
];

/// How many questions that lens asks in all.
const FRAMEWORK_QUESTIONS: i64 = 11 + 11 + 13 + 5 + 11 + 6;

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that this emulator's AWS-official lens catalog carries no
/// content for, so it asks no questions here (the catalog itself is pinned by
/// `super::official_lens_catalog`).
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// The Region the harness signs every request for, which is the Region an AWS-official
/// lens's `LensArn` is reported in.
const REGION: &str = "us-west-2";

/// The `Question` buckets a `QuestionCounts` map carries, from
/// `smithy_query(query="shape", shape="Question")`.
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

/// The longest `Notes` the model accepts: `Notes` is `@length(max: 2084)`
/// (`smithy_query(query="operation", operation="UpdateReviewTemplateLensReview")`).
const NOTES_MAX: usize = 2084;

/// The `ValidationExceptionReason` a refused member value is classified under, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// A well-formed `TemplateArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:review-template/[a-f0-9]{32}$`) that no
/// stored template carries.
fn absent_template_arn() -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    )
}

/// The `LensReview` of a successful answer, with its `TemplateArn` checked against the
/// template the request named.
fn lens_review(answer: &Answer, template_arn: &str, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("TemplateArn").and_then(Value::as_str),
        Some(template_arn),
        "case {case:?}: TemplateArn must be the template under review, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// A `QuestionCounts` map as a sorted bucket table.
fn question_counts(owner: &Value, case: &str) -> BTreeMap<String, i64> {
    owner
        .get("QuestionCounts")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(bucket, count)| (bucket.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_else(|| panic!("case {case:?}: QuestionCounts must be present, got {owner:?}"))
}

/// The `PillarReviewSummaries` of a lens review.
fn pillar_summaries(review: &Value, case: &str) -> Vec<Value> {
    review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
        })
}

/// The `Notes` a lens review or a pillar summary carries, `None` when it carries none.
fn notes(owner: &Value) -> Option<&str> {
    owner.get("Notes").and_then(Value::as_str)
}

/// A lens review's `UpdatedAt`, the restJson1 epoch-seconds timestamp.
fn updated_at(owner: &Value, case: &str) -> f64 {
    owner
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: UpdatedAt must be present, got {owner:?}"))
}

/// The expected bucket table for `answered` questions out of `total`.
fn expected_counts(answered_count: i64, total: i64) -> BTreeMap<String, i64> {
    BTreeMap::from([
        (ANSWERED.to_string(), answered_count),
        (UNANSWERED.to_string(), total - answered_count),
    ])
}

/// The notes a request supplies are the notes both the write and the following read
/// report, over a TABLE of note values spanning what `Notes` accepts: absent, empty, a
/// one-character note, punctuation and newlines, and the published maximum length.
#[tokio::test]
async fn lens_notes_round_trip_over_the_accepted_note_values() {
    let client = Client::new();
    let template_arn = template(&client, "lens-notes", &[FRAMEWORK_LENS]).await;

    let longest = "n".repeat(NOTES_MAX);
    let cases: [(&str, &str); 4] = [
        ("empty", ""),
        ("one-character", "x"),
        ("punctuation-and-newlines", "line one.\nline two — done!"),
        ("published-maximum", longest.as_str()),
    ];

    for (case, value) in cases {
        let written = client
            .update_review_template_lens_review(
                &template_arn,
                FRAMEWORK_LENS,
                json!({ "LensNotes": value }),
            )
            .await;
        let review = lens_review(&written, &template_arn, case);
        assert_eq!(
            notes(&review),
            Some(value),
            "case {case:?}: the update must report the notes it wrote, got {review:?}"
        );

        let read = client
            .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
            .await;
        let review = lens_review(&read, &template_arn, case);
        assert_eq!(
            notes(&review),
            Some(value),
            "case {case:?}: the read must report the stored notes, got {review:?}"
        );
    }
}

/// One past the published `Notes` maximum is refused at the boundary, and nothing is
/// stored.
#[tokio::test]
async fn notes_past_the_published_maximum_are_refused() {
    let client = Client::new();
    let template_arn = template(&client, "notes-bound", &[FRAMEWORK_LENS]).await;

    let cases: [(&str, usize, u16); 3] = [
        ("one-under-the-maximum", NOTES_MAX - 1, 200),
        ("at-the-maximum", NOTES_MAX, 200),
        ("one-past-the-maximum", NOTES_MAX + 1, 400),
    ];

    for (case, length, expected) in cases {
        let value = "n".repeat(length);
        let answer = client
            .update_review_template_lens_review(
                &template_arn,
                FRAMEWORK_LENS,
                json!({ "LensNotes": value }),
            )
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {answer:?}"
        );
    }

    // The refused request left the last accepted notes in place.
    let read = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    let review = lens_review(&read, &template_arn, "after-the-refusal");
    assert_eq!(
        notes(&review).map(str::len),
        Some(NOTES_MAX),
        "case \"after-the-refusal\": a refused update must store nothing, got {review:?}"
    );
}

/// A supplied `PillarNotes` map REPLACES the stored one, and each pillar summary reports
/// the notes the map gives for its own pillar and none where the map gives it none —
/// over a TABLE of maps spanning no pillar, one, several and every pillar of the lens.
#[tokio::test]
async fn pillar_notes_replace_the_stored_map() {
    let client = Client::new();
    let template_arn = template(&client, "pillar-notes", &[FRAMEWORK_LENS]).await;

    let every: Vec<(&str, String)> = FRAMEWORK_PILLARS
        .iter()
        .map(|(pillar_id, _, _)| (*pillar_id, format!("notes for {pillar_id}")))
        .collect();
    let cases: [(&str, Vec<(&str, String)>); 4] = [
        ("none", Vec::new()),
        (
            "one",
            vec![("security", "watch the perimeter".to_string())],
        ),
        (
            "three",
            vec![
                ("reliability", "test the failovers".to_string()),
                ("sustainability", "".to_string()),
                ("performance", "measure first".to_string()),
            ],
        ),
        ("every", every),
    ];

    for (case, entries) in cases {
        let supplied: BTreeMap<&str, String> = entries.iter().cloned().collect();
        let written = client
            .update_review_template_lens_review(
                &template_arn,
                FRAMEWORK_LENS,
                json!({ "PillarNotes": supplied }),
            )
            .await;
        let review = lens_review(&written, &template_arn, case);

        let read = client
            .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
            .await;
        let read_review = lens_review(&read, &template_arn, case);

        for review in [&review, &read_review] {
            let summaries = pillar_summaries(review, case);
            assert_eq!(
                summaries.len(),
                FRAMEWORK_PILLARS.len(),
                "case {case:?}: one summary per pillar of the lens, got {summaries:?}"
            );
            for (summary, (pillar_id, _, _)) in summaries.iter().zip(FRAMEWORK_PILLARS) {
                assert_eq!(
                    notes(summary),
                    supplied.get(pillar_id).map(String::as_str),
                    "case {case:?}: pillar {pillar_id:?} must report exactly the supplied \
                     notes, got {summary:?}"
                );
            }
        }
    }
}

/// A member the request omits keeps its stored value, so the two note members are
/// written independently; a request that supplies neither changes nothing at all.
#[tokio::test]
async fn an_omitted_member_keeps_its_stored_value() {
    let client = Client::new();
    let template_arn = template(&client, "partial-update", &[FRAMEWORK_LENS]).await;

    let both = client
        .update_review_template_lens_review(
            &template_arn,
            FRAMEWORK_LENS,
            json!({
                "LensNotes": "the lens note",
                "PillarNotes": { "security": "the security note" },
            }),
        )
        .await;
    let seeded = lens_review(&both, &template_arn, "both");
    let seeded_at = updated_at(&seeded, "both");

    // Supplying only `LensNotes` leaves the pillar notes as stored.
    let lens_only = client
        .update_review_template_lens_review(
            &template_arn,
            FRAMEWORK_LENS,
            json!({ "LensNotes": "a newer lens note" }),
        )
        .await;
    let review = lens_review(&lens_only, &template_arn, "lens-notes-only");
    assert_eq!(
        notes(&review),
        Some("a newer lens note"),
        "case \"lens-notes-only\": the supplied member must be replaced, got {review:?}"
    );
    let summaries = pillar_summaries(&review, "lens-notes-only");
    assert_eq!(
        notes(&summaries[1]),
        Some("the security note"),
        "case \"lens-notes-only\": the omitted PillarNotes must be left as stored, got \
         {summaries:?}"
    );

    // Supplying only `PillarNotes` leaves the lens notes as stored.
    let pillars_only = client
        .update_review_template_lens_review(
            &template_arn,
            FRAMEWORK_LENS,
            json!({ "PillarNotes": { "reliability": "a reliability note" } }),
        )
        .await;
    let review = lens_review(&pillars_only, &template_arn, "pillar-notes-only");
    assert_eq!(
        notes(&review),
        Some("a newer lens note"),
        "case \"pillar-notes-only\": the omitted LensNotes must be left as stored, got \
         {review:?}"
    );
    let summaries = pillar_summaries(&review, "pillar-notes-only");
    assert_eq!(
        notes(&summaries[1]),
        None,
        "case \"pillar-notes-only\": the supplied map replaces the stored one, got \
         {summaries:?}"
    );
    assert_eq!(
        notes(&summaries[2]),
        Some("a reliability note"),
        "case \"pillar-notes-only\": the supplied map must be stored, got {summaries:?}"
    );

    // Supplying NEITHER member writes nothing — not even `UpdatedAt`.
    let before = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    let before = lens_review(&before, &template_arn, "neither");
    let nothing = client
        .update_review_template_lens_review(&template_arn, FRAMEWORK_LENS, json!({}))
        .await;
    let after = lens_review(&nothing, &template_arn, "neither");
    assert_eq!(
        (notes(&after), updated_at(&after, "neither")),
        (notes(&before), updated_at(&before, "neither")),
        "case \"neither\": an update that supplies no member must change nothing, before \
         {before:?} after {after:?}"
    );
    assert!(
        updated_at(&before, "neither") >= seeded_at,
        "case \"neither\": UpdatedAt must not go backwards, seeded at {seeded_at} then \
         {before:?}"
    );
}

/// Everything but the notes is derived on every read: the lens's own name, version,
/// status and ARN, one pillar summary per pillar in the lens's order, and the
/// answered/unanswered roll-ups from the template's stored answers.
#[tokio::test]
async fn the_lens_review_is_derived_from_the_lens_and_the_stored_answers() {
    let client = Client::new();
    let template_arn = template(&client, "derived", &[FRAMEWORK_LENS]).await;

    // A TABLE of answer sets: nothing answered, one question of one pillar, and
    // questions spread across three pillars.
    let cases: [(&str, &[(&str, &str)]); 3] = [
        ("none", &[]),
        ("one", &[("security", "sec-01")]),
        (
            "spread",
            &[
                ("security", "sec-01"),
                ("security", "sec-02"),
                ("sustainability", "sus-06"),
                ("operationalExcellence", "ops-11"),
            ],
        ),
    ];

    // One answer per QUESTION, however many cases record it: a second answer to the same
    // question replaces the first rather than adding to the roll-up.
    let mut recorded: std::collections::BTreeSet<(&str, &str)> = std::collections::BTreeSet::new();
    for (case, answers) in cases {
        for (_, question_id) in answers {
            client.record_template_answer(
                &template_arn,
                FRAMEWORK_LENS,
                question_id,
                answered(&[("choice", "SELECTED")], true, None),
            );
        }
        recorded.extend(answers.iter().copied());

        let read = client
            .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
            .await;
        let review = lens_review(&read, &template_arn, case);

        assert_eq!(
            review.get("LensAlias").and_then(Value::as_str),
            Some(FRAMEWORK_LENS),
            "case {case:?}: LensAlias is the lens as the request named it, got {review:?}"
        );
        assert_eq!(
            review.get("LensArn").and_then(Value::as_str),
            Some(official_lens_arn(REGION, FRAMEWORK_LENS).as_str()),
            "case {case:?}: LensArn is the official lens ARN in the request's Region, got \
             {review:?}"
        );
        for member in ["LensName", "LensVersion", "LensStatus"] {
            assert!(
                review.get(member).and_then(Value::as_str).is_some(),
                "case {case:?}: {member} must be reported for a lens this emulator holds \
                 content for, got {review:?}"
            );
        }
        assert_eq!(
            question_counts(&review, case),
            expected_counts(recorded.len() as i64, FRAMEWORK_QUESTIONS),
            "case {case:?}: the lens roll-up counts the stored answers, got {review:?}"
        );

        let summaries = pillar_summaries(&review, case);
        assert_eq!(
            summaries
                .iter()
                .map(|summary| {
                    (
                        summary
                            .get("PillarId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        summary
                            .get("PillarName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>(),
            FRAMEWORK_PILLARS
                .iter()
                .map(|(pillar_id, pillar_name, _)| {
                    ((*pillar_id).to_string(), (*pillar_name).to_string())
                })
                .collect::<Vec<_>>(),
            "case {case:?}: one summary per pillar in the lens's own order, got \
             {summaries:?}"
        );
        for (summary, (pillar_id, _, questions)) in summaries.iter().zip(FRAMEWORK_PILLARS) {
            let answered_here = recorded
                .iter()
                .filter(|(pillar, _)| *pillar == pillar_id)
                .count() as i64;
            assert_eq!(
                question_counts(summary, case),
                expected_counts(answered_here, questions),
                "case {case:?}: pillar {pillar_id:?} counts its own questions, got \
                 {summary:?}"
            );
        }
    }
}

/// Either spelling of the lens — the bare alias or the AWS-official lens ARN — names the
/// same lens review, so notes written by one name are read back by the other.
#[tokio::test]
async fn either_spelling_of_the_lens_names_one_review() {
    let client = Client::new();
    let template_arn = template(&client, "lens-spelling", &[FRAMEWORK_LENS]).await;
    let arn = official_lens_arn(REGION, FRAMEWORK_LENS);

    let cases: [(&str, &str, &str); 2] = [
        ("written-by-alias", FRAMEWORK_LENS, arn.as_str()),
        ("written-by-arn", arn.as_str(), FRAMEWORK_LENS),
    ];

    for (case, written_by, read_by) in cases {
        let value = format!("notes written by {written_by}");
        let written = client
            .update_review_template_lens_review(
                &template_arn,
                written_by,
                json!({ "LensNotes": value }),
            )
            .await;
        let review = lens_review(&written, &template_arn, case);
        assert_eq!(
            review.get("LensAlias").and_then(Value::as_str),
            Some(written_by),
            "case {case:?}: LensAlias is the lens as the request named it, got {review:?}"
        );

        let read = client
            .get_review_template_lens_review(&template_arn, read_by)
            .await;
        let review = lens_review(&read, &template_arn, case);
        assert_eq!(
            notes(&review),
            Some(value.as_str()),
            "case {case:?}: the other spelling must read the same review, got {review:?}"
        );
        assert_eq!(
            review.get("LensAlias").and_then(Value::as_str),
            Some(read_by),
            "case {case:?}: LensAlias is the lens as the request named it, got {review:?}"
        );
    }
}

/// A `PillarNotes` key that names no pillar of the lens under review is a modelled 400
/// naming that member, and nothing is stored.
#[tokio::test]
async fn a_key_that_names_no_pillar_of_the_lens_is_refused() {
    let client = Client::new();
    let template_arn = template(&client, "bad-pillar", &[FRAMEWORK_LENS]).await;

    let cases: [(&str, Value); 4] = [
        ("unknown-pillar", json!({ "notAPillar": "x" })),
        // `PillarId` values are the catalog's own spelling.
        (
            "wrong-case",
            json!({ "OperationalExcellence": "x" }),
        ),
        (
            "valid-mixed-with-invalid",
            json!({ "security": "x", "notAPillar": "y" }),
        ),
        (
            "two-invalid",
            json!({ "notAPillar": "x", "alsoNotAPillar": "y" }),
        ),
    ];

    for (case, pillar_notes) in cases {
        let answer = client
            .update_review_template_lens_review(
                &template_arn,
                FRAMEWORK_LENS,
                json!({ "PillarNotes": pillar_notes }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some(REASON_FIELD_VALIDATION_FAILED),
            "case {case:?}: a refused member value is a field validation failure, got \
             {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| field.get("Name").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                }),
            Some(vec!["PillarNotes"]),
            "case {case:?}: the finding must name the offending member, got {answer:?}"
        );
    }

    let read = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    let review = lens_review(&read, &template_arn, "after-the-refusals");
    assert!(
        pillar_summaries(&review, "after-the-refusals")
            .iter()
            .all(|summary| notes(summary).is_none()),
        "case \"after-the-refusals\": a refused update must store nothing, got {review:?}"
    );

    // The resource is resolved before the request's values are judged, so a template
    // that does not exist is a 404 whatever else the request carries.
    let absent = client
        .update_review_template_lens_review(
            &absent_template_arn(),
            FRAMEWORK_LENS,
            json!({ "PillarNotes": { "notAPillar": "x" } }),
        )
        .await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"absent-template-and-bad-key\": the lookup is answered first, got {absent:?}"
    );
}

/// A lens this emulator ships no content for asks no questions and names no pillars, so
/// its lens review reports an all-zero roll-up and none of the lens's own metadata —
/// while its notes, including notes for a pillar only the real lens defines, still
/// round-trip.
#[tokio::test]
async fn a_lens_without_shipped_content_reports_an_empty_review() {
    let client = Client::new();
    let template_arn = template(&client, "unshipped-lens", &[UNSHIPPED_LENS]).await;

    let written = client
        .update_review_template_lens_review(
            &template_arn,
            UNSHIPPED_LENS,
            json!({ "LensNotes": "notes on an unshipped lens" }),
        )
        .await;
    let review = lens_review(&written, &template_arn, "unshipped");
    assert_eq!(
        notes(&review),
        Some("notes on an unshipped lens"),
        "case \"unshipped\": the notes must round-trip, got {review:?}"
    );
    assert_eq!(
        pillar_summaries(&review, "unshipped"),
        Vec::<Value>::new(),
        "case \"unshipped\": a lens this emulator holds no content for names no pillars \
         of its own, got {review:?}"
    );
    assert_eq!(
        question_counts(&review, "unshipped"),
        expected_counts(0, 0),
        "case \"unshipped\": a lens with no content asks no questions, got {review:?}"
    );
    for member in ["LensName", "LensVersion", "LensStatus"] {
        assert_eq!(
            review.get(member),
            None,
            "case \"unshipped\": {member} is lens content this emulator does not hold, got \
             {review:?}"
        );
    }

    // There is no pillar set to judge a key against, so notes for the pillars the REAL
    // lens defines are stored as given and reported back in `PillarId` order, with no
    // `PillarName` since no catalog here names them.
    let annotated = client
        .update_review_template_lens_review(
            &template_arn,
            UNSHIPPED_LENS,
            json!({ "PillarNotes": { "security": "watch it", "costOptimization": "mind it" } }),
        )
        .await;
    let review = lens_review(&annotated, &template_arn, "unshipped-pillars");
    let read = client
        .get_review_template_lens_review(&template_arn, UNSHIPPED_LENS)
        .await;
    let read_review = lens_review(&read, &template_arn, "unshipped-pillars");
    for review in [&review, &read_review] {
        let summaries = pillar_summaries(review, "unshipped-pillars");
        assert_eq!(
            summaries
                .iter()
                .map(|summary| (
                    summary.get("PillarId").and_then(Value::as_str),
                    notes(summary),
                    summary.get("PillarName").and_then(Value::as_str),
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some("costOptimization"), Some("mind it"), None),
                (Some("security"), Some("watch it"), None),
            ],
            "case \"unshipped-pillars\": the notes must be stored and readable back, got \
             {summaries:?}"
        );
    }
}

/// A `TemplateArn` nothing is stored under resolves to a modelled 404, while a lens the
/// template does not apply is the modelled 400 naming `LensAlias` — from the write as well
/// as from the read.
#[tokio::test]
async fn an_unresolvable_review_is_refused() {
    let client = Client::new();
    let template_arn = template(&client, "not-found", &[FRAMEWORK_LENS]).await;
    let absent = absent_template_arn();

    for (case, request) in [
        (
            "absent-template-read",
            client
                .get_review_template_lens_review(&absent, FRAMEWORK_LENS)
                .await,
        ),
        (
            "absent-template-write",
            client
                .update_review_template_lens_review(
                    &absent,
                    FRAMEWORK_LENS,
                    json!({ "LensNotes": "x" }),
                )
                .await,
        ),
    ] {
        assert_eq!(
            (request.status, request.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unresolvable template must answer a modelled 404, got {request:?}"
        );
    }

    // The rows differ along how the unapplied lens is named, because what the request asks
    // for does not depend on the spelling.
    for (case, lens) in [
        ("unapplied-lens", UNSHIPPED_LENS),
        (
            "unapplied-lens-by-arn",
            "arn:aws:wellarchitected:us-west-2::lens/serverless",
        ),
    ] {
        let sentence = format!(
            "No lens review for lens alias {lens} was found in review template {template_arn}."
        );
        let read = client
            .get_review_template_lens_review(&template_arn, lens)
            .await;
        assert_field_validation(&format!("{case}-read"), &read, "LensAlias", &sentence);

        let written = client
            .update_review_template_lens_review(&template_arn, lens, json!({ "LensNotes": "x" }))
            .await;
        assert_field_validation(&format!("{case}-write"), &written, "LensAlias", &sentence);
    }
}

/// `UpdatedAt` is when the lens review was last written: the template's own `UpdatedAt`
/// until something is written to it, then the instant of that write — stable across
/// reads and never going backwards.
#[tokio::test]
async fn updated_at_tracks_the_last_write() {
    let client = Client::new();
    let template_arn = template(&client, "updated-at", &[FRAMEWORK_LENS]).await;

    let stored = client.get_review_template(&template_arn).await;
    assert_eq!(
        stored.status, 200,
        "case \"created\": expected GetReviewTemplate 200, got {stored:?}"
    );
    let template_updated_at = stored
        .body
        .get("ReviewTemplate")
        .and_then(|value| value.get("UpdatedAt"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case \"created\": the template must carry UpdatedAt"));

    let fresh = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    let fresh = lens_review(&fresh, &template_arn, "created");
    assert_eq!(
        updated_at(&fresh, "created"),
        template_updated_at,
        "case \"created\": a lens review nothing was written to was last changed when the \
         template was, got {fresh:?}"
    );

    let written = client
        .update_review_template_lens_review(
            &template_arn,
            FRAMEWORK_LENS,
            json!({ "LensNotes": "a note" }),
        )
        .await;
    let written = lens_review(&written, &template_arn, "written");
    assert!(
        updated_at(&written, "written") >= template_updated_at,
        "case \"written\": UpdatedAt must not go backwards, template at \
         {template_updated_at} then {written:?}"
    );

    let first = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    let second = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    let first = lens_review(&first, &template_arn, "re-read");
    let second = lens_review(&second, &template_arn, "re-read");
    assert_eq!(
        (
            updated_at(&first, "re-read"),
            updated_at(&second, "re-read")
        ),
        (
            updated_at(&written, "written"),
            updated_at(&written, "written")
        ),
        "case \"re-read\": a read reports the write's instant and changes nothing, got \
         {first:?} then {second:?}"
    );
}
