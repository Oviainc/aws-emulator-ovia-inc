// SPDX-License-Identifier: CC0-1.0

//! `GetRevocationStatus` across the two emulated Signer services, over the WHOLE
//! router.
//!
//! The rule under test: a revocation written through the Signer CONTROL plane —
//! `RevokeSignature` on a signing job, `RevokeSigningProfile` on a profile version —
//! is reported by the service that serves `GET /revocations`, the route Signer's own
//! model binds to the `data-` endpoint host. The control plane holds those records,
//! so it claims the shared `signer` signing name whole: a request signed for `signer`
//! reaches it whichever of the two endpoint hosts an SDK sent it to, and the answer is
//! the same facts either way. The data-plane crate
//! (`services/service-com-amazonaws-signerdata-signerdataplane`) claims the key of its
//! OWN endpoint, which only a request carrying no credential scope resolves to, so no
//! request has two claimants.
//!
//! A single-service test cannot see this: the claim split only exists when both
//! services are mounted, so the whole router is assembled here, on a sized thread (see
//! `boot_serve.rs`), and every request is driven through the same `dispatch` the
//! production front door uses.
//!
//! The cases span what the rule generalises over: which entity carries the revocation
//! (a signing job's signature, a profile version, both), which endpoint host the read
//! is addressed to, and where the signature's timestamp falls relative to the instant
//! a profile's revocation becomes effective.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::Value;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request signs for and addresses.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents, spelled out so a wrong account
/// in a built ARN fails here.
const ACCOUNT_ID: &str = "000000000000";

/// The epoch second the profile revocation below becomes effective, and the instant
/// the reads hang their offsets off.
const EFFECTIVE_EPOCH_SECS: i64 = 1_700_000_000;

/// A platform the published catalog carries that supports revocation.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The status a revoking call answers with: its modelled output is
/// `smithy.api#Unit`, so the reply has nothing to report and is `204 No Content`.
const NO_CONTENT_STATUS: u16 = 204;

/// A SigV4 `Authorization` header whose credential scope names `signer`. The signature
/// is never checked: this router runs with request authentication off.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The Signer control plane's endpoint host.
fn control_plane_host() -> String {
    format!("signer.{REGION}.amazonaws.com")
}

/// The endpoint host Signer's model binds `GetRevocationStatus` to
/// (`@endpoint(hostPrefix: "data-")`), which is where an SDK sends it.
fn data_plane_host() -> String {
    format!("data-signer.{REGION}.amazonaws.com")
}

/// One request: method, host, path-and-query, body, and whether it is signed.
struct Call<'a> {
    case: &'a str,
    method: &'a str,
    host: String,
    uri: String,
    body: String,
    signed: bool,
}

/// What a request answered: the status and the body as JSON (`Value::Null` for an
/// empty body).
struct Answer {
    status: u16,
    body: Value,
    text: String,
}

impl Answer {
    /// A string member of the body, or the empty string when it carries none.
    fn member(&self, name: &str) -> String {
        self.body[name].as_str().unwrap_or_default().to_string()
    }

    /// The `revokedEntities` list, which every 200 carries.
    fn revoked_entities(&self, case: &str) -> Vec<String> {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            self.status, self.text
        );
        let list = self.body["revokedEntities"].as_array().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a revokedEntities list, got {}",
                self.text
            )
        });
        list.iter()
            .map(|entry| {
                entry
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected every revoked entity to be a string, got {}",
                            self.text
                        )
                    })
                    .to_string()
            })
            .collect()
    }
}

/// Send one [`Call`] through the whole router.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    call: &Call<'_>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(format!("https://{}{}", call.host, call.uri))
        .header("host", &call.host);
    if call.signed {
        builder = builder.header("authorization", authorization());
    }
    if !call.body.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(bytes::Bytes::from(call.body.clone()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };

    // No row may provoke either routing failure: `AmbiguousService` (more than one
    // service claimed it) or `UnknownService` (none did).
    let wire_code = body["__type"].as_str().unwrap_or_default();
    assert!(
        wire_code != "AmbiguousService" && wire_code != "UnknownService",
        "case {:?}: expected exactly one claiming service, got HTTP {status}: {text}",
        call.case
    );

    Answer { status, body, text }
}

/// The `GET /revocations` query string: every one of the five `@required` +
/// `@httpQuery` members of `GetRevocationStatus`.
fn revocations_query(
    signature_epoch_secs: i64,
    profile_version_arn: &str,
    job_arn: &str,
    certificate_hash: &str,
) -> String {
    // An `@httpQuery`-bound timestamp travels in the ISO 8601 `date-time` form by
    // default, rendered by the maintained type rather than laid out here.
    let signature_timestamp = aws_smithy_types::DateTime::from_secs(signature_epoch_secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an epoch-based instant renders as date-time");
    format!(
        "/revocations?signatureTimestamp={signature_timestamp}&platformId={PLATFORM_ID}\
         &profileVersionArn={profile_version_arn}&jobArn={job_arn}\
         &certificateHashes={certificate_hash}"
    )
}

#[test]
fn a_revocation_written_through_the_control_plane_is_reported_at_both_signer_endpoints() {
    std::thread::Builder::new()
        .name("signer-revocation-status".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let profile_name = "Revocation_Status_Profile";
            let put = send(
                &router,
                &runtime,
                &Call {
                    case: "PutSigningProfile",
                    method: "PUT",
                    host: control_plane_host(),
                    uri: format!("/signing-profiles/{profile_name}"),
                    body: format!(
                        "{{\"platformId\":\"{PLATFORM_ID}\",\"signingMaterial\":\
                         {{\"certificateArn\":\"arn:aws:acm:{REGION}:{ACCOUNT_ID}:\
                         certificate/abc\"}}}}"
                    ),
                    signed: true,
                },
            );
            assert_eq!(
                put.status, 200,
                "case \"PutSigningProfile\": expected HTTP 200, got {} {}",
                put.status, put.text
            );
            let profile_version = put.member("profileVersion");
            let profile_version_arn = put.member("profileVersionArn");
            assert!(
                !profile_version_arn.is_empty(),
                "case \"PutSigningProfile\": expected a profileVersionArn, got {}",
                put.text
            );

            let started = send(
                &router,
                &runtime,
                &Call {
                    case: "StartSigningJob",
                    method: "POST",
                    host: control_plane_host(),
                    uri: "/signing-jobs".to_string(),
                    body: format!(
                        "{{\"source\":{{\"s3\":{{\"bucketName\":\"src\",\"key\":\"code.bin\",\
                         \"version\":\"1\"}}}},\"destination\":{{\"s3\":\
                         {{\"bucketName\":\"dst\"}}}},\"profileName\":\"{profile_name}\",\
                         \"clientRequestToken\":\"revocation-status-token\"}}"
                    ),
                    signed: true,
                },
            );
            assert_eq!(
                started.status, 200,
                "case \"StartSigningJob\": expected HTTP 200, got {} {}",
                started.status, started.text
            );
            let job_id = started.member("jobId");
            assert!(
                !job_id.is_empty(),
                "case \"StartSigningJob\": expected a jobId, got {}",
                started.text
            );
            // A signing job is named by ARN only here, in `GetRevocationStatus$jobArn`:
            // `arn:aws:signer:<region>:<account>:/signing-jobs/<jobId>`, the resource
            // form Signer's profile ARNs carry.
            let job_arn = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/{job_id}");
            // A certificate hash, in the composite hex form the member documents. No
            // operation revokes a certificate, so no read below reports it.
            let certificate_hash = "ab".repeat(96);

            // Nothing is revoked yet, at either endpoint host.
            for (case, host) in [
                ("before any revoke, at the signer host", control_plane_host()),
                (
                    "before any revoke, at the data-signer host",
                    data_plane_host(),
                ),
            ] {
                let answer = send(
                    &router,
                    &runtime,
                    &Call {
                        case,
                        method: "GET",
                        host,
                        uri: revocations_query(
                            EFFECTIVE_EPOCH_SECS,
                            &profile_version_arn,
                            &job_arn,
                            &certificate_hash,
                        ),
                        body: String::new(),
                        signed: true,
                    },
                );
                let reported = answer.revoked_entities(case);
                assert!(
                    reported.is_empty(),
                    "case {case:?}: nothing has been revoked, got {reported:?}"
                );
            }

            // Revoke the SIGNATURE through the control plane.
            let revoked = send(
                &router,
                &runtime,
                &Call {
                    case: "RevokeSignature",
                    method: "PUT",
                    host: control_plane_host(),
                    uri: format!("/signing-jobs/{job_id}/revoke"),
                    body: "{\"reason\":\"the signing key was compromised\"}".to_string(),
                    signed: true,
                },
            );
            assert_eq!(
                revoked.status, NO_CONTENT_STATUS,
                "case \"RevokeSignature\": expected HTTP {NO_CONTENT_STATUS}, got {} {}",
                revoked.status, revoked.text
            );

            // Both endpoint hosts report it: a request signed for `signer` is served by
            // the service holding the records whichever host an SDK addressed.
            for (case, host) in [
                ("the revoked signature, at the signer host", control_plane_host()),
                (
                    "the revoked signature, at the data-signer host",
                    data_plane_host(),
                ),
            ] {
                let answer = send(
                    &router,
                    &runtime,
                    &Call {
                        case,
                        method: "GET",
                        host,
                        uri: revocations_query(
                            EFFECTIVE_EPOCH_SECS,
                            &profile_version_arn,
                            &job_arn,
                            &certificate_hash,
                        ),
                        body: String::new(),
                        signed: true,
                    },
                );
                assert_eq!(
                    answer.revoked_entities(case),
                    vec![job_arn.clone()],
                    "case {case:?}: expected the revoked signature's job ARN, got {}",
                    answer.text
                );
            }

            // Revoke the PROFILE VERSION too, effective from a named instant.
            let revoked = send(
                &router,
                &runtime,
                &Call {
                    case: "RevokeSigningProfile",
                    method: "PUT",
                    host: control_plane_host(),
                    uri: format!("/signing-profiles/{profile_name}/revoke"),
                    body: format!(
                        "{{\"profileVersion\":\"{profile_version}\",\"reason\":\
                         \"the profile's material is no longer trusted\",\
                         \"effectiveTime\":{EFFECTIVE_EPOCH_SECS}}}"
                    ),
                    signed: true,
                },
            );
            assert_eq!(
                revoked.status, NO_CONTENT_STATUS,
                "case \"RevokeSigningProfile\": expected HTTP {NO_CONTENT_STATUS}, got {} {}",
                revoked.status, revoked.text
            );

            // (case, the signature's instant, what the read must report)
            let rows: Vec<(&str, i64, Vec<String>)> = vec![
                (
                    "a signature made before the profile revocation became effective",
                    EFFECTIVE_EPOCH_SECS - 1,
                    vec![job_arn.clone()],
                ),
                (
                    "a signature made at the instant it became effective",
                    EFFECTIVE_EPOCH_SECS,
                    vec![profile_version_arn.clone(), job_arn.clone()],
                ),
                (
                    "a signature made a day after it became effective",
                    EFFECTIVE_EPOCH_SECS + 86_400,
                    vec![profile_version_arn.clone(), job_arn.clone()],
                ),
            ];
            for (case, signature_epoch_secs, expected) in rows {
                let answer = send(
                    &router,
                    &runtime,
                    &Call {
                        case,
                        method: "GET",
                        host: data_plane_host(),
                        uri: revocations_query(
                            signature_epoch_secs,
                            &profile_version_arn,
                            &job_arn,
                            &certificate_hash,
                        ),
                        body: String::new(),
                        signed: true,
                    },
                );
                let reported = answer.revoked_entities(case);
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected {expected:?}, got {reported:?}"
                );
                assert!(
                    !reported.contains(&certificate_hash),
                    "case {case:?}: no certificate hash can be revoked, got {reported:?}"
                );
            }

            // The data-plane crate still answers the requests addressed at its own
            // endpoint key — one signed for no service at all, which only the host
            // names — so the two claims remain disjoint and that service keeps serving
            // its own traffic.
            let host_addressed = send(
                &router,
                &runtime,
                &Call {
                    case: "unsigned, host-addressed at the data-signer endpoint",
                    method: "GET",
                    host: data_plane_host(),
                    uri: revocations_query(
                        EFFECTIVE_EPOCH_SECS,
                        &profile_version_arn,
                        &job_arn,
                        &certificate_hash,
                    ),
                    body: String::new(),
                    signed: false,
                },
            );
            assert_eq!(
                host_addressed.status, 200,
                "case \"unsigned, host-addressed at the data-signer endpoint\": expected \
                 HTTP 200, got {} {}",
                host_addressed.status, host_addressed.text
            );
            assert!(
                host_addressed.body["revokedEntities"].is_array(),
                "case \"unsigned, host-addressed at the data-signer endpoint\": expected a \
                 revokedEntities list, got {}",
                host_addressed.text
            );
        })
        .expect("spawn the revocation-status thread")
        .join()
        .expect("a control-plane revocation is reported by whoever serves /revocations");
}
