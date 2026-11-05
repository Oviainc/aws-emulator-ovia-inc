// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetLensReview`: ONE lens review of ONE workload.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: a lens
//! review reports the named lens's own metadata plus a `Risk` roll-up with one bucket per
//! `Risk` the enum publishes — once for the whole review and once per pillar the lens is
//! organized into — where each bucket counts the questions of that question set whose
//! stored answer carries that risk, and `UNANSWERED` counts the questions with no stored
//! answer. The reviewed workload is the live one, or — when the request names a milestone
//! — the one that milestone recorded; a review of a lens the workload does not carry, of
//! a workload that does not exist or of a milestone that was never recorded does not
//! resolve.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord;

use super::workload_harness::{
    answered, assert_field_validation, created_id, encode_path_segment, framework_answer,
    framework_answer_that_does_not_apply, minimal_create, official_lens_arn, Answer, Client,
    InPlace,
};

/// The AWS Well-Architected Framework lens's pillars: the `PillarId` each is named by on
/// the wire, the `PillarName` the framework publishes for it and the number of questions
/// it asks (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6). Spelled out here
/// — in the order the framework presents them — so the tests pin the catalog rather than
/// reading it back out of the code under test.
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

/// Every member the `Risk` enum publishes (`smithy_query(query="shape", shape="Risk")`),
/// so a roll-up is checked for ALL its buckets rather than the one a case moved.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The published name of that lens, as `LensReview$LensName` reports it.
const FRAMEWORK_LENS_NAME: &str = "AWS Well-Architected Framework";

/// A lens alias AWS publishes that this emulator's AWS-official lens catalog carries no
/// content for, so it asks no questions here (the catalog itself is pinned by
/// `super::official_lens_catalog`).
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The Region the shared harness signs its requests for, which is the Region the ARNs in
/// a response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`, with `MilestoneNumber` bound to
/// the query string. The lens travels percent-encoded in ONE path segment, exactly as an
/// SDK sends an ARN-spelled alias.
async fn get_lens_review(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone_number: Option<i64>,
) -> Answer {
    let encoded = encode_path_segment(lens);
    let suffix = milestone_number
        .map(|number| format!("?MilestoneNumber={number}"))
        .unwrap_or_default();
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/lensReviews/{encoded}{suffix}"),
            None,
        )
        .await
}

/// `CreateMilestone`: `POST /workloads/{WorkloadId}/milestones`.
async fn create_milestone(client: &Client, workload_id: &str, name: &str, token: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": token })),
        )
        .await
}

/// A `CreateWorkload` body that attaches exactly `lens`.
fn create_with_lens(name: &str, lens: &str) -> Value {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!([lens]);
    body
}

/// The `LensReview` a successful `GetLensReview` answered with.
fn review_of(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One `RiskCounts` map of a response, as a sorted map.
fn risk_counts(value: &Value) -> BTreeMap<String, i64> {
    value
        .get("RiskCounts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

/// The pillar review summaries of a review, in the order they were served.
fn pillar_summaries(review: &Value) -> Vec<Value> {
    review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The roll-up expected for a question set of `total` questions, of which the listed
/// risks are the ones recorded: every other question of the set is `UNANSWERED`.
fn expected_counts(total: i64, recorded: &[&str]) -> BTreeMap<String, i64> {
    let mut counts: BTreeMap<String, i64> = RISKS
        .iter()
        .map(|risk| ((*risk).to_string(), 0))
        .collect();
    for risk in recorded {
        *counts.entry((*risk).to_string()).or_default() += 1;
    }
    let answered = i64::try_from(recorded.len()).unwrap_or(i64::MAX);
    counts.insert("UNANSWERED".to_string(), total - answered);
    counts
}

/// A review nobody has answered counts every question of every pillar as `UNANSWERED`,
/// the pillar summaries are the lens's pillars in the lens's own order, and the review
/// names the lens it is of. The rows are the pillars of the catalog — each with its
/// published name and question count — so the whole question set is checked rather than
/// one cited pillar.
#[tokio::test]
async fn a_fresh_review_counts_every_question_of_every_pillar_as_unanswered() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(create_with_lens("fresh-review", FRAMEWORK_LENS)).await,
        "fresh-review",
    );

    let answer = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    let review = review_of(&answer, "fresh");

    assert_eq!(
        answer.body.get("WorkloadId").and_then(Value::as_str),
        Some(workload_id.as_str()),
        "case \"fresh\": the envelope must name the workload, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("MilestoneNumber"),
        None,
        "case \"fresh\": a read of the live review names no milestone, got {answer:?}"
    );
    assert_eq!(
        review.get("LensAlias").and_then(Value::as_str),
        Some(FRAMEWORK_LENS),
        "case \"fresh\": the review must name the lens as the request did, got {review:?}"
    );
    assert_eq!(
        review.get("LensArn").and_then(Value::as_str),
        Some(official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS).as_str()),
        "case \"fresh\": the review must carry the lens ARN, got {review:?}"
    );
    assert_eq!(
        review.get("LensName").and_then(Value::as_str),
        Some(FRAMEWORK_LENS_NAME),
        "case \"fresh\": the review must name the lens it is of, got {review:?}"
    );
    // `LensStatus` publishes CURRENT | NOT_CURRENT | DEPRECATED | DELETED | UNSHARED
    // (`smithy_query(query="shape", shape="LensStatus")`); the lens content here has one
    // version, so a review of it is on the current one.
    assert_eq!(
        review.get("LensStatus").and_then(Value::as_str),
        Some("CURRENT"),
        "case \"fresh\": the review must report the lens status, got {review:?}"
    );
    // `LensVersion` is `@length(min: 1, max: 32)`.
    let version = review
        .get("LensVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        (1..=32).contains(&version.len()),
        "case \"fresh\": LensVersion must satisfy @length(1, 32), got {version:?}"
    );
    assert!(
        review.get("UpdatedAt").is_some(),
        "case \"fresh\": the review must report when it was last updated, got {review:?}"
    );
    assert_eq!(
        review.get("Notes"),
        None,
        "case \"fresh\": no notes have been recorded against the review, got {review:?}"
    );
    assert_eq!(
        risk_counts(&review),
        expected_counts(FRAMEWORK_QUESTIONS, &[]),
        "case \"fresh\": every question of the lens is outstanding, got {review:?}"
    );

    let summaries = pillar_summaries(&review);
    assert_eq!(
        summaries.len(),
        FRAMEWORK_PILLARS.len(),
        "case \"fresh\": the lens has {} pillars, got {summaries:?}",
        FRAMEWORK_PILLARS.len()
    );
    let mut totals: BTreeMap<String, i64> = RISKS
        .iter()
        .map(|risk| ((*risk).to_string(), 0))
        .collect();
    for (index, (pillar_id, pillar_name, questions)) in FRAMEWORK_PILLARS.iter().enumerate() {
        let summary = &summaries[index];
        assert_eq!(
            summary.get("PillarId").and_then(Value::as_str),
            Some(*pillar_id),
            "case {pillar_id:?}: the pillar summaries must be in the lens's own order, \
             got {summaries:?}"
        );
        assert_eq!(
            summary.get("PillarName").and_then(Value::as_str),
            Some(*pillar_name),
            "case {pillar_id:?}: the summary must carry the pillar's name, got {summary:?}"
        );
        assert_eq!(
            risk_counts(summary),
            expected_counts(*questions, &[]),
            "case {pillar_id:?}: the pillar asks {questions} outstanding questions, \
             got {summary:?}"
        );
        assert_eq!(
            summary.get("Notes"),
            None,
            "case {pillar_id:?}: no notes have been recorded against the pillar, got {summary:?}"
        );
        for (risk, count) in risk_counts(summary) {
            *totals.entry(risk).or_default() += count;
        }
    }
    // The invariant: the pillars partition the lens's question set, so their counts sum
    // to the review's.
    assert_eq!(
        totals,
        risk_counts(&review),
        "case \"fresh\": the pillar roll-ups must sum to the review's, got {totals:?}"
    );

    // A read reports; it must not change what the next caller sees.
    let again = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
    assert_eq!(
        review_of(&again, "fresh/again"),
        review,
        "case \"fresh/again\": a read must leave the review alone, got {again:?}"
    );
}

/// An answer recorded against a question moves THAT question out of `UNANSWERED` and into
/// the bucket its answer carries, in the roll-up of its own pillar and in the review's.
/// The rows are the four risks an answered question can carry — plus a second answer in a
/// pillar that already has one, so the buckets accumulate rather than being set.
#[tokio::test]
async fn an_answer_moves_the_counts_of_its_pillar_and_of_the_whole_review() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(create_with_lens("answered-review", FRAMEWORK_LENS)).await,
        "answered-review",
    );

    // The risk a stored answer carries: a question that does not apply is NOT_APPLICABLE,
    // one answered with no best practice in place is HIGH, one with a best practice still
    // unselected is MEDIUM, and one with nothing left unselected is NONE.
    let rows: [(&str, &str, &str, AnswerRecord, &str); 5] = [
        (
            "no-best-practice-in-place",
            "reliability",
            "manage-service-quotas",
            framework_answer("manage-service-quotas", InPlace::Nothing),
            "HIGH",
        ),
        (
            "a-best-practice-still-unselected",
            "security",
            "identities",
            framework_answer("identities", InPlace::AllButLast),
            "MEDIUM",
        ),
        (
            "every-best-practice-in-place",
            "operationalExcellence",
            "organization-culture",
            framework_answer("organization-culture", InPlace::Every),
            "NONE",
        ),
        (
            "question-does-not-apply",
            "sustainability",
            "dev-deploy-patterns",
            framework_answer_that_does_not_apply("dev-deploy-patterns", "OUT_OF_SCOPE"),
            "NOT_APPLICABLE",
        ),
        (
            "second-answer-in-the-same-pillar",
            "reliability",
            "network-topology",
            framework_answer("network-topology", InPlace::Nothing),
            "HIGH",
        ),
    ];

    let mut recorded: Vec<(&str, &str)> = Vec::new();
    for (case, pillar, question_id, record, risk) in rows {
        client.record_answer(&workload_id, FRAMEWORK_LENS, question_id, record);
        recorded.push((pillar, risk));

        let answer = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await;
        let review = review_of(&answer, case);

        let all: Vec<&str> = recorded.iter().map(|(_, risk)| *risk).collect();
        assert_eq!(
            risk_counts(&review),
            expected_counts(FRAMEWORK_QUESTIONS, &all),
            "case {case:?}: the review roll-up must count every stored answer, got {review:?}"
        );

        let summaries = pillar_summaries(&review);
        for (pillar_id, _, questions) in FRAMEWORK_PILLARS {
            let summary = summaries
                .iter()
                .find(|summary| {
                    summary.get("PillarId").and_then(Value::as_str) == Some(pillar_id)
                })
                .cloned()
                .unwrap_or(Value::Null);
            let in_pillar: Vec<&str> = recorded
                .iter()
                .filter(|(recorded_pillar, _)| *recorded_pillar == pillar_id)
                .map(|(_, risk)| *risk)
                .collect();
            assert_eq!(
                risk_counts(&summary),
                expected_counts(questions, &in_pillar),
                "case {case:?}: pillar {pillar_id:?} must count only its own questions, \
                 got {summary:?}"
            );
        }
        assert!(
            pillar.len() <= 64,
            "case {case:?}: PillarId must satisfy @length(1, 64), got {pillar:?}"
        );
    }
}

/// A lens is the same lens however the request named it: the bare alias, the lens ARN and
/// a version-pinned lens ARN all reach the one review, which reports the spelling the
/// request used beside the ARN it resolves to.
#[tokio::test]
async fn every_spelling_of_the_lens_reaches_the_same_review() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(create_with_lens("spellings", FRAMEWORK_LENS)).await,
        "spellings",
    );
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "cloud-financial-management",
        answered(&[], true, None),
    );

    for (case, lens) in [
        ("alias", FRAMEWORK_LENS.to_string()),
        (
            "arn",
            official_lens_arn("us-east-1", FRAMEWORK_LENS),
        ),
        (
            "versioned-arn",
            format!(
                "{}/2023-10-03",
                official_lens_arn("us-east-1", FRAMEWORK_LENS)
            ),
        ),
    ] {
        let answer = get_lens_review(&client, &workload_id, &lens, None).await;
        let review = review_of(&answer, case);
        assert_eq!(
            review.get("LensAlias").and_then(Value::as_str),
            Some(lens.as_str()),
            "case {case:?}: the review must name the lens as the request did, got {review:?}"
        );
        assert_eq!(
            review.get("LensArn").and_then(Value::as_str),
            Some(official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS).as_str()),
            "case {case:?}: the ARN is the lens's, in the request's Region, got {review:?}"
        );
        assert_eq!(
            risk_counts(&review),
            expected_counts(FRAMEWORK_QUESTIONS, &["HIGH"]),
            "case {case:?}: every spelling must reach the one review, got {review:?}"
        );
    }
}

/// A milestone records the workload at an instant, answers included: a read scoped to a
/// milestone reports the review as it was and names the milestone, while the live review
/// moves on. Checked at each milestone the workload holds, so the rule is pinned over a
/// span rather than at one number.
#[tokio::test]
async fn a_milestone_scoped_read_reports_the_review_that_milestone_recorded() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(create_with_lens("milestoned", FRAMEWORK_LENS)).await,
        "milestoned",
    );

    // Milestone <n> is recorded with exactly <n> questions answered HIGH.
    let questions = ["manage-service-quotas", "network-topology", "service-architecture"];
    for (index, question_id) in questions.iter().enumerate() {
        client.record_answer(
            &workload_id,
            FRAMEWORK_LENS,
            question_id,
            answered(&[], true, None),
        );
        let number = i64::try_from(index + 1).unwrap_or(0);
        let recorded = create_milestone(
            &client,
            &workload_id,
            &format!("snapshot-{number}"),
            &format!("tok-ms-{number}"),
        )
        .await;
        assert_eq!(
            recorded.status, 200,
            "case \"milestone-{number}\": the milestone must be recorded, got {recorded:?}"
        );
    }
    // One more answer after the last milestone, so the live review differs from every
    // recorded one.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "securely-operate",
        answered(&[], true, None),
    );

    for number in 1..=i64::try_from(questions.len()).unwrap_or(0) {
        let case = format!("milestone-{number}");
        let answer = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, Some(number)).await;
        let review = review_of(&answer, &case);
        assert_eq!(
            answer.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(number),
            "case {case:?}: a milestone-scoped read must name the milestone, got {answer:?}"
        );
        let frozen: Vec<&str> = (0..number).map(|_| "HIGH").collect();
        assert_eq!(
            risk_counts(&review),
            expected_counts(FRAMEWORK_QUESTIONS, &frozen),
            "case {case:?}: the milestone must report the review it recorded, got {review:?}"
        );
    }

    let live = review_of(
        &get_lens_review(&client, &workload_id, FRAMEWORK_LENS, None).await,
        "live",
    );
    let answered_live: Vec<&str> = (0..=questions.len()).map(|_| "HIGH").collect();
    assert_eq!(
        risk_counts(&live),
        expected_counts(FRAMEWORK_QUESTIONS, &answered_live),
        "case \"live\": the live review must count every stored answer, got {live:?}"
    );
}

/// A lens this emulator ships no question set for is organized into no pillars and asks
/// nothing, so its review reports no pillar summaries, an all-zero roll-up and none of
/// the members that describe the lens's content.
#[tokio::test]
async fn a_review_of_a_lens_without_content_reports_no_pillars() {
    let client = Client::new();
    for (case, lens) in [
        ("unshipped-official-alias", UNSHIPPED_LENS.to_string()),
        (
            "unshipped-official-arn",
            official_lens_arn("us-east-1", UNSHIPPED_LENS),
        ),
    ] {
        let workload_id = created_id(
            &client.create(create_with_lens(case, &lens)).await,
            case,
        );
        let answer = get_lens_review(&client, &workload_id, &lens, None).await;
        let review = review_of(&answer, case);
        assert_eq!(
            pillar_summaries(&review),
            Vec::<Value>::new(),
            "case {case:?}: a lens with no content has no pillars, got {review:?}"
        );
        assert_eq!(
            risk_counts(&review),
            expected_counts(0, &[]),
            "case {case:?}: a lens that asks nothing counts nothing, got {review:?}"
        );
        for member in ["LensName", "LensVersion", "LensStatus"] {
            assert_eq!(
                review.get(member),
                None,
                "case {case:?}: {member} is lens content this emulator does not hold, \
                 got {review:?}"
            );
        }
        assert_eq!(
            review.get("LensAlias").and_then(Value::as_str),
            Some(lens.as_str()),
            "case {case:?}: the review must name the lens as the request did, got {review:?}"
        );
    }
}

/// The review that is read must exist: the workload is RESOLVED — so an id nothing is
/// stored under, and a milestone nothing recorded, are each the modelled 404 — while the
/// lens is a VALUE of the request, so a lens the workload does not carry is the modelled
/// 400 naming `LensAlias`. A `MilestoneNumber` outside the modelled range is the modelled
/// 400, checked at each published bound and one past it.
#[tokio::test]
async fn a_review_that_does_not_exist_does_not_resolve() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(create_with_lens("lookup-host", FRAMEWORK_LENS)).await,
        "lookup-host",
    );
    // One milestone exists, so a request for a later number is a genuine miss.
    let recorded = create_milestone(&client, &workload_id, "only-one", "tok-only-one").await;
    assert_eq!(
        recorded.status, 200,
        "the host workload must hold one milestone, got {recorded:?}"
    );

    for (case, workload, lens, milestone) in [
        ("absent-workload", ABSENT_HEX_ID, FRAMEWORK_LENS, None),
        (
            "milestone-not-recorded",
            workload_id.as_str(),
            FRAMEWORK_LENS,
            Some(2),
        ),
        (
            "milestone-at-the-range-maximum",
            workload_id.as_str(),
            FRAMEWORK_LENS,
            Some(100),
        ),
        (
            "absent-workload-with-milestone",
            ABSENT_HEX_ID,
            FRAMEWORK_LENS,
            Some(1),
        ),
    ] {
        let answer = get_lens_review(&client, workload, lens, milestone).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a resource that does not exist is a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: the miss must be the modelled error, got {answer:?}"
        );
    }

    // A lens the workload does not carry names no review of it, under either spelling of
    // the name: the value is refused as input.
    for (case, lens) in [
        ("lens-not-attached", UNSHIPPED_LENS),
        (
            "custom-lens-not-attached",
            "arn:aws:wellarchitected:us-west-2:000000000000:lens/0123456789abcdef0123456789abcdef",
        ),
    ] {
        let answer = get_lens_review(&client, &workload_id, lens, None).await;
        assert_field_validation(
            case,
            &answer,
            "LensAlias",
            &format!("No lens review for lens alias {lens} was found in workload {workload_id}."),
        );
    }

    // `MilestoneNumber` is `@range(min: 1, max: 100)`: 1 and 100 are inside the range (and
    // answered above), 0 and 101 are not and are refused at the boundary.
    for (case, milestone) in [("below-the-minimum", 0), ("above-the-maximum", 101)] {
        let answer = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, Some(milestone)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a MilestoneNumber outside the modelled range is a 400, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: the refusal must be the modelled error, got {answer:?}"
        );
    }

    // The review of the host workload still resolves, so every refusal above is the miss
    // it names rather than a broken read.
    let present = get_lens_review(&client, &workload_id, FRAMEWORK_LENS, Some(1)).await;
    assert_eq!(
        present.status, 200,
        "case \"present\": the recorded review must still resolve, got {present:?}"
    );
}
