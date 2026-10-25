// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateIdentityPool`, the mutation that reconfigures a stored
//! identity pool.
//!
//! The RULES every case asserts, over a table of inputs rather than one recorded call:
//!
//! * An update REPLACES the pool's configuration with the one the request carries — "if
//!   you don't provide a value for a parameter, Amazon Cognito sets it to its default
//!   value" (this service's model) — so a member an earlier call set and this one omits is
//!   gone afterwards, not carried over. What the update answers is what the pool's readers
//!   report, and the pool keeps the id it was minted with.
//! * An update naming a pool this service does not hold — one never created, or one
//!   already deleted — is the modelled 404 `ResourceNotFoundException`, and writes nothing:
//!   every other pool keeps the configuration it had.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, Emulator, ABSENT_POOL_ID};

/// The operation under test, spelled once.
const OPERATION: &str = "UpdateIdentityPool";

/// Every OPTIONAL member `UpdateIdentityPoolInput` declares, from
/// `smithy_query(query="operation", operation="UpdateIdentityPool")`. A row that omits one
/// must leave the stored pool without it, which is what makes this catalog the thing to
/// check after each update.
const OPTIONAL_MEMBERS: [&str; 7] = [
    "AllowClassicFlow",
    "SupportedLoginProviders",
    "DeveloperProviderName",
    "OpenIdConnectProviderARNs",
    "CognitoIdentityProviders",
    "SamlProviderARNs",
    "IdentityPoolTags",
];

/// An update request for `identity_pool_id` carrying `configuration`'s members.
fn update_request(identity_pool_id: &str, configuration: &Value) -> Value {
    let mut request = Map::new();
    request.insert("IdentityPoolId".to_string(), json!(identity_pool_id));
    for (member, value) in configuration
        .as_object()
        .expect("a configuration row is an object")
    {
        request.insert(member.clone(), value.clone());
    }
    Value::Object(request)
}

/// One update per shape of configuration the model allows, in the order they are applied
/// to the one pool: each row both sets what it carries and drops what the row before it
/// set, so the replace rule is asserted over every member of the catalog.
fn configurations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "every member the model declares",
            json!({
                "IdentityPoolName": "fully-configured",
                "AllowUnauthenticatedIdentities": true,
                "AllowClassicFlow": true,
                "SupportedLoginProviders": {
                    "graph.facebook.com": "1234567890",
                    "accounts.google.com": "client.apps.googleusercontent.com",
                },
                "DeveloperProviderName": "my.dev_provider-1",
                "OpenIdConnectProviderARNs": [
                    "arn:aws:iam::000000000000:oidc-provider/auth.example.com",
                ],
                "CognitoIdentityProviders": [{
                    "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_abcd1234",
                    "ClientId": "1h57kf5cpq17m0eml12eqpq8m2",
                    "ServerSideTokenCheck": true,
                }],
                "SamlProviderARNs": ["arn:aws:iam::000000000000:saml-provider/Example"],
                "IdentityPoolTags": {"Env": "test", "Owner": "identity-team"},
            }),
        ),
        (
            "only the required members, which drops every other member",
            json!({
                "IdentityPoolName": "stripped-down",
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
        (
            "classic flow turned off and one login provider",
            json!({
                "IdentityPoolName": "classic-off",
                "AllowUnauthenticatedIdentities": false,
                "AllowClassicFlow": false,
                "SupportedLoginProviders": {"graph.facebook.com": "9876543210"},
            }),
        ),
        (
            "an empty supported-login-providers map (the modelled minimum)",
            json!({
                "IdentityPoolName": "no-providers",
                "AllowUnauthenticatedIdentities": false,
                "SupportedLoginProviders": {},
            }),
        ),
        (
            "tags only",
            json!({
                "IdentityPoolName": "tagged",
                "AllowUnauthenticatedIdentities": true,
                "IdentityPoolTags": {"Env": "prod"},
            }),
        ),
        (
            "a developer provider name only",
            json!({
                "IdentityPoolName": "dev-provider",
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": "another.dev_provider-2",
            }),
        ),
        (
            "a name of the maximum modelled length",
            json!({
                "IdentityPoolName": "n".repeat(128),
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
        (
            "a name of the minimum modelled length",
            json!({"IdentityPoolName": "a", "AllowUnauthenticatedIdentities": true}),
        ),
    ]
}

/// RULE: an update replaces the pool's configuration with the request's, the readers
/// report exactly what it wrote, and the pool keeps its id.
#[tokio::test]
async fn an_update_replaces_the_stored_configuration() {
    let emulator = Emulator::new();
    let identity_pool_id = emulator.pool("before-any-update", false).await;
    // A second pool, never updated, so every row can also show the update touched only
    // the pool it named.
    let untouched_id = emulator.pool("untouched", true).await;
    let untouched_before = emulator
        .ok(
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &untouched_id}),
        )
        .await;

    for (case, configuration) in configurations() {
        let request = update_request(&identity_pool_id, &configuration);
        let updated = emulator.ok(OPERATION, &request).await;

        // Every member the request sent comes back with the value it was sent with, and
        // the id is the one the pool was minted with.
        for (member, sent) in request.as_object().expect("the request is an object") {
            assert_eq!(
                &updated[member], sent,
                "case {case:?}: the update answered {member} = {}, expected {sent}",
                updated[member]
            );
        }

        // A member this row did not send is absent from the pool afterwards: an update
        // replaces the configuration rather than merging into it.
        for member in OPTIONAL_MEMBERS {
            if request.get(member).is_some() {
                continue;
            }
            assert!(
                updated[member].is_null(),
                "case {case:?}: expected {member} to be unset after an update that omits \
                 it, got {} in {updated}",
                updated[member]
            );
        }

        // What the update wrote is what the pool's readers report.
        let described = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            described, updated,
            "case {case:?}: DescribeIdentityPool reported {described}, expected the \
             updated pool {updated}"
        );

        // Including the account-wide listing, which reports the pool under its new name.
        let listed = emulator
            .ok("ListIdentityPools", &json!({"MaxResults": 60}))
            .await;
        let names: Vec<(&str, &str)> = listed["IdentityPools"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a page carries an IdentityPools list"))
            .iter()
            .map(|pool| {
                (
                    pool["IdentityPoolId"].as_str().unwrap_or_default(),
                    pool["IdentityPoolName"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        let expected_name = configuration["IdentityPoolName"].as_str().unwrap_or_default();
        assert!(
            names.contains(&(identity_pool_id.as_str(), expected_name)),
            "case {case:?}: expected the listing to report the pool as \
             {expected_name:?}, got {names:?}"
        );

        // And the pool the update did not name is exactly as it was.
        let untouched_now = emulator
            .ok(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &untouched_id}),
            )
            .await;
        assert_eq!(
            untouched_now, untouched_before,
            "case {case:?}: the pool the update did not name expected {untouched_before}, \
             got {untouched_now}"
        );
    }
}

/// RULE: an update naming a pool this service does not hold is the modelled 404
/// `ResourceNotFoundException` and writes nothing.
#[tokio::test]
async fn an_update_of_a_pool_this_service_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let kept_id = emulator.pool("kept", true).await;
    let kept_before = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &kept_id}))
        .await;
    let deleted_id = emulator.pool("deleted", true).await;
    emulator
        .ok(
            "DeleteIdentityPool",
            &json!({"IdentityPoolId": &deleted_id}),
        )
        .await;

    let absent: Vec<(&str, String)> = vec![
        (
            "an IdentityPoolId no pool was ever created with",
            ABSENT_POOL_ID.to_string(),
        ),
        ("a pool that was deleted", deleted_id.clone()),
        (
            "a well-formed id naming another region",
            "eu-west-1:22222222-2222-2222-2222-222222222222".to_string(),
        ),
    ];
    for (case, identity_pool_id) in absent {
        let (status, body) = emulator
            .call(
                OPERATION,
                &json!({
                    "IdentityPoolId": &identity_pool_id,
                    "IdentityPoolName": "renamed-by-a-doomed-update",
                    "AllowUnauthenticatedIdentities": true,
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {identity_pool_id:?}, got {status} with \
             {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code} with {body}"
        );

        // The refused update created nothing: the pool it named is still not there.
        let (status, body) = emulator
            .call(
                "DescribeIdentityPool",
                &json!({"IdentityPoolId": &identity_pool_id}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused update must not create {identity_pool_id:?}, got \
             {status} with {body}"
        );
    }

    // And the pool that does exist is exactly as it was.
    let kept_now = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &kept_id}))
        .await;
    assert_eq!(
        kept_now, kept_before,
        "the stored pool expected {kept_before} after the refused updates, got {kept_now}"
    );
}
