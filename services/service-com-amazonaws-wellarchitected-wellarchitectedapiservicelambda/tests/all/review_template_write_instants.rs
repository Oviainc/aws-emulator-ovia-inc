// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the instant a review template reports it was last updated
//! at, across the write paths that change what the template holds.
//!
//! The rule these pin, over a TABLE of writes rather than one recorded request: a write
//! that changes part of a review template — the notes of one of its lens reviews, or the
//! answer to one of their questions — is a write to the TEMPLATE, so the template and the
//! lens review both report that write's instant as the one they were last updated at,
//! everywhere the template is projected (`GetReviewTemplate` and `ListReviewTemplates`),
//! while a request that changes nothing moves neither instant.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the review-template seeding live in the shared
//! `super::workload_harness`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::workload_harness::{official_lens_arn, review_template, Answer, Client};

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that this emulator's AWS-official lens catalog carries no
/// content for, so it organizes the review into no pillars here (the catalog itself is
/// pinned by `super::official_lens_catalog`).
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// The Region the harness signs every request for.
const REGION: &str = "us-west-2";

/// Two `PillarId`s of the framework lens, from the pillars that lens is organized into
/// (the catalog is pinned by `super::official_lens_catalog`).
const FRAMEWORK_PILLARS: [&str; 2] = ["security", "sustainability"];

/// `QuestionId`s of the framework lens's published numbering, in two different pillars.
const FRAMEWORK_QUESTIONS: [&str; 2] = ["organization-priorities", "identities"];

/// How far into the past the stored instants are moved before the request under test, so a
/// write the handler makes is observable whatever the clock's granularity between two
/// in-process requests — and so a write it must NOT make cannot hide behind that
/// granularity either.
const AGE: Duration = Duration::from_secs(3600);

/// Move a stored review template's `UpdatedAt`, and that of every lens review it holds,
/// back by [`AGE`]. Only the SEED reaches state directly; the write under test is the
/// handler's.
fn age_review_template(client: &Client, template_arn: &str) {
    let aged = SystemTime::now() - AGE;
    let state = client.store();
    let mut store = state.lock();
    let template = store
        .review_templates
        .get_mut(template_arn)
        .expect("the review template under test must exist");
    template.updated_at = aged;
    for review in template.lens_reviews.values_mut() {
        review.updated_at = aged;
    }
}

/// One timestamp member of a JSON object, as the epoch seconds restJson1 carries it.
fn timestamp(value: &Value, member: &str, case: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be present, got {value:?}"))
}

/// The `ReviewTemplate$UpdatedAt` `GetReviewTemplate` reports.
async fn template_updated_at(client: &Client, template_arn: &str, case: &str) -> f64 {
    let answer = client.get_review_template(template_arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplate 200, got {answer:?}"
    );
    let template = answer
        .body
        .get("ReviewTemplate")
        .cloned()
        .unwrap_or(Value::Null);
    timestamp(&template, "UpdatedAt", case)
}

/// The `ReviewTemplateSummary$UpdatedAt` `ListReviewTemplates` reports for this template,
/// so the second projection of the same field is pinned beside the first.
async fn summary_updated_at(client: &Client, template_arn: &str, case: &str) -> f64 {
    let answer = client.list_review_templates("").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListReviewTemplates 200, got {answer:?}"
    );
    let summary = answer
        .body
        .get("ReviewTemplates")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| {
                    summary.get("TemplateArn").and_then(Value::as_str) == Some(template_arn)
                })
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: the stored template must be listed, got {answer:?}")
        });
    timestamp(&summary, "UpdatedAt", case)
}

/// The `ReviewTemplateLensReview$UpdatedAt` `GetReviewTemplateLensReview` reports.
async fn lens_review_updated_at(
    client: &Client,
    template_arn: &str,
    lens: &str,
    case: &str,
) -> f64 {
    let answer = client
        .get_review_template_lens_review(template_arn, lens)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplateLensReview 200, got {answer:?}"
    );
    let review = answer.body.get("LensReview").cloned().unwrap_or(Value::Null);
    timestamp(&review, "UpdatedAt", case)
}

/// The `LensReview$UpdatedAt` a successful write answered with.
fn written_updated_at(answer: &Answer, case: &str) -> f64 {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    let review = answer.body.get("LensReview").cloned().unwrap_or(Value::Null);
    timestamp(&review, "UpdatedAt", case)
}

/// Writing a lens review's notes moves the OWNING template's `UpdatedAt` to the instant of
/// that write, in both projections of the template, and to the same instant the lens review
/// itself reports.
///
/// The rows span what the write carries (each note member on its own, both together, each
/// at its emptiest accepted value), how the request spells the lens (bare alias or the
/// AWS-official lens ARN) and whether this emulator holds the reviewed lens's pillar set.
#[tokio::test]
async fn writing_a_templates_lens_notes_moves_the_templates_updated_at() {
    let framework_arn = official_lens_arn(REGION, FRAMEWORK_LENS);
    let [first_pillar, second_pillar] = FRAMEWORK_PILLARS;
    let rows: [(&str, &str, &str, Value); 7] = [
        (
            "lens-notes",
            FRAMEWORK_LENS,
            FRAMEWORK_LENS,
            json!({ "LensNotes": "the notes the template applies" }),
        ),
        (
            "lens-notes-empty",
            FRAMEWORK_LENS,
            FRAMEWORK_LENS,
            json!({ "LensNotes": "" }),
        ),
        (
            "pillar-notes",
            FRAMEWORK_LENS,
            FRAMEWORK_LENS,
            json!({ "PillarNotes": { first_pillar: "a pillar note" } }),
        ),
        (
            "pillar-notes-every-pillar",
            FRAMEWORK_LENS,
            FRAMEWORK_LENS,
            json!({
                "PillarNotes": {
                    first_pillar: "one",
                    second_pillar: "another",
                }
            }),
        ),
        (
            "pillar-notes-empty-map",
            FRAMEWORK_LENS,
            FRAMEWORK_LENS,
            json!({ "PillarNotes": {} }),
        ),
        (
            "both-members-by-lens-arn",
            FRAMEWORK_LENS,
            framework_arn.as_str(),
            json!({
                "LensNotes": "both members at once",
                "PillarNotes": { second_pillar: "a pillar note" },
            }),
        ),
        (
            "lens-without-shipped-content",
            UNSHIPPED_LENS,
            UNSHIPPED_LENS,
            json!({ "LensNotes": "a note on a lens with no shipped pillars" }),
        ),
    ];

    for (case, applied_lens, spelling, body) in rows {
        let client = Client::new();
        let template_arn = review_template(&client, "write-instants", &[applied_lens]).await;
        age_review_template(&client, &template_arn);

        let before_template = template_updated_at(&client, &template_arn, case).await;
        let before_summary = summary_updated_at(&client, &template_arn, case).await;
        let before_review =
            lens_review_updated_at(&client, &template_arn, applied_lens, case).await;
        assert_eq!(
            before_summary, before_template,
            "case {case:?}: the two projections of the template must agree before the write, \
             got {before_summary} and {before_template}"
        );

        let written = client
            .update_review_template_lens_review(&template_arn, spelling, body)
            .await;
        let at = written_updated_at(&written, case);
        assert!(
            at > before_review,
            "case {case:?}: the lens review was written, so its UpdatedAt must move past \
             {before_review}, got {at}"
        );

        let after_template = template_updated_at(&client, &template_arn, case).await;
        assert!(
            after_template > before_template,
            "case {case:?}: writing a lens review changes the template, so its UpdatedAt must \
             move past {before_template}, got {after_template}"
        );
        assert_eq!(
            after_template, at,
            "case {case:?}: the template must report the instant of the write {at}, got \
             {after_template}"
        );
        let after_summary = summary_updated_at(&client, &template_arn, case).await;
        assert_eq!(
            after_summary, at,
            "case {case:?}: the listed summary must report the instant of the write {at}, got \
             {after_summary}"
        );
        let after_review =
            lens_review_updated_at(&client, &template_arn, applied_lens, case).await;
        assert_eq!(
            after_review, at,
            "case {case:?}: a read must report the write's instant {at}, got {after_review}"
        );
    }
}

/// The same rule over the template's OTHER nested write path: answering one of the
/// questions its lens review asks also moves the template's own `UpdatedAt`, to the same
/// instant the lens review reports — so the two write paths of one record agree.
///
/// The rows span the questions answered and what the answer carries.
#[tokio::test]
async fn answering_a_templates_question_moves_the_templates_updated_at() {
    let rows: [(&str, Value); 3] = [
        ("notes", json!({ "Notes": "answered with a note" })),
        ("selected-choices-empty", json!({ "SelectedChoices": [] })),
        (
            "not-applicable",
            json!({ "IsApplicable": false, "Reason": "OUT_OF_SCOPE" }),
        ),
    ];

    for (row, body) in rows {
        for question_id in FRAMEWORK_QUESTIONS {
            let case = format!("{question_id}/{row}");
            let client = Client::new();
            let template_arn =
                review_template(&client, "answer-instants", &[FRAMEWORK_LENS]).await;
            age_review_template(&client, &template_arn);

            let before = template_updated_at(&client, &template_arn, &case).await;
            let answered = client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    body.clone(),
                )
                .await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: expected UpdateReviewTemplateAnswer 200, got {answered:?}"
            );

            let after = template_updated_at(&client, &template_arn, &case).await;
            assert!(
                after > before,
                "case {case:?}: answering a question changes the template, so its UpdatedAt \
                 must move past {before}, got {after}"
            );
            let review =
                lens_review_updated_at(&client, &template_arn, FRAMEWORK_LENS, &case).await;
            assert_eq!(
                review, after,
                "case {case:?}: the template and the lens review it wrote must report one \
                 instant, got {after} and {review}"
            );
        }
    }
}

/// The other side of the rule: a lens-review update that supplies NEITHER note member asks
/// for no change, so it moves neither the lens review's `UpdatedAt` nor the template's — in
/// either projection.
///
/// The rows differ along what the template holds when the no-op arrives.
#[tokio::test]
async fn a_lens_review_update_that_writes_nothing_moves_neither_instant() {
    let [first_pillar, _] = FRAMEWORK_PILLARS;
    let rows: [(&str, Option<Value>); 3] = [
        ("as-created", None),
        ("after-lens-notes", Some(json!({ "LensNotes": "a note" }))),
        (
            "after-pillar-notes",
            Some(json!({ "PillarNotes": { first_pillar: "a pillar note" } })),
        ),
    ];

    for (case, prior) in rows {
        let client = Client::new();
        let template_arn = review_template(&client, "no-write-instants", &[FRAMEWORK_LENS]).await;
        if let Some(body) = prior {
            let seeded = client
                .update_review_template_lens_review(&template_arn, FRAMEWORK_LENS, body)
                .await;
            assert_eq!(
                seeded.status, 200,
                "case {case:?}: the seeding write must be served, got {seeded:?}"
            );
        }
        age_review_template(&client, &template_arn);

        let before_template = template_updated_at(&client, &template_arn, case).await;
        let before_summary = summary_updated_at(&client, &template_arn, case).await;
        let before_review =
            lens_review_updated_at(&client, &template_arn, FRAMEWORK_LENS, case).await;

        let patched = client
            .update_review_template_lens_review(&template_arn, FRAMEWORK_LENS, json!({}))
            .await;
        assert_eq!(
            written_updated_at(&patched, case),
            before_review,
            "case {case:?}: an update that supplies no member reports the lens review as it \
             stands, got {patched:?}"
        );

        let after_template = template_updated_at(&client, &template_arn, case).await;
        assert_eq!(
            after_template, before_template,
            "case {case:?}: nothing was written, so the template's UpdatedAt must stay at \
             {before_template}, got {after_template}"
        );
        let after_summary = summary_updated_at(&client, &template_arn, case).await;
        assert_eq!(
            after_summary, before_summary,
            "case {case:?}: nothing was written, so the listed summary's UpdatedAt must stay \
             at {before_summary}, got {after_summary}"
        );
        let after_review =
            lens_review_updated_at(&client, &template_arn, FRAMEWORK_LENS, case).await;
        assert_eq!(
            after_review, before_review,
            "case {case:?}: nothing was written, so the lens review's UpdatedAt must stay at \
             {before_review}, got {after_review}"
        );
    }
}
