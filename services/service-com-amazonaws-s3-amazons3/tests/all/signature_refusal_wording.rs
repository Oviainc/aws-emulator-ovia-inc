// SPDX-License-Identifier: CC0-1.0

//! How this service words a request whose SIGNATURE did not verify.
//!
//! Every assertion drives a real HTTP request through a router with request
//! authentication ON — as the emulator runs it by default — so what is pinned is the wire
//! behaviour a client sees: the status, the `<Code>` and the `<Message>` of S3's error
//! document.
//!
//! The rule under test: a request presenting a credential this emulator ISSUED, whose
//! signature does not verify under that credential's secret, is refused HTTP 403 with the
//! code `SignatureDoesNotMatch` and the fixed sentence the SigV4 layer produces for that one
//! condition — "The request signature we calculated does not match the signature you
//! provided. Check your key and signing method." A CLI, an SDK and a signing runbook all
//! match on that text, so an abbreviated sentence makes a signing failure undiagnosable.
//!
//! The rule is about the SIGNATURE, so its boundary is pinned too: a credential the
//! emulator never issued was refused for the credential rather than for the signature, and
//! keeps the refusal the boundary already words for it.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State;
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;

/// The sentence AWS's SigV4 layer produces for a signature that does not verify. Fixed
/// text: it names neither the request, the key nor the signature.
///
/// `pub(crate)`, like the harness below it: the suite that pins the DIAGNOSTICS this same
/// refusal carries ([`super::presigned_query_authentication`]) asserts the same sentence,
/// and one constant for both keeps them from drifting.
pub(crate) const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check \
     your key and signing method.";

/// The signing algorithm a SigV4 request names, in its `Authorization` header and in the
/// `X-Amz-Algorithm` parameter of a presigned URL.
pub(crate) const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// A signature value that is not the one the emulator computes for any of these requests:
/// 64 hex digits, the shape SigV4 signatures have, so what the request fails is the
/// comparison and not the parse.
pub(crate) const WRONG_SIGNATURE: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// One request as a row names it: the URI it addresses and the headers it carries.
type Presented = (String, Vec<(String, String)>);

/// An emulator with request authentication ON, plus the KEY PAIR it published for this run —
/// the credential a refusal has RESOLVED, and the one a presigned URL is really signed with.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) access_key_id: String,
    /// The secret half of that pair, for a suite that must produce a signature the emulator
    /// VERIFIES rather than one it refuses ([`super::presigned_url_serving`]).
    pub(crate) secret_access_key: String,
    /// The shared engine, so a suite can take a SESSION on a directory bucket and then
    /// present that credential — a second credential form the emulator ISSUED.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(true);
        let (access_key_id, secret_access_key) = match router.vended_credentials().into_iter().next()
        {
            Some(emulator_server::VendedCredentials::AwsAccessKey {
                access_key_id,
                secret_access_key,
            }) => (access_key_id, secret_access_key),
            other => panic!(
                "with authentication on the emulator must vend a SigV4 key pair, got {other:?}"
            ),
        };
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            access_key_id,
            secret_access_key,
            engine,
        }
    }

    /// Turn request authentication OFF, so a fixture can be created without signing it —
    /// which is not what this suite is about — and ON again for the request under test.
    pub(crate) fn enforce_signatures(&mut self, enabled: bool) {
        self.router.set_request_authentication(enabled);
    }

    /// Dispatch one request and reduce the answer the way the other suites here do.
    pub(crate) async fn send(&self, method: &str, uri: &str, headers: &[(String, String)]) -> Answer {
        self.send_body(method, uri, headers, b"").await
    }

    /// [`Self::send`] with a body, for the fixture requests that carry one.
    pub(crate) async fn send_body(
        &self,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Answer {
        self.fetch(
            REGIONAL_ENDPOINT_HOST,
            method,
            &format!("https://{REGIONAL_ENDPOINT_HOST}{uri}"),
            headers,
            body,
        )
        .await
    }

    /// Dispatch one request at the absolute `url`, carrying `host` as the host it was
    /// addressed to — which is the host a signature covers, so a suite whose case is the
    /// ADDRESS form drives its requests through here.
    pub(crate) async fn fetch(
        &self,
        host: &str,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Answer {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(url)
            .header("host", host);
        for (name, value) in headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let request = builder
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }
}

/// The endpoint every request this harness sends is addressed to unless the case names
/// another: the Regional one, which carries no bucket, so the request is PATH-STYLE.
pub(crate) const REGIONAL_ENDPOINT_HOST: &str = "s3.us-east-1.amazonaws.com";

/// `YYYYMMDDTHHMMSSZ` for this instant, as SigV4 writes a signing date — rendered by the
/// library's own calendar (`DateTime::fmt`, RFC 3339 in UTC) with the separators SigV4
/// omits removed, rather than by any date arithmetic written here.
pub(crate) fn amz_date_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_secs();
    DateTime::from_secs(seconds as i64)
        .fmt(Format::DateTime)
        .expect("an instant from the clock must render")
        .replace(['-', ':'], "")
}

/// The credential scope a request signed now for this service names:
/// `<key>/<date>/<region>/s3/aws4_request`.
pub(crate) fn credential_scope(access_key_id: &str, amz_date: &str) -> String {
    let date = &amz_date[..8];
    format!("{access_key_id}/{date}/us-east-1/s3/aws4_request")
}

/// The `Authorization` header a SigV4-signed request carries, with `signature` as its
/// `Signature` field.
pub(crate) fn authorization(access_key_id: &str, amz_date: &str, signature: &str) -> String {
    format!(
        "{SIGV4_ALGORITHM} Credential={}, SignedHeaders=host, Signature={signature}",
        credential_scope(access_key_id, amz_date)
    )
}

/// The query of a presigned URL, percent-encoded the way a client's signer writes it, with
/// `expires` seconds of validity and `signature` as its `X-Amz-Signature`.
///
/// `expires` is the TEXT the URL carries rather than a count, because a window a client can
/// put in a URL is not always one: the suite that pins the accepted range
/// ([`super::presigned_query_authentication`]) drives values no counter holds.
///
/// The pairs are written in the order SigV4's canonical query string sorts them, with the
/// signature — the one pair a signature cannot cover — last.
pub(crate) fn presigned_query(
    access_key_id: &str,
    amz_date: &str,
    expires: &str,
    signature: &str,
) -> String {
    let scope = credential_scope(access_key_id, amz_date).replace('/', "%2F");
    format!(
        "X-Amz-Algorithm={SIGV4_ALGORITHM}&X-Amz-Credential={scope}&X-Amz-Date={amz_date}\
         &X-Amz-Expires={expires}&X-Amz-SignedHeaders=host&X-Amz-Signature={signature}"
    )
}

/// RULE: every request whose signature does not verify under a credential the emulator
/// issued is refused 403 `SignatureDoesNotMatch` with AWS's own sentence.
///
/// The rows differ along what the rule generalises over: how the credential travelled (an
/// `Authorization` header, a presigned query, and a presigned query whose signed
/// `X-Amz-Expires` was altered after signing), the operation addressed (a read of an
/// object, a write of one, a read of a bucket subresource, a listing of the account's
/// buckets), and whether the bucket named exists at all — a refusal at the boundary never
/// reaches a handler, so it cannot depend on the resource.
#[tokio::test]
async fn an_unverifiable_signature_is_refused_in_aws_own_words() {
    let emulator = Emulator::new();
    let amz_date = amz_date_now();
    let key_id = emulator.access_key_id.clone();

    let header_signed = |uri: &str| {
        (
            uri.to_string(),
            vec![
                (
                    "authorization".to_string(),
                    authorization(&key_id, &amz_date, WRONG_SIGNATURE),
                ),
                ("x-amz-date".to_string(), amz_date.clone()),
            ],
        )
    };
    let presigned = |uri: &str, expires: &str| {
        let separator = if uri.contains('?') { '&' } else { '?' };
        (
            format!(
                "{uri}{separator}{}",
                presigned_query(&key_id, &amz_date, expires, WRONG_SIGNATURE)
            ),
            Vec::new(),
        )
    };

    let rows: [(&str, &str, Presented); 6] = [
        (
            "a header-signed GetObject",
            "GET",
            header_signed("/signature-bucket/object.txt"),
        ),
        (
            "a header-signed PutObject",
            "PUT",
            header_signed("/signature-bucket/written.txt"),
        ),
        (
            "a header-signed read of a bucket subresource",
            "GET",
            header_signed("/signature-bucket?acl"),
        ),
        (
            "a header-signed ListBuckets at the service root",
            "GET",
            header_signed("/"),
        ),
        (
            "a presigned GetObject",
            "GET",
            presigned("/signature-bucket/object.txt", "300"),
        ),
        (
            // The recorded instance: a presigned GET whose signed `X-Amz-Expires` is
            // doubled after signing, which the signature covers — so the URL no longer
            // verifies even though its `X-Amz-Signature` is untouched.
            "a presigned GetObject whose X-Amz-Expires was doubled after signing",
            "GET",
            presigned("/signature-bucket/object.txt", "600"),
        ),
    ];

    for (case, method, (uri, headers)) in rows {
        let refused = emulator.send(method, &uri, &headers).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "SignatureDoesNotMatch",
            "case {case:?}: expected the SignatureDoesNotMatch code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            SIGNATURE_MISMATCH_MESSAGE,
            "case {case:?}: expected AWS's signature-mismatch sentence, got {:?}",
            refused.error_message()
        );
    }
}

/// RULE (the boundary of the one above): a credential this emulator never ISSUED was
/// refused for the credential, not for the signature, so it is NOT answered
/// `SignatureDoesNotMatch` — the code points an operator at the secret half of a pair that
/// exists.
#[tokio::test]
async fn a_credential_the_emulator_never_issued_is_not_a_signature_mismatch() {
    let emulator = Emulator::new();
    let amz_date = amz_date_now();

    let rows: [(&str, Presented); 3] = [
        (
            "an access key id that was never issued",
            (
            "/signature-bucket/object.txt".to_string(),
            vec![
                (
                    "authorization".to_string(),
                    authorization("AKIDNOTISSUEDBYTHISEMULATOR", &amz_date, WRONG_SIGNATURE),
                ),
                ("x-amz-date".to_string(), amz_date.clone()),
            ],
            ),
        ),
        (
            "a request carrying no SigV4 credential at all",
            (
                format!(
                    "/signature-bucket/object.txt?X-Amz-Credential={}",
                    credential_scope("AKIDNOTISSUEDBYTHISEMULATOR", &amz_date)
                        .replace('/', "%2F")
                ),
                Vec::new(),
            ),
        ),
        (
            // A presigned URL whose validity window has passed is refused for the EXPIRY,
            // with its signature intact — a different condition, which AWS words
            // differently, so it must not be re-worded as a signature mismatch even though
            // the credential it presents IS one the emulator issued.
            "a presigned URL whose validity window has passed",
            (
                format!(
                    "/signature-bucket/object.txt?{}",
                    presigned_query(
                        &emulator.access_key_id,
                        "20200101T000000Z",
                        "300",
                        WRONG_SIGNATURE
                    )
                ),
                Vec::new(),
            ),
        ),
    ];

    for (case, (uri, headers)) in rows {
        let refused = emulator.send("GET", &uri, &headers).await;
        assert_ne!(
            refused.error_code(),
            "SignatureDoesNotMatch",
            "case {case:?}: a refusal that is not a signature comparison must not be reported \
             as a signature mismatch, got status {} and body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.status >= 400,
            "case {case:?}: an unverifiable request must be refused, got {}",
            refused.status
        );
    }
}

/// RULE (the other credential form the emulator ISSUES): a request signed with a SESSION
/// credential `CreateSession` vended, whose signature does not verify, is refused with the
/// same AWS sentence — the rule is about the signature, not about which kind of credential
/// carried it.
#[tokio::test]
async fn a_session_credential_with_a_bad_signature_is_a_signature_mismatch() {
    let mut emulator = Emulator::new();

    // The fixture — a directory bucket and a session on it — is created UNSIGNED, which is
    // not what this suite is about; the request under test is the signed one below.
    emulator.enforce_signatures(false);
    let created = emulator
        .send_body(
            "PUT",
            "/session--use1-az4--x-s3",
            &[("content-type".to_string(), "application/xml".to_string())],
            b"<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
              <Location><Type>AvailabilityZone</Type><Name>use1-az4</Name></Location>\
              <Bucket><Type>Directory</Type></Bucket></CreateBucketConfiguration>",
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the directory bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let session = emulator
        .send("GET", "/session--use1-az4--x-s3?session", &[])
        .await;
    assert_eq!(
        session.status, 200,
        "the session must be vended first, got {} with body {:?}",
        session.status,
        String::from_utf8_lossy(&session.body)
    );
    let session_key_id = session
        .element("AccessKeyId")
        .filter(|id| !id.is_empty())
        .expect("the session must carry an access key id");
    assert!(
        emulator
            .engine
            .state::<ComAmazonawsS3Amazons3State>()
            .lock()
            .holds_vended_session(&session_key_id),
        "the store must hold the session it vended, {session_key_id:?}"
    );

    emulator.enforce_signatures(true);
    let amz_date = amz_date_now();
    let refused = emulator
        .send(
            "GET",
            "/session--use1-az4--x-s3/object.txt",
            &[
                (
                    "authorization".to_string(),
                    authorization(&session_key_id, &amz_date, WRONG_SIGNATURE),
                ),
                ("x-amz-date".to_string(), amz_date.clone()),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 403,
        "case \"a session credential with a bad signature\": expected HTTP 403, got {} with \
         body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_code(),
        "SignatureDoesNotMatch",
        "case \"a session credential with a bad signature\": expected the \
         SignatureDoesNotMatch code, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.error_message(),
        SIGNATURE_MISMATCH_MESSAGE,
        "case \"a session credential with a bad signature\": expected AWS's \
         signature-mismatch sentence, got {:?}",
        refused.error_message()
    );
}
