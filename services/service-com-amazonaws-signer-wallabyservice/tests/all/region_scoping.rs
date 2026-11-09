// SPDX-License-Identifier: CC0-1.0

//! AWS Signer's resources are REGIONAL, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! credential scope the region is read from, the handler, the persisted record and
//! serialization — the same path a client takes. The rule under test:
//!
//! * a signing profile — and the signing jobs, tags and permissions that hang off
//!   it — belongs to the region whose endpoint created it, so every read and every
//!   mutation resolves only within the region the request was signed for, and a
//!   call naming it through any other region's endpoint finds nothing (404);
//! * the same `profileName` in two regions therefore names two INDEPENDENT
//!   profiles: each carries its own ARN, versions, platform, tags and status, and
//!   a mutation through one region's endpoint leaves the other untouched.
//!
//! The cases are tables: the rows differ along what the rule generalises over —
//! which region holds the profile, which other region the call is signed for, and
//! which operation the call names (every profile-addressing operation this crate
//! implements, reading and mutating alike).

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    ERROR_CODE_JOB_NOT_FOUND, ERROR_CODE_PROFILE_NOT_FOUND,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The regions this suite addresses Signer through.
///
/// Signer publishes a per-region endpoint (`signer.<region>.amazonaws.com`), so
/// the rule holds for every region and the rows are four spread across three
/// partitions' worth of geographies — including `us-east-1`, which is also the
/// region this service falls back to for a request carrying no credential scope,
/// so a handler that ignored the scope would still have to answer for it.
const REGIONS: [&str; 4] = ["us-west-2", "eu-west-1", "ap-southeast-1", "us-east-1"];

/// The account id every generated emulator presents, spelled out here rather than
/// imported so a wrong account in the handlers fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// A platform from this service's catalog whose profiles need no
/// `signingMaterial` (`crate::state::SIGNING_PLATFORM_CATALOG`).
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// A second such platform, so a per-region row can differ in its platform.
const OTHER_PLATFORM_ID: &str = "Notation-OCI-SHA384-ECDSA";

/// The profile name every region in this suite uses — one name, so a leak across
/// regions shows up as the WRONG profile rather than as no profile.
const PROFILE_NAME: &str = "Region_Scoped";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or `""` when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// `value` percent-encoded for ONE URI segment or query value — what an SDK does
/// to an ARN travelling as a non-greedy `@httpLabel`.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// A SigV4 `Authorization` header whose credential scope names `region` and the
/// `signer` signing name: what routes the request to this service and what the
/// region of the endpoint is read from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The ARN this service stores for `name` in `region`.
fn profile_arn(region: &str, name: &str) -> String {
    format!("arn:aws:signer:{region}:{ACCOUNT_ID}:/signing-profiles/{name}")
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
        .header("authorization", authorization(region));
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
                "{method} {uri} in {region} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn put_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    request: Value,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(request),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn list_profiles(router: &MultiServiceRouter, region: &str) -> Answer {
    call(router, region, "GET", "/signing-profiles", None).await
}

/// The profile names a `ListSigningProfiles` answer reports.
fn listed_names(answer: &Answer) -> BTreeSet<String> {
    answer.body["profiles"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListSigningProfiles must report a profiles list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|profile| {
            profile["profileName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

async fn cancel_profile(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn start_job(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
    call(router, region, "POST", "/signing-jobs", Some(request)).await
}

async fn describe_job(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-jobs/{job_id}"),
        None,
    )
    .await
}

async fn list_permissions(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-profiles/{name}/permissions"),
        None,
    )
    .await
}

/// The `StartSigningJob` request body, for a job on `PROFILE_NAME`.
fn start_job_request(token: &str) -> Value {
    json!({
        "source": { "s3": { "bucketName": "src-bucket", "key": "code.bin", "version": "1" } },
        "destination": { "s3": { "bucketName": "dst-bucket", "prefix": "signed/" } },
        "profileName": PROFILE_NAME,
        "clientRequestToken": token,
    })
}

/// Create `PROFILE_NAME` in `region`, with a grant and a signing job against it,
/// and answer `(profileVersion, jobId)` — the region-local resources every other
/// region must then fail to reach.
async fn seed_region(router: &MultiServiceRouter, region: &str) -> (String, String) {
    let put = put_profile(
        router,
        region,
        PROFILE_NAME,
        json!({ "platformId": PLATFORM_ID, "tags": { "owner": region } }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "seeding {PROFILE_NAME} in {region}: expected PutSigningProfile 200, got {} {:?}",
        put.status, put.body
    );
    assert_eq!(
        put.str_member("arn"),
        profile_arn(region, PROFILE_NAME),
        "seeding {PROFILE_NAME} in {region}: unexpected profile ARN"
    );

    let granted = call(
        router,
        region,
        "POST",
        &format!("/signing-profiles/{PROFILE_NAME}/permissions"),
        Some(json!({
            "statementId": "prod-account",
            "action": "signer:StartSigningJob",
            "principal": "123456789012",
        })),
    )
    .await;
    assert_eq!(
        granted.status, 200,
        "seeding a grant in {region}: expected AddProfilePermission 200, got {} {:?}",
        granted.status, granted.body
    );

    let started = start_job(router, region, start_job_request("token-home")).await;
    assert_eq!(
        started.status, 200,
        "seeding a job in {region}: expected StartSigningJob 200, got {} {:?}",
        started.status, started.body
    );

    (
        put.str_member("profileVersion"),
        started.str_member("jobId"),
    )
}

/// One call another region's endpoint must not serve: the request an SDK sends,
/// and the `code` the refusal carries.
struct CrossRegionCall {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    code: &'static str,
}

/// Every operation in this crate that addresses a profile (or a job of one), for
/// a profile living in `home_region` with job `job_id` and revision `revision_id`.
fn cross_region_calls(home_region: &str, job_id: &str, revision_id: &str) -> Vec<CrossRegionCall> {
    let home_arn = encode(&profile_arn(home_region, PROFILE_NAME));
    vec![
        CrossRegionCall {
            case: "GetSigningProfile",
            method: "GET",
            uri: format!("/signing-profiles/{PROFILE_NAME}"),
            body: None,
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "CancelSigningProfile",
            method: "DELETE",
            uri: format!("/signing-profiles/{PROFILE_NAME}"),
            body: None,
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "RevokeSigningProfile",
            method: "PUT",
            uri: format!("/signing-profiles/{PROFILE_NAME}/revoke"),
            body: Some(json!({
                "profileVersion": "AAAAAAAAAA",
                "reason": "key-compromise",
                "effectiveTime": 1_700_000_000,
            })),
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "StartSigningJob",
            method: "POST",
            uri: "/signing-jobs".to_string(),
            body: Some(start_job_request("token-cross-region")),
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "AddProfilePermission",
            method: "POST",
            uri: format!("/signing-profiles/{PROFILE_NAME}/permissions"),
            body: Some(json!({
                "statementId": "other-region",
                "action": "signer:StartSigningJob",
                "principal": "210987654321",
            })),
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "ListProfilePermissions",
            method: "GET",
            uri: format!("/signing-profiles/{PROFILE_NAME}/permissions"),
            body: None,
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "RemoveProfilePermission",
            method: "DELETE",
            uri: format!(
                "/signing-profiles/{PROFILE_NAME}/permissions/prod-account?revisionId={}",
                encode(revision_id)
            ),
            body: None,
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "DescribeSigningJob",
            method: "GET",
            uri: format!("/signing-jobs/{job_id}"),
            body: None,
            code: ERROR_CODE_JOB_NOT_FOUND,
        },
        CrossRegionCall {
            case: "ListTagsForResource with the profile's own ARN",
            method: "GET",
            uri: format!("/tags/{home_arn}"),
            body: None,
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "TagResource with the profile's own ARN",
            method: "POST",
            uri: format!("/tags/{home_arn}"),
            body: Some(json!({ "tags": { "owner": "another-region" } })),
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        CrossRegionCall {
            case: "UntagResource with the profile's own ARN",
            method: "DELETE",
            uri: format!("/tags/{home_arn}?tagKeys=owner"),
            body: None,
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
    ]
}

/// A profile lives in the region that created it: every other region's endpoint
/// answers 404 for it, lists nothing of it, and mutates nothing of it.
#[tokio::test]
async fn a_profile_is_reachable_only_through_the_region_that_created_it() {
    for home_region in REGIONS {
        let router = emulator();
        let (home_version, job_id) = seed_region(&router, home_region).await;
        let revision_id = list_permissions(&router, home_region, PROFILE_NAME)
            .await
            .str_member("revisionId");

        for other_region in REGIONS.iter().filter(|region| **region != home_region) {
            for call_case in cross_region_calls(home_region, &job_id, &revision_id) {
                let answer = call(
                    &router,
                    other_region,
                    call_case.method,
                    &call_case.uri,
                    call_case.body.clone(),
                )
                .await;
                assert_eq!(
                    answer.status, 404,
                    "case {} for a {home_region} profile signed for {other_region}: expected \
                     HTTP 404, got {} {:?}",
                    call_case.case, answer.status, answer.body
                );
                assert_eq!(
                    answer.code(),
                    call_case.code,
                    "case {} for a {home_region} profile signed for {other_region}: expected \
                     code {}, got {:?}",
                    call_case.case,
                    call_case.code,
                    answer.body
                );
            }

            let listed = listed_names(&list_profiles(&router, other_region).await);
            assert!(
                listed.is_empty(),
                "case ListSigningProfiles signed for {other_region} with a {home_region} \
                 profile: expected no profiles, got {listed:?}"
            );
        }

        // Nothing the other regions asked for touched the profile: it is still
        // the one the home region created, at the same version and status, with
        // its tags and its grant intact — and its job is still described there.
        let home = get_profile(&router, home_region, PROFILE_NAME).await;
        assert_eq!(
            home.status, 200,
            "case {home_region} after the cross-region calls: expected GetSigningProfile 200, \
             got {} {:?}",
            home.status, home.body
        );
        for (member, expected) in [
            ("arn", profile_arn(home_region, PROFILE_NAME)),
            ("profileVersion", home_version.clone()),
            ("status", "Active".to_string()),
            ("platformId", PLATFORM_ID.to_string()),
        ] {
            assert_eq!(
                home.str_member(member),
                expected,
                "case {home_region} after the cross-region calls: expected {member} \
                 {expected}, got {:?}",
                home.body[member]
            );
        }
        assert_eq!(
            home.body["tags"],
            json!({ "owner": home_region }),
            "case {home_region} after the cross-region calls: expected the seeded tags, got {:?}",
            home.body["tags"]
        );
        let permissions = list_permissions(&router, home_region, PROFILE_NAME).await;
        assert_eq!(
            permissions.body["permissions"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {home_region} after the cross-region calls: expected the one seeded grant, \
             got {:?}",
            permissions.body
        );
        let described = describe_job(&router, home_region, &job_id).await;
        assert_eq!(
            described.status, 200,
            "case {home_region} after the cross-region calls: expected DescribeSigningJob 200, \
             got {} {:?}",
            described.status, described.body
        );
        let listed = listed_names(&list_profiles(&router, home_region).await);
        assert_eq!(
            listed,
            BTreeSet::from([PROFILE_NAME.to_string()]),
            "case {home_region} after the cross-region calls: expected only {PROFILE_NAME} \
             listed, got {listed:?}"
        );
    }
}

/// One region's profile, as this suite creates it: the region whose endpoint
/// creates it and the platform it is put on (its tag carries the region, so a
/// leaked read reports the wrong owner).
struct RegionalProfile {
    region: &'static str,
    platform_id: &'static str,
}

/// The same name in four regions, each row differing in the platform it is put on
/// so a leaked read reports the wrong platform rather than merely the wrong ARN.
fn regional_profiles() -> Vec<RegionalProfile> {
    REGIONS
        .iter()
        .enumerate()
        .map(|(index, region)| RegionalProfile {
            region,
            platform_id: if index % 2 == 0 {
                PLATFORM_ID
            } else {
                OTHER_PLATFORM_ID
            },
        })
        .collect()
}

/// The same `profileName` in two regions is two independent profiles: each keeps
/// its own ARN, version, identity, platform and tags, and a mutation in one
/// region leaves the others alone.
#[tokio::test]
async fn the_same_profile_name_in_two_regions_is_two_independent_profiles() {
    let router = emulator();
    let rows = regional_profiles();

    let mut versions = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    for row in &rows {
        let put = put_profile(
            &router,
            row.region,
            PROFILE_NAME,
            json!({
                "platformId": row.platform_id,
                "tags": { "owner": row.region },
            }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {}: expected PutSigningProfile 200, got {} {:?}",
            row.region, put.status, put.body
        );
        assert_eq!(
            put.str_member("arn"),
            profile_arn(row.region, PROFILE_NAME),
            "case {}: expected the ARN of that region, got {:?}",
            row.region,
            put.body["arn"]
        );
        assert!(
            versions.insert(put.str_member("profileVersion")),
            "case {}: expected a version no other region's profile has, got {:?}",
            row.region,
            put.body["profileVersion"]
        );
    }

    // Each region reports its OWN profile, not whichever was written last.
    for row in &rows {
        let got = get_profile(&router, row.region, PROFILE_NAME).await;
        assert_eq!(
            got.status, 200,
            "case {}: expected GetSigningProfile 200, got {} {:?}",
            row.region, got.status, got.body
        );
        assert_eq!(
            got.str_member("arn"),
            profile_arn(row.region, PROFILE_NAME),
            "case {}: expected the ARN of that region, got {:?}",
            row.region,
            got.body["arn"]
        );
        assert_eq!(
            got.str_member("platformId"),
            row.platform_id,
            "case {}: expected platformId {}, got {:?}",
            row.region,
            row.platform_id,
            got.body["platformId"]
        );
        assert_eq!(
            got.body["tags"],
            json!({ "owner": row.region }),
            "case {}: expected that region's own tags, got {:?}",
            row.region,
            got.body["tags"]
        );
        assert!(
            got.str_member("profileVersionArn")
                .starts_with(&profile_arn(row.region, PROFILE_NAME)),
            "case {}: expected a profileVersionArn under that region's profile ARN, got {:?}",
            row.region,
            got.body["profileVersionArn"]
        );
        assert!(
            tokens.insert(got.str_member("profileToken")),
            "case {}: expected a profileToken no other region's profile has, got {:?}",
            row.region,
            got.body["profileToken"]
        );
    }

    // A cancel in one region moves that region's profile only.
    let canceled_in = rows[1].region;
    let canceled = cancel_profile(&router, canceled_in, PROFILE_NAME).await;
    assert_eq!(
        canceled.status, 204,
        "case cancel in {canceled_in}: expected 204, got {} {:?}",
        canceled.status, canceled.body
    );
    for row in &rows {
        let expected = if row.region == canceled_in {
            "Canceled"
        } else {
            "Active"
        };
        let got = get_profile(&router, row.region, PROFILE_NAME).await;
        assert_eq!(
            got.str_member("status"),
            expected,
            "case {} after the cancel in {canceled_in}: expected status {expected}, got {:?}",
            row.region,
            got.body["status"]
        );
    }

    // A second create in one region is refused there — the name is taken — and
    // leaves every region's profile, including that one's, exactly as it was.
    let recreated_in = rows[2].region;
    let before: Vec<String> = {
        let mut seen = Vec::new();
        for row in &rows {
            seen.push(
                get_profile(&router, row.region, PROFILE_NAME)
                    .await
                    .str_member("profileVersion"),
            );
        }
        seen
    };
    let refused = put_profile(
        &router,
        recreated_in,
        PROFILE_NAME,
        json!({ "platformId": PLATFORM_ID }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case re-create in {recreated_in}: expected the taken name to be refused, got {} {:?}",
        refused.status, refused.body
    );
    for (row, was) in rows.iter().zip(before) {
        let now = get_profile(&router, row.region, PROFILE_NAME)
            .await
            .str_member("profileVersion");
        assert_eq!(
            now, was,
            "case {} after the refused create in {recreated_in}: expected profileVersion {was}, \
             got {now}",
            row.region
        );
    }
}

/// A signing job — and the idempotency token that started it — belongs to the
/// region it was started in.
#[tokio::test]
async fn a_signing_job_and_its_token_belong_to_the_region_that_started_it() {
    let router = emulator();

    // The same profile name exists in every region here, so a cross-region
    // describe fails because the JOB is elsewhere, not because the profile is.
    for region in REGIONS {
        let put = put_profile(
            &router,
            region,
            PROFILE_NAME,
            json!({ "platformId": PLATFORM_ID }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "seeding {PROFILE_NAME} in {region}: expected 200, got {} {:?}",
            put.status, put.body
        );
    }

    // One `clientRequestToken`, replayed in every region: it is idempotent WITHIN
    // a region and starts that region's own job in each.
    let mut jobs = Vec::new();
    for region in REGIONS {
        let started = start_job(&router, region, start_job_request("shared-token")).await;
        assert_eq!(
            started.status, 200,
            "case {region}: expected StartSigningJob 200, got {} {:?}",
            started.status, started.body
        );
        let job_id = started.str_member("jobId");
        let replayed = start_job(&router, region, start_job_request("shared-token")).await;
        assert_eq!(
            replayed.str_member("jobId"),
            job_id,
            "case {region}: expected the replayed token to answer with jobId {job_id}, got {:?}",
            replayed.body
        );
        jobs.push((region, job_id));
    }

    let distinct: BTreeSet<&String> = jobs.iter().map(|(_, job_id)| job_id).collect();
    assert_eq!(
        distinct.len(),
        jobs.len(),
        "expected one job per region for the shared token, got {jobs:?}"
    );

    for (home_region, job_id) in &jobs {
        let described = describe_job(&router, home_region, job_id).await;
        assert_eq!(
            described.status, 200,
            "case job {job_id} in {home_region}: expected DescribeSigningJob 200, got {} {:?}",
            described.status, described.body
        );
        for other_region in REGIONS.iter().filter(|region| *region != home_region) {
            let elsewhere = describe_job(&router, other_region, job_id).await;
            assert_eq!(
                elsewhere.status, 404,
                "case job {job_id} of {home_region} signed for {other_region}: expected HTTP \
                 404, got {} {:?}",
                elsewhere.status, elsewhere.body
            );
            assert_eq!(
                elsewhere.code(),
                ERROR_CODE_JOB_NOT_FOUND,
                "case job {job_id} of {home_region} signed for {other_region}: expected code \
                 {ERROR_CODE_JOB_NOT_FOUND}, got {:?}",
                elsewhere.body
            );
        }
    }
}
