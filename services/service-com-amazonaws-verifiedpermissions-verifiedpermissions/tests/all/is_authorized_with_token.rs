// SPDX-License-Identifier: CC0-1.0

//! `IsAuthorizedWithToken`: the decision for a principal the REQUEST never names.
//!
//! The rule under test: an authorization call decides for the principal a token's
//! VERIFIED claims name — the token is a compact JWS, its issuer is one of the policy
//! store's identity sources, that issuer signed it, it is the kind of token the member it
//! arrived in takes, it was issued to a client the source accepts and the clock has not
//! run out on it — and the principal's id, its groups and its attributes then come from
//! those claims and from nowhere else. A token that fails any of those is refused with
//! `ValidationException`, never answered with a decision.
//!
//! Every token here is minted by the Amazon Cognito user-pool service mounted on the same
//! engine and signed with that pool's own key, so the signature check is the real one:
//! nothing in this suite hands the service a token it made up.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, error_code, fault_message};

/// A password satisfying a default user pool's password policy.
const PASSWORD: &str = "Passw0rd!2024";

/// The entity type the identity sources in this suite generate principals as.
const PRINCIPAL_TYPE: &str = "App::User";

/// The entity type they map user-pool groups to.
const GROUP_TYPE: &str = "App::UserGroup";

/// The group a permit policy in this suite is written over.
const PERMITTED_GROUP: &str = "admins";

/// `bytes` as one base64url segment, unpadded, as a compact JWS carries it
/// (RFC 7515 §2).
fn base64url(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// One base64url segment's bytes.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url: {error}"))
}

/// The three segments of a compact JWS.
fn segments(token: &str) -> (String, String, String) {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "an issued token is three '.'-separated segments, got {token:?}"
    );
    (
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    )
}

/// `token` with `claims` merged into its payload and its ORIGINAL signature kept: the
/// forgery a client makes when it wants claims the issuer never signed.
fn with_forged_claims(token: &str, claims: &Value) -> String {
    let (header, payload, signature) = segments(token);
    let mut forged: Value = serde_json::from_slice(&decode_base64url(&payload))
        .expect("an issued token's payload is JSON");
    for (claim, value) in claims.as_object().expect("the forged claims are an object") {
        forged
            .as_object_mut()
            .expect("the payload is an object")
            .insert(claim.clone(), value.clone());
    }
    format!(
        "{header}.{}.{signature}",
        base64url(forged.to_string().as_bytes())
    )
}

/// `token` with one byte of its signature changed, inside the base64url alphabet.
fn with_tampered_signature(token: &str) -> String {
    let (header, payload, signature) = segments(token);
    let mut characters: Vec<char> = signature.chars().collect();
    let last = characters.len() - 1;
    characters[last] = if characters[last] == 'A' { 'B' } else { 'A' };
    let tampered: String = characters.into_iter().collect();
    format!("{header}.{payload}.{tampered}")
}

/// A user signed in to a user pool: the pool, the app client and the identity the tokens
/// name, plus the tokens themselves.
struct SignedIn {
    user_pool_id: String,
    client_id: String,
    sub: String,
    access_token: String,
    id_token: String,
}

impl SignedIn {
    /// The principal entity id Verified Permissions generates for this identity:
    /// `<user pool id>|<sub>`.
    fn entity_id(&self) -> String {
        format!("{}|{}", self.user_pool_id, self.sub)
    }
}

/// Create a user pool and return its id.
async fn user_pool(router: &MultiServiceRouter, name: &str) -> String {
    harness::cognito(router, "CreateUserPool", &json!({ "PoolName": name }))
        .await["UserPool"]["Id"]
        .as_str()
        .expect("a minted user pool id")
        .to_string()
}

/// Create an app client of `pool` that admin password sign-in is enabled for.
async fn app_client(router: &MultiServiceRouter, pool: &str, name: &str) -> String {
    harness::cognito(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": pool,
            "ClientName": name,
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted app client id")
        .to_string()
}

/// Create `username` in `pool` with `attributes`, put it in `groups`, and sign it in
/// through `client`.
async fn sign_in(
    router: &MultiServiceRouter,
    pool: &str,
    client: &str,
    username: &str,
    groups: &[&str],
    attributes: &Value,
) -> SignedIn {
    let created = harness::cognito(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": pool,
            "Username": username,
            "UserAttributes": attributes,
        }),
    )
    .await;
    let sub = created["User"]["Attributes"]
        .as_array()
        .expect("the stored user attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string();
    harness::cognito(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": pool,
            "Username": username,
            "Password": PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    for group in groups {
        harness::cognito(
            router,
            "CreateGroup",
            &json!({ "UserPoolId": pool, "GroupName": group }),
        )
        .await;
        harness::cognito(
            router,
            "AdminAddUserToGroup",
            &json!({ "UserPoolId": pool, "Username": username, "GroupName": group }),
        )
        .await;
    }
    let result = reauthenticate(router, pool, client, username).await;
    SignedIn {
        user_pool_id: pool.to_string(),
        client_id: client.to_string(),
        sub,
        access_token: result.0,
        id_token: result.1,
    }
}

/// Sign `username` in again and return the `(access token, ID token)` the pool issues.
async fn reauthenticate(
    router: &MultiServiceRouter,
    pool: &str,
    client: &str,
    username: &str,
) -> (String, String) {
    let result = harness::cognito(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": pool,
            "ClientId": client,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": PASSWORD },
        }),
    )
    .await;
    let token = |member: &str| {
        result["AuthenticationResult"][member]
            .as_str()
            .unwrap_or_else(|| panic!("a sign-in returns {member}, got {result}"))
            .to_string()
    };
    (token("AccessToken"), token("IdToken"))
}

/// Create an identity source for `pool` in `store`, accepting `client_ids` and mapping
/// groups to `group_entity_type` when one is given, and return its `identitySourceId`.
async fn identity_source(
    router: &MultiServiceRouter,
    store: &str,
    pool: &str,
    client_ids: &[&str],
    group_entity_type: Option<&str>,
) -> String {
    let mut cognito = json!({
        "userPoolArn": harness::user_pool_arn(harness::SIGNING_REGION, pool),
        "clientIds": client_ids,
    });
    if let Some(entity_type) = group_entity_type {
        cognito["groupConfiguration"] = json!({ "groupEntityType": entity_type });
    }
    harness::ok(
        router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store,
            "principalEntityType": PRINCIPAL_TYPE,
            "configuration": { "cognitoUserPoolConfiguration": cognito },
        }),
    )
    .await["identitySourceId"]
        .as_str()
        .expect("a minted identitySourceId")
        .to_string()
}

/// The issuer a user pool's tokens claim, in the form the model documents for
/// `CognitoUserPoolConfigurationDetail$issuer`:
/// `https://cognito-idp.<region>.amazonaws.com/<user pool id>`.
fn pool_issuer(pool: &str) -> String {
    format!(
        "https://cognito-idp.{}.amazonaws.com/{pool}",
        harness::SIGNING_REGION
    )
}

/// The refusal a token-based authorization call answers when the policy store it addressed
/// has no identity source for the issuer the token claims.
fn no_identity_source_message(issuer: &str, store: &str) -> String {
    format!("Identity Source does not exist for issuer {issuer} in policy store {store}")
}

/// Store a static policy and return its `policyId`.
async fn policy(router: &MultiServiceRouter, store: &str, statement: String) -> String {
    harness::ok(
        router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store,
            "definition": { "static": { "statement": statement } },
        }),
    )
    .await["policyId"]
        .as_str()
        .expect("a minted policyId")
        .to_string()
}

/// The `permit` policy this suite writes over a group: the group's members may read
/// [`DOCUMENT`].
fn group_permit(group_type: &str, pool: &str) -> String {
    format!(
        "permit(principal in {group_type}::\"{pool}|{PERMITTED_GROUP}\", \
         action == App::Action::\"read\", resource == App::Doc::\"{DOCUMENT}\");"
    )
}

/// The resource every request in this suite asks about.
const DOCUMENT: &str = "doc-1";

/// One `IsAuthorizedWithToken` request against `store`, carrying whatever token members
/// `tokens` names.
fn request(store: &str, tokens: Value) -> Value {
    let mut body = json!({
        "policyStoreId": store,
        "action": { "actionType": "App::Action", "actionId": "read" },
        "resource": { "entityType": "App::Doc", "entityId": DOCUMENT },
    });
    for (member, value) in tokens.as_object().expect("the token members are an object") {
        body[member.clone()] = value.clone();
    }
    body
}

/// What a row expects of the call.
enum Expected {
    /// A 200 carrying this decision for this principal entity id, determined by these
    /// policies.
    Decision {
        decision: &'static str,
        entity_id: String,
        determining: Vec<String>,
    },
    /// A 400 `ValidationException` whose message carries this clause.
    Refused(String),
    /// A 400 `ValidationException` whose `fieldList` names these request members.
    InvalidInput(Vec<&'static str>),
    /// A 404 `ResourceNotFoundException` for this resource: the call referenced something
    /// the policy store does not hold.
    NotFound {
        resource_type: &'static str,
        resource_id: String,
    },
}

/// A decision is made for the principal a verified token names, and a token that does not
/// verify is refused rather than decided.
#[tokio::test]
async fn a_decision_follows_the_claims_of_a_token_the_issuer_signed() {
    let (router, _engine) = harness::router_with_user_pools();

    // One pool with two app clients and two users — one in the permitted group, one in no
    // group — plus a SECOND pool nothing in the policy store references.
    let pool = user_pool(&router, "avp-pool").await;
    let client = app_client(&router, &pool, "avp-client").await;
    let unlisted_client = app_client(&router, &pool, "unlisted-client").await;
    let member = sign_in(
        &router,
        &pool,
        &client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;
    let outsider = sign_in(&router, &pool, &client, "outsider", &[], &json!([])).await;
    let through_unlisted = reauthenticate(&router, &pool, &unlisted_client, "member").await;

    let foreign_pool = user_pool(&router, "foreign-pool").await;
    let foreign_client = app_client(&router, &foreign_pool, "foreign-client").await;
    let foreign = sign_in(
        &router,
        &foreign_pool,
        &foreign_client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;

    let store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &store, &pool, &[&client], Some(GROUP_TYPE)).await;
    let permit = policy(&router, &store, group_permit(GROUP_TYPE, &pool)).await;

    // The rows differ along what the rule generalises over: which member carried the
    // token, which identity it names, which pool and app client issued it, and what a
    // presenter changed about it after it was issued.
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "a group member's identity token",
            request(&store, json!({ "identityToken": member.id_token })),
            Expected::Decision {
                decision: "ALLOW",
                entity_id: member.entity_id(),
                determining: vec![permit.clone()],
            },
        ),
        (
            "the same member's access token",
            request(&store, json!({ "accessToken": member.access_token })),
            Expected::Decision {
                decision: "ALLOW",
                entity_id: member.entity_id(),
                determining: vec![permit.clone()],
            },
        ),
        (
            "both of the member's tokens",
            request(
                &store,
                json!({
                    "accessToken": member.access_token,
                    "identityToken": member.id_token,
                }),
            ),
            Expected::Decision {
                decision: "ALLOW",
                entity_id: member.entity_id(),
                determining: vec![permit.clone()],
            },
        ),
        (
            "a non-member's identity token",
            request(&store, json!({ "identityToken": outsider.id_token })),
            Expected::Decision {
                decision: "DENY",
                entity_id: outsider.entity_id(),
                determining: Vec::new(),
            },
        ),
        (
            "a non-member's identity token with the group claim forged into it",
            request(
                &store,
                json!({
                    "identityToken": with_forged_claims(
                        &outsider.id_token,
                        &json!({ "cognito:groups": [PERMITTED_GROUP] }),
                    ),
                }),
            ),
            Expected::Refused("signature is invalid".to_string()),
        ),
        (
            "a member's identity token with its signature tampered with",
            request(
                &store,
                json!({ "identityToken": with_tampered_signature(&member.id_token) }),
            ),
            Expected::Refused("signature is invalid".to_string()),
        ),
        (
            "a member's access token with its signature tampered with",
            request(
                &store,
                json!({ "accessToken": with_tampered_signature(&member.access_token) }),
            ),
            Expected::Refused("signature is invalid".to_string()),
        ),
        (
            "an identity token from a user pool the policy store does not reference",
            request(&store, json!({ "identityToken": foreign.id_token })),
            Expected::NotFound {
                resource_type: "IDENTITY_SOURCE",
                resource_id: pool_issuer(&foreign_pool),
            },
        ),
        (
            "an identity token issued to an app client the identity source does not list",
            request(&store, json!({ "identityToken": through_unlisted.1 })),
            Expected::Refused(format!(
                "its aud claim {unlisted_client:?} is not one of the clientIds the identity \
                 source accepts"
            )),
        ),
        (
            "an access token issued to an app client the identity source does not list",
            request(&store, json!({ "accessToken": through_unlisted.0 })),
            Expected::Refused(format!(
                "its client_id claim {unlisted_client:?} is not one of the clientIds the \
                 identity source accepts"
            )),
        ),
        (
            "an access token presented as the identity token",
            request(&store, json!({ "identityToken": member.access_token })),
            Expected::Refused("its token_use claim is \"access\", not \"id\"".to_string()),
        ),
        (
            "an identity token presented as the access token",
            request(&store, json!({ "accessToken": member.id_token })),
            Expected::Refused("its token_use claim is \"id\", not \"access\"".to_string()),
        ),
        (
            "three segments that are not a token",
            request(&store, json!({ "identityToken": "aaaa.bbbb.cccc" })),
            Expected::Refused("base64url".to_string()),
        ),
        (
            "an unsecured token carrying the member's own claims",
            request(
                &store,
                json!({
                    "identityToken": format!(
                        "{}.{}.{}",
                        base64url(br#"{"alg":"none","typ":"JWT"}"#),
                        segments(&member.id_token).1,
                        base64url(b"signature"),
                    ),
                }),
            ),
            Expected::Refused("unsecured".to_string()),
        ),
        (
            "no token at all",
            request(&store, json!({})),
            Expected::InvalidInput(vec!["/accessToken", "/identityToken"]),
        ),
        (
            "two tokens naming two different principals",
            request(
                &store,
                json!({
                    "accessToken": outsider.access_token,
                    "identityToken": member.id_token,
                }),
            ),
            Expected::InvalidInput(vec!["/accessToken", "/identityToken"]),
        ),
        (
            "a Cedar-JSON entities payload that is no entity store",
            {
                let mut body = request(&store, json!({ "identityToken": member.id_token }));
                body["entities"] = json!({ "cedarJson": "{\"not\":\"a list\"}" });
                body
            },
            Expected::InvalidInput(vec!["/entities/cedarJson"]),
        ),
        (
            "an entity of the group type the identity source maps, for a token naming none",
            {
                let mut body = request(&store, json!({ "identityToken": outsider.id_token }));
                body["entities"] = json!({
                    "entityList": [{
                        "identifier": {
                            "entityType": GROUP_TYPE,
                            "entityId": format!("{pool}|{PERMITTED_GROUP}"),
                        },
                    }],
                });
                body
            },
            Expected::InvalidInput(vec!["/entities"]),
        ),
        (
            "an entity of the type the identity source itself generates",
            {
                let mut body = request(&store, json!({ "identityToken": member.id_token }));
                body["entities"] = json!({
                    "entityList": [{
                        "identifier": {
                            "entityType": PRINCIPAL_TYPE,
                            "entityId": member.entity_id(),
                        },
                    }],
                });
                body
            },
            Expected::InvalidInput(vec!["/entities"]),
        ),
    ];

    for (case, body, expected) in rows {
        let (status, answered) = harness::call(&router, "IsAuthorizedWithToken", &body).await;
        match expected {
            Expected::Decision {
                decision,
                entity_id,
                determining,
            } => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a decision, got {status} with {answered}"
                );
                assert_eq!(
                    answered["decision"].as_str(),
                    Some(decision),
                    "case {case:?}: expected decision {decision}, got {answered}"
                );
                assert_eq!(
                    answered["principal"],
                    json!({ "entityType": PRINCIPAL_TYPE, "entityId": entity_id }),
                    "case {case:?}: expected the principal the token names, got {answered}"
                );
                let reported: Vec<String> = answered["determiningPolicies"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item["policyId"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    reported, determining,
                    "case {case:?}: expected the determining policies {determining:?}, got \
                     {answered}"
                );
                assert_eq!(
                    answered["errors"],
                    json!([]),
                    "case {case:?}: a clean evaluation reports no errors, got {answered}"
                );
            }
            Expected::Refused(clause) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} with \
                     {answered}"
                );
                let message = fault_message(&answered);
                assert!(
                    message.contains(&clause),
                    "case {case:?}: expected a message carrying {clause:?}, got {message:?}"
                );
                assert!(
                    message.starts_with("Failed to verify "),
                    "case {case:?}: a token that does not verify is refused as such, got \
                     {message:?}"
                );
            }
            Expected::InvalidInput(members) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} with \
                     {answered}"
                );
                let named: Vec<String> = answered["fieldList"]
                    .as_array()
                    .map(|fields| {
                        fields
                            .iter()
                            .filter_map(|field| field["path"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    named, members,
                    "case {case:?}: expected fieldList to name {members:?}, got {answered}"
                );
            }
            Expected::NotFound {
                resource_type,
                resource_id,
            } => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: expected a 404 ResourceNotFoundException, got {status} \
                     with {answered}"
                );
                assert_eq!(
                    (
                        answered["resourceType"].as_str(),
                        answered["resourceId"].as_str()
                    ),
                    (Some(resource_type), Some(resource_id.as_str())),
                    "case {case:?}: expected the missing resource to be reported, got \
                     {answered}"
                );
                assert_eq!(
                    fault_message(&answered),
                    no_identity_source_message(&resource_id, &store),
                    "case {case:?}: a token whose issuer the addressed policy store has no \
                     identity source for is refused naming that issuer and that store"
                );
            }
        }
    }
}

/// A user pool's group claim is a HIERARCHY the principal is `in`, under the entity type
/// the identity source configures — or the service's own default where it configures none.
#[tokio::test]
async fn a_group_claim_maps_to_the_group_entity_type_the_source_configures() {
    let (router, _engine) = harness::router_with_user_pools();
    let pool = user_pool(&router, "group-type-pool").await;
    let client = app_client(&router, &pool, "group-type-client").await;
    let member = sign_in(
        &router,
        &pool,
        &client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;

    // The rows differ along what the source configures: an entity type of the store's own
    // namespace, and none at all — which is the documented `AWS::CognitoGroup` default.
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("a configured group entity type", Some(GROUP_TYPE), GROUP_TYPE),
        ("no group configuration", None, "AWS::CognitoGroup"),
    ];

    for (case, configured, expected_type) in rows {
        let store = harness::store_with_validation_off(&router).await;
        identity_source(&router, &store, &pool, &[&member.client_id], configured).await;
        let permit = policy(&router, &store, group_permit(expected_type, &pool)).await;
        let answered = harness::ok(
            &router,
            "IsAuthorizedWithToken",
            &request(&store, json!({ "identityToken": member.id_token })),
        )
        .await;
        assert_eq!(
            answered["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: a policy over {expected_type} must match the token's group, got \
             {answered}"
        );
        assert_eq!(
            answered["determiningPolicies"][0]["policyId"].as_str(),
            Some(permit.as_str()),
            "case {case:?}: the group policy must be the determining one, got {answered}"
        );
    }
}

/// A claim is in force for exactly as long as a token carrying it is: a membership the
/// user pool has since changed applies only from the token it issues NEXT, and the token
/// already in a caller's hands keeps deciding as it did.
#[tokio::test]
async fn a_group_change_applies_only_to_a_reissued_token() {
    let (router, _engine) = harness::router_with_user_pools();
    let pool = user_pool(&router, "reissue-pool").await;
    let client = app_client(&router, &pool, "reissue-client").await;
    let member = sign_in(
        &router,
        &pool,
        &client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;
    let store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &store, &pool, &[&client], Some(GROUP_TYPE)).await;
    policy(&router, &store, group_permit(GROUP_TYPE, &pool)).await;

    let decision = |token: String| {
        let router = &router;
        let store = store.clone();
        async move {
            harness::ok(
                router,
                "IsAuthorizedWithToken",
                &request(&store, json!({ "identityToken": token })),
            )
            .await["decision"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
    };

    assert_eq!(
        decision(member.id_token.clone()).await,
        "ALLOW",
        "case the member's own token: a group member is permitted"
    );

    harness::cognito(
        &router,
        "AdminRemoveUserFromGroup",
        &json!({ "UserPoolId": pool, "Username": "member", "GroupName": PERMITTED_GROUP }),
    )
    .await;

    assert_eq!(
        decision(member.id_token.clone()).await,
        "ALLOW",
        "case the token issued before the removal: its claims are what the decision follows"
    );
    let (_, reissued) = reauthenticate(&router, &pool, &client, "member").await;
    assert_ne!(
        reissued, member.id_token,
        "case the reissued token: the pool must issue a new token to carry new claims"
    );
    assert_eq!(
        decision(reissued).await,
        "DENY",
        "case the token issued after the removal: the membership is gone from its claims"
    );
}

/// Every other claim of a verified token is an ATTRIBUTE of the principal it names, so a
/// policy conditioned on one reads it off the principal — and the scope claim, which
/// travels as one space-delimited string, is the set of scopes it lists.
#[tokio::test]
async fn a_policy_over_a_claim_reads_it_from_the_principals_attributes() {
    let (router, _engine) = harness::router_with_user_pools();
    let pool = user_pool(&router, "claims-pool").await;
    let client = app_client(&router, &pool, "claims-client").await;
    let listed = sign_in(
        &router,
        &pool,
        &client,
        "listed",
        &[],
        &json!([{ "Name": "email", "Value": "listed@example.test" }]),
    )
    .await;
    let other = sign_in(
        &router,
        &pool,
        &client,
        "other",
        &[],
        &json!([{ "Name": "email", "Value": "other@example.test" }]),
    )
    .await;

    let email_store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &email_store, &pool, &[&client], Some(GROUP_TYPE)).await;
    policy(
        &router,
        &email_store,
        format!(
            "permit(principal, action == App::Action::\"read\", \
             resource == App::Doc::\"{DOCUMENT}\") \
             when {{ principal.email == \"listed@example.test\" }};"
        ),
    )
    .await;
    // A second store, because a policy reading a claim ONE kind of token carries would
    // otherwise report an in-band evaluation error on every request carrying the other.
    let scope_store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &scope_store, &pool, &[&client], Some(GROUP_TYPE)).await;
    policy(
        &router,
        &scope_store,
        format!(
            "permit(principal, action == App::Action::\"read\", \
             resource == App::Doc::\"{DOCUMENT}\") \
             when {{ principal.scope.contains(\"aws.cognito.signin.user.admin\") }};"
        ),
    )
    .await;

    // The rows differ along which claim the decision turns on and which token carries it:
    // an ID token's string attribute, the same token for an identity the policy does not
    // name, and an access token's `scope` SET.
    let rows: Vec<(&str, &String, Value, &str)> = vec![
        (
            "an ID token whose email claim the policy names",
            &email_store,
            json!({ "identityToken": listed.id_token }),
            "ALLOW",
        ),
        (
            "an ID token whose email claim it does not",
            &email_store,
            json!({ "identityToken": other.id_token }),
            "DENY",
        ),
        (
            "an access token whose scope claim the policy reads as a set",
            &scope_store,
            json!({ "accessToken": other.access_token }),
            "ALLOW",
        ),
    ];

    for (case, store, tokens, expected) in rows {
        let answered =
            harness::ok(&router, "IsAuthorizedWithToken", &request(store, tokens)).await;
        assert_eq!(
            answered["decision"].as_str(),
            Some(expected),
            "case {case:?}: expected {expected}, got {answered}"
        );
        assert_eq!(
            answered["errors"],
            json!([]),
            "case {case:?}: the claim the policy reads is an attribute of the principal, so \
             nothing errors: {answered}"
        );
    }
}

/// A token's issuer is matched against the identity sources of the policy store the request
/// ADDRESSED, so a store holding no source for that issuer refuses the call as a missing
/// identity source and names BOTH halves of that reference: the issuer the token claims and
/// the store that was searched. The store itself stays usable — a token from an issuer it
/// does cover decides as it did before the refusal and as it does after.
#[tokio::test]
async fn an_issuer_with_no_identity_source_is_refused_naming_the_issuer_and_the_store() {
    let (router, _engine) = harness::router_with_user_pools();

    // One pool a store's identity source names, and two pools no store references — so a
    // row below differs from the covered call in the `iss` claim alone.
    let home_pool = user_pool(&router, "home-pool").await;
    let home_client = app_client(&router, &home_pool, "home-client").await;
    let home = sign_in(
        &router,
        &home_pool,
        &home_client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;
    let second_pool = user_pool(&router, "second-pool").await;
    let second_client = app_client(&router, &second_pool, "second-client").await;
    let second = sign_in(
        &router,
        &second_pool,
        &second_client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;
    let third_pool = user_pool(&router, "third-pool").await;
    let third_client = app_client(&router, &third_pool, "third-client").await;
    let third = sign_in(
        &router,
        &third_pool,
        &third_client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;

    // Three stores, each with its own minted id: one covering the home pool, one holding
    // no identity source at all, and one whose source for the home pool was deleted.
    let covered = harness::store_with_validation_off(&router).await;
    identity_source(
        &router,
        &covered,
        &home_pool,
        &[&home_client],
        Some(GROUP_TYPE),
    )
    .await;
    let permit = policy(&router, &covered, group_permit(GROUP_TYPE, &home_pool)).await;
    let sourceless = harness::store_with_validation_off(&router).await;
    let emptied = harness::store_with_validation_off(&router).await;
    let deleted_source = identity_source(
        &router,
        &emptied,
        &home_pool,
        &[&home_client],
        Some(GROUP_TYPE),
    )
    .await;
    harness::ok(
        &router,
        "DeleteIdentitySource",
        &json!({ "policyStoreId": emptied, "identitySourceId": deleted_source }),
    )
    .await;

    let covered_decision = |token: String| {
        let router = &router;
        let store = covered.clone();
        async move {
            harness::ok(
                router,
                "IsAuthorizedWithToken",
                &request(&store, json!({ "identityToken": token })),
            )
            .await
        }
    };

    // The control, before any refusal: the store decides for the issuer it does hold.
    let before = covered_decision(home.id_token.clone()).await;
    assert_eq!(
        (
            before["decision"].as_str(),
            before["determiningPolicies"][0]["policyId"].as_str()
        ),
        (Some("ALLOW"), Some(permit.as_str())),
        "case the covered issuer before the refusals: expected ALLOW on {permit}, got {before}"
    );

    // The rows differ along what the rule generalises over: which member carried the token,
    // which issuer minted it, and WHY the addressed store holds no source for that issuer —
    // it covers another pool, it holds no source at all, or the source it held was deleted.
    let rows: Vec<(&str, &String, Value, String)> = vec![
        (
            "an identity token from a pool the store does not cover",
            &covered,
            json!({ "identityToken": second.id_token }),
            pool_issuer(&second_pool),
        ),
        (
            "the same identity's access token from that pool",
            &covered,
            json!({ "accessToken": second.access_token }),
            pool_issuer(&second_pool),
        ),
        (
            "both tokens from that pool",
            &covered,
            json!({
                "accessToken": second.access_token,
                "identityToken": second.id_token,
            }),
            pool_issuer(&second_pool),
        ),
        (
            "an identity token from a third pool, naming a different issuer",
            &covered,
            json!({ "identityToken": third.id_token }),
            pool_issuer(&third_pool),
        ),
        (
            "the covered pool's own token against a store holding no identity source",
            &sourceless,
            json!({ "identityToken": home.id_token }),
            pool_issuer(&home_pool),
        ),
        (
            "an access token against a store holding no identity source",
            &sourceless,
            json!({ "accessToken": third.access_token }),
            pool_issuer(&third_pool),
        ),
        (
            "a token whose issuer's identity source was deleted from the store",
            &emptied,
            json!({ "identityToken": home.id_token }),
            pool_issuer(&home_pool),
        ),
    ];

    for (case, store, tokens, issuer) in rows {
        let (status, answered) =
            harness::call(&router, "IsAuthorizedWithToken", &request(store, tokens)).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {status} with \
             {answered}"
        );
        let message = fault_message(&answered);
        assert_eq!(
            message,
            no_identity_source_message(&issuer, store),
            "case {case:?}: expected the issuer and the addressed store to be named, got \
             {message:?}"
        );
        assert_eq!(
            (
                answered["resourceType"].as_str(),
                answered["resourceId"].as_str()
            ),
            (Some("IDENTITY_SOURCE"), Some(issuer.as_str())),
            "case {case:?}: expected the missing identity source to be reported as the \
             issuer, got {answered}"
        );
    }

    // The control again: a refusal decided nothing and consumed nothing, so the store still
    // decides for the issuer it holds a source for.
    let after = covered_decision(home.id_token.clone()).await;
    assert_eq!(
        (
            after["decision"].as_str(),
            after["determiningPolicies"][0]["policyId"].as_str()
        ),
        (Some("ALLOW"), Some(permit.as_str())),
        "case the covered issuer after the refusals: expected ALLOW on {permit}, got {after}"
    );
}
