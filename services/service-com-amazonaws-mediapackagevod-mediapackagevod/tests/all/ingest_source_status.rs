// SPDX-License-Identifier: CC0-1.0

//! What an egress endpoint's `Status` REPORTS: the real outcome of the packaging job the
//! ingest of one asset by one packaging configuration started.
//!
//! THE RULE UNDER TEST. `EgressEndpoint$Status` is "The current processing status of the
//! asset used for the packaging configuration … QUEUED, PROCESSING, PLAYABLE, or FAILED",
//! and MediaPackage VOD ingest reads the object `CreateAssetRequest$SourceArn` names ("ARN
//! of the source object in S3") AS the role `SourceRoleArn` names ("The IAM role ARN used
//! to access the source S3 bucket"). So a job that is not running yet is `QUEUED`, one
//! that is running is `PROCESSING`, and the TERMINAL status is the job's OUTCOME: a source
//! the service could read ends `PLAYABLE`, and a source it established it could NOT read —
//! an object the store does not serve, a role the account's IAM holds none of or whose
//! trust withholds the assume — ends `FAILED`, because no packaging job could have run.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` over ONE engine carrying
//! this service's store, Amazon S3's and IAM's — the way the assembled emulator runs them —
//! so each source is staged with a real `PutObject` and each role created with a real
//! `CreateRole`, and the ingest reads exactly those records.
//!
//! The rows differ along what the rule generalises over: which half of the source does not
//! resolve (the object, the bucket, the role), the shape of the key (a bare name, a nested
//! prefix, a prefix of a stored key), the form of the trust policy (another service, an
//! explicit deny), whose account the role belongs to, and whether the source ARN names an
//! object at all. One test stands apart: it mounts NEITHER of the other two services, to
//! pin that a failure is only ever reported where one of them ANSWERED.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::packaging_lifecycle::{account_id, call, create_config, create_group, REGION};
// The one percent-encoder this test target already has, for IAM's form-encoded body.
use crate::resource_tag_removal::urlencode;

/// The group and configuration every asset below is ingested into.
const GROUP: &str = "ingest-group";
const CONFIG: &str = "ingest-cfg";

/// The bucket the readable sources are staged in.
const BUCKET: &str = "vod-sources";

/// A bucket no `CreateBucket` here ever creates.
const ABSENT_BUCKET: &str = "vod-sources-missing";

/// Past [`vod::INGEST_PLAYABLE_AFTER`], so a packaging job's terminal transition is due.
const PAST_TERMINAL: Duration = vod::INGEST_PLAYABLE_AFTER.saturating_add(Duration::from_millis(150));

/// Inside the processing span: just past [`vod::INGEST_PROCESSING_AFTER`] and well short
/// of [`vod::INGEST_PLAYABLE_AFTER`], which is where a running job is observed — the
/// offset is small so the margin to the terminal instant stays the larger of the two.
const INSIDE_PROCESSING: Duration =
    vod::INGEST_PROCESSING_AFTER.saturating_add(Duration::from_millis(40));

/// A SigV4 credential scope naming Amazon S3, which is what claims a request for the
/// object store. No authenticator is registered, so the signature is never checked; the
/// scope is what routes it.
fn s3_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=unverified"
    )
}

/// One emulator: this service, Amazon S3 and IAM on a single shared engine behind the
/// production router — what `core/router` assembles, narrowed to the three services the
/// rule under test spans.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", s3_authorization())
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

/// `PUT /<bucket>` — `CreateBucket`, naming this Region so the endpoint the request was
/// sent to can serve it.
async fn given_bucket(router: &MultiServiceRouter) {
    let body = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let status = s3(router, "PUT", &format!("/{BUCKET}"), body.as_bytes()).await;
    assert_eq!(
        status, 200,
        "the source bucket must be created first, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`, storing a source manifest under `key`.
async fn given_object(router: &MultiServiceRouter, key: &str) {
    let body = b"<smil><body><switch/></body></smil>";
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert_eq!(
        status, 200,
        "case {key:?}: the source object must be stored, got {status}"
    );
}

/// Create an IAM role carrying `document` as its trust policy, and answer the ARN IAM
/// minted for it — read off the response rather than constructed, so the asset names the
/// ARN IAM actually holds.
async fn create_role(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        urlencode(name),
        urlencode(document)
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 200,
        "CreateRole {name:?} expected 200, got {status} {xml}"
    );
    let start = xml
        .find("<Arn>")
        .unwrap_or_else(|| panic!("CreateRole {name:?} returned no <Arn>: {xml}"))
        + "<Arn>".len();
    let end = xml[start..]
        .find("</Arn>")
        .unwrap_or_else(|| panic!("CreateRole {name:?} left <Arn> unclosed: {xml}"));
    xml[start..start + end].to_string()
}

/// A trust policy with one statement, written as a client would send it.
fn policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":\"{service}\"}},\"Action\":\"{action}\"}}]}}"
    )
}

/// The trust policy that lets MediaPackage read the source as the role, for the rows whose
/// subject is the OBJECT rather than the role.
fn assumable() -> String {
    policy(
        "Allow",
        "sts:AssumeRole",
        vod::ingest_source::SERVICE_PRINCIPAL,
    )
}

/// `CreateAsset` with the source the row names, asserting the submit itself succeeded: an
/// unreadable source is not a bad REQUEST — AWS accepts the asset and the ingest fails
/// afterwards — so every row here is an HTTP 200 that stores an asset.
async fn create_asset(
    router: &MultiServiceRouter,
    id: &str,
    source_arn: &str,
    source_role_arn: &str,
) -> Value {
    let body = json!({
        "id": id,
        "packagingGroupId": GROUP,
        "sourceArn": source_arn,
        "sourceRoleArn": source_role_arn,
    });
    let (status, asset) = call(router, "POST", "/assets", Some(body)).await;
    assert_eq!(status, 200, "CreateAsset {id}: body {asset}");
    asset
}

/// The `status` of the one egress endpoint `DescribeAsset` reports for `id`.
async fn described_status(router: &MultiServiceRouter, id: &str) -> String {
    let (status, asset) = call(router, "GET", &format!("/assets/{id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {id}: body {asset}");
    endpoint_status(&asset, CONFIG)
}

/// The `status` an asset projection reports for the endpoint of `config`.
fn endpoint_status(asset: &Value, config: &str) -> String {
    let endpoints = asset["egressEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("an asset carries an egressEndpoints list, got {asset}"));
    let endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint["packagingConfigurationId"] == json!(config))
        .unwrap_or_else(|| panic!("no endpoint for {config:?} in {asset}"));
    endpoint["status"]
        .as_str()
        .unwrap_or_else(|| panic!("an endpoint carries a status, got {endpoint}"))
        .to_string()
}

/// The group, the packaging configuration and the staged source objects every row below
/// ingests from.
async fn provision(router: &MultiServiceRouter) {
    create_group(router, GROUP, None).await;
    create_config(
        router,
        CONFIG,
        GROUP,
        json!({ "hlsPackage": { "hlsManifests": [{}] } }),
    )
    .await;
    given_bucket(router).await;
    given_object(router, "movie.smil").await;
    given_object(router, "2026/01/episode.smil").await;
}

/// One source an asset is ingested with, and the terminal status its endpoints must settle
/// on.
struct Row {
    case: &'static str,
    /// The asset id this row ingests under.
    id: &'static str,
    /// `SourceArn`, built from the bucket and key the row names.
    source_arn: String,
    /// Which role the row names, resolved to an ARN once the roles exist.
    role: Role,
    /// The terminal `EgressEndpoint$Status` the row must settle on.
    expected: &'static str,
}

/// Which `SourceRoleArn` a row names.
enum Role {
    /// A role IAM holds whose trust policy lets MediaPackage assume it.
    Assumable,
    /// A role IAM holds whose trust policy names another service.
    OtherService,
    /// A role IAM holds whose trust policy denies MediaPackage explicitly.
    Denied,
    /// A well-formed role ARN of this account that IAM holds no role for.
    Absent,
    /// A role of ANOTHER account, which this emulator's IAM cannot answer for.
    OtherAccount,
}

/// An ingest settles on `PLAYABLE` only where the service read the asset's source, and on
/// `FAILED` wherever it established it could not — whichever half of the source it was.
#[tokio::test]
async fn an_egress_endpoint_settles_on_the_outcome_of_the_packaging_job() {
    let router = emulator();
    provision(&router).await;

    let assumable_role = create_role(&router, "vod-ingest", &assumable()).await;
    let other_service_role = create_role(
        &router,
        "vod-ingest-other",
        &policy("Allow", "sts:AssumeRole", "medialive.amazonaws.com"),
    )
    .await;
    let denied_role = create_role(
        &router,
        "vod-ingest-denied",
        &policy(
            "Deny",
            "sts:AssumeRole",
            vod::ingest_source::SERVICE_PRINCIPAL,
        ),
    )
    .await;
    let absent_role = format!("arn:aws:iam::{}:role/vod-ingest-never-created", account_id());
    // Another account's roles are not this emulator's IAM to answer for, so nothing about
    // such a value is established and the ingest is not refuted by it.
    let other_account_role = "arn:aws:iam::123456789012:role/vod-ingest".to_string();

    let rows: Vec<Row> = vec![
        Row {
            case: "a staged object, read as a role that trusts MediaPackage",
            id: "readable",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie.smil"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_PLAYABLE,
        },
        Row {
            case: "a staged object under a nested prefix",
            id: "readable-nested",
            source_arn: format!("arn:aws:s3:::{BUCKET}/2026/01/episode.smil"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_PLAYABLE,
        },
        Row {
            case: "a key the bucket does not hold",
            id: "absent-key",
            source_arn: format!("arn:aws:s3:::{BUCKET}/no-such-object.smil"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a key that only prefixes a stored one",
            id: "prefix-of-key",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a key that only extends a stored one",
            id: "extension-of-key",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie.smil.bak"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a bucket that was never created",
            id: "absent-bucket",
            source_arn: format!("arn:aws:s3:::{ABSENT_BUCKET}/movie.smil"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "an ARN naming a bucket and no object within it",
            id: "bucket-only",
            source_arn: format!("arn:aws:s3:::{BUCKET}"),
            role: Role::Assumable,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a staged object, read as a role the account holds none of",
            id: "absent-role",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie.smil"),
            role: Role::Absent,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a staged object, read as a role that trusts another service",
            id: "other-service-role",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie.smil"),
            role: Role::OtherService,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a staged object, read as a role that denies MediaPackage",
            id: "denied-role",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie.smil"),
            role: Role::Denied,
            expected: vod::EGRESS_STATUS_FAILED,
        },
        Row {
            case: "a staged object, read as another account's role",
            id: "other-account-role",
            source_arn: format!("arn:aws:s3:::{BUCKET}/movie.smil"),
            role: Role::OtherAccount,
            expected: vod::EGRESS_STATUS_PLAYABLE,
        },
    ];

    // Every submit is accepted and reports a job that has not run yet, whatever its
    // source: an unreadable source is not a bad request.
    for row in &rows {
        let role_arn = match row.role {
            Role::Assumable => &assumable_role,
            Role::OtherService => &other_service_role,
            Role::Denied => &denied_role,
            Role::Absent => &absent_role,
            Role::OtherAccount => &other_account_role,
        };
        let asset = create_asset(&router, row.id, &row.source_arn, role_arn).await;
        let status = endpoint_status(&asset, CONFIG);
        assert_eq!(
            status,
            vod::EGRESS_STATUS_QUEUED,
            "case {:?}: at submit the packaging job has not been picked up, got {status:?}",
            row.case
        );
    }

    // Past the terminal instant each job has run, so each endpoint reports its OUTCOME.
    tokio::time::sleep(PAST_TERMINAL).await;
    for row in &rows {
        let status = described_status(&router, row.id).await;
        assert_eq!(
            status, row.expected,
            "case {:?}: expected {:?}, got {status:?}",
            row.case, row.expected
        );
        // A terminal status is terminal: reading again neither advances nor undoes it.
        let again = described_status(&router, row.id).await;
        assert_eq!(
            again, row.expected,
            "case {:?}: a second read must report the same terminal status, got {again:?}",
            row.case
        );
    }
}

/// An ingest is reported as FAILED only where IAM or Amazon S3 ANSWERED that the source
/// cannot be read. An assembly carrying NEITHER establishes nothing about the source, so
/// it refutes nothing: the job keeps the outcome it would have had before the question
/// could be put at all.
///
/// This pins the `UNREADABLE_OBJECT_CODES` reading against the OTHER 404 a probe can meet
/// — the router's own "no service is registered for this" — so an answer that is not the
/// object store's can never be mistaken for one that is.
#[tokio::test]
async fn an_ingest_with_no_store_to_ask_is_not_reported_as_failed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(engine),
    );
    create_group(&router, GROUP, None).await;
    create_config(
        &router,
        CONFIG,
        GROUP,
        json!({ "hlsPackage": { "hlsManifests": [{}] } }),
    )
    .await;

    // Neither half of the source resolves to anything here, because there is nothing to
    // resolve it against: no object store and no IAM are mounted beside this service.
    let asset = create_asset(
        &router,
        "unestablished",
        &format!("arn:aws:s3:::{BUCKET}/movie.smil"),
        &format!("arn:aws:iam::{}:role/vod-ingest", account_id()),
    )
    .await;
    let queued = endpoint_status(&asset, CONFIG);
    assert_eq!(
        queued,
        vod::EGRESS_STATUS_QUEUED,
        "a submitted job starts queued however much is mounted, got {queued:?}"
    );

    tokio::time::sleep(PAST_TERMINAL).await;
    let terminal = described_status(&router, "unestablished").await;
    assert_eq!(
        terminal,
        vod::EGRESS_STATUS_PLAYABLE,
        "nothing answered the read of this source, so nothing refutes it: expected {:?}, got \
         {terminal:?}",
        vod::EGRESS_STATUS_PLAYABLE
    );
}

/// A packaging job walks QUEUED -> PROCESSING -> its outcome, so all four modelled
/// statuses are reachable — and a configuration attached AFTER the asset runs its own job
/// from when IT was created, settling on the same outcome the asset's source decides.
#[tokio::test]
async fn a_packaging_job_walks_the_modelled_statuses_to_its_outcome() {
    let router = emulator();
    provision(&router).await;
    let role = create_role(&router, "vod-ingest", &assumable()).await;

    // (case, the source this asset names, the outcome its job must reach)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a source the service reads",
            format!("arn:aws:s3:::{BUCKET}/movie.smil"),
            vod::EGRESS_STATUS_PLAYABLE,
        ),
        (
            "a source the store does not serve",
            format!("arn:aws:s3:::{BUCKET}/no-such-object.smil"),
            vod::EGRESS_STATUS_FAILED,
        ),
    ];

    for (case, source_arn, outcome) in &rows {
        let id = format!("walk-{}", outcome.to_ascii_lowercase());
        let asset = create_asset(&router, &id, source_arn, &role).await;
        let queued = endpoint_status(&asset, CONFIG);
        assert_eq!(
            queued,
            vod::EGRESS_STATUS_QUEUED,
            "case {case:?}: a submitted job starts queued, got {queued:?}"
        );

        tokio::time::sleep(INSIDE_PROCESSING).await;
        let running = described_status(&router, &id).await;
        assert_eq!(
            running,
            vod::EGRESS_STATUS_PROCESSING,
            "case {case:?}: a picked-up job reports processing, got {running:?}"
        );

        tokio::time::sleep(PAST_TERMINAL - INSIDE_PROCESSING).await;
        let terminal = described_status(&router, &id).await;
        assert_eq!(
            terminal, *outcome,
            "case {case:?}: expected {outcome:?}, got {terminal:?}"
        );

        // A SECOND packaging configuration attached now: its own job is queued from its
        // own creation, and settles on the outcome this asset's source decides — the
        // status is per (asset, configuration), the outcome is the asset's.
        let late_config = format!("late-{}", outcome.to_ascii_lowercase());
        create_config(
            &router,
            &late_config,
            GROUP,
            json!({ "dashPackage": { "dashManifests": [{}] } }),
        )
        .await;
        let (_, asset) = call(&router, "GET", &format!("/assets/{id}"), None).await;
        let late = endpoint_status(&asset, &late_config);
        assert_eq!(
            late,
            vod::EGRESS_STATUS_QUEUED,
            "case {case:?}: a configuration added now has not packaged anything yet, got \
             {late:?}"
        );
        // The endpoint of the configuration created earlier is unmoved by it.
        let first = endpoint_status(&asset, CONFIG);
        assert_eq!(
            first, *outcome,
            "case {case:?}: the first configuration's endpoint keeps its outcome, got {first:?}"
        );

        tokio::time::sleep(PAST_TERMINAL).await;
        let (_, asset) = call(&router, "GET", &format!("/assets/{id}"), None).await;
        let late = endpoint_status(&asset, &late_config);
        assert_eq!(
            late, *outcome,
            "case {case:?}: the late configuration's job reaches the same outcome, expected \
             {outcome:?}, got {late:?}"
        );
    }
}
