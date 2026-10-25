// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH members a `GetIdentityPoolRoles` response carries.
//!
//! RULE under test: `Roles` and `RoleMappings` are optional members
//! (`GetIdentityPoolRolesResponse`, from
//! `smithy_query(query="operation",operation="GetIdentityPoolRoles")`: neither is
//! `@required` and neither carries a `@default`), and a role configuration with no entry
//! is the ABSENCE of that configuration rather than a configuration that is empty — so
//! each member is carried only when the pool holds at least one entry for it and is LEFT
//! OUT of the body otherwise.
//!
//! The table below spans the configurations the model admits — none set at all, an empty
//! map, one role, both roles, with and without role mappings, and an empty mapping map —
//! so the case the issue was filed about is one row of it.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::identity_pool_flow::{error_code, member, non_empty, Emulator};
use crate::identity_pool_roles::role_arn;

/// One role mapping, complete for its `Type` as the model documents it.
fn token_mapping() -> Value {
    json!({
        "graph.facebook.com": {"Type": "Token", "AmbiguousRoleResolution": "AuthenticatedRole"},
    })
}

/// The configuration a pool is left in before the read, as
/// `(case, Some((Roles, RoleMappings)))` — `None` means `SetIdentityPoolRoles` is never
/// called for that pool, and a `None` `RoleMappings` means the call omits that member.
///
/// The rows differ in whether anything was stored at all, in the NUMBER of roles stored
/// (none, one of either kind, both — `RolesMap` is modelled `length 0..2`) and in whether
/// and how many role mappings came with them.
#[allow(clippy::type_complexity)]
fn configurations() -> Vec<(&'static str, Option<(Value, Option<Value>)>)> {
    vec![
        ("no SetIdentityPoolRoles call at all", None),
        ("an empty Roles map, no RoleMappings", Some((json!({}), None))),
        (
            "the authenticated role alone",
            Some((json!({"authenticated": role_arn("Cognito_poolAuth_Role")}), None)),
        ),
        (
            "the unauthenticated role alone",
            Some((
                json!({"unauthenticated": role_arn("Cognito_poolUnauth_Role")}),
                None,
            )),
        ),
        (
            "both roles the map admits",
            Some((
                json!({
                    "authenticated": role_arn("Cognito_poolAuth_Role"),
                    "unauthenticated": role_arn("Cognito_poolUnauth_Role"),
                }),
                None,
            )),
        ),
        (
            "both roles and one role mapping",
            Some((
                json!({
                    "authenticated": role_arn("Cognito_poolAuth_Role"),
                    "unauthenticated": role_arn("Cognito_poolUnauth_Role"),
                }),
                Some(token_mapping()),
            )),
        ),
        (
            "an empty Roles map with a role mapping",
            Some((json!({}), Some(token_mapping()))),
        ),
        (
            "one role with an empty RoleMappings map",
            Some((
                json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
                Some(json!({})),
            )),
        ),
    ]
}

/// The `SetIdentityPoolRoles` request for one configuration; `RoleMappings` is sent only
/// when the row carries it.
fn set_request(pool_id: &str, roles: &Value, role_mappings: Option<&Value>) -> Value {
    let mut request = json!({"IdentityPoolId": pool_id, "Roles": roles.clone()});
    if let Some(role_mappings) = role_mappings {
        request["RoleMappings"] = role_mappings.clone();
    }
    request
}

/// RULE: `Roles` and `RoleMappings` are carried only when the pool holds at least one
/// entry for them, and then carry exactly the entries stored — a pool with no role of
/// either kind, whether nobody set roles for it or its last call stored none, reports no
/// `Roles` member at all.
#[tokio::test]
async fn get_identity_pool_roles_carries_only_the_members_the_pool_holds_entries_for() {
    let emulator = Emulator::new();

    for (case, configured) in configurations() {
        // A fresh pool per row, so the row that stores nothing really has nothing stored.
        let pool_id = emulator.pool("roles-presence", true).await;
        if let Some((roles, role_mappings)) = &configured {
            emulator
                .ok(
                    "SetIdentityPoolRoles",
                    &set_request(&pool_id, roles, role_mappings.as_ref()),
                )
                .await;
        }

        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            member(&got, "IdentityPoolId"),
            Some(json!(pool_id)),
            "case {case:?}: the read names the pool it was asked about, got {got}"
        );

        let expected_roles = configured
            .as_ref()
            .map(|(roles, _)| roles.clone())
            .filter(non_empty);
        let actual_roles = member(&got, "Roles");
        assert_eq!(
            actual_roles, expected_roles,
            "case {case:?}: expected Roles {expected_roles:?}, got {actual_roles:?} in {got}"
        );
        let expected_mappings = configured
            .as_ref()
            .and_then(|(_, role_mappings)| role_mappings.clone())
            .filter(non_empty);
        let actual_mappings = member(&got, "RoleMappings");
        assert_eq!(
            actual_mappings, expected_mappings,
            "case {case:?}: expected RoleMappings {expected_mappings:?}, got \
             {actual_mappings:?} in {got}"
        );
    }
}

/// RULE: the read does not change what a later caller sees — reading a pool's roles
/// repeatedly reports the same members, so an unconfigured pool stays unconfigured rather
/// than acquiring an empty configuration by being read.
#[tokio::test]
async fn repeated_reads_report_the_same_members() {
    let emulator = Emulator::new();

    for (case, configured) in configurations() {
        let pool_id = emulator.pool("roles-presence-reread", true).await;
        if let Some((roles, role_mappings)) = &configured {
            emulator
                .ok(
                    "SetIdentityPoolRoles",
                    &set_request(&pool_id, roles, role_mappings.as_ref()),
                )
                .await;
        }

        let first = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        let second = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            second, first,
            "case {case:?}: a second read expected {first}, got {second}"
        );
    }
}

/// RULE: `SetIdentityPoolRoles` replaces the whole configuration, so a replacement that
/// stores no entry leaves the pool with no role configuration — the member a read carried
/// before the replacement is left out of the reads after it.
#[tokio::test]
async fn a_replacement_that_stores_no_entry_leaves_the_member_out() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("roles-presence-replaced", true).await;

    let roles = json!({
        "authenticated": role_arn("Cognito_poolAuth_Role"),
        "unauthenticated": role_arn("Cognito_poolUnauth_Role"),
    });
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &set_request(&pool_id, &roles, Some(&token_mapping())),
        )
        .await;
    let got = emulator
        .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
        .await;
    assert_eq!(
        member(&got, "Roles"),
        Some(roles.clone()),
        "a configured pool expected Roles {roles}, got {got}"
    );
    assert_eq!(
        member(&got, "RoleMappings"),
        Some(token_mapping()),
        "a configured pool expected its stored RoleMappings, got {got}"
    );

    // (case, the replacement's Roles, its RoleMappings)
    for (case, roles, role_mappings) in [
        (
            "a replacement naming one role and no mapping",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            None,
        ),
        (
            "a replacement naming one role with an empty mapping map",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({})),
        ),
        ("a replacement naming no role at all", json!({}), None),
    ] {
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &set_request(&pool_id, &roles, role_mappings.as_ref()),
            )
            .await;
        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        let expected_roles = Some(roles.clone()).filter(non_empty);
        let actual_roles = member(&got, "Roles");
        assert_eq!(
            actual_roles, expected_roles,
            "case {case:?}: expected Roles {expected_roles:?}, got {actual_roles:?} in {got}"
        );
        let expected_mappings = role_mappings.clone().filter(non_empty);
        let actual_mappings = member(&got, "RoleMappings");
        assert_eq!(
            actual_mappings, expected_mappings,
            "case {case:?}: expected RoleMappings {expected_mappings:?}, got \
             {actual_mappings:?} in {got}"
        );
    }
}

/// RULE: a refused `SetIdentityPoolRoles` stores nothing, so a pool it was refused for is
/// still a pool with no configuration — the read carries neither member, exactly as it did
/// before the refused call.
#[tokio::test]
async fn a_refused_set_leaves_the_pool_with_no_configuration() {
    let emulator = Emulator::new();

    // Each row is a configuration `SetIdentityPoolRoles` refuses: a role value that is not
    // an IAM role ARN, a `Roles` key that is not one of the two modelled role kinds, and a
    // role mapping missing what the model documents as required for its `Type`.
    let refusals: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "a role value that is not an IAM role ARN",
            json!({"authenticated": "arn:aws:iam::123456789012:user/alice"}),
            None,
        ),
        (
            "a Roles key that is not a role kind",
            json!({"admin": role_arn("Cognito_poolAuth_Role")}),
            None,
        ),
        (
            "a Token mapping with no AmbiguousRoleResolution",
            json!({"authenticated": role_arn("Cognito_poolAuth_Role")}),
            Some(json!({"accounts.google.com": {"Type": "Token"}})),
        ),
    ];

    for (case, roles, role_mappings) in refusals {
        let pool_id = emulator.pool("roles-presence-refused", true).await;
        let (status, body) = emulator
            .call(
                "SetIdentityPoolRoles",
                &set_request(&pool_id, &roles, role_mappings.as_ref()),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {case:?}: expected a modelled error code, got {body}"
        );

        let got = emulator
            .ok("GetIdentityPoolRoles", &json!({"IdentityPoolId": &pool_id}))
            .await;
        assert_eq!(
            member(&got, "Roles"),
            None,
            "case {case:?}: a refused call stores no Roles, got {got}"
        );
        assert_eq!(
            member(&got, "RoleMappings"),
            None,
            "case {case:?}: a refused call stores no RoleMappings, got {got}"
        );
    }
}
