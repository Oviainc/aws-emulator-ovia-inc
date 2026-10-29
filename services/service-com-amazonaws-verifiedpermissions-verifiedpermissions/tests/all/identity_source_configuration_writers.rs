// SPDX-License-Identifier: CC0-1.0

//! The two writers of an identity source's stored configuration agree.
//!
//! `CreateIdentitySource` and `UpdateIdentitySource` describe the same identity
//! provider in two member-identical request unions (`Configuration` and
//! `UpdateConfiguration`), and both reduce it to the one stored configuration a
//! read reports. The rule under test, one sentence: the provider a write
//! describes is stored the same way whichever operation carried it — the same
//! resolution of a Cognito pool reference, the same default for an absent
//! optional list, the same token selection — so a read cannot tell which writer
//! put it there.
//!
//! Every row therefore writes ONE provider description TWICE: once as the
//! `configuration` of a create, once as the `updateConfiguration` of an update
//! replacing a seeded source. Each read-back is compared both against the other
//! and against what the read must report, so the suite fails either when the two
//! writers diverge or when they agree on the wrong thing.
//!
//! Every assertion goes over the wire through the same router the emulator
//! binary serves, reusing the harness in [`crate::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, error_code, ok, router, store_with_validation_off, user_pool_arn};
use crate::identity_source_refusal_pointers::POOL_ARN_IN_CONFIGURATION;

/// The region every request in this suite is signed for, from the harness, so a
/// Cognito pool ARN and the emulator agree on the region.
const REGION: &str = crate::harness::SIGNING_REGION;

/// The Cedar entity type every row's principals carry. Named on every call, so a
/// row's two sources differ in nothing but the writer that configured them —
/// `principalEntityType` has its own rule per operation and is not what this
/// suite is about.
const PRINCIPAL_ENTITY_TYPE: &str = "MyCorp::User";

/// An OIDC issuer is a provider OUTSIDE the emulator, accepted as given (the
/// model's `^https://.*$`).
const OIDC_ISSUER: &str = "https://accounts.example.com";

/// The provider an update's source is seeded with before the row replaces it: a
/// Cognito pool, so a row replacing it with an OIDC provider crosses union arms.
/// The pool is created first, because an identity source can only reference a
/// pool the user-pool service holds.
async fn seed_provider(router: &MultiServiceRouter) -> Value {
    let pool = crate::harness::user_pool(router, "writers-seed-pool").await;
    json!({
        "cognitoUserPoolConfiguration": { "userPoolArn": user_pool_arn(REGION, &pool) }
    })
}

/// Create an identity source carrying `provider` as its `configuration`, and
/// return its id.
async fn created_with(router: &MultiServiceRouter, store_id: &str, provider: &Value) -> String {
    let created = ok(
        router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": provider,
        }),
    )
    .await;
    created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string()
}

/// Create an identity source on the seed provider, then UPDATE it to `provider`,
/// and return its id.
async fn updated_to(router: &MultiServiceRouter, store_id: &str, provider: &Value) -> String {
    let id = created_with(router, store_id, &seed_provider(router).await).await;
    ok(
        router,
        "UpdateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "identitySourceId": id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "updateConfiguration": provider,
        }),
    )
    .await;
    id
}

/// Read an identity source back.
async fn read(router: &MultiServiceRouter, store_id: &str, id: &str) -> Value {
    ok(
        router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store_id, "identitySourceId": id }),
    )
    .await
}

/// The `fieldList` of a fault, as `(path, message)` pairs in the order the
/// response carries them — what tells a caller which member to fix.
fn field_list(body: &Value) -> Vec<(String, String)> {
    body.get("fieldList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|field| {
            (
                field["path"].as_str().unwrap_or_default().to_string(),
                field["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE: one provider description is stored identically whichever writer carried
/// it, so `GetIdentitySource` reports the same `configuration` and the same
/// deprecated `details` for a source CREATED with it as for one UPDATED to it.
///
/// The rows span what the translation decides over: the arm of the union
/// (Cognito, OIDC), the arm of the token selection (access token, identity
/// token), an optional list absent, empty and populated, and the optional
/// `entityIdPrefix` and `groupConfiguration` present and absent.
#[tokio::test]
async fn both_writers_store_one_provider_description_the_same_way() {
    let router = router();
    let pool = crate::harness::user_pool(&router, "writers-pool").await;
    let arn = user_pool_arn(REGION, &pool);
    // The issuer and discovery URL the model documents for a Cognito user pool:
    // `https://cognito-idp.<region>.amazonaws.com/<user-pool-id>`, and that URL
    // plus `/.well-known/openid-configuration`.
    let issuer = format!("https://cognito-idp.{REGION}.amazonaws.com/{pool}");
    let discovery = format!("{issuer}/.well-known/openid-configuration");

    // (case, the provider description both writers carry, the `configuration` a
    //  read must report, the deprecated `details` a read must report)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            // No optional member at all: `clientIds` is absent on the request
            // and a list on every read, so it is stored — and reported — empty.
            "cognito: no optional member",
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn": arn } }),
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": arn,
                "clientIds": [],
                "issuer": issuer,
            } }),
            json!({
                "clientIds": [],
                "userPoolArn": arn,
                "discoveryUrl": discovery,
                "openIdIssuer": "COGNITO",
            }),
        ),
        (
            // An EMPTY list and an absent one are one stored value, so this row
            // must read back exactly as the row above.
            "cognito: an empty clientIds list",
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": arn,
                "clientIds": [],
            } }),
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": arn,
                "clientIds": [],
                "issuer": issuer,
            } }),
            json!({
                "clientIds": [],
                "userPoolArn": arn,
                "discoveryUrl": discovery,
                "openIdIssuer": "COGNITO",
            }),
        ),
        (
            "cognito: client ids and a group entity type",
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": arn,
                "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
            } }),
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": arn,
                "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                "issuer": issuer,
                "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
            } }),
            json!({
                "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                "userPoolArn": arn,
                "discoveryUrl": discovery,
                "openIdIssuer": "COGNITO",
            }),
        ),
        (
            "oidc: accessTokenOnly, no audiences and no optional member",
            json!({ "openIdConnectConfiguration": {
                "issuer": OIDC_ISSUER,
                "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
            } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": OIDC_ISSUER,
                "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
            } }),
            // `details` describes a Cognito user pool, so an OIDC source has none.
            Value::Null,
        ),
        (
            "oidc: accessTokenOnly with audiences, an entityIdPrefix and groups",
            json!({ "openIdConnectConfiguration": {
                "issuer": "https://oidc.example.org/tenant",
                "entityIdPrefix": "MyOIDCProvider",
                "groupConfiguration": {
                    "groupClaim": "groups",
                    "groupEntityType": "MyCorp::UserGroup",
                },
                "tokenSelection": { "accessTokenOnly": {
                    "principalIdClaim": "uid",
                    "audiences": ["https://api.example.com", "https://api.example.org"],
                } },
            } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": "https://oidc.example.org/tenant",
                "entityIdPrefix": "MyOIDCProvider",
                "groupConfiguration": {
                    "groupClaim": "groups",
                    "groupEntityType": "MyCorp::UserGroup",
                },
                "tokenSelection": { "accessTokenOnly": {
                    "principalIdClaim": "uid",
                    "audiences": ["https://api.example.com", "https://api.example.org"],
                } },
            } }),
            Value::Null,
        ),
        (
            "oidc: identityTokenOnly with client ids",
            json!({ "openIdConnectConfiguration": {
                "issuer": OIDC_ISSUER,
                "tokenSelection": { "identityTokenOnly": {
                    "principalIdClaim": "sub",
                    "clientIds": ["clientone", "clienttwo"],
                } },
            } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": OIDC_ISSUER,
                "tokenSelection": { "identityTokenOnly": {
                    "principalIdClaim": "sub",
                    "clientIds": ["clientone", "clienttwo"],
                } },
            } }),
            Value::Null,
        ),
        (
            // An absent `clientIds` on this arm is stored empty too, and a read
            // of this detail shape omits an empty list rather than reporting it.
            "oidc: identityTokenOnly without client ids",
            json!({ "openIdConnectConfiguration": {
                "issuer": OIDC_ISSUER,
                "entityIdPrefix": "Tenant",
                "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "email" } },
            } }),
            json!({ "openIdConnectConfiguration": {
                "issuer": OIDC_ISSUER,
                "entityIdPrefix": "Tenant",
                "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "email" } },
            } }),
            Value::Null,
        ),
    ];

    let store_id = store_with_validation_off(&router).await;
    for (case, provider, configuration, details) in cases {
        let created = created_with(&router, &store_id, &provider).await;
        let updated = updated_to(&router, &store_id, &provider).await;

        let from_create = read(&router, &store_id, &created).await;
        let from_update = read(&router, &store_id, &updated).await;

        assert_eq!(
            from_create["configuration"], configuration,
            "case {case:?}: expected the created source's configuration {configuration}, got {:?}",
            from_create["configuration"]
        );
        assert_eq!(
            from_update["configuration"], from_create["configuration"],
            "case {case:?}: expected the updated source's configuration {:?}, got {:?}",
            from_create["configuration"], from_update["configuration"]
        );
        assert_eq!(
            from_create["details"], details,
            "case {case:?}: expected the created source's details {details}, got {:?}",
            from_create["details"]
        );
        assert_eq!(
            from_update["details"], from_create["details"],
            "case {case:?}: expected the updated source's details {:?}, got {:?}",
            from_create["details"], from_update["details"]
        );
        assert_eq!(
            from_update["principalEntityType"], from_create["principalEntityType"],
            "case {case:?}: expected principalEntityType {:?}, got {:?}",
            from_create["principalEntityType"], from_update["principalEntityType"]
        );
    }
}

/// RULE: a provider reference this emulator cannot resolve is refused the same
/// way whichever writer carried it — the same HTTP 400 `ValidationException`
/// refusing the same member for the same reason — and neither writer persists
/// anything. The `fieldList` pointer is rooted at the request member the refused
/// OPERATION declares (`configuration` on the create, `updateConfiguration` on
/// the update), so the two refusals differ in that root and in nothing else.
///
/// The rows differ along WHY the reference does not resolve: the ARN's account,
/// its region, the region the pool id itself names, its partition, and a pool id
/// no user pool carries. An ARN
/// that does not even have a user pool ARN's SHAPE is refused by the modelled
/// pattern on the member before either handler runs, so it says nothing about the
/// translation and is not a row here.
#[tokio::test]
async fn both_writers_refuse_an_unresolvable_pool_reference_identically() {
    let account = engine::emulator_account_id();
    // (case, the userPoolArn the writers carry)
    let cases: Vec<(&str, String)> = vec![
        (
            // Every resource this emulator holds is in `emulator_account_id()`.
            "another account",
            format!("arn:aws:cognito-idp:{REGION}:123456789012:userpool/{REGION}_1a2b3c4d5"),
        ),
        (
            // A pool has to be in the policy store's own region.
            "another region",
            user_pool_arn("eu-west-1", "eu-west-1_1a2b3c4d5"),
        ),
        (
            // A user pool id names the region the pool was created in, so an id
            // that disagrees with its own ARN names no pool at all.
            "a pool id naming another region",
            user_pool_arn(REGION, "eu-west-1_1a2b3c4d5"),
        ),
        (
            // A pool in another partition is not reachable from this one, even
            // with this emulator's own account and the request's own region.
            "another partition",
            format!("arn:aws-us-gov:cognito-idp:{REGION}:{account}:userpool/{REGION}_1a2b3c4d5"),
        ),
        (
            "another partition and another account",
            "arn:aws-us-gov:cognito-idp:us-east-1:210987654321:userpool/us-east-1_9z8y7x6w5"
                .to_string(),
        ),
        (
            // The ARN has every segment this emulator serves, and still names no
            // pool: nothing ever created one with that id.
            "a pool that was never created",
            user_pool_arn(REGION, &format!("{REGION}_nevercreated")),
        ),
    ];

    let router = router();
    let store_id = store_with_validation_off(&router).await;
    // One live source the update rows aim at, so a refusal is the configuration's
    // and not the source's absence — and so "nothing was written" is observable.
    let target = created_with(&router, &store_id, &seed_provider(&router).await).await;
    let before = read(&router, &store_id, &target).await;

    for (case, arn) in cases {
        let provider = json!({ "cognitoUserPoolConfiguration": { "userPoolArn": arn } });

        let (create_status, create_body) = call(
            &router,
            "CreateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "principalEntityType": PRINCIPAL_ENTITY_TYPE,
                "configuration": provider,
            }),
        )
        .await;
        let (update_status, update_body) = call(
            &router,
            "UpdateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "identitySourceId": target,
                "updateConfiguration": provider,
            }),
        )
        .await;

        assert_eq!(
            create_status, 400,
            "case {case:?}: expected HTTP 400 from the create for {arn}, got {create_status} {create_body}"
        );
        assert_eq!(
            update_status, create_status,
            "case {case:?}: expected HTTP {create_status} from the update for {arn}, got {update_status} {update_body}"
        );
        assert_eq!(
            error_code(&create_body),
            "ValidationException",
            "case {case:?}: expected a ValidationException from the create for {arn}, got {create_body}"
        );
        assert_eq!(
            error_code(&update_body),
            error_code(&create_body),
            "case {case:?}: expected a {} from the update for {arn}, got {update_body}",
            error_code(&create_body)
        );
        let refused = field_list(&create_body);
        assert_eq!(
            refused.len(),
            1,
            "case {case:?}: expected one refused member for {arn}, got {refused:?}"
        );
        let answered = field_list(&update_body);
        assert_eq!(
            answered.len(),
            1,
            "case {case:?}: expected one refused member from the update for {arn}, got {answered:?}"
        );
        // The REASON comes from the one shared translation, so it is identical;
        // the pointer differs only in the input member each operation declares,
        // which `identity_source_refusal_pointers` pins per operation.
        assert_eq!(
            answered[0].1, refused[0].1,
            "case {case:?}: expected the update to refuse for the same reason ({:?}), got {:?}",
            refused[0].1, answered[0].1
        );
        let (create_root, update_root) = (
            refused[0].0.strip_suffix(POOL_ARN_IN_CONFIGURATION),
            answered[0].0.strip_suffix(POOL_ARN_IN_CONFIGURATION),
        );
        assert_eq!(
            (create_root, update_root),
            (Some("/configuration"), Some("/updateConfiguration")),
            "case {case:?}: expected each refusal to name {POOL_ARN_IN_CONFIGURATION} under its \
             own operation's input member, got {:?} and {:?}",
            refused[0].0,
            answered[0].0
        );

        // Neither writer wrote: the live source still reads back as it was, and
        // the refused create added nothing to the store.
        let after = read(&router, &store_id, &target).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must change nothing, got {after} after {before}"
        );
        let listed = ok(
            &router,
            "ListIdentitySources",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        let count = listed["identitySources"]
            .as_array()
            .map_or(0, |sources| sources.len());
        assert_eq!(
            count, 1,
            "case {case:?}: a refused create must persist nothing, got {listed}"
        );
    }
}
