// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the AWS-OFFICIAL lens catalog: the lenses `ListLenses`
//! publishes, and the content a review of each of them reports.
//!
//! The rule these pin, over the WHOLE catalog rather than over one lens: the service
//! publishes a catalog of AWS-official lenses, each with its own `LensAlias`, `LensName`,
//! `LensVersion`, description, pillar set and question set, so every one of them is
//! listed by `ListLenses` (and selectable by `LensType`/`LensName`), is reviewable on a
//! workload — `GetLensReview` reports that lens's name, version, status, one pillar
//! summary per pillar it asks questions in and a risk roll-up over its questions — and has
//! its questions listed by `ListAnswers`. The rows of every table below are the catalog's
//! own lenses, which differ along the member the rule generalises over: the alias, the
//! name, how many pillars the lens is organized into and how many questions it asks.
//!
//! Each assertion that could be satisfied by a single hardcoded answer is a CROSS-CHECK
//! between independent responses: the question count a review's roll-up reports, the
//! number of pillar summaries it carries, the sum of the pillars' own roll-ups and the
//! number of answers `ListAnswers` serves all have to agree for the same lens.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::workload_harness::{
    answered, answered_over, created_id, encode_path_segment, minimal_create, official_lens_arn,
    Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The AWS-official lenses this emulator publishes CONTENT for: the `LensAlias` AWS names
/// each by, the full `LensName` AWS publishes for it, and how many questions this
/// emulator's content for that lens asks.
///
/// Spelled out here rather than read back out of the code under test — the question count
/// included, so a row whose question set changed fails here rather than silently agreeing
/// with itself. Source of the aliases and names: the `LensAlias` doc, which names
/// `serverless` ("the lens alias, such as `serverless`") beside the framework's own alias
/// (`smithy_query(query="doc", shape="LensAlias")`), and the Well-Architected lens catalog
/// AWS publishes for the rest
/// (<https://docs.aws.amazon.com/wellarchitected/latest/userguide/lenses.html>), whose
/// aliases this crate also carries as `state::AWS_OFFICIAL_LENS_ALIASES`. The
/// `healthcareindustry` and `games` rows are entries the issues that asked for this
/// catalog did not cite; the framework row's 57 is the framework's own published numbering
/// (OPS 11, SEC 11, REL 13, PERF 5, COST 11, SUS 6).
const PUBLISHED_OFFICIAL_LENSES: &[(&str, &str, usize)] = &[
    ("wellarchitected", "AWS Well-Architected Framework", 57),
    ("connectedmobility", "Connected Mobility Lens", 8),
    ("dataanalytics", "Data Analytics Lens", 7),
    ("devops", "DevOps Lens", 8),
    ("financialservices", "Financial Services Industry Lens", 7),
    ("games", "Games Industry Lens", 8),
    ("genai", "Generative AI Lens", 8),
    ("healthcareindustry", "Healthcare Industry Lens", 7),
    ("iot", "IoT Lens", 8),
    ("machinelearning", "Machine Learning Lens", 7),
    ("migration", "Migration Lens", 6),
    ("sap", "SAP Lens", 7),
    ("serverless", "Serverless Lens", 9),
    ("softwareasaservice", "SaaS Lens", 8),
];

/// A lens alias AWS publishes that this emulator carries no CONTENT for: the content
/// catalog is the subset of the published aliases this emulator holds questions for, and a
/// lens that has not been added to it is not reported as one this service publishes. It is
/// still a published alias, so it can be APPLIED to a workload
/// (`state::is_published_official_lens`), which is what makes it the negative row of the
/// tables below.
const UNCARRIED_OFFICIAL_ALIAS: &str = "mergersandacquisitions";

/// The `PillarId`/`PillarName` pairs the Well-Architected Framework publishes, in the
/// order it presents them — every AWS-official lens is organized into these pillars.
/// `PillarId` is "The ID used to identify a pillar, for example, `security`"
/// (`smithy_query(query="shape", shape="PillarId")`); the names are the framework's own.
const PUBLISHED_PILLARS: &[(&str, &str)] = &[
    ("operationalExcellence", "Operational Excellence"),
    ("security", "Security"),
    ("reliability", "Reliability"),
    ("performance", "Performance Efficiency"),
    ("costOptimization", "Cost Optimization"),
    ("sustainability", "Sustainability"),
];

/// The Region the shared harness signs its requests for, which is the Region the ARNs in
/// a response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// `LensDescription`'s modelled `@length(min: 1, max: 1024)`
/// (`smithy_query(query="shape", shape="LensSummary")`).
const DESCRIPTION_LENGTH: (usize, usize) = (1, 1024);

/// `LensVersion`'s modelled `@length(min: 1, max: 32)`
/// (`smithy_query(query="shape", shape="LensReview")`).
const VERSION_LENGTH: (usize, usize) = (1, 32);

/// `QuestionId`'s modelled `@length(min: 1, max: 128)`
/// (`smithy_query(query="shape", shape="AnswerSummary")`).
const QUESTION_ID_LENGTH: (usize, usize) = (1, 128);

/// The `Risk` bucket a question with no stored answer counts under
/// (`smithy_query(query="shape", shape="Risk")`).
const UNANSWERED: &str = "UNANSWERED";

/// Which of a question's best practices an answer records as in place, out of the
/// question's OWN choice set: the plans span the risk an applicable answer is rated at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    /// None of them, with every choice left unselected: the answer says nothing about any
    /// best practice, so the question is still unanswered.
    Nothing,
    /// None of them in place, with the last recorded `NOT_APPLICABLE`: the answer speaks
    /// about that choice, so the question is rated with no best practice in place.
    NothingAndTheLastDoesNotApply,
    /// All but the last, so one best practice is still unselected.
    AllButLast,
    /// Every one of them, so nothing is left unselected.
    Every,
}

fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `GET /lenses`, the wire form of `ListLenses`: every input member is bound to the query
/// string, so `query` is passed as sent.
async fn list_lenses(client: &Client, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/lenses".to_string()
    } else {
        format!("/lenses?{query}")
    };
    client.send("GET", &uri, None).await
}

fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLenses 200, got {answer:?}"
    );
    answer
        .body
        .get("LensSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every official summary of the catalog, keyed by `LensAlias`, following `NextToken` so
/// the whole catalog is collected however it is paged.
async fn official_catalog(client: &Client, case: &str) -> BTreeMap<String, Value> {
    let mut collected: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = "LensType=AWS_OFFICIAL&LensStatus=ALL".to_string();
        if let Some(token) = &token {
            query.push_str(&format!("&NextToken={token}"));
        }
        let page = list_lenses(client, &query).await;
        for summary in summaries(&page, case) {
            let alias = member(&summary, "LensAlias");
            assert!(
                collected.insert(alias.clone(), summary).is_none(),
                "case {case:?}: lens {alias:?} must be reported once"
            );
        }
        token = page
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    collected
}

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`, the wire form of
/// `GetLensReview`.
async fn get_lens_review(client: &Client, workload_id: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers`, following `NextToken`
/// to the end: every `AnswerSummary` of one lens review, in the order it was served.
async fn all_answers(client: &Client, workload_id: &str, lens: &str, case: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?NextToken={token}"),
            None => String::new(),
        };
        let answer = client
            .send(
                "GET",
                &format!(
                    "/workloads/{workload_id}/lensReviews/{}/answers{query}",
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
                .cloned()
                .unwrap_or_default(),
        );
        token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            collected.len() < 1_000,
            "case {case:?}: paging must terminate, got {} summaries",
            collected.len()
        );
    }
    collected
}

/// A workload reviewing exactly one lens, by the name the caller gives that lens.
async fn workload_reviewing(client: &Client, case: &str, lens: &str) -> String {
    let mut request = minimal_create(&format!("catalog-{case}"), &format!("tok-catalog-{case}"));
    request["Lenses"] = json!([lens]);
    created_id(&client.create(request).await, case)
}

/// Rule: `ListLenses` reports one `LensSummary` per AWS-official lens the service
/// publishes, each described by its OWN catalog entry — its alias, its full name, its
/// version, its description and the account-less ARN that version is published under —
/// and nothing else is reported as official.
#[tokio::test]
async fn list_lenses_reports_every_aws_official_lens() {
    let client = Client::new();
    let catalog = official_catalog(&client, "catalog").await;

    let reported: BTreeSet<&str> = catalog.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = PUBLISHED_OFFICIAL_LENSES
        .iter()
        .map(|(alias, _, _)| *alias)
        .collect();
    assert_eq!(
        reported, expected,
        "case \"catalog\": ListLenses must publish every AWS-official lens, got {reported:?}"
    );

    for (alias, name, _) in PUBLISHED_OFFICIAL_LENSES {
        let summary = catalog
            .get(*alias)
            .unwrap_or_else(|| panic!("case {alias:?}: must be reported, got {catalog:?}"));
        assert_eq!(
            member(summary, "LensName"),
            *name,
            "case {alias:?}: expected the name AWS publishes, got {summary:?}"
        );
        assert_eq!(
            member(summary, "LensArn"),
            official_lens_arn(REQUEST_REGION, alias),
            "case {alias:?}: expected the account-less official ARN, got {summary:?}"
        );
        assert_eq!(
            member(summary, "LensType"),
            "AWS_OFFICIAL",
            "case {alias:?}: a lens AWS publishes is AWS_OFFICIAL, got {summary:?}"
        );
        assert_eq!(
            member(summary, "LensStatus"),
            "PUBLISHED",
            "case {alias:?}: AWS's own lenses are published content, got {summary:?}"
        );
        assert_eq!(
            member(summary, "Owner"),
            state::AWS_OFFICIAL_LENS_OWNER,
            "case {alias:?}: a lens AWS publishes is owned by AWS, got {summary:?}"
        );

        let version = member(summary, "LensVersion");
        assert!(
            (VERSION_LENGTH.0..=VERSION_LENGTH.1).contains(&version.len()),
            "case {alias:?}: LensVersion is @length{VERSION_LENGTH:?}, got {version:?}"
        );
        let description = member(summary, "Description");
        assert!(
            (DESCRIPTION_LENGTH.0..=DESCRIPTION_LENGTH.1).contains(&description.len()),
            "case {alias:?}: Description is @length{DESCRIPTION_LENGTH:?}, got {description:?}"
        );
        // The description must describe THIS lens, so it names the lens — the framework
        // lens's published description spells its name out in full ("Amazon Web Services"
        // rather than "AWS"), so the subject rather than the exact `LensName` is what is
        // looked for.
        let subject = name.strip_prefix("AWS ").unwrap_or(name).to_lowercase();
        assert!(
            description.to_lowercase().contains(&subject),
            "case {alias:?}: the description must describe THIS lens, got {description:?}"
        );

        // `CreatedAt`/`UpdatedAt` are the instants the version this lens reports was
        // published, so they name the same day the version does — one hand-written fact
        // cannot drift from the other.
        let created = summary.get("CreatedAt").and_then(Value::as_f64);
        let updated = summary.get("UpdatedAt").and_then(Value::as_f64);
        assert_eq!(
            created, updated,
            "case {alias:?}: a lens nobody changed was created and updated at one instant, \
             got {summary:?}"
        );
        let rendered = created
            .map(::aws_smithy_types::DateTime::from_secs_f64)
            .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
            .unwrap_or_default();
        assert_eq!(
            rendered,
            format!("{version}T00:00:00Z"),
            "case {alias:?}: CreatedAt must be midnight of the day {version:?} names, got \
             {rendered:?}"
        );
    }

    // The catalog is what the service publishes, so a lens nobody added to it is not
    // reported — neither listed nor selectable by type.
    assert!(
        !catalog.contains_key(UNCARRIED_OFFICIAL_ALIAS),
        "case \"uncarried\": {UNCARRIED_OFFICIAL_ALIAS:?} is carried by no catalog entry, got \
         {catalog:?}"
    );

    // And a listing is a REPORT: a second one answers exactly the same thing.
    let again = official_catalog(&client, "re-read").await;
    assert_eq!(
        again, catalog,
        "case \"re-read\": listing must not change what a later caller sees"
    );
}

/// Rule: `LensName` is "The full name of the lens", so it selects the ONE official lens
/// carrying that name — every row of the catalog, and nothing for a name no lens carries.
#[tokio::test]
async fn the_name_filter_selects_one_official_lens() {
    let client = Client::new();

    for (alias, name, _) in PUBLISHED_OFFICIAL_LENSES {
        let query = format!("LensStatus=ALL&LensName={}", encode_path_segment(name));
        let listed = list_lenses(&client, &query).await;
        let selected: Vec<String> = summaries(&listed, alias)
            .iter()
            .map(|summary| member(summary, "LensAlias"))
            .collect();
        assert_eq!(
            selected,
            vec![(*alias).to_string()],
            "case {alias:?}: {name:?} must select exactly that lens, got {listed:?}"
        );

        // A PREFIX of the name is not the full name, so it selects nothing.
        let prefix: String = name.chars().take(name.len() / 2).collect();
        let partial = list_lenses(
            &client,
            &format!("LensStatus=ALL&LensName={}", encode_path_segment(&prefix)),
        )
        .await;
        assert!(
            summaries(&partial, alias).is_empty(),
            "case {alias:?}: a prefix {prefix:?} is not a full lens name, got {partial:?}"
        );
    }
}

/// Rule: every AWS-official lens the catalog publishes is reviewable on a workload. The
/// review reports THAT lens's name, version and status, one pillar summary per pillar it
/// asks questions in, and a risk roll-up over its own questions; `ListAnswers` lists
/// exactly those questions. Every count is cross-checked against the others.
#[tokio::test]
async fn every_official_lens_is_reviewable_on_a_workload() {
    let pillar_names: BTreeMap<&str, &str> = PUBLISHED_PILLARS.iter().copied().collect();

    for (index, (alias, name, questions)) in PUBLISHED_OFFICIAL_LENSES.iter().enumerate() {
        let client = Client::new();
        // A lens is named either by its alias or by its ARN (`LensAlias` doc), so the rows
        // alternate between the two spellings: both must resolve to the same lens.
        let named = if index % 2 == 0 {
            (*alias).to_string()
        } else {
            official_lens_arn(REQUEST_REGION, alias)
        };
        let workload_id = workload_reviewing(&client, alias, &named).await;

        let review = get_lens_review(&client, &workload_id, &named).await;
        assert_eq!(
            review.status, 200,
            "case {alias:?}: expected GetLensReview 200, got {review:?}"
        );
        let lens_review = review
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&lens_review, "LensName"),
            *name,
            "case {alias:?}: the review must name the lens, got {lens_review:?}"
        );
        assert_eq!(
            member(&lens_review, "LensStatus"),
            "CURRENT",
            "case {alias:?}: a review of the published content is CURRENT, got {lens_review:?}"
        );
        assert_eq!(
            member(&lens_review, "LensArn"),
            official_lens_arn(REQUEST_REGION, alias),
            "case {alias:?}: expected the account-less official ARN, got {lens_review:?}"
        );
        let version = member(&lens_review, "LensVersion");
        assert!(
            (VERSION_LENGTH.0..=VERSION_LENGTH.1).contains(&version.len()),
            "case {alias:?}: the review must report the lens version, got {lens_review:?}"
        );

        // The pillars the lens is organized into, each with its published name and its own
        // roll-up over the questions THAT pillar asks.
        let pillars = lens_review
            .get("PillarReviewSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !pillars.is_empty(),
            "case {alias:?}: an official lens is organized into pillars, got {lens_review:?}"
        );
        let reported_ids: Vec<String> = pillars
            .iter()
            .map(|pillar| member(pillar, "PillarId"))
            .collect();
        let unique: BTreeSet<&String> = reported_ids.iter().collect();
        assert_eq!(
            unique.len(),
            reported_ids.len(),
            "case {alias:?}: a pillar is summarised once, got {reported_ids:?}"
        );
        let published_order: Vec<&str> = PUBLISHED_PILLARS.iter().map(|(id, _)| *id).collect();
        let expected_order: Vec<String> = published_order
            .iter()
            .filter(|id| reported_ids.iter().any(|reported| reported == **id))
            .map(|id| (*id).to_string())
            .collect();
        assert_eq!(
            reported_ids, expected_order,
            "case {alias:?}: pillars must be reported in the framework's pillar order"
        );

        let mut pillar_questions = 0i64;
        for pillar in &pillars {
            let pillar_id = member(pillar, "PillarId");
            assert_eq!(
                member(pillar, "PillarName"),
                *pillar_names.get(pillar_id.as_str()).unwrap_or(&""),
                "case {alias:?}/{pillar_id:?}: expected the published pillar name, got {pillar:?}"
            );
            let counts = pillar
                .get("RiskCounts")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let unanswered = counts.get(UNANSWERED).and_then(Value::as_i64).unwrap_or(-1);
            assert!(
                unanswered > 0,
                "case {alias:?}/{pillar_id:?}: a reported pillar asks questions, got {pillar:?}"
            );
            let answered_elsewhere: i64 = counts
                .iter()
                .filter(|(risk, _)| risk.as_str() != UNANSWERED)
                .filter_map(|(_, count)| count.as_i64())
                .sum();
            assert_eq!(
                answered_elsewhere, 0,
                "case {alias:?}/{pillar_id:?}: nothing is answered yet, got {pillar:?}"
            );
            pillar_questions += unanswered;
        }

        let lens_unanswered = lens_review
            .get("RiskCounts")
            .and_then(Value::as_object)
            .and_then(|counts| counts.get(UNANSWERED))
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        // The count this suite's table states for the lens, so a catalog entry whose
        // question set changed fails here rather than agreeing with itself.
        assert_eq!(
            lens_unanswered,
            i64::try_from(*questions).unwrap_or(-1),
            "case {alias:?}: the lens asks {questions} questions, the review counted \
             {lens_unanswered}"
        );
        assert_eq!(
            lens_unanswered, pillar_questions,
            "case {alias:?}: the lens roll-up must be the sum of its pillars', got {lens_review:?}"
        );

        // ListAnswers lists exactly the questions the roll-up counted, in the lens's own
        // order (pillar by pillar, in the framework's pillar order).
        let answers = all_answers(&client, &workload_id, &named, alias).await;
        assert_eq!(
            i64::try_from(answers.len()).unwrap_or(-1),
            lens_unanswered,
            "case {alias:?}: ListAnswers must list every question the roll-up counted, got {} \
             summaries",
            answers.len()
        );
        let mut seen_ids: BTreeSet<String> = BTreeSet::new();
        let mut answered_pillars: Vec<String> = Vec::new();
        for summary in &answers {
            let question_id = member(summary, "QuestionId");
            assert!(
                (QUESTION_ID_LENGTH.0..=QUESTION_ID_LENGTH.1).contains(&question_id.len()),
                "case {alias:?}: QuestionId is @length{QUESTION_ID_LENGTH:?}, got {question_id:?}"
            );
            assert!(
                seen_ids.insert(question_id.clone()),
                "case {alias:?}: QuestionId {question_id:?} must be unique within the lens"
            );
            assert!(
                !member(summary, "QuestionTitle").is_empty(),
                "case {alias:?}/{question_id:?}: a question carries its title, got {summary:?}"
            );
            assert_eq!(
                member(summary, "Risk"),
                UNANSWERED,
                "case {alias:?}/{question_id:?}: nothing is answered yet, got {summary:?}"
            );
            let choices = summary
                .get("Choices")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !choices.is_empty(),
                "case {alias:?}/{question_id:?}: a question asks about best practices, got \
                 {summary:?}"
            );
            // A choice is named by the id the LENS names it by: the framework lens
            // publishes its own `ChoiceId`s, and a lens whose content this emulator
            // authored names a choice by its position in the question.
            let mut choice_ids: BTreeSet<String> = BTreeSet::new();
            for (position, choice) in choices.iter().enumerate() {
                let choice_id = member(choice, "ChoiceId");
                assert!(
                    choice_ids.insert(choice_id.clone()),
                    "case {alias:?}/{question_id:?}: a ChoiceId names one choice of the \
                     question, got {choice_id:?} twice"
                );
                assert!(
                    (1..=64).contains(&choice_id.len()),
                    "case {alias:?}/{question_id:?}: ChoiceId is @length(1, 64), got \
                     {choice_id:?}"
                );
                if alias != &"wellarchitected" {
                    assert_eq!(
                        choice_id,
                        format!("{question_id}-bp-{:02}", position + 1),
                        "case {alias:?}/{question_id:?}: an authored lens names a choice for \
                         its question and position, got {choice:?}"
                    );
                }
                assert!(
                    !member(choice, "Title").is_empty(),
                    "case {alias:?}/{question_id:?}: a choice carries its best practice, got \
                     {choice:?}"
                );
            }
            let pillar_id = member(summary, "PillarId");
            assert!(
                pillar_names.contains_key(pillar_id.as_str()),
                "case {alias:?}/{question_id:?}: PillarId must be a published pillar, got \
                 {pillar_id:?}"
            );
            if answered_pillars.last() != Some(&pillar_id) {
                answered_pillars.push(pillar_id);
            }
        }
        assert_eq!(
            answered_pillars, reported_ids,
            "case {alias:?}: the questions must be grouped in the pillar order the review \
             reports"
        );
    }
}

/// Rule: a lens AWS publishes that this catalog carries no content for is not a lens this
/// service publishes — it is listed nowhere, and a review of it reports no lens content
/// rather than another lens's.
#[tokio::test]
async fn an_official_alias_the_catalog_does_not_carry_has_no_content() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "uncarried", UNCARRIED_OFFICIAL_ALIAS).await;

    let listed = list_lenses(
        &client,
        &format!(
            "LensStatus=ALL&LensName={}",
            encode_path_segment("Mergers and Acquisitions Lens")
        ),
    )
    .await;
    assert!(
        summaries(&listed, "uncarried").is_empty(),
        "case \"uncarried\": a lens this catalog does not carry is listed nowhere, got {listed:?}"
    );

    let review = get_lens_review(&client, &workload_id, UNCARRIED_OFFICIAL_ALIAS).await;
    assert_eq!(
        review.status, 200,
        "case \"uncarried\": the lens is associated, so the review exists, got {review:?}"
    );
    let lens_review = review
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or(Value::Null);
    for absent in ["LensName", "LensVersion", "LensStatus"] {
        assert!(
            lens_review.get(absent).is_none(),
            "case \"uncarried\": {absent} is lens content this catalog does not hold, got \
             {lens_review:?}"
        );
    }
    assert_eq!(
        lens_review.get("PillarReviewSummaries"),
        Some(&json!([])),
        "case \"uncarried\": a lens with no content here names no pillars, got {lens_review:?}"
    );
    let answers = all_answers(&client, &workload_id, UNCARRIED_OFFICIAL_ALIAS, "uncarried").await;
    assert!(
        answers.is_empty(),
        "case \"uncarried\": a lens with no content here asks nothing, got {answers:?}"
    );
}

/// Rule: a review of a specialised official lens is DERIVED from the answers stored
/// against that lens, exactly as the framework lens's is — recording an answer moves that
/// lens's roll-up and only that lens's. Rows differ along the answer recorded and the risk
/// it carries.
#[tokio::test]
async fn an_answer_recorded_against_a_specialised_lens_moves_its_roll_up() {
    // Two official lenses on ONE workload, so an answer to one must not move the other.
    let answered_lens = "serverless";
    let untouched_lens = "iot";

    let cases: [(&str, Selection, bool, &str); 5] = [
        // (case, which of the question's best practices are in place, whether the question
        // applies, the risk that rates)
        (
            "nothing-selected-at-all",
            Selection::Nothing,
            true,
            UNANSWERED,
        ),
        (
            "no-best-practice-selected",
            Selection::NothingAndTheLastDoesNotApply,
            true,
            "HIGH",
        ),
        (
            "some-choice-unselected",
            Selection::AllButLast,
            true,
            "MEDIUM",
        ),
        ("nothing-left-unselected", Selection::Every, true, "NONE"),
        ("not-applicable", Selection::Every, false, "NOT_APPLICABLE"),
    ];

    for (case, selection, is_applicable, risk) in cases {
        let client = Client::new();
        let mut request = minimal_create(&format!("rollup-{case}"), &format!("tok-rollup-{case}"));
        request["Lenses"] = json!([answered_lens, untouched_lens]);
        let workload_id = created_id(&client.create(request).await, case);

        // The question answered, and its choices, come from the lens itself: whatever the
        // catalog carries for it is what the answer is recorded against.
        let answers = all_answers(&client, &workload_id, answered_lens, case).await;
        let question = answers
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the lens must ask questions"));
        let question_id = member(&question, "QuestionId");
        let choices: Vec<String> = question
            .get("Choices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|choice| member(choice, "ChoiceId"))
            .collect();
        assert!(
            choices.len() > 1,
            "case {case:?}: the question under test must offer more than one best \
             practice, so leaving one out differs from leaving none out, got {choices:?}"
        );

        // The best practices in place, out of the question's OWN choice set: every other
        // choice of the question is left unselected, which is what rates the answer.
        let selected: Vec<&str> = match selection {
            Selection::Nothing | Selection::NothingAndTheLastDoesNotApply => Vec::new(),
            Selection::AllButLast => choices
                .iter()
                .take(choices.len().saturating_sub(1))
                .map(String::as_str)
                .collect(),
            Selection::Every => choices.iter().map(String::as_str).collect(),
        };
        let record = if selection == Selection::NothingAndTheLastDoesNotApply {
            // The last choice is recorded NOT_APPLICABLE, so the answer SPEAKS about the
            // question while leaving no best practice in place.
            let statuses: Vec<(&str, &str)> = choices
                .iter()
                .enumerate()
                .map(|(position, choice_id)| {
                    let status = if position + 1 == choices.len() {
                        "NOT_APPLICABLE"
                    } else {
                        "UNSELECTED"
                    };
                    (choice_id.as_str(), status)
                })
                .collect();
            answered(&statuses, is_applicable, None)
        } else {
            answered_over(&choices, &selected, is_applicable, None)
        };
        client.record_answer(&workload_id, answered_lens, &question_id, record);

        let before_total = i64::try_from(answers.len()).unwrap_or(-1);
        let review = get_lens_review(&client, &workload_id, answered_lens).await;
        let counts = review
            .body
            .get("LensReview")
            .and_then(|review| review.get("RiskCounts"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        // A question whose answer leaves it unanswered stays in that bucket, so the row
        // that records one expects the whole question set there.
        let expected_unanswered = if risk == UNANSWERED {
            before_total
        } else {
            before_total - 1
        };
        assert_eq!(
            counts.get(UNANSWERED).and_then(Value::as_i64),
            Some(expected_unanswered),
            "case {case:?}: expected {expected_unanswered} question(s) unanswered, got {review:?}"
        );
        assert_eq!(
            counts.get(risk).and_then(Value::as_i64),
            Some(if risk == UNANSWERED {
                before_total
            } else {
                1
            }),
            "case {case:?}: the stored answer must rate {risk}, got {review:?}"
        );

        // The OTHER lens's review is untouched: a roll-up counts its own lens's questions.
        let untouched = get_lens_review(&client, &workload_id, untouched_lens).await;
        let untouched_counts = untouched
            .body
            .get("LensReview")
            .and_then(|review| review.get("RiskCounts"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let untouched_answers =
            all_answers(&client, &workload_id, untouched_lens, case).await.len();
        assert_eq!(
            untouched_counts.get(UNANSWERED).and_then(Value::as_i64),
            i64::try_from(untouched_answers).ok(),
            "case {case:?}: the other lens's roll-up counts its own questions, got {untouched:?}"
        );
        assert_eq!(
            untouched_counts.get(risk).and_then(Value::as_i64),
            Some(if risk == UNANSWERED {
                i64::try_from(untouched_answers).unwrap_or(-1)
            } else {
                0
            }),
            "case {case:?}: the answer belongs to one lens only, got {untouched:?}"
        );

        // And the answer is reported back on the question it was recorded against.
        let reread = all_answers(&client, &workload_id, answered_lens, case).await;
        let summary = reread
            .iter()
            .find(|summary| member(summary, "QuestionId") == question_id)
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            member(&summary, "Risk"),
            risk,
            "case {case:?}: ListAnswers must report the stored answer's risk, got {summary:?}"
        );
    }
}

/// The catalog's own invariants, over every entry rather than over the one lens the issue
/// cited: each lens has a unique alias and name, its pillar counts add up to its question
/// set, its questions are grouped in the framework's pillar order under ids unique to the
/// lens, and its version names a real day.
#[tokio::test]
async fn the_catalog_is_internally_consistent() {
    let published_pillars: BTreeMap<&str, &str> = PUBLISHED_PILLARS.iter().copied().collect();
    let mut aliases: BTreeSet<&str> = BTreeSet::new();
    let mut names: BTreeSet<&str> = BTreeSet::new();

    assert_eq!(
        state::OFFICIAL_LENS_CATALOG.len(),
        PUBLISHED_OFFICIAL_LENSES.len(),
        "the catalog must carry every lens this suite pins, got {} entries",
        state::OFFICIAL_LENS_CATALOG.len()
    );

    for official in state::OFFICIAL_LENS_CATALOG {
        let case = official.alias;
        assert!(
            aliases.insert(official.alias),
            "case {case:?}: a LensAlias names one lens"
        );
        assert!(
            names.insert(official.name),
            "case {case:?}: a LensName names one lens, got {:?} twice",
            official.name
        );
        assert!(
            !official.questions.is_empty(),
            "case {case:?}: an official lens asks questions"
        );
        assert_eq!(
            PUBLISHED_OFFICIAL_LENSES
                .iter()
                .find(|(alias, _, _)| *alias == official.alias)
                .map(|(_, _, questions)| *questions),
            Some(official.questions.len()),
            "case {case:?}: the entry must ask the number of questions this suite's table \
             states, got {}",
            official.questions.len()
        );

        let pillars = official.pillars();
        let counted: usize = pillars.iter().map(|(_, _, count)| *count).sum();
        assert_eq!(
            counted,
            official.questions.len(),
            "case {case:?}: the pillar counts must add up to the lens's {} questions, got \
             {counted}",
            official.questions.len()
        );
        for (pillar_id, pillar_name, count) in &pillars {
            assert_eq!(
                published_pillars.get(pillar_id),
                Some(pillar_name),
                "case {case:?}: {pillar_id:?} must be a published pillar with its published name"
            );
            assert!(
                *count > 0,
                "case {case:?}: {pillar_id:?} is reported only because it asks questions"
            );
        }

        // The questions are grouped by pillar, in the pillar order the lens reports.
        let mut groups: Vec<&str> = Vec::new();
        for question in official.questions {
            if groups.last() != Some(&question.pillar_id) {
                groups.push(question.pillar_id);
            }
            assert!(
                !question.best_practices.is_empty(),
                "case {case:?}: question {:?} asks about best practices",
                question.question_id
            );
            assert!(
                (QUESTION_ID_LENGTH.0..=QUESTION_ID_LENGTH.1).contains(&question.question_id.len()),
                "case {case:?}: QuestionId is @length{QUESTION_ID_LENGTH:?}, got {:?}",
                question.question_id
            );
        }
        let expected_groups: Vec<&str> = pillars.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(
            groups, expected_groups,
            "case {case:?}: the questions must be grouped in the lens's pillar order"
        );

        // The version names a day, and the instants the summary reports are that day's
        // midnight — the two cannot drift apart.
        let rendered = ::aws_smithy_types::DateTime::from(official.published_at())
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default();
        assert_eq!(
            rendered,
            format!("{}T00:00:00Z", official.version),
            "case {case:?}: the publication instant must be midnight of the day the version \
             names, got {rendered:?}"
        );

        // A lens is the same lens however a request names it: alias, ARN, version-pinned
        // ARN. A CUSTOM lens ARN carrying the same trailing segment is a different lens.
        for named in [
            official.alias.to_string(),
            official_lens_arn(REQUEST_REGION, official.alias),
            format!(
                "{}/{}",
                official_lens_arn(REQUEST_REGION, official.alias),
                official.version
            ),
        ] {
            assert_eq!(
                state::lens_question_count(&named),
                official.questions.len(),
                "case {case:?}: {named:?} must name the same lens"
            );
        }
        let custom = format!(
            "arn:aws:wellarchitected:{REQUEST_REGION}:210987654321:lens/{}",
            official.alias
        );
        assert_eq!(
            state::lens_question_count(&custom),
            0,
            "case {case:?}: a custom lens ARN names no official lens, got {custom:?}"
        );
    }

    assert!(
        state::official_lens(UNCARRIED_OFFICIAL_ALIAS).is_none(),
        "case \"uncarried\": {UNCARRIED_OFFICIAL_ALIAS:?} is carried by no catalog entry"
    );
    // ... and it is still an alias AWS publishes, so a workload may be reviewed against
    // it: the content catalog is a SUBSET of the published aliases, never a different set.
    assert!(
        state::is_published_official_lens(UNCARRIED_OFFICIAL_ALIAS),
        "case \"uncarried\": {UNCARRIED_OFFICIAL_ALIAS:?} is an alias AWS publishes"
    );
    for official in state::OFFICIAL_LENS_CATALOG {
        assert!(
            state::is_published_official_lens(official.alias),
            "case {:?}: every catalog entry must name a lens AWS publishes",
            official.alias
        );
    }
}
