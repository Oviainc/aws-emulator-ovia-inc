// SPDX-License-Identifier: CC0-1.0

//! How `ListSigningJobs` reports the signing jobs a region holds, end to end over
//! HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpQuery` bindings, deserialization, the handler, the persisted jobs and
//! serialization — the same path a client takes. The rule under test:
//!
//! * a listing reports the jobs of the region whose endpoint answered, and only
//!   the ones every filter the call named selects — each filter compared against
//!   the value the job itself records for that member — each reported out of the
//!   stored record, so a listed job says what the reader of that one job says;
//! * the reported jobs come in one stable total order, cut into pages of at most
//!   `maxResults`, and a page that stops short hands back an opaque `nextToken`
//!   that resumes only a call enumerating that same listing: a token the service
//!   never handed out for it is refused 400 `ValidationException` (`code` naming
//!   the invalid-token condition) rather than served as a page out of some other
//!   ordering;
//! * listing changes nothing a later caller sees, beyond materialising the
//!   completion the clock had already made due for a job it reports.
//!
//! The cases are tables. The filter rows differ along which member is filtered
//! (platform, status, revocation, requesting principal, invoking account, the
//! expiry window), whether the filter selects every job, some or none, and how
//! many filters are combined. The pagination rows differ along the page size and
//! the filter set walked; the refusal rows along the FORM of the rejected value
//! and the LISTING a real token is replayed against.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::JOB_COMPLETION_SPAN;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite lists through unless a case names another one.
const REGION: &str = "us-west-2";

/// A second region, so a listing can be shown to report none of its jobs and a
/// token can be replayed against its endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// A platform this service publishes that signs through the S3 code-signing
/// workflow `StartSigningJob` serves, and whose signatures can be revoked.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// A second such platform, so a listing can be narrowed to a subset of the jobs.
/// Its catalog entry publishes no revocation capability, so no seed on it is
/// revoked.
const OTHER_PLATFORM_ID: &str = "AWSIoTDeviceManagement-SHA256-ECDSA";

/// A platform id this service publishes no entry for, so no job can be on it.
const UNPUBLISHED_PLATFORM_ID: &str = "Nonexistent-Platform";

/// An ACM certificate ARN for `signingMaterial`, carried by every put below so a
/// profile can be created on a platform that requires the certificate its
/// signatures are issued under.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The account id every generated emulator presents, spelled out here rather than
/// imported so a wrong account in the handlers fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// The error `code` a `nextToken` this service never handed out carries, spelled
/// out here rather than imported so a wrong code in the handler fails this suite.
const ERROR_CODE_INVALID_NEXT_TOKEN: &str = "InvalidNextToken";

/// The modelled error shape a refused token is answered with (HTTP 400).
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// `MaxResults`'s modelled minimum (`@range(min: 1, max: 25)`).
const MAX_RESULTS_FLOOR: i32 = 1;

/// `MaxResults`'s modelled maximum (the same `@range`).
const MAX_RESULTS_CEILING: i32 = 25;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or the empty string when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The reported jobs, in the order this answer reported them.
    fn listed_jobs(&self) -> &[Value] {
        self.body["jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("ListSigningJobs must report a jobs list: {:?}", self.body))
    }

    /// The profile name of every reported job, in order — the label this suite
    /// gives each seeded job.
    fn listed_labels(&self) -> Vec<String> {
        self.listed_jobs()
            .iter()
            .map(|job| job["profileName"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The job id of every reported job, in order.
    fn listed_ids(&self) -> Vec<String> {
        self.listed_jobs()
            .iter()
            .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The `nextToken` this answer carries, or `None` on a final page.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Create an ACTIVE profile named `name` in `region`, on `platform_id`.
async fn put_profile(router: &MultiServiceRouter, region: &str, name: &str, platform_id: &str) {
    let answer = call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform_id,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name} in {region}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// Start a signing job in `region` against `profile_name`, answering its job id.
async fn start_job(
    router: &MultiServiceRouter,
    region: &str,
    profile_name: &str,
    token: &str,
) -> String {
    let answer = call(
        router,
        region,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
            "profileName": profile_name,
            "clientRequestToken": token,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "starting a job for {profile_name} in {region}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("jobId")
}

/// Revoke the signature of `job_id` in `region`.
async fn revoke_signature(router: &MultiServiceRouter, region: &str, job_id: &str) {
    let answer = call(
        router,
        region,
        "PUT",
        &format!("/signing-jobs/{job_id}/revoke"),
        Some(json!({ "reason": "key-compromise" })),
    )
    .await;
    assert!(
        (200..300).contains(&answer.status),
        "revoking {job_id} in {region}: expected a success, got {} {:?}",
        answer.status, answer.body
    );
}

async fn describe_job(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(router, region, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// `ListSigningJobs` with the raw query string `query`, through `region`.
async fn list(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    call(router, region, "GET", &format!("/signing-jobs{query}"), None).await
}

/// One seeded job: the label this suite identifies it by (which is also the name
/// of the profile it signs with), the platform that profile is on, and whether its
/// signature is revoked after the job is started.
struct Seed {
    label: &'static str,
    platform_id: &'static str,
    revoked: bool,
}

/// The jobs every case in this suite lists over: enough to truncate a page at
/// every page size under test, spread across two platforms, with a revoked
/// signature on the one whose catalog entry publishes the revocation capability
/// so a revocation filter and a platform filter can be crossed in either
/// direction (the cross with the other platform selects nothing, which is the
/// answer a well-formed filter pair that matches no job must get).
const SEEDS: &[Seed] = &[
    Seed {
        label: "Listed_Job_A",
        platform_id: PLATFORM_ID,
        revoked: false,
    },
    Seed {
        label: "Listed_Job_B",
        platform_id: OTHER_PLATFORM_ID,
        revoked: false,
    },
    Seed {
        label: "Listed_Job_C",
        platform_id: PLATFORM_ID,
        revoked: true,
    },
    Seed {
        label: "Listed_Job_D",
        platform_id: OTHER_PLATFORM_ID,
        revoked: false,
    },
    Seed {
        label: "Listed_Job_E",
        platform_id: PLATFORM_ID,
        revoked: false,
    },
];

/// Seed [`SEEDS`] into `region` — one profile and one job per row, started in the
/// order the rows are written — and answer each label's job id.
async fn seed(router: &MultiServiceRouter, region: &str) -> Vec<(&'static str, String)> {
    let mut started = Vec::with_capacity(SEEDS.len());
    for job in SEEDS {
        put_profile(router, region, job.label, job.platform_id).await;
        let job_id = start_job(router, region, job.label, &format!("tok-{region}-{}", job.label)).await;
        if job.revoked {
            revoke_signature(router, region, &job_id).await;
        }
        started.push((job.label, job_id));
    }
    started
}

/// The labels a listing narrowed to `platform_id` (`None` for every platform) and
/// to revoked signatures (`revoked_only`) must report, in the order the jobs were
/// started.
fn expected(platform_id: Option<&str>, revoked_only: bool) -> Vec<String> {
    SEEDS
        .iter()
        .filter(|job| platform_id.is_none_or(|wanted| wanted == job.platform_id))
        .filter(|job| !revoked_only || job.revoked)
        .map(|job| job.label.to_string())
        .collect()
}

/// `labels` as the owned form a listing answers with.
fn labels(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|label| label.to_string()).collect()
}

/// An instant as a `@httpQuery`-bound timestamp travels: the ISO 8601
/// (`date-time`) rendering such a binding carries by default.
fn instant_param(epoch_secs: i64) -> String {
    DateTime::from_secs(epoch_secs)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// Every filter the call named selects the jobs whose own recorded value it
/// matches: one row per filter shape, with the expectation derived from the seeds
/// rather than from the handler.
#[tokio::test]
async fn a_job_listing_reports_the_jobs_the_filters_the_call_named_select() {
    let router = emulator();
    let jobs = seed(&router, REGION).await;
    // Another region's jobs are in the store throughout, and belong to no case
    // below.
    let _elsewhere = seed(&router, OTHER_REGION).await;

    // Past the completion span every seeded job holds its terminal status, so a
    // status filter has one stable answer however long the rows below take.
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    // The principal the emulator resolved for these calls, read off the job
    // itself rather than written out here.
    let caller = describe_job(&router, REGION, &jobs[0].1)
        .await
        .str_member("requestedBy");
    assert!(
        !caller.is_empty(),
        "expected a started job to record the principal that requested it"
    );

    // (case, query string, the labels the listing must report, in order)
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter at all", String::new(), expected(None, false)),
        (
            "isRevoked spelled out as the modelled default",
            "?isRevoked=false".to_string(),
            expected(None, false),
        ),
        (
            "one platform",
            format!("?platformId={PLATFORM_ID}"),
            expected(Some(PLATFORM_ID), false),
        ),
        (
            "the other platform",
            format!("?platformId={OTHER_PLATFORM_ID}"),
            expected(Some(OTHER_PLATFORM_ID), false),
        ),
        (
            "a platform this service publishes no entry for",
            format!("?platformId={UNPUBLISHED_PLATFORM_ID}"),
            Vec::new(),
        ),
        (
            "revoked signatures only",
            "?isRevoked=true".to_string(),
            expected(None, true),
        ),
        (
            "revoked signatures on one platform",
            format!("?isRevoked=true&platformId={PLATFORM_ID}"),
            expected(Some(PLATFORM_ID), true),
        ),
        (
            "revoked signatures on the other platform",
            format!("?isRevoked=true&platformId={OTHER_PLATFORM_ID}"),
            expected(Some(OTHER_PLATFORM_ID), true),
        ),
        (
            "the terminal status every seeded job has reached",
            "?status=Succeeded".to_string(),
            expected(None, false),
        ),
        (
            "a status no seeded job is in any more",
            "?status=InProgress".to_string(),
            Vec::new(),
        ),
        (
            "a status no seeded job ever reached",
            "?status=Failed".to_string(),
            Vec::new(),
        ),
        (
            "a status and a platform at once",
            format!("?status=Succeeded&platformId={OTHER_PLATFORM_ID}"),
            expected(Some(OTHER_PLATFORM_ID), false),
        ),
        (
            "a status, a platform and the revocation at once",
            format!("?status=Succeeded&platformId={PLATFORM_ID}&isRevoked=true"),
            expected(Some(PLATFORM_ID), true),
        ),
        (
            "the principal that requested every seeded job",
            format!("?requestedBy={caller}"),
            expected(None, false),
        ),
        (
            "a principal that requested none of them",
            format!("?requestedBy=arn:aws:iam::{ACCOUNT_ID}:user/somebody_else"),
            Vec::new(),
        ),
        (
            // Every seeded job was started by this account's credentials, so
            // every one records it as its invoker.
            "the account that invoked every seeded job",
            format!("?jobInvoker={ACCOUNT_ID}"),
            expected(None, false),
        ),
        (
            // A well-formed account id (`AccountId` is 12 digits) that invoked
            // none of them.
            "an account that invoked no job",
            "?jobInvoker=210987654321".to_string(),
            Vec::new(),
        ),
        (
            "the invoking account crossed with a platform",
            format!("?jobInvoker={ACCOUNT_ID}&platformId={PLATFORM_ID}"),
            expected(Some(PLATFORM_ID), false),
        ),
    ];

    for (case, query, want) in cases {
        let answer = list(&router, REGION, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: {query} expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.listed_labels(),
            want,
            "case {case:?}: {query} reported the wrong jobs, got {:?}",
            answer.body["jobs"]
        );
        assert!(
            answer.next_token().is_none(),
            "case {case:?}: a page holding every selected job hands out no token, got {:?}",
            answer.body["nextToken"]
        );
        // Each reported job says, in its own members, what the seeds say about it.
        for reported in answer.listed_jobs() {
            let label = reported["profileName"].as_str().unwrap_or_default();
            let seeded = SEEDS
                .iter()
                .find(|job| job.label == label)
                .unwrap_or_else(|| panic!("case {case:?}: reported an unseeded job {label:?}"));
            assert_eq!(
                reported["platformId"].as_str().unwrap_or_default(),
                seeded.platform_id,
                "case {case:?}: job {label} reported the wrong platform, got {reported:?}"
            );
            assert_eq!(
                reported["isRevoked"].as_bool(),
                Some(seeded.revoked),
                "case {case:?}: job {label} reported the wrong revocation, got {reported:?}"
            );
        }
    }
}

/// Whether a signature expiring at `expiry` (epoch seconds, `None` for a job that
/// records no expiry) falls inside the window `after`..`before`.
///
/// The rule the two bounds state, written out here so the expectation below comes
/// from the window and the job's own reported instant rather than from the
/// handler: the bounds partition at the instant they name, and a signature that is
/// not known to expire at all falls inside no window a caller can name.
fn expires_inside(expiry: Option<f64>, after: Option<i64>, before: Option<i64>) -> bool {
    match expiry {
        Some(at) => {
            after.is_none_or(|after| at >= after as f64)
                && before.is_none_or(|before| at < before as f64)
        }
        None => after.is_none() && before.is_none(),
    }
}

/// `signatureExpiresBefore` and `signatureExpiresAfter` narrow a listing to the
/// jobs whose signature expires inside the window: one row per window shape, with
/// the expectation taken from the expiry each job itself reports.
#[tokio::test]
async fn an_expiry_window_reports_only_the_jobs_whose_signature_expires_inside_it() {
    let router = emulator();
    seed(&router, REGION).await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    // The expiry each job reports, by label — whatever the service records for it.
    let unfiltered = list(&router, REGION, "").await;
    let reported: Vec<(String, Option<f64>)> = unfiltered
        .listed_jobs()
        .iter()
        .map(|job| {
            (
                job["profileName"].as_str().unwrap_or_default().to_string(),
                job["signatureExpiresAt"].as_f64(),
            )
        })
        .collect();
    assert_eq!(
        reported.len(),
        SEEDS.len(),
        "expected every seeded job to be listed, got {:?}",
        unfiltered.body["jobs"]
    );

    /// An instant comfortably before any signature this suite's jobs could
    /// expire at: the epoch.
    const EPOCH: i64 = 0;
    /// An instant comfortably after any of them: 2100-01-01T00:00:00Z.
    const FAR_FUTURE: i64 = 4_102_444_800;

    // (case, the lower bound, the upper bound)
    let windows: Vec<(&str, Option<i64>, Option<i64>)> = vec![
        ("no bound at all", None, None),
        ("a lower bound alone", Some(EPOCH), None),
        ("an upper bound alone", None, Some(FAR_FUTURE)),
        ("both bounds", Some(EPOCH), Some(FAR_FUTURE)),
        ("a window that closes before it opens", Some(FAR_FUTURE), Some(EPOCH)),
    ];

    for (case, after, before) in windows {
        let mut query = String::new();
        if let Some(after) = after {
            query.push_str(&format!("?signatureExpiresAfter={}", instant_param(after)));
        }
        if let Some(before) = before {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("signatureExpiresBefore={}", instant_param(before)));
        }
        let answer = list(&router, REGION, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: {query} expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let want: Vec<String> = reported
            .iter()
            .filter(|(_, expiry)| expires_inside(*expiry, after, before))
            .map(|(label, _)| label.clone())
            .collect();
        assert_eq!(
            answer.listed_labels(),
            want,
            "case {case:?}: {query} reported the wrong jobs for the window, got {:?}",
            answer.body["jobs"]
        );
    }
}

/// A listing reports the jobs of the region whose endpoint answered, and none of
/// any other region's — each region's own ids and nothing else.
#[tokio::test]
async fn a_job_listing_reports_no_other_regions_jobs() {
    let router = emulator();
    let here = seed(&router, REGION).await;
    let there = seed(&router, OTHER_REGION).await;

    for (region, started) in [(REGION, &here), (OTHER_REGION, &there)] {
        let answer = list(&router, region, "").await;
        assert_eq!(
            answer.status, 200,
            "case {region}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let reported: BTreeSet<String> = answer.listed_ids().into_iter().collect();
        let own: BTreeSet<String> = started.iter().map(|(_, id)| id.clone()).collect();
        assert_eq!(
            reported, own,
            "case {region}: expected exactly the jobs started through this endpoint, got {:?}",
            answer.body["jobs"]
        );
    }

    // The two regions' jobs are different jobs, so the check above is evidence.
    let here_ids: BTreeSet<&String> = here.iter().map(|(_, id)| id).collect();
    let there_ids: BTreeSet<&String> = there.iter().map(|(_, id)| id).collect();
    assert!(
        here_ids.is_disjoint(&there_ids),
        "expected the two regions to hold different jobs, got {here_ids:?} and {there_ids:?}"
    );
}

/// Walk a listing with the tokens it hands out, asserting every page is a 200 of
/// at most `page_size` jobs carrying an opaque token, and answering every label
/// reported, in order.
async fn walk(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    filters: &str,
    page_size: i32,
) -> Vec<String> {
    let mut reported = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=SEEDS.len() {
        let query = match token.as_deref() {
            Some(token) => format!("?maxResults={page_size}{filters}&nextToken={token}"),
            None => format!("?maxResults={page_size}{filters}"),
        };
        let page = list(router, region, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: page {query} expected 200, got {} {:?}",
            page.status, page.body
        );
        let page_labels = page.listed_labels();
        assert!(
            page_labels.len() <= usize::try_from(page_size).expect("a positive page size"),
            "case {case:?}: page {query} reported {} jobs, more than the {page_size} asked for",
            page_labels.len()
        );
        reported.extend(page_labels);
        match page.next_token() {
            Some(next) => {
                assert!(
                    next.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "case {case:?}: expected an opaque token, got {next:?}"
                );
                for job in SEEDS {
                    assert!(
                        !next.contains(job.label),
                        "case {case:?}: expected the token to carry no resource name, got {next:?}"
                    );
                }
                token = Some(next);
            }
            None => return reported,
        }
    }
    panic!("case {case:?}: walking at page size {page_size} never reached a final page");
}

/// Walking a listing with its own tokens reports every job of that listing exactly
/// once, in one stable order, whatever page size the caller asked for — and the
/// walk changes nothing a later caller sees.
#[tokio::test]
async fn walking_a_job_listing_with_its_own_tokens_reports_every_job_once() {
    let router = emulator();
    let jobs = seed(&router, REGION).await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    // (case, the filters appended to each page request, the labels expected)
    let listings: Vec<(&str, String, Vec<String>)> = vec![
        ("every job of the region", String::new(), expected(None, false)),
        (
            "one platform",
            format!("&platformId={PLATFORM_ID}"),
            expected(Some(PLATFORM_ID), false),
        ),
        (
            "revoked signatures only",
            "&isRevoked=true".to_string(),
            expected(None, true),
        ),
    ];

    for (case, filters, want) in &listings {
        for page_size in [MAX_RESULTS_FLOOR, 2, 3, MAX_RESULTS_CEILING] {
            let walked = walk(&router, REGION, case, filters, page_size).await;
            assert_eq!(
                &walked, want,
                "case {case:?} at page size {page_size}: expected every job of the listing once, \
                 in the listing's order"
            );
        }
    }

    // Nothing the walk did changed what the jobs are: every job is still
    // readable, in the status and with the completion it reached.
    for (label, job_id) in &jobs {
        let described = describe_job(&router, REGION, job_id).await;
        assert_eq!(
            described.status, 200,
            "case {label}: expected the walked job to still be readable, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_member("status"),
            "Succeeded",
            "case {label}: expected the walked job to keep its status, got {:?}",
            described.body
        );
    }
}

/// `nextToken` is this listing's own opaque cursor: a value it never handed out is
/// refused with the modelled 400 rather than served as a page. One row per FORM of
/// the rejected value, plus the listings a real token must not resume.
#[tokio::test]
async fn a_next_token_this_listing_never_handed_out_is_refused() {
    let router = emulator();
    let jobs = seed(&router, REGION).await;
    seed(&router, OTHER_REGION).await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    let unfiltered = list(&router, REGION, "?maxResults=1")
        .await
        .next_token()
        .expect("a page that stops short hands out a token");
    let filtered = list(&router, REGION, &format!("?maxResults=1&platformId={PLATFORM_ID}"))
        .await
        .next_token()
        .expect("a filtered page that stops short hands out a token");
    let corrupted = {
        let mut digits: Vec<char> = unfiltered.chars().collect();
        let last = digits.len() - 1;
        digits[last] = if digits[last] == '0' { '1' } else { '0' };
        digits.into_iter().collect::<String>()
    };

    // (case, the region the token is replayed against, the filters it is replayed
    //  with, the rejected value)
    let cases: Vec<(&str, &str, String, String)> = vec![
        ("a job id", REGION, String::new(), jobs[0].1.clone()),
        (
            "a resource-name-shaped value",
            REGION,
            String::new(),
            SEEDS[0].label.to_string(),
        ),
        (
            "hex this service never minted",
            REGION,
            String::new(),
            "ab".repeat(40),
        ),
        ("the empty string", REGION, String::new(), String::new()),
        (
            "a token truncated in transit",
            REGION,
            String::new(),
            unfiltered[..unfiltered.len() - 4].to_string(),
        ),
        (
            "a token altered in transit",
            REGION,
            String::new(),
            corrupted,
        ),
        (
            "a token issued for another filter set",
            REGION,
            String::new(),
            filtered.clone(),
        ),
        (
            "a token issued for the unfiltered listing, replayed against a filtered one",
            REGION,
            format!("&platformId={PLATFORM_ID}"),
            unfiltered.clone(),
        ),
        (
            "a token issued for another region",
            OTHER_REGION,
            String::new(),
            unfiltered.clone(),
        ),
    ];

    for (case, region, filters, token) in cases {
        let answer = list(&router, region, &format!("?maxResults=1{filters}&nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains(ERROR_SHAPE_VALIDATION),
            "case {case:?}: expected a {ERROR_SHAPE_VALIDATION}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.code(),
            ERROR_CODE_INVALID_NEXT_TOKEN,
            "case {case:?}: expected the invalid-token code, got {:?}",
            answer.body["code"]
        );
        assert!(
            answer.body["jobs"].is_null(),
            "case {case:?}: expected a refusal to report no jobs, got {:?}",
            answer.body["jobs"]
        );
    }

    // The refusals changed nothing: the listing still serves its own tokens.
    let walked = walk(&router, REGION, "after the refusals", "", 2).await;
    assert_eq!(
        walked,
        expected(None, false),
        "expected the listing to still walk every job once"
    );
}

/// `maxResults` is modelled `@range(min: 1, max: 25)`: each published bound is
/// served, and one step past either is refused before any job is reported.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    seed(&router, REGION).await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    // (case, maxResults, whether the model admits it)
    let cases: Vec<(&str, i32, bool)> = vec![
        ("one below the floor", MAX_RESULTS_FLOOR - 1, false),
        ("the floor", MAX_RESULTS_FLOOR, true),
        ("the ceiling", MAX_RESULTS_CEILING, true),
        ("one past the ceiling", MAX_RESULTS_CEILING + 1, false),
    ];

    for (case, max_results, admitted) in cases {
        let answer = list(&router, REGION, &format!("?maxResults={max_results}")).await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: maxResults={max_results} is in range, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer.listed_labels();
            assert!(
                !page.is_empty()
                    && page.len() <= usize::try_from(max_results).expect("a positive page size"),
                "case {case:?}: expected at most {max_results} jobs and at least one, got {page:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: maxResults={max_results} is outside the modelled range, got {} \
                 {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.body["jobs"].is_null(),
                "case {case:?}: expected a refusal to report no jobs, got {:?}",
                answer.body["jobs"]
            );
        }
    }

    // An absent `maxResults` serves the whole listing, up to the modelled
    // maximum.
    let unbounded = list(&router, REGION, "").await;
    assert_eq!(
        unbounded.listed_labels(),
        expected(None, false),
        "expected a call naming no maxResults to serve the listing, got {:?}",
        unbounded.body["jobs"]
    );
}

/// A listed job carries what the store holds for it — the same values the reader
/// of that one job reports — including the completion the clock has already made
/// due, and listing does not move anything itself.
#[tokio::test]
async fn a_listed_job_reports_what_describe_signing_job_reports() {
    let router = emulator();
    put_profile(&router, REGION, "Round_Trip_Job", PLATFORM_ID).await;
    let job_id = start_job(&router, REGION, "Round_Trip_Job", "tok-round-trip").await;

    // Straight after the start the job is still running — the completion span is
    // never zero — so that is the status the listing reports, and the status
    // filters follow it.
    let running = list(&router, REGION, "").await;
    assert_eq!(
        running.listed_labels(),
        labels(&["Round_Trip_Job"]),
        "expected the started job to be listed, got {:?}",
        running.body["jobs"]
    );
    assert_eq!(
        running.listed_jobs()[0]["status"].as_str(),
        Some("InProgress"),
        "expected a just-started job to be listed InProgress, got {:?}",
        running.body["jobs"]
    );
    assert!(
        running.listed_jobs()[0]["signedObject"].is_null(),
        "expected no signed object while the job is running, got {:?}",
        running.body["jobs"]
    );
    assert_eq!(
        list(&router, REGION, "?status=InProgress")
            .await
            .listed_ids(),
        labels(&[&job_id]),
        "expected the running job to be selected by its own status"
    );
    assert!(
        list(&router, REGION, "?status=Succeeded")
            .await
            .listed_ids()
            .is_empty(),
        "expected the running job not to be selected by a status it has not reached"
    );

    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    let done = list(&router, REGION, "").await;
    let listed = done.listed_jobs()[0].clone();
    let described = describe_job(&router, REGION, &job_id).await;
    assert_eq!(
        described.status, 200,
        "expected the job to be readable, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        listed["status"].as_str(),
        Some("Succeeded"),
        "expected the completion the clock made due to be reported, got {listed:?}"
    );

    // Member for member, the summary says what the reader of that one job says.
    for member in [
        "jobId",
        "status",
        "source",
        "signedObject",
        "signingMaterial",
        "createdAt",
        "profileName",
        "profileVersion",
        "platformId",
        "platformDisplayName",
        "signatureExpiresAt",
        "jobOwner",
        "jobInvoker",
    ] {
        assert_eq!(
            listed[member], described.body[member],
            "case {member:?}: expected the listed job to report what DescribeSigningJob reports, \
             got {:?} and {:?}",
            listed[member], described.body[member]
        );
    }
    assert_eq!(
        listed["jobId"].as_str(),
        Some(job_id.as_str()),
        "expected the listed job to be the started one, got {listed:?}"
    );

    // Listing moved nothing on its own: a later reader sees the same completion.
    let again = list(&router, REGION, "").await;
    assert_eq!(
        again.listed_jobs()[0], listed,
        "expected a second listing to report the same job, got {:?}",
        again.body["jobs"]
    );
    assert_eq!(
        describe_job(&router, REGION, &job_id).await.body["completedAt"],
        described.body["completedAt"],
        "expected the completion instant to be unmoved by the listings"
    );
}
