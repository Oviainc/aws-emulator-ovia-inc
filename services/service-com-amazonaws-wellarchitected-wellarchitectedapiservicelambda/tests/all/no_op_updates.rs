// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the update that writes nothing.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: an update
//! whose every WRITABLE member is optional treats a request that supplies NONE of them as
//! asking for no change — it brings no record into existence and moves no `UpdatedAt`, and
//! reports the resource exactly as it stands — while a member the request DOES supply is a
//! write even when the value it carries is empty.
//!
//! The suites cover the three operations in this crate the rule reaches:
//! `UpdateAnswer` (whose five writable members are all optional, so an all-absent request
//! must leave the question `UNANSWERED`, the workload's risk roll-up where it was and the
//! question out of the improvement plan), `UpdateReviewTemplateAnswer` (the same five
//! members over a review template) and `UpdateWorkload`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::workload_harness::{
    age_workload, created_id, encode_path_segment, minimal_create, review_template,
    stored_workload, Answer, Client,
};

/// The alias of the one AWS-official lens the resources under test review.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// How far into the past a stored `UpdatedAt` is moved before the request under test, so a
/// write the handler makes to it is observable whatever the clock's granularity between two
/// in-process requests — and so a write it must NOT make cannot hide behind that
/// granularity either.
const AGE: Duration = Duration::from_secs(3600);

/// `UpdateAnswer`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
async fn update_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(body),
        )
        .await
}

/// `GetAnswer`: the same path read with `GET`.
async fn get_answer(client: &Client, workload_id: &str, lens: &str, question_id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// `ListLensReviewImprovements`:
/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/improvements`.
async fn improvements(client: &Client, workload_id: &str, lens: &str, case: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/improvements",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviewImprovements 200, got {answer:?}"
    );
    answer
        .body
        .get("ImprovementSummaries")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A workload reviewing the framework lens, and its `WorkloadId`.
async fn framework_workload(client: &Client, name: &str) -> String {
    created_id(
        &client
            .create(minimal_create(name, &format!("tok-{name}")))
            .await,
        name,
    )
}

/// The `Answer` / `ReviewTemplateAnswer` an answer read or an answer write reported.
fn answer_of(answer: &Answer) -> Value {
    answer.body.get("Answer").cloned().unwrap_or(Value::Null)
}

/// One string member of a JSON object, `None` when it is absent.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One timestamp member of a JSON object, as the epoch seconds restJson1 carries it.
fn timestamp(value: &Value, member: &str, case: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be present, got {value:?}"))
}

/// The `ChoiceId`s the question asks about, in the order its own catalog publishes them —
/// where a client discovers them too.
fn choice_ids(answer: &Value) -> Vec<String> {
    answer
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| text(choice, "ChoiceId"))
                .collect()
        })
        .unwrap_or_default()
}

/// What a row records against the question BEFORE the no-op update, so the rule is pinned
/// over an unanswered question and over answers in each state one can be in.
#[derive(Clone, Copy, Debug)]
enum Seed {
    /// Nothing has answered the question.
    Nothing,
    /// Exactly one best practice is in place.
    FirstChoice,
    /// Every best practice the question asks about is in place.
    EveryChoice,
    /// The question does not apply to this review.
    NotApplicable,
    /// The question is answered with no best practice in place.
    NotesOnly,
}

impl Seed {
    /// The `UpdateAnswer` / `UpdateReviewTemplateAnswer` body that records this seed, or
    /// `None` for the question nothing answers.
    fn body(self, choices: &[String]) -> Option<Value> {
        match self {
            Seed::Nothing => None,
            Seed::FirstChoice => Some(json!({ "SelectedChoices": &choices[..1] })),
            Seed::EveryChoice => Some(json!({ "SelectedChoices": choices })),
            Seed::NotApplicable => {
                Some(json!({ "IsApplicable": false, "Reason": "OUT_OF_SCOPE" }))
            }
            Seed::NotesOnly => Some(json!({ "Notes": "answered with nothing in place" })),
        }
    }
}

/// The questions these suites answer, chosen from three pillars of the framework lens's
/// published numbering and differing in HOW MANY best practices they ask about — `sus-01`
/// asks about exactly one, so "the first choice" and "every choice" are one selection
/// there.
const QUESTIONS: [&str; 3] = ["organization-priorities", "identities", "region-selection"];

/// A request that supplies no writable member records no answer and moves nothing: the
/// question stays `UNANSWERED` where nothing has answered it, an answered question is
/// reported exactly as stored, and the workload — its `UpdatedAt`, its risk roll-up — and
/// the improvement plan are left as they stand.
///
/// The rows span the questions above crossed with every state the answer can be in before
/// the request.
#[tokio::test]
async fn an_answer_update_that_supplies_no_member_writes_nothing() {
    let seeds = [
        Seed::Nothing,
        Seed::FirstChoice,
        Seed::EveryChoice,
        Seed::NotApplicable,
        Seed::NotesOnly,
    ];

    for question_id in QUESTIONS {
        for seed in seeds {
            let case = format!("{question_id}/{seed:?}");
            let client = Client::new();
            let workload_id = framework_workload(&client, "no-op-answer").await;

            // The question's own choice catalog is read first, so no `ChoiceId` is written
            // down here.
            let catalog = answer_of(&get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id).await);
            if let Some(body) = seed.body(&choice_ids(&catalog)) {
                let seeded =
                    update_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, body).await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case:?}: the seeding update must be served, got {seeded:?}"
                );
            }

            // Aged, so a write to `UpdatedAt` the handler must not make would show.
            age_workload(&client, &workload_id, AGE);

            let before_answer =
                answer_of(&get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id).await);
            let before_workload = stored_workload(&client, &workload_id, &case).await;
            let before_plan = improvements(&client, &workload_id, FRAMEWORK_LENS, &case).await;

            let patched =
                update_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, json!({})).await;
            assert_eq!(
                patched.status, 200,
                "case {case:?}: every writable member is optional, so an empty body is \
                 served, got {patched:?}"
            );
            assert_eq!(
                answer_of(&patched),
                before_answer,
                "case {case:?}: expected the answer as it stands {before_answer:?}, got {:?}",
                answer_of(&patched)
            );

            let after_answer =
                answer_of(&get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id).await);
            assert_eq!(
                after_answer, before_answer,
                "case {case:?}: expected the stored answer unchanged {before_answer:?}, got \
                 {after_answer:?}"
            );

            let after_workload = stored_workload(&client, &workload_id, &case).await;
            assert_eq!(
                timestamp(&after_workload, "UpdatedAt", &case),
                timestamp(&before_workload, "UpdatedAt", &case),
                "case {case:?}: expected UpdatedAt {}, got {}",
                timestamp(&before_workload, "UpdatedAt", &case),
                timestamp(&after_workload, "UpdatedAt", &case)
            );
            assert_eq!(
                after_workload.get("RiskCounts"),
                before_workload.get("RiskCounts"),
                "case {case:?}: expected the risk roll-up {:?}, got {:?}",
                before_workload.get("RiskCounts"),
                after_workload.get("RiskCounts")
            );
            assert_eq!(
                after_workload, before_workload,
                "case {case:?}: expected the workload as it stands {before_workload:?}, got \
                 {after_workload:?}"
            );

            let after_plan = improvements(&client, &workload_id, FRAMEWORK_LENS, &case).await;
            assert_eq!(
                after_plan, before_plan,
                "case {case:?}: expected the improvement plan {before_plan:?}, got {after_plan:?}"
            );
        }
    }
}

/// The other side of the rule: a member the request DOES supply is a write even when the
/// value it carries is empty, so each row records an answer — the question leaves
/// `UNANSWERED` for the `Risk` its record derives — and moves the workload's `UpdatedAt`.
#[tokio::test]
async fn a_supplied_answer_member_writes_even_when_its_value_is_empty() {
    // Each row supplies exactly ONE of the five writable members, at the emptiest value the
    // model allows it: `SelectedChoices` naming no choice, `ChoiceUpdates` carrying no
    // entry, `Notes` as the empty string (`@length(max: 2084)` sets no minimum),
    // `IsApplicable` either way and `Reason` at the `NONE` member `AnswerReason` publishes.
    let rows: [(&str, Value, &str); 6] = [
        ("selected-choices-empty", json!({ "SelectedChoices": [] }), "HIGH"),
        ("choice-updates-empty", json!({ "ChoiceUpdates": {} }), "HIGH"),
        ("notes-empty", json!({ "Notes": "" }), "HIGH"),
        ("is-applicable-true", json!({ "IsApplicable": true }), "HIGH"),
        (
            "is-applicable-false",
            json!({ "IsApplicable": false }),
            "NOT_APPLICABLE",
        ),
        ("reason-none", json!({ "Reason": "NONE" }), "HIGH"),
    ];

    for (row, body, expected_risk) in rows {
        for question_id in QUESTIONS {
            let case = format!("{question_id}/{row}");
            let client = Client::new();
            let workload_id = framework_workload(&client, "supplied-member").await;
            age_workload(&client, &workload_id, AGE);

            let before = stored_workload(&client, &workload_id, &case).await;
            let patched =
                update_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, body.clone())
                    .await;
            assert_eq!(
                patched.status, 200,
                "case {case:?}: expected UpdateAnswer 200, got {patched:?}"
            );
            let answer = answer_of(&patched);
            assert_eq!(
                text(&answer, "Risk").as_deref(),
                Some(expected_risk),
                "case {case:?}: expected Risk {expected_risk}, got {answer:?}"
            );

            // The write is persisted, not just echoed.
            let read_back =
                answer_of(&get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id).await);
            assert_eq!(
                read_back, answer,
                "case {case:?}: GetAnswer must report the recorded answer {answer:?}, got \
                 {read_back:?}"
            );

            let after = stored_workload(&client, &workload_id, &case).await;
            assert!(
                timestamp(&after, "UpdatedAt", &case) > timestamp(&before, "UpdatedAt", &case),
                "case {case:?}: a supplied member is a write, so UpdatedAt must move past {}, \
                 got {}",
                timestamp(&before, "UpdatedAt", &case),
                timestamp(&after, "UpdatedAt", &case)
            );
        }
    }
}

/// Move a stored review template's `UpdatedAt`, and that of every lens review it holds,
/// back by `age`, so a write the handler makes to either is observable whatever the
/// clock's granularity between two in-process requests. Only the SEED reaches state
/// directly; the write under test is the handler's.
fn age_review_template(client: &Client, template_arn: &str, age: Duration) {
    let aged = SystemTime::now() - age;
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

/// The same rule over a review template: a request that supplies no writable member leaves
/// the question `UNANSWERED` where nothing has answered it, reports an answered question as
/// stored, and moves neither the template's `UpdatedAt`, nor its lens review's, nor its
/// question roll-up — while a supplied member, however empty, writes.
#[tokio::test]
async fn a_review_template_answer_update_that_supplies_no_member_writes_nothing() {
    let seeds = [
        Seed::Nothing,
        Seed::FirstChoice,
        Seed::NotApplicable,
        Seed::NotesOnly,
    ];

    for question_id in QUESTIONS {
        for seed in seeds {
            let case = format!("{question_id}/{seed:?}");
            let client = Client::new();
            let template_arn = review_template(&client, "no-op-template", &[FRAMEWORK_LENS]).await;

            let catalog = answer_of(
                &client
                    .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                    .await,
            );
            if let Some(body) = seed.body(&choice_ids(&catalog)) {
                let seeded = client
                    .update_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id, body)
                    .await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case:?}: the seeding update must be served, got {seeded:?}"
                );
            }

            age_review_template(&client, &template_arn, AGE);

            let before_answer = answer_of(
                &client
                    .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                    .await,
            );
            let before_template = template_of(&client, &template_arn, &case).await;
            let before_review = template_lens_review(&client, &template_arn, &case).await;

            let patched = client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    json!({}),
                )
                .await;
            assert_eq!(
                patched.status, 200,
                "case {case:?}: every writable member is optional, so an empty body is \
                 served, got {patched:?}"
            );
            assert_eq!(
                answer_of(&patched),
                before_answer,
                "case {case:?}: expected the answer as it stands {before_answer:?}, got {:?}",
                answer_of(&patched)
            );

            let after_answer = answer_of(
                &client
                    .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                    .await,
            );
            assert_eq!(
                after_answer, before_answer,
                "case {case:?}: expected the stored answer unchanged {before_answer:?}, got \
                 {after_answer:?}"
            );

            let after_template = template_of(&client, &template_arn, &case).await;
            assert_eq!(
                after_template, before_template,
                "case {case:?}: expected the template as it stands {before_template:?}, got \
                 {after_template:?}"
            );
            let after_review = template_lens_review(&client, &template_arn, &case).await;
            assert_eq!(
                timestamp(&after_review, "UpdatedAt", &case),
                timestamp(&before_review, "UpdatedAt", &case),
                "case {case:?}: expected the lens review's UpdatedAt {}, got {}",
                timestamp(&before_review, "UpdatedAt", &case),
                timestamp(&after_review, "UpdatedAt", &case)
            );

            // The other side of the rule: one supplied member, carrying nothing, answers
            // the question and moves both instants.
            let written = client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    json!({ "SelectedChoices": [] }),
                )
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected UpdateReviewTemplateAnswer 200, got {written:?}"
            );
            assert_eq!(
                text(&answer_of(&written), "AnswerStatus").as_deref(),
                Some("ANSWERED"),
                "case {case:?}: a supplied member records the answer, got {written:?}"
            );
            let written_template = template_of(&client, &template_arn, &case).await;
            assert!(
                timestamp(&written_template, "UpdatedAt", &case)
                    > timestamp(&before_template, "UpdatedAt", &case),
                "case {case:?}: a supplied member is a write, so the template's UpdatedAt \
                 must move past {}, got {}",
                timestamp(&before_template, "UpdatedAt", &case),
                timestamp(&written_template, "UpdatedAt", &case)
            );
        }
    }
}

/// The `ReviewTemplate` a template read reports.
async fn template_of(client: &Client, template_arn: &str, case: &str) -> Value {
    let answer = client.get_review_template(template_arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("ReviewTemplate")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `ReviewTemplateLensReview` a template's lens-review read reports.
async fn template_lens_review(client: &Client, template_arn: &str, case: &str) -> Value {
    let answer = client
        .get_review_template_lens_review(template_arn, FRAMEWORK_LENS)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplateLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or(Value::Null)
}

/// `UpdateWorkload` follows the same rule: every member it can write is optional, so a
/// request that supplies none reports the workload as it stands and leaves its `UpdatedAt`
/// where it was — while a supplied member, however empty, moves it.
///
/// The rows differ along what the workload holds when the request arrives.
#[tokio::test]
async fn a_workload_update_that_supplies_no_member_writes_nothing() {
    // Each row is a prior update, so the no-op lands on a workload in a different state:
    // one straight from the create, one whose notes were written, and one renamed (the
    // member that also moves the name reservation).
    let rows: [(&str, Option<Value>); 3] = [
        ("as-created", None),
        ("after-notes", Some(json!({ "Notes": "a note" }))),
        (
            "after-a-rename",
            Some(json!({ "WorkloadName": "no-op-renamed" })),
        ),
    ];

    for (case, prior) in rows {
        let client = Client::new();
        let workload_id = framework_workload(&client, "no-op-workload").await;
        if let Some(body) = prior {
            let seeded = client.update(&workload_id, body).await;
            assert_eq!(
                seeded.status, 200,
                "case {case:?}: the seeding update must be served, got {seeded:?}"
            );
        }
        age_workload(&client, &workload_id, AGE);

        let before = stored_workload(&client, &workload_id, case).await;
        let patched = client.update(&workload_id, json!({})).await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: every writable member is optional, so an empty body is served, \
             got {patched:?}"
        );
        let reported = patched
            .body
            .get("Workload")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            reported, before,
            "case {case:?}: expected the workload as it stands {before:?}, got {reported:?}"
        );
        let after = stored_workload(&client, &workload_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the stored workload unchanged {before:?}, got {after:?}"
        );

        // The other side of the rule: one supplied member moves `UpdatedAt`.
        let written = client
            .update(&workload_id, json!({ "Notes": "" }))
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected UpdateWorkload 200, got {written:?}"
        );
        let after = stored_workload(&client, &workload_id, case).await;
        assert!(
            timestamp(&after, "UpdatedAt", case) > timestamp(&before, "UpdatedAt", case),
            "case {case:?}: a supplied member is a write, so UpdatedAt must move past {}, got {}",
            timestamp(&before, "UpdatedAt", case),
            timestamp(&after, "UpdatedAt", case)
        );
    }
}
