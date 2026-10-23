// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the BackupSearch search-job slice, driven over HTTP exactly as
//! a client drives it: `StartSearchJob`, `GetSearchJob`, `ListSearchJobs`,
//! `ListSearchJobResults`, `StopSearchJob`, `TagResource` and `ListTagsForResource`.
//!
//! The rule under test is that every one of those answers is computed from the record
//! `StartSearchJob` persisted — so each assertion is made over a TABLE of requests that
//! differ in the members the rule generalises over (name, scope, resource type, filters,
//! tags, signing region, page size, job count), never over the one request the issue
//! happened to cite.
//!
//! The HTTP harness below (`emulator`, `call`, `Answer`) and the two transition-span
//! constants are `pub(crate)` because the export-job suite (`tests/all/export_jobs.rs`)
//! drives the same emulator the same way.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::EmulatedService;

/// Longer than `crate::state::TRANSITION_DELAY`, so the clock has made a search job's
/// owed status transition due by the time the next call is made.
pub(crate) const PAST_THE_DUE_INSTANT: Duration = Duration::from_millis(900);

/// Shorter than the transition span, to pin that the span is NOT zero: a caller acting
/// straight after `StartSearchJob` still observes the transient `RUNNING`.
pub(crate) const WITHIN_THE_DUE_INSTANT: Duration = Duration::from_millis(50);

/// One emulator holding one service over one shared engine, as the router builds it,
/// with that engine so a test can seed the store directly where driving thousands of
/// requests would only be a slow way to reach the same state.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One emulator holding one service over one shared engine, as the router builds it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// One recovery point of the backup index a search job's `SearchScope` is resolved
/// against — what the Backup data plane holds and what a scan has to get through. The
/// members are the ones a scope filters on: the model's `ResourceType`, the
/// recovery-point and source-resource ARNs, and the instant the backup was created.
pub(crate) fn recovery_point(
    resource_type: &str,
    backup_resource_arn: &str,
    source_resource_arn: &str,
    created: std::time::SystemTime,
) -> types::SearchJobBackupsResult {
    types::SearchJobBackupsResult {
        status: Some(state::COMPLETED.to_string()),
        status_message: None,
        resource_type: Some(resource_type.to_string()),
        backup_resource_arn: Some(backup_resource_arn.to_string()),
        source_resource_arn: Some(source_resource_arn.to_string()),
        index_creation_time: Some(state::timestamp(created)),
        backup_creation_time: Some(state::timestamp(created)),
    }
}

/// Put `points` in the store as the recovery points this account holds. Nothing in THIS
/// service creates one — the Backup data plane does — so a test that needs a scope to
/// resolve to something seeds them, exactly as the store would hold them.
pub(crate) fn seed_recovery_points(
    engine: &engine::InMemoryEngine,
    points: Vec<types::SearchJobBackupsResult>,
) {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    store.lock().recovery_points = points;
}

/// A request's SigV4 credential scope, which is where the service reads the region its
/// ARNs name from. Signatures are not verified here (authentication is off above); the
/// header is what routes the request to this service.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/backup-search/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// What one call answered: the HTTP status, the parsed JSON body (`Value::Null` for an
/// empty one) and the wire error code restJson1 resolves an error shape from.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) error_code: Option<String>,
}

impl Answer {
    /// The body member `name`, or `Value::Null` when the response does not carry it.
    pub(crate) fn get(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    /// A string body member, for the many members modelled as strings.
    pub(crate) fn string(&self, name: &str) -> String {
        self.get(name)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) async fn call(
    emulator: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "backup-search.amazonaws.com")
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        body,
        error_code,
    }
}

/// One row of the start-a-job table: a `StartSearchJob` body and the region it is signed
/// for. The rows differ in resource type, in whether `Name`/`ItemFilters`/`Tags` are
/// present, in filter count and in signing region.
struct StartCase {
    case: &'static str,
    region: &'static str,
    body: Value,
}

fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "s3 scope, named, tagged, one object-key filter",
            region: "us-west-2",
            body: json!({
                "Name": "demo",
                "SearchScope": {"BackupResourceTypes": ["S3"]},
                "ItemFilters": {"S3ItemFilters": [{
                    "ObjectKeys": [{"Value": "reports/", "Operator": "BEGINS_WITH"}]
                }]},
                "Tags": {"env": "test"}
            }),
        },
        StartCase {
            // `ResourceType` accepts S3 and EBS (model enum `ResourceType`); EBS is the
            // entry the issue did not cite.
            case: "ebs scope, no name, two file-path filters, no tags",
            region: "eu-west-1",
            body: json!({
                "SearchScope": {"BackupResourceTypes": ["EBS"]},
                "ItemFilters": {"EBSItemFilters": [{
                    "FilePaths": [
                        {"Value": "/var/log", "Operator": "BEGINS_WITH"},
                        {"Value": ".tmp", "Operator": "DOES_NOT_END_WITH"}
                    ],
                    "Sizes": [{"Value": 1024, "Operator": "GREATER_THAN_EQUAL_TO"}]
                }]}
            }),
        },
        StartCase {
            case: "s3 scope with source arns and an encryption key, no item filters",
            region: "us-east-1",
            body: json!({
                "Name": "scoped-to-two-buckets",
                "EncryptionKeyArn": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
                "SearchScope": {
                    "BackupResourceTypes": ["S3"],
                    "SourceResourceArns": ["arn:aws:s3:::alpha", "arn:aws:s3:::beta"],
                    "BackupResourceTags": {"team": "a", "tier": "gold"}
                },
                "Tags": {"env": "prod", "owner": "platform"}
            }),
        },
    ]
}

#[tokio::test]
async fn a_started_job_is_persisted_and_every_member_is_read_back_out_of_the_store() {
    for StartCase { case, region, body } in start_cases() {
        let emulator = emulator();
        let started = call(&emulator, region, "PUT", "/search-jobs", Some(body.clone())).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from StartSearchJob, got {} ({:?})",
            started.status, started.body
        );

        let identifier = started.string("SearchJobIdentifier");
        assert!(
            !identifier.is_empty(),
            "case {case:?}: StartSearchJob must mint a SearchJobIdentifier, got {:?}",
            started.body
        );
        let expected_arn = format!(
            "arn:aws:backup-search:{region}:{}:search-job/{identifier}",
            engine::emulator_account_id()
        );
        assert_eq!(
            started.string("SearchJobArn"),
            expected_arn,
            "case {case:?}: the ARN must name the signed region, the emulator account and the minted id, got {:?}",
            started.get("SearchJobArn")
        );
        let creation_time = started.get("CreationTime").as_f64();
        assert!(
            creation_time.is_some_and(|seconds| seconds > 1_600_000_000.0),
            "case {case:?}: CreationTime must come from the server clock, got {:?}",
            started.get("CreationTime")
        );

        let fetched = call(
            &emulator,
            region,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetSearchJob, got {} ({:?})",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("SearchJobIdentifier"),
            identifier,
            "case {case:?}: GetSearchJob must report the stored identifier, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.string("SearchJobArn"),
            expected_arn,
            "case {case:?}: GetSearchJob must report the stored ARN, got {:?}",
            fetched.get("SearchJobArn")
        );
        assert_eq!(
            fetched.get("CreationTime").as_f64(),
            creation_time,
            "case {case:?}: GetSearchJob must report the stored CreationTime, got {:?}",
            fetched.get("CreationTime")
        );
        assert_eq!(
            fetched.string("Status"),
            "RUNNING",
            "case {case:?}: a job started a moment ago is still RUNNING, got {:?}",
            fetched.get("Status")
        );
        assert_eq!(
            fetched.get("SearchScope"),
            &body["SearchScope"],
            "case {case:?}: GetSearchJob must report the submitted SearchScope, got {:?}",
            fetched.get("SearchScope")
        );
        assert_eq!(
            fetched.get("Name"),
            body.get("Name").unwrap_or(&Value::Null),
            "case {case:?}: Name must round-trip (absent stays absent), got {:?}",
            fetched.get("Name")
        );
        assert_eq!(
            fetched.get("EncryptionKeyArn"),
            body.get("EncryptionKeyArn").unwrap_or(&Value::Null),
            "case {case:?}: EncryptionKeyArn must round-trip, got {:?}",
            fetched.get("EncryptionKeyArn")
        );
        let expected_filters = body
            .get("ItemFilters")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            fetched.get("ItemFilters"),
            &expected_filters,
            "case {case:?}: ItemFilters is @required on the output and must report what was submitted, got {:?}",
            fetched.get("ItemFilters")
        );

        // The results list and the matched count come from the same record, so they
        // cannot disagree: this emulator holds no recovery points, so an empty scope
        // yields no matches.
        let results = call(
            &emulator,
            region,
            "GET",
            &format!("/search-jobs/{identifier}/search-results"),
            None,
        )
        .await;
        assert_eq!(
            results.status, 200,
            "case {case:?}: expected 200 from ListSearchJobResults, got {} ({:?})",
            results.status, results.body
        );
        let listed = results.get("Results").as_array().cloned().unwrap_or_default();
        let matched = fetched.get("CurrentSearchProgress")["ItemsMatchedCount"].as_i64();
        assert_eq!(
            Some(listed.len() as i64),
            matched,
            "case {case:?}: ListSearchJobResults must agree with ItemsMatchedCount, got {} results and {matched:?}",
            listed.len()
        );
        assert!(
            results.get("NextToken").is_null(),
            "case {case:?}: a single-page result must not carry a NextToken, got {:?}",
            results.get("NextToken")
        );

        // The tags stored at creation are the tags the resource reports.
        let tags = call(
            &emulator,
            region,
            "GET",
            &format!("/tags/{}", percent_encode(&expected_arn)),
            None,
        )
        .await;
        assert_eq!(
            tags.status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {} ({:?})",
            tags.status, tags.body
        );
        let expected_tags = body.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            tags.get("Tags"),
            &expected_tags,
            "case {case:?}: ListTagsForResource must return exactly the stored tags, got {:?}",
            tags.get("Tags")
        );
    }
}

#[tokio::test]
async fn a_started_job_appears_in_the_list_with_the_members_it_was_stored_with() {
    let emulator = emulator();
    let mut expected: Vec<(String, Value)> = Vec::new();
    for StartCase { case, region, body } in start_cases() {
        let started = call(&emulator, region, "PUT", "/search-jobs", Some(body.clone())).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200 from StartSearchJob, got {}",
            started.status
        );
        expected.push((started.string("SearchJobIdentifier"), body));
    }

    let listed = call(&emulator, "us-west-2", "GET", "/search-jobs", None).await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListSearchJobs, got {} ({:?})",
        listed.status, listed.body
    );
    let summaries = listed
        .get("SearchJobs")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        expected.len(),
        "ListSearchJobs must return one summary per persisted job, got {summaries:?}"
    );
    // The listing reports the NEWEST-created job first, so the rows come back in the
    // reverse of the order they were started in.
    for (position, (identifier, body)) in expected.iter().rev().enumerate() {
        let summary = &summaries[position];
        assert_eq!(
            summary["SearchJobIdentifier"].as_str(),
            Some(identifier.as_str()),
            "position {position}: jobs must be listed newest-created first, got {summary:?}"
        );
        assert_eq!(
            summary["Name"],
            *body.get("Name").unwrap_or(&Value::Null),
            "position {position}: the summary's Name must be the stored one, got {summary:?}"
        );
        assert_eq!(
            summary["Status"].as_str(),
            Some("RUNNING"),
            "position {position}: a job started a moment ago is RUNNING, got {summary:?}"
        );
        assert!(
            summary["CreationTime"].as_f64().is_some(),
            "position {position}: the summary must carry the stored CreationTime, got {summary:?}"
        );
        assert!(
            summary["CompletionTime"].is_null(),
            "position {position}: a running job has not completed, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn the_status_filter_keeps_only_jobs_in_that_status() {
    let emulator = emulator();
    let started = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "filtered", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    let identifier = started.string("SearchJobIdentifier");

    // Every value the model's `SearchJobState` enum declares (`SEARCH_JOB_STATE_VALUES`
    // in the service's own types): the query naming the job's CURRENT status keeps it and
    // each of the other four excludes it. The status is read back per row rather than
    // assumed to be RUNNING, so the clock settling the job mid-loop cannot make the
    // assertion wrong — what is asserted is that the filter agrees with the record.
    for status in types::SEARCH_JOB_STATE_VALUES {
        let listed = call(
            &emulator,
            "us-west-2",
            "GET",
            &format!("/search-jobs?Status={status}"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {status:?}: expected 200 from ListSearchJobs, got {} ({:?})",
            listed.status, listed.body
        );
        let identifiers: Vec<String> = listed
            .get("SearchJobs")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["SearchJobIdentifier"].as_str().unwrap_or_default().to_string())
            .collect();
        let current = call(
            &emulator,
            "us-west-2",
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await
        .string("Status");
        let expected = if *status == current {
            vec![identifier.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            identifiers, expected,
            "case {status:?}: ByStatus must keep exactly the jobs in that status (the job is {current:?}), got {identifiers:?}"
        );
    }
}

#[tokio::test]
async fn paging_returns_every_job_exactly_once_whatever_the_page_size() {
    // The invariant: for any page size, the first page opens with the newest-created
    // job, following NextToken to exhaustion yields each persisted job exactly once
    // newest-first, no page exceeds MaxResults, and a page carries a NextToken exactly
    // when it FILLED MaxResults (a full page says nothing about what follows it; a short
    // page is the end of the sequence).
    for job_count in [1_usize, 2, 3, 5] {
        for max_results in [1_usize, 2, 3, 5, 1000] {
            let emulator = emulator();
            let mut created: Vec<String> = Vec::new();
            for index in 0..job_count {
                let started = call(
                    &emulator,
                    "us-west-2",
                    "PUT",
                    "/search-jobs",
                    Some(json!({
                        "Name": format!("job-{index}"),
                        "SearchScope": {"BackupResourceTypes": ["S3"]}
                    })),
                )
                .await;
                created.push(started.string("SearchJobIdentifier"));
            }
            let newest_first: Vec<String> = created.iter().rev().cloned().collect();

            let case = format!("{job_count} jobs, MaxResults={max_results}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let uri = match &token {
                    Some(token) => format!("/search-jobs?MaxResults={max_results}&NextToken={token}"),
                    None => format!("/search-jobs?MaxResults={max_results}"),
                };
                let listed = call(&emulator, "us-west-2", "GET", &uri, None).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected 200 from ListSearchJobs, got {} ({:?})",
                    listed.status, listed.body
                );
                let page = listed
                    .get("SearchJobs")
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= max_results,
                    "case {case:?}: a page must hold at most MaxResults jobs, got {}",
                    page.len()
                );
                let page_identifiers: Vec<String> = page
                    .iter()
                    .map(|summary| {
                        summary["SearchJobIdentifier"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                if pages == 0 {
                    assert_eq!(
                        page_identifiers.first().map(String::as_str),
                        newest_first.first().map(String::as_str),
                        "case {case:?}: the first page must open with the newest job, got {page_identifiers:?}"
                    );
                }
                let full = page_identifiers.len() == max_results;
                assert_eq!(
                    listed.get("NextToken").as_str().is_some(),
                    full,
                    "case {case:?}: a page of {} must {}carry a NextToken, got {:?}",
                    page_identifiers.len(),
                    if full { "" } else { "not " },
                    listed.get("NextToken")
                );
                seen.extend(page_identifiers);
                pages += 1;
                assert!(
                    pages <= job_count + 1,
                    "case {case:?}: paging must terminate, took {pages} pages"
                );
                match listed.get("NextToken").as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                seen, newest_first,
                "case {case:?}: paging must yield every job exactly once, newest-created first, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = emulator();
    call(
        &emulator,
        "us-west-2",
        "PUT",
        "/search-jobs",
        Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;

    for token in ["not-a-cursor", "9999", "-1", "9999@jobs:ALL", "1@jobs:RUNNING"] {
        let listed = call(
            &emulator,
            "us-west-2",
            "GET",
            &format!("/search-jobs?NextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 400,
            "case {token:?}: a token this service did not issue for THIS request must be refused, got {} ({:?})",
            listed.status, listed.body
        );
        assert_eq!(
            listed.error_code.as_deref(),
            Some("ValidationException"),
            "case {token:?}: the refusal must carry ValidationException, got {:?}",
            listed.error_code
        );
    }
}

#[tokio::test]
async fn a_page_token_cannot_resume_a_differently_filtered_listing() {
    // The cursor names the sequence it indexes, so a token issued for one `ByStatus`
    // value (or for an unfiltered listing, or for another job's results) is refused
    // rather than silently resuming a different sequence at that offset.
    let emulator = emulator();
    let mut identifiers: Vec<String> = Vec::new();
    for index in 0..3 {
        let started = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/search-jobs",
            Some(json!({
                "Name": format!("job-{index}"),
                "SearchScope": {"BackupResourceTypes": ["S3"]}
            })),
        )
        .await;
        identifiers.push(started.string("SearchJobIdentifier"));
    }

    // A token from the unfiltered listing.
    let first_page = call(
        &emulator,
        "us-west-2",
        "GET",
        "/search-jobs?MaxResults=1",
        None,
    )
    .await;
    let token = first_page.string("NextToken");
    assert!(
        !token.is_empty(),
        "a first page of three jobs at MaxResults=1 must carry a NextToken, got {:?}",
        first_page.body
    );

    // It resumes the listing it was issued for...
    let resumed = call(
        &emulator,
        "us-west-2",
        "GET",
        &format!("/search-jobs?MaxResults=1&NextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case {:?}: the token must resume the listing it was issued for, got {} ({:?})",
        "same request", resumed.status, resumed.body
    );
    assert_eq!(
        resumed.get("SearchJobs")[0]["SearchJobIdentifier"].as_str(),
        Some(identifiers[1].as_str()),
        "case {:?}: the second page must start at the second job, got {:?}",
        "same request",
        resumed.body
    );

    // ...and nothing else. Each row is a request the token was NOT issued for.
    let others: Vec<(&str, String)> = vec![
        ("a status filter", format!("/search-jobs?Status=RUNNING&MaxResults=1&NextToken={token}")),
        ("another status filter", format!("/search-jobs?Status=STOPPED&MaxResults=1&NextToken={token}")),
        (
            "another job's results",
            format!(
                "/search-jobs/{}/search-results?nextToken={token}",
                identifiers[0]
            ),
        ),
    ];
    for (case, uri) in others {
        let answer = call(&emulator, "us-west-2", "GET", &uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token issued for another sequence must be refused, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal must carry ValidationException, got {:?}",
            answer.error_code
        );
    }
}

#[tokio::test]
async fn a_client_token_is_idempotent_for_the_same_body_and_conflicts_for_a_different_one() {
    for StartCase { case, region, body } in start_cases() {
        let emulator = emulator();
        let mut first_body = body.clone();
        first_body["ClientToken"] = json!("token-1");

        let first = call(&emulator, region, "PUT", "/search-jobs", Some(first_body.clone())).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200 from the first StartSearchJob, got {}",
            first.status
        );

        // The same token with the same body: the same job, and no second job.
        let repeat = call(&emulator, region, "PUT", "/search-jobs", Some(first_body.clone())).await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a repeat under the same ClientToken must succeed, got {} ({:?})",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.string("SearchJobIdentifier"),
            first.string("SearchJobIdentifier"),
            "case {case:?}: a repeat must answer with the first job's identifier, got {:?}",
            repeat.body
        );
        assert_eq!(
            repeat.string("SearchJobArn"),
            first.string("SearchJobArn"),
            "case {case:?}: a repeat must answer with the first job's ARN, got {:?}",
            repeat.body
        );
        assert_eq!(
            repeat.get("CreationTime").as_f64(),
            first.get("CreationTime").as_f64(),
            "case {case:?}: a repeat must answer with the first job's CreationTime, got {:?}",
            repeat.get("CreationTime")
        );
        let listed = call(&emulator, region, "GET", "/search-jobs", None).await;
        assert_eq!(
            listed.get("SearchJobs").as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a repeat under the same token must not create a second job, got {:?}",
            listed.body
        );

        // The same token with a different body: a conflict, in each of the ways a body
        // can differ.
        let mut variants: Vec<(&str, Value)> = Vec::new();
        let mut renamed = first_body.clone();
        renamed["Name"] = json!("something-else");
        variants.push(("a different Name", renamed));
        let mut rescoped = first_body.clone();
        // A scope no row above submits, so the difference holds for every row.
        rescoped["SearchScope"] =
            json!({"BackupResourceTypes": ["S3"], "SourceResourceArns": ["arn:aws:s3:::gamma"]});
        variants.push(("a different SearchScope", rescoped));
        let mut retagged = first_body.clone();
        retagged["Tags"] = json!({"env": "staging"});
        variants.push(("different Tags", retagged));

        for (difference, variant) in variants {
            let conflict = call(&emulator, region, "PUT", "/search-jobs", Some(variant)).await;
            assert_eq!(
                conflict.status, 409,
                "case {case:?} with {difference}: reusing a ClientToken for a different request must conflict, got {} ({:?})",
                conflict.status, conflict.body
            );
            assert_eq!(
                conflict.error_code.as_deref(),
                Some("ConflictException"),
                "case {case:?} with {difference}: the refusal must carry ConflictException, got {:?}",
                conflict.error_code
            );
        }

        // A different token is a different job.
        let mut second_token = body.clone();
        second_token["ClientToken"] = json!("token-2");
        let second = call(&emulator, region, "PUT", "/search-jobs", Some(second_token)).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: a fresh ClientToken must mint a new job, got {} ({:?})",
            second.status, second.body
        );
        assert_ne!(
            second.string("SearchJobIdentifier"),
            first.string("SearchJobIdentifier"),
            "case {case:?}: a fresh ClientToken must mint a DIFFERENT identifier"
        );
    }
}

#[tokio::test]
async fn an_identifier_no_job_exists_for_is_not_found_on_every_operation_that_takes_one() {
    let emulator = emulator();
    // Routes that resolve a search job by identifier, and the tagging routes that
    // resolve one by ARN. The last element says whether the refusal carries this
    // service's fixed not-found text: every JOB-IDENTIFIER lookup does, and none of them
    // echoes the identifier back (`state::RESOURCE_NOT_FOUND`), while the tag APIs, which
    // resolve a resource by ARN, are recorded with a refusal of their own.
    let cases: Vec<(&str, &str, &str, Option<Value>, bool)> = vec![
        (
            "GetSearchJob",
            "GET",
            "/search-jobs/does-not-exist",
            None,
            true,
        ),
        (
            "ListSearchJobResults",
            "GET",
            "/search-jobs/does-not-exist/search-results",
            None,
            true,
        ),
        (
            "StopSearchJob",
            "PUT",
            "/search-jobs/does-not-exist/actions/cancel",
            None,
            true,
        ),
        (
            "GetSearchResultExportJob",
            "GET",
            "/export-search-jobs/does-not-exist",
            None,
            true,
        ),
        (
            "StartSearchResultExportJob",
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": "does-not-exist",
                "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "b"}}
            })),
            true,
        ),
        (
            "ListTagsForResource",
            "GET",
            "/tags/arn%3Aaws%3Abackup-search%3Aus-west-2%3A000000000000%3Asearch-job%2Fdoes-not-exist",
            None,
            false,
        ),
        (
            "TagResource",
            "POST",
            "/tags/arn%3Aaws%3Abackup-search%3Aus-west-2%3A000000000000%3Asearch-job%2Fdoes-not-exist",
            Some(json!({"Tags": {"env": "test"}})),
            false,
        ),
    ];

    for (operation, method, uri, body, fixed_text) in cases {
        let answer = call(&emulator, "us-west-2", method, uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {operation:?}: an unknown resource must be 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {operation:?}: the refusal must carry ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        if fixed_text {
            let message = answer.string("message");
            // The recorded sentence, pinned literally: the constant the handlers use must
            // be THIS text, so a drift in it fails here rather than passing silently.
            assert_eq!(
                message, "Resource is not found",
                "case {operation:?}: a job-identifier lookup refuses with this service's fixed not-found text, got {message:?}"
            );
            assert_eq!(
                message,
                state::RESOURCE_NOT_FOUND,
                "case {operation:?}: every handler must take that text from one constant, got {message:?}"
            );
            assert!(
                !message.contains("does-not-exist"),
                "case {operation:?}: the fixed not-found text must not echo the requested identifier, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn stopping_a_running_job_moves_it_to_stopping_and_then_to_stopped() {
    let emulator = emulator();
    let started = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "to-stop", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    let identifier = started.string("SearchJobIdentifier");
    let job_uri = format!("/search-jobs/{identifier}");

    let stopped = call(
        &emulator,
        "us-west-2",
        "PUT",
        &format!("{job_uri}/actions/cancel"),
        None,
    )
    .await;
    assert_eq!(
        stopped.status, 200,
        "StopSearchJob on a RUNNING job must succeed, got {} ({:?})",
        stopped.status, stopped.body
    );

    // Straight after the call the stop is in flight: the transient status is observable
    // because the span the clock owes is never zero.
    tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
    let stopping = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        stopping.string("Status"),
        "STOPPING",
        "a job whose stop is in flight is STOPPING, got {:?}",
        stopping.get("Status")
    );

    // A repeat while the stop is in flight is idempotent, not a conflict.
    let repeat = call(
        &emulator,
        "us-west-2",
        "PUT",
        &format!("{job_uri}/actions/cancel"),
        None,
    )
    .await;
    assert_eq!(
        repeat.status, 200,
        "a repeated StopSearchJob while STOPPING must be idempotent, got {} ({:?})",
        repeat.status, repeat.body
    );

    // Once the clock has made it due the job is STOPPED, with a completion time, and
    // the list reports the same status.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let settled = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        settled.string("Status"),
        "STOPPED",
        "once the stop is due the job is STOPPED, got {:?}",
        settled.get("Status")
    );
    let completion = settled.get("CompletionTime").as_f64();
    assert!(
        completion.is_some_and(|seconds| seconds >= started.get("CreationTime").as_f64().unwrap_or_default()),
        "a stopped job carries a CompletionTime at or after its CreationTime, got {:?}",
        settled.get("CompletionTime")
    );

    let listed = call(&emulator, "us-west-2", "GET", "/search-jobs?Status=STOPPED", None).await;
    let identifiers: Vec<String> = listed
        .get("SearchJobs")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["SearchJobIdentifier"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        identifiers,
        vec![identifier.clone()],
        "ListSearchJobs must observe the stopped status, got {identifiers:?}"
    );

    // Stopping a job that has already STOPPED is the end state this `@idempotent`
    // operation asks for, so the repeat is an empty 200 that leaves the job stopped.
    // (A terminal status this operation does NOT produce is the conflict; that is
    // asserted per status in `search_job_outcome_and_paging.rs`.)
    let again = call(
        &emulator,
        "us-west-2",
        "PUT",
        &format!("{job_uri}/actions/cancel"),
        None,
    )
    .await;
    assert_eq!(
        again.status, 200,
        "a repeated StopSearchJob on a STOPPED job must be a no-op success, got {} ({:?})",
        again.status, again.body
    );
    assert_eq!(
        call(&emulator, "us-west-2", "GET", &job_uri, None)
            .await
            .string("Status"),
        "STOPPED",
        "the repeat must leave the job STOPPED"
    );
}

#[tokio::test]
async fn a_running_job_settles_when_the_clock_makes_it_due() {
    // This store holds no recovery points, so the scope resolves to nothing and the
    // terminal status the clock owes is the failure that says so. Which status follows
    // from which resolved scope is the table in `search_job_outcome_and_paging.rs`.
    let emulator = emulator();
    let started = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "runs-to-completion", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    let identifier = started.string("SearchJobIdentifier");
    let job_uri = format!("/search-jobs/{identifier}");

    tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
    let running = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        running.string("Status"),
        "RUNNING",
        "the span before completion is never zero, so the job is still RUNNING, got {:?}",
        running.get("Status")
    );
    assert!(
        running.get("CompletionTime").is_null(),
        "a running job has no CompletionTime, got {:?}",
        running.get("CompletionTime")
    );

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let settled = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        settled.string("Status"),
        state::FAILED,
        "a scan whose scope resolved to nothing ends FAILED, got {:?}",
        settled.get("Status")
    );
    assert_eq!(
        settled.string("StatusMessage"),
        state::SCOPE_EMPTY,
        "the failure must explain the empty scope, got {:?}",
        settled.get("StatusMessage")
    );
    assert!(
        settled.get("CompletionTime").as_f64().is_some(),
        "a settled job carries a CompletionTime, got {:?}",
        settled.get("CompletionTime")
    );
    // A scan that failed never got through its scope, so it reports no progress.
    assert_eq!(
        settled.get("CurrentSearchProgress")["RecoveryPointsScannedCount"].as_i64(),
        Some(0),
        "a failed scan has scanned no recovery point, got {:?}",
        settled.get("CurrentSearchProgress")
    );
    assert_eq!(
        settled.get("CurrentSearchProgress")["ItemsScannedCount"].as_i64(),
        Some(0),
        "a failed scan has scanned no item, got {:?}",
        settled.get("CurrentSearchProgress")
    );

    // A read did not move the status on: the completion time it reports is the instant
    // the transition was DUE, not the instant of the call, so two reads agree.
    let read_again = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        read_again.get("CompletionTime").as_f64(),
        settled.get("CompletionTime").as_f64(),
        "reading a job must not change what a later reader sees, got {:?}",
        read_again.get("CompletionTime")
    );
}

/// One row of the tagging table: the `TagResource` body to write, and the tags the
/// resource must report once it is written.
struct TagCase {
    case: &'static str,
    body: Value,
    expected: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn tags_added_after_creation_join_the_ones_stored_at_creation() {
    let emulator = emulator();
    let started = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/search-jobs",
        Some(json!({
            "Name": "tagged",
            "SearchScope": {"BackupResourceTypes": ["S3"]},
            "Tags": {"env": "test"}
        })),
    )
    .await;
    let arn = started.string("SearchJobArn");
    let tags_uri = format!("/tags/{}", percent_encode(&arn));

    // Rows differ in how the write interacts with what is stored: a new key, a second
    // new key, and a key that already has a value.
    let cases = vec![
        TagCase {
            case: "a new key",
            body: json!({"Tags": {"team": "a"}}),
            expected: vec![("env", "test"), ("team", "a")],
        },
        TagCase {
            case: "another new key",
            body: json!({"Tags": {"tier": "gold"}}),
            expected: vec![("env", "test"), ("team", "a"), ("tier", "gold")],
        },
        TagCase {
            case: "a key that already has a value",
            body: json!({"Tags": {"env": "prod"}}),
            expected: vec![("env", "prod"), ("team", "a"), ("tier", "gold")],
        },
    ];

    for TagCase {
        case,
        body,
        expected,
    } in cases
    {
        let tagged = call(&emulator, "us-west-2", "POST", &tags_uri, Some(body)).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected 200 from TagResource, got {} ({:?})",
            tagged.status, tagged.body
        );
        let listed = call(&emulator, "us-west-2", "GET", &tags_uri, None).await;
        let actual: BTreeMap<String, String> = listed
            .get("Tags")
            .as_object()
            .map(|members| {
                members
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let wanted: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            actual, wanted,
            "case {case:?}: ListTagsForResource must return exactly the stored tags, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_statuses_this_service_uses_are_the_ones_its_model_declares() {
    // Source: the `SearchJobState` enum in the service's Smithy model, rendered by the
    // scaffold as `SEARCH_JOB_STATE_VALUES`.
    let declared = types::SEARCH_JOB_STATE_VALUES;
    for status in [
        state::RUNNING,
        state::COMPLETED,
        state::STOPPING,
        state::STOPPED,
        state::FAILED,
    ] {
        assert!(
            declared.contains(&status),
            "case {status:?}: a status this service stores must be a value the model declares, got {declared:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_past_the_caps_that_bound_one_record_is_refused() {
    // The caps live in state.rs; the rows walk each one at its bound and one past it.
    let max_tags = state::MAX_TAGS_PER_JOB;
    let max_key = state::MAX_TAG_KEY_LENGTH;
    let max_value = state::MAX_TAG_VALUE_LENGTH;

    let cases: Vec<(String, Value, u16)> = vec![
        (
            format!("{max_tags} tags, at the cap"),
            json!({"Tags": tag_set(max_tags)}),
            200,
        ),
        (
            format!("{} tags, one past the cap", max_tags + 1),
            json!({"Tags": tag_set(max_tags + 1)}),
            400,
        ),
        (
            format!("a {max_key}-character key, at the cap"),
            json!({"Tags": one_tag("k".repeat(max_key), "v".to_string())}),
            200,
        ),
        (
            format!("a {}-character key, one past the cap", max_key + 1),
            json!({"Tags": one_tag("k".repeat(max_key + 1), "v".to_string())}),
            400,
        ),
        (
            format!("a {max_value}-character value, at the cap"),
            json!({"Tags": one_tag("k".to_string(), "v".repeat(max_value))}),
            200,
        ),
        (
            format!("a {}-character value, one past the cap", max_value + 1),
            json!({"Tags": one_tag("k".to_string(), "v".repeat(max_value + 1))}),
            400,
        ),
    ];

    for (case, tags, expected_status) in cases {
        // At creation.
        let at_creation = emulator();
        let mut body = json!({"SearchScope": {"BackupResourceTypes": ["S3"]}});
        body["Tags"] = tags["Tags"].clone();
        let started = call(&at_creation, "us-west-2", "PUT", "/search-jobs", Some(body)).await;
        assert_eq!(
            started.status, expected_status,
            "case {case:?} at StartSearchJob: expected {expected_status}, got {} ({:?})",
            started.status, started.body
        );
        if expected_status == 400 {
            assert_eq!(
                started.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?} at StartSearchJob: the refusal must name the constraint, got {:?}",
                started.error_code
            );
        }

        // And on the write that merges into a stored record.
        let on_merge = emulator();
        let job = call(
            &on_merge,
            "us-west-2",
            "PUT",
            "/search-jobs",
            Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
        )
        .await;
        let tags_uri = format!("/tags/{}", percent_encode(&job.string("SearchJobArn")));
        let tagged = call(&on_merge, "us-west-2", "POST", &tags_uri, Some(tags)).await;
        assert_eq!(
            tagged.status, expected_status,
            "case {case:?} at TagResource: expected {expected_status}, got {} ({:?})",
            tagged.status, tagged.body
        );
        if expected_status == 400 {
            let listed = call(&on_merge, "us-west-2", "GET", &tags_uri, None).await;
            assert_eq!(
                listed.get("Tags"),
                &json!({}),
                "case {case:?}: a refused TagResource must not have written part of the set, got {:?}",
                listed.get("Tags")
            );
        }
    }
}

#[tokio::test]
async fn starting_a_job_past_the_store_cap_is_refused_with_the_quota_error() {
    let (emulator, engine) = emulator_with_engine();

    // Seed the store to ONE short of the cap, so the next two calls walk the boundary.
    // Driving `MAX_SEARCH_JOBS` requests would only be a slower way to reach this state.
    {
        let state = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
        let mut state = state.lock();
        for index in 0..state::MAX_SEARCH_JOBS - 1 {
            let identifier = format!("seeded-{index}");
            state.insert_job(state::SearchJob {
                identifier: identifier.clone(),
                arn: state::search_job_arn("backup-search", "us-west-2", &identifier),
                name: None,
                status: state::COMPLETED.to_string(),
                creation_time: state::timestamp(std::time::SystemTime::now()),
                completion_time: None,
                status_message: None,
                encryption_key_arn: None,
                search_scope: types::SearchScope::default(),
                item_filters: types::ItemFilters::default(),
                search_scope_summary: types::SearchScopeSummary::default(),
                current_search_progress: types::CurrentSearchProgress::default(),
                results: Vec::new(),
                backups: Vec::new(),
                tags: std::collections::HashMap::new(),
                client_token: None,
                request_fingerprint: identifier,
                next_status_due: None,
            });
        }
    }

    let body = json!({"SearchScope": {"BackupResourceTypes": ["S3"]}});
    let at_the_cap = call(&emulator, "us-west-2", "PUT", "/search-jobs", Some(body.clone())).await;
    assert_eq!(
        at_the_cap.status, 200,
        "the call that fills the store to its cap must still succeed, got {} ({:?})",
        at_the_cap.status, at_the_cap.body
    );

    let past_the_cap = call(&emulator, "us-west-2", "PUT", "/search-jobs", Some(body)).await;
    assert_eq!(
        past_the_cap.status, 402,
        "past the cap StartSearchJob must refuse with the modelled quota error, got {} ({:?})",
        past_the_cap.status, past_the_cap.body
    );
    assert_eq!(
        past_the_cap.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "the refusal must carry ServiceQuotaExceededException, got {:?}",
        past_the_cap.error_code
    );
}

/// `count` distinct tag entries, for the rows that walk the per-resource tag cap.
fn tag_set(count: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        tags.insert(format!("key-{index}"), json!("value"));
    }
    Value::Object(tags)
}

/// A one-entry tag map, for the rows whose key or value is built at a cap's length.
fn one_tag(key: String, value: String) -> Value {
    let mut tags = serde_json::Map::new();
    tags.insert(key, Value::String(value));
    Value::Object(tags)
}

/// An ARN as ONE URI path segment, for the `@httpLabel`-bound `ResourceArn`.
///
/// Only `/` is escaped: RFC 3986 lets a path segment carry `:` unescaped (it is a
/// `pchar`), and `/` is the one character in a search-job ARN that would otherwise split
/// the segment. The framework percent-decodes the label before the handler sees it.
pub(crate) fn percent_encode(segment: &str) -> String {
    segment.replace('/', "%2F")
}
