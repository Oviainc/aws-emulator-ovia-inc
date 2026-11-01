// SPDX-License-Identifier: CC0-1.0

//! Every Signer success that has nothing to report is framed the SAME way,
//! whichever operation produced it.
//!
//! The rule under test: the success envelope of a call whose modelled output
//! declares no members is a property of that CLASS of answers, not of the
//! individual operation — one status (`204 No Content`), an empty body, and the
//! length of that body stated — so no member of the class is framed differently
//! from its siblings. Which class an answer belongs to is read off the modelled
//! output shape, never off the operation's name, the method it is bound to, the
//! output schema hand-written in its own file, or what the call happened to
//! change.
//!
//! Source of the class: `smithy_query(query="digest", model_path="services/\
//! service-com-amazonaws-signer-wallabyservice/smithy-model/\
//! com-amazonaws-signer-wallabyservice.json")` gives `CancelSigningProfile`,
//! `RevokeSignature` and `RevokeSigningProfile` the output `smithy.api#Unit`, and
//! `smithy_query(query="shape", shape="TagResourceResponse,UntagResourceResponse")`
//! gives both of those `members: {}`. Every other operation of this service
//! outputs a structure with members. No operation declares an
//! `@httpResponseCode`, so the status under test is not the one the route
//! resolves to.
//!
//! The rows differ along what the rule generalises over: the operation, the
//! method it is bound to (`POST`, `PUT`, `DELETE`), the sub-class its output
//! belongs to (`smithy.api#Unit` and a structure declaring no members), the size
//! of the value the request carries, which optional members it carries, and
//! whether the call changed anything at all — a first write, a repeat of the same
//! write, a repeated revocation, a removal of a key the resource never held.
//!
//! The statuses those rows answer are asserted as a SET rather than row by row,
//! because what this suite pins is the class holding together: one sibling
//! drifting onto another status — the protocol default a hand-written output
//! schema falls back to, say — is exactly the divergence the rule forbids, and
//! the failure names the sibling that drifted.
//!
//! [`a_bodyless_answer_still_recorded_what_the_call_did`] is the other half of
//! the rule: an answer with no document is not an answer that did nothing. Each
//! call of the class is read back through the reporting operation that reports its
//! effect, and those reads answer a document under a status the class does NOT
//! share.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// spelled out rather than imported so a wrong account in the handlers fails here.
const ACCOUNT_ID: &str = "000000000000";

/// A platform whose profiles need no `signingMaterial` and that publishes
/// revocation support, so one `PutSigningProfile` is enough to seed a row and a
/// revoke of its signatures is served.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The status, and the stated length, of a Signer answer with nothing to report.
///
/// Recorded behaviour: the model carries no `@httpResponseCode` anywhere in this
/// service, yet the recorded AWS reply to a call whose output declares no members
/// is `204` with no body and a stated length. The service states the class once
/// (`src/service.rs`) and every operation of it refers to that, which is the
/// property this suite drives over the wire.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// The status an answer that DOES report a document carries, so the class above
/// is not simply what every success of this service answers.
const DOCUMENT_STATUS: u16 = 200;

/// `RevocationReasonString`'s `@length` maximum, from
/// `smithy_query(query="shape", shape="RevocationReasonString")` — one row sends a
/// reason of this size, because the size of what the request carries does not
/// change the envelope of an answer that reports nothing.
const LONGEST_REASON: usize = 500;

/// The `effectiveTime` a profile revocation names, as epoch seconds.
const EFFECTIVE_TIME: i64 = 1_700_000_000;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the raw bytes, the body as JSON, and
/// the framing header this suite reads.
struct Answer {
    status: u16,
    raw_body: Vec<u8>,
    body: Value,
    content_length: Option<String>,
}

impl Answer {
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// Percent-encode everything outside the unreserved set, so an ARN travels as ONE
/// path label.
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
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", authorization);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body().to_vec();
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
    Answer {
        status,
        raw_body: bytes,
        body,
        content_length,
    }
}

/// A profile of `name`; answers the ARN and the profile version this service
/// stored for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, DOCUMENT_STATUS,
        "seeding profile {name}: expected {DOCUMENT_STATUS}, got {} {:?}",
        answer.status, answer.body
    );
    (
        answer.str_member("arn"),
        answer.str_member("profileVersion"),
    )
}

/// A signing job on `profile`; answers the job id this service minted.
async fn seed_job(router: &MultiServiceRouter, profile: &str, token: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
            "profileName": profile,
            "clientRequestToken": token,
        })),
    )
    .await;
    assert_eq!(
        answer.status, DOCUMENT_STATUS,
        "seeding a job on {profile}: expected {DOCUMENT_STATUS}, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("jobId")
}

/// A tag map as the request body carries it.
fn tags_body(pairs: &[(&str, &str)]) -> Value {
    let map: Map<String, Value> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
        .collect();
    json!({ "tags": Value::Object(map) })
}

/// One call of the class, named by the operation that serves it.
struct Row {
    case: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation whose modelled output declares no members answers the SAME
/// envelope, and the statuses the whole class answers form one value.
#[tokio::test]
async fn every_answer_with_nothing_to_report_shares_one_envelope() {
    let router = emulator();

    // One profile for the tagging rows, one to revoke, one to cancel (both are
    // terminal), and one carrying the jobs whose signatures are revoked.
    let (tagged, _) = seed_profile(&router, "One_Class_Tagged").await;
    let (_, revocable_version) = seed_profile(&router, "One_Class_Revoke").await;
    seed_profile(&router, "One_Class_Cancel").await;
    seed_profile(&router, "One_Class_Signature").await;
    let first_job = seed_job(&router, "One_Class_Signature", "tok-one-class-1").await;
    let second_job = seed_job(&router, "One_Class_Signature", "tok-one-class-2").await;

    let rows = vec![
        Row {
            case: "TagResource writing a key the profile does not hold".to_string(),
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("env", "prod")])),
        },
        Row {
            case: "TagResource writing the same pair again (a no-op write)".to_string(),
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("env", "prod")])),
        },
        Row {
            case: "TagResource replacing the value of a key the profile holds".to_string(),
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("env", "staging")])),
        },
        Row {
            case: "TagResource writing several keys at once".to_string(),
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("team", "signing"), ("owner", "platform")])),
        },
        Row {
            case: "UntagResource removing a key the profile holds".to_string(),
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=env", encode(&tagged)),
            body: None,
        },
        Row {
            case: "UntagResource removing several keys at once".to_string(),
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=team&tagKeys=owner", encode(&tagged)),
            body: None,
        },
        Row {
            case: "UntagResource naming a key the profile never held".to_string(),
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=never-written", encode(&tagged)),
            body: None,
        },
        Row {
            case: "RevokeSignature with the shortest reason the model admits".to_string(),
            method: "PUT",
            uri: format!("/signing-jobs/{first_job}/revoke"),
            body: Some(json!({ "reason": "x" })),
        },
        Row {
            case: "RevokeSignature repeated on an already revoked signature".to_string(),
            method: "PUT",
            uri: format!("/signing-jobs/{first_job}/revoke"),
            body: Some(json!({ "reason": "x" })),
        },
        Row {
            case: "RevokeSignature with the longest reason and jobOwner supplied".to_string(),
            method: "PUT",
            uri: format!("/signing-jobs/{second_job}/revoke"),
            body: Some(json!({
                "reason": "r".repeat(LONGEST_REASON),
                "jobOwner": ACCOUNT_ID,
            })),
        },
        Row {
            case: "RevokeSigningProfile".to_string(),
            method: "PUT",
            uri: "/signing-profiles/One_Class_Revoke/revoke".to_string(),
            body: Some(json!({
                "profileVersion": revocable_version,
                "reason": "rotate-signing-key",
                "effectiveTime": EFFECTIVE_TIME,
            })),
        },
        Row {
            case: "CancelSigningProfile".to_string(),
            method: "DELETE",
            uri: "/signing-profiles/One_Class_Cancel".to_string(),
            body: None,
        },
    ];

    // What each row answered, so a drifting sibling is named rather than merely
    // counted.
    let mut answered: Vec<(String, u16, Option<String>)> = Vec::new();

    for row in rows {
        let case = row.case.clone();
        let answer = call(&router, row.method, &row.uri, row.body).await;
        assert!(
            answer.raw_body.is_empty(),
            "case {case:?}: expected no body at all, got {:?}",
            String::from_utf8_lossy(&answer.raw_body)
        );
        answered.push((case, answer.status, answer.content_length));
    }

    let statuses: BTreeSet<u16> = answered.iter().map(|(_, status, _)| *status).collect();
    assert_eq!(
        statuses.len(),
        1,
        "expected every answer with nothing to report to share ONE status, got {answered:?}"
    );
    assert_eq!(
        statuses.iter().copied().next_back(),
        Some(NO_CONTENT_STATUS),
        "expected the shared status to be {NO_CONTENT_STATUS}, got {answered:?}"
    );

    let lengths: BTreeSet<Option<String>> = answered
        .iter()
        .map(|(_, _, length)| length.clone())
        .collect();
    assert_eq!(
        lengths,
        BTreeSet::from([Some(NO_CONTENT_LENGTH.to_string())]),
        "expected every such answer to state the length {NO_CONTENT_LENGTH} and nothing else, \
         got {answered:?}"
    );
}

/// The other half: an answer that reports nothing is not a call that did nothing.
///
/// Each member of the class is read back through the operation that reports its
/// effect — and those reads answer a document under a status the class does not
/// share, so the shared status above is a fact about having nothing to report.
#[tokio::test]
async fn a_bodyless_answer_still_recorded_what_the_call_did() {
    let router = emulator();
    let (tagged, _) = seed_profile(&router, "Recorded_Tagged").await;
    let (_, revocable_version) = seed_profile(&router, "Recorded_Revoke").await;
    seed_profile(&router, "Recorded_Cancel").await;
    seed_profile(&router, "Recorded_Signature").await;
    let job_id = seed_job(&router, "Recorded_Signature", "tok-recorded").await;
    let reason = "key compromise: cert rotated";

    // TagResource, read back by ListTagsForResource.
    let written = call(
        &router,
        "POST",
        &format!("/tags/{}", encode(&tagged)),
        Some(tags_body(&[("env", "prod")])),
    )
    .await;
    assert_eq!(
        written.status, NO_CONTENT_STATUS,
        "case TagResource: expected {NO_CONTENT_STATUS}, got {} {:?}",
        written.status, written.body
    );
    let listed = call(&router, "GET", &format!("/tags/{}", encode(&tagged)), None).await;
    assert_eq!(
        listed.status, DOCUMENT_STATUS,
        "case ListTagsForResource: expected {DOCUMENT_STATUS}, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["tags"]["env"],
        json!("prod"),
        "case TagResource: expected the written tag to be readable, got {:?}",
        listed.body["tags"]
    );

    // RevokeSignature, read back by DescribeSigningJob.
    let revoked_signature = call(
        &router,
        "PUT",
        &format!("/signing-jobs/{job_id}/revoke"),
        Some(json!({ "reason": reason })),
    )
    .await;
    assert_eq!(
        revoked_signature.status, NO_CONTENT_STATUS,
        "case RevokeSignature: expected {NO_CONTENT_STATUS}, got {} {:?}",
        revoked_signature.status, revoked_signature.body
    );
    let described = call(&router, "GET", &format!("/signing-jobs/{job_id}"), None).await;
    assert_eq!(
        described.status, DOCUMENT_STATUS,
        "case DescribeSigningJob: expected {DOCUMENT_STATUS}, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.body["revocationRecord"]["reason"],
        json!(reason),
        "case RevokeSignature: expected the revocation to carry the request's reason, got {:?}",
        described.body["revocationRecord"]
    );

    // RevokeSigningProfile and CancelSigningProfile, read back by
    // GetSigningProfile.
    let revoked_profile = call(
        &router,
        "PUT",
        "/signing-profiles/Recorded_Revoke/revoke",
        Some(json!({
            "profileVersion": revocable_version,
            "reason": reason,
            "effectiveTime": EFFECTIVE_TIME,
        })),
    )
    .await;
    assert_eq!(
        revoked_profile.status, NO_CONTENT_STATUS,
        "case RevokeSigningProfile: expected {NO_CONTENT_STATUS}, got {} {:?}",
        revoked_profile.status, revoked_profile.body
    );
    let read_revoked = call(&router, "GET", "/signing-profiles/Recorded_Revoke", None).await;
    assert_eq!(
        read_revoked.status, DOCUMENT_STATUS,
        "case GetSigningProfile on the revoked profile: expected {DOCUMENT_STATUS}, got {} {:?}",
        read_revoked.status, read_revoked.body
    );
    assert_eq!(
        read_revoked.str_member("status"),
        "Revoked",
        "case RevokeSigningProfile: expected the profile to read back revoked, got {:?}",
        read_revoked.body["status"]
    );
    assert_eq!(
        read_revoked.body["revocationRecord"]["revocationEffectiveFrom"].as_f64(),
        Some(EFFECTIVE_TIME as f64),
        "case RevokeSigningProfile: expected the record to carry the requested effective time, \
         got {:?}",
        read_revoked.body["revocationRecord"]
    );

    let canceled = call(&router, "DELETE", "/signing-profiles/Recorded_Cancel", None).await;
    assert_eq!(
        canceled.status, NO_CONTENT_STATUS,
        "case CancelSigningProfile: expected {NO_CONTENT_STATUS}, got {} {:?}",
        canceled.status, canceled.body
    );
    let read_canceled = call(&router, "GET", "/signing-profiles/Recorded_Cancel", None).await;
    assert_eq!(
        read_canceled.status, DOCUMENT_STATUS,
        "case GetSigningProfile on the canceled profile: expected {DOCUMENT_STATUS}, got {} {:?}",
        read_canceled.status, read_canceled.body
    );
    assert_eq!(
        read_canceled.str_member("status"),
        "Canceled",
        "case CancelSigningProfile: expected the profile to read back canceled, got {:?}",
        read_canceled.body["status"]
    );

    assert_ne!(
        NO_CONTENT_STATUS, DOCUMENT_STATUS,
        "the class under test is defined by having nothing to report, so its status must differ \
         from the status of an answer that reports a document"
    );
}
