// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH version of a lens's content a lens review reports.
//!
//! The rule these pin, over a table of lens documents rather than one recorded request: a
//! lens review — of a workload or of a review template — is a review against ONE version
//! of the lens, so everything it reports about the lens is read off the content of the
//! version it is pinned at: its `LensName`, its pillar review summaries and the roll-up
//! over the questions it asks. Publishing a later version of the lens does not move any of
//! them; the upgrade that moves the review to the lens's latest version is what does, and a
//! milestone keeps the review as it stood.
//!
//! The rows differ along how the later version's document DIFFERS from the pinned one —
//! more questions, fewer questions, a pillar added, a pillar dropped — and along how many
//! later versions were published and the form of their version names, because the question
//! "which version's content is this?" is only answerable where the two differ.
//!
//! The reads covered are the single lens-review read (`GetLensReview`), the review listing
//! (`ListLensReviews`), the answer list and single answer read of that review
//! (`ListAnswers`, `GetAnswer`), the workload's own roll-up (`GetWorkload`), and the review
//! template counterparts (`GetReviewTemplateLensReview`, `ListReviewTemplateAnswers`).
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the workload seeding are the shared
//! `super::workload_harness`; the import, publish and document helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::custom_lens_import::{import, imported_arn, lens_document, wait_for_the_import_to_land};
use super::custom_lens_review_answers::{expected_questions, question_count, Shape};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, review_template, stored_workload, Answer,
    Client,
};

/// The alias of the AWS Well-Architected Framework lens — the official half of the catalog
/// every row attaches beside its custom lens, so a roll-up that counted the wrong half
/// would fail.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// How many questions that lens asks: the published Well-Architected Framework numbering
/// (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6), spelled out here rather
/// than read back out of the code under test.
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// The `LensVersion` every row publishes its lens as FIRST — the version each review under
/// test is pinned at.
const PINNED_VERSION: &str = "1.0";

/// The two `LensStatus` members a lens review's currency is reported with
/// (`smithy_query(query="shape", shape="LensStatus")` publishes `CURRENT`, `NOT_CURRENT`,
/// `DEPRECATED`, `DELETED` and `UNSHARED`).
const CURRENT: &str = "CURRENT";
const NOT_CURRENT: &str = "NOT_CURRENT";

/// Every member the `Risk` enum publishes (`smithy_query(query="shape", shape="Risk")`), so
/// a roll-up is checked for ALL its buckets rather than the one a case moved.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// The `Question` buckets a `QuestionCounts` map carries
/// (`smithy_query(query="shape", shape="Question")`).
const QUESTION_BUCKETS: [&str; 2] = ["ANSWERED", "UNANSWERED"];

/// One row of the table: the document the review is pinned at, and the documents published
/// after it, each with the `LensVersion` it is published as.
struct Row {
    case: &'static str,
    /// The shape of the document version [`PINNED_VERSION`] carries.
    pinned: Shape,
    /// The versions published AFTER the review was created, in publication order:
    /// `(LensVersion, the shape of that version's document)`.
    later: &'static [(&'static str, Shape)],
}

/// The rows: each pairs the pinned document with later ones that differ from it along a
/// different axis, so a read that followed the lens's LATEST content rather than the
/// review's own version fails on every row.
const ROWS: &[Row] = &[
    Row {
        case: "a-later-version-asks-more-questions",
        pinned: &[("governance", 2, 2)],
        later: &[("2.0", &[("governance", 5, 2)])],
    },
    Row {
        case: "a-later-version-asks-fewer-questions",
        pinned: &[("security", 4, 1), ("cost", 3, 2)],
        later: &[("2.0", &[("security", 1, 1)])],
    },
    Row {
        case: "a-later-version-adds-a-pillar",
        pinned: &[("operations", 1, 3)],
        later: &[("2024-01-01", &[("operations", 1, 3), ("resilience", 2, 2)])],
    },
    Row {
        case: "two-later-versions",
        pinned: &[("reliability", 3, 2)],
        later: &[
            ("2.0", &[("reliability", 6, 1)]),
            (
                "3.0",
                &[("reliability", 2, 1), ("sustainability", 4, 2)],
            ),
        ],
    },
];

/// The `PillarName` a document gives the pillar at `index`, mirroring what
/// `super::custom_lens_import::lens_document` writes, so the expectation comes from the
/// document the test sent.
fn expected_pillar_name(index: usize, pillar_id: &str) -> String {
    format!("Pillar {index}: {pillar_id}")
}

/// The name the document of `case` at `version` gives its lens: every version of a row's
/// lens is named differently, so the name a review reports proves WHICH version it read.
fn expected_lens_name(case: &str, version: &str) -> String {
    format!("Lens {case} at {version}")
}

/// `(PillarId, PillarName, questions in it)` for every pillar a document of `shape`
/// defines, in the document's own order.
fn expected_pillars(shape: Shape) -> Vec<(String, String, usize)> {
    shape
        .iter()
        .enumerate()
        .map(|(index, (pillar_id, questions, _))| {
            (
                (*pillar_id).to_string(),
                expected_pillar_name(index, pillar_id),
                *questions,
            )
        })
        .collect()
}

/// The `QuestionId`s a document of `shape` defines, in the document's own order.
fn expected_question_ids(shape: Shape) -> Vec<String> {
    expected_questions(shape)
        .into_iter()
        .map(|question| question.question_id)
        .collect()
}

/// Import a document of `shape` under `version`'s name as a FRESH lens, wait for the import
/// to land and publish it as `version`. Answers the lens ARN.
async fn import_and_publish(client: &Client, case: &str, version: &str, shape: Shape) -> String {
    let document = lens_document(&expected_lens_name(case, version), None, shape).to_string();
    let imported = import(client, &document, None, &format!("tok-import-{case}"), None).await;
    let arn = imported_arn(&imported, case);
    wait_for_the_import_to_land();
    publish(client, case, &arn, version).await;
    arn
}

/// Re-import `arn` with a document of `shape` and publish it as `version`: the publish that
/// supersedes whatever version a review of this lens is pinned at.
async fn publish_later(client: &Client, case: &str, arn: &str, version: &str, shape: Shape) {
    let document = lens_document(&expected_lens_name(case, version), None, shape).to_string();
    let update = import(
        client,
        &document,
        Some(arn),
        &format!("tok-reimport-{case}-{version}"),
        None,
    )
    .await;
    assert_eq!(
        update.status, 200,
        "case {case:?}: the update import of {version} must be accepted, got {update:?}"
    );
    wait_for_the_import_to_land();
    publish(client, case, arn, version).await;
}

/// `CreateLensVersion`: `POST /lenses/{LensAlias}/versions`.
async fn publish(client: &Client, case: &str, arn: &str, version: &str) {
    let published = create_version(
        client,
        arn,
        version,
        Some(true),
        &format!("tok-publish-{case}-{version}"),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: publishing {version} must succeed, got {published:?}"
    );
}

/// A workload reviewing `lenses`, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, case: &str, lenses: &[&str]) -> String {
    let mut request = minimal_create(&format!("wl-{case}"), &format!("tok-wl-{case}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, case)
}

/// One string member of a JSON object, the empty string when the object does not carry it.
fn string_member(owner: &Value, name: &str) -> String {
    owner
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One count map of a response (`RiskCounts` / `QuestionCounts`), as a sorted bucket table.
fn counts(owner: &Value, member: &str, case: &str) -> BTreeMap<String, i64> {
    owner
        .get(member)
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(bucket, count)| (bucket.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be present, got {owner:?}"))
}

/// The roll-up expected over `total` questions none of which carries an answer: every
/// bucket the `Risk` enum publishes is present and only `UNANSWERED` is non-zero.
fn all_unanswered_risks(total: usize) -> BTreeMap<String, i64> {
    let mut expected: BTreeMap<String, i64> =
        RISKS.iter().map(|risk| ((*risk).to_string(), 0)).collect();
    expected.insert(
        "UNANSWERED".to_string(),
        i64::try_from(total).unwrap_or(i64::MAX),
    );
    expected
}

/// The question roll-up expected over `total` questions none of which carries an answer.
fn all_unanswered_questions(total: usize) -> BTreeMap<String, i64> {
    QUESTION_BUCKETS
        .iter()
        .map(|bucket| {
            let count = if *bucket == "ANSWERED" {
                0
            } else {
                i64::try_from(total).unwrap_or(i64::MAX)
            };
            ((*bucket).to_string(), count)
        })
        .collect()
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`, optionally scoped
/// to a milestone. The lens travels percent-encoded, because a custom lens is named by its
/// ARN — which carries `:` and `/` inside ONE path segment.
async fn get_lens_review(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone: Option<i64>,
    case: &str,
) -> Value {
    let query = milestone
        .map(|number| format!("?MilestoneNumber={number}"))
        .unwrap_or_default();
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}{query}",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// The `LensReviewSummary` `ListLensReviews` reports for one lens of a workload.
async fn listed_lens_review(client: &Client, workload_id: &str, lens: &str, case: &str) -> Value {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| string_member(summary, "LensAlias") == lens)
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: a summary of lens {lens:?} must be listed, got {answer:?}")
        })
}

/// Every `QuestionId` `ListAnswers` reports for one lens review, following `NextToken` to
/// the end, in the order they were served.
async fn listed_answer_ids(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone: Option<i64>,
    case: &str,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(number) = milestone {
            query.push(format!("MilestoneNumber={number}"));
        }
        if let Some(token) = &next_token {
            query.push(format!("NextToken={}", encode_path_segment(token)));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let answer = client
            .send(
                "GET",
                &format!(
                    "/workloads/{}/lensReviews/{}/answers{suffix}",
                    encode_path_segment(workload_id),
                    encode_path_segment(lens)
                ),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListAnswers 200, got {answer:?}"
        );
        collected.extend(
            answer
                .body
                .get("AnswerSummaries")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .map(|summary| string_member(summary, "QuestionId")),
        );
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
}

/// `GetAnswer`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
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

/// `UpgradeLensReview`: `PUT /workloads/{WorkloadId}/lensReviews/{LensAlias}/upgrade`.
async fn upgrade_lens_review(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone_name: &str,
    case: &str,
) {
    let answer = client
        .send(
            "PUT",
            &format!(
                "/workloads/{}/lensReviews/{}/upgrade",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            Some(json!({ "MilestoneName": milestone_name })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpgradeLensReview 200, got {answer:?}"
    );
}

/// Assert that a workload's lens review reports the content of the version `version` of its
/// lens carries — its name, its pillar summaries and the roll-up over the questions that
/// version's document of `shape` asks — reported identically by the single read and by the
/// listing.
async fn assert_review_reports(
    client: &Client,
    workload_id: &str,
    lens: &str,
    case: &str,
    version: &str,
    status: &str,
    shape: Shape,
) {
    let review = get_lens_review(client, workload_id, lens, None, case).await;
    let asked = question_count(shape);
    assert_eq!(
        (
            string_member(&review, "LensName"),
            string_member(&review, "LensVersion"),
            string_member(&review, "LensStatus"),
        ),
        (
            expected_lens_name(case, version),
            version.to_string(),
            status.to_string(),
        ),
        "case {case:?}: the review must identify the lens by the version it is against, got \
         {review:?}"
    );
    assert_eq!(
        counts(&review, "RiskCounts", case),
        all_unanswered_risks(asked),
        "case {case:?}: the review must count the {asked} questions version {version} asks, \
         got {review:?}"
    );

    let summaries = review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
        });
    let expected = expected_pillars(shape);
    assert_eq!(
        summaries.len(),
        expected.len(),
        "case {case:?}: the review must report one summary per pillar version {version} \
         defines, got {summaries:?}"
    );
    for (summary, (pillar_id, pillar_name, questions)) in summaries.iter().zip(&expected) {
        assert_eq!(
            (
                string_member(summary, "PillarId"),
                string_member(summary, "PillarName"),
            ),
            (pillar_id.clone(), pillar_name.clone()),
            "case {case:?}: the pillar summaries must follow version {version}'s own order \
             and names, got {summaries:?}"
        );
        assert_eq!(
            counts(summary, "RiskCounts", case),
            all_unanswered_risks(*questions),
            "case {case:?}: pillar {pillar_id:?} must count its own {questions} questions of \
             version {version}, got {summary:?}"
        );
    }

    // The listing of the same review cannot disagree with the single read.
    let listed = listed_lens_review(client, workload_id, lens, case).await;
    assert_eq!(
        (
            string_member(&listed, "LensName"),
            string_member(&listed, "LensVersion"),
            string_member(&listed, "LensStatus"),
            counts(&listed, "RiskCounts", case),
        ),
        (
            string_member(&review, "LensName"),
            string_member(&review, "LensVersion"),
            string_member(&review, "LensStatus"),
            counts(&review, "RiskCounts", case),
        ),
        "case {case:?}: the listing and the single read must agree about the review, got \
         {listed:?}"
    );

    // The answers of the review are that version's questions, in its own order.
    assert_eq!(
        listed_answer_ids(client, workload_id, lens, None, case).await,
        expected_question_ids(shape),
        "case {case:?}: the answer list must be version {version}'s own questions"
    );

    // The workload's own roll-up spans every lens it reviews, each counted at the version
    // ITS review is against.
    let workload = stored_workload(client, workload_id, case).await;
    assert_eq!(
        counts(&workload, "RiskCounts", case),
        all_unanswered_risks(asked + FRAMEWORK_QUESTIONS),
        "case {case:?}: the workload must count version {version}'s {asked} questions beside \
         the framework lens's {FRAMEWORK_QUESTIONS}, got {workload:?}"
    );
}

/// Rule 1. A workload's lens review reports the content of the lens VERSION it is pinned
/// at: publishing a later version of the lens moves neither its name, nor its pillar
/// summaries, nor the questions its roll-up and answer list are taken over — only the
/// upgrade does, and the milestone the upgrade records keeps the review as it stood.
#[tokio::test]
async fn a_lens_review_reports_the_content_of_the_version_it_is_against() {
    for row in ROWS {
        let case = row.case;
        let client = Client::new();
        let arn = import_and_publish(&client, case, PINNED_VERSION, row.pinned).await;
        let workload_id = workload_reviewing(&client, case, &[arn.as_str(), FRAMEWORK_LENS]).await;

        // A fresh review is against the version the lens was associated at, which is the
        // lens's latest, so it is CURRENT.
        assert_review_reports(
            &client,
            &workload_id,
            &arn,
            case,
            PINNED_VERSION,
            CURRENT,
            row.pinned,
        )
        .await;

        // Every later publish supersedes the version the review is against and carries a
        // DIFFERENT document, so what the review reports proves which version it reads.
        for (version, shape) in row.later {
            publish_later(&client, case, &arn, version, shape).await;
            assert_review_reports(
                &client,
                &workload_id,
                &arn,
                case,
                PINNED_VERSION,
                NOT_CURRENT,
                row.pinned,
            )
            .await;

            // A question only the published version asks belongs to no answer of this
            // review — a value of `QuestionId` this service refuses — and every question the
            // pinned version asks still resolves.
            let pinned_ids = expected_question_ids(row.pinned);
            for question_id in expected_question_ids(shape) {
                let answer = get_answer(&client, &workload_id, &arn, &question_id).await;
                let expected = if pinned_ids.contains(&question_id) {
                    200
                } else {
                    400
                };
                assert_eq!(
                    answer.status, expected,
                    "case {case:?}: question {question_id:?} of version {version} must answer \
                     {expected} on a review against {PINNED_VERSION}, got {answer:?}"
                );
            }
        }

        // The upgrade moves the review to the lens's latest version, so every read of it now
        // reports THAT version's content.
        let Some((latest_version, latest_shape)) = row.later.last() else {
            continue;
        };
        upgrade_lens_review(
            &client,
            &workload_id,
            &arn,
            &format!("before upgrading {case}"),
            case,
        )
        .await;
        assert_review_reports(
            &client,
            &workload_id,
            &arn,
            case,
            latest_version,
            CURRENT,
            latest_shape,
        )
        .await;

        // The milestone the upgrade recorded froze the review as it stood, so a read scoped
        // to it still asks the version it was against then.
        let frozen = get_lens_review(&client, &workload_id, &arn, Some(1), case).await;
        assert_eq!(
            (
                string_member(&frozen, "LensVersion"),
                counts(&frozen, "RiskCounts", case),
            ),
            (
                PINNED_VERSION.to_string(),
                all_unanswered_risks(question_count(row.pinned)),
            ),
            "case {case:?}: the milestone must report the review as it stood before the \
             upgrade, got {frozen:?}"
        );
        assert_eq!(
            listed_answer_ids(&client, &workload_id, &arn, Some(1), case).await,
            expected_question_ids(row.pinned),
            "case {case:?}: the milestone's answer list must still be version \
             {PINNED_VERSION}'s questions"
        );
    }
}

/// `GetReviewTemplateLensReview`:
/// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}`.
async fn get_template_lens_review(
    client: &Client,
    template_arn: &str,
    lens: &str,
    case: &str,
) -> Value {
    let answer = client
        .get_review_template_lens_review(template_arn, lens)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplateLensReview 200, got {answer:?}"
    );
    answer.body.get("LensReview").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: LensReview must be present, got {answer:?}")
    })
}

/// Every `QuestionId` `ListReviewTemplateAnswers` reports for one lens review of a template,
/// following `NextToken` to the end.
async fn listed_template_answer_ids(
    client: &Client,
    template_arn: &str,
    lens: &str,
    case: &str,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let query = next_token
            .as_ref()
            .map(|token| format!("NextToken={}", encode_path_segment(token)))
            .unwrap_or_default();
        let answer = client
            .list_review_template_answers(template_arn, lens, &query)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListReviewTemplateAnswers 200, got {answer:?}"
        );
        collected.extend(
            answer
                .body
                .get("AnswerSummaries")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .map(|summary| string_member(summary, "QuestionId")),
        );
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
}

/// Rule 2. A REVIEW TEMPLATE's lens review follows the same rule as a workload's: it reports
/// the name, the pillar summaries and the question roll-up of the lens version it is pinned
/// at, and the upgrade is what moves them to the lens's latest version.
#[tokio::test]
async fn a_review_templates_lens_review_reports_the_version_it_is_against() {
    for row in ROWS {
        let case = row.case;
        let client = Client::new();
        let arn = import_and_publish(&client, case, PINNED_VERSION, row.pinned).await;
        let template_arn = review_template(
            &client,
            &format!("tpl-{case}"),
            &[arn.as_str(), FRAMEWORK_LENS],
        )
        .await;

        for (version, shape) in row.later {
            publish_later(&client, case, &arn, version, shape).await;
        }

        // Every later version is published, and the template's review is still against the
        // version the lens was applied at.
        let review = get_template_lens_review(&client, &template_arn, &arn, case).await;
        let asked = question_count(row.pinned);
        assert_eq!(
            (
                string_member(&review, "LensName"),
                string_member(&review, "LensVersion"),
                string_member(&review, "LensStatus"),
            ),
            (
                expected_lens_name(case, PINNED_VERSION),
                PINNED_VERSION.to_string(),
                NOT_CURRENT.to_string(),
            ),
            "case {case:?}: the template's review must stay against the version the lens was \
             applied at, got {review:?}"
        );
        assert_eq!(
            counts(&review, "QuestionCounts", case),
            all_unanswered_questions(asked),
            "case {case:?}: the template's review must count the {asked} questions version \
             {PINNED_VERSION} asks, got {review:?}"
        );
        let summaries = review
            .get("PillarReviewSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
            });
        let expected = expected_pillars(row.pinned);
        assert_eq!(
            summaries
                .iter()
                .map(|summary| (
                    string_member(summary, "PillarId"),
                    string_member(summary, "PillarName")
                ))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(pillar_id, pillar_name, _)| (pillar_id.clone(), pillar_name.clone()))
                .collect::<Vec<_>>(),
            "case {case:?}: the template's review must report version {PINNED_VERSION}'s own \
             pillars, got {summaries:?}"
        );
        assert_eq!(
            listed_template_answer_ids(&client, &template_arn, &arn, case).await,
            expected_question_ids(row.pinned),
            "case {case:?}: the template's answer list must be version {PINNED_VERSION}'s own \
             questions"
        );

        // The upgrade moves the template's review to the lens's latest version, and every
        // read of it follows.
        let Some((latest_version, latest_shape)) = row.later.last() else {
            continue;
        };
        let upgraded = client
            .send(
                "PUT",
                &format!(
                    "/reviewTemplates/{}/lensReviews/{}/upgrade",
                    encode_path_segment(&template_arn),
                    encode_path_segment(&arn)
                ),
                Some(json!({ "ClientRequestToken": format!("tok-upgrade-{case}") })),
            )
            .await;
        assert_eq!(
            upgraded.status, 200,
            "case {case:?}: expected UpgradeReviewTemplateLensReview 200, got {upgraded:?}"
        );

        let moved = get_template_lens_review(&client, &template_arn, &arn, case).await;
        let now_asked = question_count(latest_shape);
        assert_eq!(
            (
                string_member(&moved, "LensName"),
                string_member(&moved, "LensVersion"),
                string_member(&moved, "LensStatus"),
                counts(&moved, "QuestionCounts", case),
            ),
            (
                expected_lens_name(case, latest_version),
                (*latest_version).to_string(),
                CURRENT.to_string(),
                all_unanswered_questions(now_asked),
            ),
            "case {case:?}: the upgrade must move the template's review to version \
             {latest_version} and its {now_asked} questions, got {moved:?}"
        );
        assert_eq!(
            listed_template_answer_ids(&client, &template_arn, &arn, case).await,
            expected_question_ids(latest_shape),
            "case {case:?}: the upgraded answer list must be version {latest_version}'s own \
             questions"
        );
    }
}
