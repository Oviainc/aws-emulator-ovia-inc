// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLensReviews`: the lens reviews of ONE workload.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. The listing REPORTS one `LensReviewSummary` per lens the reviewed workload carries —
//!    a lens attached under two spellings is ONE review — each naming the lens as the
//!    workload records it, beside the ARN it resolves to in the request's Region, the
//!    lens's own content members for the lens whose definition this emulator ships, and a
//!    `Risk` roll-up with one bucket per `Risk` the enum publishes counted over the
//!    questions THAT lens asks.
//! 2. Paging enumerates every review exactly once, in one stable order, and the last page
//!    omits `NextToken`.
//! 3. The reviewed workload is the live one, or — when the request names a milestone — the
//!    one that milestone recorded; a workload that does not exist and a milestone that was
//!    never recorded do not resolve.
//! 4. A `NextToken` this service did not issue (including one another paginated operation
//!    issued) is refused rather than answered with a page.
//! 5. Listing is reporting only: it changes nothing a later caller sees.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::workload_harness::{
    answered, created_id, encode_path_segment, framework_answer,
    framework_answer_that_does_not_apply, minimal_create, official_lens_arn, Answer, Client,
    InPlace,
};

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The published name of that lens, as a lens review reports it.
const FRAMEWORK_LENS_NAME: &str = "AWS Well-Architected Framework";

/// The members the `LensStatus` enum publishes
/// (`smithy_query(query="shape", shape="LensStatus")`), so a reported status is checked
/// against the whole set rather than one expected value.
const LENS_STATUSES: [&str; 5] = [
    "CURRENT",
    "NOT_CURRENT",
    "DEPRECATED",
    "DELETED",
    "UNSHARED",
];

/// Aliases AWS publishes lenses under, besides the framework lens.
///
/// SOURCE: the Lens Catalog of the AWS Well-Architected Tool user guide, the same catalog
/// `state::AWS_OFFICIAL_LENS_ALIASES` carries — written out here so the tests pin it. How
/// many questions each asks is NOT written out: it is read back over HTTP from that
/// lens review's own answer list ([`question_count`]), so a lens whose content this
/// emulator gains later needs no change here.
const OTHER_LENSES: [&str; 9] = [
    "connectedmobility",
    "dataanalytics",
    "devops",
    "financialservices",
    "games",
    "iot",
    "machinelearning",
    "sap",
    "serverless",
];

/// Every member the `Risk` enum publishes (`smithy_query(query="shape", shape="Risk")`),
/// so a roll-up is checked for ALL its buckets rather than the one a case moved.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The Region the shared harness signs its requests for, which is the Region the ARNs in a
/// response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// The largest page this service serves, and the page size a request that omits
/// `MaxResults` gets — the modelled `MaxResults` `@range(min: 1, max: 50)` maximum.
const MAX_RESULTS_CEILING: i64 = 50;

/// `ListLensReviews`: `GET /workloads/{WorkloadId}/lensReviews`, with every paging member
/// and `MilestoneNumber` bound to the query string. `query` is passed as sent, empty for a
/// request that names none.
async fn list_lens_reviews(client: &Client, workload_id: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/workloads/{workload_id}/lensReviews")
    } else {
        format!("/workloads/{workload_id}/lensReviews?{query}")
    };
    client.send("GET", &uri, None).await
}

/// How many questions the named lens review asks, read back over HTTP from `ListAnswers`
/// — the operation that enumerates exactly that question set, one `AnswerSummary` per
/// question — paged to the end. The roll-up under test must agree with it.
async fn question_count(client: &Client, workload_id: &str, lens: &str) -> i64 {
    let mut counted = 0i64;
    let mut next_token: Option<String> = None;
    loop {
        let query = match &next_token {
            None => format!("MaxResults={MAX_RESULTS_CEILING}"),
            Some(token) => format!("MaxResults={MAX_RESULTS_CEILING}&NextToken={token}"),
        };
        let answer = client
            .send(
                "GET",
                &format!(
                    "/workloads/{workload_id}/lensReviews/{}/answers?{query}",
                    encode_path_segment(lens)
                ),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {lens:?}: expected ListAnswers 200 while counting the question set, \
             got {answer:?}"
        );
        counted += answer
            .body
            .get("AnswerSummaries")
            .and_then(Value::as_array)
            .map(|summaries| i64::try_from(summaries.len()).unwrap_or(i64::MAX))
            .unwrap_or_default();
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return counted;
        }
    }
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

/// A workload attaching exactly `lenses`, answered by its `WorkloadId`.
async fn workload_with_lenses(client: &Client, name: &str, lenses: &[String]) -> String {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!(lenses);
    created_id(&client.create(body).await, name)
}

/// The `LensReviewSummaries` of a successful answer.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: LensReviewSummaries must be present, got {answer:?}")
        })
}

/// The `LensAlias` of each summary, in the order they were served.
fn aliases(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|summary| {
            summary
                .get("LensAlias")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
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

/// The roll-up expected for a question set of `total` questions, of which the listed risks
/// are the ones recorded: every other question of the set is `UNANSWERED`.
fn expected_counts(total: i64, recorded: &[&str]) -> BTreeMap<String, i64> {
    let mut counts: BTreeMap<String, i64> =
        RISKS.iter().map(|risk| ((*risk).to_string(), 0)).collect();
    for risk in recorded {
        *counts.entry((*risk).to_string()).or_default() += 1;
    }
    let answered = i64::try_from(recorded.len()).unwrap_or(i64::MAX);
    counts.insert("UNANSWERED".to_string(), total - answered);
    counts
}

/// The bare alias the ARN of an AWS-official lens ends in — what the lens's identity is,
/// read off the ARN the response reports rather than out of the code under test.
fn alias_of(arn: &str) -> String {
    arn.rsplit_once("lens/")
        .map(|(_, alias)| alias)
        .unwrap_or(arn)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert every member of one summary against rule 1, for the lens the workload stored as
/// `stored`, whose question set — as that review's own answer list enumerates it — has
/// `questions` entries.
fn assert_summary(summary: &Value, stored: &str, questions: i64, case: &str) {
    assert_eq!(
        summary.get("LensAlias").and_then(Value::as_str),
        Some(stored),
        "case {case:?}: the summary must name the lens as the workload records it, \
         got {summary:?}"
    );
    let arn = summary
        .get("LensArn")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        arn,
        official_lens_arn(REQUEST_REGION, &alias_of(stored)).as_str(),
        "case {case:?}: the summary must carry the lens ARN in the request's Region, \
         got {summary:?}"
    );
    assert!(
        summary.get("UpdatedAt").is_some(),
        "case {case:?}: the summary must report when the review was last updated, \
         got {summary:?}"
    );
    assert_eq!(
        risk_counts(summary),
        expected_counts(questions, &[]),
        "case {case:?}: the roll-up counts the questions this lens asks, got {summary:?}"
    );
    // A lens's name, version and status are lens CONTENT: all three describe the same
    // content, so a lens this emulator holds content for reports every one of them and a
    // lens it holds none for omits every one rather than guessing at any.
    let content: Vec<&str> = ["LensName", "LensVersion", "LensStatus"]
        .into_iter()
        .filter(|member| summary.get(*member).is_some())
        .collect();
    assert!(
        content.len() == 3 || content.is_empty(),
        "case {case:?}: the members describing the lens's content must be reported \
         together or not at all, got {content:?} in {summary:?}"
    );
    if !content.is_empty() {
        let name = summary
            .get("LensName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // `LensName` is `@length(min: 1, max: 128)`.
        assert!(
            (1..=128).contains(&name.len()),
            "case {case:?}: LensName must satisfy @length(1, 128), got {name:?}"
        );
        // `LensVersion` is `@length(min: 1, max: 32)`.
        let version = summary
            .get("LensVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            (1..=32).contains(&version.len()),
            "case {case:?}: LensVersion must satisfy @length(1, 32), got {version:?}"
        );
        let status = summary
            .get("LensStatus")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            LENS_STATUSES.contains(&status),
            "case {case:?}: LensStatus must be a member the enum publishes, got {status:?}"
        );
        if alias_of(stored) == FRAMEWORK_LENS {
            assert_eq!(
                name, FRAMEWORK_LENS_NAME,
                "case {case:?}: the framework lens is named by its published name, \
                 got {summary:?}"
            );
        }
    }
    // A review carries profiles, and the risk counts restricted to the questions they
    // prioritise, only once a profile is associated with the workload; no operation that
    // associates one is implemented yet.
    for member in ["Profiles", "PrioritizedRiskCounts"] {
        assert_eq!(
            summary.get(member),
            None,
            "case {case:?}: {member} exists only once a profile is associated, \
             got {summary:?}"
        );
    }
}

/// Rule 1 and rule 5: one summary per lens the workload carries, however many lenses it
/// carries and however each was named. The rows differ along what the rule generalises
/// over — the number of lenses and the spelling each was attached under — and a lens
/// attached under two spellings is one review. The `UNANSWERED` bucket of each review is
/// checked against the question set that review's OWN answer list enumerates, so a lens
/// whose content this emulator ships later needs no change here.
#[tokio::test]
async fn a_summary_is_reported_for_every_lens_the_workload_carries() {
    let framework_arn = official_lens_arn("us-east-1", FRAMEWORK_LENS);
    let rows: [(&str, Vec<String>, usize); 7] = [
        ("no-lenses", Vec::new(), 0),
        ("the-framework-lens", vec![FRAMEWORK_LENS.to_string()], 1),
        ("another-official-lens", vec![OTHER_LENSES[8].to_string()], 1),
        (
            "three-lenses",
            vec![
                OTHER_LENSES[2].to_string(),
                FRAMEWORK_LENS.to_string(),
                OTHER_LENSES[8].to_string(),
            ],
            3,
        ),
        (
            "arn-spelled-lens",
            vec![official_lens_arn("us-east-1", OTHER_LENSES[0])],
            1,
        ),
        (
            "two-spellings-of-one-lens",
            vec![FRAMEWORK_LENS.to_string(), framework_arn.clone()],
            1,
        ),
        (
            "version-pinned-arn",
            vec![format!("{framework_arn}/2023-10-03")],
            1,
        ),
    ];

    for (case, lenses, expected_reviews) in rows {
        let client = Client::new();
        let workload_id = workload_with_lenses(&client, case, &lenses).await;

        let answer = list_lens_reviews(&client, &workload_id, "").await;
        let page = summaries(&answer, case);
        assert_eq!(
            answer.body.get("WorkloadId").and_then(Value::as_str),
            Some(workload_id.as_str()),
            "case {case:?}: the envelope must name the workload listed, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("MilestoneNumber"),
            None,
            "case {case:?}: a read of the live reviews names no milestone, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("NextToken"),
            None,
            "case {case:?}: every review fits in one page here, got {answer:?}"
        );
        assert_eq!(
            page.len(),
            expected_reviews,
            "case {case:?}: one summary per lens the workload carries, got {page:?}"
        );

        // Each summary describes the lens the workload stored, under the spelling it was
        // attached with: the FIRST spelling of a lens attached under several.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for stored in &lenses {
            let alias = alias_of(stored);
            if !seen.insert(alias.clone()) {
                continue;
            }
            let summary = page
                .iter()
                .find(|summary| {
                    summary.get("LensAlias").and_then(Value::as_str) == Some(stored.as_str())
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the review of {stored:?} must be reported, got {page:?}")
                });
            let questions = question_count(&client, &workload_id, stored).await;
            assert_summary(summary, stored, questions, case);
        }

        // The served order is the WORKLOAD's own lens order: a review is reported where
        // its lens is in `Workload$Lenses` — the order the lenses were applied — so a
        // client sees a lens and its review in the same place in both lists.
        let reported: Vec<String> = page
            .iter()
            .map(|summary| {
                summary
                    .get("LensAlias")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut applied: Vec<String> = Vec::new();
        let mut counted: BTreeSet<String> = BTreeSet::new();
        for stored in &lenses {
            if counted.insert(alias_of(stored)) {
                applied.push(stored.clone());
            }
        }
        assert_eq!(
            reported, applied,
            "case {case:?}: the summaries must be served in the order the workload applies \
             its lenses, got {reported:?}"
        );

        // Rule 5: a read reports; it must not change what the next caller sees.
        let again = list_lens_reviews(&client, &workload_id, "").await;
        assert_eq!(
            summaries(&again, case),
            page,
            "case {case:?}: a listing must leave the reviews alone, got {again:?}"
        );
    }
}

/// Rule 2: paging enumerates every review exactly once, in the order an unpaged listing
/// serves them, and the last page omits `NextToken`. Checked exhaustively over every
/// (lens count, page size) pair in a bounded range, so the invariant is pinned at both ends
/// of the range and wherever the page boundary lands.
#[tokio::test]
async fn paging_enumerates_every_lens_review_exactly_once() {
    for lens_count in 1usize..=5 {
        let client = Client::new();
        // The lens set grows along the catalog, so each count is a different set of lenses
        // rather than the same one renamed.
        let mut lenses: Vec<String> = OTHER_LENSES[..lens_count - 1]
            .iter()
            .map(ToString::to_string)
            .collect();
        lenses.push(FRAMEWORK_LENS.to_string());
        let workload_id =
            workload_with_lenses(&client, &format!("pager-{lens_count}"), &lenses).await;

        let unpaged = aliases(&summaries(
            &list_lens_reviews(&client, &workload_id, "").await,
            "unpaged",
        ));
        assert_eq!(
            unpaged.len(),
            lens_count,
            "case \"unpaged-{lens_count}\": every attached lens must have a review, \
             got {unpaged:?}"
        );

        for page_size in 1usize..=(lens_count + 2) {
            let case = format!("{lens_count} lenses, MaxResults {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                pages += 1;
                assert!(
                    pages <= lens_count + 2,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
                let query = match &next_token {
                    None => format!("MaxResults={page_size}"),
                    Some(token) => format!("MaxResults={page_size}&NextToken={token}"),
                };
                let answer = list_lens_reviews(&client, &workload_id, &query).await;
                let page = summaries(&answer, &case);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page must not exceed MaxResults, got {} entries",
                    page.len()
                );
                seen.extend(aliases(&page));
                next_token = answer
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if next_token.is_none() {
                    break;
                }
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case:?}: a page that is not the last must be full, got {page:?}"
                );
            }
            assert_eq!(
                seen, unpaged,
                "case {case:?}: paging must serve every review exactly once, in the one \
                 order, got {seen:?}"
            );
        }
    }
}

/// Rule 1 over the answers a workload stores: an answer recorded against a question moves
/// THAT question out of `UNANSWERED` and into the bucket its answer carries, in the roll-up
/// of its own lens's review and of no other. The rows are the four risks an answered
/// question can carry, plus a second answer so the buckets accumulate rather than being
/// set.
#[tokio::test]
async fn the_roll_up_of_each_review_counts_only_that_lens_s_answers() {
    let client = Client::new();
    let other_lens = OTHER_LENSES[8];
    let workload_id = workload_with_lenses(
        &client,
        "answered-reviews",
        &[FRAMEWORK_LENS.to_string(), other_lens.to_string()],
    )
    .await;
    // The question set of each review, as that review's own answer list enumerates it, and
    // the other lens's roll-up before any answer is recorded: an answer to one review must
    // leave the other exactly where it was.
    let framework_questions = question_count(&client, &workload_id, FRAMEWORK_LENS).await;
    let untouched = risk_counts(
        &summaries(
            &list_lens_reviews(&client, &workload_id, "").await,
            "before-any-answer",
        )
        .into_iter()
        .find(|summary| summary.get("LensAlias").and_then(Value::as_str) == Some(other_lens))
        .unwrap_or(Value::Null),
    );

    // The risk a stored answer carries: a question that does not apply is NOT_APPLICABLE,
    // one answered with no best practice in place is HIGH, one with a best practice still
    // unselected is MEDIUM, and one with nothing left unselected is NONE.
    let rows: [(&str, &str, _, &str); 5] = [
        (
            "no-best-practice-in-place",
            "manage-service-quotas",
            framework_answer("manage-service-quotas", InPlace::Nothing),
            "HIGH",
        ),
        (
            "a-best-practice-still-unselected",
            "identities",
            framework_answer("identities", InPlace::AllButLast),
            "MEDIUM",
        ),
        (
            "every-best-practice-in-place",
            "organization-culture",
            framework_answer("organization-culture", InPlace::Every),
            "NONE",
        ),
        (
            "question-does-not-apply",
            "dev-deploy-patterns",
            framework_answer_that_does_not_apply("dev-deploy-patterns", "OUT_OF_SCOPE"),
            "NOT_APPLICABLE",
        ),
        (
            "second-answer-of-the-same-review",
            "network-topology",
            framework_answer("network-topology", InPlace::Nothing),
            "HIGH",
        ),
    ];

    let mut recorded: Vec<&str> = Vec::new();
    for (case, question_id, record, risk) in rows {
        client.record_answer(&workload_id, FRAMEWORK_LENS, question_id, record);
        recorded.push(risk);

        let answer = list_lens_reviews(&client, &workload_id, "").await;
        let page = summaries(&answer, case);
        let summary_of = |lens: &str| {
            page.iter()
                .find(|summary| summary.get("LensAlias").and_then(Value::as_str) == Some(lens))
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(
            risk_counts(&summary_of(FRAMEWORK_LENS)),
            expected_counts(framework_questions, &recorded),
            "case {case:?}: the review's roll-up must count every answer stored against \
             it, got {page:?}"
        );
        assert_eq!(
            risk_counts(&summary_of(other_lens)),
            untouched,
            "case {case:?}: an answer to one review must leave the other review's roll-up \
             exactly where it was, got {page:?}"
        );
    }
}

/// Rule 3: a milestone records the workload at an instant, answers included, so a listing
/// scoped to a milestone reports the reviews as they were and names the milestone, while
/// the live listing moves on. Checked at each milestone the workload holds, so the rule is
/// pinned over a span rather than at one number.
#[tokio::test]
async fn a_milestone_scoped_listing_reports_the_reviews_that_milestone_recorded() {
    let client = Client::new();
    let workload_id =
        workload_with_lenses(&client, "milestoned", &[FRAMEWORK_LENS.to_string()]).await;
    // The question set of the review under test, as its own answer list enumerates it.
    let framework_questions = question_count(&client, &workload_id, FRAMEWORK_LENS).await;

    // Milestone <n> is recorded with exactly <n> questions answered HIGH.
    let questions = ["manage-service-quotas", "network-topology", "service-architecture"];
    for (index, question_id) in questions.iter().enumerate() {
        client.record_answer(
            &workload_id,
            FRAMEWORK_LENS,
            question_id,
            answered(&[], true, None),
        );
        let number = index + 1;
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
    // One more answer after the last milestone, so the live listing differs from every
    // recorded one.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "securely-operate",
        answered(&[], true, None),
    );

    for number in 1..=questions.len() {
        let case = format!("milestone-{number}");
        let answer =
            list_lens_reviews(&client, &workload_id, &format!("MilestoneNumber={number}")).await;
        let page = summaries(&answer, &case);
        assert_eq!(
            answer
                .body
                .get("MilestoneNumber")
                .and_then(Value::as_i64)
                .map(usize::try_from)
                .and_then(Result::ok),
            Some(number),
            "case {case:?}: a milestone-scoped listing must name the milestone, got {answer:?}"
        );
        let frozen: Vec<&str> = (0..number).map(|_| "HIGH").collect();
        assert_eq!(
            risk_counts(&page[0]),
            expected_counts(framework_questions, &frozen),
            "case {case:?}: the milestone must report the reviews it recorded, got {page:?}"
        );
    }

    let live = summaries(&list_lens_reviews(&client, &workload_id, "").await, "live");
    let answered_live: Vec<&str> = (0..=questions.len()).map(|_| "HIGH").collect();
    assert_eq!(
        risk_counts(&live[0]),
        expected_counts(framework_questions, &answered_live),
        "case \"live\": the live listing must count every stored answer, got {live:?}"
    );
}

/// Rules 3 and 4: what the listing refuses. The workload and the milestone are RESOLVED, so
/// a miss is the modelled 404 rather than an empty page; `MilestoneNumber` and `MaxResults`
/// are checked at each published bound and one past it; and a `NextToken` this service did
/// not issue — including one another paginated operation issued — is the modelled 400.
#[tokio::test]
async fn a_listing_that_does_not_resolve_is_refused() {
    let client = Client::new();
    let workload_id =
        workload_with_lenses(&client, "lookup-host", &[FRAMEWORK_LENS.to_string()]).await;
    // Two milestones exist, so a request for a later number is a genuine miss and
    // `ListMilestones` has a cursor to issue.
    for number in 1..=2 {
        let recorded = create_milestone(
            &client,
            &workload_id,
            &format!("only-{number}"),
            &format!("tok-only-{number}"),
        )
        .await;
        assert_eq!(
            recorded.status, 200,
            "the host workload must hold milestone {number}, got {recorded:?}"
        );
    }

    for (case, workload, query) in [
        ("absent-workload", ABSENT_HEX_ID, ""),
        ("absent-workload-with-milestone", ABSENT_HEX_ID, "MilestoneNumber=1"),
        (
            "milestone-not-recorded",
            workload_id.as_str(),
            "MilestoneNumber=3",
        ),
        (
            "milestone-at-the-range-maximum",
            workload_id.as_str(),
            "MilestoneNumber=100",
        ),
    ] {
        let answer = list_lens_reviews(&client, workload, query).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a listing of what does not exist is a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: the miss must be the modelled error, got {answer:?}"
        );
    }

    // `MilestoneNumber` is `@range(min: 1, max: 100)` and `MaxResults` `@range(min: 1,
    // max: 50)`: each bound is inside the range (and answered below), one past it is not.
    let milestone_token = {
        let answer = client
            .send(
                "POST",
                &format!("/workloads/{workload_id}/milestonesSummaries"),
                Some(json!({ "MaxResults": 1 })),
            )
            .await;
        answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("ListMilestones must issue a cursor, got {answer:?}"))
            .to_string()
    };
    for (case, query) in [
        ("milestone-below-the-minimum", "MilestoneNumber=0".to_string()),
        ("milestone-above-the-maximum", "MilestoneNumber=101".to_string()),
        ("max-results-below-the-minimum", "MaxResults=0".to_string()),
        (
            "max-results-above-the-maximum",
            format!("MaxResults={}", MAX_RESULTS_CEILING + 1),
        ),
        ("token-this-service-did-not-issue", "NextToken=not-a-token".to_string()),
        (
            "token-of-another-paginated-operation",
            format!("NextToken={milestone_token}"),
        ),
    ] {
        let answer = list_lens_reviews(&client, &workload_id, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a request outside what the model allows is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: the refusal must be the modelled error, got {answer:?}"
        );
    }

    // The published bounds themselves are served, so every refusal above is the rule it
    // names rather than a broken read.
    for (case, query) in [
        ("milestone-at-the-minimum", "MilestoneNumber=1".to_string()),
        ("max-results-at-the-minimum", "MaxResults=1".to_string()),
        (
            "max-results-at-the-maximum",
            format!("MaxResults={MAX_RESULTS_CEILING}"),
        ),
    ] {
        let answer = list_lens_reviews(&client, &workload_id, &query).await;
        let page = summaries(&answer, case);
        assert_eq!(
            aliases(&page),
            vec![FRAMEWORK_LENS.to_string()],
            "case {case:?}: the host workload's one review must be served, got {answer:?}"
        );
    }
}
