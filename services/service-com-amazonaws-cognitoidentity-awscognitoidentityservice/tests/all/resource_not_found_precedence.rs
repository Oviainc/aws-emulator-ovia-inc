// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH resource a `ResourceNotFoundException` names when a
//! request carries more than one.
//!
//! The rule asserted over a table of inputs, not one recorded call: an operation that
//! takes an `IdentityId` as a **required** input resolves that identity before any other
//! resource the request names, so an identity this service does not hold is the resource
//! reported missing — even when the `IdentityPoolId` sent beside it is unknown too.
//! An operation that takes no required identity of its own (the pool-scoped reads and
//! writes, including the one whose `IdentityId` is OPTIONAL) resolves its pool first and
//! reports the pool.
//!
//! The required/optional split comes from the model:
//! `smithy_query(query="shape", shape="UnlinkDeveloperIdentityInput,LookupDeveloperIdentityInput")`
//! — `UnlinkDeveloperIdentity` requires both `IdentityId` and `IdentityPoolId`, while
//! `LookupDeveloperIdentity` requires only `IdentityPoolId`.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// Well-formed `IdentityId`s that nothing ever minted. `IdentityId` is modelled
/// `len 1..55, pattern ^[\w-]+:[0-9a-f-]+$`, so these span that shape: a REGION:GUID of
/// the usual form, one in another region, and the two published length bounds.
const ABSENT_IDENTITY_ID: &str = "us-east-1:77777777-7777-7777-7777-777777777777";
const ABSENT_IDENTITY_ID_OTHER_REGION: &str = "eu-west-2:deadbeef-cafe-4fed-9999-feedfacecafe";
const SHORTEST_ABSENT_IDENTITY_ID: &str = "a:0";

/// A second well-formed `IdentityPoolId` no pool was ever created with, in another region:
/// the pool named beside an unknown identity must not change which resource is reported.
const ABSENT_POOL_ID_OTHER_REGION: &str = "eu-west-2:00000000-0000-0000-0000-00000000beef";

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// Amazon Cognito Identity's not-found wording for each of the two resources: fixed text
/// around the one variable part, the value the request carried.
fn identity_not_found(identity_id: &str) -> String {
    format!("Identity '{identity_id}' not found.")
}

fn identity_pool_not_found(identity_pool_id: &str) -> String {
    format!("IdentityPool '{identity_pool_id}' not found.")
}

/// The `UnlinkDeveloperIdentity` request for one row, with all four modelled-required
/// members sent.
fn unlink_developer_request(
    pool_id: &str,
    identity_id: &str,
    developer_provider: &str,
    identifier: &str,
) -> Value {
    json!({
        "IdentityPoolId": pool_id,
        "IdentityId": identity_id,
        "DeveloperProviderName": developer_provider,
        "DeveloperUserIdentifier": identifier,
    })
}

/// Mint an identity of `pool_id` carrying `identifier` under the pool's developer
/// provider, and answer its `IdentityId`.
async fn register(
    emulator: &Emulator,
    pool_id: &str,
    developer_provider: &str,
    identifier: &str,
) -> String {
    let minted = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &token_request(pool_id, &[(developer_provider, identifier)], None, None),
        )
        .await;
    minted["IdentityId"]
        .as_str()
        .expect("GetOpenIdTokenForDeveloperIdentity returns an IdentityId")
        .to_string()
}

/// RULE: `UnlinkDeveloperIdentity` resolves its required `IdentityId` before the
/// `IdentityPoolId`, so an identity this service does not hold is reported missing with
/// AWS's identity wording whatever pool the same request names.
#[tokio::test]
async fn unlink_developer_identity_reports_the_identity_it_cannot_resolve() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id =
        pool_with_developer_provider(&emulator, "not-found-precedence", Some(developer_provider))
            .await;

    // `IdentityId` is modelled `len 1..55`, so this row sits exactly on the published
    // maximum; `DeveloperProviderName` (`len 1..128`) and `DeveloperUserIdentifier`
    // (`len 1..1024`) sit on theirs, to show the refusal is decided by the identity alone
    // and not by anything else the request carries.
    let longest_absent_identity = format!("us-east-1:{}", "a".repeat(45));
    let longest_provider = "p".repeat(128);
    let longest_identifier = "u".repeat(1024);

    // (case, IdentityPoolId sent, IdentityId sent, DeveloperProviderName, DeveloperUserIdentifier)
    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "neither the pool nor the identity is stored",
            ABSENT_POOL_ID,
            ABSENT_IDENTITY_ID,
            "some.dev.provider",
            "nobody",
        ),
        (
            "the pool is stored and the identity is not",
            &pool_id,
            ABSENT_IDENTITY_ID,
            developer_provider,
            "nobody",
        ),
        (
            "both ids name another region",
            ABSENT_POOL_ID_OTHER_REGION,
            ABSENT_IDENTITY_ID_OTHER_REGION,
            "dev_provider-1",
            "someone-else",
        ),
        (
            "an absent identity at the shortest form the pattern allows",
            ABSENT_POOL_ID,
            SHORTEST_ABSENT_IDENTITY_ID,
            "p",
            "x",
        ),
        (
            "an absent identity at the modelled maximum length",
            ABSENT_POOL_ID,
            &longest_absent_identity,
            &longest_provider,
            &longest_identifier,
        ),
    ];

    for (case, pool, identity, provider, identifier) in cases {
        let request = unlink_developer_request(pool, identity, provider, identifier);
        let (status, body) = emulator.call("UnlinkDeveloperIdentity", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an identity this service does not hold, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(case, &body),
            identity_not_found(identity),
            "case {case:?}: expected the identity to be the resource named, got {body}"
        );
    }
}

/// RULE: the pool is still the resource reported missing when the identity resolves — the
/// identity-first precedence replaces no pool-not-found refusal, it only orders the two.
#[tokio::test]
async fn unlink_developer_identity_reports_the_pool_when_the_identity_resolves() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "stored-identity", Some(developer_provider))
        .await;
    let identity_id = register(&emulator, &pool_id, developer_provider, "user-1").await;

    // (case, IdentityPoolId sent)
    let cases: Vec<(&str, &str)> = vec![
        ("a pool id nothing created", ABSENT_POOL_ID),
        ("a pool id of another region", ABSENT_POOL_ID_OTHER_REGION),
    ];

    for (case, pool) in cases {
        let request =
            unlink_developer_request(pool, &identity_id, developer_provider, "user-1");
        let (status, body) = emulator.call("UnlinkDeveloperIdentity", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a pool this service does not hold, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(case, &body),
            identity_pool_not_found(pool),
            "case {case:?}: expected the pool to be the resource named, got {body}"
        );
    }
}

/// RULE: every operation whose `IdentityId` is modelled **required** reports the identity
/// it cannot resolve, and every operation that requires only an `IdentityPoolId` reports
/// the pool — including the one whose `IdentityId` is optional and carries an unknown one.
#[tokio::test]
async fn each_operation_reports_the_resource_it_resolves_first() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";

    // (case, operation, request, expected message) — the operations come from
    // `smithy_query(query="digest")`, split by whether `IdentityId` is required.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "DescribeIdentity requires an IdentityId",
            "DescribeIdentity",
            json!({"IdentityId": ABSENT_IDENTITY_ID}),
            identity_not_found(ABSENT_IDENTITY_ID),
        ),
        (
            "GetCredentialsForIdentity requires an IdentityId",
            "GetCredentialsForIdentity",
            json!({"IdentityId": ABSENT_IDENTITY_ID_OTHER_REGION}),
            identity_not_found(ABSENT_IDENTITY_ID_OTHER_REGION),
        ),
        (
            "GetOpenIdToken requires an IdentityId",
            "GetOpenIdToken",
            json!({"IdentityId": ABSENT_IDENTITY_ID}),
            identity_not_found(ABSENT_IDENTITY_ID),
        ),
        (
            "UnlinkIdentity requires an IdentityId",
            "UnlinkIdentity",
            json!({
                "IdentityId": ABSENT_IDENTITY_ID,
                "Logins": {"graph.facebook.com": "a-token"},
                "LoginsToRemove": ["graph.facebook.com"],
            }),
            identity_not_found(ABSENT_IDENTITY_ID),
        ),
        (
            "UnlinkDeveloperIdentity requires an IdentityId",
            "UnlinkDeveloperIdentity",
            unlink_developer_request(
                ABSENT_POOL_ID,
                ABSENT_IDENTITY_ID,
                developer_provider,
                "nobody",
            ),
            identity_not_found(ABSENT_IDENTITY_ID),
        ),
        (
            "ListIdentities requires only an IdentityPoolId",
            "ListIdentities",
            json!({"IdentityPoolId": ABSENT_POOL_ID, "MaxResults": 5}),
            identity_pool_not_found(ABSENT_POOL_ID),
        ),
        (
            "LookupDeveloperIdentity's IdentityId is optional",
            "LookupDeveloperIdentity",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "DeveloperUserIdentifier": "nobody",
                "MaxResults": 1,
            }),
            identity_pool_not_found(ABSENT_POOL_ID),
        ),
        (
            "LookupDeveloperIdentity carrying an unknown identity still reports the pool",
            "LookupDeveloperIdentity",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "IdentityId": ABSENT_IDENTITY_ID,
                "MaxResults": 1,
            }),
            identity_pool_not_found(ABSENT_POOL_ID),
        ),
        (
            "MergeDeveloperIdentities requires only an IdentityPoolId",
            "MergeDeveloperIdentities",
            json!({
                "SourceUserIdentifier": "source-user",
                "DestinationUserIdentifier": "dest-user",
                "DeveloperProviderName": developer_provider,
                "IdentityPoolId": ABSENT_POOL_ID,
            }),
            identity_pool_not_found(ABSENT_POOL_ID),
        ),
        (
            "DescribeIdentityPool requires only an IdentityPoolId",
            "DescribeIdentityPool",
            json!({"IdentityPoolId": ABSENT_POOL_ID_OTHER_REGION}),
            identity_pool_not_found(ABSENT_POOL_ID_OTHER_REGION),
        ),
    ];

    for (case, operation, request, expected) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException from {operation}, got {body}"
        );
        assert_eq!(
            error_message(case, &body),
            expected,
            "case {case:?}: expected {expected:?} from {operation}, got {body}"
        );
    }
}
