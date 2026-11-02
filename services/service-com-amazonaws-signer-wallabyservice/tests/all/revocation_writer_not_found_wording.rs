// SPDX-License-Identifier: CC0-1.0

//! What this service's REVOCATION WRITERS say when the resource a request
//! addressed does not resolve, end to end over HTTP.
//!
//! Every case goes through the router, so it exercises routing, the
//! `@httpLabel` binding, deserialization, the handler, the persisted state and
//! serialization — the path a client takes. The rules under test:
//!
//! * a writer that revokes a SIGNATURE addresses one signing job, so its 404
//!   states the job it could not resolve: "Signing Job with id <jobId> does not
//!   exist.", the single variable part taken from the request's path label;
//! * a writer that revokes one VERSION of a signing profile addresses both
//!   halves of that resource — its `profileVersion` is `@required` — so its 404
//!   states both: "Signing Profile with name <profileName> and version
//!   <profileVersion> does not exist.", whichever half failed to resolve (no
//!   such profile at all, or a profile that never had that version), with the
//!   name taken from the path label and the version from the request member;
//! * that sentence is the service's ONE wording for a version it cannot
//!   resolve, so the sibling operation that also resolves a version of a profile
//!   words the condition identically;
//! * a refused revoke writes nothing: a real resource in the same account and
//!   region gains no revocation from an attempt that named another one.
//!
//! The cases are tables: a row per input class the rules generalise over — the
//! character make-up of an unknown profile name and of an unknown profile
//! version at the bounds the model publishes for each, a version that exists but
//! belongs to another profile, and the form of a job id that resolves to
//! nothing.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::JOB_COMPLETION_SPAN;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A SigV4 credential scope naming this service for [`REGION`]: what routes a
/// request to signer, and whose region holds the resources it addresses.
const SIGNED_FOR: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The status a revocation answers with: the modelled output is `Unit`, so the
/// reply has nothing to report.
const NO_CONTENT_STATUS: u16 = 204;

/// A published platform whose profiles need no `signingMaterial` and which
/// publishes `revocationSupported`, so a seeded profile is one call and its
/// revocation is admitted.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The `code` a 404 whose PROFILE does not exist carries. Spelled out here
/// rather than imported, so a wrong code in a handler fails this suite.
const ERROR_CODE_PROFILE_NOT_FOUND: &str = "ProfileNotFound";

/// The `code` a 404 about a sub-resource of a profile that WAS resolved carries
/// — the modelled shape's own name.
const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `code` a 404 for a signing job carries.
const ERROR_CODE_JOB_NOT_FOUND: &str = "JobNotFound";

/// An action a cross-account grant may carry. Source:
/// `AddProfilePermissionRequest$action` (`smithy_query(query="doc",
/// shape="AddProfilePermissionRequest", member="action")`).
const GRANTABLE_ACTION: &str = "signer:GetSigningProfile";

/// The account a grant is written for: `principal` is an unconstrained string an
/// account id is a valid value of.
const PRINCIPAL: &str = "123456789012";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or the empty string when there is none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// `value` percent-encoded for ONE URI segment: the members under test are
/// `@httpLabel`s admitting characters a URI cannot hold literally, so everything
/// outside the unreserved set is encoded, the way an SDK sends it.
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

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", SIGNED_FOR);
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

/// The sentence a 404 about one version of a signing profile states, built the
/// way AWS builds it: fixed text around the request's own profile name and
/// profile version.
fn version_not_found_sentence(profile_name: &str, profile_version: &str) -> String {
    format!("Signing Profile with name {profile_name} and version {profile_version} does not exist.")
}

/// The sentence a 404 about a signing job states: fixed text around the
/// request's own job id.
fn job_not_found_sentence(job_id: &str) -> String {
    format!("Signing Job with id {job_id} does not exist.")
}

/// A profile of `name`, and the `profileVersion` the create minted for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let version = answer.str_member("profileVersion");
    assert!(
        !version.is_empty(),
        "seeding profile {name}: expected a profileVersion, got {:?}",
        answer.body
    );
    version
}

async fn revoke_profile(router: &MultiServiceRouter, name: &str, profile_version: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{}/revoke", encode(name)),
        Some(json!({
            "profileVersion": profile_version,
            "reason": "probe-nonexistent-profile",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await
}

async fn add_permission(
    router: &MultiServiceRouter,
    name: &str,
    profile_version: &str,
    statement_id: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/signing-profiles/{}/permissions", encode(name)),
        Some(json!({
            "action": GRANTABLE_ACTION,
            "principal": PRINCIPAL,
            "statementId": statement_id,
            "profileVersion": profile_version,
        })),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
}

async fn revoke_signature(router: &MultiServiceRouter, job_id: &str, reason: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-jobs/{}/revoke", encode(job_id)),
        Some(json!({ "reason": reason })),
    )
    .await
}

async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-jobs/{}", encode(job_id)),
        None,
    )
    .await
}

/// Profile names no profile is held under. Rows differ along the make-up of the
/// name: `ProfileName` is `@length 2..64` and `@pattern ^[a-zA-Z0-9_]{2,}$`
/// (`smithy_query(query="operation", operation="RevokeSigningProfile")`), so
/// every row is a value the boundary admits — both published length bounds, an
/// underscored name and a digits-only one.
fn absent_profile_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest admitted name", "Ab".to_string()),
        ("an underscored name", "No_Such_Profile".to_string()),
        ("a digits-only name", "90210".to_string()),
        ("the longest admitted name", "L".repeat(64)),
    ]
}

/// `profileVersion` values a profile does not have. Rows differ along the
/// character make-up of the version: `ProfileVersion` is `@length 10..10` and
/// `@pattern ^[a-zA-Z0-9]{10}$`, so every row is exactly ten admitted
/// characters.
fn absent_profile_versions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("all upper-case letters", "AAAAAAAAAA"),
        ("all digits", "0123456789"),
        ("all lower-case letters", "qwertyuiop"),
        ("letters and digits mixed", "aB3dE5gH7j"),
    ]
}

/// A profile name no profile exists under is refused by the profile-revocation
/// writer with the one sentence that names BOTH halves of the version the
/// request addressed.
#[tokio::test]
async fn revoking_an_unknown_profile_states_the_name_and_the_version() {
    let router = emulator();

    for (name_case, name) in absent_profile_names() {
        for (version_case, version) in absent_profile_versions() {
            let case = format!("{name_case} with {version_case}");
            let refused = revoke_profile(&router, &name, version).await;
            assert_eq!(
                refused.status, 404,
                "case {case:?}: expected HTTP 404, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                ERROR_CODE_PROFILE_NOT_FOUND,
                "case {case:?}: expected code {ERROR_CODE_PROFILE_NOT_FOUND}, got {:?}",
                refused.body
            );
            let expected = version_not_found_sentence(&name, version);
            assert_eq!(
                refused.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                refused.message()
            );
        }
    }
}

/// A profile that WAS resolved and a version it has never had is refused with
/// that same sentence — the prose does not depend on which half of the addressed
/// resource was missing — and the sibling operation that also resolves a version
/// of a profile words it identically. Nothing is written by any of the refusals,
/// and the version the profile DOES have is revocable.
#[tokio::test]
async fn revoking_an_unknown_version_states_the_same_sentence_as_its_sibling() {
    let router = emulator();
    let subject_version = seed_profile(&router, "Subject_Profile").await;
    // A second profile, so one row can name a version that EXISTS and belongs to
    // another profile: a real version is still no version of this profile.
    let other_version = seed_profile(&router, "Other_Profile").await;

    let mut rows: Vec<(String, String)> = absent_profile_versions()
        .into_iter()
        .map(|(case, version)| (case.to_string(), version.to_string()))
        .collect();
    rows.push((
        "another profile's version".to_string(),
        other_version.clone(),
    ));

    for (case, version) in &rows {
        assert_ne!(
            version, &subject_version,
            "case {case:?}: the row must name a version the subject profile does not have, \
             got the minted one {version:?}"
        );

        let refused = revoke_profile(&router, "Subject_Profile", version).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            ERROR_CODE_NOT_FOUND,
            "case {case:?}: expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
            refused.body
        );
        let expected = version_not_found_sentence("Subject_Profile", version);
        assert_eq!(
            refused.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.message()
        );

        // The sibling that also resolves a version of a profile: one condition,
        // one sentence.
        let granted = add_permission(
            &router,
            "Subject_Profile",
            version,
            &format!("stmt-{case}"),
        )
        .await;
        assert_eq!(
            granted.message(),
            expected,
            "case {case:?}: expected AddProfilePermission to word the condition alike \
             ({expected:?}), got {:?}",
            granted.message()
        );
    }

    // Every refusal above wrote nothing: the profile is still Active and carries
    // no revocation record.
    let untouched = get_profile(&router, "Subject_Profile").await;
    assert_eq!(
        untouched.str_member("status"),
        "Active",
        "expected the refusals to have written nothing, got {:?}",
        untouched.body
    );
    assert!(
        untouched.body["revocationRecord"].is_null(),
        "expected the refusals to have recorded no revocation, got {:?}",
        untouched.body
    );

    // And the version the profile DOES have is revoked, read back through the
    // profile reader.
    let revoked = revoke_profile(&router, "Subject_Profile", &subject_version).await;
    assert_eq!(
        revoked.status, NO_CONTENT_STATUS,
        "revoking the minted version: expected {NO_CONTENT_STATUS}, got {} {:?}",
        revoked.status, revoked.body
    );
    let after = get_profile(&router, "Subject_Profile").await;
    assert_eq!(
        after.str_member("status"),
        "Revoked",
        "expected the revoked profile to read back Revoked, got {:?}",
        after.body
    );
    assert!(
        after.body["revocationRecord"]["revokedAt"].is_number(),
        "expected the revoked profile to carry a revocation record, got {:?}",
        after.body["revocationRecord"]
    );
}

/// Job ids that resolve to no signing job. Rows differ along the form of the id:
/// `JobId` is a bare string in the model, so a UUID nothing was minted under, a
/// one-character value, one carrying characters a path label has to encode and a
/// long one all reach the handler.
fn absent_job_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "a UUID nothing was minted under",
            "449dd784-867e-4c32-9dbc-d724d57d7fca".to_string(),
        ),
        ("a one-character id", "x".to_string()),
        ("an id needing URI encoding", "not a job/id".to_string()),
        ("a long id", "j".repeat(300)),
    ]
}

/// A job id this endpoint holds no signing job for is refused by the
/// signature-revocation writer with the one sentence that names the job it could
/// not resolve — and the attempt writes nothing: a real, Succeeded job of the
/// same account and region gains no revocation record from it, while revoking
/// THAT job does.
#[tokio::test]
async fn revoking_an_unknown_signature_states_the_job_id_and_writes_nothing() {
    let router = emulator();
    seed_profile(&router, "Job_Profile").await;
    let started = call(
        &router,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst" } },
            "profileName": "Job_Profile",
            "clientRequestToken": "tok-job-profile",
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "seeding a signing job: expected 200, got {} {:?}",
        started.status, started.body
    );
    let real_job_id = started.str_member("jobId");

    // The job the attempts below do NOT name has finished signing by the time
    // they run, so the refusals are judged against a real, settled job.
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;
    let settled = describe_job(&router, &real_job_id).await;
    assert_eq!(
        settled.str_member("status"),
        "Succeeded",
        "expected the seeded job to have Succeeded once due, got {:?}",
        settled.body
    );

    for (case, job_id) in absent_job_ids() {
        assert_ne!(
            job_id, real_job_id,
            "case {case:?}: the row must name an id no job was minted under, got {job_id:?}"
        );
        let refused = revoke_signature(&router, &job_id, "probe-nonexistent-job").await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            ERROR_CODE_JOB_NOT_FOUND,
            "case {case:?}: expected code {ERROR_CODE_JOB_NOT_FOUND}, got {:?}",
            refused.body
        );
        let expected = job_not_found_sentence(&job_id);
        assert_eq!(
            refused.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.message()
        );

        // The real job is untouched by an attempt that named another id.
        let described = describe_job(&router, &real_job_id).await;
        assert!(
            described.body["revocationRecord"].is_null(),
            "case {case:?}: expected the real job to gain no revocationRecord, got {:?}",
            described.body["revocationRecord"]
        );
    }

    // Revoking the job that DOES exist records the revocation, read back through
    // the job reader.
    let revoked = revoke_signature(&router, &real_job_id, "key compromise").await;
    assert_eq!(
        revoked.status, NO_CONTENT_STATUS,
        "revoking the real job: expected {NO_CONTENT_STATUS}, got {} {:?}",
        revoked.status, revoked.body
    );
    let described = describe_job(&router, &real_job_id).await;
    assert_eq!(
        described.body["revocationRecord"]["reason"],
        json!("key compromise"),
        "expected the real job's record to carry the request's reason, got {:?}",
        described.body["revocationRecord"]
    );
}
