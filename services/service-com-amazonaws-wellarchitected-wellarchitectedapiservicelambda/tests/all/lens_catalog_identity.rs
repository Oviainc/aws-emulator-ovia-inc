// SPDX-License-Identifier: CC0-1.0

//! The identity a lens is reported by, over the wire.
//!
//! Three rules, each asserted over a SET of inputs rather than over the one lens and the
//! one question an issue cited:
//!
//! 1. A lens Amazon Web Services publishes is partition-global and owned by no customer
//!    account, so every surface that reports its identity reports the same
//!    `arn:aws:wellarchitected::aws:lens/<alias>` whatever Region the request was signed
//!    for, and `LensSummary$Owner` reports the publisher. A Region-qualified spelling of
//!    that ARN is still accepted on input and names the same lens.
//! 2. Every surface that echoes a lens's identity echoes the catalog's name and version
//!    for THAT lens, and a lens review's risk roll-up counts that lens's own questions.
//! 3. The framework lens addresses its content by the identifiers it publishes: a
//!    `QuestionId` of its own and, per best practice, a `ChoiceId` and the prose the lens
//!    publishes for it — which is what an answer is read and written under.
//!
//! Every request goes through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary validation and the handler all run as they do in
//! production.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, official_lens_arn, regional_lens_arn, Answer,
    Client, SIGNED_REGION,
};

/// Regions a case can sign its request in: an official lens's identity may not depend on
/// any of them, and one of them is not the Region the harness signs for by default.
const SIGNING_REGIONS: [&str; 3] = [SIGNED_REGION, "us-east-1", "eu-west-1"];

/// Published identifiers of the AWS Well-Architected Framework lens, spelled out here
/// rather than read back out of the code under test: the `PillarId`, the `QuestionId` the
/// lens asks the question under, and the `ChoiceId` of one best practice of it.
///
/// SOURCE: the framework lens's own content, as the Well-Architected Framework publishes
/// its pillars, questions and best practices
/// (<https://docs.aws.amazon.com/wellarchitected/latest/framework/welcome.html>) — the
/// same source `state::FRAMEWORK_LENS_QUESTIONS` names. The rows span the pillars: COST 1
/// is the question an issue cited, and the others are questions it did not.
const PUBLISHED_QUESTIONS: &[(&str, &str, &str)] = &[
    (
        "costOptimization",
        "cloud-financial-management",
        "cost_cloud_financial_management_function",
    ),
    (
        "operationalExcellence",
        "organization-priorities",
        "ops_priorities_ext_cust_needs",
    ),
    ("security", "identities", "sec_identities_strong_sign_in"),
    (
        "reliability",
        "back-up-data",
        "rel_backing_up_data_secured_backups_data",
    ),
    (
        "performance",
        "data-management",
        "perf_data_purpose_built",
    ),
    (
        "sustainability",
        "region-selection",
        "sus_region_selection_business_sustainability",
    ),
];

/// `ChoiceId`'s modelled `@length(min: 1, max: 64)`
/// (`smithy_query(query="shape", shape="ChoiceId")`).
const CHOICE_ID_LENGTH: (usize, usize) = (1, 64);

/// `Choice$Description`'s modelled `@length(min: 1, max: 1024)`
/// (`smithy_query(query="shape", shape="Choice")`).
const CHOICE_DESCRIPTION_LENGTH: (usize, usize) = (1, 1024);

fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A workload reviewing `lenses`, created in `region`.
async fn workload(client: &Client, region: &str, case: &str, lenses: &[&str]) -> String {
    let mut body = minimal_create(&format!("wl-{case}"), &format!("tok-{case}"));
    body["Lenses"] = json!(lenses);
    let answer = client
        .send_in(region, "POST", "/workloads", Some(body))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkload 200, got {answer:?}"
    );
    created_id(&answer, case)
}

async fn get_lens_review(client: &Client, region: &str, workload_id: &str, lens: &str) -> Answer {
    client
        .send_in(
            region,
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

async fn get_answer(
    client: &Client,
    region: &str,
    workload_id: &str,
    lens: &str,
    question_id: &str,
) -> Answer {
    client
        .send_in(
            region,
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// Rule 1. Whatever Region a request is signed for, and however it spells the lens, every
/// surface that reports an AWS-official lens's ARN reports the one partition-global ARN —
/// no Region segment, the publisher in the account position — and a lens summary reports
/// that publisher as its `Owner`.
#[tokio::test]
async fn an_official_lens_is_reported_by_one_partition_global_arn() {
    for region in SIGNING_REGIONS {
        let client = Client::new();
        // The lens set spans the catalog rather than naming one lens, and the rows
        // alternate the spelling the REQUEST uses: the bare alias, the reported ARN and a
        // Region-qualified ARN of a DIFFERENT Region all name the same lens.
        let spellings: Vec<(&str, String)> = vec![
            ("wellarchitected", "wellarchitected".to_string()),
            (
                "serverless",
                official_lens_arn(region, "serverless"),
            ),
            ("iot", regional_lens_arn("ap-southeast-2", "iot")),
            ("genai", regional_lens_arn(region, "genai")),
        ];
        let named: Vec<&str> = spellings
            .iter()
            .map(|(_, named)| named.as_str())
            .collect();
        let workload_id = workload(&client, region, &format!("arn-{region}"), &named).await;

        for (alias, named) in &spellings {
            let case = format!("{region}/{alias}");
            let expected = official_lens_arn(region, alias);
            assert_eq!(
                expected,
                format!("arn:aws:wellarchitected::aws:lens/{alias}"),
                "case {case:?}: the expectation itself must be the partition-global form"
            );

            let review = get_lens_review(&client, region, &workload_id, named).await;
            assert_eq!(
                review.status, 200,
                "case {case:?}: expected GetLensReview 200, got {review:?}"
            );
            let lens_review = review
                .body
                .get("LensReview")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                text(&lens_review, "LensArn"),
                expected,
                "case {case:?}: a lens review must report the partition-global lens ARN, got \
                 {lens_review:?}"
            );

            let lens = client
                .send_in(
                    region,
                    "GET",
                    &format!("/lenses/{}", encode_path_segment(named)),
                    None,
                )
                .await;
            let read = lens.body.get("Lens").cloned().unwrap_or(Value::Null);
            assert_eq!(
                text(&read, "LensArn"),
                expected,
                "case {case:?}: GetLens must report the partition-global lens ARN, got {lens:?}"
            );
            assert_eq!(
                text(&read, "Owner"),
                state::AWS_OFFICIAL_LENS_OWNER,
                "case {case:?}: a lens AWS publishes is owned by AWS, got {lens:?}"
            );

            let answers = client
                .send_in(
                    region,
                    "GET",
                    &format!(
                        "/workloads/{workload_id}/lensReviews/{}/answers",
                        encode_path_segment(named)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                text(&answers.body, "LensArn"),
                expected,
                "case {case:?}: ListAnswers must report the partition-global lens ARN, got \
                 {answers:?}"
            );
        }

        // ListLenses reports the same identity for every lens it publishes, in every
        // Region, and names the publisher as the owner.
        let listed = client
            .send_in(region, "GET", "/lenses?LensType=AWS_OFFICIAL&MaxResults=50", None)
            .await;
        let summaries = listed
            .body
            .get("LensSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            state::OFFICIAL_LENS_CATALOG.len(),
            "case {region:?}: every lens the service publishes must be listed, got {listed:?}"
        );
        for summary in &summaries {
            let alias = text(summary, "LensAlias");
            assert_eq!(
                text(summary, "LensArn"),
                official_lens_arn(region, &alias),
                "case {region:?}/{alias:?}: expected the partition-global lens ARN, got \
                 {summary:?}"
            );
            assert_eq!(
                text(summary, "Owner"),
                state::AWS_OFFICIAL_LENS_OWNER,
                "case {region:?}/{alias:?}: a lens AWS publishes is owned by AWS, got {summary:?}"
            );
        }

        // The lens library leads with the Well-Architected Framework lens — the lens every
        // workload is reviewed against by default — and the rest follow in alias order.
        let reported: Vec<String> = summaries.iter().map(|s| text(s, "LensAlias")).collect();
        let mut expected: Vec<String> = reported
            .iter()
            .filter(|alias| *alias != state::FRAMEWORK_LENS_ALIAS)
            .cloned()
            .collect();
        expected.sort();
        expected.insert(0, state::FRAMEWORK_LENS_ALIAS.to_string());
        assert_eq!(
            reported, expected,
            "case {region:?}: the framework lens leads the library, got {reported:?}"
        );
    }
}

/// Rule 2. A workload's lens reviews are reported in the order the workload applies its
/// lenses, each carrying THAT lens's catalog name and version, and each roll-up counting
/// that lens's own questions.
#[tokio::test]
async fn each_lens_review_reports_its_own_lenss_identity_and_roll_up() {
    // Rows differ along WHICH lenses the workload applies and in WHAT ORDER, so a summary
    // cannot be matched to the right lens by luck of a sorted list.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("framework-then-serverless", vec!["wellarchitected", "serverless"]),
        ("serverless-then-framework", vec!["serverless", "wellarchitected"]),
        ("three-lenses", vec!["iot", "wellarchitected", "genai"]),
    ];

    for (case, lenses) in rows {
        let client = Client::new();
        let workload_id = workload(&client, SIGNED_REGION, case, &lenses).await;

        let listed = client
            .send_in(
                SIGNED_REGION,
                "GET",
                &format!("/workloads/{workload_id}/lensReviews"),
                None,
            )
            .await;
        let summaries = listed
            .body
            .get("LensReviewSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.iter().map(|s| text(s, "LensAlias")).collect::<Vec<String>>(),
            lenses.iter().map(ToString::to_string).collect::<Vec<String>>(),
            "case {case:?}: the reviews must come in the order the workload applies its \
             lenses, got {listed:?}"
        );

        for (summary, alias) in summaries.iter().zip(&lenses) {
            let official = state::official_lens(alias)
                .unwrap_or_else(|| panic!("case {case:?}: {alias:?} must be a published lens"));
            assert_eq!(
                (
                    text(summary, "LensName"),
                    text(summary, "LensVersion"),
                    text(summary, "LensArn"),
                ),
                (
                    official.name.to_string(),
                    official.version.to_string(),
                    official_lens_arn(SIGNED_REGION, alias),
                ),
                "case {case:?}/{alias:?}: the summary must carry that lens's identity, got \
                 {summary:?}"
            );
            // The roll-up counts THAT lens's questions: nothing is answered yet, so every
            // one of them is UNANSWERED.
            assert_eq!(
                summary
                    .get("RiskCounts")
                    .and_then(|counts| counts.get("UNANSWERED"))
                    .and_then(Value::as_i64),
                i64::try_from(official.questions.len()).ok(),
                "case {case:?}/{alias:?}: the roll-up must count that lens's own questions, \
                 got {summary:?}"
            );

            // And the single-review read agrees with the listing about all of it.
            let review = get_lens_review(&client, SIGNED_REGION, &workload_id, alias).await;
            let lens_review = review
                .body
                .get("LensReview")
                .cloned()
                .unwrap_or(Value::Null);
            for member in ["LensName", "LensVersion", "LensArn", "LensStatus"] {
                assert_eq!(
                    text(&lens_review, member),
                    text(summary, member),
                    "case {case:?}/{alias:?}: GetLensReview and ListLensReviews must agree \
                     about {member}, got {lens_review:?}"
                );
            }
        }
    }
}

/// Rule 2, on the write path: an update of a lens review echoes the reviewed lens's
/// identity and reports the Jira question selection the review carries afterwards — an
/// empty `SelectedPillars` list when the review selects no pillar.
#[tokio::test]
async fn updating_a_lens_review_echoes_the_lens_identity_and_its_jira_selection() {
    // Rows differ along what the update WRITES: notes only, a selection, and nothing.
    let rows: Vec<(&str, Value, usize)> = vec![
        ("notes-only", json!({ "LensNotes": "reviewed" }), 0),
        ("nothing", json!({}), 0),
        (
            "one-pillar-selected",
            json!({
                "JiraConfiguration": {
                    "SelectedPillars": [{
                        "PillarId": "costOptimization",
                        "SelectedQuestionIds": ["cloud-financial-management"],
                    }],
                },
            }),
            1,
        ),
    ];

    for (case, body, selected_pillars) in rows {
        let client = Client::new();
        let workload_id = workload(&client, SIGNED_REGION, case, &["wellarchitected"]).await;
        let answer = client
            .send_in(
                SIGNED_REGION,
                "PATCH",
                &format!(
                    "/workloads/{workload_id}/lensReviews/{}",
                    encode_path_segment(state::FRAMEWORK_LENS_ALIAS)
                ),
                Some(body),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateLensReview 200, got {answer:?}"
        );
        let lens_review = answer
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            (
                text(&lens_review, "LensName"),
                text(&lens_review, "LensVersion"),
                text(&lens_review, "LensArn"),
            ),
            (
                state::FRAMEWORK_LENS_NAME.to_string(),
                state::FRAMEWORK_LENS_VERSION.to_string(),
                official_lens_arn(SIGNED_REGION, state::FRAMEWORK_LENS_ALIAS),
            ),
            "case {case:?}: the updated review must echo the lens's identity, got {answer:?}"
        );
        let pillars = lens_review
            .get("JiraConfiguration")
            .and_then(|configuration| configuration.get("SelectedPillars"))
            .and_then(Value::as_array)
            .cloned();
        assert_eq!(
            pillars.as_ref().map(Vec::len),
            Some(selected_pillars),
            "case {case:?}: the update must report the selection it leaves behind, got {answer:?}"
        );
    }
}

/// Rule 3. The framework lens asks its questions under the `QuestionId`s it publishes, and
/// each best practice carries the `ChoiceId` and the prose the lens publishes for it: an
/// answer is READ and WRITTEN under those identifiers.
#[tokio::test]
async fn the_framework_lens_addresses_its_content_by_its_published_identifiers() {
    let client = Client::new();
    let workload_id = workload(&client, SIGNED_REGION, "published-ids", &["wellarchitected"]).await;

    // Every question the published table names is a question of the lens, under its own
    // pillar, and offers the published choice.
    for (pillar_id, question_id, choice_id) in PUBLISHED_QUESTIONS {
        let case = format!("{pillar_id}/{question_id}");
        let read = get_answer(
            &client,
            SIGNED_REGION,
            &workload_id,
            state::FRAMEWORK_LENS_ALIAS,
            question_id,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a question the lens asks must be readable by its published id, \
             got {read:?}"
        );
        assert_eq!(
            (
                text(&read.body, "WorkloadId"),
                text(&read.body, "LensAlias"),
                text(&read.body, "LensArn"),
            ),
            (
                workload_id.clone(),
                state::FRAMEWORK_LENS_ALIAS.to_string(),
                official_lens_arn(SIGNED_REGION, state::FRAMEWORK_LENS_ALIAS),
            ),
            "case {case:?}: the answer must name the workload and the lens it belongs to, got \
             {read:?}"
        );
        let answer = read.body.get("Answer").cloned().unwrap_or(Value::Null);
        assert_eq!(
            (text(&answer, "PillarId"), text(&answer, "QuestionId")),
            ((*pillar_id).to_string(), (*question_id).to_string()),
            "case {case:?}: the answer must name the question and its pillar, got {read:?}"
        );
        assert_eq!(
            text(&answer, "Risk"),
            "UNANSWERED",
            "case {case:?}: nothing is answered yet, got {read:?}"
        );

        let choices = answer
            .get("Choices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let ids: Vec<String> = choices.iter().map(|choice| text(choice, "ChoiceId")).collect();
        assert!(
            ids.iter().any(|id| id == choice_id),
            "case {case:?}: expected the published choice {choice_id:?} among {ids:?}"
        );
        for choice in &choices {
            let id = text(choice, "ChoiceId");
            assert!(
                (CHOICE_ID_LENGTH.0..=CHOICE_ID_LENGTH.1).contains(&id.len()),
                "case {case:?}: ChoiceId is @length{CHOICE_ID_LENGTH:?}, got {id:?}"
            );
            let description = text(choice, "Description");
            assert!(
                (CHOICE_DESCRIPTION_LENGTH.0..=CHOICE_DESCRIPTION_LENGTH.1)
                    .contains(&description.len()),
                "case {case:?}/{id:?}: the lens publishes prose for every best practice, \
                 @length{CHOICE_DESCRIPTION_LENGTH:?}, got {choice:?}"
            );
            assert_ne!(
                description,
                text(choice, "Title"),
                "case {case:?}/{id:?}: the prose describes the best practice rather than \
                 repeating its title, got {choice:?}"
            );
        }

        // The id a client discovers is the id it may answer with, and the write is read
        // back under the same ids.
        let notes = format!("answered {question_id}");
        let written = client
            .send_in(
                SIGNED_REGION,
                "PATCH",
                &format!(
                    "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                    encode_path_segment(state::FRAMEWORK_LENS_ALIAS),
                    encode_path_segment(question_id)
                ),
                Some(json!({ "SelectedChoices": [choice_id], "Notes": notes })),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: an answer must be writable under the published ids, got {written:?}"
        );
        let stored = written.body.get("Answer").cloned().unwrap_or(Value::Null);
        assert_eq!(
            (
                text(&stored, "QuestionId"),
                text(&stored, "Notes"),
                stored
                    .get("SelectedChoices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            ),
            (
                (*question_id).to_string(),
                notes.clone(),
                vec![json!(choice_id)],
            ),
            "case {case:?}: the write must record the choice the client named, got {written:?}"
        );
        assert!(
            ["HIGH", "MEDIUM", "NONE"].contains(&text(&stored, "Risk").as_str()),
            "case {case:?}: an answered question carries the risk its selection leaves, \
             rather than staying UNANSWERED, got {written:?}"
        );
    }

    // And the lens's whole question set is addressed by published ids: no `QuestionId` or
    // `ChoiceId` repeats, and none falls back to a position-derived spelling.
    let mut question_ids: BTreeSet<&str> = BTreeSet::new();
    let mut choice_ids: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for question in state::FRAMEWORK_LENS_QUESTIONS {
        assert!(
            question_ids.insert(question.question_id),
            "a QuestionId names one question of the lens, got {:?} twice",
            question.question_id
        );
        let per_question = choice_ids.entry(question.question_id).or_default();
        for (position, practice) in question.best_practices.iter().enumerate() {
            let id = question.choice_id(position);
            assert_ne!(
                id,
                format!("{}-bp-{:02}", question.question_id, position + 1),
                "case {:?}: the framework lens publishes its own ChoiceId per best practice",
                question.question_id
            );
            assert!(
                per_question.insert(id.clone()),
                "case {:?}: a ChoiceId names one best practice, got {id:?} twice",
                question.question_id
            );
            assert!(
                practice.description.is_some_and(|prose| !prose.is_empty()),
                "case {:?}/{id:?}: the lens publishes prose for every best practice",
                question.question_id
            );
        }
    }
}
