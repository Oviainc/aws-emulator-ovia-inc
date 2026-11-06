// SPDX-License-Identifier: CC0-1.0

//! What `ListEvaluationJobs` reports, over the same `MultiServiceRouter` the emulator boots
//! (`crate::support`).
//!
//! THE RULE UNDER TEST: the listing reports the stored evaluation jobs of the account-region
//! the request addressed, one `EvaluationSummary` apiece, ordered by the creation time
//! `sortBy` admits and in the direction `sortOrder` asks for, narrowed by every filter the
//! request carries (`nameContains`, `statusEquals`, `applicationTypeEquals`,
//! `creationTimeAfter` / `creationTimeBefore`), and paged with `maxResults` / `nextToken` so
//! the pages partition the listing with neither a gap nor a repeat. Every member of a
//! summary comes off the same stored record `GetEvaluationJob` reports, so a listing and a
//! read of one job agree.
//!
//! The assertions run over a SET of jobs that differ along what the rule generalises over:
//! the Region they were created in, their names, their application types, the task types and
//! datasets their configurations carry, and how many models their `inferenceConfig` names.
//!
//! Every expectation is derived from the model
//! (`smithy_query(query="operation",operation="ListEvaluationJobs")` /
//! `query="shape",shape="EvaluationSummary"`).

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, REGION};

/// The Region the listing is asserted in, and one other: a job of another Region is another
/// account-region's resource.
const OTHER_REGION: &str = "eu-central-1";

/// A `RoleArn` and an `EvaluationOutputDataConfig$s3Uri` the model admits.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/bedrock-evaluation";
const OUTPUT_URI: &str = "s3://evaluation-results/model-eval/";

/// `EvaluationJobStatus` values: the status every accepted job carries while it runs, and a
/// terminal one no job of this suite has reached.
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// `jobSummaries` targets `EvaluationSummaries`, which is `@length(min: 1, max: 5)`, so a
/// page carries at most five jobs whatever `maxResults` asks for.
const MAX_PAGE_SIZE: usize = 5;

/// One job this suite creates: its Region, name, application type, task type and dataset.
struct Job {
    region: &'static str,
    name: &'static str,
    application_type: Option<&'static str>,
    task_type: &'static str,
    dataset: &'static str,
}

/// Seven jobs: six in [`REGION`] (more than one page of them) and one in another Region.
const JOBS: &[Job] = &[
    Job {
        region: REGION,
        name: "listing-generation",
        application_type: Some("ModelEvaluation"),
        task_type: "Generation",
        dataset: "Builtin.RealToxicityPrompts",
    },
    Job {
        region: REGION,
        name: "listing-summaries",
        application_type: Some("ModelEvaluation"),
        task_type: "Summarization",
        dataset: "Builtin.Gigaword",
    },
    Job {
        region: REGION,
        name: "listing-questions",
        application_type: None,
        task_type: "QuestionAndAnswer",
        dataset: "Builtin.NaturalQuestions",
    },
    Job {
        region: REGION,
        name: "listing-classification",
        application_type: Some("RagEvaluation"),
        task_type: "Classification",
        dataset: "Builtin.WomensEcommerceClothingReviews",
    },
    Job {
        region: REGION,
        name: "other-generation",
        application_type: Some("ModelEvaluation"),
        task_type: "Generation",
        dataset: "Builtin.Bold",
    },
    Job {
        region: REGION,
        name: "other-trivia",
        application_type: None,
        task_type: "QuestionAndAnswer",
        dataset: "Builtin.TriviaQA",
    },
    Job {
        region: OTHER_REGION,
        name: "listing-elsewhere",
        application_type: Some("ModelEvaluation"),
        task_type: "Generation",
        dataset: "Builtin.Bold",
    },
];

/// A model the published catalogue carries.
const MODEL_ID: &str = "amazon.nova-lite-v1:0";

/// Create every job of [`JOBS`], in order, and hand back the `jobArn` each one minted.
async fn given_jobs(emu: &Emulator) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    for job in JOBS {
        let mut body = json!({
            "jobName": job.name,
            "roleArn": ROLE_ARN,
            "outputDataConfig": {"s3Uri": OUTPUT_URI},
            "evaluationConfig": {"automated": {"datasetMetricConfigs": [{
                "taskType": job.task_type,
                "dataset": {"name": job.dataset},
                "metricNames": ["Builtin.Toxicity"],
            }]}},
            "inferenceConfig": {"models": [{"bedrockModel": {"modelIdentifier": MODEL_ID}}]},
        });
        if let Some(application_type) = job.application_type {
            body["applicationType"] = json!(application_type);
        }
        let created = emu
            .call_in_region(job.region, "POST", "/evaluation-jobs", Some(body))
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: the create that seeds this row must be accepted, got {} {:?}",
            job.name, created.status, created.body
        );
        arns.push(created.text("jobArn"));
    }
    arns
}

/// `GET /evaluation-jobs` in `region`, with `query` appended when it carries anything.
async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Value {
    let path = if query.is_empty() {
        "/evaluation-jobs".to_string()
    } else {
        format!("/evaluation-jobs?{query}")
    };
    let listed = emu.call_in_region(region, "GET", &path, None).await;
    assert_eq!(
        listed.status, 200,
        "case {query:?}: ListEvaluationJobs is bound to @http(code: 200), got {} {:?}",
        listed.status, listed.body
    );
    listed.body
}

/// The `jobName` of each summary a listing page carries, in the order it reports them.
fn names_of(body: &Value) -> Vec<String> {
    body.get("jobSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("jobName").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every name a listing reports across its pages, following `nextToken` the way a client
/// does.
async fn every_name(emu: &Emulator, region: &str, query: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let paged = match token.as_deref() {
            Some(token) => format!("{query}&nextToken={token}"),
            None => query.to_string(),
        };
        let body = list_in_region(emu, region, paged.trim_start_matches('&')).await;
        let page = names_of(&body);
        assert!(
            page.len() <= MAX_PAGE_SIZE,
            "case {query:?}: a page carries at most {MAX_PAGE_SIZE} summaries, got {page:?}"
        );
        names.extend(page);
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return names;
        }
        assert!(
            names.len() <= JOBS.len(),
            "case {query:?}: the pages must not repeat a job, got {names:?}"
        );
    }
}

/// The names of the jobs this suite created in `region`, in creation order.
fn created_in(region: &str) -> Vec<String> {
    JOBS.iter()
        .filter(|job| job.region == region)
        .map(|job| job.name.to_string())
        .collect()
}

/// The epoch seconds a `Timestamp` member carries, read through either wire form a
/// `Timestamp` of this service arrives in — the RFC 3339 text a `@timestampFormat` member
/// carries, and the epoch number one without it does.
fn instant_of(case: &str, value: Option<&Value>) -> f64 {
    match value {
        Some(Value::String(text)) => crate::support::date_time_seconds(case, text),
        Some(Value::Number(seconds)) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {seconds} is not a number of seconds")),
        other => panic!("case {case:?}: expected a Timestamp, got {other:?}"),
    }
}

#[tokio::test]
async fn the_listing_reports_every_job_of_the_region_in_creation_order() {
    let emu = emulator();
    given_jobs(&emu).await;

    // Ascending (the order an absent `sortOrder` takes) is creation order, and every job of
    // the Region is reported across the pages exactly once.
    let ascending = every_name(&emu, REGION, "").await;
    assert_eq!(
        ascending,
        created_in(REGION),
        "case \"every job of the region\": expected the jobs in creation order, got {ascending:?}"
    );

    // `sortOrder=Descending` reverses it, and `sortBy=CreationTime` is the one value the
    // member admits.
    let descending = every_name(&emu, REGION, "sortBy=CreationTime&sortOrder=Descending").await;
    let mut reversed = created_in(REGION);
    reversed.reverse();
    assert_eq!(
        descending, reversed,
        "case \"descending\": expected the reverse of creation order, got {descending:?}"
    );

    // A job created in another Region is another account-region's resource.
    let elsewhere = every_name(&emu, OTHER_REGION, "").await;
    assert_eq!(
        elsewhere,
        created_in(OTHER_REGION),
        "case \"another region\": a listing reports that region's own jobs, got {elsewhere:?}"
    );
}

#[tokio::test]
async fn a_page_carries_at_most_the_requested_number_of_jobs() {
    let emu = emulator();
    given_jobs(&emu).await;

    // (case, maxResults, the page size it yields)
    let rows = [
        ("one job per page", Some(1), 1),
        ("two jobs per page", Some(2), 2),
        ("the modelled page ceiling", Some(5), MAX_PAGE_SIZE),
        (
            "more than the response's list admits",
            Some(1000),
            MAX_PAGE_SIZE,
        ),
        ("no maxResults at all", None, MAX_PAGE_SIZE),
    ];
    for (case, max_results, expected) in rows {
        let query = max_results.map_or_else(String::new, |max| format!("maxResults={max}"));
        let body = list_in_region(&emu, REGION, &query).await;
        let page = names_of(&body);
        assert_eq!(
            page.len(),
            expected,
            "case {case:?}: expected {expected} summaries on the first page, got {page:?}"
        );
        // Four jobs of this Region are still unreported, so the response hands back the
        // cursor the next page starts on.
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_some(),
            "case {case:?}: a listing with jobs left to report carries a nextToken, got {body:?}"
        );
        // The pages partition the listing: every job, once, whatever the page size.
        let paged = every_name(&emu, REGION, &query).await;
        assert_eq!(
            paged,
            created_in(REGION),
            "case {case:?}: the pages report every job of the region once, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn every_filter_narrows_the_listing_to_the_jobs_it_names() {
    let emu = emulator();
    given_jobs(&emu).await;

    // (case, the query, the names it reports)
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "nameContains a fragment four jobs carry",
            "nameContains=listing".to_string(),
            vec![
                "listing-generation".to_string(),
                "listing-summaries".to_string(),
                "listing-questions".to_string(),
                "listing-classification".to_string(),
            ],
        ),
        (
            "nameContains a fragment one job carries",
            "nameContains=trivia".to_string(),
            vec!["other-trivia".to_string()],
        ),
        (
            "nameContains a fragment no job carries",
            "nameContains=absent".to_string(),
            Vec::new(),
        ),
        (
            "statusEquals the status every accepted job carries",
            format!("statusEquals={IN_PROGRESS}"),
            created_in(REGION),
        ),
        (
            "statusEquals a status no job of this account has reached",
            format!("statusEquals={COMPLETED}"),
            Vec::new(),
        ),
        (
            "applicationTypeEquals the type three jobs carry",
            "applicationTypeEquals=ModelEvaluation".to_string(),
            vec![
                "listing-generation".to_string(),
                "listing-summaries".to_string(),
                "other-generation".to_string(),
            ],
        ),
        (
            "applicationTypeEquals the type one job carries",
            "applicationTypeEquals=RagEvaluation".to_string(),
            vec!["listing-classification".to_string()],
        ),
        (
            "creationTimeAfter an instant every job was created before",
            "creationTimeAfter=2100-01-01T00%3A00%3A00Z".to_string(),
            Vec::new(),
        ),
        (
            "creationTimeBefore an instant every job was created before",
            "creationTimeBefore=2100-01-01T00%3A00%3A00Z".to_string(),
            created_in(REGION),
        ),
        (
            "creationTimeAfter an instant every job was created after",
            "creationTimeAfter=2020-01-01T00%3A00%3A00Z".to_string(),
            created_in(REGION),
        ),
        (
            "creationTimeBefore an instant every job was created after",
            "creationTimeBefore=2020-01-01T00%3A00%3A00Z".to_string(),
            Vec::new(),
        ),
        (
            "two filters at once",
            "nameContains=listing&applicationTypeEquals=ModelEvaluation".to_string(),
            vec![
                "listing-generation".to_string(),
                "listing-summaries".to_string(),
            ],
        ),
    ];

    for (case, query, expected) in rows {
        let reported = every_name(&emu, REGION, &query).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the jobs {expected:?}, got {reported:?}"
        );
        if expected.is_empty() {
            // The member is reported whether or not a job answers the filter, so a client
            // that reads the list out of the response finds an empty list rather than a
            // response without the member.
            let body = list_in_region(&emu, REGION, &query).await;
            assert_eq!(
                body.get("jobSummaries"),
                Some(&json!([])),
                "case {case:?}: a listing with no job to report carries an empty jobSummaries, \
                 got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_reports_what_a_read_of_the_same_job_reports() {
    let emu = emulator();
    let arns = given_jobs(&emu).await;

    let body = list_in_region(&emu, REGION, "maxResults=5").await;
    let summaries = body
        .get("jobSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !summaries.is_empty(),
        "the listing must report the jobs this suite created, got {body:?}"
    );

    for summary in &summaries {
        let arn = summary
            .get("jobArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("every summary carries a jobArn, got {summary}"));
        assert!(
            arns.contains(&arn.to_string()),
            "case {arn:?}: the listing reports a job some create minted, got {summary}"
        );
        let read = emu
            .call_in_region(
                REGION,
                "GET",
                &format!("/evaluation-jobs/{}", label(arn)),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {arn:?}: the job the listing reported must be readable, got {} {:?}",
            read.status, read.body
        );
        // The members the two responses share come off ONE stored record.
        for member in ["jobName", "status", "jobType", "applicationType"] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                read.get(member),
                "case {arn:?}: {member} must agree between the listing and the read, got {:?} \
                 and {:?}",
                summary.get(member),
                read.get(member)
            );
        }
        // `creationTime` is the same INSTANT in both, read through the one codec that reads
        // either wire form a `Timestamp` member of this service arrives in.
        let listed_at = instant_of(arn, summary.get("creationTime"));
        let read_at = instant_of(arn, Some(&read.get("creationTime")));
        assert!(
            (listed_at - read_at).abs() < 1e-3,
            "case {arn:?}: creationTime must agree between the listing and the read, got \
             {listed_at} and {read_at}"
        );
        // `evaluationTaskTypes` is the task type of each `datasetMetricConfigs` entry the
        // job was created with, and `modelIdentifiers` / `inferenceConfigSummary` the models
        // its `inferenceConfig` names.
        let configured = read.get("evaluationConfig");
        let task_types: Vec<&str> = configured
            .pointer("/automated/datasetMetricConfigs")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("taskType").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            summary.get("evaluationTaskTypes"),
            Some(&json!(task_types)),
            "case {arn:?}: evaluationTaskTypes are the configured task types {task_types:?}, got \
             {summary}"
        );
        assert_eq!(
            summary.get("modelIdentifiers"),
            Some(&json!([MODEL_ID])),
            "case {arn:?}: modelIdentifiers are the models the inferenceConfig names, got \
             {summary}"
        );
        assert_eq!(
            summary.pointer("/inferenceConfigSummary/modelConfigSummary/bedrockModelIdentifiers"),
            Some(&json!([MODEL_ID])),
            "case {arn:?}: the inferenceConfigSummary names the same models, got {summary}"
        );
    }
}
