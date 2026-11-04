// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAnswer`: the read of ONE answer of ONE lens review of a
//! workload.
//!
//! The rule pinned here, over tables of inputs rather than one request: a read of an
//! answer REPORTS the question as the lens under review defines it — its pillar, title and
//! whole choice catalog — together with the answer the reviewed workload stores for it,
//! where the review read is the live one or the one a milestone recorded, the lens resolves
//! by either spelling of its name (its alias or its ARN), and the read changes nothing a
//! later caller sees.
//!
//! Spelled out as the rows below vary it:
//!
//! * every question of every lens of the AWS-official catalog
//!   (`state::OFFICIAL_LENS_CATALOG`) is readable through a workload reviewing that lens,
//!   named by its alias or by its official ARN, and reports that lens's own content with
//!   the `UNANSWERED` reading of a question no answer was recorded for.
//! * a read is a REPORT: repeated reads of the same question answer identically, and
//!   neither the workload's `UpdatedAt`, nor its risk roll-up, nor the answers it stores
//!   move because something was read.
//! * a read scoped to a milestone reports the answer the milestone RECORDED, not the live
//!   one, for a question answered before it as well as for one answered after it, and
//!   echoes the milestone it was scoped to; a read of the live review names no milestone.
//! * a read that resolves no workload, no lens review of it, no question of that lens or
//!   no milestone of that workload is a 404 over the modelled resource type of whatever was
//!   missing, and a `MilestoneNumber` outside the published range is a 400 before any
//!   lookup.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::time::Duration;

use serde_json::{json, Value};

use super::workload_harness::{
    age_workload, assert_field_validation, assert_not_found, created_id, encode_path_segment,
    member, minimal_create, official_lens_arn, updated_at_of, Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The Region the shared harness signs its requests for, which is the Region the ARNs in a
/// response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// The Region the official lens ARNs below are SENT with, deliberately not the Region the
/// request is signed for: an AWS-official lens is AWS's resource rather than a Region's, so
/// either spelling names the same lens.
const SENT_ARN_REGION: &str = "us-east-1";

/// A well-formed `WorkloadId` (32 lowercase hex, the modelled `^[0-9a-f]{32}$`) that no
/// workload carries.
const ABSENT_HEX_ID: &str = "0123456789abcdef0123456789abcdef";

/// `GetAnswer`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`,
/// with the optional `MilestoneNumber` bound to the query string. Every label is
/// non-greedy, so each travels percent-encoded in one path segment, exactly as an SDK sends
/// it.
async fn get_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    milestone_number: Option<i64>,
) -> Answer {
    let suffix = match milestone_number {
        None => String::new(),
        Some(number) => format!("?MilestoneNumber={number}"),
    };
    client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}{suffix}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// `UpdateAnswer`: the same path written with `PATCH` — the operation that records the
/// answer these reads report.
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

/// `CreateMilestone`: `POST /workloads/{WorkloadId}/milestones`.
async fn create_milestone(client: &Client, workload_id: &str, name: &str) -> i64 {
    let answer = client
        .send(
            "POST",
            &format!("/workloads/{}/milestones", encode_path_segment(workload_id)),
            Some(json!({
                "MilestoneName": name,
                "ClientRequestToken": format!("tok-{name}"),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: CreateMilestone must succeed, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {name:?}: CreateMilestone must answer a MilestoneNumber"))
}

/// A workload reviewing exactly `lens`, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, lens: &str) -> String {
    let name = format!("wl-{lens}");
    let mut request = minimal_create(&name, &format!("tok-{name}"));
    request["Lenses"] = json!([lens]);
    created_id(&client.create(request).await, &name)
}

/// The `Answer` a read reported.
fn answer_of(answer: &Answer) -> Value {
    answer.body.get("Answer").cloned().unwrap_or(Value::Null)
}

/// One list-of-strings member of a JSON object, empty when it carries none.
fn strings(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One member of every entry of the reported `Answer$Choices`, in the order they are
/// reported.
fn choice_members(answer: &Value, name: &str) -> Vec<String> {
    answer
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|choice| {
                    choice
                        .get(name)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `ChoiceId` the lens defines for one question, in the lens's own order — read off
/// the lens content the service reports from, so a case states the question and not its
/// wire ids.
fn choice_ids_of(question: &state::FrameworkQuestion) -> Vec<String> {
    (0..question.best_practices.len())
        .map(|index| question.choice_id(index))
        .collect()
}

/// The question the framework lens asks under `question_id`.
fn framework_question(question_id: &str) -> &'static state::FrameworkQuestion {
    state::lens_question(state::FRAMEWORK_LENS_ALIAS, question_id)
        .unwrap_or_else(|| panic!("the framework lens must ask {question_id:?}"))
}

/// The `LensReview$RiskCounts` a workload's review of one lens reports: the roll-up a read
/// of a single answer must not move.
async fn risk_counts(client: &Client, workload_id: &str, lens: &str, case: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}",
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
        .and_then(|review| review.get("RiskCounts"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// How many answers the workload stores, over every lens it reviews — read off the shared
/// state, because the assertion is about what a READ recorded and an unanswered question
/// has no projection that would show it. Every request the test asserts about still
/// crosses the HTTP boundary.
fn stored_answer_count(client: &Client, workload_id: &str) -> usize {
    let state = client.store();
    let store = state.lock();
    store
        .workloads
        .get(workload_id)
        .map(|workload| workload.answers.values().map(|answers| answers.len()).sum())
        .unwrap_or_default()
}

/// Every question of every AWS-official lens this emulator carries content for is readable
/// through a workload reviewing that lens, under either spelling of the lens's name, and
/// reports that lens's own content.
///
/// The rows are the WHOLE catalog (`state::OFFICIAL_LENS_CATALOG`) — the framework lens and
/// every specialised lens beside it, including the ones the issue never cited — times every
/// question each of them asks, times the two spellings the `LensAlias` doc gives for an
/// official lens: the bare alias, and the account-less ARN. No answer has been recorded, so
/// each read reports the `UNANSWERED` reading of a question: the question's full choice
/// catalog, nothing selected and nothing answered per choice.
#[tokio::test]
async fn every_question_of_every_official_lens_is_reported_from_that_lenss_content() {
    let client = Client::new();
    for lens in state::OFFICIAL_LENS_CATALOG {
        let alias = lens.alias;
        let workload_id = workload_reviewing(&client, alias).await;
        assert!(
            !lens.questions.is_empty(),
            "case {alias:?}: a catalog lens must carry questions to read"
        );

        for question in lens.questions {
            for (form, named) in [
                ("alias", alias.to_string()),
                ("official-arn", official_lens_arn(SENT_ARN_REGION, alias)),
            ] {
                let case = format!("{alias}/{}/{form}", question.question_id);
                let read = get_answer(&client, &workload_id, &named, question.question_id, None)
                    .await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected GetAnswer 200, got {read:?}"
                );
                assert_eq!(
                    member(&read, "WorkloadId"),
                    workload_id,
                    "case {case:?}: the read must name the workload it read, got {read:?}"
                );
                assert_eq!(
                    member(&read, "LensAlias"),
                    named,
                    "case {case:?}: the read must echo the lens as sent, got {read:?}"
                );
                assert_eq!(
                    member(&read, "LensArn"),
                    official_lens_arn(REQUEST_REGION, alias),
                    "case {case:?}: the read must resolve the lens to its ARN in the Region the \
                     request was signed for, got {read:?}"
                );
                assert_eq!(
                    read.body.get("MilestoneNumber"),
                    None,
                    "case {case:?}: a read of the live review names no milestone, got {read:?}"
                );

                let answer = answer_of(&read);
                assert_eq!(
                    answer.get("QuestionId").and_then(Value::as_str),
                    Some(question.question_id),
                    "case {case:?}: the answer must name the question read, got {read:?}"
                );
                assert_eq!(
                    answer.get("PillarId").and_then(Value::as_str),
                    Some(question.pillar_id),
                    "case {case:?}: the answer must name the question's pillar, got {read:?}"
                );
                assert_eq!(
                    answer.get("QuestionTitle").and_then(Value::as_str),
                    Some(question.title),
                    "case {case:?}: the answer must carry the question's title, got {read:?}"
                );
                assert_eq!(
                    choice_members(&answer, "ChoiceId"),
                    choice_ids_of(question),
                    "case {case:?}: the answer must report the question's whole choice catalog, \
                     got {read:?}"
                );
                assert_eq!(
                    choice_members(&answer, "Title"),
                    question.best_practices,
                    "case {case:?}: each reported choice must carry the best practice it asks \
                     about, got {read:?}"
                );
                assert_eq!(
                    answer.get("Risk").and_then(Value::as_str),
                    Some(state::RISK_UNANSWERED),
                    "case {case:?}: a question no answer was recorded for is UNANSWERED, got \
                     {read:?}"
                );
                assert_eq!(
                    answer.get("IsApplicable").and_then(Value::as_bool),
                    Some(true),
                    "case {case:?}: a question applies until an answer says it does not, got \
                     {read:?}"
                );
                assert_eq!(
                    strings(&answer, "SelectedChoices"),
                    Vec::<String>::new(),
                    "case {case:?}: an unanswered question has no choice in place, got {read:?}"
                );
                assert_eq!(
                    answer.get("ChoiceAnswers"),
                    Some(&json!([])),
                    "case {case:?}: an unanswered question answers no choice, got {read:?}"
                );
                assert_eq!(
                    answer.get("Notes"),
                    None,
                    "case {case:?}: notes exist only once an answer records them, got {read:?}"
                );
                assert_eq!(
                    answer.get("Reason"),
                    None,
                    "case {case:?}: a reason exists only once an answer records one, got {read:?}"
                );
            }
        }
    }
}

/// A read REPORTS: it answers the same thing every time and changes nothing a later caller
/// sees.
///
/// The rows differ along whether the question read has a stored answer and along which
/// review is read (the live one, or the one a milestone recorded) — the four combinations —
/// and each row is read four times over. The workload's `UpdatedAt` is moved back before
/// the reads, so a write the handler made to it would be visible whatever the clock's
/// granularity between two in-process requests, and the roll-up and the stored answers are
/// compared before and after.
#[tokio::test]
async fn a_read_of_an_answer_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let workload_id = workload_reviewing(&client, lens).await;

    // One answered question, so the rows below span an answer that exists and one that does
    // not.
    let answered = framework_question("ops-01");
    let unanswered = framework_question("sec-02");
    let recorded = update_answer(
        &client,
        &workload_id,
        lens,
        answered.question_id,
        json!({
            "SelectedChoices": [answered.choice_id(0)],
            "Notes": "the first best practice is in place",
        }),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "UpdateAnswer must record the answer the reads report, got {recorded:?}"
    );
    let milestone = create_milestone(&client, &workload_id, "read-only-milestone").await;

    age_workload(&client, &workload_id, Duration::from_secs(600));
    let updated_at_before = updated_at_of(&client, &workload_id, "before-the-reads").await;
    let risk_counts_before = risk_counts(&client, &workload_id, lens, "before-the-reads").await;
    let stored_before = stored_answer_count(&client, &workload_id);

    for (case, question_id, milestone_number) in [
        ("answered-live", answered.question_id, None),
        ("answered-milestone", answered.question_id, Some(milestone)),
        ("unanswered-live", unanswered.question_id, None),
        (
            "unanswered-milestone",
            unanswered.question_id,
            Some(milestone),
        ),
    ] {
        let first = get_answer(&client, &workload_id, lens, question_id, milestone_number).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected GetAnswer 200, got {first:?}"
        );
        for repeat in 1..=3 {
            let again = get_answer(&client, &workload_id, lens, question_id, milestone_number).await;
            assert_eq!(
                again.status, first.status,
                "case {case:?}: read {repeat} must answer the same status, got {again:?}"
            );
            assert_eq!(
                again.body, first.body,
                "case {case:?}: read {repeat} must report exactly what the first read did, got \
                 {again:?}"
            );
        }
    }

    assert_eq!(
        updated_at_of(&client, &workload_id, "after-the-reads").await,
        updated_at_before,
        "a read must not move the workload's UpdatedAt"
    );
    assert_eq!(
        risk_counts(&client, &workload_id, lens, "after-the-reads").await,
        risk_counts_before,
        "a read must not move the lens review's risk roll-up"
    );
    assert_eq!(
        stored_answer_count(&client, &workload_id),
        stored_before,
        "a read of an unanswered question must not record an answer for it"
    );
}

/// A read scoped to a milestone reports the answer that milestone RECORDED, and a read of
/// the live review reports the answer as it stands now.
///
/// The rows are the two questions a milestone separates — one answered before it and
/// changed after it, one answered only after it — times the two reviews a request can
/// scope to, so the recorded and the live reading of the same question are both exercised
/// and a question the milestone recorded no answer for is `UNANSWERED` in it.
#[tokio::test]
async fn a_milestone_scoped_read_reports_the_review_the_milestone_recorded() {
    let client = Client::new();
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let workload_id = workload_reviewing(&client, lens).await;

    let before_the_milestone = framework_question("rel-01");
    let after_the_milestone = framework_question("cost-01");
    let every_choice_before = choice_ids_of(before_the_milestone);
    let every_choice_after = choice_ids_of(after_the_milestone);

    // Answered with every best practice in place, so the milestone records an answer at
    // NONE risk.
    let recorded = update_answer(
        &client,
        &workload_id,
        lens,
        before_the_milestone.question_id,
        json!({ "SelectedChoices": every_choice_before }),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "UpdateAnswer must record the answer the milestone captures, got {recorded:?}"
    );

    let milestone = create_milestone(&client, &workload_id, "before-the-later-answers").await;

    // After the milestone: the first question's answer drops every selection, and the
    // second question is answered for the first time.
    for (question_id, selected) in [
        (before_the_milestone.question_id, Vec::new()),
        (after_the_milestone.question_id, every_choice_after.clone()),
    ] {
        let written = update_answer(
            &client,
            &workload_id,
            lens,
            question_id,
            json!({ "SelectedChoices": selected }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {question_id:?}: UpdateAnswer must record the later answer, got {written:?}"
        );
    }

    for (case, question_id, milestone_number, expected_selected, expected_risk) in [
        (
            "answered-before-and-changed-after-live",
            before_the_milestone.question_id,
            None,
            Vec::new(),
            state::RISK_HIGH,
        ),
        (
            "answered-before-and-changed-after-milestone",
            before_the_milestone.question_id,
            Some(milestone),
            every_choice_before.clone(),
            state::RISK_NONE,
        ),
        (
            "answered-after-the-milestone-live",
            after_the_milestone.question_id,
            None,
            every_choice_after.clone(),
            state::RISK_NONE,
        ),
        (
            "answered-after-the-milestone-milestone",
            after_the_milestone.question_id,
            Some(milestone),
            Vec::new(),
            state::RISK_UNANSWERED,
        ),
    ] {
        let read = get_answer(&client, &workload_id, lens, question_id, milestone_number).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAnswer 200, got {read:?}"
        );
        assert_eq!(
            read.body.get("MilestoneNumber").and_then(Value::as_i64),
            milestone_number,
            "case {case:?}: a read must name the milestone it was scoped to and no other, got \
             {read:?}"
        );
        let answer = answer_of(&read);
        assert_eq!(
            strings(&answer, "SelectedChoices"),
            expected_selected,
            "case {case:?}: the read must report the choices that review records as in place, got \
             {read:?}"
        );
        assert_eq!(
            answer.get("Risk").and_then(Value::as_str),
            Some(expected_risk),
            "case {case:?}: the read must report the risk that review's answer carries, got \
             {read:?}"
        );
    }
}

/// A read that resolves nothing is refused: a 404 over the modelled resource type of the
/// RESOURCE that was missing, and the modelled 400 naming the member that carried a value
/// the lens content does not define. Either way it records nothing.
///
/// The rows differ along WHICH part of the `(WorkloadId, LensAlias, QuestionId,
/// MilestoneNumber)` identity fails to resolve: a workload id no workload carries, a lens
/// the workload does not review (named both ways), a question the reviewed lens does not
/// ask (a word no lens asks, and a question another lens asks), and a milestone number the
/// workload never recorded — including the published upper bound of that range, which is a
/// lookup miss rather than a bad value.
#[tokio::test]
async fn a_read_that_resolves_nothing_is_refused_over_the_resource_that_was_missing() {
    let client = Client::new();
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let workload_id = workload_reviewing(&client, lens).await;
    let question_id = framework_question("sec-01").question_id;
    let recorded_milestone = create_milestone(&client, &workload_id, "the-one-milestone").await;

    // A lens of the official catalog this workload does not review, so it names no review
    // of this workload — and a question that lens asks, which the reviewed lens does not.
    let unreviewed_lens = "serverless";
    let unreviewed_lens_question = state::lens_question(unreviewed_lens, "serverless-ops-01")
        .expect("the serverless lens must ask serverless-ops-01")
        .question_id;

    // The workload id does not resolve.
    assert_not_found(
        "absent-workload",
        &get_answer(&client, ABSENT_HEX_ID, lens, question_id, None).await,
        state::WORKLOAD_RESOURCE_TYPE,
        ABSENT_HEX_ID,
        &format!("workload with ID {ABSENT_HEX_ID}"),
    );

    // The lens names no review OF this workload, under either spelling of its name: the
    // lens review is a VALUE of the request, so it is refused as input.
    for named in [
        unreviewed_lens.to_string(),
        official_lens_arn(SENT_ARN_REGION, unreviewed_lens),
    ] {
        assert_field_validation(
            &format!("unreviewed-lens-{named}"),
            &get_answer(&client, &workload_id, &named, unreviewed_lens_question, None).await,
            "LensAlias",
            &format!(
                "No lens review for lens alias {named} was found in workload {workload_id}."
            ),
        );
    }

    // The question is not one the reviewed lens asks: the questions are the LENS's content,
    // so the finding names `QuestionId` and the lens it was looked for in.
    for unasked in ["not-a-question", unreviewed_lens_question] {
        assert_field_validation(
            &format!("unasked-question-{unasked}"),
            &get_answer(&client, &workload_id, lens, unasked, None).await,
            "QuestionId",
            &format!("No question with ID {unasked} was found in the lens with alias {lens}."),
        );
    }

    // The milestone number is inside the published range and names no recorded milestone:
    // the one past the single milestone this workload recorded, and the range's upper bound.
    for number in [recorded_milestone + 1, 100] {
        let answer = get_answer(&client, &workload_id, lens, question_id, Some(number)).await;
        let missing = i32::try_from(number).expect("a milestone number is modelled an i32");
        assert_not_found(
            &format!("absent-milestone-{number}"),
            &answer,
            state::MILESTONE_RESOURCE_TYPE,
            &state::milestone_resource_id(&workload_id, missing),
            &format!("milestone with number {number} in a workload with ID {workload_id}"),
        );
    }
}

/// A `MilestoneNumber` outside the range the model publishes is refused as a field
/// validation, before any lookup: `MilestoneNumber` is `@range(min: 1, max: 100)` — "A
/// workload can have a maximum of 100 milestones"
/// (`smithy_query(query="operation", operation="GetAnswer")`).
///
/// Each published bound is tested ON the bound — where the request is accepted, so what it
/// then answers is about the milestone and not about the value — and one step past it,
/// beside a value far outside the range on each side.
#[tokio::test]
async fn a_milestone_number_outside_the_published_range_is_refused() {
    let client = Client::new();
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let workload_id = workload_reviewing(&client, lens).await;
    let question_id = framework_question("sus-01").question_id;
    let recorded = create_milestone(&client, &workload_id, "the-first-milestone").await;
    assert_eq!(
        recorded, 1,
        "the first milestone of a workload must be number 1, got {recorded}"
    );

    for number in [-1, 0, 101, 1_000] {
        let answer = get_answer(&client, &workload_id, lens, question_id, Some(number)).await;
        assert_eq!(
            answer.status, 400,
            "case {number}: a MilestoneNumber outside the published range must be refused, got \
             {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {number}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {number}: a value outside a published range is a field validation, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields"),
            Some(&json!([{
                "Name": "MilestoneNumber",
                "Message": "MilestoneNumber must be between 1 and 100.",
            }])),
            "case {number}: Fields must name the member and its bounds, got {answer:?}"
        );
    }

    // ON the lower bound the value is accepted and reads the milestone that exists; ON the
    // upper bound it is accepted too and answers the lookup miss asserted above.
    let read = get_answer(&client, &workload_id, lens, question_id, Some(recorded)).await;
    assert_eq!(
        read.status, 200,
        "case 1: the range's lower bound names the recorded milestone, got {read:?}"
    );
    assert_eq!(
        read.body.get("MilestoneNumber").and_then(Value::as_i64),
        Some(recorded),
        "case 1: the read must name the milestone it was scoped to, got {read:?}"
    );
    let upper = get_answer(&client, &workload_id, lens, question_id, Some(100)).await;
    assert_eq!(
        upper.status, 404,
        "case 100: the range's upper bound is a lookup, not a bad value, got {upper:?}"
    );
}
