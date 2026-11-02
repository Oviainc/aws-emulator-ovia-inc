// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests pinning where the ARN `CreateWorkloadIdentity` reports comes
//! from.
//!
//! The rule under test: the ARN a create mints carries the Region the request was signed
//! for, is minted once and stored with the identity, and every later read of that identity
//! reports the stored ARN whatever Region the reader signed for — so the ARN is a property
//! of the persisted record and not of the request that reads it.
//!
//! The suites beside this one sign every request for one Region, so none of them can tell
//! an ARN read off the stored record from one re-derived on each call. These rows vary the
//! Region the create is signed for, and then read each identity back from a DIFFERENT
//! Region.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! `MultiServiceRouter::dispatch`, so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    error_code, listed, router, text, ACCOUNT, MAX_RESULTS_MAX, NAME_MAX_LENGTH, NAME_MIN_LENGTH,
};

/// The modelled bounds of `WorkloadIdentityArnType` (len 1..1024), from
/// `smithy_query shape=WorkloadIdentityArnType`.
const ARN_MIN_LENGTH: usize = 1;
const ARN_MAX_LENGTH: usize = 1024;

/// The Region the identities are read back from — deliberately not the Region any row
/// below creates in, so a read that re-derived the ARN from its own request would report
/// this label instead of the stored one.
const READER_REGION: &str = "sa-east-1";

/// The Region the emulator falls back to when a request carries no credential scope to
/// read one from (`crate::state::DEFAULT_REGION`).
const FALLBACK_REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header for this service whose credential scope names `region`.
/// It is what the router routes on, and what the handlers read the ARN Region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One call to `uri`, signed for `region` when one is given and carrying no credential
/// scope at all when it is `None` (the service is then claimed off the host).
async fn send(
    router: &MultiServiceRouter,
    uri: &str,
    region: Option<&str>,
    body: &Value,
) -> (u16, Value) {
    let payload = Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
    let mut request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header("content-type", "application/json");
    if let Some(region) = region {
        request = request.header("authorization", authorization(region));
    }
    let response = router
        .dispatch(
            request
                .body(payload)
                .expect("a well-formed request"),
        )
        .await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The ARN an identity of this account carries in `region`: the token vault's `default`
/// workload-identity directory, ending in the identity's own name.
fn arn_in(region: &str, name: &str) -> String {
    format!(
        "arn:aws:bedrock-agentcore:{region}:{ACCOUNT}:workload-identity-directory/default/\
         workload-identity/{name}"
    )
}

/// A create stamps the signed Region into the ARN it mints, and every later read reports
/// that stored ARN rather than one derived from the reading request.
///
/// Rows differ along the Region the create is signed for — labels from several Regions of
/// the public grammar (lowercase letters, digits and dashes), a scope region outside that
/// grammar, and no credential scope at all — and along the name's shape and length, so the
/// two are seen to be independent.
#[tokio::test]
async fn a_created_identity_carries_the_arn_region_of_the_request_that_created_it() {
    let router = router();

    // (case, the Region the create signs for, the Region its ARN must carry, name)
    let rows: [(&str, Option<&str>, &str, String); 7] = [
        (
            "us-east-1, the shortest name the model admits",
            Some("us-east-1"),
            "us-east-1",
            "a".repeat(NAME_MIN_LENGTH),
        ),
        (
            "eu-west-1, dots, dashes and underscores",
            Some("eu-west-1"),
            "eu-west-1",
            "payments.service_v1-west".to_string(),
        ),
        (
            "ap-southeast-2, digits only",
            Some("ap-southeast-2"),
            "ap-southeast-2",
            "20261029".to_string(),
        ),
        (
            "us-west-2, mixed case",
            Some("us-west-2"),
            "us-west-2",
            "MixedCaseIdentity".to_string(),
        ),
        (
            "eu-central-1, the longest name the model admits",
            Some("eu-central-1"),
            "eu-central-1",
            "z".repeat(NAME_MAX_LENGTH),
        ),
        (
            // An ARN's Region field is a Region label, so a credential scope outside that
            // grammar is not stamped into one: the emulator's default Region stands in.
            "a scope region outside the Region-label grammar",
            Some("US-East-1"),
            FALLBACK_REGION,
            "scope-outside-the-grammar".to_string(),
        ),
        (
            "no credential scope at all",
            None,
            FALLBACK_REGION,
            "no-credential-scope".to_string(),
        ),
    ];

    for (case, signed_for, expected_region, name) in &rows {
        let (status, created) = send(
            &router,
            "/identities/CreateWorkloadIdentity",
            *signed_for,
            &json!({"name": name}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateWorkloadIdentity, got {status} ({created})"
        );

        let expected = arn_in(expected_region, name);
        let minted = text(&created, "workloadIdentityArn");
        assert_eq!(
            minted, expected,
            "case {case:?}: expected the ARN to carry Region {expected_region:?}, got {minted}"
        );
        assert!(
            (ARN_MIN_LENGTH..=ARN_MAX_LENGTH).contains(&minted.len()),
            "case {case:?}: expected an ARN within the modelled {ARN_MIN_LENGTH}..{ARN_MAX_LENGTH} \
             characters, got {} ({minted})",
            minted.len()
        );

        // The read is signed for a Region no row creates in: it must still report the ARN
        // the create stored.
        let (status, fetched) = send(
            &router,
            "/identities/GetWorkloadIdentity",
            Some(READER_REGION),
            &json!({"name": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetWorkloadIdentity, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "workloadIdentityArn"),
            expected,
            "case {case:?}: a read does not re-mint the ARN in its own Region, got {}",
            text(&fetched, "workloadIdentityArn")
        );
    }

    // The listing, signed for that same foreign Region, reports each identity's stored ARN
    // too: one record, read the same way by every operation that reports it.
    let (status, page) = send(
        &router,
        "/identities/ListWorkloadIdentities",
        Some(READER_REGION),
        &json!({"maxResults": MAX_RESULTS_MAX}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the listing after every row\": expected HTTP 200, got {status} ({page})"
    );
    let mut expected_arns: Vec<String> = rows
        .iter()
        .map(|(_, _, region, name)| arn_in(region, name))
        .collect();
    expected_arns.sort();
    let mut reported: Vec<String> = listed(&page)
        .iter()
        .map(|entry| text(entry, "workloadIdentityArn"))
        .collect();
    reported.sort();
    assert_eq!(
        reported, expected_arns,
        "case \"the listing after every row\": expected every stored ARN, got {reported:?} for \
         {expected_arns:?}"
    );
}

/// The Region a request is signed for selects no store of its own: the `name` is unique
/// within the account, so a create of a name this account already holds is refused however
/// the second request is signed, and the identity keeps the ARN its own create minted.
#[tokio::test]
async fn a_held_name_is_refused_whatever_region_the_second_create_signs_for() {
    let router = router();
    let name = "cross-region-identity";
    let home = "eu-west-1";

    let (status, created) = send(
        &router,
        "/identities/CreateWorkloadIdentity",
        Some(home),
        &json!({"name": name}),
    )
    .await;
    assert_eq!(
        (status, text(&created, "workloadIdentityArn")),
        (201, arn_in(home, name)),
        "case \"the identity the refusals are measured against\": expected HTTP 201 and the \
         {home} ARN, got {status} ({created})"
    );

    for signed_for in [Some(home), Some("us-west-2"), Some("ap-southeast-2"), None] {
        let case = signed_for.unwrap_or("no credential scope");
        let (status, answer) = send(
            &router,
            "/identities/CreateWorkloadIdentity",
            signed_for,
            &json!({"name": name}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a name this account already holds is refused, got {status} {} \
             ({answer})",
            error_code(&answer)
        );

        // The refusal left the first identity exactly as its own create stored it.
        let (status, fetched) = send(
            &router,
            "/identities/GetWorkloadIdentity",
            Some(READER_REGION),
            &json!({"name": name}),
        )
        .await;
        assert_eq!(
            (status, text(&fetched, "workloadIdentityArn")),
            (200, arn_in(home, name)),
            "case {case:?}: the refused create did not re-mint the ARN, got {status} ({fetched})"
        );
    }
}
