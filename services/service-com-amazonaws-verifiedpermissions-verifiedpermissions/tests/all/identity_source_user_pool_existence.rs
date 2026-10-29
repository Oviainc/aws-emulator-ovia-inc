// SPDX-License-Identifier: CC0-1.0

//! Whether the Amazon Cognito user pool an identity source points at EXISTS.
//!
//! The rule under test, one sentence: a write that stores a reference to a
//! resource another service owns resolves that reference against the state of the
//! service that owns it, and refuses a reference that resolves to no resource —
//! so `CreateIdentitySource` and `UpdateIdentitySource` accept a `userPoolArn`
//! only while the user-pool service really holds a pool with that id, and
//! otherwise answer the modelled 400 `ValidationException` naming the member that
//! carried the ARN.
//!
//! Every pool a row references is created (and, for one row, deleted) through the
//! Amazon Cognito user-pool service mounted on the same engine, so nothing here
//! invents a pool id: an accepted row names an id that service minted, and a
//! refused row names one no pool carries.
//!
//! Every assertion goes over the wire through the shared [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cognito, error_code, ok, router, store_with_validation_off, user_pool, user_pool_arn,
    SIGNING_REGION,
};
use crate::identity_source_refusal_pointers::POOL_ARN_IN_CONFIGURATION;

/// The entity type every row names, so no row is refused for an absent
/// `principalEntityType` instead of the pool reference it is about.
const PRINCIPAL_ENTITY_TYPE: &str = "MyCorp::User";

/// `CreateIdentitySourceInput`'s provider member.
const CONFIGURATION: &str = "/configuration";

/// `UpdateIdentitySourceInput`'s provider member.
const UPDATE_CONFIGURATION: &str = "/updateConfiguration";

/// A Cognito provider description naming `arn`, which is the only thing a row
/// varies.
fn cognito_provider(arn: &str) -> Value {
    json!({ "cognitoUserPoolConfiguration": { "userPoolArn": arn } })
}

/// Delete a user pool through the service that owns it, so a later reference to
/// it resolves to nothing.
async fn delete_user_pool(router: &MultiServiceRouter, pool: &str) {
    cognito(router, "DeleteUserPool", &json!({ "UserPoolId": pool })).await;
}

/// The issuer a Cognito-backed identity source reports, in the form the model
/// documents for `CognitoUserPoolConfigurationDetail$issuer`:
/// `https://cognito-idp.<region>.amazonaws.com/<user-pool-id>`.
fn pool_issuer(pool: &str) -> String {
    format!("https://cognito-idp.{SIGNING_REGION}.amazonaws.com/{pool}")
}

/// `pool` with its last character changed, inside the pool id's modelled
/// alphabet: a value one character off an id that really exists.
fn one_character_off(pool: &str) -> String {
    let mut characters: Vec<char> = pool.chars().collect();
    let last = characters.len() - 1;
    characters[last] = if characters[last] == 'a' { 'b' } else { 'a' };
    characters.into_iter().collect()
}

/// The `userPoolArn` an identity source reports.
fn stored_pool_arn(source: &Value) -> Option<&str> {
    source["configuration"]["cognitoUserPoolConfiguration"]["userPoolArn"].as_str()
}

/// RULE: a `userPoolArn` is accepted exactly while the user-pool service holds a
/// pool with the id it names — by either writer of an identity source — and a
/// reference that resolves to no pool is the modelled 400 `ValidationException`
/// addressing the member the refused operation declares, with the pool id the
/// request named in its reason.
///
/// The rows differ along WHY the id does or does not name a pool: one the service
/// just minted, a second one minted beside it, an id nothing ever created, an id
/// one character off a live one, an id whose pool was created and then DELETED
/// (so the check reads live state rather than admitting anything well formed),
/// and a one-character suffix. Every row is sent to BOTH writers.
#[tokio::test]
async fn a_user_pool_reference_resolves_against_the_pools_that_exist() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    let live = user_pool(&router, "existence-live-pool").await;
    let second = user_pool(&router, "existence-second-pool").await;
    let deleted = user_pool(&router, "existence-deleted-pool").await;
    delete_user_pool(&router, &deleted).await;

    // (case, the pool id the row's ARN names, whether a pool carries it)
    let rows: Vec<(&str, String, bool)> = vec![
        ("a pool the service just minted", live.clone(), true),
        ("a second pool minted beside it", second.clone(), true),
        (
            "a pool id nothing ever created",
            format!("{SIGNING_REGION}_nevercreated"),
            false,
        ),
        (
            "an id one character off a live pool's",
            one_character_off(&live),
            false,
        ),
        (
            "a pool that was created and then deleted",
            deleted.clone(),
            false,
        ),
        (
            "a one-character pool id suffix",
            format!("{SIGNING_REGION}_0"),
            false,
        ),
    ];

    // One live identity source for the update rows to aim at, so an update is
    // refused for the pool it names and not for a source that does not exist.
    let live_arn = user_pool_arn(SIGNING_REGION, &live);
    let target = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": cognito_provider(&live_arn),
        }),
    )
    .await["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();
    // What the update rows' source reports as the walk goes on: an accepted update
    // replaces it, a refused one must leave it exactly as it was.
    let mut target_pool = live.clone();
    // Every identity source an accepted create row minted, so a refused row can be
    // shown to have minted none.
    let mut created_sources = 1usize;

    for (case, pool, exists) in &rows {
        let arn = user_pool_arn(SIGNING_REGION, pool);
        for (operation, root) in [
            ("CreateIdentitySource", CONFIGURATION),
            ("UpdateIdentitySource", UPDATE_CONFIGURATION),
        ] {
            let request = if operation == "CreateIdentitySource" {
                json!({
                    "policyStoreId": store_id,
                    "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                    "configuration": cognito_provider(&arn),
                })
            } else {
                json!({
                    "policyStoreId": store_id,
                    "identitySourceId": target,
                    "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                    "updateConfiguration": cognito_provider(&arn),
                })
            };
            let (status, response) = call(&router, operation, &request).await;

            if *exists {
                assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: expected HTTP 200 for {arn}, got {status} \
                     {response}"
                );
                let source_id = if operation == "CreateIdentitySource" {
                    created_sources += 1;
                    response["identitySourceId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                } else {
                    target_pool = pool.clone();
                    target.clone()
                };
                // The reference is only accepted if it was stored: read it back.
                let got = ok(
                    &router,
                    "GetIdentitySource",
                    &json!({ "policyStoreId": store_id, "identitySourceId": source_id }),
                )
                .await;
                assert_eq!(
                    stored_pool_arn(&got),
                    Some(arn.as_str()),
                    "case {case:?} on {operation}: expected the stored userPoolArn {arn}, got \
                     {got}"
                );
                assert_eq!(
                    got["configuration"]["cognitoUserPoolConfiguration"]["issuer"],
                    json!(pool_issuer(pool)),
                    "case {case:?} on {operation}: expected the issuer of {pool}, got {got}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?} on {operation}: expected HTTP 400 for {arn}, got {status} \
                     {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {case:?} on {operation}: expected a ValidationException, got {response}"
                );
                let expected_path = format!("{root}{POOL_ARN_IN_CONFIGURATION}");
                assert_eq!(
                    response["fieldList"][0]["path"],
                    json!(expected_path),
                    "case {case:?} on {operation}: expected the refusal to address \
                     {expected_path:?}, got {response}"
                );
                let reason = response["fieldList"][0]["message"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    reason.contains(pool.as_str()),
                    "case {case:?} on {operation}: expected the reason to name the pool {pool}, \
                     got {reason:?}"
                );
            }

            // Whatever the answer, the source the update rows aim at reports the
            // pool that was last stored for it: a refused write changes nothing.
            let aimed_at = ok(
                &router,
                "GetIdentitySource",
                &json!({ "policyStoreId": store_id, "identitySourceId": target }),
            )
            .await;
            let expected_arn = user_pool_arn(SIGNING_REGION, &target_pool);
            assert_eq!(
                stored_pool_arn(&aimed_at),
                Some(expected_arn.as_str()),
                "case {case:?} on {operation}: expected the source to still carry \
                 {expected_arn}, got {aimed_at}"
            );

            // A refused create minted nothing, so the store holds exactly the
            // sources the accepted rows created.
            let listed = ok(
                &router,
                "ListIdentitySources",
                &json!({ "policyStoreId": store_id }),
            )
            .await;
            assert_eq!(
                listed["identitySources"].as_array().map(Vec::len),
                Some(created_sources),
                "case {case:?} on {operation}: expected the store to hold {created_sources} \
                 identity sources, got {listed}"
            );
        }
    }
}
