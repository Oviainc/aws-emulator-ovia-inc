// SPDX-License-Identifier: CC0-1.0

//! WHICH roles a vend's `CustomRoleArn` may name.
//!
//! RULE: `CustomRoleArn` is "the Amazon Resource Name (ARN) of the role to be assumed when
//! MULTIPLE ROLES WERE RECEIVED IN THE TOKEN from the identity provider"
//! (`GetCredentialsForIdentityInput$CustomRoleArn`, this service's model), so the roles such
//! a request may pick from are the roles the pool's own configuration names TOGETHER WITH
//! the roles the presented token itself carries — the `cognito:roles` and
//! `cognito:preferred_role` claims a Cognito identity-provider token maps groups to roles
//! through (`RoleMapping$Type`) — and the session vended is a session of exactly the role
//! named. A role neither the pool nor the presented token names is a role this exchange
//! could not have resolved, so it vends nothing.
//!
//! The rows differ along what the rule generalises over: WHERE the named role comes from
//! (the pool's `authenticated` role, its `unauthenticated` role, a single-valued group
//! claim, one of several values of a group claim, the preference claim), the COUNT of roles
//! the token carries (one, two, three), and a role no side names.
//!
//! The fixture is `mapped_role_resolution`'s, driven through `vended_role_session`'s
//! harness: this service and STS on ONE shared engine behind the same `MultiServiceRouter`
//! the production router builds, with the IAM roles the assume's decision reads seeded into
//! that same engine.

use serde_json::{json, Value};

use crate::mapped_role_resolution::{
    identity_token, pool, seeded_roles, set_roles, user_pool_claims, Role, USER_POOL_PROVIDER,
};
use crate::vended_role_session::{element, expected_session_arn, vended, Emulator};

/// A `Token` role mapping of the user-pool provider, the type that "will use `cognito:roles`
/// and `cognito:preferred_role` claims from the Cognito identity provider token to map
/// groups to roles" (`RoleMapping$Type`, this service's model). Stating
/// `AuthenticatedRole` so a row whose claims name no ONE role is still decided, which keeps
/// every row below about the `CustomRoleArn` and not about the resolution.
fn token_mapping() -> Value {
    json!({
        USER_POOL_PROVIDER: {"Type": "Token", "AmbiguousRoleResolution": "AuthenticatedRole"},
    })
}

/// What a row expects of its vend.
enum Outcome<'a> {
    /// A session of this role is vended.
    Session(&'a Role),
    /// Nothing is vended: the named role is one this exchange could not have resolved.
    Nothing,
}

/// One row: the case, the group claims the presented token carries, the `CustomRoleArn` the
/// request names, and what the vend must come to.
struct Row<'a> {
    case: &'a str,
    claims: Value,
    custom_role_arn: &'a str,
    outcome: Outcome<'a>,
}

#[tokio::test]
async fn a_custom_role_arn_is_honoured_for_a_role_the_pool_or_the_token_names() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "custom-role-token-claims").await;
    let roles = seeded_roles(
        &emulator,
        &pool_id,
        &[
            "custom-auth",
            "custom-unauth",
            "custom-group-a",
            "custom-group-b",
            "custom-group-c",
            "custom-unnamed",
        ],
    );
    let (authenticated, unauthenticated) = (&roles[0], &roles[1]);
    let (group_a, group_b, group_c) = (&roles[2], &roles[3], &roles[4]);
    let unnamed = &roles[5];
    set_roles(
        &emulator,
        &pool_id,
        authenticated,
        unauthenticated,
        token_mapping(),
    )
    .await;

    let rows = [
        Row {
            case: "the token's single cognito:roles entry",
            claims: json!({"cognito:roles": [&group_a.arn]}),
            custom_role_arn: &group_a.arn,
            outcome: Outcome::Session(group_a),
        },
        Row {
            case: "one of two cognito:roles entries: the documented several-roles case",
            claims: json!({"cognito:roles": [&group_a.arn, &group_b.arn]}),
            custom_role_arn: &group_b.arn,
            outcome: Outcome::Session(group_b),
        },
        Row {
            case: "the last of three cognito:roles entries",
            claims: json!({"cognito:roles": [&group_a.arn, &group_b.arn, &group_c.arn]}),
            custom_role_arn: &group_c.arn,
            outcome: Outcome::Session(group_c),
        },
        Row {
            case: "a role the token names only in cognito:preferred_role",
            claims: json!({"cognito:preferred_role": &group_c.arn}),
            custom_role_arn: &group_c.arn,
            outcome: Outcome::Session(group_c),
        },
        Row {
            case: "the pool's own authenticated role, beside the token's roles",
            claims: json!({"cognito:roles": [&group_a.arn, &group_b.arn]}),
            custom_role_arn: &authenticated.arn,
            outcome: Outcome::Session(authenticated),
        },
        Row {
            case: "the pool's own unauthenticated role, beside the token's roles",
            claims: json!({"cognito:roles": [&group_a.arn, &group_b.arn]}),
            custom_role_arn: &unauthenticated.arn,
            outcome: Outcome::Session(unauthenticated),
        },
        Row {
            case: "a role neither the pool nor the token names",
            claims: json!({"cognito:roles": [&group_a.arn, &group_b.arn]}),
            custom_role_arn: &unnamed.arn,
            outcome: Outcome::Nothing,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let logins = json!({USER_POOL_PROVIDER: identity_token(&user_pool_claims(
            &format!("custom-role-subject-{index}"),
            &row.claims,
        ))});
        let identity_id = emulator.identity(&pool_id, Some(logins.clone())).await;
        let request = json!({
            "IdentityId": &identity_id,
            "Logins": &logins,
            "CustomRoleArn": row.custom_role_arn,
        });
        let (status, body) = emulator.call("GetCredentialsForIdentity", &request).await;

        match &row.outcome {
            Outcome::Session(role) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the vend must be served, got {status} with {body}"
                );
                let (access_key_id, _) = vended(&body);
                let (caller_status, caller_body) = emulator.caller_identity(access_key_id).await;
                assert_eq!(
                    caller_status, 200,
                    "case {case:?}: GetCallerIdentity under the vended credential expected \
                     200, got {caller_status} with {caller_body}"
                );
                let arn = element(&caller_body, "Arn");
                assert_eq!(
                    arn,
                    expected_session_arn(&role.name),
                    "case {case:?}: expected a session of {}, got {arn}",
                    role.name
                );
            }
            Outcome::Nothing => {
                assert_ne!(
                    status, 200,
                    "case {case:?}: a role neither side names vends nothing, got {status} \
                     with {body}"
                );
                assert!(
                    body.get("Credentials").is_none(),
                    "case {case:?}: a refused vend carries no credential, got {body}"
                );
            }
        }
    }
}
