// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the BackupSearch BACKUP-LISTING and EXPORT-LISTING slice and for
//! the two refusals a start reaches only once its resource checks hold: `RoleArn` and
//! `ExportSpecification`. Driven over HTTP exactly as a client drives it:
//! `ListSearchJobBackups`, `ListSearchResultExportJobs` and
//! `StartSearchResultExportJob`.
//!
//! Each test states one rule and asserts it over a TABLE of requests that differ in what
//! the rule generalises over — the identifier's form, the search job's status, the
//! partition and account of the `RoleArn`, the shape of the `ExportSpecification`, the
//! filters, the record count and the page size — never over one cited request:
//!
//! * a listing bound to a resource (`ListSearchJobBackups`) refuses an identifier that
//!   names no such resource with the modelled `ResourceNotFoundException` (404), and
//!   otherwise pages over the records that resource holds;
//! * a listing whose members are FILTERS (`ListSearchResultExportJobs`) never refuses a
//!   filter value for naming nothing: it answers 200 with the records that match, which
//!   is the empty page when none do;
//! * an export can be started only from a search job whose results are final
//!   (`state::EXPORTABLE_SEARCH_JOB_STATES`), else the modelled `ValidationException`;
//! * an `ExportSpecification` with no member set reaches the service and is answered
//!   with the modelled `InternalServerException` (500), not refused as invalid input;
//! * a `RoleArn` the model's `IamRoleArn` pattern admits is never refused as invalid
//!   input: it reaches the pass-role check, which refuses a role outside the caller's
//!   account with the modelled `AccessDeniedException` (403);
//! * a refused call creates nothing.
//!
//! The HTTP harness (`emulator`, `emulator_with_engine`, `call`, `Answer`) is the
//! search-job suite's: every suite drives the same emulator the same way.

use std::collections::HashMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{call, emulator, emulator_with_engine, Answer};

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

/// A `RoleArn` the pass-role check accepts: a role in the account this emulator
/// attributes every caller to. The `@pattern` on the model's `IamRoleArn` member is
/// `^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$`, which this satisfies.
fn passable_role(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::emulator_account_id()
    )
}

/// A `StartSearchResultExportJob` body naming `identifier`, with an S3 destination and
/// an optional `RoleArn`.
fn export_body(identifier: &str, role_arn: Option<&str>) -> Value {
    let mut body = json!({
        "SearchJobIdentifier": identifier,
        "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "exports"}}
    });
    if let Some(role_arn) = role_arn {
        body["RoleArn"] = json!(role_arn);
    }
    body
}

/// Start a search job and return its identifier. The scope is the one the search-job
/// suite uses, because what this suite varies is the listing, not the scan.
async fn started_job(emulator: &MultiServiceRouter) -> String {
    let started = call(
        emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchJob must succeed to set up a listing case, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobIdentifier")
}

/// Seed one search job in `status`, with `backups` recovery points covered, straight
/// into the store: `FAILED` is a status no sequence of calls reaches in this emulator,
/// and seeding keeps every row of a status table deterministic and sleep-free.
fn seed_job(
    engine: &engine::InMemoryEngine,
    identifier: &str,
    status: &str,
    backups: Vec<types::SearchJobBackupsResult>,
) {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    store.insert_job(state::SearchJob {
        identifier: identifier.to_string(),
        arn: state::search_job_arn("backup-search", REGION, identifier),
        name: None,
        status: status.to_string(),
        creation_time: state::timestamp(std::time::SystemTime::now()),
        completion_time: None,
        status_message: None,
        encryption_key_arn: None,
        search_scope: types::SearchScope::default(),
        item_filters: types::ItemFilters::default(),
        search_scope_summary: types::SearchScopeSummary::default(),
        current_search_progress: types::CurrentSearchProgress::default(),
        results: Vec::new(),
        backups,
        tags: HashMap::new(),
        client_token: None,
        request_fingerprint: identifier.to_string(),
        next_status_due: None,
    });
}

/// One covered recovery point, with the members `SearchJobBackupsResult` declares.
fn backup(index: usize, status: &str, resource_type: &str) -> types::SearchJobBackupsResult {
    types::SearchJobBackupsResult {
        status: Some(status.to_string()),
        status_message: None,
        resource_type: Some(resource_type.to_string()),
        backup_resource_arn: Some(format!(
            "arn:aws:backup:{REGION}:{}:recovery-point:rp-{index}",
            engine::emulator_account_id()
        )),
        source_resource_arn: Some(format!("arn:aws:s3:::bucket-{index}")),
        index_creation_time: None,
        backup_creation_time: None,
    }
}

/// Every export job currently listed, with the filters `query` carries.
async fn listed_exports(emulator: &MultiServiceRouter, query: &str) -> Answer {
    call(
        emulator,
        REGION,
        "GET",
        &format!("/export-search-jobs{query}"),
        None,
    )
    .await
}

#[tokio::test]
async fn listing_the_backups_of_an_unknown_search_job_is_the_modelled_not_found() {
    // The rule: a listing bound to a resource refuses an identifier that names no such
    // resource, whatever that identifier looks like. The rows differ in form — a UUID,
    // a minted-shaped id, a single character, a long one — because the refusal is about
    // what the store holds and not about the shape of the string.
    let emulator = emulator();
    let unknown = [
        "48434c42-40a0-4057-b04d-3b484c610e2b",
        "searchjob-000000000042",
        "x",
        "a-very-long-identifier-that-no-call-ever-minted-0000000000000000",
    ];
    for identifier in unknown {
        let answer = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/backups"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {identifier:?}: expected 404 from ListSearchJobBackups, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {identifier:?}: the refusal must be the modelled ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        assert_eq!(
            answer.string("message"),
            state::RESOURCE_NOT_FOUND,
            "case {identifier:?}: this service answers every job-identifier miss with its fixed not-found text, got {:?}",
            answer.get("message")
        );
        assert_eq!(
            answer.string("resourceId"),
            identifier,
            "case {identifier:?}: ResourceNotFoundException names the resource that was not found, got {:?}",
            answer.body
        );
    }

    // The positive control, in the same emulator: a job that exists answers 200.
    let identifier = started_job(&emulator).await;
    let answer = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}/backups"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {identifier:?}: a search job that exists must answer 200, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.get("Results"),
        &json!([]),
        "case {identifier:?}: this emulator holds no recovery points, so the job covered none, got {:?}",
        answer.get("Results")
    );
}

#[tokio::test]
async fn backups_are_paged_out_of_the_job_record_exactly_once_whatever_the_page_size() {
    // The invariant: for any page size, following NextToken to exhaustion yields each
    // covered recovery point exactly once, in the order the record holds them, and no
    // page exceeds MaxResults. The rows walk record counts against page sizes, with
    // `MaxResults` at the published upper bound of its @range(min: 1, max: 1000).
    for backup_count in [0_usize, 1, 3, 7] {
        for max_results in [1_usize, 2, 3, 1000] {
            let (emulator, engine) = emulator_with_engine();
            let identifier = format!("seeded-{backup_count}");
            // `ResourceType` accepts S3 and EBS and `SearchJobState` COMPLETED/FAILED
            // among others (model enums `ResourceType` / `SearchJobState`); the rows mix
            // them so the page carries more than one shape of record.
            let seeded: Vec<types::SearchJobBackupsResult> = (0..backup_count)
                .map(|index| {
                    let status = if index % 2 == 0 {
                        state::COMPLETED
                    } else {
                        state::FAILED
                    };
                    let resource_type = if index % 3 == 0 { "S3" } else { "EBS" };
                    backup(index, status, resource_type)
                })
                .collect();
            seed_job(&engine, &identifier, state::COMPLETED, seeded.clone());

            let case = format!("{backup_count} backups, MaxResults={max_results}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let uri = match &token {
                    Some(token) => format!(
                        "/search-jobs/{identifier}/backups?maxResults={max_results}&nextToken={token}"
                    ),
                    None => format!("/search-jobs/{identifier}/backups?maxResults={max_results}"),
                };
                let listed = call(&emulator, REGION, "GET", &uri, None).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected 200 from ListSearchJobBackups, got {} ({:?})",
                    listed.status, listed.body
                );
                let page = listed.get("Results").as_array().cloned().unwrap_or_default();
                assert!(
                    page.len() <= max_results,
                    "case {case:?}: a page must hold at most MaxResults backups, got {}",
                    page.len()
                );
                seen.extend(
                    page.iter()
                        .map(|entry| entry["BackupResourceArn"].as_str().unwrap_or_default().to_string()),
                );
                token = listed.get("NextToken").as_str().map(ToString::to_string);
                if token.is_none() {
                    break;
                }
            }

            let expected: Vec<String> = seeded
                .iter()
                .map(|entry| entry.backup_resource_arn.clone().unwrap_or_default())
                .collect();
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield every stored backup exactly once in order, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn export_job_listing_treats_its_members_as_filters_and_never_answers_not_found() {
    // The rule: `SearchJobIdentifier` and `Status` narrow the listing; neither is looked
    // up as a resource, so a value naming nothing yields an empty page with no error —
    // the same answer as filtering by a real search job that has no export jobs.
    let (emulator, engine) = emulator_with_engine();

    // A real search job with no export jobs, the reference answer every row is
    // compared against.
    seed_job(&engine, "has-no-exports", state::COMPLETED, Vec::new());

    let rows: Vec<(&str, String)> = vec![
        ("no filter at all", String::new()),
        (
            "an identifier that names no search job",
            "?SearchJobIdentifier=48434c42-40a0-4057-b04d-3b484c610e2b".to_string(),
        ),
        (
            "a minted-shaped identifier that names no search job",
            "?SearchJobIdentifier=searchjob-000000000099".to_string(),
        ),
        (
            "a real search job with no export jobs",
            "?SearchJobIdentifier=has-no-exports".to_string(),
        ),
        // `ExportJobStatus` accepts COMPLETED, FAILED and RUNNING (model enum
        // `ExportJobStatus`, rendered as `types::EXPORT_JOB_STATUS_VALUES`).
        ("a status filter", "?Status=FAILED".to_string()),
        (
            "both filters, neither matching anything",
            "?Status=RUNNING&SearchJobIdentifier=searchjob-000000000098".to_string(),
        ),
    ];
    for (case, query) in rows {
        let listed = listed_exports(&emulator, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a filtered listing must answer 200, got {} ({:?})",
            listed.status, listed.body
        );
        assert_eq!(
            listed.error_code, None,
            "case {case:?}: a filter value that names nothing is not an error, got {:?}",
            listed.error_code
        );
        assert_eq!(
            listed.get("ExportJobs"),
            &json!([]),
            "case {case:?}: nothing matches the filter, so the page is empty, got {:?}",
            listed.get("ExportJobs")
        );
        assert!(
            listed.get("NextToken").is_null(),
            "case {case:?}: an empty page must not carry a NextToken, got {:?}",
            listed.get("NextToken")
        );
    }

    // And the filter really filters: an export job on one search job is listed for that
    // identifier and for no other, and only under its own status.
    seed_job(&engine, "has-one-export", state::STOPPED, Vec::new());
    let started = call(
        &emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(export_body("has-one-export", Some(&passable_role("export")))),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchResultExportJob must succeed for a STOPPED job, got {} ({:?})",
        started.status, started.body
    );
    let export_identifier = started.string("ExportJobIdentifier");
    let export_arn = started.string("ExportJobArn");

    let matching: Vec<(&str, String, usize)> = vec![
        ("no filter", String::new(), 1),
        (
            "its own search job",
            "?SearchJobIdentifier=has-one-export".to_string(),
            1,
        ),
        (
            "another search job",
            "?SearchJobIdentifier=has-no-exports".to_string(),
            0,
        ),
        ("its own status", "?Status=RUNNING".to_string(), 1),
        ("another status", "?Status=COMPLETED".to_string(), 0),
        (
            "its own search job and another status",
            "?SearchJobIdentifier=has-one-export&Status=FAILED".to_string(),
            0,
        ),
    ];
    for (case, query, expected) in matching {
        let listed = listed_exports(&emulator, &query).await;
        let page = listed
            .get("ExportJobs")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            page.len(),
            expected,
            "case {case:?}: expected {expected} export job(s) through filter {query:?}, got {:?}",
            listed.body
        );
        for entry in &page {
            // The summary is read from the same record the start answered with, so the
            // two cannot disagree.
            assert_eq!(
                entry["ExportJobIdentifier"].as_str(),
                Some(export_identifier.as_str()),
                "case {case:?}: the listed export job must be the stored one, got {entry:?}"
            );
            assert_eq!(
                entry["ExportJobArn"].as_str(),
                Some(export_arn.as_str()),
                "case {case:?}: the summary must carry the ARN the start answered with, got {entry:?}"
            );
            assert_eq!(
                entry["SearchJobArn"].as_str(),
                Some(state::search_job_arn("backup-search", REGION, "has-one-export").as_str()),
                "case {case:?}: the summary must name the search job's own ARN, got {entry:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_export_can_only_be_started_from_a_search_job_whose_results_are_final() {
    // The rule: the source job's status decides. Every `SearchJobState` value the model
    // declares is a row here, so the gate is asserted over the whole status set rather
    // than over the one status the issue cited.
    let permitted = state::EXPORTABLE_SEARCH_JOB_STATES;
    for status in types::SEARCH_JOB_STATE_VALUES {
        let (emulator, engine) = emulator_with_engine();
        let identifier = format!("job-in-{status}");
        seed_job(&engine, &identifier, status, Vec::new());

        let answer = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(export_body(&identifier, Some(&passable_role("export")))),
        )
        .await;

        if permitted.contains(status) {
            assert_eq!(
                answer.status, 200,
                "case {status:?}: a job whose results are final can be exported, got {} ({:?})",
                answer.status, answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {status:?}: expected 400 from StartSearchResultExportJob, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {status:?}: the refusal must be ValidationException, got {:?}",
            answer.error_code
        );
        let message = answer.string("message");
        assert!(
            message.contains(status),
            "case {status:?}: the message must name the job's current status, got {message:?}"
        );
        for allowed in permitted {
            assert!(
                message.contains(allowed),
                "case {status:?}: the message must name the statuses export permits, got {message:?}"
            );
        }

        // A refused call creates nothing.
        let listed = listed_exports(&emulator, "").await;
        assert_eq!(
            listed.get("ExportJobs"),
            &json!([]),
            "case {status:?}: a refused export must create nothing, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn an_export_specification_with_no_member_is_a_server_fault_and_creates_nothing() {
    // The rule: an `ExportSpecification` carrying no member is not refused at the input
    // boundary — it reaches the service and is answered with the modelled
    // `InternalServerException` (500). The rows put that case beside the malformed
    // specifications that ARE input refusals, so the two classes stay apart.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "the union with no member set",
            json!({}),
            500,
            "InternalServerException",
        ),
        (
            "the union's member present but empty (DestinationBucket is @required)",
            json!({"s3ExportSpecification": {}}),
            400,
            "ValidationException",
        ),
        (
            "a well-formed union",
            json!({"s3ExportSpecification": {"DestinationBucket": "exports"}}),
            200,
            "",
        ),
        (
            "a well-formed union with the optional prefix",
            json!({"s3ExportSpecification": {
                "DestinationBucket": "exports",
                "DestinationPrefix": "searches/2026/"
            }}),
            200,
            "",
        ),
    ];
    for (case, specification, expected_status, expected_code) in rows {
        let (emulator, engine) = emulator_with_engine();
        seed_job(&engine, "exportable", state::COMPLETED, Vec::new());

        let answer = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": "exportable",
                "ExportSpecification": specification,
                "RoleArn": passable_role("export")
            })),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} ({:?})",
            answer.status, answer.body
        );
        if !expected_code.is_empty() {
            assert_eq!(
                answer.error_code.as_deref(),
                Some(expected_code),
                "case {case:?}: expected the {expected_code} class, got {:?}",
                answer.error_code
            );
        }

        let listed = listed_exports(&emulator, "").await;
        let created = listed
            .get("ExportJobs")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected_created = usize::from(expected_status == 200);
        assert_eq!(
            created.len(),
            expected_created,
            "case {case:?}: expected {expected_created} export job(s) to exist afterwards, got {:?}",
            listed.body
        );
    }

    // An absent `ExportSpecification` is a different refusal again: the member is
    // `@required`, so the request never reaches the service.
    let (emulator, engine) = emulator_with_engine();
    seed_job(&engine, "exportable", state::COMPLETED, Vec::new());
    let answer = call(
        &emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(json!({"SearchJobIdentifier": "exportable", "RoleArn": passable_role("export")})),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"ExportSpecification absent\": a @required member must be refused 400, got {} ({:?})",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_well_formed_role_arn_reaches_the_pass_role_check_and_is_denied_across_accounts() {
    // The rule: a `RoleArn` the model's `IamRoleArn` pattern admits is never an input
    // refusal. It reaches the pass-role check, which accepts a role in the account this
    // emulator attributes the caller to and refuses every other with the modelled
    // `AccessDeniedException` (403). The rows differ in partition, account and role
    // name/path, all inside the model's
    // `^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$`.
    let account = engine::emulator_account_id();
    let rows: Vec<(&str, String, u16)> = vec![
        ("the caller's account", passable_role("backup-search"), 200),
        (
            "the caller's account, a role under a path",
            passable_role("service-role/backup-search-export"),
            200,
        ),
        (
            "the caller's account in another partition",
            format!("arn:aws-us-gov:iam::{account}:role/backup-search"),
            200,
        ),
        (
            "another account, a role that exists nowhere",
            "arn:aws:iam::123456789012:role/does-not-exist".to_string(),
            403,
        ),
        (
            "another account, a plausible role name",
            "arn:aws:iam::210987654321:role/AWSBackupSearchExportRole".to_string(),
            403,
        ),
        (
            "another account in the china partition",
            "arn:aws-cn:iam::123456789012:role/backup-search".to_string(),
            403,
        ),
        (
            // `[a-z0-9-]+` is what the pattern allows in the account position, so a
            // non-numeric account is a well-formed ARN too — and still not the
            // caller's account.
            "a non-numeric account the pattern admits",
            "arn:aws:iam::aws-reserved:role/backup-search".to_string(),
            403,
        ),
    ];
    for (case, role_arn, expected_status) in rows {
        let (emulator, engine) = emulator_with_engine();
        seed_job(&engine, "exportable", state::STOPPED, Vec::new());

        let answer = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(export_body("exportable", Some(&role_arn))),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status} for RoleArn {role_arn:?}, got {} ({:?})",
            answer.status, answer.body
        );
        if expected_status == 403 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("AccessDeniedException"),
                "case {case:?}: a pass-role refusal is an authorization failure, got {:?}",
                answer.error_code
            );
            assert_eq!(
                answer.string("message"),
                state::CROSS_ACCOUNT_PASS_ROLE,
                "case {case:?}: AccessDeniedException$message is @required, got {:?}",
                answer.body
            );
        }

        let listed = listed_exports(&emulator, "").await;
        let created = listed
            .get("ExportJobs")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected_created = usize::from(expected_status == 200);
        assert_eq!(
            created.len(),
            expected_created,
            "case {case:?}: expected {expected_created} export job(s) afterwards, got {:?}",
            listed.body
        );
    }

    // A request with no `RoleArn` needs no role passed, so it is accepted: the member is
    // optional in the model.
    let (emulator, engine) = emulator_with_engine();
    seed_job(&engine, "exportable", state::STOPPED, Vec::new());
    let answer = call(
        &emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(export_body("exportable", None)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"RoleArn absent\": an optional member left out must not be refused, got {} ({:?})",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn an_export_page_token_cannot_resume_a_differently_filtered_listing() {
    // The cursor names the sequence it indexes, so a token issued for one set of filters
    // is refused rather than silently resuming another sequence at that offset.
    let (emulator, engine) = emulator_with_engine();
    seed_job(&engine, "exportable", state::COMPLETED, Vec::new());
    for index in 0..3 {
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": "exportable",
                "ExportSpecification": {"s3ExportSpecification": {
                    "DestinationBucket": format!("exports-{index}")
                }},
                "RoleArn": passable_role("export")
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "export {index} must start, got {} ({:?})",
            started.status, started.body
        );
    }

    let first = listed_exports(&emulator, "?MaxResults=1").await;
    let token = first.string("NextToken");
    assert!(
        !token.is_empty(),
        "a first page of one of three must carry a NextToken, got {:?}",
        first.body
    );

    // It resumes the listing it was issued for...
    let resumed = listed_exports(&emulator, &format!("?MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        resumed.status, 200,
        "the token must resume the listing it was issued for, got {} ({:?})",
        resumed.status, resumed.body
    );

    // ...and nothing else.
    let others: Vec<(&str, String)> = vec![
        (
            "a status filter",
            format!("?Status=RUNNING&MaxResults=1&NextToken={token}"),
        ),
        (
            "a search-job filter",
            format!("?SearchJobIdentifier=exportable&MaxResults=1&NextToken={token}"),
        ),
    ];
    for (case, query) in others {
        let answer = listed_exports(&emulator, &query).await;
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
