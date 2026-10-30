// SPDX-License-Identifier: CC0-1.0

//! The gate on an identity pool's basic (classic) authentication flow.
//!
//! RULE: `GetOpenIdToken` is the entry point of that flow, so it is served only by a pool
//! whose stored configuration serves the flow, and refused — with the modelled
//! `InvalidParameterException` (HTTP 400), and writing nothing, neither the implicit login
//! link a served call makes nor a token — by one that does not. Two things close the flow,
//! each worded with its own fixed sentence:
//!
//! * `AllowClassicFlow` "enables or disables the Basic (Classic) authentication flow"
//!   (`smithy_query(query="doc",shape="CreateIdentityPoolInput",member="AllowClassicFlow")`),
//!   so a pool that stored `false` for it forbids the flow outright, while a pool that
//!   enables it, and one that stored no value for the flag, are served;
//! * a ROLE MAPPING chooses the role for an identity from the claims of the token an
//!   exchange presents, which the service does as it vends — the enhanced flow — so a pool
//!   whose roles are mapped does not serve a flow in which the client picks the role
//!   itself.
//!
//! Every case asserts that rule over a TABLE whose rows differ along what closes the flow
//! and HOW the pool came to be in that state: the create call, an update that turns the flag
//! off or on again (`UpdateIdentityPool` replaces a pool's configuration, so the same pool
//! is both states in turn) and a `SetIdentityPoolRoles` call that adds or drops the mapping.
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::developer_identity_tokens::listed_logins;
use crate::identity_pool_flow::{error_code, member, seeded_role_arn, Emulator};

/// The operation the gate is enforced at.
const OPERATION: &str = "GetOpenIdToken";

/// The error code every refusal of the flow carries: a call into a flow the pool does not
/// serve is a bad input parameter, which `GetOpenIdToken` models as
/// `InvalidParameterException` (HTTP 400,
/// `smithy_query(query="operation",operation="GetOpenIdToken")`).
const REFUSED_CODE: &str = "InvalidParameterException";

/// How each refusal is worded (`crate::state::ClassicFlowRefusal::message`), spelled here as
/// literals so the wire-visible sentences are pinned by the test and not by the code under
/// test. Fixed text: no part of either is taken from the request or from stored state.
const DISABLED_MESSAGE: &str = "Basic (classic) flow is not enabled, please use enhanced flow.";
const ROLE_MAPPINGS_MESSAGE: &str =
    "Basic (classic) flow is not supported with RoleMappings, please use enhanced flow.";

/// The public login provider every row presents, which the fixture pools below accept.
const LOGIN_PROVIDER: &str = "graph.facebook.com";

/// Create a pool carrying `allow_classic_flow` (the member is sent only when the row has a
/// value for it, so a row with `None` exercises its ABSENCE) and answer its minted id, with
/// what the create answered for the flag asserted to be what the row asked for.
async fn pool_with_flag(
    emulator: &Emulator,
    case: &str,
    name: &str,
    allow_classic_flow: Option<bool>,
) -> String {
    let mut request = json!({
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": true,
        "SupportedLoginProviders": {LOGIN_PROVIDER: "0123456789"},
    });
    if let Some(allow_classic_flow) = allow_classic_flow {
        request["AllowClassicFlow"] = json!(allow_classic_flow);
    }
    let created = emulator.ok("CreateIdentityPool", &request).await;
    assert_eq!(
        member(&created, "AllowClassicFlow"),
        allow_classic_flow.map(Value::from),
        "case {case:?}: expected the pool to store the flag the create carried, got {created}"
    );
    created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateIdentityPool answers an id: {created}"))
        .to_string()
}

/// Replace `pool_id`'s configuration, carrying `allow_classic_flow` when the row has one:
/// the two required members plus the login provider the rows present, so only the flag
/// differs between the two states of one pool.
async fn update_flag(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    name: &str,
    allow_classic_flow: Option<bool>,
) {
    let mut request = json!({
        "IdentityPoolId": pool_id,
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": true,
        "SupportedLoginProviders": {LOGIN_PROVIDER: "0123456789"},
    });
    if let Some(allow_classic_flow) = allow_classic_flow {
        request["AllowClassicFlow"] = json!(allow_classic_flow);
    }
    let updated = emulator.ok("UpdateIdentityPool", &request).await;
    assert_eq!(
        member(&updated, "AllowClassicFlow"),
        allow_classic_flow.map(Value::from),
        "case {case:?}: expected the update to store the flag it carried, got {updated}"
    );
}

/// The flag `DescribeIdentityPool` reports for `pool_id`.
async fn described_flag(emulator: &Emulator, pool_id: &str) -> Option<Value> {
    let described = emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": pool_id}))
        .await;
    member(&described, "AllowClassicFlow")
}

/// Configure `pool_id`'s roles, with a role MAPPING for the login provider the rows present
/// when `mapped` — the configuration only the enhanced flow can honour — and with the
/// pool's plain `authenticated` role otherwise. `SetIdentityPoolRoles` replaces the whole
/// configuration, so one call puts the pool in either state.
async fn set_roles(emulator: &Emulator, pool_id: &str, name: &str, mapped: bool) {
    let role_arn = seeded_role_arn(name, "auth");
    let mut request = json!({
        "IdentityPoolId": pool_id,
        "Roles": {"authenticated": &role_arn},
    });
    if mapped {
        request["RoleMappings"] = json!({
            LOGIN_PROVIDER: {
                "Type": "Rules",
                "AmbiguousRoleResolution": "Deny",
                "RulesConfiguration": {"Rules": [{
                    "Claim": "email",
                    "MatchType": "Equals",
                    "Value": "someone@example.com",
                    "RoleARN": &role_arn,
                }]},
            },
        });
    }
    emulator.ok("SetIdentityPoolRoles", &request).await;
}

/// `GetOpenIdToken` for `identity_id`, presenting one login of its pool — the shape of call
/// the served flow links a login with, so a refusal that wrote nothing is observable.
async fn get_open_id_token(emulator: &Emulator, identity_id: &str, login: &str) -> (u16, Value) {
    emulator
        .call(
            OPERATION,
            &json!({
                "IdentityId": identity_id,
                "Logins": {LOGIN_PROVIDER: login},
            }),
        )
        .await
}

/// RULE: the flow is served exactly while the pool's stored configuration serves it, and
/// each thing that closes it is refused with its own fixed sentence.
#[tokio::test]
async fn the_basic_flow_is_served_only_while_the_pool_serves_it() {
    let emulator = Emulator::new();

    // One row: how the pool came to be in the state it is in — the flag the create carried,
    // the flag an update replaced it with (`None` for a row that does not update), and
    // whether its roles are configured with a mapping — and the refusal the flow earns in
    // that state (`None` for a state that serves it).
    type Row<'a> = (
        &'a str,
        Option<bool>,
        Option<Option<bool>>,
        Option<bool>,
        Option<&'a str>,
    );
    let rows: Vec<Row<'_>> = vec![
        ("created allowing the flow", Some(true), None, None, None),
        (
            "created forbidding the flow",
            Some(false),
            None,
            None,
            Some(DISABLED_MESSAGE),
        ),
        ("created naming no flag at all", None, None, None, None),
        (
            "updated to forbid the flow",
            Some(true),
            Some(Some(false)),
            None,
            Some(DISABLED_MESSAGE),
        ),
        (
            "updated to allow the flow again",
            Some(false),
            Some(Some(true)),
            None,
            None,
        ),
        (
            "updated to name no flag at all",
            Some(false),
            Some(None),
            None,
            None,
        ),
        (
            "roles configured without a mapping",
            Some(true),
            None,
            Some(false),
            None,
        ),
        (
            "roles configured with a mapping",
            Some(true),
            None,
            Some(true),
            Some(ROLE_MAPPINGS_MESSAGE),
        ),
        (
            "roles configured with a mapping, on a pool naming no flag",
            None,
            None,
            Some(true),
            Some(ROLE_MAPPINGS_MESSAGE),
        ),
        (
            "a mapping dropped by a later call",
            Some(true),
            None,
            Some(false),
            None,
        ),
        (
            // Both reasons at once: the flag is the one AWS words the refusal for.
            "forbidding the flow AND mapping its roles",
            Some(false),
            None,
            Some(true),
            Some(DISABLED_MESSAGE),
        ),
    ];

    for (index, (case, created_with, updated_to, mapped, refusal)) in rows.iter().enumerate() {
        let name = format!("classic-flow-{index}");
        let pool_id = pool_with_flag(&emulator, case, &name, *created_with).await;
        let login = format!("a-facebook-token-{index}");
        // The identity is reached through the very login the call below presents: an
        // identity is reached by a login OF it, so what each row varies is the pool's
        // configuration and nothing else.
        let identity_id = emulator
            .identity(&pool_id, Some(json!({LOGIN_PROVIDER: &login})))
            .await;
        if let Some(updated_to) = updated_to {
            update_flag(&emulator, case, &pool_id, &name, *updated_to).await;
        }
        if let Some(mapped) = mapped {
            // The row that DROPS a mapping is in the mapped state first, so the drop is the
            // call that reopens the flow.
            if !mapped {
                set_roles(&emulator, &pool_id, &name, true).await;
            }
            set_roles(&emulator, &pool_id, &name, *mapped).await;
        }
        let stored = updated_to.unwrap_or(*created_with);

        let (status, body) = get_open_id_token(&emulator, &identity_id, &login).await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the pool serves the flow, so the call must be served, got \
                     {status} with {body}"
                );
                assert!(
                    body["Token"].as_str().is_some_and(|token| !token.is_empty()),
                    "case {case:?}: a served call answers a Token, got {body}"
                );
            }
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the pool does not serve the flow, so no token stands for \
                     it, got {status} with {body}"
                );
                let code = error_code(&body);
                assert!(
                    code.ends_with(REFUSED_CODE),
                    "case {case:?}: expected {REFUSED_CODE}, got {code:?} with {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(*message),
                    "case {case:?}: expected the refusal to be worded {message:?}, got {body}"
                );
            }
        }

        // Whatever the call answered, the pool's own configuration is what it was: a gate
        // is a decision, not a change.
        let described = described_flag(&emulator, &pool_id).await;
        assert_eq!(
            described,
            stored.map(Value::from),
            "case {case:?}: expected the pool to still report the stored flag, got \
             {described:?}"
        );

        // The identity was reached THROUGH that login, so it carries it either way: what a
        // refusal must not do is change that, and what a served call adds is nothing more —
        // the login it presented is the one the identity already had.
        let logins = listed_logins(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            logins,
            vec![LOGIN_PROVIDER.to_string()],
            "case {case:?}: expected the identity's logins to be unchanged, got {logins:?}"
        );
    }
}

/// RULE, the other side of the gate: a pool that forbids the flow still serves the ENHANCED
/// flow, and turning the flag back on makes the very call it refused serve — so the refusal
/// damaged neither the pool nor the identity.
#[tokio::test]
async fn a_pool_that_forbids_the_flow_still_serves_the_enhanced_one() {
    let emulator = Emulator::new();
    let case = "a pool created forbidding the classic flow";
    let name = "classic-flow-enhanced";
    let pool_id = pool_with_flag(&emulator, case, name, Some(false)).await;
    // Reached through the login the classic-flow call below presents, which is what makes
    // that call one the identity's own login authorizes.
    let identity_id = emulator
        .identity(&pool_id, Some(json!({LOGIN_PROVIDER: "a-facebook-token"})))
        .await;

    let (status, refused) = get_open_id_token(&emulator, &identity_id, "a-facebook-token").await;
    assert_eq!(
        status, 400,
        "case {case:?}: the classic flow is refused, got {status} with {refused}"
    );

    // The enhanced flow is the one such a pool serves: this pool configures no role, so the
    // vend is answered the pool-configuration refusal rather than the flow refusal — the
    // call was decided on the pool's ROLES, which is the enhanced flow being served.
    let (status, vend) = emulator
        .call(
            "GetCredentialsForIdentity",
            &json!({
                "IdentityId": &identity_id,
                "Logins": {LOGIN_PROVIDER: "a-facebook-token"},
            }),
        )
        .await;
    let code = error_code(&vend);
    assert!(
        status == 400 && code.ends_with("InvalidIdentityPoolConfigurationException"),
        "case {case:?}: expected the enhanced flow to be decided on the pool's roles, got \
         {status} {code:?} with {vend}"
    );

    // Flipping the flag on serves the very call that was refused, for the same identity.
    update_flag(&emulator, case, &pool_id, name, Some(true)).await;
    let (status, served) = get_open_id_token(&emulator, &identity_id, "a-facebook-token").await;
    assert_eq!(
        status, 200,
        "case {case:?}: with the flow allowed the same call must be served, got {status} \
         with {served}"
    );
    assert_eq!(
        served["IdentityId"].as_str(),
        Some(identity_id.as_str()),
        "case {case:?}: the served call names the identity it was asked for, got {served}"
    );
}
