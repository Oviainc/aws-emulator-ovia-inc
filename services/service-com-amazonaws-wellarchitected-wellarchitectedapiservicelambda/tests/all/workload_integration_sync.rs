// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateIntegration`: the sync of ONE workload with an
//! integrated service.
//!
//! The rule pinned here, over tables of inputs rather than one request: a sync of a
//! resource with an integrated service RECORDS, against every question of that resource's
//! reviews the integration covers, the instant the sync was served, which every later read
//! of those answers reports and no read moves; a sync of a resource the service does not
//! hold is the modelled 404, and one idempotency token syncs once.
//!
//! Spelled out as the rows below vary it:
//!
//! * a sync covers every question of every lens the workload reviews — the rows are the
//!   whole AWS-official catalog (`state::OFFICIAL_LENS_CATALOG`), one lens at a time and
//!   two lenses at once — and reports ONE instant for all of them, with no issue URL,
//!   whether the answer is read singly (`GetAnswer`), in a page (`ListAnswers`) or as an
//!   improvement item (`ListLensReviewImprovements`).
//! * under the manual issue management type the questions covered are exactly the ones the
//!   review's own selection names: one question of one pillar, two of one pillar, one of
//!   each of two pillars, a pillar selecting nothing, no pillar selected, and a review with
//!   no selection recorded at all — and switching the workload back to the automatic type
//!   covers every question again. The issue management STATUS narrows nothing, over every
//!   member that enum publishes.
//! * a read is a REPORT: repeated reads of a synced question answer identically and the
//!   workload's `UpdatedAt` does not move because something was read.
//! * one `ClientRequestToken` syncs ONCE: a retry carrying it with the same parameters
//!   replays the original empty success without moving the instant it recorded, the same
//!   token with different parameters is the modelled 409, and a fresh token syncs again.
//! * a sync naming a workload the store does not hold is a 404 over the workload, and
//!   syncs nothing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::workload_harness::{
    assert_not_found, created_id, encode_path_segment, member, minimal_create, updated_at_of,
    Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The only `IntegratingService` the model publishes
/// (`smithy_query(query="shape", shape="IntegratingService")`), so the one integration a
/// request can ask to sync.
const JIRA: &str = "JIRA";

/// A well-formed `WorkloadId` (32 lowercase hex, the modelled `^[0-9a-f]{32}$`) that no
/// workload carries.
const ABSENT_HEX_ID: &str = "fedcba9876543210fedcba9876543210";

/// `UpdateIntegration`: `POST /workloads/{WorkloadId}/updateIntegration`. The id is a
/// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment, exactly as
/// an SDK sends it.
async fn update_integration(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "/workloads/{}/updateIntegration",
                encode_path_segment(workload_id)
            ),
            Some(body),
        )
        .await
}

/// A sync that must succeed, and the empty body it answers with.
async fn sync(client: &Client, workload_id: &str, token: &str, case: &str) {
    let answer = update_integration(
        client,
        workload_id,
        json!({ "ClientRequestToken": token, "IntegratingService": JIRA }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateIntegration 200, got {answer:?}"
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case:?}: UpdateIntegration answers an empty body, got {answer:?}"
    );
}

/// A workload reviewing exactly `lenses`, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, case: &str, lenses: &[&str]) -> String {
    let name = format!("wl-{case}");
    let mut request = minimal_create(&name, &format!("tok-{name}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, case)
}

/// The `JiraConfiguration` one question of a lens review reports through `GetAnswer`,
/// `Value::Null` when it reports none.
async fn answer_jira(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    case: &str,
) -> Value {
    let answer = client
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
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetAnswer 200, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .and_then(|reported| reported.get("JiraConfiguration"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every entry of a paged listing, following `NextToken` to the end so the assertions are
/// about the WHOLE review rather than one page.
async fn listed(client: &Client, path: &str, member_name: &str, case: &str) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut next_token = None::<String>;
    loop {
        let uri = match next_token.as_deref() {
            None => path.to_string(),
            Some(token) => format!("{path}?NextToken={}", encode_path_segment(token)),
        };
        let answer = client.send("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {member_name} listing 200, got {answer:?}"
        );
        entries.extend(
            answer
                .body
                .get(member_name)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return entries;
        }
    }
}

/// `QuestionId` -> the `JiraConfiguration` that question reports, over every page of one
/// listing route.
async fn jira_by_question(
    client: &Client,
    workload_id: &str,
    lens: &str,
    route: &str,
    member_name: &str,
    case: &str,
) -> BTreeMap<String, Value> {
    let path = format!(
        "/workloads/{}/lensReviews/{}/{route}",
        encode_path_segment(workload_id),
        encode_path_segment(lens)
    );
    listed(client, &path, member_name, case)
        .await
        .into_iter()
        .map(|entry| {
            (
                entry
                    .get("QuestionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("JiraConfiguration")
                    .cloned()
                    .unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// The `QuestionId`s of one lens review that report a sync, read through `ListAnswers`.
async fn synced_questions(
    client: &Client,
    workload_id: &str,
    lens: &str,
    case: &str,
) -> BTreeSet<String> {
    jira_by_question(client, workload_id, lens, "answers", "AnswerSummaries", case)
        .await
        .into_iter()
        .filter(|(_, jira)| !jira.is_null())
        .map(|(question_id, _)| question_id)
        .collect()
}

/// The `LastSyncedTime` a reported `JiraConfiguration` carries, as the epoch seconds
/// restJson1 carries it — and the assertion that it carries no issue URL, which is the
/// member this emulator holds no Jira connection to fill.
fn last_synced(jira: &Value, case: &str) -> f64 {
    assert!(
        jira.get("JiraIssueUrl").is_none(),
        "case {case:?}: a synced question reports no JiraIssueUrl, got {jira:?}"
    );
    jira.get("LastSyncedTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: a synced question must report LastSyncedTime"))
}

/// `UpdateWorkload` recording this workload's Jira configuration, which is what decides
/// whether a sync covers every question of a review or only the selected ones.
async fn record_jira_configuration(
    client: &Client,
    workload_id: &str,
    issue_management_status: &str,
    issue_management_type: &str,
    case: &str,
) {
    let answer = client
        .update(
            workload_id,
            json!({
                "JiraConfiguration": {
                    "IssueManagementStatus": issue_management_status,
                    "IssueManagementType": issue_management_type,
                    "JiraProjectKey": "WAT",
                }
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateWorkload 200, got {answer:?}"
    );
}

/// `UpdateLensReview` recording the questions of one review selected for the integrated
/// service (`LensReview$JiraConfiguration` -> `SelectedPillar$SelectedQuestionIds`).
async fn record_selection(
    client: &Client,
    workload_id: &str,
    lens: &str,
    selection: &[(&str, Vec<&str>)],
    case: &str,
) {
    let selected_pillars: Vec<Value> = selection
        .iter()
        .map(|(pillar_id, question_ids)| {
            json!({ "PillarId": pillar_id, "SelectedQuestionIds": question_ids })
        })
        .collect();
    let answer = client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            Some(json!({ "JiraConfiguration": { "SelectedPillars": selected_pillars } })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateLensReview 200, got {answer:?}"
    );
}

/// Every `QuestionId` one lens asks, read off the lens content the service reports from,
/// so a case states the lens and not its question list.
fn question_ids(lens: &str) -> BTreeSet<String> {
    state::lens_questions(lens)
        .iter()
        .map(|question| question.question_id.to_string())
        .collect()
}

/// The questions of the framework lens grouped by the pillar that asks them, in the lens's
/// own pillar order — what the selection rows below are built from.
fn questions_by_pillar() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut grouped: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for question in state::lens_questions(state::FRAMEWORK_LENS_ALIAS) {
        grouped
            .entry(question.pillar_id)
            .or_default()
            .push(question.question_id);
    }
    grouped
}

/// Move the sync instants this workload's reviews recorded back by `age`, so a LATER sync's
/// instant is observably greater whatever the clock's granularity between two in-process
/// requests. Only the SEED reaches state directly; every sync and every read the assertions
/// are about crosses the HTTP boundary.
fn age_syncs(client: &Client, workload_id: &str, age: Duration) {
    let state = client.store();
    let mut store = state.lock();
    let workload = store
        .workloads
        .get_mut(workload_id)
        .expect("the workload under test must exist");
    let aged = SystemTime::now() - age;
    for review in workload.lens_review_records.values_mut() {
        for synced_at in review.jira_question_syncs.values_mut() {
            *synced_at = aged;
        }
    }
}

/// A sync covers every question of every lens the workload reviews, and reports ONE instant
/// for all of them.
///
/// The rows are the WHOLE AWS-official catalog (`state::OFFICIAL_LENS_CATALOG`), including
/// the lenses the issue never cited: before the sync no question of the review reports a
/// Jira configuration at all, and after it every question of that lens does, through the
/// single answer read and the answer listing alike.
#[tokio::test]
async fn a_sync_covers_every_question_of_the_reviewed_lens() {
    let client = Client::new();
    for lens in state::OFFICIAL_LENS_CATALOG {
        let alias = lens.alias;
        let asked = question_ids(alias);
        assert!(
            !asked.is_empty(),
            "case {alias:?}: a catalog lens must ask questions to sync"
        );
        let workload_id = workload_reviewing(&client, alias, &[alias]).await;

        assert_eq!(
            synced_questions(&client, &workload_id, alias, alias).await,
            BTreeSet::new(),
            "case {alias:?}: a workload nothing has synced reports no synced question"
        );
        let first_question = asked
            .iter()
            .next()
            .expect("the lens asks at least one question");
        assert_eq!(
            answer_jira(&client, &workload_id, alias, first_question, alias).await,
            Value::Null,
            "case {alias:?}: an unsynced question reports no JiraConfiguration"
        );

        sync(&client, &workload_id, &format!("tok-{alias}"), alias).await;

        let listed = jira_by_question(
            &client,
            &workload_id,
            alias,
            "answers",
            "AnswerSummaries",
            alias,
        )
        .await;
        assert_eq!(
            listed.keys().cloned().collect::<BTreeSet<String>>(),
            asked,
            "case {alias:?}: the listing must report every question of the lens"
        );
        let mut instants = BTreeSet::new();
        for (position, (question_id, jira)) in listed.iter().enumerate() {
            let case = format!("{alias}/{question_id}");
            let synced_at = last_synced(jira, &case);
            instants.insert(synced_at.to_bits());
            // The listing is the reading asserted for every question; the single answer
            // read is cross-checked against it on the first few, which is what keeps the
            // suite's request count bounded as the catalog grows.
            if position < 3 {
                assert_eq!(
                    answer_jira(&client, &workload_id, alias, question_id, &case).await,
                    *jira,
                    "case {case:?}: the single answer read and the listing must agree, got {jira:?}"
                );
            }
        }
        assert_eq!(
            instants.len(),
            1,
            "case {alias:?}: one sync reports one instant, got {instants:?}"
        );
    }
}

/// A sync is of the WORKLOAD, so it covers each lens review the workload carries: a
/// workload reviewing two lenses has both reviews synced by one request, at one instant.
#[tokio::test]
async fn a_sync_covers_every_lens_the_workload_reviews() {
    let client = Client::new();
    let case = "two-lenses";
    let lenses: Vec<&str> = state::OFFICIAL_LENS_CATALOG
        .iter()
        .take(2)
        .map(|lens| lens.alias)
        .collect();
    assert_eq!(
        lenses.len(),
        2,
        "case {case:?}: the catalog must carry two lenses to review at once"
    );
    let workload_id = workload_reviewing(&client, case, &lenses).await;

    sync(&client, &workload_id, "tok-two-lenses", case).await;

    let mut instants = BTreeSet::new();
    for lens in &lenses {
        let per_lens = format!("{case}/{lens}");
        assert_eq!(
            synced_questions(&client, &workload_id, lens, &per_lens).await,
            question_ids(lens),
            "case {per_lens:?}: every question of each reviewed lens must be synced"
        );
        for jira in jira_by_question(
            &client,
            &workload_id,
            lens,
            "answers",
            "AnswerSummaries",
            &per_lens,
        )
        .await
        .values()
        {
            instants.insert(last_synced(jira, &per_lens).to_bits());
        }
    }
    assert_eq!(
        instants.len(),
        1,
        "case {case:?}: one sync reports one instant across every review, got {instants:?}"
    );
}

/// Under the manual issue management type a sync covers EXACTLY the questions the review's
/// own selection names; under the automatic one it covers every question the lens asks.
///
/// The rows vary the selection along its shape — how many pillars it names and how many
/// questions under each — and include the two forms that select nothing, where the sync
/// covers no question and therefore leaves the workload as it stood.
#[tokio::test]
async fn manual_issue_management_syncs_exactly_the_selected_questions() {
    let client = Client::new();
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let grouped = questions_by_pillar();
    let mut pillars = grouped.iter();
    let (first_pillar, first_questions) = pillars
        .next()
        .expect("the framework lens must be organized into pillars");
    let (second_pillar, second_questions) = pillars
        .next()
        .expect("the framework lens must ask about more than one pillar");
    assert!(
        first_questions.len() >= 2,
        "the first pillar must ask at least two questions, got {first_questions:?}"
    );

    // Each row: the case, the selection recorded against the review (None records none at
    // all), and the questions the sync must then cover.
    type SelectionRow<'a> = (
        &'a str,
        Option<Vec<(&'a str, Vec<&'a str>)>>,
        BTreeSet<String>,
    );
    let rows: Vec<SelectionRow<'_>> = vec![
        (
            "one question of one pillar",
            Some(vec![(*first_pillar, vec![first_questions[0]])]),
            [first_questions[0].to_string()].into_iter().collect(),
        ),
        (
            "two questions of one pillar",
            Some(vec![(
                *first_pillar,
                vec![first_questions[0], first_questions[1]],
            )]),
            [
                first_questions[0].to_string(),
                first_questions[1].to_string(),
            ]
            .into_iter()
            .collect(),
        ),
        (
            "one question of each of two pillars",
            Some(vec![
                (*first_pillar, vec![first_questions[0]]),
                (*second_pillar, vec![second_questions[0]]),
            ]),
            [
                first_questions[0].to_string(),
                second_questions[0].to_string(),
            ]
            .into_iter()
            .collect(),
        ),
        (
            "a pillar selecting no question",
            Some(vec![(*first_pillar, Vec::new())]),
            BTreeSet::new(),
        ),
        ("no pillar selected", Some(Vec::new()), BTreeSet::new()),
        ("no selection recorded at all", None, BTreeSet::new()),
    ];

    for (case, selection, expected) in rows {
        let workload_id = workload_reviewing(&client, case, &[lens]).await;
        record_jira_configuration(
            &client,
            &workload_id,
            "ENABLED",
            state::ISSUE_MANAGEMENT_TYPE_MANUAL,
            case,
        )
        .await;
        if let Some(selection) = selection.as_deref() {
            record_selection(&client, &workload_id, lens, selection, case).await;
        }
        let before = updated_at_of(&client, &workload_id, case).await;

        sync(&client, &workload_id, &format!("tok-{case}"), case).await;

        assert_eq!(
            synced_questions(&client, &workload_id, lens, case).await,
            expected,
            "case {case:?}: a manual sync covers exactly the selected questions"
        );
        let after = updated_at_of(&client, &workload_id, case).await;
        if expected.is_empty() {
            assert_eq!(
                after, before,
                "case {case:?}: a sync that covers no question writes nothing, so UpdatedAt \
                 must not move"
            );
        }

        // The same review under the AUTOMATIC type: the selection no longer narrows
        // anything, so a fresh sync covers every question the lens asks.
        record_jira_configuration(&client, &workload_id, "ENABLED", "AUTO", case).await;
        sync(&client, &workload_id, &format!("tok-auto-{case}"), case).await;
        assert_eq!(
            synced_questions(&client, &workload_id, lens, case).await,
            question_ids(lens),
            "case {case:?}: an automatic sync covers every question of the lens"
        );
    }
}

/// The issue management STATUS does not narrow an explicit sync: a call that asks for one
/// is the caller asking outright, where the status governs whether the service manages a
/// question's issue on its own.
///
/// The rows are every member `WorkloadIssueManagementStatus` publishes
/// (`smithy_query(query="shape", shape="WorkloadIssueManagementStatus")`), including the
/// one that inherits the account setting no request can turn on, so each shape is pinned
/// rather than left to the one status the rows above record.
#[tokio::test]
async fn the_issue_management_status_does_not_narrow_a_sync() {
    let client = Client::new();
    let lens = state::FRAMEWORK_LENS_ALIAS;
    // The type the sync reads is one the enum publishes, read off the generated value set
    // so the constant the state names it by cannot drift from the model.
    assert!(
        service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ops::types::ISSUE_MANAGEMENT_TYPE_VALUES
            .contains(&state::ISSUE_MANAGEMENT_TYPE_MANUAL),
        "the manual issue management type must be a member IssueManagementType publishes, got {:?}",
        state::ISSUE_MANAGEMENT_TYPE_MANUAL
    );
    for status in ["ENABLED", "DISABLED", "INHERIT"] {
        let case = format!("status-{status}");
        let workload_id = workload_reviewing(&client, &case, &[lens]).await;
        record_jira_configuration(&client, &workload_id, status, "AUTO", &case).await;

        sync(&client, &workload_id, &format!("tok-{case}"), &case).await;

        assert_eq!(
            synced_questions(&client, &workload_id, lens, &case).await,
            question_ids(lens),
            "case {case:?}: a sync covers every question of the lens whatever the status"
        );
    }
}

/// A read of a synced question is a REPORT: it answers the same `JiraConfiguration` however
/// often and through whichever route it is read, and nothing a read does moves the
/// workload's `UpdatedAt`.
///
/// The improvement plan is read too, because an answered question at risk reports the same
/// integration state there as it does in the answer it belongs to.
#[tokio::test]
async fn reads_of_a_synced_question_report_the_same_sync() {
    let client = Client::new();
    let case = "reports";
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let workload_id = workload_reviewing(&client, case, &[lens]).await;
    let question = state::lens_questions(lens)
        .first()
        .expect("the framework lens must ask a question");
    let question_id = question.question_id;

    // An answer that leaves one best practice of several unselected is at `MEDIUM` risk,
    // which is what puts the question on the improvement plan: an answer that selects
    // nothing at all would leave the question unanswered, and so unrated.
    let in_place: Vec<String> = (0..question.best_practices.len().saturating_sub(1))
        .map(|position| question.choice_id(position))
        .collect();
    assert!(
        !in_place.is_empty(),
        "case {case:?}: the question under test must offer more than one best practice"
    );
    let answered = client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(&workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(json!({
                "Notes": "an answer under test",
                "SelectedChoices": in_place,
            })),
        )
        .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected UpdateAnswer 200, got {answered:?}"
    );

    sync(&client, &workload_id, "tok-reports", case).await;

    let reported = answer_jira(&client, &workload_id, lens, question_id, case).await;
    let synced_at = last_synced(&reported, case);
    let after_sync = updated_at_of(&client, &workload_id, case).await;

    for round in 1..=2 {
        let repeat = format!("{case}/read-{round}");
        assert_eq!(
            answer_jira(&client, &workload_id, lens, question_id, &repeat).await,
            reported,
            "case {repeat:?}: repeated reads of a synced question must agree"
        );
        for (route, member_name) in [
            ("answers", "AnswerSummaries"),
            ("improvements", "ImprovementSummaries"),
        ] {
            let listed = jira_by_question(
                &client,
                &workload_id,
                lens,
                route,
                member_name,
                &repeat,
            )
            .await;
            let in_listing = listed.get(question_id).unwrap_or_else(|| {
                panic!("case {repeat:?}: {member_name} must report {question_id}")
            });
            assert_eq!(
                last_synced(in_listing, &repeat),
                synced_at,
                "case {repeat:?}: {member_name} must report the instant the sync recorded"
            );
        }
        assert_eq!(
            updated_at_of(&client, &workload_id, &repeat).await,
            after_sync,
            "case {repeat:?}: a read writes nothing, so UpdatedAt must not move"
        );
    }
}

/// One `ClientRequestToken` syncs ONCE: a retry carrying it with the same parameters
/// replays the original empty success and leaves the instant it recorded where it is, the
/// same token with different parameters is the modelled 409, and a fresh token syncs again.
#[tokio::test]
async fn one_token_syncs_once() {
    let client = Client::new();
    let case = "idempotent sync";
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let workload_id = workload_reviewing(&client, "idempotent", &[lens]).await;
    let other_id = workload_reviewing(&client, "idempotent-other", &[lens]).await;
    let question_id = state::lens_questions(lens)
        .first()
        .expect("the framework lens must ask a question")
        .question_id;

    sync(&client, &workload_id, "tok-once", case).await;
    // Aged so a second sync's instant is observably later; the sync and the reads below
    // still cross the HTTP boundary.
    age_syncs(&client, &workload_id, Duration::from_secs(5));
    let first = last_synced(
        &answer_jira(&client, &workload_id, lens, question_id, case).await,
        case,
    );
    let after_first = updated_at_of(&client, &workload_id, case).await;

    // The replay: same token, same parameters. The original result was an empty body, so
    // that is what comes back, and nothing is synced a second time.
    sync(&client, &workload_id, "tok-once", case).await;
    assert_eq!(
        last_synced(
            &answer_jira(&client, &workload_id, lens, question_id, case).await,
            case
        ),
        first,
        "case {case:?}: a replay syncs nothing, so the recorded instant must not move"
    );
    assert_eq!(
        updated_at_of(&client, &workload_id, case).await,
        after_first,
        "case {case:?}: a replay writes nothing, so UpdatedAt must not move"
    );

    // The same token with DIFFERENT parameters is not the same request.
    let conflicting = update_integration(
        &client,
        &other_id,
        json!({ "ClientRequestToken": "tok-once", "IntegratingService": JIRA }),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case {case:?}: expected HTTP 409, got {conflicting:?}"
    );
    assert_eq!(
        conflicting.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {conflicting:?}"
    );
    assert_eq!(
        member(&conflicting, "ResourceType"),
        state::WORKLOAD_RESOURCE_TYPE,
        "case {case:?}: the conflict names the workload it already synced, got {conflicting:?}"
    );
    assert_eq!(
        member(&conflicting, "ResourceId"),
        workload_id,
        "case {case:?}: ResourceId must be that workload's id, got {conflicting:?}"
    );
    assert_eq!(
        member(&conflicting, "Message"),
        "[Conflict] A request with ClientRequestToken tok-once was already submitted with \
         different parameters.",
        "case {case:?}: unexpected Message, got {conflicting:?}"
    );
    assert_eq!(
        synced_questions(&client, &other_id, lens, case).await,
        BTreeSet::new(),
        "case {case:?}: the refused token syncs nothing"
    );

    // A FRESH token is a new request, so it syncs again and records a later instant.
    sync(&client, &workload_id, "tok-again", case).await;
    let second = last_synced(
        &answer_jira(&client, &workload_id, lens, question_id, case).await,
        case,
    );
    assert!(
        second > first,
        "case {case:?}: a new token must record a later instant, got {second} after {first}"
    );
    assert!(
        updated_at_of(&client, &workload_id, case).await >= after_first,
        "case {case:?}: a sync that covers questions updates the workload"
    );
}

/// A sync naming a workload the store does not hold is the modelled 404 over the workload,
/// worded as every operation that resolves a `WorkloadId` words it, and syncs nothing.
#[tokio::test]
async fn a_sync_of_a_workload_that_is_not_stored_is_refused() {
    let client = Client::new();
    let case = "absent-workload";
    let lens = state::FRAMEWORK_LENS_ALIAS;
    let stored_id = workload_reviewing(&client, "absent-neighbour", &[lens]).await;

    let answer = update_integration(
        &client,
        ABSENT_HEX_ID,
        json!({ "ClientRequestToken": "tok-absent", "IntegratingService": JIRA }),
    )
    .await;
    assert_not_found(
        case,
        &answer,
        state::WORKLOAD_RESOURCE_TYPE,
        ABSENT_HEX_ID,
        &format!("workload with ID {ABSENT_HEX_ID}"),
    );
    assert_eq!(
        synced_questions(&client, &stored_id, lens, case).await,
        BTreeSet::new(),
        "case {case:?}: a refused sync must leave every other workload unsynced"
    );
}
