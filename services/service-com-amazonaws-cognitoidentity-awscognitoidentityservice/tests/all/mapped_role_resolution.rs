// SPDX-License-Identifier: CC0-1.0

//! WHICH role a pool's `RoleMappings` resolve for a vend.
//!
//! RULE: where an identity pool carries a `RoleMappings` entry keyed by the identity
//! provider a presented login was made under, that mapping decides the role
//! `GetCredentialsForIdentity` vends a session of — for a `Rules` mapping, the `RoleARN` of
//! the first rule, in the DECLARED order the rules were stored in, whose match condition
//! holds for the claims of the presented token ("Rules are evaluated in order. The first one
//! to match specifies the role", `RulesConfigurationType$Rules`) — and the pool-wide
//! `authenticated`/`unauthenticated` role is the fallback for an exchange no mapping of the
//! pool's resolves a role for. The role the vend stands for is observable through
//! `sts:GetCallerIdentity` under the vended credential, which answers
//! `arn:<partition>:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials`.
//!
//! The rows differ along what the rule generalises over: the ORDER the rules are declared
//! in and which of them match, the MATCH TYPE each rule states and the claim value it is
//! applied to (single-valued, multi-valued, numeric, absent), the KEY the mapping is held
//! under and the provider the login is made under, and the flow the request is in.
//!
//! The harness is `vended_role_session`'s: this service and STS on ONE shared engine behind
//! the same `MultiServiceRouter` the production router builds, with the IAM roles the
//! assume's decision reads seeded into that same engine.

use serde_json::{json, Value};

use crate::vended_role_session::{element, expected_session_arn, vended, Emulator};

/// The Cognito Identity principal a role's trust policy names to admit an identity pool's
/// users, as `AssumeRoleWithWebIdentityRequest$RoleArn` documents it.
const COGNITO_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// The `Logins` provider name of the Amazon Cognito user pool the fixture pool trusts: a
/// user pool is named `cognito-idp.<region>.amazonaws.com/<UserPoolId>` (`GetId.Logins`).
///
/// `pub(crate)` — with the fixture helpers below — so the sibling suite that asserts what
/// becomes of an exchange a mapping resolves NO role for drives the same fixture rather
/// than a second copy of it.
pub(crate) const USER_POOL_PROVIDER: &str =
    "cognito-idp.us-east-1.amazonaws.com/us-east-1_mapped01";

/// The app client of that user pool the fixture pool trusts, which the identifier a role
/// mapping of that provider is keyed by names beside the provider name.
pub(crate) const CLIENT_ID: &str = "mappedclient1";

/// A SECOND app client of the same user pool the fixture pool trusts, so a mapping keyed by
/// one client is told apart from a mapping keyed by the other.
const OTHER_CLIENT_ID: &str = "otherclient22";

/// A public login provider the fixture pool accepts, identified by its NAME alone.
pub(crate) const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// One role of the fixture, by the two things a row needs: the ARN a mapping rule names it
/// by and the NAME the assumed-role ARN of a session of it carries.
pub(crate) struct Role {
    pub(crate) name: String,
    pub(crate) arn: String,
}

/// One base64url segment, unpadded, as a compact JWS carries it (RFC 7515 §2).
fn segment(value: &str) -> String {
    aws_smithy_types::base64::encode(value)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// An identity token as a provider issues one: a compact JWS whose header names a signing
/// algorithm, whose payload is `claims` and whose signature segment is not empty (RFC 7515
/// §3.1, RFC 7519 §3). Nothing in this emulator can verify the signature — that needs the
/// issuer's published key — so the claims are what a role mapping reads.
pub(crate) fn identity_token(claims: &Value) -> String {
    format!(
        "{}.{}.{}",
        segment(r#"{"alg":"RS256","kid":"kid-1","typ":"JWT"}"#),
        segment(&claims.to_string()),
        segment("a-signature")
    )
}

/// The claim set of a user-pool ID token for `subject`, carrying the audience claim naming
/// the app client it was issued to and the extra `claims` a row's rules are written against.
pub(crate) fn user_pool_claims(subject: &str, claims: &Value) -> Value {
    let mut token_claims = json!({
        "iss": format!("https://{USER_POOL_PROVIDER}"),
        "sub": subject,
        "aud": CLIENT_ID,
        "token_use": "id",
        "exp": 4_102_444_800u64,
    });
    for (name, value) in claims.as_object().into_iter().flatten() {
        token_claims[name] = value.clone();
    }
    token_claims
}

/// A trust policy that admits the Cognito Identity principal for the identity pool
/// `audience`: the `Federated` principal, the web-identity assume, and the `:aud` condition
/// that restricts the assume to users of that one pool.
fn trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PRINCIPAL},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {format!("{COGNITO_PRINCIPAL}:aud"): audience}},
        }],
    })
}

/// A pool fronted by the user pool the rows present — through both of that user pool's app
/// clients, so a mapping keyed by either is one the pool stores — accepting the public
/// provider too and allowing unauthenticated identities, so every flow below is one it
/// serves. Answers its minted `IdentityPoolId`.
pub(crate) async fn pool(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "CognitoIdentityProviders": [
                    {"ProviderName": USER_POOL_PROVIDER, "ClientId": CLIENT_ID},
                    {"ProviderName": USER_POOL_PROVIDER, "ClientId": OTHER_CLIENT_ID},
                ],
                "SupportedLoginProviders": {PUBLIC_PROVIDER: "0123456789"},
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
        .to_string()
}

/// The roles a suite needs, each seeded into the IAM store of the same shared engine with a
/// trust policy that admits `pool_id`'s users, in the order the names are given.
pub(crate) fn seeded_roles(emulator: &Emulator, pool_id: &str, names: &[&str]) -> Vec<Role> {
    names
        .iter()
        .map(|name| Role {
            name: (*name).to_string(),
            arn: emulator.seed_role(name, Some(trust_policy(pool_id))),
        })
        .collect()
}

/// Replace `pool_id`'s whole role configuration: the pool's two roles, and `role_mappings`
/// when the row configures one.
pub(crate) async fn set_roles(
    emulator: &Emulator,
    pool_id: &str,
    authenticated: &Role,
    unauthenticated: &Role,
    role_mappings: Value,
) {
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": pool_id,
                "Roles": {
                    "authenticated": &authenticated.arn,
                    "unauthenticated": &unauthenticated.arn,
                },
                "RoleMappings": role_mappings,
            }),
        )
        .await;
}

/// One `MappingRule`, as `SetIdentityPoolRoles` carries one.
pub(crate) fn rule(claim: &str, match_type: &str, value: &str, role: &Role) -> Value {
    json!({
        "Claim": claim,
        "MatchType": match_type,
        "Value": value,
        "RoleARN": &role.arn,
    })
}

/// A `Rules` role mapping of `rules`, keyed by `key`. `AmbiguousRoleResolution` is the
/// `AuthenticatedRole` value, the resolution whose fallback the rows below assert.
fn rules_mapping(key: &str, rules: Vec<Value>) -> Value {
    json!({
        key: {
            "Type": "Rules",
            "AmbiguousRoleResolution": "AuthenticatedRole",
            "RulesConfiguration": {"Rules": rules},
        },
    })
}

/// Vend for an identity of `pool_id` presenting `logins`, and answer the assumed-role ARN
/// `sts:GetCallerIdentity` reports under the vended credential — which is what the vended
/// session STANDS FOR.
pub(crate) async fn vended_session_arn(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    logins: &Value,
) -> String {
    let federated = logins
        .as_object()
        .is_some_and(|presented| !presented.is_empty());
    let identity_id = emulator
        .identity(pool_id, federated.then(|| logins.clone()))
        .await;
    let mut request = json!({"IdentityId": &identity_id});
    if federated {
        request["Logins"] = logins.clone();
    }
    let issued = emulator.ok("GetCredentialsForIdentity", &request).await;
    let (access_key_id, _) = vended(&issued);
    let (status, body) = emulator.caller_identity(access_key_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCallerIdentity under the vended credential expected 200, got \
         {status} with {body}"
    );
    element(&body, "Arn")
}

/// RULE: the first rule to match, in the order the rules were DECLARED, specifies the role
/// the vend is a session of — so reordering the same two rules moves the vend to the other
/// role, and a rule that does not match is passed over whatever its position.
#[tokio::test]
async fn the_first_matching_rule_in_declared_order_specifies_the_vended_role() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "mapped-role-order").await;
    let roles = seeded_roles(
        &emulator,
        &pool_id,
        &["order-auth", "order-unauth", "order-a", "order-b"],
    );
    let (authenticated, unauthenticated) = (&roles[0], &roles[1]);
    let (role_a, role_b) = (&roles[2], &roles[3]);
    let key = format!("{USER_POOL_PROVIDER}:{CLIENT_ID}");

    // Each row declares the same two always-matching rules in a different ORDER, or puts a
    // rule that cannot match in front of the one that can.
    let rows: Vec<(&str, Vec<Value>, &Role)> = vec![
        (
            "two matching rules, the first naming role A",
            vec![
                rule("aud", "Equals", CLIENT_ID, role_a),
                rule("token_use", "Equals", "id", role_b),
            ],
            role_a,
        ),
        (
            "the same two rules reordered, the first naming role B",
            vec![
                rule("aud", "Equals", CLIENT_ID, role_b),
                rule("token_use", "Equals", "id", role_a),
            ],
            role_b,
        ),
        (
            "a rule that cannot match in front of one naming role B",
            vec![
                rule("token_use", "Equals", "access", role_a),
                rule("token_use", "Equals", "id", role_b),
            ],
            role_b,
        ),
        (
            "one matching rule alone, naming role A",
            vec![rule("aud", "Equals", CLIENT_ID, role_a)],
            role_a,
        ),
        (
            "no rule matches: the pool's own authenticated role",
            vec![rule("custom:dept", "Equals", "engineering", role_b)],
            authenticated,
        ),
    ];

    for (index, (case, rules, expected)) in rows.iter().enumerate() {
        set_roles(
            &emulator,
            &pool_id,
            authenticated,
            unauthenticated,
            rules_mapping(&key, rules.clone()),
        )
        .await;
        // The configuration the vend is decided by is the one a READ reports back, so the
        // vend below is decided by rules this call can see in the order it reports them.
        let reported = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            reported["RoleMappings"][&key]["RulesConfiguration"]["Rules"],
            json!(rules),
            "case {case:?}: GetIdentityPoolRoles must report the rules just set, got {reported}"
        );

        let token = identity_token(&user_pool_claims(&format!("subject-{index}"), &json!({})));
        let logins = json!({USER_POOL_PROVIDER: token});
        let arn = vended_session_arn(&emulator, case, &pool_id, &logins).await;
        assert_eq!(
            arn,
            expected_session_arn(&expected.name),
            "case {case:?}: expected a session of {}, got {arn}",
            expected.name
        );
    }
}

/// RULE: a rule matches its claim exactly as its `MatchType` states, over the values the
/// presented token carries for the claim the rule names — and a claim the token does not
/// carry matches no condition, `NotEqual` included, so the vend falls back to the pool's own
/// role for the flow.
#[tokio::test]
async fn a_rule_matches_its_claim_the_way_its_match_type_states() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "mapped-role-match").await;
    let roles = seeded_roles(
        &emulator,
        &pool_id,
        &["match-auth", "match-unauth", "match-mapped"],
    );
    let (authenticated, unauthenticated, mapped) = (&roles[0], &roles[1], &roles[2]);

    // The rows span the four match types the model defines (`MappingRuleMatchType`:
    // Equals, Contains, StartsWith, NotEqual — the enum the wire refuses any other value
    // for), each on a claim value its condition holds for and one it does not, plus a
    // multi-valued claim, a numeric one and an absent one.
    let rows: Vec<(&str, &str, &str, &str, Value, bool)> = vec![
        (
            "Equals, the claim value is the rule's",
            "custom:dept",
            "Equals",
            "engineering",
            json!({"custom:dept": "engineering"}),
            true,
        ),
        (
            "Equals, the claim value is another",
            "custom:dept",
            "Equals",
            "engineering",
            json!({"custom:dept": "sales"}),
            false,
        ),
        (
            "Contains, the rule's value lies inside the claim's",
            "custom:dept",
            "Contains",
            "gineer",
            json!({"custom:dept": "engineering"}),
            true,
        ),
        (
            "Contains, the rule's value is nowhere in the claim's",
            "custom:dept",
            "Contains",
            "sales",
            json!({"custom:dept": "engineering"}),
            false,
        ),
        (
            "StartsWith, the claim value begins with the rule's",
            "custom:dept",
            "StartsWith",
            "eng",
            json!({"custom:dept": "engineering"}),
            true,
        ),
        (
            "StartsWith, the rule's value is a suffix, not a prefix",
            "custom:dept",
            "StartsWith",
            "ring",
            json!({"custom:dept": "engineering"}),
            false,
        ),
        (
            "NotEqual, the claim value is another",
            "custom:dept",
            "NotEqual",
            "sales",
            json!({"custom:dept": "engineering"}),
            true,
        ),
        (
            "NotEqual, the claim value is the rule's",
            "custom:dept",
            "NotEqual",
            "engineering",
            json!({"custom:dept": "engineering"}),
            false,
        ),
        (
            "a claim the token does not carry",
            "custom:absent",
            "Equals",
            "engineering",
            json!({"custom:dept": "engineering"}),
            false,
        ),
        (
            "NotEqual on a claim the token does not carry",
            "custom:absent",
            "NotEqual",
            "engineering",
            json!({"custom:dept": "engineering"}),
            false,
        ),
        (
            "a multi-valued claim, one of whose values the rule's is",
            "cognito:groups",
            "Equals",
            "admins",
            json!({"cognito:groups": ["members", "admins"]}),
            true,
        ),
        (
            "a multi-valued claim, none of whose values the rule's is",
            "cognito:groups",
            "Equals",
            "owners",
            json!({"cognito:groups": ["members", "admins"]}),
            false,
        ),
        (
            "a claim carried as a number, equal to the rule's value",
            "custom:level",
            "Equals",
            "7",
            json!({"custom:level": 7}),
            true,
        ),
    ];

    for (index, (case, claim, match_type, value, claims, resolves)) in rows.iter().enumerate() {
        set_roles(
            &emulator,
            &pool_id,
            authenticated,
            unauthenticated,
            rules_mapping(
                &format!("{USER_POOL_PROVIDER}:{CLIENT_ID}"),
                vec![rule(claim, match_type, value, mapped)],
            ),
        )
        .await;
        let token = identity_token(&user_pool_claims(&format!("match-subject-{index}"), claims));
        let logins = json!({USER_POOL_PROVIDER: token});
        let arn = vended_session_arn(&emulator, case, &pool_id, &logins).await;
        let expected = if *resolves { mapped } else { authenticated };
        assert_eq!(
            arn,
            expected_session_arn(&expected.name),
            "case {case:?}: expected a session of {}, got {arn}",
            expected.name
        );
    }
}

/// RULE: the mapping that decides a vend is the one keyed by the identity provider the
/// presented login was made under — a provider that issues its tokens to an app client is
/// identified by that client as well as by its name — so a mapping keyed by anything else,
/// a login that carries no token to read a claim from, and an exchange presenting no login
/// at all each fall back to the pool's own role for the flow.
#[tokio::test]
async fn the_mapping_that_decides_is_the_one_keyed_by_the_presented_login_s_provider() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "mapped-role-key").await;
    let roles = seeded_roles(
        &emulator,
        &pool_id,
        &["key-auth", "key-unauth", "key-mapped"],
    );
    let (authenticated, unauthenticated, mapped) = (&roles[0], &roles[1], &roles[2]);
    let user_pool_token = identity_token(&user_pool_claims(
        "key-subject",
        &json!({"custom:dept": "engineering"}),
    ));
    let public_token = identity_token(&json!({
        "iss": format!("https://{PUBLIC_PROVIDER}"),
        "sub": "key-subject-public",
        "aud": "9876543210",
        "custom:dept": "engineering",
        "exp": 4_102_444_800u64,
    }));
    // A token that names no client at all: there the clients the POOL stores for the
    // provider are what a key naming one can be matched against.
    let audienceless_token = identity_token(&json!({
        "iss": format!("https://{USER_POOL_PROVIDER}"),
        "sub": "key-subject-audienceless",
        "token_use": "id",
        "custom:dept": "engineering",
        "exp": 4_102_444_800u64,
    }));

    // The rows differ along the KEY the one mapping is held under (the provider with the
    // client the token names, with another client the pool stores, with one it does not, the
    // provider name alone, another provider), the provider the login is made under, and
    // whether the exchange presents a login at all.
    let rows: Vec<(&str, String, Value, &Role)> = vec![
        (
            "keyed by the provider and the client the token names",
            format!("{USER_POOL_PROVIDER}:{CLIENT_ID}"),
            json!({USER_POOL_PROVIDER: &user_pool_token}),
            mapped,
        ),
        (
            "keyed by the provider name alone",
            USER_POOL_PROVIDER.to_string(),
            json!({USER_POOL_PROVIDER: &user_pool_token}),
            mapped,
        ),
        (
            "keyed by the provider and a client the pool does not store",
            format!("{USER_POOL_PROVIDER}:anotherclient"),
            json!({USER_POOL_PROVIDER: &user_pool_token}),
            authenticated,
        ),
        (
            "keyed by the provider and the pool's OTHER app client",
            format!("{USER_POOL_PROVIDER}:{OTHER_CLIENT_ID}"),
            json!({USER_POOL_PROVIDER: &user_pool_token}),
            authenticated,
        ),
        (
            "a token naming no client, keyed by a client the pool stores",
            format!("{USER_POOL_PROVIDER}:{CLIENT_ID}"),
            json!({USER_POOL_PROVIDER: &audienceless_token}),
            mapped,
        ),
        (
            "keyed by a provider the login was not made under",
            "accounts.google.com".to_string(),
            json!({USER_POOL_PROVIDER: &user_pool_token}),
            authenticated,
        ),
        (
            "a public provider, identified by its name alone",
            PUBLIC_PROVIDER.to_string(),
            json!({PUBLIC_PROVIDER: &public_token}),
            mapped,
        ),
        (
            "a login whose presented value is no token at all",
            PUBLIC_PROVIDER.to_string(),
            json!({PUBLIC_PROVIDER: "not-a-token"}),
            authenticated,
        ),
        (
            "no login at all: the pool's own unauthenticated role",
            format!("{USER_POOL_PROVIDER}:{CLIENT_ID}"),
            json!({}),
            unauthenticated,
        ),
    ];

    for (case, key, logins, expected) in &rows {
        set_roles(
            &emulator,
            &pool_id,
            authenticated,
            unauthenticated,
            rules_mapping(
                key,
                vec![rule("custom:dept", "Equals", "engineering", mapped)],
            ),
        )
        .await;
        let arn = vended_session_arn(&emulator, case, &pool_id, logins).await;
        assert_eq!(
            arn,
            expected_session_arn(&expected.name),
            "case {case:?}: expected a session of {}, got {arn}",
            expected.name
        );
    }
}
