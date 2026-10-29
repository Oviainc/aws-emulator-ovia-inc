// SPDX-License-Identifier: CC0-1.0

//! `ListBackupJobs`: the backup jobs this account holds, reported from the
//! stored jobs and driven as real HTTP through the router.
//!
//! The rules under test:
//!
//! * a paginated list reports the STORED resources of its kind, in one total
//!   order, narrowed by every `By*` filter the model declares — a filter the
//!   stored resource cannot match selects nothing rather than everything;
//! * a page is at most `MaxResults` items plus a cursor that resumes exactly
//!   where the page stopped, and a page size or cursor the service could not
//!   have vended is refused with its modelled exception;
//! * a listed job and a described one answer from the same stored job, so the
//!   transition the clock owes shows in both and neither invents a member;
//! * the listing reaches back only as far as the operation documents, while a
//!   read naming the job still reports it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_backup_cryocontrollerusermanager::ComAmazonawsBackupCryocontrollerusermanagerState as BackupState;
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{
    call, create_vault, error_code, router, start_job, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// Seconds in a day, for the day offsets the listing-window cases backdate by.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// A 12-digit account id that is not the one this emulator presents
/// ([`ACCOUNT`]), so `ByAccountId` names an account it holds no jobs of. It
/// satisfies the member's `^[0-9]{12}$` pattern, so the request reaches the
/// handler.
const OTHER_ACCOUNT: &str = "999988887777";

/// A router plus the engine behind it, so a case can age a stored job the way
/// only the clock otherwise could. The shared [`router`] helper owns its engine,
/// which is all every other case needs.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Start one job on `resource` in `vault` and return its `BackupJobId`.
async fn start_job_on(router: &MultiServiceRouter, vault: &str, resource: &str) -> String {
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let body =
        format!(r#"{{"BackupVaultName":"{vault}","ResourceArn":"{resource}","IamRoleArn":"{role}"}}"#);
    let (status, started) = start_job(router, &body).await;
    assert_eq!(
        status, 200,
        "setup {resource:?}: expected StartBackupJob 200, got {status} {started:?}"
    );
    string_at(&started, "BackupJobId")
}

/// The `BackupJobId`s of one `ListBackupJobs` page, with the cursor it vended.
async fn list_jobs(
    router: &MultiServiceRouter,
    query: &str,
) -> (u16, Vec<String>, Option<String>, Value) {
    let (status, body) = call(router, "GET", &format!("/backup-jobs{query}"), None).await;
    let ids = body["BackupJobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| string_at(job, "BackupJobId"))
        .collect();
    let token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (status, ids, token, body)
}

/// Every `By*` filter the model declares, over a set of stored jobs that differ
/// in vault, protected resource and derived resource type.
///
/// Each row names the filter and the jobs it must select; a filter whose value
/// no stored job carries selects none, and one the model documents as standing
/// for every value of its kind selects all.
#[tokio::test]
async fn every_modelled_filter_narrows_the_listing_to_the_stored_jobs_it_names() {
    let router = router();
    for vault in ["list-a", "list-b", "list-empty"] {
        let (status, _) = create_vault(&router, vault, "{}").await;
        assert_eq!(status, 200, "setup: CreateBackupVault {vault} should succeed");
    }

    // (vault, protected resource, the ResourceType derived from its ARN)
    let started = [
        (
            "list-a",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/alpha"),
            "DynamoDB",
        ),
        (
            "list-a",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-beta"),
            "EBS",
        ),
        ("list-b", "arn:aws:s3:::gamma-bucket".to_string(), "S3"),
    ];
    let mut ids = Vec::new();
    for (vault, resource, _) in &started {
        ids.push(start_job_on(&router, vault, resource).await);
    }
    // Past the settle span, so the completion filters have an instant to match.
    std::thread::sleep(PAST_SETTLE);

    let all: Vec<String> = ids.clone();
    let none: Vec<String> = Vec::new();
    let mut cases: Vec<(String, String, Vec<String>)> = vec![
        ("no filter".to_string(), String::new(), all.clone()),
        (
            "ByBackupVaultName selects the jobs of that vault".to_string(),
            "?backupVaultName=list-a".to_string(),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "ByBackupVaultName of the other vault".to_string(),
            "?backupVaultName=list-b".to_string(),
            vec![ids[2].clone()],
        ),
        (
            "ByBackupVaultName of a vault holding no job".to_string(),
            "?backupVaultName=list-empty".to_string(),
            none.clone(),
        ),
        (
            "ByState of the state every settled job is in".to_string(),
            "?state=COMPLETED".to_string(),
            all.clone(),
        ),
        (
            "ByState no stored job is in".to_string(),
            "?state=RUNNING".to_string(),
            none.clone(),
        ),
        (
            "ByAccountId of the account that holds the jobs".to_string(),
            format!("?accountId={ACCOUNT}"),
            all.clone(),
        ),
        (
            "ByAccountId of another account".to_string(),
            format!("?accountId={OTHER_ACCOUNT}"),
            none.clone(),
        ),
        (
            "ByParentJobId: no stored job has a parent".to_string(),
            format!("?parentJobId={}", ids[0]),
            none.clone(),
        ),
        (
            "ByMessageCategory no stored job carries".to_string(),
            "?messageCategory=SUCCESS".to_string(),
            none.clone(),
        ),
        (
            "ByMessageCategory wildcard, which names every category".to_string(),
            "?messageCategory=*".to_string(),
            all.clone(),
        ),
        (
            "ByMessageCategory AGGREGATE_ALL, which names every category".to_string(),
            "?messageCategory=AGGREGATE_ALL".to_string(),
            all.clone(),
        ),
        (
            "ByCreatedAfter an instant before every job".to_string(),
            "?createdAfter=1970-01-01T00:00:00Z".to_string(),
            all.clone(),
        ),
        (
            "ByCreatedBefore an instant before every job".to_string(),
            "?createdBefore=1970-01-01T00:00:00Z".to_string(),
            none.clone(),
        ),
        (
            "ByCreatedBefore an instant after every job".to_string(),
            "?createdBefore=2970-01-01T00:00:00Z".to_string(),
            all.clone(),
        ),
        (
            "ByCompleteAfter an instant before every completion".to_string(),
            "?completeAfter=1970-01-01T00:00:00Z".to_string(),
            all.clone(),
        ),
        (
            "ByCompleteBefore an instant before every completion".to_string(),
            "?completeBefore=1970-01-01T00:00:00Z".to_string(),
            none.clone(),
        ),
        (
            "two filters that agree".to_string(),
            "?backupVaultName=list-a&resourceType=EBS".to_string(),
            vec![ids[1].clone()],
        ),
        (
            "two filters that disagree".to_string(),
            "?backupVaultName=list-b&resourceType=EBS".to_string(),
            none.clone(),
        ),
    ];
    // One row per stored job for the two filters that address the protected
    // resource, plus a resource type no stored job was derived from.
    for (index, (_, resource, resource_type)) in started.iter().enumerate() {
        cases.push((
            format!("ByResourceArn {resource}"),
            format!("?resourceArn={resource}"),
            vec![ids[index].clone()],
        ));
        cases.push((
            format!("ByResourceType {resource_type}"),
            format!("?resourceType={resource_type}"),
            vec![ids[index].clone()],
        ));
    }
    cases.push((
        "ByResourceType no stored job was derived from".to_string(),
        "?resourceType=Redshift".to_string(),
        none.clone(),
    ));
    cases.push((
        "ByResourceArn of a resource with no backup job".to_string(),
        format!("?resourceArn=arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/never-backed-up"),
        none,
    ));

    for (case, query, expected) in cases {
        let (status, listed, _, body) = list_jobs(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

/// A listed job carries the stored job, member for member the same as the
/// described one, and the `CREATED` -> `COMPLETED` transition the clock owes
/// shows in the listing exactly as it does in the describe.
#[tokio::test]
async fn a_listed_job_is_the_stored_job_and_settles_with_it() {
    let router = router();
    let (status, _) = create_vault(&router, "agree-vault", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/agree");
    let job_id = start_job_on(&router, "agree-vault", &resource).await;

    // Straight after the call, before the settle span: the listing reports the
    // transient state and no completion instant.
    let (status, listed, _, body) = list_jobs(&router, "?state=CREATED").await;
    assert_eq!(
        status, 200,
        "case \"before the settle span\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        listed,
        vec![job_id.clone()],
        "case \"before the settle span\": expected the started job, got {body:?}"
    );
    assert!(
        body["BackupJobs"][0].get("CompletionDate").is_none(),
        "case \"before the settle span\": an unsettled job has no CompletionDate, got {body:?}"
    );

    std::thread::sleep(PAST_SETTLE);

    let (status, listed, _, body) = list_jobs(&router, "").await;
    assert_eq!(
        status, 200,
        "case \"after the settle span\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        listed,
        vec![job_id.clone()],
        "case \"after the settle span\": expected the started job, got {body:?}"
    );
    let job = &body["BackupJobs"][0];
    let (status, described) = call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBackupJob 200, got {status} {described:?}"
    );
    // Every member the two reads share comes from the one stored job.
    for member in [
        "AccountId",
        "BackupJobId",
        "BackupVaultName",
        "BackupVaultArn",
        "RecoveryPointArn",
        "ResourceArn",
        "ResourceType",
        "IamRoleArn",
        "State",
        "PercentDone",
        "CreationDate",
        "CompletionDate",
        "ExpectedCompletionDate",
        "BackupSizeInBytes",
        "BytesTransferred",
        "IsEncrypted",
        "IsParent",
    ] {
        assert_eq!(
            job.get(member),
            described.get(member),
            "case {member:?}: the listed and the described job must agree, got {job:?} and {described:?}"
        );
    }
    assert_eq!(
        job.get("State").and_then(Value::as_str),
        Some("COMPLETED"),
        "case \"after the settle span\": expected State COMPLETED, got {job:?}"
    );
    assert_eq!(
        string_at(job, "ResourceArn"),
        resource,
        "the listed job must carry the stored resource ARN, got {job:?}"
    );
    // A member nothing in this slice records is absent, not invented.
    for member in ["ParentJobId", "CreatedBy", "MessageCategory", "BackupType"] {
        assert!(
            job.get(member).is_none(),
            "case {member:?}: nothing records it, so it must be absent, got {job:?}"
        );
    }
}

/// The listing pages by `MaxResults` with a cursor that resumes where the page
/// stopped: every page size yields every stored job exactly once, in one order.
#[tokio::test]
async fn the_listing_pages_by_max_results_with_a_round_tripping_cursor() {
    let router = router();
    let (status, _) = create_vault(&router, "page-jobs", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let mut ids = Vec::new();
    for resource in [
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/first"),
        format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-second"),
        "arn:aws:s3:::third-bucket".to_string(),
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:fourth"),
    ] {
        ids.push(start_job_on(&router, "page-jobs", &resource).await);
    }

    for page_size in [1_usize, 2, 3, 4, 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("?maxResults={page_size}");
            if let Some(cursor) = &token {
                query.push_str(&format!("&nextToken={cursor}"));
            }
            let (status, page, next, body) = list_jobs(&router, &query).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: expected 200, got {status} {body:?}"
            );
            assert!(
                page.len() <= page_size,
                "case page_size={page_size}: a page must not exceed MaxResults, got {page:?}"
            );
            seen.extend(page);
            token = next;
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case page_size={page_size}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, ids,
            "case page_size={page_size}: expected every stored job once, in order, got {seen:?}"
        );
        let expected_pages = ids.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page_size={page_size}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

/// A page size outside the model's `@range(min: 1, max: 1000)`, a cursor this
/// service did not vend and a `ByResourceArn` that is not an ARN are each
/// refused with the modelled `InvalidParameterValueException`; each published
/// bound of the range is accepted.
#[tokio::test]
async fn a_request_the_listing_could_not_serve_is_refused_by_its_modelled_code() {
    let router = router();
    let (status, _) = create_vault(&router, "refusal-jobs", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/refusal");
    start_job_on(&router, "refusal-jobs", &resource).await;

    // The published ends of the `MaxResults` range, and one step past each.
    for (case, query, expected) in [
        ("MaxResults at the published minimum", "?maxResults=1", 200),
        ("MaxResults at the published maximum", "?maxResults=1000", 200),
        ("MaxResults below the published minimum", "?maxResults=0", 400),
        (
            "MaxResults above the published maximum",
            "?maxResults=1001",
            400,
        ),
        ("a cursor that is not hex", "?nextToken=zzzz", 400),
        (
            "hex that decodes to no stored job id",
            "?nextToken=6e6f7065",
            400,
        ),
        ("ByResourceArn that is not an ARN", "?resourceArn=demo", 400),
        (
            "ByResourceArn missing the resource field",
            "?resourceArn=arn:aws:dynamodb:us-west-2:000000000000:",
            400,
        ),
    ] {
        let (status, _, _, body) = list_jobs(&router, query).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the modelled refusal, got {body:?}"
            );
        }
    }
}

/// The listing reaches back exactly as far as the operation documents (the last
/// 30 days); a job older than that is still stored, and a read naming it still
/// reports it.
///
/// The jobs are aged in the store, because only the clock could otherwise put
/// one outside the window.
#[tokio::test]
async fn a_job_older_than_the_documented_window_is_not_listed_but_is_still_stored() {
    // (case, how many days back the job was created, is it listed?)
    let cases: [(&str, u64, bool); 4] = [
        ("created just now", 0, true),
        ("created well inside the window", 15, true),
        ("created just inside the window", 29, true),
        ("created outside the window", 31, false),
    ];
    for (case, days_back, listed) in cases {
        let (router, engine) = router_with_engine();
        let (status, _) = create_vault(&router, "window-jobs", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed"
        );
        let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/window");
        let job_id = start_job_on(&router, "window-jobs", &resource).await;

        let backdated = SystemTime::now()
            .checked_sub(Duration::from_secs(days_back * SECONDS_PER_DAY))
            .expect("an instant that many days back");
        engine
            .state::<BackupState>()
            .lock()
            .backup_jobs
            .get_mut(&job_id)
            .expect("the started job is stored")
            .creation_date = backdated;

        let (status, ids, _, body) = list_jobs(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let expected = if listed {
            vec![job_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected,
            "case {case:?} ({days_back} day(s) back): expected {expected:?}, got {ids:?}"
        );

        // Whether or not the listing reaches it, the job is still stored.
        let (status, described) =
            call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            (status, string_at(&described, "BackupJobId")),
            (200, job_id.clone()),
            "case {case:?}: a read naming the job must still report it, got {status} {described:?}"
        );
    }
}
