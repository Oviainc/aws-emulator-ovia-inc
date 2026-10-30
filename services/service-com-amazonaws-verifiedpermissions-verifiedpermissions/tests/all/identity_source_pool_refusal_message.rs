// SPDX-License-Identifier: CC0-1.0

//! What a refusal of an identity source's Amazon Cognito user pool reference
//! SAYS.
//!
//! The rule under test, one sentence: an identity-source write whose
//! `cognitoUserPoolConfiguration.userPoolArn` is well formed and still resolves
//! to no user pool this service holds is refused with the modelled 400
//! `ValidationException` whose `message` states `User pool <poolId> does not
//! exist.` — the pool id read out of the request's own ARN — whichever segment of
//! the ARN is why the pool is unresolvable and whichever of the two writers
//! carried it.
//!
//! The rows differ along WHY the ARN resolves to no pool (an id nothing ever
//! created, an id whose pool was deleted, a one-character id, a pool that really
//! exists but in another region, an id naming another region than its ARN, an ARN
//! in another account, an ARN in another partition), so the sentence is pinned as
//! a TEMPLATE filled from the request rather than as one recorded value: every
//! row names a different pool and expects a different sentence.
//!
//! Every pool a row references is created — and for one row deleted — through the
//! Amazon Cognito user-pool service mounted on the same engine, so an accepted
//! reference names an id that service minted.
//!
//! Every assertion goes over the wire through the shared [`super::harness`].

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, cognito, error_code, fault_message, ok, router, store_with_validation_off, user_pool,
    user_pool_arn, user_pool_in, SIGNING_REGION,
};
use crate::identity_source_refusal_pointers::POOL_ARN_IN_CONFIGURATION;

/// `CreateIdentitySourceInput`'s provider member.
const CONFIGURATION: &str = "/configuration";

/// `UpdateIdentitySourceInput`'s provider member.
const UPDATE_CONFIGURATION: &str = "/updateConfiguration";

/// The entity type every row names, so no row is refused for an absent
/// `principalEntityType` instead of the pool reference it is about.
const PRINCIPAL_ENTITY_TYPE: &str = "MyCorp::User";

/// The sentence a write that references a user pool that does not exist is
/// refused with, written out HERE rather than derived from the code under test:
/// the fixed text is `User pool ` and ` does not exist.`, and the only variable
/// part is the pool id the request's `userPoolArn` carried.
fn does_not_exist(user_pool_id: &str) -> String {
    format!("User pool {user_pool_id} does not exist.")
}

/// A Cognito provider description naming `arn`, which is the only thing a row
/// varies.
fn cognito_provider(arn: &str) -> Value {
    json!({ "cognitoUserPoolConfiguration": { "userPoolArn": arn } })
}

/// Delete a user pool through the service that owns it, so a reference to it
/// resolves to nothing afterwards.
async fn delete_user_pool(router: &MultiServiceRouter, pool: &str) {
    cognito(router, "DeleteUserPool", &json!({ "UserPoolId": pool })).await;
}

/// The `userPoolArn` an identity source reports.
fn stored_pool_arn(source: &Value) -> Option<&str> {
    source["configuration"]["cognitoUserPoolConfiguration"]["userPoolArn"].as_str()
}

/// One row: what a failing assertion calls it, the `userPoolArn` both writers
/// carry, and the pool id that ARN names — which is the only variable part of the
/// sentence the refusal has to state.
struct Row {
    case: &'static str,
    arn: String,
    pool_id: String,
}

/// RULE: a `userPoolArn` that resolves to no user pool is refused 400
/// `ValidationException` stating `User pool <poolId> does not exist.`, with the
/// pool id taken from the request's ARN, by either writer of an identity source —
/// and the refused write leaves the store exactly as it was.
#[tokio::test]
async fn an_unresolvable_pool_reference_states_that_the_pool_does_not_exist() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let account = engine::emulator_account_id();

    // The pool every ACCEPTED call in this suite references: one the user-pool
    // service really minted, in the region the harness signs for.
    let live = user_pool(&router, "message-live-pool").await;
    let live_arn = user_pool_arn(SIGNING_REGION, &live);
    // A pool that existed and then stopped existing, so a row reads live state
    // rather than admitting anything well formed.
    let deleted = user_pool(&router, "message-deleted-pool").await;
    delete_user_pool(&router, &deleted).await;
    // A pool that really exists — in ANOTHER region, which this policy store
    // cannot reference, so the pool does not exist for this call either.
    let elsewhere = user_pool_in(&router, "eu-west-1", "message-other-region-pool").await;

    let rows = vec![
        Row {
            case: "a pool id nothing ever created",
            arn: user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_nevercreated")),
            pool_id: format!("{SIGNING_REGION}_nevercreated"),
        },
        Row {
            case: "a pool that was created and then deleted",
            arn: user_pool_arn(SIGNING_REGION, &deleted),
            pool_id: deleted.clone(),
        },
        Row {
            case: "a one-character pool id suffix",
            arn: user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_0")),
            pool_id: format!("{SIGNING_REGION}_0"),
        },
        Row {
            // A pool is reachable only from its own region, so one that exists
            // elsewhere exists for no call this store makes.
            case: "a live pool in another region",
            arn: user_pool_arn("eu-west-1", &elsewhere),
            pool_id: elsewhere.clone(),
        },
        Row {
            // A pool id names the region the pool was created in, so an id that
            // disagrees with its own ARN names no pool.
            case: "a pool id naming another region than its ARN",
            arn: user_pool_arn(SIGNING_REGION, "eu-west-1_1a2b3c4d5"),
            pool_id: "eu-west-1_1a2b3c4d5".to_string(),
        },
        Row {
            // Every resource this emulator holds is in its one account.
            case: "a pool ARN in another account",
            arn: format!(
                "arn:aws:cognito-idp:{SIGNING_REGION}:123456789012:userpool/{SIGNING_REGION}_1a2b3c4d5"
            ),
            pool_id: format!("{SIGNING_REGION}_1a2b3c4d5"),
        },
        Row {
            case: "a pool ARN in another partition",
            arn: format!(
                "arn:aws-us-gov:cognito-idp:{SIGNING_REGION}:{account}:userpool/{SIGNING_REGION}_9z8y7x6w5"
            ),
            pool_id: format!("{SIGNING_REGION}_9z8y7x6w5"),
        },
    ];

    // One live identity source for the update rows to aim at, so a refusal is the
    // configuration's and not the source's absence — and so "nothing was written"
    // is observable.
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

    // Every sentence the rows earned, so the set can be shown to carry one per
    // pool rather than one constant.
    let mut stated: Vec<(&str, String)> = Vec::new();

    for Row { case, arn, pool_id } in &rows {
        let expected = does_not_exist(pool_id);
        for (operation, root) in [
            ("CreateIdentitySource", CONFIGURATION),
            ("UpdateIdentitySource", UPDATE_CONFIGURATION),
        ] {
            let request = if operation == "CreateIdentitySource" {
                json!({
                    "policyStoreId": store_id,
                    "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                    "configuration": cognito_provider(arn),
                })
            } else {
                json!({
                    "policyStoreId": store_id,
                    "identitySourceId": target,
                    "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                    "updateConfiguration": cognito_provider(arn),
                })
            };
            let (status, body) = call(&router, operation, &request).await;

            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400 for {arn}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} on {operation}: expected a ValidationException, got {body}"
            );
            let message = fault_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the message {expected:?}, got {message:?}"
            );
            // The sentence reports the pool the caller referenced, not the ARN it
            // referenced it by, and nothing of this emulator's own wiring.
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?} on {operation}: the message must not echo the whole ARN {arn:?}, \
                 got {message:?}"
            );
            // A client that reads `fieldList` is still pointed at the member the
            // refused operation declares.
            let expected_path = format!("{root}{POOL_ARN_IN_CONFIGURATION}");
            assert_eq!(
                body["fieldList"][0]["path"],
                json!(expected_path),
                "case {case:?} on {operation}: expected the refusal to address \
                 {expected_path:?}, got {body}"
            );

            // The refused write changed nothing: the source the update rows aim at
            // still carries the pool it was created with, and no create minted a
            // second source.
            let aimed_at = ok(
                &router,
                "GetIdentitySource",
                &json!({ "policyStoreId": store_id, "identitySourceId": target }),
            )
            .await;
            assert_eq!(
                stored_pool_arn(&aimed_at),
                Some(live_arn.as_str()),
                "case {case:?} on {operation}: expected the source to still carry {live_arn}, \
                 got {aimed_at}"
            );
            let listed = ok(
                &router,
                "ListIdentitySources",
                &json!({ "policyStoreId": store_id }),
            )
            .await;
            assert_eq!(
                listed["identitySources"].as_array().map(Vec::len),
                Some(1),
                "case {case:?} on {operation}: expected the store to still hold one identity \
                 source, got {listed}"
            );

            stated.push((case, message));
        }
    }

    // The sentence is a template filled from the request: as many distinct
    // sentences as the rows named distinct pools.
    let distinct_messages: BTreeSet<&str> =
        stated.iter().map(|(_, message)| message.as_str()).collect();
    let distinct_pools: BTreeSet<&str> = rows.iter().map(|row| row.pool_id.as_str()).collect();
    assert_eq!(
        distinct_messages.len(),
        distinct_pools.len(),
        "expected one sentence per referenced pool ({}), got {stated:?}",
        distinct_pools.len()
    );
}

/// RULE (the other side of the same one): a pool reference that DOES resolve is
/// not refused, so the sentence is answered for an unresolvable pool only — the
/// reference is stored and read back through `GetIdentitySource`.
///
/// The rows differ along which writer stored the reference and which of two live
/// pools it names, so neither writer is refused for a pool that exists.
#[tokio::test]
async fn a_resolvable_pool_reference_is_stored_rather_than_refused() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let first = user_pool(&router, "message-accepted-first").await;
    let second = user_pool(&router, "message-accepted-second").await;
    let first_arn = user_pool_arn(SIGNING_REGION, &first);
    let second_arn = user_pool_arn(SIGNING_REGION, &second);

    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": cognito_provider(&first_arn),
        }),
    )
    .await;
    let source_id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();
    let read = json!({ "policyStoreId": store_id, "identitySourceId": source_id });
    let after_create = ok(&router, "GetIdentitySource", &read).await;
    assert_eq!(
        stored_pool_arn(&after_create),
        Some(first_arn.as_str()),
        "expected the created source to carry {first_arn}, got {after_create}"
    );

    ok(
        &router,
        "UpdateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "identitySourceId": source_id,
            "updateConfiguration": cognito_provider(&second_arn),
        }),
    )
    .await;
    let after_update = ok(&router, "GetIdentitySource", &read).await;
    assert_eq!(
        stored_pool_arn(&after_update),
        Some(second_arn.as_str()),
        "expected the updated source to carry {second_arn}, got {after_update}"
    );
}
