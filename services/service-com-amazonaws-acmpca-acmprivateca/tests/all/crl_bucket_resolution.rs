// SPDX-License-Identifier: CC0-1.0

//! The CRL configuration a private CA is created or updated with, resolved against the
//! emulator's OWN object storage.
//!
//! The RULE under test: an enabled `CrlConfiguration` names the S3 bucket the CA writes its
//! CRLs to, so that bucket is resolved when the configuration is submitted — a bucket the
//! account holds is accepted and the CA keeps the configuration, while one no bucket answers
//! to is refused 400 `ValidationException` and NOTHING is created or changed. The same holds
//! whichever call carries the configuration (the create and the update both do), and a
//! configuration with CRLs disabled names no bucket to resolve.
//!
//! So this suite mounts the S3 service beside this one on ONE engine — what the assembled
//! emulator does — and creates its buckets through S3's own front door, so what the refusal
//! is grounded in is S3's answer rather than anything this test seeded.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    CRL_BUCKET_ABSENT_MESSAGE, SETTLE_AFTER,
};
use service_com_amazonaws_acmpca_acmprivateca::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::private_ca_lifecycle::{
    blob, certificate_pem, root_ca_request, string, AUTHORIZATION, REGION,
};
use crate::resource_policy::error_message;

/// A SigV4 `Authorization` header in the `s3` scope — the routing signal that picks the S3
/// service for a request to the bucket front door, in the same Region this suite's private
/// CA calls are signed for.
pub(crate) const S3_AUTHORIZATION: &str =
    "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTTESTTEST/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=deadbeef";

/// An emulator holding this service AND S3 over one shared engine.
///
/// `pub(crate)` so a sibling suite that needs BOTH services — the private CA and the object
/// storage its CRLs are published to — drives the same two-service router rather than
/// growing a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// Call one private-CA operation over its awsJson1_1 wire.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://acm-pca.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("host", format!("acm-pca.{REGION}.amazonaws.com"))
            .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// One request against the emulator's S3 front door, path-style (the emulator serves
    /// one endpoint per Region, so the bucket travels in the path): `path` is everything
    /// after the host, headers are `(name, value)` pairs, and the answer is the status with
    /// the response body as it came.
    pub(crate) async fn s3(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com/{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", S3_AUTHORIZATION);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(bytes::Bytes::from(body))
            .expect("the S3 request builds");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            response.body().as_ref().to_vec(),
        )
    }

    /// `PUT /<bucket>` against the emulator's S3 front door — `CreateBucket`, with the
    /// location constraint a bucket outside `us-east-1` is created under, and the Object
    /// Ownership setting the bucket comes into being with when one is named.
    pub(crate) async fn create_bucket_owned(
        &self,
        bucket: &str,
        object_ownership: Option<&str>,
    ) -> u16 {
        let body = format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        );
        let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/xml")];
        if let Some(ownership) = object_ownership {
            headers.push(("x-amz-object-ownership", ownership));
        }
        self.s3("PUT", bucket, &headers, body.into_bytes()).await.0
    }

    /// `CreateBucket` for a bucket with the settings S3 gives a new one.
    pub(crate) async fn create_bucket(&self, bucket: &str) -> u16 {
        self.create_bucket_owned(bucket, None).await
    }
}

/// The revocation configuration a row submits: CRLs enabled for `bucket`, expiring after
/// `days`.
fn crl_configuration(bucket: &str, days: i32) -> Value {
    json!({
        "CrlConfiguration": {
            "Enabled": true,
            "S3BucketName": bucket,
            "ExpirationInDays": days,
        }
    })
}

/// The `CertificateAuthorities` this emulator holds, as the ARNs they are keyed by.
async fn listed_cas(emulator: &Emulator, case: &str) -> Vec<String> {
    let answer = emulator.ok(case, "ListCertificateAuthorities", &json!({})).await;
    answer
        .get("CertificateAuthorities")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|authority| string(authority, "Arn", case))
        .collect()
}

/// A CRL configuration naming a bucket the account holds is accepted and kept; one naming a
/// bucket nothing answers to is refused and no CA is created.
#[tokio::test]
async fn a_ca_is_created_only_for_a_crl_bucket_the_account_holds() {
    let emulator = Emulator::new();

    // Bucket names differing along what the rule generalises over: length, and the
    // characters an S3 bucket name may carry (hyphens, digits, dots). Each is created
    // through S3, so this service's acceptance is grounded in S3 holding it.
    for bucket in ["crl", "acme-widgets-crl-publication", "crl.acme-widgets-01"] {
        let created = emulator.create_bucket(bucket).await;
        assert_eq!(
            created, 200,
            "bucket {bucket}: expected CreateBucket 200, got {created}"
        );
    }

    // Rows: the bucket the configuration names, the CRL lifetime it asks for, and whether
    // object storage holds that bucket. The lifetimes span the published range's ends.
    let rows: [(&str, &str, i32, bool); 6] = [
        ("a short bucket name the account holds", "crl", 7, true),
        (
            "a hyphenated bucket name the account holds, at the range floor",
            "acme-widgets-crl-publication",
            1,
            true,
        ),
        (
            "a dotted bucket name the account holds, at the range ceiling",
            "crl.acme-widgets-01",
            5000,
            true,
        ),
        ("a bucket nothing answers to", "no-such-crl-bucket", 7, false),
        (
            "a bucket whose name only resembles one that exists",
            "crl-publication",
            30,
            false,
        ),
        (
            "a bucket nothing answers to, at the range ceiling",
            "another-absent-bucket",
            5000,
            false,
        ),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (index, (case, bucket, days, held)) in rows.iter().enumerate() {
        let mut body = root_ca_request(&format!("crl-root-{index}"));
        body["RevocationConfiguration"] = crl_configuration(bucket, *days);

        if *held {
            let created = emulator.ok(case, "CreateCertificateAuthority", &body).await;
            let arn = string(&created, "CertificateAuthorityArn", case);
            accepted.push(arn.clone());

            // The CA kept the configuration it was created with, read back through the
            // service's own description of it.
            let described = emulator
                .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
                .await;
            let crl = described
                .get("CertificateAuthority")
                .and_then(|authority| authority.get("RevocationConfiguration"))
                .and_then(|revocation| revocation.get("CrlConfiguration"))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                crl.get("S3BucketName").and_then(Value::as_str),
                Some(*bucket),
                "case {case:?}: expected the CA to keep bucket {bucket:?}, got {crl:?}"
            );
            assert_eq!(
                crl.get("ExpirationInDays").and_then(Value::as_i64),
                Some(i64::from(*days)),
                "case {case:?}: expected the CA to keep {days} day(s), got {crl:?}"
            );
            continue;
        }

        let (status, answer) = emulator.call("CreateCertificateAuthority", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} {answer:?}"
        );
        let code = answer
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?} ({answer:?})"
        );
        let message = error_message(&answer, case);
        assert_eq!(
            message, CRL_BUCKET_ABSENT_MESSAGE,
            "case {case:?}: expected {CRL_BUCKET_ABSENT_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(bucket),
            "case {case:?}: the sentence names no bucket of its own, got {message:?}"
        );
        assert!(
            answer.get("CertificateAuthorityArn").is_none(),
            "case {case:?}: a refused create answers no ARN, got {answer:?}"
        );
    }

    // The refusals created nothing: the only CAs this emulator holds are the accepted ones.
    let mut held = listed_cas(&emulator, "the CAs the emulator holds").await;
    held.sort();
    accepted.sort();
    assert_eq!(
        held, accepted,
        "a refused create must register no CA: expected {accepted:?}, got {held:?}"
    );
}

/// A configuration with CRLs DISABLED names no bucket, so there is nothing to resolve and
/// the call is served — the rule is about the bucket an ENABLED configuration publishes to.
#[tokio::test]
async fn a_disabled_crl_configuration_resolves_no_bucket() {
    let emulator = Emulator::new();

    let rows: [(&str, Value); 3] = [
        (
            "CRLs disabled and nothing else stated",
            json!({"CrlConfiguration": {"Enabled": false}}),
        ),
        (
            "OCSP enabled and no CRL configuration at all",
            json!({"OcspConfiguration": {"Enabled": true}}),
        ),
        (
            "both mechanisms disabled",
            json!({
                "CrlConfiguration": {"Enabled": false},
                "OcspConfiguration": {"Enabled": false},
            }),
        ),
    ];

    for (index, (case, revocation)) in rows.iter().enumerate() {
        let mut body = root_ca_request(&format!("no-crl-{index}"));
        body["RevocationConfiguration"] = revocation.clone();
        let created = emulator.ok(case, "CreateCertificateAuthority", &body).await;
        let arn = string(&created, "CertificateAuthorityArn", case);
        assert!(
            arn.contains(":certificate-authority/"),
            "case {case:?}: expected a CA ARN, got {arn:?}"
        );
    }
}

/// The same rule on the UPDATE: the configuration a CA is changed to is resolved before it
/// is written, and a refusal leaves the CA holding the configuration it had.
#[tokio::test]
async fn an_update_writes_a_crl_configuration_only_for_a_bucket_that_resolves() {
    let emulator = Emulator::new();
    let case = "the CA the updates are made against";
    assert_eq!(
        emulator.create_bucket("crl-updates").await,
        200,
        "the bucket the accepted update names is created through S3"
    );

    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request("update-root"))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    // An update is allowed only from ACTIVE or DISABLED, so the CA is given its
    // certificate first; the clock-owed settle out of CREATING comes due before that.
    std::thread::sleep(SETTLE_AFTER + std::time::Duration::from_millis(250));
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Certificate": blob(&certificate_pem("update-root")),
            }),
        )
        .await;

    // The accepted update: a bucket the account holds, which the CA then reports.
    emulator
        .ok(
            "an update naming a bucket that resolves",
            "UpdateCertificateAuthority",
            &json!({
                "CertificateAuthorityArn": arn,
                "RevocationConfiguration": crl_configuration("crl-updates", 14),
            }),
        )
        .await;

    // The refused update: a bucket nothing answers to, which leaves the CA as it was.
    let (status, answer) = emulator
        .call(
            "UpdateCertificateAuthority",
            &json!({
                "CertificateAuthorityArn": arn,
                "RevocationConfiguration": crl_configuration("no-such-update-bucket", 21),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "an update naming no bucket must be refused, got {status} {answer:?}"
    );
    let message = error_message(&answer, "an update naming no bucket");
    assert_eq!(
        message, CRL_BUCKET_ABSENT_MESSAGE,
        "an update naming no bucket: expected {CRL_BUCKET_ABSENT_MESSAGE:?}, got {message:?}"
    );

    let described = emulator
        .ok(
            "the CA after the refused update",
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    let crl = described
        .get("CertificateAuthority")
        .and_then(|authority| authority.get("RevocationConfiguration"))
        .and_then(|revocation| revocation.get("CrlConfiguration"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        crl.get("S3BucketName").and_then(Value::as_str),
        Some("crl-updates"),
        "a refused update must leave the CA's configuration as it was, got {crl:?}"
    );
    assert_eq!(
        crl.get("ExpirationInDays").and_then(Value::as_i64),
        Some(14),
        "a refused update must leave the CA's configuration as it was, got {crl:?}"
    );
}
