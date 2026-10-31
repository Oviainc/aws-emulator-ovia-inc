// SPDX-License-Identifier: CC0-1.0

//! Which account a signing job records as its invoker, end to end over HTTP.
//!
//! The rule under test: a signing job records the AWS account that invoked it —
//! the account the request's credentials resolve to — and that ONE stored value is
//! both what every read of the job reports as `jobInvoker` and what a job
//! listing's invoker filter is compared against. So a caller that enumerates its
//! own jobs by invoker, which is the documented way to scope a listing to the
//! entity that started them, sees exactly the jobs it started.
//!
//! The rows span what the rule generalises over: how many jobs the account
//! started, which region's endpoint started them, whether the filter names the
//! invoking account or another one, whether it is crossed with a second filter,
//! and which reader reports the member (the start answer's own job, the single-job
//! read, the listing's summary). One row is a repeated idempotency token, where
//! the deduplicated job must still be the one job the invoker filter reports.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite starts its jobs in unless a case names another one.
const REGION: &str = "us-west-2";

/// A second region, so the rule can be shown to hold per endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// A platform this service publishes that signs through the S3 code-signing
/// workflow `StartSigningJob` serves.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// A second such platform, so an invoker filter can be crossed with a platform
/// filter.
const OTHER_PLATFORM_ID: &str = "AWSIoTDeviceManagement-SHA256-ECDSA";

/// The account id every generated emulator presents, spelled out here rather than
/// imported so a wrong account in the handlers fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// A well-formed account id that invokes nothing here (`AccountId` is modelled
/// `@length(12..12)` with pattern `^[0-9]{12}$`, so a filter value has to be
/// twelve digits).
const OTHER_ACCOUNT_ID: &str = "210987654321";

/// An ACM certificate ARN for `signingMaterial`, carried by every put so a profile
/// can be created on a platform that requires the certificate its signatures are
/// issued under.
const CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/12345678-1234-1234-1234-123456789012";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The reported jobs, in the order this answer reported them.
    fn listed_jobs(&self) -> &[Value] {
        self.body["jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("ListSigningJobs must report a jobs list: {:?}", self.body))
    }
}

/// One request through the router, signed for `region`.
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

/// Start a signing job in `region` against `profile_name` under `token`, and
/// answer the whole reply.
async fn start_job(
    router: &MultiServiceRouter,
    region: &str,
    profile_name: &str,
    token: &str,
) -> Answer {
    call(
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
    .await
}

/// `DescribeSigningJob` for `job_id` through `region`.
async fn describe_job(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(router, region, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// `ListSigningJobs` with the raw query string `query`, through `region`.
async fn list(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    call(router, region, "GET", &format!("/signing-jobs{query}"), None).await
}

/// Create a profile and start one job on it, answering the job id.
async fn seed_job(
    router: &MultiServiceRouter,
    region: &str,
    profile_name: &str,
    platform_id: &str,
) -> String {
    put_profile(router, region, profile_name, platform_id).await;
    let started = start_job(router, region, profile_name, &format!("tok-{region}-{profile_name}"))
        .await;
    assert_eq!(
        started.status, 200,
        "seeding a job from {profile_name} in {region}: expected 200, got {} {:?}",
        started.status, started.body
    );
    let job_id = started.str_member("jobId");
    assert!(
        !job_id.is_empty(),
        "seeding a job from {profile_name} in {region}: expected a jobId, got {:?}",
        started.body
    );
    job_id
}

/// Every reader of a started job reports the invoking account, and the listing's
/// invoker filter selects on that same recorded value.
#[tokio::test]
async fn a_started_job_records_the_account_that_invoked_it() {
    let router = emulator();

    // The rows differ along the region the job was started in and the platform its
    // profile is on, so the recorded invoker is shown not to depend on either.
    let cases: &[(&str, &str, &str)] = &[
        ("the home region", REGION, PLATFORM_ID),
        ("a second region", OTHER_REGION, PLATFORM_ID),
        ("a second platform", REGION, OTHER_PLATFORM_ID),
    ];

    for (case, region, platform_id) in cases {
        let profile_name = format!("Invoker_{}", platform_id.replace('-', "_"));
        let job_id = seed_job(&router, region, &profile_name, platform_id).await;

        let described = describe_job(&router, region, &job_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected the started job to read back, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_member("jobInvoker"),
            ACCOUNT_ID,
            "case {case:?}: expected the invoking account recorded, got {:?}",
            described.body
        );

        // The listing's summary says what the single-job read says.
        let listed = list(&router, region, &format!("?jobInvoker={ACCOUNT_ID}")).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the invoker listing to answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let summary = listed
            .listed_jobs()
            .iter()
            .find(|job| job["jobId"].as_str() == Some(job_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the invoker listing to report {job_id}, got {:?}",
                    listed.body
                )
            });
        assert_eq!(
            summary["jobInvoker"], described.body["jobInvoker"],
            "case {case:?}: expected the summary to report what the job read reports, got {:?} \
             and {:?}",
            summary["jobInvoker"], described.body["jobInvoker"]
        );
    }
}

/// The invoker filter reports exactly the jobs the named account started: all of
/// them for the invoking account, none for another, and the intersection when it
/// is crossed with a second filter.
#[tokio::test]
async fn the_invoker_filter_selects_on_the_recorded_account() {
    let router = emulator();

    // Three jobs, two platforms, so a crossed filter has a proper subset to
    // report.
    let seeds: &[(&str, &str)] = &[
        ("Invoker_Listed_A", PLATFORM_ID),
        ("Invoker_Listed_B", OTHER_PLATFORM_ID),
        ("Invoker_Listed_C", PLATFORM_ID),
    ];
    let mut started = Vec::new();
    for (profile_name, platform_id) in seeds {
        started.push(seed_job(&router, REGION, profile_name, platform_id).await);
    }

    // (case, query, how many jobs the listing must report)
    let cases: Vec<(&str, String, usize)> = vec![
        (
            "the invoking account",
            format!("?jobInvoker={ACCOUNT_ID}"),
            seeds.len(),
        ),
        (
            "an account that invoked nothing",
            format!("?jobInvoker={OTHER_ACCOUNT_ID}"),
            0,
        ),
        (
            "the invoking account crossed with a platform",
            format!("?jobInvoker={ACCOUNT_ID}&platformId={PLATFORM_ID}"),
            seeds
                .iter()
                .filter(|(_, platform_id)| *platform_id == PLATFORM_ID)
                .count(),
        ),
        (
            "the invoking account crossed with a platform nothing is on",
            format!("?jobInvoker={ACCOUNT_ID}&platformId=Nonexistent-Platform"),
            0,
        ),
        (
            "no filter at all, for comparison",
            String::new(),
            seeds.len(),
        ),
    ];

    for (case, query, want) in cases {
        let answer = list(&router, REGION, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {query}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.listed_jobs().len(),
            want,
            "case {case:?}: expected {want} job(s) for {query}, got {:?}",
            answer.body
        );
        for job in answer.listed_jobs() {
            assert_eq!(
                job["jobInvoker"].as_str(),
                Some(ACCOUNT_ID),
                "case {case:?}: expected every reported job to name its invoker, got {job:?}"
            );
        }
    }

    // Another region's endpoint reports none of them, invoker filter or not: the
    // invoker narrows a listing, it does not widen it past the region.
    let elsewhere = list(&router, OTHER_REGION, &format!("?jobInvoker={ACCOUNT_ID}")).await;
    assert_eq!(
        elsewhere.listed_jobs().len(),
        0,
        "case \"another region\": expected no jobs, got {:?}",
        elsewhere.body
    );

    // And the invoker listing reports those jobs by IDENTITY, not merely by
    // count: exactly the ids the starts handed back, in the order they started.
    let listed = list(&router, REGION, &format!("?jobInvoker={ACCOUNT_ID}")).await;
    let reported: Vec<&str> = listed
        .listed_jobs()
        .iter()
        .filter_map(|job| job["jobId"].as_str())
        .collect();
    let want: Vec<&str> = started.iter().map(String::as_str).collect();
    assert_eq!(
        reported, want,
        "case \"the invoking account\": expected exactly the started job ids, got {:?}",
        listed.body
    );
}

/// A repeated idempotency token starts no second job, and the one job it did start
/// is the one the invoker filter reports — the listing a caller verifies the
/// deduplication through.
#[tokio::test]
async fn a_repeated_token_leaves_one_job_under_the_invoker_filter() {
    let router = emulator();
    let profile_name = "Invoker_Idempotent";
    put_profile(&router, REGION, profile_name, PLATFORM_ID).await;

    let token = "the-same-token-twice";
    let first = start_job(&router, REGION, profile_name, token).await;
    let second = start_job(&router, REGION, profile_name, token).await;
    for (case, answer) in [("the first call", &first), ("the repeated call", &second)] {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    assert_eq!(
        first.str_member("jobId"),
        second.str_member("jobId"),
        "expected the repeated token to answer with the first job, got {:?} and {:?}",
        first.body,
        second.body
    );

    let listed = list(&router, REGION, &format!("?jobInvoker={ACCOUNT_ID}&maxResults=25")).await;
    let reported: Vec<&str> = listed
        .listed_jobs()
        .iter()
        .filter_map(|job| job["jobId"].as_str())
        .collect();
    assert_eq!(
        reported,
        vec![first.str_member("jobId").as_str()],
        "expected exactly the one deduplicated job under the invoker filter, got {:?}",
        listed.body
    );
}
