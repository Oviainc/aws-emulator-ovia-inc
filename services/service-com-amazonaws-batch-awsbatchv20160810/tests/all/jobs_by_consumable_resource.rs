// SPDX-License-Identifier: CC0-1.0

//! `ListJobsByConsumableResource`, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a listing reports exactly the jobs whose persisted `consumableResourceProperties`
//!   require the resource the request names — resolved by name or by ARN, whichever form
//!   either side used — each summary's `quantity` being what that job's requirement asks
//!   of it, and every other member read from the persisted job; a resource nothing
//!   requires answers an empty list rather than a refusal;
//! * a `filters` entry narrows it: `JOB_NAME` matches the job name case-insensitively,
//!   with a trailing `*` matching any name that begins with the text before it, and
//!   `JOB_STATUS` matches the job's status, any of an entry's values matching and several
//!   entries narrowing cumulatively; a filter name the operation does not document is the
//!   modelled `ClientException` (HTTP 400);
//! * paging with `maxResults`/`nextToken` sees every job exactly once in the unpaged
//!   order, `maxResults` is accepted on both published bounds and refused one past either,
//!   a request that names none is still bounded at the documented default, and a
//!   `nextToken` that resumes nothing is the modelled `ClientException`;
//! * a `consumableResource` this service holds nothing for is the modelled
//!   `ClientException` (HTTP 400), by name and by ARN, and so is a request past either
//!   ceiling this emulator applies to the `filters` it will walk — each ceiling being
//!   accepted at its exact value;
//! * a listing is a READ: two listings at the same instant answer identically, nothing it
//!   reports differs from what `DescribeJobs` reports, and it moves no quantity of the
//!   resource it lists.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::{
    MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTERS, MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTER_VALUES,
    MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS, MIN_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS,
};

use crate::job_lifecycle::{
    arn, compute_environment, container_properties, describe_job, error_code, error_message,
    job_queue, Batch,
};

/// `ListJobsByConsumableResource`'s route (`smithy_query(query="operation",
/// operation="ListJobsByConsumableResource")`: POST /v1/listjobsbyconsumableresource).
const LIST: &str = "/v1/listjobsbyconsumableresource";

/// The filter names this operation documents. Source: the
/// `ListJobsByConsumableResourceRequest$filters` doc (`smithy_query(query="doc",
/// shape="ListJobsByConsumableResourceRequest", member="filters")`): "The filter names and
/// values can be: name: JOB_STATUS ... name: JOB_NAME".
const DOCUMENTED_FILTERS: [&str; 2] = ["JOB_STATUS", "JOB_NAME"];

/// The members `ListJobsByConsumableResourceSummary` models. Source:
/// `smithy_query(query="shape", shape="ListJobsByConsumableResourceSummary")`.
const MODELLED_SUMMARY_MEMBERS: [&str; 11] = [
    "jobArn",
    "jobQueueArn",
    "jobName",
    "jobDefinitionArn",
    "shareIdentifier",
    "jobStatus",
    "quantity",
    "statusReason",
    "startedAt",
    "createdAt",
    "consumableResourceProperties",
];

/// One job this suite submits: the name it carries, and what it requires of which
/// consumable resource.
struct Submission {
    job_name: &'static str,
    /// Each requirement as `(consumableResource reference, quantity)` — the reference
    /// being either the resource's NAME or its ARN, the two forms the member documents.
    requirements: Vec<(String, i64)>,
}

/// The consumable resources this suite creates: one that several jobs require, one that a
/// single job requires, and one that nothing requires at all.
const RESOURCES: [&str; 3] = ["cr-gpu", "cr-license", "cr-idle"];

/// The ARN this region and account give the consumable resource `name`.
fn resource_arn(name: &str) -> String {
    arn(&format!("consumable-resource/{name}"))
}

/// The table this suite submits. Rows differ in how many resources a job requires, in
/// whether the requirement names the resource by NAME or by ARN, in the quantity it asks
/// for (its documented floor of 0, a small count and a large one), in the case and shape
/// of the job name, and in whether the job requires the listed resource at all.
fn submissions() -> Vec<Submission> {
    vec![
        Submission {
            job_name: "gpu-by-name",
            requirements: vec![(RESOURCES[0].to_string(), 2)],
        },
        Submission {
            job_name: "GPU-By-Arn",
            requirements: vec![(resource_arn(RESOURCES[0]), 5)],
        },
        Submission {
            job_name: "gpu-and-license",
            requirements: vec![
                (RESOURCES[0].to_string(), 1_000_000),
                (resource_arn(RESOURCES[1]), 1),
            ],
        },
        Submission {
            job_name: "gpu-zero",
            requirements: vec![(RESOURCES[0].to_string(), 0)],
        },
        Submission {
            job_name: "needs-nothing",
            requirements: Vec::new(),
        },
    ]
}

/// One emulator carrying the consumable resources, the queue and the submitted jobs of
/// [`submissions`].
struct Fixture {
    batch: Batch,
    queue_arn: String,
    /// `(job name, job id)` per submitted job, in submission order.
    jobs: Vec<(String, String)>,
    /// `(job name, job definition ARN)` per submitted job.
    definitions: Vec<(String, String)>,
}

impl Fixture {
    /// The id of the job called `job_name`.
    fn job_id(&self, job_name: &str) -> String {
        self.jobs
            .iter()
            .find(|(name, _)| name == job_name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("the fixture submitted a job called {job_name:?}"))
    }

    /// The job definition ARN the job called `job_name` ran.
    fn definition_arn(&self, job_name: &str) -> String {
        self.definitions
            .iter()
            .find(|(name, _)| name == job_name)
            .map(|(_, definition)| definition.clone())
            .unwrap_or_else(|| panic!("the fixture registered a definition for {job_name:?}"))
    }
}

/// `consumableResourceProperties` for `requirements`.
fn properties(requirements: &[(String, i64)]) -> Value {
    json!({
        "consumableResourceList": requirements
            .iter()
            .map(|(reference, quantity)| json!({
                "consumableResource": reference,
                "quantity": quantity,
            }))
            .collect::<Vec<Value>>(),
    })
}

/// An emulator with [`RESOURCES`] created, a queue to submit into, and every row of
/// [`submissions`] submitted — each through a job definition carrying that row's
/// requirements, so what the listing reports comes from PERSISTED state.
async fn fixture() -> Fixture {
    let batch = Batch::new();
    for name in RESOURCES {
        batch
            .ok(
                "/v1/createconsumableresource",
                json!({"consumableResourceName": name, "totalQuantity": 2_000_000}),
            )
            .await;
    }
    let environment_arn = compute_environment(&batch, "ce-cr").await;
    let queue_arn = job_queue(&batch, "q-cr", &environment_arn).await;

    let mut jobs = Vec::new();
    let mut definitions = Vec::new();
    for submission in submissions() {
        let mut definition = json!({
            "jobDefinitionName": format!("jd-{}", submission.job_name),
            "type": "container",
            "containerProperties": container_properties(),
        });
        if !submission.requirements.is_empty() {
            definition["consumableResourceProperties"] = properties(&submission.requirements);
        }
        let registered = batch.ok("/v1/registerjobdefinition", definition).await;
        let definition_arn = registered["jobDefinitionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected a jobDefinitionArn, got {registered}", submission.job_name))
            .to_string();
        let answered = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": submission.job_name,
                    "jobQueue": &queue_arn,
                    "jobDefinition": &definition_arn,
                }),
            )
            .await;
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected a jobId, got {answered}", submission.job_name))
            .to_string();
        jobs.push((submission.job_name.to_string(), job_id));
        definitions.push((submission.job_name.to_string(), definition_arn));
    }

    Fixture {
        batch,
        queue_arn,
        jobs,
        definitions,
    }
}

/// `ListJobsByConsumableResource` over the wire: the status and the decoded body.
async fn list(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call(LIST, body).await
}

/// `ListJobsByConsumableResource` over the wire, requiring the 200.
async fn listed(batch: &Batch, case: &str, body: Value) -> Value {
    let (status, decoded) = list(batch, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected ListJobsByConsumableResource to answer 200, got {status} {decoded}"
    );
    decoded
}

/// The `jobName`s of a listing, in the order it answered them.
fn job_names(body: &Value) -> Vec<String> {
    body["jobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["jobName"]
                .as_str()
                .unwrap_or("<no jobName>")
                .to_string()
        })
        .collect()
}

/// The one summary a listing answered for `job_name`.
fn summary_of(body: &Value, job_name: &str) -> Value {
    body["jobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["jobName"].as_str() == Some(job_name))
        .unwrap_or_else(|| panic!("case {job_name}: the listing answered no summary for it: {body}"))
}

/// Poll every submitted job until it reports `RUNNABLE`, where a job with nothing to place
/// it on comes to rest. Driven off OBSERVED state rather than a fixed sleep.
async fn wait_until_resting(fixture: &Fixture) {
    let span = u64::try_from(
        service_com_amazonaws_batch_awsbatchv20160810::state::JOB_PHASE_MILLIS,
    )
    .unwrap_or(500);
    for (job_name, job_id) in &fixture.jobs {
        for _ in 0..60 {
            let detail = describe_job(&fixture.batch, job_id).await;
            if detail["status"].as_str() == Some("RUNNABLE") {
                break;
            }
            tokio::time::sleep(Duration::from_millis((span / 20).max(1))).await;
        }
        let detail = describe_job(&fixture.batch, job_id).await;
        assert_eq!(
            detail["status"].as_str(),
            Some("RUNNABLE"),
            "case {job_name}: expected the job to come to rest in RUNNABLE, got {detail}"
        );
    }
}

/// RULE: a listing reports exactly the jobs whose persisted requirements name the listed
/// resource, whichever form either side used, and a resource nothing requires answers an
/// empty list.
#[tokio::test]
async fn a_listing_reports_every_job_that_requires_the_resource() {
    let fixture = fixture().await;

    // Rows differ in WHICH resource is listed and in whether the request names it by name
    // or by ARN; the two forms must select the same jobs.
    let cases: Vec<(String, Vec<&str>)> = vec![
        (
            RESOURCES[0].to_string(),
            vec!["gpu-by-name", "GPU-By-Arn", "gpu-and-license", "gpu-zero"],
        ),
        (
            resource_arn(RESOURCES[0]),
            vec!["gpu-by-name", "GPU-By-Arn", "gpu-and-license", "gpu-zero"],
        ),
        (RESOURCES[1].to_string(), vec!["gpu-and-license"]),
        (resource_arn(RESOURCES[1]), vec!["gpu-and-license"]),
        (RESOURCES[2].to_string(), Vec::new()),
        (resource_arn(RESOURCES[2]), Vec::new()),
    ];
    for (reference, expected) in cases {
        let case = format!("listing {reference}");
        let body = listed(
            &fixture.batch,
            &case,
            json!({"consumableResource": reference}),
        )
        .await;
        let mut answered = job_names(&body);
        answered.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        wanted.sort();
        assert_eq!(
            answered, wanted,
            "case {case}: expected {wanted:?}, got {body}"
        );
        // A job requiring nothing of it is never listed.
        assert!(
            !answered.contains(&"needs-nothing".to_string()),
            "case {case}: a job that requires none of the resource is not listed, got {body}"
        );
    }
}

/// RULE: a summary's `quantity` is what that job's requirement asks of the LISTED
/// resource, and every other member is read from the persisted job; the summary carries
/// only the members the model declares for it.
#[tokio::test]
async fn a_summary_reports_the_persisted_job_and_its_required_quantity() {
    let fixture = fixture().await;

    let body = listed(
        &fixture.batch,
        "every member",
        json!({"consumableResource": RESOURCES[0]}),
    )
    .await;

    // The quantity each row asks of the listed resource, from the table itself.
    for submission in submissions() {
        let required: Option<i64> = submission
            .requirements
            .iter()
            .filter(|(reference, _)| {
                *reference == RESOURCES[0] || *reference == resource_arn(RESOURCES[0])
            })
            .map(|(_, quantity)| *quantity)
            .reduce(i64::saturating_add);
        let Some(required) = required else {
            continue;
        };
        let case = submission.job_name;
        let summary = summary_of(&body, case);
        assert_eq!(
            summary["quantity"].as_i64(),
            Some(required),
            "case {case}: expected the quantity the job requires of {}, which is {required}, \
             got {summary}",
            RESOURCES[0]
        );
        assert_eq!(
            summary["jobArn"].as_str(),
            Some(arn(&format!("job/{}", fixture.job_id(case))).as_str()),
            "case {case}: expected the persisted job ARN, got {summary}"
        );
        assert_eq!(
            summary["jobQueueArn"].as_str(),
            Some(fixture.queue_arn.as_str()),
            "case {case}: expected the queue the job was submitted to, got {summary}"
        );
        assert_eq!(
            summary["jobDefinitionArn"].as_str(),
            Some(fixture.definition_arn(case).as_str()),
            "case {case}: expected the job definition the job ran, got {summary}"
        );
        // Every requirement the job carries is reported, not only the listed one.
        assert_eq!(
            summary["consumableResourceProperties"],
            properties(&submission.requirements),
            "case {case}: expected the persisted consumableResourceProperties, got {summary}"
        );
        // Nothing is invented for a job nothing placed on compute capacity.
        assert!(
            summary["startedAt"].is_null(),
            "case {case}: a job nothing started reports no startedAt, got {summary}"
        );
        // The listing agrees with what `DescribeJobs` reports for the same job.
        let detail = describe_job(&fixture.batch, &fixture.job_id(case)).await;
        for member in ["jobArn", "jobName", "createdAt", "status"] {
            let in_summary = if member == "status" {
                summary["jobStatus"].clone()
            } else {
                summary[member].clone()
            };
            assert_eq!(
                in_summary, detail[member],
                "case {case}: the listing's {member} must be what DescribeJobs reports, got \
                 {summary} and {detail}"
            );
        }
        let object = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a summary object, got {summary}"));
        for member in object.keys() {
            assert!(
                MODELLED_SUMMARY_MEMBERS.contains(&member.as_str()),
                "case {case}: {member} is not a member ListJobsByConsumableResourceSummary \
                 models, got {summary}"
            );
        }
    }
}

/// RULE: a `filters` entry narrows the listing — `JOB_NAME` case-insensitively with a
/// trailing `*` matching a prefix, `JOB_STATUS` on the job's status, any of an entry's
/// values matching and several entries narrowing cumulatively.
#[tokio::test]
async fn a_listing_honours_the_documented_filters() {
    let fixture = fixture().await;
    wait_until_resting(&fixture).await;

    // Rows differ in which documented filter is used, in the case and shape of the value,
    // in how many values an entry holds, in how many entries narrow the listing, and in
    // whether anything matches at all.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an exact job name",
            json!([{"name": DOCUMENTED_FILTERS[1], "values": ["gpu-by-name"]}]),
            vec!["gpu-by-name"],
        ),
        (
            "a job name in the other case",
            json!([{"name": DOCUMENTED_FILTERS[1], "values": ["gpu-by-arn"]}]),
            vec!["GPU-By-Arn"],
        ),
        (
            "a trailing * matches a prefix, across cases",
            json!([{"name": DOCUMENTED_FILTERS[1], "values": ["gpu-*"]}]),
            vec!["GPU-By-Arn", "gpu-and-license", "gpu-by-name", "gpu-zero"],
        ),
        (
            "several values match any of them",
            json!([{"name": DOCUMENTED_FILTERS[1], "values": ["gpu-zero", "gpu-by-name"]}]),
            vec!["gpu-by-name", "gpu-zero"],
        ),
        (
            "a job name nothing answers to selects nothing",
            json!([{"name": DOCUMENTED_FILTERS[1], "values": ["absent"]}]),
            Vec::new(),
        ),
        (
            "the status every resting job is in",
            json!([{"name": DOCUMENTED_FILTERS[0], "values": ["RUNNABLE"]}]),
            vec!["GPU-By-Arn", "gpu-and-license", "gpu-by-name", "gpu-zero"],
        ),
        (
            "a status no job is in",
            json!([{"name": DOCUMENTED_FILTERS[0], "values": ["SUCCEEDED"]}]),
            Vec::new(),
        ),
        (
            "several statuses match any of them",
            json!([{"name": DOCUMENTED_FILTERS[0], "values": ["SUCCEEDED", "RUNNABLE"]}]),
            vec!["GPU-By-Arn", "gpu-and-license", "gpu-by-name", "gpu-zero"],
        ),
        (
            "two entries narrow cumulatively",
            json!([
                {"name": DOCUMENTED_FILTERS[0], "values": ["RUNNABLE"]},
                {"name": DOCUMENTED_FILTERS[1], "values": ["gpu-zero"]},
            ]),
            vec!["gpu-zero"],
        ),
        (
            "two entries that cannot both match select nothing",
            json!([
                {"name": DOCUMENTED_FILTERS[0], "values": ["SUBMITTED"]},
                {"name": DOCUMENTED_FILTERS[1], "values": ["gpu-zero"]},
            ]),
            Vec::new(),
        ),
    ];
    for (case, filters, expected) in cases {
        let body = listed(
            &fixture.batch,
            case,
            json!({"consumableResource": RESOURCES[0], "filters": filters}),
        )
        .await;
        let mut answered = job_names(&body);
        answered.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        wanted.sort();
        assert_eq!(
            answered, wanted,
            "case {case}: expected {wanted:?}, got {body}"
        );
    }
}

/// RULE: the listing follows the writes — a terminate moves a job's status and reason, and
/// the status filter then selects it there — and reports no quantity of its own invention.
#[tokio::test]
async fn a_listing_follows_what_the_writes_left() {
    let fixture = fixture().await;

    let reason = "ended by the suite";
    fixture
        .batch
        .ok(
            "/v1/terminatejob",
            json!({"jobId": fixture.job_id("gpu-zero"), "reason": reason}),
        )
        .await;

    let body = listed(
        &fixture.batch,
        "after the terminate",
        json!({
            "consumableResource": RESOURCES[0],
            "filters": [{"name": DOCUMENTED_FILTERS[0], "values": ["FAILED"]}],
        }),
    )
    .await;
    assert_eq!(
        job_names(&body),
        vec!["gpu-zero".to_string()],
        "case after the terminate: expected only the terminated job, got {body}"
    );
    let summary = summary_of(&body, "gpu-zero");
    assert_eq!(
        summary["statusReason"].as_str(),
        Some(reason),
        "case after the terminate: expected the reason the terminate stored, got {summary}"
    );
    assert_eq!(
        summary["quantity"].as_i64(),
        Some(0),
        "case after the terminate: expected the quantity the job requires, got {summary}"
    );
}

/// RULE: paging with `maxResults`/`nextToken` sees every job exactly once in the unpaged
/// order, and `maxResults` is accepted on both published bounds.
#[tokio::test]
async fn paging_sees_every_job_exactly_once() {
    let fixture = fixture().await;

    let unpaged = listed(
        &fixture.batch,
        "unpaged",
        json!({"consumableResource": RESOURCES[0]}),
    )
    .await;
    let expected = job_names(&unpaged);
    assert_eq!(
        expected.len(),
        4,
        "case unpaged: expected the four jobs that require it, got {unpaged}"
    );

    // Page sizes differ: one at a time, a size that does not divide the total, the exact
    // total, and both published bounds of `maxResults` (1 and 100).
    for page_size in [
        MIN_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS,
        3,
        i32::try_from(expected.len()).expect("the fixture fits in an i32"),
        MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS,
    ] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request =
                json!({"consumableResource": RESOURCES[0], "maxResults": page_size});
            if let Some(resume) = token.as_deref() {
                request["nextToken"] = json!(resume);
            }
            let body = listed(&fixture.batch, &case, request).await;
            let page = job_names(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page holds at most maxResults jobs, got {body}"
            );
            seen.extend(page);
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging did not terminate after {pages} pages, last token {token:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must see every job exactly once in the unpaged order, got \
             {seen:?}"
        );
    }
}

/// RULE: a request that names no `maxResults` is still bounded at the documented default
/// ("returns up to 100 results and a nextToken value if applicable"), and paging from that
/// first page sees the rest.
#[tokio::test]
async fn a_listing_that_names_no_page_size_is_bounded_at_the_documented_default() {
    let batch = Batch::new();
    batch
        .ok(
            "/v1/createconsumableresource",
            json!({"consumableResourceName": RESOURCES[0], "totalQuantity": 1_000}),
        )
        .await;
    let environment_arn = compute_environment(&batch, "ce-paged").await;
    let queue_arn = job_queue(&batch, "q-paged", &environment_arn).await;
    let registered = batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "jd-paged",
                "type": "container",
                "containerProperties": container_properties(),
                "consumableResourceProperties": properties(&[(RESOURCES[0].to_string(), 1)]),
            }),
        )
        .await;
    let definition_arn = registered["jobDefinitionArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let default = usize::try_from(MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS)
        .expect("the documented default is positive");
    // One past the default, so the first page cannot hold them all.
    let total = default + 1;
    for index in 0..total {
        batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("paged-{index:04}"),
                    "jobQueue": &queue_arn,
                    "jobDefinition": &definition_arn,
                }),
            )
            .await;
    }

    let first = listed(
        &batch,
        "no maxResults",
        json!({"consumableResource": RESOURCES[0]}),
    )
    .await;
    let page = job_names(&first);
    assert_eq!(
        page.len(),
        default,
        "case no maxResults: expected the documented default page size {default}, got {}",
        page.len()
    );
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    assert!(
        token.is_some(),
        "case no maxResults: a page that leaves results carries a nextToken, got {first}"
    );

    let rest = listed(
        &batch,
        "resuming the default page",
        json!({
            "consumableResource": RESOURCES[0],
            "nextToken": token.expect("the first page carries a token"),
        }),
    )
    .await;
    let mut seen = page;
    seen.extend(job_names(&rest));
    assert_eq!(
        seen.len(),
        total,
        "case resuming the default page: expected every job exactly once, got {} of {total}",
        seen.len()
    );
    assert!(
        rest.get("nextToken").is_none(),
        "case resuming the default page: the last page carries no nextToken, got {rest}"
    );
}

/// RULE: a request this operation cannot answer is the modelled `ClientException` (HTTP
/// 400): a `consumableResource` nothing answers to (by name or by ARN), a filter name it
/// does not document, a `maxResults` outside the published 1..100, and a `nextToken` that
/// resumes nothing.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_answer() {
    let fixture = fixture().await;

    // Rows differ in WHICH part of the request is unusable; each must be the modelled
    // 400, never a 5xx or a 501.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a consumable resource name nothing answers to",
            json!({"consumableResource": "cr-absent"}),
        ),
        (
            "a consumable resource ARN nothing answers to",
            json!({"consumableResource": resource_arn("cr-absent")}),
        ),
        (
            "a filter name the operation does not document",
            json!({"consumableResource": RESOURCES[0], "filters": [{"name": "JOB_DEFINITION", "values": ["jd-gpu-zero"]}]}),
        ),
        (
            "a filter entry carrying no name",
            json!({"consumableResource": RESOURCES[0], "filters": [{"values": ["gpu-zero"]}]}),
        ),
        (
            "a documented filter name in the wrong case",
            json!({"consumableResource": RESOURCES[0], "filters": [{"name": "job_name", "values": ["gpu-zero"]}]}),
        ),
        (
            "maxResults one below the published minimum",
            json!({
                "consumableResource": RESOURCES[0],
                "maxResults": MIN_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS - 1,
            }),
        ),
        (
            "maxResults one past the published maximum",
            json!({
                "consumableResource": RESOURCES[0],
                "maxResults": MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS + 1,
            }),
        ),
        (
            "a negative maxResults",
            json!({"consumableResource": RESOURCES[0], "maxResults": -1}),
        ),
        (
            "a nextToken that resumes nothing",
            json!({"consumableResource": RESOURCES[0], "maxResults": 1, "nextToken": "not-a-token"}),
        ),
        (
            "a nextToken past the end of the results",
            json!({"consumableResource": RESOURCES[0], "nextToken": "9999"}),
        ),
        (
            "one filter entry past the ceiling this emulator applies",
            json!({
                "consumableResource": RESOURCES[0],
                "filters": (0..=MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTERS)
                    .map(|_| json!({"name": DOCUMENTED_FILTERS[1], "values": ["gpu-*"]}))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "one filter value past the ceiling this emulator applies",
            json!({
                "consumableResource": RESOURCES[0],
                "filters": [{
                    "name": DOCUMENTED_FILTERS[1],
                    "values": (0..=MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTER_VALUES)
                        .map(|index| json!(format!("gpu-{index}")))
                        .collect::<Vec<Value>>(),
                }],
            }),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = list(&fixture.batch, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException with HTTP 400, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected ClientException, got {body} ({})",
            error_message(&body)
        );
    }

    // The refusal for an unknown resource names the ARN the reference resolves to, the
    // way every resolver in this service words a not-found.
    let (_, body) = list(
        &fixture.batch,
        json!({"consumableResource": "cr-absent"}),
    )
    .await;
    assert!(
        error_message(&body).contains(&resource_arn("cr-absent")),
        "case an unknown resource: expected the resolved ARN in the message, got {body}"
    );

    // Both published bounds of `maxResults` are ACCEPTED, so the refusals above are the
    // bound and not the member.
    for accepted in [
        MIN_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS,
        MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS,
    ] {
        let case = format!("maxResults {accepted}");
        listed(
            &fixture.batch,
            &case,
            json!({"consumableResource": RESOURCES[0], "maxResults": accepted}),
        )
        .await;
    }

    // Each filter ceiling is ACCEPTED at its exact value, and a repeated filter narrows
    // to the same jobs one copy of it does.
    let at_entry_ceiling = listed(
        &fixture.batch,
        "filters at the ceiling",
        json!({
            "consumableResource": RESOURCES[0],
            "filters": (0..MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTERS)
                .map(|_| json!({"name": DOCUMENTED_FILTERS[1], "values": ["gpu-zero"]}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        job_names(&at_entry_ceiling),
        vec!["gpu-zero".to_string()],
        "case filters at the ceiling: a repeated filter selects what one copy selects, got \
         {at_entry_ceiling}"
    );
    let at_value_ceiling = listed(
        &fixture.batch,
        "filter values at the ceiling",
        json!({
            "consumableResource": RESOURCES[0],
            "filters": [{
                "name": DOCUMENTED_FILTERS[1],
                "values": (0..MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTER_VALUES)
                    .map(|index| if index == 0 {
                        json!("gpu-zero")
                    } else {
                        json!(format!("absent-{index}"))
                    })
                    .collect::<Vec<Value>>(),
            }],
        }),
    )
    .await;
    assert_eq!(
        job_names(&at_value_ceiling),
        vec!["gpu-zero".to_string()],
        "case filter values at the ceiling: any of the entry's values matching selects the \
         job, got {at_value_ceiling}"
    );
}

/// RULE: a listing is a READ — two listings at the same instant answer identically, and it
/// moves no quantity of the resource it lists.
#[tokio::test]
async fn a_listing_changes_nothing_a_later_caller_sees() {
    let fixture = fixture().await;
    wait_until_resting(&fixture).await;

    let before = fixture
        .batch
        .ok(
            "/v1/describeconsumableresource",
            json!({"consumableResource": RESOURCES[0]}),
        )
        .await;

    let first = listed(
        &fixture.batch,
        "twice",
        json!({"consumableResource": RESOURCES[0]}),
    )
    .await;
    let again = listed(
        &fixture.batch,
        "twice",
        json!({"consumableResource": RESOURCES[0]}),
    )
    .await;
    assert_eq!(
        first, again,
        "case twice: listing twice must answer identically, got {first} then {again}"
    );

    let after = fixture
        .batch
        .ok(
            "/v1/describeconsumableresource",
            json!({"consumableResource": RESOURCES[0]}),
        )
        .await;
    assert_eq!(
        before, after,
        "case the resource's own quantities: a listing moves none of them, got {before} then \
         {after}"
    );

    // Every job the listing reported is still exactly where `DescribeJobs` says it is.
    for (job_name, job_id) in &fixture.jobs {
        let detail = describe_job(&fixture.batch, job_id).await;
        assert_eq!(
            detail["status"].as_str(),
            Some("RUNNABLE"),
            "case {job_name}: a listing moves no job's status, got {detail}"
        );
    }
}
