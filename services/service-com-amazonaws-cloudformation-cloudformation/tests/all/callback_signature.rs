// SPDX-License-Identifier: CC0-1.0

//! The signature a stack's resource provisioning puts on its in-process call.
//!
//! The rule under test: **the emulator verifies every request signature, so the call
//! CloudFormation makes to the service that owns a stack's resource must carry a SigV4
//! signature that emulator's OWN verifier re-computes** — otherwise provisioning works
//! only with request authentication off, and a stack deployed against a real (signed)
//! emulator creates nothing.
//!
//! The suite holds the signer to exactly that: it signs with the credentials this process
//! vends and then hands the signed request to `SigV4Authenticator::verify`, the one
//! verifier `MultiServiceRouter` runs with `--auth on`. The rows differ along what the
//! canonical request is built from — the signing name (which is also what routes the
//! call), the region, and the body — and the last rows show the verification is real: a
//! tampered body, a tampered header and a wrong secret are all refused.

use emulator_server::{CredentialStore, RequestAuthenticator, SigV4Authenticator};
use service_com_amazonaws_cloudformation_cloudformation::state::provisioning::sign_callback_request;

/// Build the in-process request a stack's provisioning sends: the same shape
/// `provisioning::dispatch` builds — `POST /` at the owning service's endpoint host, with
/// the awsJson target and the request document as the body.
fn callback_request(signing_name: &str, region: &str, target: &str, body: &str) -> http::Request<Vec<u8>> {
    http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .body(body.as_bytes().to_vec())
        .expect("a well-formed request")
}

/// Sign `request` with this process's own emulator credentials, as the callback does.
fn sign_with_the_emulators_credentials(case: &str, request: &mut http::Request<Vec<u8>>, signing_name: &str, region: &str) {
    let keys = SigV4Authenticator::for_this_process();
    sign_callback_request(
        request,
        signing_name,
        region,
        &keys.access_key_id,
        &keys.secret_access_key,
    )
    .unwrap_or_else(|error| panic!("case {case:?}: signing failed: {error}"));
}

/// Verify a signed request exactly as the router does for the service it is addressed to.
fn verify(request: http::Request<Vec<u8>>, serving: &str) -> Result<(), emulator_server::Denied> {
    let (parts, body) = request.into_parts();
    SigV4Authenticator::for_this_process().verify(&parts, &body, serving, &CredentialStore::default())
}

/// Every row: what the provisioning signer produces verifies for the service the call is
/// addressed to.
#[test]
fn the_signature_the_callback_puts_on_a_request_verifies() {
    // The rows differ in the signing name (also the routing key), the region, and the
    // size and shape of the body being signed.
    let rows = [
        (
            "a Firehose create in the default region",
            "firehose",
            "us-east-1",
            "Firehose_20150804.CreateDeliveryStream",
            r#"{"DeliveryStreamName":"a-stream"}"#.to_string(),
        ),
        (
            "a Firehose delete in another region",
            "firehose",
            "eu-west-3",
            "Firehose_20150804.DeleteDeliveryStream",
            r#"{"DeliveryStreamName":"a-stream"}"#.to_string(),
        ),
        (
            "an empty document",
            "firehose",
            "ap-southeast-2",
            "Firehose_20150804.ListDeliveryStreams",
            "{}".to_string(),
        ),
        (
            "a body far larger than one buffer",
            "firehose",
            "us-west-2",
            "Firehose_20150804.CreateDeliveryStream",
            format!(r#"{{"DeliveryStreamName":"{}"}}"#, "n".repeat(64 * 1024)),
        ),
        (
            "another service's signing name",
            "s3",
            "us-east-1",
            "AmazonS3.CreateBucket",
            "{}".to_string(),
        ),
    ];

    for (case, signing_name, region, target, body) in rows {
        let mut request = callback_request(signing_name, region, target, &body);
        sign_with_the_emulators_credentials(case, &mut request, signing_name, region);
        let verified = verify(request, signing_name);
        assert!(
            verified.is_ok(),
            "case {case:?}: expected the emulator's own verifier to accept the callback's \
             signature, got {verified:?}"
        );
    }
}

/// The verification is real: a request changed after it was signed, or signed with a
/// secret the emulator does not hold, is refused.
#[test]
fn a_tampered_or_wrongly_signed_callback_is_refused() {
    let (signing_name, region) = ("firehose", "us-east-1");
    let target = "Firehose_20150804.CreateDeliveryStream";

    // The body is changed after signing.
    let case = "a body changed after signing";
    let mut request = callback_request(signing_name, region, target, r#"{"DeliveryStreamName":"a"}"#);
    sign_with_the_emulators_credentials(case, &mut request, signing_name, region);
    *request.body_mut() = br#"{"DeliveryStreamName":"b"}"#.to_vec();
    let verified = verify(request, signing_name);
    assert!(
        verified.is_err(),
        "case {case:?}: expected the verifier to refuse it, got {verified:?}"
    );

    // A signed header is changed after signing.
    let case = "a signed header changed after signing";
    let mut request = callback_request(signing_name, region, target, "{}");
    sign_with_the_emulators_credentials(case, &mut request, signing_name, region);
    request.headers_mut().insert(
        "x-amz-target",
        http::HeaderValue::from_static("Firehose_20150804.DeleteDeliveryStream"),
    );
    let verified = verify(request, signing_name);
    assert!(
        verified.is_err(),
        "case {case:?}: expected the verifier to refuse it, got {verified:?}"
    );

    // Signed with a secret this emulator does not hold.
    let case = "signed with the wrong secret";
    let mut request = callback_request(signing_name, region, target, "{}");
    let keys = SigV4Authenticator::for_this_process();
    sign_callback_request(
        &mut request,
        signing_name,
        region,
        &keys.access_key_id,
        "0000000000000000000000000000000000000000",
    )
    .unwrap_or_else(|error| panic!("case {case:?}: signing failed: {error}"));
    let verified = verify(request, signing_name);
    assert!(
        verified.is_err(),
        "case {case:?}: expected the verifier to refuse it, got {verified:?}"
    );

    // And a request signed for one service is not served by another: the credential scope
    // names the service, which is what routes the call in the first place.
    let case = "signed for another service";
    let mut request = callback_request(signing_name, region, target, "{}");
    sign_with_the_emulators_credentials(case, &mut request, signing_name, region);
    let verified = verify(request, "s3");
    assert!(
        verified.is_err(),
        "case {case:?}: expected the verifier to refuse it, got {verified:?}"
    );
}
