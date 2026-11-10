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

/// A margin on top of a span, so a sleep that is meant to carry the clock PAST a due
/// instant is not racing it on a loaded machine.
pub(crate) const PAST_THE_SPAN: Duration = Duration::from_millis(150);

/// Longer than the whole schedule a new search job is owed — its set-up span plus the
/// span its scan runs for (`state::CREATING_DELAY` then `state::SCAN_DELAY`, each counted
/// from the phase before it) — so the clock has made every owed transition due by the time
/// the next call is made. Derived from those spans rather than restated, so it cannot
/// drift from them.
pub(crate) const PAST_THE_DUE_INSTANT: Duration = state::CREATING_DELAY
    .saturating_add(state::SCAN_DELAY)
    .saturating_add(PAST_THE_SPAN);

/// Longer than `crate::state::CREATING_DELAY` but shorter than the instant the terminal
/// status is owed from, so a search job has left `CREATING` for the `RUNNING` phase of
/// its scan and not yet reached its terminal status.
pub(crate) const PAST_THE_CREATING_INSTANT: Duration =
    state::CREATING_DELAY.saturating_add(PAST_THE_SPAN);

/// Shorter than either span, to pin that neither is zero: a caller acting straight after
/// `StartSearchJob` still observes the transient `CREATING` the job is created in.
pub(crate) const WITHIN_THE_DUE_INSTANT: Duration = Duration::from_millis(50);

/// One emulator holding one service over one shared engine, as the router builds it,
/// with that engine so a test can seed the store directly where driving thousands of
/// requests would only be a slow way to reach the same state.
///
/// KMS is mounted beside this service, on the SAME engine the router gives both, because
/// `StartSearchJob`'s `EncryptionKeyArn` is an `@arnReference` to a KMS key and is
/// resolved in KMS's own store: a row that starts a job under a key creates that key here
/// first ([`create_kms_key`]).
///
/// Amazon S3 is mounted for the same reason: `StartSearchResultExportJob` WRITES the
/// exported results into the destination bucket the request names, and that bucket lives in
/// the object store's own records — so an export of this emulator reaches the object store
/// mounted here, and a suite that expects an export to complete creates its destination
/// bucket first ([`create_bucket`]).
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// An Amazon S3 client over this emulator's own in-process callback — the very seam an
/// export writes its results object through, so a bucket created here is the bucket that
/// write lands in and an object listed here is the object it wrote.
pub(crate) fn s3(emulator: &MultiServiceRouter, region: &str) -> ::aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(emulator.clone());
    ::service_common::s3_client::s3_client(&callback, region)
}

/// Create `bucket` through Amazon S3's OWN `CreateBucket`, so a destination an export names
/// is a bucket the object store really holds.
///
/// A regional endpoint requires the create to state its own location constraint, which
/// `us-east-1` — the one Region that constraint is not stated for — is the exception to.
pub(crate) async fn create_bucket(emulator: &MultiServiceRouter, region: &str, bucket: &str) {
    let mut create = s3(emulator, region).create_bucket().bucket(bucket);
    if region != state::DEFAULT_REGION {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    let created = create.send().await;
    assert!(
        created.is_ok(),
        "seed: s3:CreateBucket must create {bucket:?} in {region}, got {:?}",
        created.err()
    );
}

/// Create a customer managed KMS key through KMS's OWN `CreateKey` and answer its key
/// ARN, so a value passed as `EncryptionKeyArn` names a key the store really holds.
pub(crate) async fn create_kms_key(emulator: &MultiServiceRouter, region: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "kms.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "TrentService.CreateKey")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/kms/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::from(b"{}".to_vec()))
        .expect("a well-formed request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "seed: kms:CreateKey must succeed, got {status} ({body:?})"
    );
    body["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
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

/// Put `points` in the store as the recovery points this account holds, each with an
/// EMPTY backup index. Nothing in THIS service creates one — the Backup data plane does —
/// so a test that needs a scope to resolve to something seeds them, exactly as the store
/// would hold them. A test that needs the ITEM layer too seeds the items with
/// [`seed_indexed_recovery_points`].
pub(crate) fn seed_recovery_points(
    engine: &engine::InMemoryEngine,
    points: Vec<types::SearchJobBackupsResult>,
) {
    seed_indexed_recovery_points(engine, points.into_iter().map(Into::into).collect());
}

/// Put `points` in the store as the recovery points this account holds, each with the
/// items its backup index holds — the layer a job's `ItemFilters` select from and
/// `ListSearchJobResults` pages.
pub(crate) fn seed_indexed_recovery_points(
    engine: &engine::InMemoryEngine,
    points: Vec<state::IndexedRecoveryPoint>,
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

/// `StartSearchJob` for an S3 scope, answering the identifier it minted. The first call
/// of every suite that needs a job to act on, so it lives beside the harness rather than
/// once per suite.
pub(crate) async fn start_s3_search_job(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
) -> String {
    let started = call(
        emulator,
        region,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "started", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobIdentifier")
}

/// `count` S3 recovery points this account holds, which is what a `SearchScope` resolves
/// against — so a row that needs the scope to resolve to something seeds these and a row
/// that needs it to resolve to nothing asks for none. Beside the harness because every
/// suite whose rows differ in what the scope resolved to needs exactly this.
pub(crate) fn held_s3_recovery_points(count: usize) -> Vec<types::SearchJobBackupsResult> {
    (0..count)
        .map(|index| {
            recovery_point(
                // `ResourceType` accepts S3 and EBS (model enum `ResourceType`); the
                // scope `start_s3_search_job` above submits selects S3.
                "S3",
                &format!("arn:aws:backup:us-west-2:000000000000:recovery-point:rp-{index}"),
                &format!("arn:aws:s3:::bucket-{index}"),
                std::time::SystemTime::now(),
            )
        })
        .collect()
}

/// Carry this store's clock `span` forward WITHOUT waiting for it: every instant a record
/// is owed a transition from is stored, so settling at an instant `span` ahead of now
/// materialises exactly what the clock would have materialised then — and because a status
/// only ever moves forward, the real clock every handler settles at afterwards can neither
/// take that transition back nor add one that was not due.
///
/// What it is for: the span a scan runs for is the window a cancel has, so it is longer
/// than any gap a client leaves between two calls — long enough that a suite pinning where
/// that window ends would spend its time asleep. Beside the harness because the span
/// belongs to the store and not to one suite.
pub(crate) fn settle_a_span_ahead(engine: &engine::InMemoryEngine, span: Duration) {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    store
        .lock()
        .settle(state::due_at(std::time::SystemTime::now(), span));
}

/// `StopSearchJob` for `identifier`, which is one call every suite that cancels a job
/// makes, so it lives beside the harness rather than once per suite.
pub(crate) async fn cancel_search_job(
    emulator: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> Answer {
    call(
        emulator,
        region,
        "PUT",
        &format!("/search-jobs/{identifier}/actions/cancel"),
        None,
    )
    .await
}

/// One row of the start-a-job table: a `StartSearchJob` body and the region it is signed
/// for. The rows differ in resource type, in whether `Name`/`ItemFilters`/`Tags` are
/// present, in filter count and in signing region.
struct StartCase {
    case: &'static str,
    region: &'static str,
    body: Value,
    /// Whether the row starts its job under a customer managed KMS key. The ARN cannot be
    /// written into the row: `EncryptionKeyArn` is an `@arnReference` to a KMS key, so the
    /// value has to name a key the emulator serving the row HOLDS, which
    /// [`request_body`] creates in it.
    encryption_key: bool,
}

/// The row's request body, with `EncryptionKeyArn` set to a key created in `emulator` when
/// the row asks to be encrypted under one.
async fn request_body(emulator: &MultiServiceRouter, row: &StartCase) -> Value {
    let mut body = row.body.clone();
    if row.encryption_key {
        body["EncryptionKeyArn"] = json!(create_kms_key(emulator, row.region).await);
    }
    body
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
            encryption_key: false,
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
            encryption_key: false,
        },
        StartCase {
            case: "s3 scope with source arns and an encryption key, no item filters",
            region: "us-east-1",
            body: json!({
                "Name": "scoped-to-two-buckets",
                "SearchScope": {
                    "BackupResourceTypes": ["S3"],
                    "SourceResourceArns": ["arn:aws:s3:::alpha", "arn:aws:s3:::beta"],
                    "BackupResourceTags": {"team": "a", "tier": "gold"}
                },
                "Tags": {"env": "prod", "owner": "platform"}
            }),
            encryption_key: true,
        },
    ]
}

#[tokio::test]
async fn a_started_job_is_persisted_and_every_member_is_read_back_out_of_the_store() {
    for row in start_cases() {
        let (case, region) = (row.case, row.region);
        let emulator = emulator();
        let body = request_body(&emulator, &row).await;
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
            state::CREATING,
            "case {case:?}: a job started a moment ago is still being created, got {:?}",
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
    for row in start_cases() {
        let (case, region) = (row.case, row.region);
        let body = request_body(&emulator, &row).await;
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
            Some(state::CREATING),
            "position {position}: a job started a moment ago is still being created, got {summary:?}"
        );
        assert!(
            summary["CreationTime"].as_f64().is_some(),
            "position {position}: the summary must carry the stored CreationTime, got {summary:?}"
        );
        assert!(
            summary["CompletionTime"].is_null(),
            "position {position}: a job that has not reached a terminal status has not completed, got {summary:?}"
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
    for row in start_cases() {
        let (case, region) = (row.case, row.region);
        let emulator = emulator();
        let body = request_body(&emulator, &row).await;
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
    // A WELL-FORMED identifier — the shape this service mints, which is what its syntax
    // guard admits (`state::job_identifier_well_formed`) — that no call ever minted, so
    // every refusal below is the LOOKUP's answer and not an input refusal. A string
    // outside that shape is refused 400 instead; that table is
    // `tests/all/job_identifier_syntax.rs`.
    let unknown = "48434c42-40a0-4057-b04d-3b484c610e2b";
    // Routes that resolve a search job by identifier, and the tagging routes that
    // resolve one by ARN. The last element says WHICH refusal the route answers: a
    // JOB-IDENTIFIER lookup answers the modelled `ResourceNotFoundException` with this
    // service's fixed not-found text, echoing nothing of the request
    // (`state::RESOURCE_NOT_FOUND`), while the tag APIs, which resolve a resource by ARN,
    // are recorded with a refusal of their own — the `NotFoundException` code and
    // sentence `tests/all/tag_not_found_refusal.rs` holds all three of them to.
    let cases: Vec<(&str, &str, String, Option<Value>, bool)> = vec![
        (
            "GetSearchJob",
            "GET",
            format!("/search-jobs/{unknown}"),
            None,
            true,
        ),
        (
            "ListSearchJobResults",
            "GET",
            format!("/search-jobs/{unknown}/search-results"),
            None,
            true,
        ),
        (
            "StopSearchJob",
            "PUT",
            format!("/search-jobs/{unknown}/actions/cancel"),
            None,
            true,
        ),
        (
            "GetSearchResultExportJob",
            "GET",
            format!("/export-search-jobs/{unknown}"),
            None,
            true,
        ),
        (
            "StartSearchResultExportJob",
            "PUT",
            "/export-search-jobs".to_string(),
            Some(json!({
                "SearchJobIdentifier": unknown,
                "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "b"}}
            })),
            true,
        ),
        (
            "ListTagsForResource",
            "GET",
            format!(
                "/tags/arn%3Aaws%3Abackup-search%3Aus-west-2%3A000000000000%3Asearch-job%2F{unknown}"
            ),
            None,
            false,
        ),
        (
            "TagResource",
            "POST",
            format!(
                "/tags/arn%3Aaws%3Abackup-search%3Aus-west-2%3A000000000000%3Asearch-job%2F{unknown}"
            ),
            Some(json!({"Tags": {"env": "test"}})),
            false,
        ),
    ];

    for (operation, method, uri, body, by_identifier) in cases {
        let answer = call(&emulator, "us-west-2", method, &uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {operation:?}: an unknown resource must be 404, got {} ({:?})",
            answer.status, answer.body
        );
        let expected_code = if by_identifier {
            "ResourceNotFoundException"
        } else {
            "NotFoundException"
        };
        assert_eq!(
            answer.error_code.as_deref(),
            Some(expected_code),
            "case {operation:?}: the refusal must carry {expected_code}, got {:?}",
            answer.error_code
        );
        let message = answer.string("message");
        if by_identifier {
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
        } else {
            assert_eq!(
                message,
                state::TAG_RESOURCE_NOT_FOUND,
                "case {operation:?}: a tag lookup refuses with the tag APIs' own not-found text, got {message:?}"
            );
        }
        assert!(
            !message.contains(unknown),
            "case {operation:?}: a not-found refusal of this service echoes nothing of the request, got {message:?}"
        );
    }
}

#[tokio::test]
async fn stopping_a_running_job_moves_it_to_stopping_and_then_to_stopped() {
    let (emulator, engine) = emulator_with_engine();
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

    // Past the set-up phase, so the scan is the RUNNING one this operation acts from.
    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
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
        "StopSearchJob on a running scan must succeed, got {} ({:?})",
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
    // the list reports the same status. The due instant is stored, so the clock is
    // carried to it rather than waited out.
    settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);
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
    // (A status outside the gate the operation acts from is the conflict; that is
    // asserted per status in `search_job_outcome_and_paging.rs` and end-to-end in
    // `stop_search_job_status_gate.rs`.)
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

    // A job is created before it runs, so the first status a caller can observe is
    // CREATING and the scan's RUNNING phase follows it; neither span is zero.
    tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
    let creating = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        creating.string("Status"),
        state::CREATING,
        "the span before the scan starts is never zero, so the job is still CREATING, got {:?}",
        creating.get("Status")
    );
    assert!(
        creating.get("CompletionTime").is_null(),
        "a job being created has no CompletionTime, got {:?}",
        creating.get("CompletionTime")
    );

    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
    let running = call(&emulator, "us-west-2", "GET", &job_uri, None).await;
    assert_eq!(
        running.string("Status"),
        state::RUNNING,
        "once the job has been created its scan is RUNNING, got {:?}",
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
            tagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: expected {} from TagResource, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            tagged.status,
            tagged.body
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
        // A refused write answers the modelled error's status; an accepted one answers
        // this service's no-content success status, since `TagResourceResponse` is empty
        // and its success code is defaulted in the model.
        let expected_on_merge = if expected_status == 400 {
            expected_status
        } else {
            state::NO_CONTENT_SUCCESS_STATUS
        };
        assert_eq!(
            tagged.status, expected_on_merge,
            "case {case:?} at TagResource: expected {expected_on_merge}, got {} ({:?})",
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
                scan_matches: Vec::new(),
                backups: Vec::new(),
                tags: std::collections::HashMap::new(),
                client_token: None,
                request_fingerprint: identifier,
                owed_statuses: std::collections::VecDeque::new(),
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

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, for the `@httpQuery`-bound members (`UntagResource$TagKeys`).
///
/// `+` in particular HAS to be escaped: a query value is decoded with `+` meaning a
/// space, so an unescaped `+` would name a different tag key than the stored one.
/// [`percent_encode`] above is the path-label encoder (it escapes only `/`) and cannot
/// serve a query value, so the two live side by side here — one encoder each, shared by
/// every suite rather than copied into them.
///
/// Written out rather than pulled from `percent-encoding`: a dependency edge is a
/// `Cargo.lock` edit, and every concurrently landing fix rewrites that file, which made
/// an earlier change unlandable (the gate's squash merge refused it with
/// ManualMergeRequiredException on three successive submits that all ten reviewers had
/// approved). Eight lines, test-only, over values these suites write themselves.
pub(crate) fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
