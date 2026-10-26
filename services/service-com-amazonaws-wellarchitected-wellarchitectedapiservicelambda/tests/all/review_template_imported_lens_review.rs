// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateReviewTemplateLensReview` and
//! `GetReviewTemplateLensReview` when the lens the review template applies is a CUSTOM lens
//! the caller imported and published.
//!
//! The rule these pin, over a TABLE of lens documents rather than one recorded request: a
//! review template's lens review reports and counts the REVIEWED LENS's own content — the
//! name that content gives the lens, the version the review is against and that version's
//! currency, one pillar review summary per pillar that content organizes the lens into (in
//! the content's own order, each carrying the name that content gives it) and
//! `QuestionCounts` counted over the questions that content asks, `UNANSWERED` until the
//! template records an answer. For a custom lens that content is the document its applied
//! version was published from; for an AWS-official lens it is that lens's catalog entry. The
//! write and the read project the same review, and a `PillarId` a write supplies is judged
//! against the same pillar set.
//!
//! The rows differ along the SHAPE of the document — how many pillars it defines, how many
//! questions each pillar asks and how many choices each question offers — and along how many
//! of those questions the template has answered, because those are what the reported pillars
//! and the roll-ups move with. Every row applies the AWS-official framework lens beside the
//! imported one, so a review that resolved the wrong half of the catalog would fail.
//!
//! `super::custom_lens_review_rollup` pins the same resolution for a WORKLOAD's lens review
//! and for the roll-ups of a template that has answered nothing;
//! `super::review_template_imported_lens_answers` pins the answer LISTING of such a review.
//! This suite is the review-template lens review itself — written as well as read, and with
//! answers recorded against it. The document builder and the import/publish wire helpers are
//! `super::custom_lens_import`'s and `super::custom_lens_review_answers`', reused here
//! rather than respelled.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{
    expected_questions, import_shape, publish, question_count, string_member, Shape,
};
use super::workload_harness::{official_lens_arn, review_template, Answer, Client};

/// The alias of the AWS Well-Architected Framework lens — the official half of the catalog
/// every row applies beside its imported lens — and how many questions it asks: the
/// published Well-Architected Framework numbering (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5,
/// COST 1-11, SUS 1-6), spelled out here rather than read back out of the code under test.
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// The Region the harness signs every request for, which is the Region an AWS-official
/// lens's `LensArn` is reported in.
const REGION: &str = "us-west-2";

/// The `Question` buckets a `QuestionCounts` map carries
/// (`smithy_query(query="shape", shape="Question")`).
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

/// The `LensVersion` every row publishes its imported lens as, mirroring
/// `super::custom_lens_review_answers::publish`: the version the review of it is against.
const PUBLISHED_VERSION: &str = "1.0";

/// The `LensStatus` of a review against the lens's latest version
/// (`smithy_query(query="shape", shape="LensStatus")`).
const LENS_STATUS_CURRENT: &str = "CURRENT";

/// The `ValidationExceptionReason` a refused member value is classified under
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The documents under test: the rows differ along the number of pillars, the number of
/// questions a pillar asks and the number of choices a question offers — from the smallest
/// document the lens JSON format admits up to one organized into several pillars.
const SHAPES: &[(&str, Shape)] = &[
    ("smallest-document", &[("governance", 1, 1)]),
    ("one-pillar-many-questions", &[("security", 5, 2)]),
    (
        "several-pillars",
        &[("operations", 2, 1), ("security", 1, 3), ("cost", 4, 2)],
    ),
];

/// The name the document of `case` gives its lens, mirroring
/// `super::custom_lens_review_answers::import_shape`, so the expectation comes from the
/// document the test sent rather than from the service that answered.
fn expected_lens_name(case: &str) -> String {
    format!("Lens {case}")
}

/// The `PillarName` the document of a shape gives the pillar at `index`, mirroring what
/// `super::custom_lens_import::lens_document` writes.
fn expected_pillar_name(index: usize, pillar_id: &str) -> String {
    format!("Pillar {index}: {pillar_id}")
}

/// The `LensReview` of a successful answer, with the envelope's `TemplateArn` checked
/// against the template the request named.
fn lens_review(answer: &Answer, template_arn: &str, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("TemplateArn").and_then(Value::as_str),
        Some(template_arn),
        "case {case:?}: the envelope must name the template under review, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// One `QuestionCounts` map of a response, as a sorted bucket table.
fn question_counts(owner: &Value, case: &str) -> BTreeMap<String, i64> {
    owner
        .get("QuestionCounts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(bucket, count)| (bucket.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_else(|| panic!("case {case:?}: QuestionCounts must be present, got {owner:?}"))
}

/// The `PillarReviewSummaries` of a lens review, in the order they were served.
fn pillar_summaries(review: &Value, case: &str) -> Vec<Value> {
    review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
        })
}

/// The question roll-up expected over `total` questions of which `answered` carry an answer:
/// both buckets the enum publishes are present, so a bucket a case did not move is checked
/// too.
fn expected_counts(answered: usize, total: usize) -> BTreeMap<String, i64> {
    let total = i64::try_from(total).unwrap_or(i64::MAX);
    let answered = i64::try_from(answered).unwrap_or(i64::MAX);
    BTreeMap::from([
        (ANSWERED.to_string(), answered),
        (UNANSWERED.to_string(), total - answered),
    ])
}

/// Import a document of `shape`, wait for the import to land and publish it: a lens
/// "cannot be applied to workloads … until it's published with CreateLensVersion"
/// (`ImportLens` doc). Answers the lens ARN.
async fn published_lens(client: &Client, case: &str, shape: Shape) -> String {
    let lens_arn = import_shape(client, case, shape).await;
    wait_for_the_import_to_land();
    publish(client, case, &lens_arn).await;
    lens_arn
}

/// `UpdateReviewTemplateLensReview` with the notes a case supplies.
async fn update_review(client: &Client, template_arn: &str, lens: &str, body: Value) -> Answer {
    client
        .update_review_template_lens_review(template_arn, lens, body)
        .await
}

/// The notes one row writes for every pillar its document defines, keyed by the `PillarId`
/// the document gives that pillar.
fn pillar_notes(shape: Shape) -> BTreeMap<String, String> {
    shape
        .iter()
        .map(|(pillar_id, _, _)| {
            (
                (*pillar_id).to_string(),
                format!("notes about {pillar_id}"),
            )
        })
        .collect()
}

/// Rule 1. Writing and then reading one lens review of a template reports the IMPORTED
/// lens's own content: the name its document gives it, the version the template applied it
/// at and that version's currency, one pillar summary per pillar the document defines — in
/// the document's order, with the name the document gives it and the notes the write wrote —
/// and `QuestionCounts` over the questions the document asks, unanswered until an answer is
/// recorded. The write and the following read project the same review.
#[tokio::test]
async fn a_templates_imported_lens_review_reports_that_documents_content() {
    let client = Client::new();

    // Every row's lens is imported first, so the whole table waits for the import window
    // once rather than per row.
    let mut lenses: Vec<(&str, Shape, String)> = Vec::new();
    for (case, shape) in SHAPES {
        lenses.push((case, shape, import_shape(&client, case, shape).await));
    }
    wait_for_the_import_to_land();

    for (case, shape, lens_arn) in &lenses {
        publish(&client, case, lens_arn).await;
        let template_arn =
            review_template(&client, &format!("tpl-{case}"), &[lens_arn, FRAMEWORK_LENS]).await;
        let asked = question_count(shape);
        let notes = pillar_notes(shape);
        let lens_notes = format!("notes about the {case} lens");

        let written = update_review(
            &client,
            &template_arn,
            lens_arn,
            json!({ "LensNotes": lens_notes, "PillarNotes": notes }),
        )
        .await;
        let review = lens_review(&written, &template_arn, case);

        assert_eq!(
            string_member(&review, "LensAlias"),
            *lens_arn,
            "case {case:?}: the review must name the lens as sent, got {review:?}"
        );
        assert_eq!(
            string_member(&review, "LensArn"),
            *lens_arn,
            "case {case:?}: the review must report the ARN the import minted, got {review:?}"
        );
        assert_eq!(
            string_member(&review, "LensName"),
            expected_lens_name(case),
            "case {case:?}: the review must name the lens its document names, got {review:?}"
        );
        assert_eq!(
            string_member(&review, "LensVersion"),
            PUBLISHED_VERSION,
            "case {case:?}: the review must report the version the lens was applied at, got \
             {review:?}"
        );
        assert_eq!(
            string_member(&review, "LensStatus"),
            LENS_STATUS_CURRENT,
            "case {case:?}: a review against the lens's latest version is current, got {review:?}"
        );
        assert_eq!(
            string_member(&review, "Notes"),
            lens_notes,
            "case {case:?}: the write must report the notes it wrote, got {review:?}"
        );
        assert_eq!(
            question_counts(&review, case),
            expected_counts(0, asked),
            "case {case:?}: the review must count the {asked} questions the document defines, \
             got {review:?}"
        );

        let summaries = pillar_summaries(&review, case);
        assert_eq!(
            summaries.len(),
            shape.len(),
            "case {case:?}: one pillar summary per pillar the document defines, got {summaries:?}"
        );
        for (summary, (index, (pillar_id, questions, _))) in
            summaries.iter().zip(shape.iter().enumerate())
        {
            let case = format!("{case}-{pillar_id}");
            assert_eq!(
                string_member(summary, "PillarId"),
                *pillar_id,
                "case {case:?}: the pillar summaries must follow the document's order, got \
                 {summaries:?}"
            );
            assert_eq!(
                string_member(summary, "PillarName"),
                expected_pillar_name(index, pillar_id),
                "case {case:?}: the pillar must carry the name its document gives it, got \
                 {summary:?}"
            );
            assert_eq!(
                string_member(summary, "Notes"),
                notes.get(*pillar_id).cloned().unwrap_or_default(),
                "case {case:?}: the pillar must report the notes written for it, got {summary:?}"
            );
            assert_eq!(
                question_counts(summary, &case),
                expected_counts(0, *questions),
                "case {case:?}: the pillar must count its own {questions} questions, got \
                 {summary:?}"
            );
        }

        // The round trip: the read reports the review the write projected, member for
        // member.
        let read = client
            .get_review_template_lens_review(&template_arn, lens_arn)
            .await;
        assert_eq!(
            lens_review(&read, &template_arn, case),
            review,
            "case {case:?}: the read must report the review the write projected, got {read:?}"
        );
    }
}

/// Rule 2. The roll-ups are DERIVED from the template's stored answers on every read:
/// recording an answer against a question the imported document defines moves that
/// question out of `UNANSWERED` — on the lens review, on the summary of the pillar that
/// question belongs to and on the template's own roll-up, which spans every lens it applies
/// — and leaves every other pillar where it was.
#[tokio::test]
async fn answering_a_templates_imported_question_moves_its_reviews_question_counts() {
    // Several pillars, so the summary of the answered pillar can be told from the others.
    const SHAPE: Shape = &[("governance", 3, 2), ("cost", 2, 1)];
    const ANSWERED_PILLAR: &str = "governance";

    let client = Client::new();
    let lens_arn = published_lens(&client, "answered", SHAPE).await;
    let template_arn =
        review_template(&client, "tpl-answered", &[&lens_arn, FRAMEWORK_LENS]).await;

    let questions = expected_questions(SHAPE);
    let asked = question_count(SHAPE);
    // Answered one at a time, so every intermediate roll-up is checked rather than only the
    // final one.
    let answering: Vec<_> = questions
        .iter()
        .filter(|question| question.pillar_id == ANSWERED_PILLAR)
        .collect();

    for (recorded, question) in answering.iter().enumerate() {
        let case = format!("answered-{}", question.question_id);
        let selected: Vec<String> = question
            .choices
            .iter()
            .map(|(choice_id, _)| choice_id.clone())
            .collect();
        let answer = client
            .update_review_template_answer(
                &template_arn,
                &lens_arn,
                &question.question_id,
                json!({ "SelectedChoices": selected }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a question the document defines must be answerable, got {answer:?}"
        );

        let recorded = recorded + 1;
        let read = client
            .get_review_template_lens_review(&template_arn, &lens_arn)
            .await;
        let review = lens_review(&read, &template_arn, &case);
        assert_eq!(
            question_counts(&review, &case),
            expected_counts(recorded, asked),
            "case {case:?}: {recorded} of the document's {asked} questions carry an answer, got \
             {review:?}"
        );

        let summaries = pillar_summaries(&review, &case);
        assert_eq!(
            summaries.len(),
            SHAPE.len(),
            "case {case:?}: one pillar summary per pillar the document defines, got {summaries:?}"
        );
        for (summary, (pillar_id, pillar_questions, _)) in summaries.iter().zip(SHAPE) {
            let answered_here = if *pillar_id == ANSWERED_PILLAR { recorded } else { 0 };
            assert_eq!(
                question_counts(summary, &case),
                expected_counts(answered_here, *pillar_questions),
                "case {case:?}: pillar {pillar_id:?} must count {answered_here} of its \
                 {pillar_questions} questions answered, got {summary:?}"
            );
        }

        let fetched = client.get_review_template(&template_arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetReviewTemplate 200, got {fetched:?}"
        );
        let template = fetched
            .body
            .get("ReviewTemplate")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: ReviewTemplate must be present, got {fetched:?}")
            });
        assert_eq!(
            question_counts(&template, &case),
            expected_counts(recorded, asked + FRAMEWORK_QUESTIONS),
            "case {case:?}: the template's roll-up must span both applied lenses, got \
             {template:?}"
        );
    }
}

/// Rule 3. A `PillarNotes` key a write supplies is judged against the IMPORTED document's
/// pillars: a pillar the document defines is accepted and read back on that pillar's
/// summary, and one it does not define — including a pillar of the AWS-official framework
/// lens the same template applies — is the modelled 400, with nothing stored.
#[tokio::test]
async fn a_pillar_note_on_a_templates_imported_lens_review_is_judged_against_the_document() {
    const SHAPE: Shape = &[("governance", 1, 2), ("resilience", 2, 1)];

    let client = Client::new();
    let lens_arn = published_lens(&client, "pillar-notes", SHAPE).await;
    let template_arn =
        review_template(&client, "tpl-pillar-notes", &[&lens_arn, FRAMEWORK_LENS]).await;

    let notes = pillar_notes(SHAPE);
    let accepted = update_review(
        &client,
        &template_arn,
        &lens_arn,
        json!({ "PillarNotes": notes }),
    )
    .await;
    let review = lens_review(&accepted, &template_arn, "every-defined-pillar");
    let summaries = pillar_summaries(&review, "every-defined-pillar");
    assert_eq!(
        summaries.len(),
        SHAPE.len(),
        "case \"every-defined-pillar\": one pillar summary per pillar the document defines, got \
         {summaries:?}"
    );
    for summary in &summaries {
        let pillar_id = string_member(summary, "PillarId");
        assert_eq!(
            string_member(summary, "Notes"),
            notes.get(&pillar_id).cloned().unwrap_or_default(),
            "case {pillar_id:?}: the notes written for the pillar must be read back on its \
             summary, got {summary:?}"
        );
    }

    // A key naming no pillar of THIS document: one of the framework lens's pillars, an id
    // of no lens at all, and a defined pillar beside an undefined one.
    let rows: [(&str, &[&str]); 3] = [
        ("official-lenss-pillar", &["security"]),
        ("no-lenss-pillar", &["not-a-pillar"]),
        (
            "a-defined-and-an-undefined-pillar",
            &["governance", "sustainability"],
        ),
    ];
    for (case, keys) in rows {
        let body = json!({
            "PillarNotes": keys
                .iter()
                .map(|key| ((*key).to_string(), json!("notes")))
                .collect::<serde_json::Map<String, Value>>(),
        });
        let refused = update_review(&client, &template_arn, &lens_arn, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a pillar the document does not define must be refused, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.body.get("Reason").and_then(Value::as_str),
            Some(REASON_FIELD_VALIDATION_FAILED),
            "case {case:?}: a refused member value is a field validation failure, got {refused:?}"
        );

        let read = client
            .get_review_template_lens_review(&template_arn, &lens_arn)
            .await;
        assert_eq!(
            lens_review(&read, &template_arn, case),
            review,
            "case {case:?}: a refused write must leave the review as it stood, got {read:?}"
        );
    }
}

/// Rule 4. Each lens review of one template resolves ITS OWN lens's content: the imported
/// lens's review reports the document's pillars and question set, and the AWS-official
/// framework lens's review of the same template reports that lens's catalog — its own
/// pillars, each with a name, and its own question total — with no pillar reported for both.
#[tokio::test]
async fn each_lens_review_of_a_template_reports_its_own_lenss_content() {
    const SHAPE: Shape = &[("governance", 2, 2)];

    let client = Client::new();
    let lens_arn = published_lens(&client, "two-halves", SHAPE).await;
    let template_arn =
        review_template(&client, "tpl-two-halves", &[&lens_arn, FRAMEWORK_LENS]).await;

    let custom = lens_review(
        &client
            .get_review_template_lens_review(&template_arn, &lens_arn)
            .await,
        &template_arn,
        "custom-half",
    );
    let official = lens_review(
        &client
            .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
            .await,
        &template_arn,
        "official-half",
    );

    assert_eq!(
        question_counts(&custom, "custom-half"),
        expected_counts(0, question_count(SHAPE)),
        "case \"custom-half\": the imported lens's review counts its document's questions, got \
         {custom:?}"
    );
    assert_eq!(
        string_member(&official, "LensArn"),
        official_lens_arn(REGION, FRAMEWORK_LENS),
        "case \"official-half\": an AWS-official lens is reported by its official ARN, got \
         {official:?}"
    );
    assert_eq!(
        question_counts(&official, "official-half"),
        expected_counts(0, FRAMEWORK_QUESTIONS),
        "case \"official-half\": the framework lens's review counts its catalog's \
         {FRAMEWORK_QUESTIONS} questions, got {official:?}"
    );
    assert_eq!(
        string_member(&official, "LensStatus"),
        LENS_STATUS_CURRENT,
        "case \"official-half\": a review of the one version AWS published here is current, got \
         {official:?}"
    );

    // The pillars of the official half are its own: each carries a name and its own
    // questions, and the two halves report no pillar in common.
    let official_pillars = pillar_summaries(&official, "official-half");
    let counted: i64 = official_pillars
        .iter()
        .map(|summary| {
            question_counts(summary, "official-half")
                .get(UNANSWERED)
                .copied()
                .unwrap_or(-1)
        })
        .sum();
    assert_eq!(
        counted,
        i64::try_from(FRAMEWORK_QUESTIONS).unwrap_or(i64::MAX),
        "case \"official-half\": the pillar summaries must account for every question the lens \
         asks, got {official_pillars:?}"
    );
    for summary in &official_pillars {
        assert!(
            !string_member(summary, "PillarName").is_empty(),
            "case \"official-half\": every pillar of the catalog carries a name, got {summary:?}"
        );
    }
    let shared: BTreeSet<String> = official_pillars
        .iter()
        .map(|summary| string_member(summary, "PillarId"))
        .collect::<BTreeSet<String>>()
        .intersection(
            &pillar_summaries(&custom, "custom-half")
                .iter()
                .map(|summary| string_member(summary, "PillarId"))
                .collect(),
        )
        .cloned()
        .collect();
    assert!(
        shared.is_empty(),
        "case \"two-halves\": each review reports only its own lens's pillars, got {shared:?} in \
         both"
    );
}
