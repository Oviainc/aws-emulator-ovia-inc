// SPDX-License-Identifier: CC0-1.0

//! What becomes of a vend whose role MAPPING resolves no role.
//!
//! RULE: where an identity pool carries a `RoleMappings` entry keyed by the identity
//! provider a presented login was made under and that mapping resolves no role from the
//! presented token's claims, the mapping's own `AmbiguousRoleResolution` states what
//! becomes of the exchange — "Specifies the action to be taken if either no rules match the
//! claim value for the `Rules` type, or there is no `cognito:preferred_role` claim and there
//! are multiple `cognito:roles` matches for the `Token` type"
//! (`RoleMapping$AmbiguousRoleResolution`, this service's model): the `AuthenticatedRole`
//! value falls the exchange back to the pool's own `authenticated` role, and the `Deny`
//! value refuses the exchange, so `GetCredentialsForIdentity` answers the modelled
//! `NotAuthorizedException` (HTTP 403) and vends nothing. That refusal is an authorization
//! decision about the token presented, not a report that the pool is misconfigured —
//! `InvalidIdentityPoolConfigurationException` is for a pool that "has no authenticated role
//! configured, or STS returned an error response" (that error's own doc), which a pool whose
//! mapping denies the exchange is not. A mapping that DOES resolve a role is unaffected by
//! its resolution, and so is an exchange no mapping of the pool's decides.
//!
//! That refusal NAMES the mapped identity provider that decided against the exchange: the
//! message is `The ambiguous role mapping rules for: <provider> denied this request.`, fixed
//! around one variable part, and that part is the provider the denied login was made under —
//! the `Logins` key — whatever identifier the `RoleMappings` entry is keyed by, which for a
//! provider issuing to an app client also names the client.
//!
//! The rows differ along what the rule generalises over: the resolution the mapping states,
//! the mapping `Type` (`Rules` and `Token`, the two the model defines), what the presented
//! token's claims make of that mapping's rules or group claims (matching, non-matching,
//! absent, single-valued, multi-valued), the key the mapping is held under, the provider the
//! denied login is made under, and the flow the request is in.
//!
//! The fixture is `mapped_role_resolution`'s — the suite that asserts WHICH role a mapping
//! resolves — driven through `vended_role_session`'s harness: this service and STS on ONE
//! shared engine behind the same `MultiServiceRouter` the production router builds, with the
//! IAM roles the assume's decision reads seeded into that same engine.

use serde_json::{json, Value};

use crate::mapped_role_resolution::{
    identity_token, pool, rule, seeded_roles, set_roles, user_pool_claims, vended_session_arn,
    Role, CLIENT_ID, PUBLIC_PROVIDER, USER_POOL_PROVIDER,
};
use crate::vended_role_session::{expected_session_arn, Emulator};

/// The values `AmbiguousRoleResolutionType` admits, from the model
/// (`smithy_query(query="shape",shape="AmbiguousRoleResolutionType")`). Spelled as literals
/// here so the wire-visible values are pinned by the test, not by the code under test.
const AUTHENTICATED_ROLE: &str = "AuthenticatedRole";
const DENY: &str = "Deny";

/// The message the refusal of a vend a role mapping DENIED carries — spelled out here, so
/// the wire-visible wording is pinned by the test rather than by the code under test.
///
/// The fixed text is `The ambiguous role mapping rules for: ` … ` denied this request.` and
/// the one variable part is `provider`: the identity provider the login the mapping decided
/// was made under. That is the provider NAME — the `Logins` key — whatever identifier the
/// `RoleMappings` entry that denied is keyed by, which for a provider issuing to an app
/// client also names the client.
fn denied_message(provider: &str) -> String {
    format!("The ambiguous role mapping rules for: {provider} denied this request.")
}

/// A `Rules` role mapping of `rules`, keyed by `key` and stating `resolution` for the case
/// no rule matches.
fn rules_mapping(key: &str, resolution: &str, rules: Vec<Value>) -> Value {
    json!({
        key: {
            "Type": "Rules",
            "AmbiguousRoleResolution": resolution,
            "RulesConfiguration": {"Rules": rules},
        },
    })
}

/// A `Token` role mapping keyed by `key`, stating `resolution`: the type that "will use
/// `cognito:roles` and `cognito:preferred_role` claims from the Cognito identity provider
/// token to map groups to roles" (`RoleMapping$Type`, this service's model), which carries
/// no `RulesConfiguration` of its own.
fn token_mapping(key: &str, resolution: &str) -> Value {
    json!({key: {"Type": "Token", "AmbiguousRoleResolution": resolution}})
}

/// What a row expects of its vend.
enum Outcome<'a> {
    /// A session of this role is vended.
    Session(&'a Role),
    /// Nothing is vended: the mapping denied the exchange and the call is refused, naming
    /// this identity provider — the one the denied login was made under.
    Refused(&'a str),
}

/// Assert `outcome` of a vend for an identity of `pool_id` presenting `logins`.
async fn assert_outcome(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    logins: &Value,
    outcome: &Outcome<'_>,
) {
    match outcome {
        Outcome::Session(role) => {
            let arn = vended_session_arn(emulator, case, pool_id, logins).await;
            assert_eq!(
                arn,
                expected_session_arn(&role.name),
                "case {case:?}: expected a session of {}, got {arn}",
                role.name
            );
        }
        Outcome::Refused(provider) => {
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
            let (status, body) = emulator.call("GetCredentialsForIdentity", &request).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected the HTTP 403 of a denied vend, got {status} with \
                 {body}"
            );
            let code = body["__type"].as_str().unwrap_or_default();
            assert!(
                code.ends_with("NotAuthorizedException"),
                "case {case:?}: expected NotAuthorizedException, got {code:?} in {body}"
            );
            let expected = denied_message(provider);
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {body}"
            );
            assert!(
                body.get("Credentials").is_none(),
                "case {case:?}: a refused vend carries no credential, got {body}"
            );
        }
    }
}

/// One row of the `Rules` table: the case name, the key the mapping is held under, the
/// `AmbiguousRoleResolution` it states, its rules, the `Logins` the exchange presents, and
/// what the vend must come to.
type RulesRow<'a> = (
    &'a str,
    String,
    &'a str,
    Vec<Value>,
    &'a Value,
    Outcome<'a>,
);

/// One row of the `Token` table: the case name, the key the mapping is held under, the
/// resolution the mapping states, the group claims the presented token carries, and what the
/// vend must come to.
type TokenRow<'a> = (&'a str, String, &'a str, Value, Outcome<'a>);

/// RULE, the `Rules` type: the resolution decides only the exchange no rule of the mapping
/// matches — a matching rule still specifies the role whatever the resolution states, and an
/// exchange the mapping does not decide at all still falls back to the pool's own role.
#[tokio::test]
async fn a_rules_mapping_matching_nothing_is_decided_by_its_ambiguous_role_resolution() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "ambiguous-rules").await;
    let roles = seeded_roles(
        &emulator,
        &pool_id,
        &["ambig-auth", "ambig-unauth", "ambig-mapped"],
    );
    let (authenticated, unauthenticated, mapped) = (&roles[0], &roles[1], &roles[2]);
    let key = USER_POOL_PROVIDER.to_string();

    // One token per claim set the rows read, each presented under the provider the mapping
    // is keyed by.
    let engineering = json!({USER_POOL_PROVIDER: identity_token(&user_pool_claims(
        "ambiguous-engineering",
        &json!({"custom:dept": "engineering"}),
    ))});
    let sales = json!({USER_POOL_PROVIDER: identity_token(&user_pool_claims(
        "ambiguous-sales",
        &json!({"custom:dept": "sales"}),
    ))});
    // A login that is no token at all, and the empty map of an exchange presenting none.
    let untokenized = json!({PUBLIC_PROVIDER: "not-a-token"});
    let no_login = json!({});
    // A token of the OTHER provider the fixture pool accepts, so a refusal under a second
    // provider names that provider rather than the first.
    let public_sales = json!({PUBLIC_PROVIDER: identity_token(&json!({
        "iss": format!("https://{PUBLIC_PROVIDER}"),
        "sub": "ambiguous-public-sales",
        "aud": "9876543210",
        "custom:dept": "sales",
        "exp": 4_102_444_800u64,
    }))});

    let rows: Vec<RulesRow<'_>> = vec![
        (
            "a matching rule under a Deny resolution: the rule's own role",
            key.clone(),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &engineering,
            Outcome::Session(mapped),
        ),
        (
            "no rule matches, under Deny: nothing is vended",
            key.clone(),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &sales,
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "no rule matches, under AuthenticatedRole: the pool's authenticated role",
            key.clone(),
            AUTHENTICATED_ROLE,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &sales,
            Outcome::Session(authenticated),
        ),
        (
            "the rule's claim is absent from the token, under Deny",
            key.clone(),
            DENY,
            vec![rule("custom:absent", "Equals", "engineering", mapped)],
            &engineering,
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "the rule's claim is absent from the token, under AuthenticatedRole",
            key.clone(),
            AUTHENTICATED_ROLE,
            vec![rule("custom:absent", "Equals", "engineering", mapped)],
            &engineering,
            Outcome::Session(authenticated),
        ),
        (
            "several rules, none of them matching, under Deny",
            key.clone(),
            DENY,
            vec![
                rule("custom:dept", "Equals", "sales", mapped),
                rule("custom:dept", "StartsWith", "mark", mapped),
                rule("cognito:groups", "Contains", "admins", mapped),
            ],
            &engineering,
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "no rule matches, under Deny, through a mapping keyed by the provider AND the \
             client the token names",
            format!("{USER_POOL_PROVIDER}:{CLIENT_ID}"),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &sales,
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "no rule matches, under Deny, for a login of the pool's OTHER provider",
            PUBLIC_PROVIDER.to_string(),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &public_sales,
            Outcome::Refused(PUBLIC_PROVIDER),
        ),
        (
            "several rules, the last of them matching, under Deny",
            key.clone(),
            DENY,
            vec![
                rule("custom:dept", "Equals", "sales", mapped),
                rule("custom:dept", "StartsWith", "eng", mapped),
            ],
            &engineering,
            Outcome::Session(mapped),
        ),
        (
            "a mapping keyed by a provider the login was not made under, under Deny",
            "accounts.google.com".to_string(),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &engineering,
            Outcome::Session(authenticated),
        ),
        (
            "a login whose presented value is no token at all, under Deny",
            PUBLIC_PROVIDER.to_string(),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &untokenized,
            Outcome::Session(authenticated),
        ),
        (
            "no login at all, under Deny: the pool's unauthenticated role",
            key.clone(),
            DENY,
            vec![rule("custom:dept", "Equals", "engineering", mapped)],
            &no_login,
            Outcome::Session(unauthenticated),
        ),
    ];

    for (case, key, resolution, rules, logins, outcome) in &rows {
        set_roles(
            &emulator,
            &pool_id,
            authenticated,
            unauthenticated,
            rules_mapping(key, resolution, rules.clone()),
        )
        .await;
        assert_outcome(&emulator, case, &pool_id, logins, outcome).await;
    }
}

/// RULE, the `Token` type: the role is the one the presented token's own group claims name —
/// the preferred role where it names one, otherwise the single role the groups grant — and
/// the resolution decides the exchange those claims name no one role for.
#[tokio::test]
async fn a_token_mapping_falls_to_its_resolution_when_the_group_claims_name_no_one_role() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "ambiguous-token").await;
    let roles = seeded_roles(
        &emulator,
        &pool_id,
        &["token-auth", "token-unauth", "token-group", "token-other"],
    );
    let (authenticated, unauthenticated) = (&roles[0], &roles[1]);
    let (group, other) = (&roles[2], &roles[3]);

    // The rows differ along what the token's group claims say: one role, several with a
    // preference among them, several without one, and none at all — and along the key the
    // mapping is held under, which the refusal does NOT name.
    let rows: Vec<TokenRow<'_>> = vec![
        (
            "a single cognito:roles entry under Deny: that role",
            USER_POOL_PROVIDER.to_string(),
            DENY,
            json!({"cognito:roles": [&group.arn]}),
            Outcome::Session(group),
        ),
        (
            "a cognito:preferred_role claim under Deny: the preferred role",
            USER_POOL_PROVIDER.to_string(),
            DENY,
            json!({"cognito:preferred_role": &other.arn}),
            Outcome::Session(other),
        ),
        (
            "several roles with a preference among them, under Deny",
            USER_POOL_PROVIDER.to_string(),
            DENY,
            json!({
                "cognito:roles": [&group.arn, &other.arn],
                "cognito:preferred_role": &other.arn,
            }),
            Outcome::Session(other),
        ),
        (
            "several roles and no preference, under Deny: nothing is vended",
            USER_POOL_PROVIDER.to_string(),
            DENY,
            json!({"cognito:roles": [&group.arn, &other.arn]}),
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "several roles and no preference, under Deny, keyed by the provider AND the \
             client the token names",
            format!("{USER_POOL_PROVIDER}:{CLIENT_ID}"),
            DENY,
            json!({"cognito:roles": [&group.arn, &other.arn]}),
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "several roles and no preference, under AuthenticatedRole",
            USER_POOL_PROVIDER.to_string(),
            AUTHENTICATED_ROLE,
            json!({"cognito:roles": [&group.arn, &other.arn]}),
            Outcome::Session(authenticated),
        ),
        (
            "no group claim at all, under Deny: nothing is vended",
            USER_POOL_PROVIDER.to_string(),
            DENY,
            json!({}),
            Outcome::Refused(USER_POOL_PROVIDER),
        ),
        (
            "no group claim at all, under AuthenticatedRole",
            USER_POOL_PROVIDER.to_string(),
            AUTHENTICATED_ROLE,
            json!({}),
            Outcome::Session(authenticated),
        ),
    ];

    for (index, (case, key, resolution, claims, outcome)) in rows.iter().enumerate() {
        set_roles(
            &emulator,
            &pool_id,
            authenticated,
            unauthenticated,
            token_mapping(key, resolution),
        )
        .await;
        let logins = json!({USER_POOL_PROVIDER: identity_token(&user_pool_claims(
            &format!("token-subject-{index}"),
            claims,
        ))});
        assert_outcome(&emulator, case, &pool_id, &logins, outcome).await;
    }
}
