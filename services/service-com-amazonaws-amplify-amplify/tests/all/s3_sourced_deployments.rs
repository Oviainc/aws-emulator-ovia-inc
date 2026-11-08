// SPDX-License-Identifier: CC0-1.0

//! `StartDeployment`: deploying a manually deployed Amplify app from Amazon S3.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — against a router that
//! mounts the S3 service BESIDE Amplify on ONE shared engine, which is what the
//! assembled emulator does. Every bucket, object, and bucket policy a case reads is
//! created through S3's own front door, so what Amplify answers is grounded in what S3
//! holds rather than in anything this suite reached into.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over:
//!
//! 1. Amplify reads a deployment's Amazon S3 source in its OWN name, so the source
//!    bucket's policy has to grant the Amplify service principal the read of the
//!    objects the source names: a bucket whose policy does not — no policy at all,
//!    another principal, another action, another bucket, another prefix, an explicit
//!    deny, a condition about another account — is refused with the modelled HTTP 401
//!    `UnauthorizedException`, and no job is started.
//! 2. A source Amplify MAY read starts a job: HTTP 200 with a `jobSummary` whose status
//!    is the non-terminal `PENDING`, whose `jobType` is `MANUAL`, and which is readable
//!    through `GetJob` and `ListJobs` under the branch it deploys to.
//! 3. That job's terminal status is computed from what the read FOUND: it settles to
//!    `SUCCEED` when the source's files were read and carry the root document, and to
//!    `FAILED` — with the reason on its one execution step — when they could not be
//!    (nothing to list, an archive that carries no root document, a source outside this
//!    emulator).
//! 4. A deployment `CreateDeployment` reserved runs under the `jobId` it reserved, from
//!    the uploads that reservation's destinations hold, and the reservation is SPENT by
//!    the call that starts it.
//! 5. A request that names neither a reservation nor a source URL names nothing to
//!    deploy and is refused with the modelled 400; an app, branch or reservation that
//!    names nothing is refused with the modelled 404.
//! 6. A source URL is RESOLVED while the caller waits, so a URL that addresses nothing
//!    deployable — a prefix the bucket holds no root document under, an archive object the
//!    bucket does not hold — takes the modelled 400 and starts no job.
//! 7. The decision about the source bucket's policy carries a value for every global
//!    condition key the read itself fixes, so a clause over one of them is decided rather
//!    than left undecided: the usual statement refusing everything not sent over a secure
//!    transport refuses nothing here, and one refusing what IS sent over one refuses the
//!    read.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::ops::start_deployment::{
    BUCKET_POLICY_WITHOUT_AMPLIFY_ACCESS, NO_SOURCE_MESSAGE, PREFIX_WITHOUT_ROOT_DOCUMENT,
    ROOT_DOCUMENT, SOURCE_URL_WITHOUT_OBJECT,
};
use service_com_amazonaws_amplify_amplify::state::{
    ComAmazonawsAmplifyAmplifyState, JOB_SETTLE_MILLIS,
};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply, ACCOUNT_ID, REGION};

/// A SigV4 `Authorization` header in the `s3` scope: the credential scope is how the
/// router picks the service a request is for, so a request to the object storage front
/// door carries one naming `s3`. Nothing here is verified — the harness turns request
/// authentication off, exactly as an unauthenticated emulator runs.
const SIGNED_FOR_S3: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTS3/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The status Amplify answers a started deployment with: `JobStatus` is modelled
/// `CREATED | PENDING | PROVISIONING | RUNNING | FAILED | SUCCEED | CANCELLING |
/// CANCELLED` (`smithy_query(query="shape",shape="JobStatus")`), and a job that has
/// deployed nothing yet is `PENDING`.
const PENDING: &str = "PENDING";

/// The terminal status of a deployment whose files were read and published.
const SUCCEED: &str = "SUCCEED";

/// The terminal status of a deployment whose files could not be published.
const FAILED: &str = "FAILED";

/// `JobSummary$jobType` for "a manually deployed app"
/// (`smithy_query(query="shape",shape="JobType")`), which is the only kind of app this
/// operation deploys.
const MANUAL: &str = "MANUAL";

/// How long a case waits before expecting the clock-owed status change: the span the
/// service itself publishes, plus a margin for a slow machine.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 500)
}

/// The Amplify emulator with object storage beside it, and the engine both share.
struct Deployments {
    emulator: Emulator,
    engine: Arc<engine::InMemoryEngine>,
}

impl Deployments {
    fn new() -> Self {
        let (emulator, engine) = Emulator::with_object_storage();
        Self { emulator, engine }
    }

    /// One request to the object storage front door, path-style (the emulator serves one
    /// endpoint per region, so the bucket travels in the path). Answers the HTTP status.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", SIGNED_FOR_S3)
            .header("content-type", "application/octet-stream")
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed S3 request");
        self.emulator.dispatch(request).await.status().as_u16()
    }

    /// `PUT /<bucket>` — `CreateBucket`, with no location constraint, so the bucket lands
    /// in the region these requests are signed for.
    async fn create_bucket(&self, bucket: &str) {
        let status = self.s3("PUT", &format!("/{bucket}"), &[]).await;
        assert_eq!(
            status, 200,
            "bucket {bucket:?}: expected CreateBucket 200, got {status}"
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject`.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        let status = self.s3("PUT", &format!("/{bucket}/{key}"), body).await;
        assert_eq!(
            status, 200,
            "object s3://{bucket}/{key}: expected PutObject 200, got {status}"
        );
    }

    /// `PUT /<bucket>?policy` — `PutBucketPolicy`.
    async fn put_bucket_policy(&self, bucket: &str, policy: &Value) {
        let document = serde_json::to_vec(policy).expect("a policy document serializes");
        let status = self.s3("PUT", &format!("/{bucket}?policy"), &document).await;
        assert_eq!(
            status, 204,
            "bucket {bucket:?}: expected PutBucketPolicy 204, got {status}"
        );
    }

    /// A bucket holding a built site under `src/`, and nothing at its root: the layout a
    /// `BUCKET_PREFIX` deployment of `src/` publishes and a deployment of the root
    /// cannot.
    async fn seed_site(&self, bucket: &str) {
        self.create_bucket(bucket).await;
        self.put_object(bucket, "src/index.html", b"<html>site</html>")
            .await;
        self.put_object(bucket, "src/assets/app.js", b"console.log(1)")
            .await;
        self.put_object(bucket, "src/assets/styles/main.css", b"body{}")
            .await;
        self.put_object(bucket, "other/notes.txt", b"not a site")
            .await;
        self.put_object(bucket, "archives/site.zip", &zip_of(&["index.html", "assets/app.js"]))
            .await;
        self.put_object(bucket, "archives/no-root.zip", &zip_of(&["site/index.html"]))
            .await;
    }

    /// An app with one branch on it, and the app's id.
    async fn app_with_branch(&self, name: &str, branch: &str) -> String {
        let app_id = self.emulator.app_id_of_new_app(name).await;
        let created = self
            .emulator
            .create_branch(&app_id, &json!({ "branchName": branch }))
            .await;
        assert_eq!(
            created.status, 200,
            "app {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        app_id
    }

    /// `StartDeployment` over the wire.
    async fn start_deployment(&self, app_id: &str, branch: &str, request: &Value) -> Reply {
        self.emulator
            .call(
                "POST",
                &format!("/apps/{app_id}/branches/{branch}/deployments/start"),
                Some(request),
            )
            .await
    }

    /// `CreateDeployment` over the wire.
    async fn create_deployment(&self, app_id: &str, branch: &str, request: &Value) -> Reply {
        self.emulator
            .call(
                "POST",
                &format!("/apps/{app_id}/branches/{branch}/deployments"),
                Some(request),
            )
            .await
    }

    /// `GetJob` over the wire.
    async fn get_job(&self, app_id: &str, branch: &str, job_id: &str) -> Reply {
        self.emulator
            .call(
                "GET",
                &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
                None,
            )
            .await
    }

    /// `ListJobs` over the wire.
    async fn list_jobs(&self, app_id: &str, branch: &str) -> Reply {
        self.emulator
            .call(
                "GET",
                &format!("/apps/{app_id}/branches/{branch}/jobs"),
                None,
            )
            .await
    }

    /// One request FOLLOWING an upload destination `CreateDeployment` handed back,
    /// exactly as a client follows it: the method and URL the service published, and NO
    /// credential of any kind. Answers the HTTP status.
    async fn upload(&self, method: &str, url: &str, body: &[u8]) -> u16 {
        let request = http::Request::builder()
            .method(method)
            .uri(url)
            .header("content-type", "application/zip")
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("an upload destination is a well-formed URL");
        self.emulator.dispatch(request).await.status().as_u16()
    }

    /// The asset paths one job published, as this service persists them — the content the
    /// deployment put on the branch, which no modelled read reports.
    fn deployed_assets(&self, app_id: &str, branch: &str, job_id: &str) -> BTreeSet<String> {
        let state = self.engine.state::<ComAmazonawsAmplifyAmplifyState>();
        let store = state.lock();
        store
            .apps
            .get(app_id)
            .and_then(|app| app.branches.get(branch))
            .and_then(|branch| branch.jobs.get(job_id))
            .map(|job| job.deployed_assets.clone())
            .unwrap_or_default()
    }
}

/// A `.zip` carrying one (empty-bodied) entry per path named, as a real archive a
/// deployment source can be: the archive format is the `zip` crate's, here and in the
/// handler that reads it back.
fn zip_of(paths: &[&str]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    for path in paths {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("an archive entry starts");
        std::io::Write::write_all(&mut writer, b"deployed")
            .expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}

/// The bucket policy AWS documents for deploying to Amplify from Amazon S3, with the
/// principal, actions, resources and condition the row varies: the Amplify service
/// principal is granted the actions on the resources, scoped to the account and the
/// apps the deployment is for.
fn bucket_policy(
    principal: &str,
    actions: &[&str],
    resources: &[String],
    source_account: &str,
) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AmplifyManualDeployment",
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": actions,
            "Resource": resources,
            "Condition": {
                "StringEquals": { "aws:SourceAccount": source_account },
                "ArnLike": {
                    "aws:SourceArn": format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/*"),
                },
            },
        }],
    })
}

/// The documented grant, in full: the Amplify service principal may read every object of
/// the bucket and list the bucket itself.
fn full_grant(bucket: &str) -> Value {
    bucket_policy(
        AMPLIFY,
        &["s3:GetObject", "s3:ListBucket"],
        &[
            format!("arn:aws:s3:::{bucket}"),
            format!("arn:aws:s3:::{bucket}/*"),
        ],
        ACCOUNT_ID,
    )
}

/// The Amplify service principal a source bucket's policy grants deployment access to.
const AMPLIFY: &str = "amplify.amazonaws.com";

/// The `jobSummary` of a reply, as a client reads it.
fn job_summary(reply: &Reply) -> Value {
    reply.member("jobSummary")
}

/// The one execution step a settled job carries, as a client reads it.
fn settled_step(job: &Value) -> Value {
    let steps = list_member(job, "steps");
    assert_eq!(
        steps.len(),
        1,
        "a settled job carries one execution step, got {steps:?}"
    );
    steps[0].clone()
}

/// Rule 1: a source whose bucket policy does not grant Amplify the read of it is refused
/// with the modelled 401, and starts nothing.
#[tokio::test]
async fn a_source_the_bucket_policy_does_not_grant_amplify_is_refused() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("deploy-refusals", "main").await;

    // Each row: the case, the policy the source bucket carries (none at all for the
    // first), and the deployment request. They differ along every element an IAM
    // decision is made of — the principal, the action, the resource, the prefix the
    // resource covers, the effect, and the condition — plus the source type.
    let rows: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "no bucket policy at all",
            None,
            json!({ "sourceUrl": "s3://deploy-refusal-0/src/", "sourceUrlType": "BUCKET_PREFIX" }),
        ),
        (
            "a policy granting another service principal",
            Some(bucket_policy(
                "cloudfront.amazonaws.com",
                &["s3:GetObject", "s3:ListBucket"],
                &["arn:aws:s3:::deploy-refusal-1/*".to_string()],
                ACCOUNT_ID,
            )),
            json!({ "sourceUrl": "s3://deploy-refusal-1/src/", "sourceUrlType": "BUCKET_PREFIX" }),
        ),
        (
            "a policy granting Amplify another action",
            Some(bucket_policy(
                AMPLIFY,
                &["s3:PutObject"],
                &["arn:aws:s3:::deploy-refusal-2/*".to_string()],
                ACCOUNT_ID,
            )),
            json!({ "sourceUrl": "s3://deploy-refusal-2/src/", "sourceUrlType": "BUCKET_PREFIX" }),
        ),
        (
            "a policy granting Amplify the objects of another bucket",
            Some(bucket_policy(
                AMPLIFY,
                &["s3:GetObject", "s3:ListBucket"],
                &["arn:aws:s3:::some-other-bucket/*".to_string()],
                ACCOUNT_ID,
            )),
            json!({ "sourceUrl": "s3://deploy-refusal-3/src/", "sourceUrlType": "BUCKET_PREFIX" }),
        ),
        (
            "a policy scoped to a prefix the source does not sit under",
            Some(bucket_policy(
                AMPLIFY,
                &["s3:GetObject", "s3:ListBucket"],
                &["arn:aws:s3:::deploy-refusal-4/src/*".to_string()],
                ACCOUNT_ID,
            )),
            json!({ "sourceUrl": "s3://deploy-refusal-4/", "sourceUrlType": "BUCKET_PREFIX" }),
        ),
        (
            "a policy whose condition names another account",
            Some(bucket_policy(
                AMPLIFY,
                &["s3:GetObject", "s3:ListBucket"],
                &["arn:aws:s3:::deploy-refusal-5/*".to_string()],
                "999999999999",
            )),
            json!({ "sourceUrl": "s3://deploy-refusal-5/src/", "sourceUrlType": "BUCKET_PREFIX" }),
        ),
        (
            "a ZIP source whose bucket carries no policy",
            None,
            json!({ "sourceUrl": "s3://deploy-refusal-6/archives/site.zip", "sourceUrlType": "ZIP" }),
        ),
        (
            "a ZIP source whose bucket names no object of it",
            Some(bucket_policy(
                AMPLIFY,
                &["s3:GetObject"],
                &["arn:aws:s3:::deploy-refusal-7".to_string()],
                ACCOUNT_ID,
            )),
            json!({ "sourceUrl": "s3://deploy-refusal-7/archives/site.zip", "sourceUrlType": "ZIP" }),
        ),
    ];

    for (index, (case, policy, request)) in rows.into_iter().enumerate() {
        let bucket = format!("deploy-refusal-{index}");
        deployments.seed_site(&bucket).await;
        if let Some(policy) = policy.as_ref() {
            deployments.put_bucket_policy(&bucket, policy).await;
        }

        let reply = deployments.start_deployment(&app_id, "main", &request).await;
        assert_eq!(
            reply.status, 401,
            "case {case:?}: expected the modelled HTTP 401, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("UnauthorizedException"),
            "case {case:?}: expected the modelled UnauthorizedException, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(BUCKET_POLICY_WITHOUT_AMPLIFY_ACCESS),
            "case {case:?}: expected the recorded refusal text, got {:?}",
            reply.message()
        );
    }

    // A refused deployment starts nothing: the branch has run no job at all.
    let listing = deployments.list_jobs(&app_id, "main").await;
    assert_eq!(
        list_member(&listing.body, "jobSummaries").len(),
        0,
        "case \"refusals start nothing\": expected an empty job listing, got {listing:?}"
    );
    let record = deployments
        .emulator
        .get_branch(&app_id, "main")
        .await
        .member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        "0",
        "case \"refusals start nothing\": a refused deployment is not a started job, \
         got {record:?}"
    );
}

/// One row of the accepted table: the case, the request, the terminal status the job must
/// settle to, the asset paths it must publish, and a fragment its step's reason must name
/// when it failed.
struct AcceptedRow {
    case: &'static str,
    request: Value,
    status: &'static str,
    assets: Vec<&'static str>,
    reason_names: Option<&'static str>,
}

/// Rules 2 and 3: a source Amplify may read starts a job, and that job's terminal status
/// is what reading the source found.
#[tokio::test]
async fn an_authorized_source_starts_a_job_whose_outcome_is_what_was_read() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("deploy-outcomes", "main").await;

    // One bucket with the documented grant in full, and one granting only the read of
    // the objects — which leaves a BUCKET_PREFIX deployment unable to find out WHICH
    // objects to read.
    let granted = "deploy-site-granted";
    deployments.seed_site(granted).await;
    deployments
        .put_bucket_policy(granted, &full_grant(granted))
        .await;
    let read_only = "deploy-site-read-only";
    deployments.seed_site(read_only).await;
    deployments
        .put_bucket_policy(
            read_only,
            &bucket_policy(
                AMPLIFY,
                &["s3:GetObject"],
                &[format!("arn:aws:s3:::{read_only}/*")],
                ACCOUNT_ID,
            ),
        )
        .await;

    // The rows differ along what the rule generalises over: the source type, where under
    // the bucket the source sits, whether the prefix was written with its separator,
    // whether the archive's files carry a root document, whether the listing is
    // permitted, and whether the source is in this emulator. (A source URL that addresses
    // nothing deployable at all is refused before a job exists, which
    // `a_source_url_addressing_nothing_deployable_is_refused` covers.)
    let rows = vec![
        AcceptedRow {
            case: "a prefix holding the built site",
            request: json!({
                "sourceUrl": format!("s3://{granted}/src/"),
                "sourceUrlType": "BUCKET_PREFIX",
            }),
            status: SUCCEED,
            assets: vec!["assets/app.js", "assets/styles/main.css", "index.html"],
            reason_names: None,
        },
        AcceptedRow {
            case: "the same prefix written without its separator",
            request: json!({
                "sourceUrl": format!("s3://{granted}/src"),
                "sourceUrlType": "BUCKET_PREFIX",
            }),
            status: SUCCEED,
            assets: vec!["assets/app.js", "assets/styles/main.css", "index.html"],
            reason_names: None,
        },
        AcceptedRow {
            case: "an archive carrying the built site",
            request: json!({
                "sourceUrl": format!("s3://{granted}/archives/site.zip"),
                "sourceUrlType": "ZIP",
            }),
            status: SUCCEED,
            assets: vec!["assets/app.js", "index.html"],
            reason_names: None,
        },
        AcceptedRow {
            case: "an archive naming no type, which defaults to ZIP",
            request: json!({ "sourceUrl": format!("s3://{granted}/archives/site.zip") }),
            status: SUCCEED,
            assets: vec!["assets/app.js", "index.html"],
            reason_names: None,
        },
        AcceptedRow {
            case: "an archive whose files sit under a folder of their own",
            request: json!({
                "sourceUrl": format!("s3://{granted}/archives/no-root.zip"),
                "sourceUrlType": "ZIP",
            }),
            status: FAILED,
            assets: vec![],
            reason_names: Some(ROOT_DOCUMENT),
        },
        AcceptedRow {
            case: "a prefix whose bucket policy permits no listing",
            request: json!({
                "sourceUrl": format!("s3://{read_only}/src/"),
                "sourceUrlType": "BUCKET_PREFIX",
            }),
            status: FAILED,
            assets: vec![],
            reason_names: Some("s3:ListBucket"),
        },
        AcceptedRow {
            case: "an archive behind a URL outside this emulator",
            request: json!({
                "sourceUrl": "https://downloads.example.test/site.zip",
                "sourceUrlType": "ZIP",
            }),
            status: FAILED,
            assets: vec![],
            reason_names: Some("outbound"),
        },
    ];

    // Every deployment is started first and read afterwards, so the clock-owed settling
    // is waited for ONCE rather than per row.
    let mut started: Vec<(AcceptedRow, String)> = Vec::new();
    for row in rows {
        let reply = deployments
            .start_deployment(&app_id, "main", &row.request)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected HTTP 200 from StartDeployment, got {reply:?}",
            row.case
        );
        let summary = job_summary(&reply);
        assert_eq!(
            string_member(&summary, "status"),
            PENDING,
            "case {:?}: a started deployment has deployed nothing yet, got {summary:?}",
            row.case
        );
        assert_eq!(
            string_member(&summary, "jobType"),
            MANUAL,
            "case {:?}: a manual deployment's job is MANUAL, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("sourceUrl").and_then(Value::as_str),
            row.request.get("sourceUrl").and_then(Value::as_str),
            "case {:?}: the job names the source it deploys, got {summary:?}",
            row.case
        );
        assert_eq!(
            string_member(&summary, "sourceUrlType"),
            row.request
                .get("sourceUrlType")
                .and_then(Value::as_str)
                .unwrap_or("ZIP"),
            "case {:?}: the job names the kind of source, defaulting to ZIP, got {summary:?}",
            row.case
        );
        let job_id = string_member(&summary, "jobId");
        assert!(
            !job_id.is_empty() && job_id.chars().all(|digit| digit.is_ascii_digit()),
            "case {:?}: expected a modelled jobId of decimal digits, got {job_id:?}",
            row.case
        );
        started.push((row, job_id));
    }

    sleep(past_the_settle_span());

    for (row, job_id) in &started {
        let reply = deployments.get_job(&app_id, "main", job_id).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: a started deployment's job is readable, got {reply:?}",
            row.case
        );
        let job = reply.member("job");
        let summary = job.get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            row.status,
            "case {:?}: expected the job to settle to {}, got {summary:?}",
            row.case,
            row.status
        );
        assert!(
            summary.get("endTime").is_some(),
            "case {:?}: a settled job carries an end time, got {summary:?}",
            row.case
        );

        let step = settled_step(&job);
        assert_eq!(
            string_member(&step, "status"),
            row.status,
            "case {:?}: the step reports the status the job reached, got {step:?}",
            row.case
        );
        match row.reason_names {
            Some(fragment) => {
                let reason = step
                    .get("statusReason")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    reason.contains(fragment),
                    "case {:?}: expected the step's reason to name {fragment:?}, got {reason:?}",
                    row.case
                );
            }
            None => assert_eq!(
                step.get("statusReason").and_then(Value::as_str),
                None,
                "case {:?}: a deployment that was published explains nothing, got {step:?}",
                row.case
            ),
        }

        let published = deployments.deployed_assets(&app_id, "main", job_id);
        let expected: BTreeSet<String> = row.assets.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            published, expected,
            "case {:?}: expected the deployment to publish {expected:?}, got {published:?}",
            row.case
        );
    }

    // Every started deployment is one job of this branch, in the order they started.
    let listing = deployments.list_jobs(&app_id, "main").await;
    let summaries = list_member(&listing.body, "jobSummaries");
    assert_eq!(
        summaries.len(),
        started.len(),
        "case \"every deployment is a job\": expected {} jobs, got {listing:?}",
        started.len()
    );
}

/// Rule 4: a reserved deployment runs under its reserved id, from what was uploaded to
/// the destinations the reservation handed out, and is spent by the call that starts it.
#[tokio::test]
async fn a_reserved_deployment_is_started_from_its_uploads() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("deploy-reservations", "main").await;

    // Each row: the case, the `CreateDeployment` request, whether what it reserved is
    // uploaded, the terminal status, and the assets the deployment publishes.
    type UploadRow<'a> = (&'a str, Value, bool, &'a str, Vec<&'a str>);
    let rows: Vec<UploadRow<'_>> = vec![
        (
            "a reservation naming its files, all uploaded",
            json!({ "fileMap": {
                "index.html": "d41d8cd98f00b204e9800998ecf8427e",
                "assets/app.js": "d41d8cd98f00b204e9800998ecf8427e",
            }}),
            true,
            SUCCEED,
            vec!["assets/app.js", "index.html"],
        ),
        (
            "a reservation of the zipped deployment, uploaded",
            json!({}),
            true,
            SUCCEED,
            vec!["assets/app.js", "index.html"],
        ),
        (
            // A reservation whose uploads do not make up a servable site: the files
            // ARRIVED, so the deployment starts, and its job ends FAILED because the
            // branch has no root document to serve. (A reservation nothing was uploaded
            // to is refused instead of started — see `deployment_upload_plane.rs`.)
            "a reservation naming one file that is not the root document",
            json!({ "fileMap": { "assets/app.js": "d41d8cd98f00b204e9800998ecf8427e" }}),
            true,
            FAILED,
            vec![],
        ),
    ];

    for (case, request, upload, status, assets) in rows {
        let reserved = deployments
            .create_deployment(&app_id, "main", &request)
            .await;
        assert_eq!(
            reserved.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}"
        );
        let job_id = string_member(&reserved.body, "jobId");

        if upload {
            let destinations: Vec<(String, Vec<u8>)> = match reserved
                .body
                .get("fileUploadUrls")
                .and_then(Value::as_object)
            {
                Some(per_file) if !per_file.is_empty() => per_file
                    .values()
                    .map(|url| {
                        (
                            url.as_str().expect("a destination is a string").to_string(),
                            b"uploaded".to_vec(),
                        )
                    })
                    .collect(),
                _ => vec![(
                    string_member(&reserved.body, "zipUploadUrl"),
                    zip_of(&["index.html", "assets/app.js"]),
                )],
            };
            for (url, body) in destinations {
                // Followed exactly as a client follows it: the destination the
                // reservation published accepts the upload itself.
                let status = deployments.upload("PUT", &url, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the destination {url:?} must accept the upload, \
                     got {status}"
                );
            }
        }

        let started = deployments
            .start_deployment(&app_id, "main", &json!({ "jobId": job_id }))
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartDeployment, got {started:?}"
        );
        let summary = job_summary(&started);
        assert_eq!(
            string_member(&summary, "jobId"),
            job_id,
            "case {case:?}: the deployment runs under the id it reserved, got {summary:?}"
        );
        assert_eq!(
            summary.get("sourceUrl").and_then(Value::as_str),
            None,
            "case {case:?}: a reserved deployment names no source URL, got {summary:?}"
        );

        // The reservation is SPENT: the same id cannot be started a second time.
        let again = deployments
            .start_deployment(&app_id, "main", &json!({ "jobId": job_id.clone() }))
            .await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a started reservation cannot be started again, got {again:?}"
        );

        sleep(past_the_settle_span());
        let read_back = deployments.get_job(&app_id, "main", &job_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the started deployment's job is readable, got {read_back:?}"
        );
        let job = read_back.member("job");
        let settled = job.get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&settled, "status"),
            status,
            "case {case:?}: expected the job to settle to {status}, got {settled:?}"
        );
        let published = deployments.deployed_assets(&app_id, "main", &job_id);
        let expected: BTreeSet<String> = assets.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            published, expected,
            "case {case:?}: expected the deployment to publish {expected:?}, got {published:?}"
        );
    }
}

/// Rule 5: a request that names nothing to deploy, or an entity that does not exist, is
/// refused with the modelled status for that failure.
#[tokio::test]
async fn a_request_naming_nothing_to_deploy_is_refused() {
    let deployments = Deployments::new();
    let app_id = deployments.app_with_branch("deploy-addressing", "main").await;
    // A branch of ANOTHER app: the name exists in the store and still names nothing on
    // this app.
    let elsewhere = deployments
        .app_with_branch("deploy-addressing-elsewhere", "staging")
        .await;
    assert_ne!(
        app_id, elsewhere,
        "two apps must have distinct ids: got {app_id:?} twice"
    );

    // Each row: the case, the app and branch addressed, the request, the status and the
    // error code expected, and the message the refusal has to carry.
    // (An empty `sourceUrl` or `jobId` is not among the rows: both members are modelled
    // with a `pattern`, so the empty value is refused by the boundary validator before
    // this operation runs at all.)
    let rows: Vec<(&str, String, String, Value, u16, &str, String)> = vec![
        (
            "neither a reservation nor a source URL",
            app_id.clone(),
            "main".to_string(),
            json!({}),
            400,
            "BadRequestException",
            NO_SOURCE_MESSAGE.to_string(),
        ),
        (
            "a reservation that does not exist",
            app_id.clone(),
            "main".to_string(),
            json!({ "jobId": "4242" }),
            404,
            "NotFoundException",
            "Job 4242 not found.".to_string(),
        ),
        (
            "an app that does not exist",
            "d0000000000000".to_string(),
            "main".to_string(),
            json!({ "sourceUrl": "s3://any-bucket/src/", "sourceUrlType": "BUCKET_PREFIX" }),
            404,
            "NotFoundException",
            "App d0000000000000 not found.".to_string(),
        ),
        (
            "a branch that does not exist",
            app_id.clone(),
            "no-such-branch".to_string(),
            json!({ "sourceUrl": "s3://any-bucket/src/", "sourceUrlType": "BUCKET_PREFIX" }),
            404,
            "NotFoundException",
            "Branch no-such-branch not found.".to_string(),
        ),
        (
            "a branch of another app",
            app_id.clone(),
            "staging".to_string(),
            json!({ "sourceUrl": "s3://any-bucket/src/", "sourceUrlType": "BUCKET_PREFIX" }),
            404,
            "NotFoundException",
            "Branch staging not found.".to_string(),
        ),
    ];

    for (case, app, branch, request, status, code, message) in rows {
        let reply = deployments.start_deployment(&app, &branch, &request).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(code),
            "case {case:?}: expected {code}, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.message().as_deref(),
            Some(message.as_str()),
            "case {case:?}: expected the message {message:?}, got {:?}",
            reply.message()
        );
    }
}

/// The documented grant, with the statements a row adds BESIDE it: a bucket policy is a
/// list of statements, and what is under test is how one more statement beside the grant
/// decides.
fn grant_beside(bucket: &str, extra: &[Value]) -> Value {
    let mut policy = full_grant(bucket);
    let statements = policy
        .get_mut("Statement")
        .and_then(Value::as_array_mut)
        .expect("the documented grant carries a statement list");
    statements.extend(extra.iter().cloned());
    policy
}

/// One statement about the whole bucket, conditioned on a single clause: the shape of
/// AWS's own "refuse everything that did not arrive over a secure transport" idiom, with
/// the effect and the clause the row varies.
fn conditioned_statement(bucket: &str, effect: &str, condition: Value) -> Value {
    json!({
        "Effect": effect,
        "Principal": { "Service": AMPLIFY },
        "Action": ["s3:GetObject", "s3:ListBucket"],
        "Resource": [
            format!("arn:aws:s3:::{bucket}"),
            format!("arn:aws:s3:::{bucket}/*"),
        ],
        "Condition": condition,
    })
}

/// Rule 7: the decision about the source bucket's policy carries a value for every global
/// condition key the read itself fixes.
///
/// The rows differ along what that generalises over: the effect the conditioned statement
/// states, the value its clause demands of the key, whether the clause is the grant's own
/// or a statement beside it, and whether the key is one this read models at all — the last
/// of which pins the fail-closed reading that stays in force for everything the read
/// cannot answer.
/// (`aws:SecureTransport` — "whether the request was sent using SSL" — and
/// `aws:PrincipalOrgID` are IAM's global condition keys.)
/// One row of the condition table: the case, the policy the source bucket carries (built
/// from the bucket's own name), and the status `StartDeployment` must answer.
struct ConditionRow {
    case: &'static str,
    policy: fn(&str) -> Value,
    status: u16,
}

#[tokio::test]
async fn a_global_condition_the_source_read_answers_is_decided() {
    let deployments = Deployments::new();
    let app_id = deployments
        .app_with_branch("deploy-conditions", "main")
        .await;

    let rows = vec![
        ConditionRow {
            case: "the documented grant beside a deny of what did not arrive over a secure \
                   transport",
            policy: |bucket| {
                grant_beside(
                    bucket,
                    &[conditioned_statement(
                        bucket,
                        "Deny",
                        json!({ "Bool": { "aws:SecureTransport": "false" } }),
                    )],
                )
            },
            status: 200,
        },
        ConditionRow {
            case: "a grant that holds only for a read over a secure transport",
            policy: |bucket| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [conditioned_statement(
                        bucket,
                        "Allow",
                        json!({ "Bool": { "aws:SecureTransport": "true" } }),
                    )],
                })
            },
            status: 200,
        },
        ConditionRow {
            case: "the documented grant beside a deny of what DID arrive over a secure \
                   transport",
            policy: |bucket| {
                grant_beside(
                    bucket,
                    &[conditioned_statement(
                        bucket,
                        "Deny",
                        json!({ "Bool": { "aws:SecureTransport": "true" } }),
                    )],
                )
            },
            status: 401,
        },
        ConditionRow {
            case: "a grant that holds only for a read that was NOT over a secure transport",
            policy: |bucket| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [conditioned_statement(
                        bucket,
                        "Allow",
                        json!({ "Bool": { "aws:SecureTransport": "false" } }),
                    )],
                })
            },
            status: 401,
        },
        ConditionRow {
            case: "the documented grant beside a deny over a key this read does not answer",
            policy: |bucket| {
                grant_beside(
                    bucket,
                    &[conditioned_statement(
                        bucket,
                        "Deny",
                        json!({ "StringEquals": { "aws:PrincipalOrgID": "o-abc123" } }),
                    )],
                )
            },
            status: 401,
        },
    ];

    let mut accepted: Vec<(&str, String)> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let ConditionRow {
            case,
            policy: policy_of,
            status,
        } = row;
        let bucket = format!("deploy-condition-{index}");
        deployments.seed_site(&bucket).await;
        deployments
            .put_bucket_policy(&bucket, &policy_of(&bucket))
            .await;

        let reply = deployments
            .start_deployment(
                &app_id,
                "main",
                &json!({
                    "sourceUrl": format!("s3://{bucket}/src/"),
                    "sourceUrlType": "BUCKET_PREFIX",
                }),
            )
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {reply:?}"
        );
        if status == 200 {
            let summary = job_summary(&reply);
            assert_eq!(
                string_member(&summary, "status"),
                PENDING,
                "case {case:?}: a started deployment has deployed nothing yet, got {summary:?}"
            );
            accepted.push((case, string_member(&summary, "jobId")));
        } else {
            assert_eq!(
                reply.error_code.as_deref(),
                Some("UnauthorizedException"),
                "case {case:?}: expected the modelled UnauthorizedException, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message().as_deref(),
                Some(BUCKET_POLICY_WITHOUT_AMPLIFY_ACCESS),
                "case {case:?}: expected the recorded refusal text, got {:?}",
                reply.message()
            );
        }
    }

    // A deployment the policy DID grant is one the read then carried out: the files under
    // the prefix are published and the job settles to SUCCEED, read back through GetJob.
    sleep(past_the_settle_span());
    for (case, job_id) in &accepted {
        let reply = deployments.get_job(&app_id, "main", job_id).await;
        let summary = reply.member("job").get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            SUCCEED,
            "case {case:?}: expected the granted deployment's job to succeed, got {reply:?}"
        );
        let published = deployments.deployed_assets(&app_id, "main", job_id);
        let expected: BTreeSet<String> = ["assets/app.js", "assets/styles/main.css", "index.html"]
            .iter()
            .map(|path| (*path).to_string())
            .collect();
        assert_eq!(
            published, expected,
            "case {case:?}: expected the deployment to publish {expected:?}, got {published:?}"
        );
    }
}

/// Rule 6: a source URL that addresses nothing deployable is refused while the caller
/// waits, with the modelled 400 for that source type, and starts no job.
///
/// The rows differ along what the rule generalises over: the source type, whether the
/// addressed path holds anything at all, whether what it holds carries the root document,
/// whether the prefix was written with its separator, and whether the URL names a key at
/// all.
#[tokio::test]
async fn a_source_url_addressing_nothing_deployable_is_refused() {
    let deployments = Deployments::new();
    let app_id = deployments
        .app_with_branch("deploy-resolution", "main")
        .await;
    let bucket = "deploy-resolution-site";
    deployments.seed_site(bucket).await;
    deployments
        .put_bucket_policy(bucket, &full_grant(bucket))
        .await;

    // Each row: the case, the request, and either the refusal it takes or `None` for a
    // source that does resolve.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a prefix at the bucket root, whose files sit one folder down",
            json!({ "sourceUrl": format!("s3://{bucket}/"), "sourceUrlType": "BUCKET_PREFIX" }),
            Some(PREFIX_WITHOUT_ROOT_DOCUMENT),
        ),
        (
            "a prefix holding a file but no root document",
            json!({ "sourceUrl": format!("s3://{bucket}/other/"), "sourceUrlType": "BUCKET_PREFIX" }),
            Some(PREFIX_WITHOUT_ROOT_DOCUMENT),
        ),
        (
            "a prefix holding no object at all",
            json!({ "sourceUrl": format!("s3://{bucket}/nothing-here/"), "sourceUrlType": "BUCKET_PREFIX" }),
            Some(PREFIX_WITHOUT_ROOT_DOCUMENT),
        ),
        (
            "a prefix whose root document sits under a deeper folder still",
            json!({ "sourceUrl": format!("s3://{bucket}/src/assets/"), "sourceUrlType": "BUCKET_PREFIX" }),
            Some(PREFIX_WITHOUT_ROOT_DOCUMENT),
        ),
        (
            "the prefix that does hold the root document",
            json!({ "sourceUrl": format!("s3://{bucket}/src/"), "sourceUrlType": "BUCKET_PREFIX" }),
            None,
        ),
        (
            "that same prefix written without its separator",
            json!({ "sourceUrl": format!("s3://{bucket}/src"), "sourceUrlType": "BUCKET_PREFIX" }),
            None,
        ),
        (
            "an archive object the bucket does not hold",
            json!({ "sourceUrl": format!("s3://{bucket}/archives/absent.zip"), "sourceUrlType": "ZIP" }),
            Some(SOURCE_URL_WITHOUT_OBJECT),
        ),
        (
            "an archive URL naming a folder rather than an object",
            json!({ "sourceUrl": format!("s3://{bucket}/archives/"), "sourceUrlType": "ZIP" }),
            Some(SOURCE_URL_WITHOUT_OBJECT),
        ),
        (
            "an archive URL naming no key at all",
            json!({ "sourceUrl": format!("s3://{bucket}"), "sourceUrlType": "ZIP" }),
            Some(SOURCE_URL_WITHOUT_OBJECT),
        ),
        (
            "the archive object the bucket does hold",
            json!({ "sourceUrl": format!("s3://{bucket}/archives/site.zip"), "sourceUrlType": "ZIP" }),
            None,
        ),
    ];

    let mut started = 0usize;
    for (case, request, refusal) in rows {
        let reply = deployments.start_deployment(&app_id, "main", &request).await;
        match refusal {
            Some(message) => {
                assert_eq!(
                    reply.status, 400,
                    "case {case:?}: expected the modelled HTTP 400, got {reply:?}"
                );
                assert_eq!(
                    reply.error_code.as_deref(),
                    Some("BadRequestException"),
                    "case {case:?}: expected the modelled BadRequestException, got {:?}",
                    reply.error_code
                );
                assert_eq!(
                    reply.message().as_deref(),
                    Some(message),
                    "case {case:?}: expected the recorded refusal text, got {:?}",
                    reply.message()
                );
            }
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected HTTP 200 from StartDeployment, got {reply:?}"
                );
                started += 1;
            }
        }
    }

    // A refused source started nothing: the branch ran exactly the deployments that were
    // accepted.
    let listing = deployments.list_jobs(&app_id, "main").await;
    assert_eq!(
        list_member(&listing.body, "jobSummaries").len(),
        started,
        "case \"a refused source starts nothing\": expected {started} jobs, got {listing:?}"
    );
}
