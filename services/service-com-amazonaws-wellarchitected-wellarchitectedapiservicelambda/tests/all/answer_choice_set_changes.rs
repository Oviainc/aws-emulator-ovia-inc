// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a stored answer reports once the question's CHOICE SET
//! changes under it — which a lens-version upgrade does, because a lens review is a review
//! against ONE version of the lens and a custom lens's document defines the choices of
//! each question it asks.
//!
//! The rule these pin, over a TABLE of choice-set changes rather than one request: an
//! answer's per-choice state and the `Risk` derived from it are reported over the choices
//! the QUESTION asks about in the version of the lens the review is against — so a choice
//! that version does not define is not reported or rated, and one it defines that the
//! answer records no status for counts as unselected, since a best practice is not in place
//! until an answer says it is. Every reader of an answer follows it: `GetAnswer`,
//! `ListAnswers`, the risk roll-up `GetLensReview` reports and the improvement plan.
//!
//! The rows differ along the kind of change the new lens version makes to the question's
//! choice set — a choice added, several added, the unselected choice dropped, the selected
//! choice dropped, every choice replaced, the same set reordered and the same set unchanged
//! — because that is what the reported choices and the derived rating move with. The
//! answer is always written over the wire, so the choices it speaks about are the ones a
//! client discovered from the question's own catalog.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the workload seeding are the shared
//! `super::workload_harness`; the lens import and publish wire helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{import, imported_arn, wait_for_the_import_to_land};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, review_template, Answer, Client,
};

/// The pillar and the question every document below defines: the question whose choice set
/// the rows change between two published versions of the lens.
const PILLAR_ID: &str = "governance";
const QUESTION_ID: &str = "governance_q1";

/// The `Risk` members a question an answer speaks about can be rated at
/// (`smithy_query(query="shape", shape="Risk")` publishes these beside `UNANSWERED`),
/// and `UNANSWERED` itself — which a question whose answer says nothing about any choice
/// the new version asks about is back to.
const RISK_MEDIUM: &str = "MEDIUM";
const RISK_NONE: &str = "NONE";
const RISK_UNANSWERED: &str = "UNANSWERED";

/// Two of the `ChoiceStatus` members
/// (`smithy_query(query="shape", shape="ChoiceStatus")`).
const SELECTED: &str = "SELECTED";
const UNSELECTED: &str = "UNSELECTED";

/// A lens document defining ONE pillar with ONE question offering exactly `choices`, in
/// that order.
///
/// The shared `super::custom_lens_import::lens_document` builds a document from choice
/// COUNTS, deriving each `ChoiceId` from its position, which cannot express a version that
/// keeps a question's id while renaming or reordering its choices — the change these rows
/// are about — so the ids are spelled here instead. Everything else follows the same lens
/// JSON format specification: the `schemaVersion` the import requires, and one risk rule
/// per choice with the `default` condition on the last.
fn lens_document_offering(name: &str, choices: &[&str]) -> Value {
    let choices_json: Vec<Value> = choices
        .iter()
        .map(|choice_id| json!({ "id": choice_id, "title": format!("Choice {choice_id}") }))
        .collect();
    let mut rules: Vec<Value> = choices
        .iter()
        .map(|choice_id| json!({ "condition": choice_id, "risk": "NO_RISK" }))
        .collect();
    rules.push(json!({ "condition": "default", "risk": "HIGH_RISK" }));
    json!({
        "schemaVersion": "2021-11-01",
        "name": name,
        "pillars": [{
            "id": PILLAR_ID,
            "name": "Governance",
            "questions": [{
                "id": QUESTION_ID,
                "title": "How do you govern this workload?",
                "description": "what this question asks about",
                "choices": choices_json,
                "riskRules": rules,
            }],
        }],
    })
}

/// Import a lens offering `choices` and publish it as version `1`, answering its ARN.
async fn published_lens(client: &Client, case: &str, choices: &[&str]) -> String {
    let document = lens_document_offering(&format!("Lens {case}"), choices).to_string();
    let lens_arn = imported_arn(
        &import(client, &document, None, &format!("tok-import-{case}"), None).await,
        case,
    );
    wait_for_the_import_to_land();
    let published = create_version(
        client,
        &lens_arn,
        "1",
        Some(true),
        &format!("tok-publish-{case}"),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: CreateLensVersion must publish the imported lens, got {published:?}"
    );
    lens_arn
}

/// Re-import `lens_arn`'s document so the question offers `choices` instead, and publish
/// that as version `2` — the publish a review has to be UPGRADED to reach.
async fn publish_second_version(client: &Client, case: &str, lens_arn: &str, choices: &[&str]) {
    let document = lens_document_offering(&format!("Lens {case}"), choices).to_string();
    let reimported = import(
        client,
        &document,
        Some(lens_arn),
        &format!("tok-reimport-{case}"),
        None,
    )
    .await;
    assert_eq!(
        reimported.status, 200,
        "case {case:?}: re-importing the lens must be accepted, got {reimported:?}"
    );
    wait_for_the_import_to_land();
    let published = create_version(
        client,
        lens_arn,
        "2",
        Some(true),
        &format!("tok-publish2-{case}"),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: the second version must publish, got {published:?}"
    );
}

/// `UpdateAnswer`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
async fn update_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    selected: &[&str],
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(QUESTION_ID)
            ),
            Some(json!({ "SelectedChoices": selected })),
        )
        .await
}

/// `GetAnswer`: the same path read with `GET`.
async fn get_answer(client: &Client, workload_id: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(QUESTION_ID)
            ),
            None,
        )
        .await
}

/// `UpgradeLensReview`: `PUT /workloads/{WorkloadId}/lensReviews/{LensAlias}/upgrade`, with
/// the `MilestoneName` the operation requires.
async fn upgrade_lens_review(
    client: &Client,
    workload_id: &str,
    lens: &str,
    milestone: &str,
) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/workloads/{}/lensReviews/{}/upgrade",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            Some(json!({ "MilestoneName": milestone })),
        )
        .await
}

/// The `Answer` a read or a write reported.
fn answer_of(case: &str, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Answer must be present, got {answer:?}"))
}

/// One list-of-strings member, empty when the member is absent.
fn strings(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `ChoiceId`s a question's own catalog offers, in the order served.
fn catalog_ids(answer: &Value) -> Vec<String> {
    answer
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|choice| {
                    choice
                        .get("ChoiceId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The per-choice answers a read reported, as `(ChoiceId, Status)` in the order served.
fn choice_answers(answer: &Value, member: &str) -> Vec<(String, String)> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry
                            .get("ChoiceId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("Status")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One string member of a JSON object.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The risk roll-up of one lens review of a workload, as `GetLensReview` reports it.
async fn review_risk_counts(client: &Client, case: &str, workload_id: &str, lens: &str) -> Value {
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

/// The `(QuestionId, Risk)` of every item of one lens review's improvement plan.
async fn improvement_plan(
    client: &Client,
    case: &str,
    workload_id: &str,
    lens: &str,
) -> Vec<(String, String)> {
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
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| (text(summary, "QuestionId"), text(summary, "Risk")))
                .collect()
        })
        .unwrap_or_default()
}

/// The `AnswerSummary` `ListAnswers` reports for the question under test.
async fn listed_answer(client: &Client, case: &str, workload_id: &str, lens: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers",
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
    answer
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| text(summary, "QuestionId") == QUESTION_ID)
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {case:?}: {QUESTION_ID} must be listed, got {answer:?}"))
}

/// One row of the choice-set table: the choices the question offers in each published
/// version, which of version 1's choices the answer records as in place, and the rating
/// the answer carries before and after the review is moved to version 2.
struct ChoiceSetChange {
    case: &'static str,
    first_version: &'static [&'static str],
    in_place: &'static [&'static str],
    risk_before: &'static str,
    second_version: &'static [&'static str],
    risk_after: &'static str,
}

/// The rows: every kind of change a later lens version can make to one question's choice
/// set, each with an answer recorded against the FIRST version's choices.
const CHANGES: &[ChoiceSetChange] = &[
    // A choice the question did not ask about before has never been selected, so a
    // question that was fully answered is left with a best practice outstanding.
    ChoiceSetChange {
        case: "adds-a-choice",
        first_version: &["c1", "c2"],
        in_place: &["c1", "c2"],
        risk_before: RISK_NONE,
        second_version: &["c1", "c2", "c3"],
        risk_after: RISK_MEDIUM,
    },
    ChoiceSetChange {
        case: "adds-two-choices",
        first_version: &["c1"],
        in_place: &["c1"],
        risk_before: RISK_NONE,
        second_version: &["c1", "c2", "c3"],
        risk_after: RISK_MEDIUM,
    },
    // The dropped choice is no longer a best practice the question asks about, so it stops
    // holding the question's rating down.
    ChoiceSetChange {
        case: "drops-the-unselected-choice",
        first_version: &["c1", "c2"],
        in_place: &["c1"],
        risk_before: RISK_MEDIUM,
        second_version: &["c1"],
        risk_after: RISK_NONE,
    },
    // Dropping the one choice that WAS in place leaves the answer saying nothing about any
    // choice the new version asks about, so the question is unanswered against it again.
    ChoiceSetChange {
        case: "drops-the-selected-choice",
        first_version: &["c1", "c2"],
        in_place: &["c1"],
        risk_before: RISK_MEDIUM,
        second_version: &["c2"],
        risk_after: RISK_UNANSWERED,
    },
    ChoiceSetChange {
        case: "replaces-every-choice",
        first_version: &["c1", "c2"],
        in_place: &["c1", "c2"],
        risk_before: RISK_NONE,
        second_version: &["d1", "d2"],
        risk_after: RISK_UNANSWERED,
    },
    // The same best practices in another order are the same best practices: the rating
    // does not move, and the choices are reported in the new version's order.
    ChoiceSetChange {
        case: "reorders-the-same-choices",
        first_version: &["c1", "c2"],
        in_place: &["c1", "c2"],
        risk_before: RISK_NONE,
        second_version: &["c2", "c1"],
        risk_after: RISK_NONE,
    },
    // The control row: a version that asks the same question about the same choices
    // changes nothing about the answer.
    ChoiceSetChange {
        case: "keeps-the-same-choices",
        first_version: &["c1", "c2"],
        in_place: &["c1"],
        risk_before: RISK_MEDIUM,
        second_version: &["c1", "c2"],
        risk_after: RISK_MEDIUM,
    },
];

/// What every reader of the answer must report once the review is against `choices`: one
/// per-choice entry per choice of THAT version, in its order, `SELECTED` exactly where the
/// answer recorded a choice the version still asks about as in place.
fn expected_choice_answers(
    choices: &[&str],
    in_place: &[&str],
) -> Vec<(String, String)> {
    choices
        .iter()
        .map(|choice_id| {
            let status = if in_place.contains(choice_id) {
                SELECTED
            } else {
                UNSELECTED
            };
            ((*choice_id).to_string(), status.to_string())
        })
        .collect()
}

/// A workload reviewing one lens, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, case: &str, lens_arn: &str) -> String {
    let mut request = minimal_create(&format!("wl-{case}"), &format!("tok-wl-{case}"));
    request["Lenses"] = json!([lens_arn]);
    created_id(&client.create(request).await, case)
}

/// Rule: a workload's answer is reported and rated over the choices the question asks about
/// in the lens version its review is against. Moving the review to a version that defines
/// another choice set for the same question re-reports and re-rates the stored answer
/// accordingly, through every reader of it, and the answer can then be corrected over the
/// wire with the new version's own `ChoiceId`s.
#[tokio::test]
async fn a_lens_version_that_changes_a_questions_choices_re_rates_the_stored_answer() {
    for change in CHANGES {
        let ChoiceSetChange {
            case,
            first_version,
            in_place,
            risk_before,
            second_version,
            risk_after,
        } = change;
        let client = Client::new();
        let lens_arn = published_lens(&client, case, first_version).await;
        let workload_id = workload_reviewing(&client, case, &lens_arn).await;

        // The answer is written over the wire, with the `ChoiceId`s the question's own
        // catalog offers at version 1.
        let written = update_answer(&client, &workload_id, &lens_arn, in_place).await;
        let answer = answer_of(case, &written);
        assert_eq!(
            catalog_ids(&answer),
            first_version
                .iter()
                .map(|choice_id| (*choice_id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the question must offer the first version's choices, got {answer:?}"
        );
        assert_eq!(
            text(&answer, "Risk"),
            *risk_before,
            "case {case:?}: expected Risk {risk_before} before the upgrade, got {answer:?}"
        );

        // A later version of the lens asks the same question about another choice set, and
        // the upgrade is what moves the review to it.
        publish_second_version(&client, case, &lens_arn, second_version).await;
        let upgraded = upgrade_lens_review(
            &client,
            &workload_id,
            &lens_arn,
            &format!("before-the-upgrade-{case}"),
        )
        .await;
        assert_eq!(
            upgraded.status, 200,
            "case {case:?}: UpgradeLensReview must move the review, got {upgraded:?}"
        );

        let expected_catalog: Vec<String> = second_version
            .iter()
            .map(|choice_id| (*choice_id).to_string())
            .collect();
        let expected_answers = expected_choice_answers(second_version, in_place);
        let expected_selected: Vec<String> = expected_answers
            .iter()
            .filter(|(_, status)| status == SELECTED)
            .map(|(choice_id, _)| choice_id.clone())
            .collect();

        let read = answer_of(case, &get_answer(&client, &workload_id, &lens_arn).await);
        assert_eq!(
            catalog_ids(&read),
            expected_catalog,
            "case {case:?}: the question must offer the new version's choices, got {read:?}"
        );
        assert_eq!(
            choice_answers(&read, "ChoiceAnswers"),
            expected_answers,
            "case {case:?}: expected the per-choice answers {expected_answers:?}, got {read:?}"
        );
        assert_eq!(
            strings(&read, "SelectedChoices"),
            expected_selected,
            "case {case:?}: expected SelectedChoices {expected_selected:?}, got {read:?}"
        );
        assert_eq!(
            text(&read, "Risk"),
            *risk_after,
            "case {case:?}: expected Risk {risk_after} after the upgrade, got {read:?}"
        );

        // The listing of the same review reports the same answer, through its own
        // projection of it.
        let listed = listed_answer(&client, case, &workload_id, &lens_arn).await;
        assert_eq!(
            catalog_ids(&listed),
            expected_catalog,
            "case {case:?}: the listing must offer the new version's choices, got {listed:?}"
        );
        assert_eq!(
            choice_answers(&listed, "ChoiceAnswerSummaries"),
            expected_answers,
            "case {case:?}: the listing must report the same per-choice answers, got {listed:?}"
        );
        assert_eq!(
            text(&listed, "Risk"),
            *risk_after,
            "case {case:?}: the listing must report Risk {risk_after}, got {listed:?}"
        );

        // The review's roll-up counts the one question of this lens at that same rating,
        // and the improvement plan carries it exactly while the rating is an identified
        // risk.
        let counts = review_risk_counts(&client, case, &workload_id, &lens_arn).await;
        assert_eq!(
            counts.get(*risk_after).and_then(Value::as_i64),
            Some(1),
            "case {case:?}: the roll-up must count the question as {risk_after}, got {counts:?}"
        );
        let plan = improvement_plan(&client, case, &workload_id, &lens_arn).await;
        // Only an IDENTIFIED risk leaves something to improve: a question rated NONE has
        // nothing outstanding, and one the new version left unanswered is not yet rated.
        let expected_plan: Vec<(String, String)> =
            if *risk_after == RISK_NONE || *risk_after == RISK_UNANSWERED {
                Vec::new()
            } else {
                vec![(QUESTION_ID.to_string(), (*risk_after).to_string())]
            };
        assert_eq!(
            plan, expected_plan,
            "case {case:?}: expected the improvement plan {expected_plan:?}, got {plan:?}"
        );

        // And the answer is correctable: the new version's own choices are the ones an
        // update may name, so selecting every one of them clears the question's risk.
        let corrected = update_answer(&client, &workload_id, &lens_arn, second_version).await;
        let corrected = answer_of(case, &corrected);
        assert_eq!(
            text(&corrected, "Risk"),
            RISK_NONE,
            "case {case:?}: selecting every choice the question now asks about must clear \
             the risk, got {corrected:?}"
        );
        assert_eq!(
            strings(&corrected, "SelectedChoices"),
            expected_catalog,
            "case {case:?}: expected SelectedChoices {expected_catalog:?}, got {corrected:?}"
        );
    }
}

/// `UpdateReviewTemplateAnswer`:
/// `PATCH /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers/{QuestionId}`.
async fn update_template_answer(
    client: &Client,
    template_arn: &str,
    lens: &str,
    selected: &[&str],
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens),
                encode_path_segment(QUESTION_ID)
            ),
            Some(json!({ "SelectedChoices": selected })),
        )
        .await
}

/// `GetReviewTemplateAnswer`: the same path read with `GET`.
async fn get_template_answer(client: &Client, template_arn: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens),
                encode_path_segment(QUESTION_ID)
            ),
            None,
        )
        .await
}

/// Rule, on the review-template half of the same rule: a template's answer is reported
/// over the choices the question asks about in the version of the lens ITS review is
/// against, so `UpgradeReviewTemplateLensReview` re-reports the stored answer the same way
/// a workload's upgrade does. `ReviewTemplateAnswer` models no `Risk` member
/// (`smithy_query(query="shape", shape="ReviewTemplateAnswer")`), so what it reports is the
/// question's catalog and the answer's per-choice state. The rows are the same choice-set
/// changes.
#[tokio::test]
async fn a_review_templates_answer_follows_the_choice_set_of_the_version_it_is_upgraded_to() {
    for change in CHANGES {
        let ChoiceSetChange {
            case,
            first_version,
            in_place,
            second_version,
            ..
        } = change;
        let case = &format!("template-{case}");
        let client = Client::new();
        let lens_arn = published_lens(&client, case, first_version).await;
        let template_arn = review_template(&client, case, &[&lens_arn]).await;

        let written = update_template_answer(&client, &template_arn, &lens_arn, in_place).await;
        let answer = answer_of(case, &written);
        assert_eq!(
            choice_answers(&answer, "ChoiceAnswers"),
            expected_choice_answers(first_version, in_place),
            "case {case:?}: the answer must speak about the first version's choices, got \
             {answer:?}"
        );

        publish_second_version(&client, case, &lens_arn, second_version).await;
        let upgraded = client
            .send(
                "PUT",
                &format!(
                    "/reviewTemplates/{}/lensReviews/{}/upgrade",
                    encode_path_segment(&template_arn),
                    encode_path_segment(&lens_arn)
                ),
                Some(json!({ "ClientRequestToken": format!("tok-upgrade-{case}") })),
            )
            .await;
        assert_eq!(
            upgraded.status, 200,
            "case {case:?}: UpgradeReviewTemplateLensReview must move the review, got \
             {upgraded:?}"
        );

        let expected_catalog: Vec<String> = second_version
            .iter()
            .map(|choice_id| (*choice_id).to_string())
            .collect();
        let expected_answers = expected_choice_answers(second_version, in_place);
        let expected_selected: Vec<String> = expected_answers
            .iter()
            .filter(|(_, status)| status == SELECTED)
            .map(|(choice_id, _)| choice_id.clone())
            .collect();

        let read = answer_of(
            case,
            &get_template_answer(&client, &template_arn, &lens_arn).await,
        );
        assert_eq!(
            catalog_ids(&read),
            expected_catalog,
            "case {case:?}: the question must offer the new version's choices, got {read:?}"
        );
        assert_eq!(
            choice_answers(&read, "ChoiceAnswers"),
            expected_answers,
            "case {case:?}: expected the per-choice answers {expected_answers:?}, got {read:?}"
        );
        assert_eq!(
            strings(&read, "SelectedChoices"),
            expected_selected,
            "case {case:?}: expected SelectedChoices {expected_selected:?}, got {read:?}"
        );
    }
}
