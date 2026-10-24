// SPDX-License-Identifier: CC0-1.0

//! The WORDING of three refusals, over the wire.
//!
//! An error's status and code say what kind of refusal it is; its message is
//! what a caller, a log scrubber or a CLI user reads, so each of these three is
//! built by a rule rather than freely worded. The rules under test:
//!
//! * `UserNotFoundException` carries ONE fixed sentence, the same for every
//!   request and every operation that resolves a username: it reports that the
//!   lookup found no user and names neither the username asked for nor the pool,
//!   so the refusal cannot be read as an answer about one particular identity.
//! * the `ResourceNotFoundException` of a group lookup carries ONE fixed
//!   sentence too, the same for every group name and for every operation that
//!   resolves a `GroupName` against a pool's stored groups.
//! * a password-policy refusal (`InvalidPasswordException`) carries a fixed
//!   prefix that says the password missed the pool's password policy, followed by
//!   the fixed clause for the requirement it missed — a clause per requirement,
//!   never the value the pool configured for that requirement.
//! * a feature-plan refusal (`FeatureUnavailableInTierException`) carries one
//!   template naming the pool's configured feature plan and the feature that plan
//!   does not include, so the feature can be parsed back out of it.
//!
//! Each table row names the case it asserts, and every assertion reports the
//! actual message it saw.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The one sentence a `UserNotFoundException` says.
const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

/// The one sentence a group lookup that found nothing says.
const GROUP_NOT_FOUND_MESSAGE: &str = "Group not found.";

/// The prefix a password-policy refusal carries ahead of the clause for the
/// requirement the password missed.
const PASSWORD_POLICY_PREFIX: &str = "Password did not conform with password policy: ";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`. `extra` carries whatever
/// members the case needs beside `PoolName`.
async fn pool(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    let mut body = json!({ "PoolName": name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

#[tokio::test]
async fn an_absent_user_is_refused_with_one_fixed_message() {
    let router = router();
    let user_pool_id = pool(&router, "absent-user-pool", json!({})).await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "staff" }),
    )
    .await;
    // The app client leaves `PreventUserExistenceErrors` unset, so it keeps the
    // documented `LEGACY` default: the mode whose sign-in refusal for an absent
    // user IS `UserNotFoundException`.
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "absent-user-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {client}"))
        .to_string();

    // A user that DOES exist, so the lookups below fail on the name they are
    // given rather than on an empty pool.
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "present" }),
    )
    .await;

    // The names span what `UsernameType` admits (length 1..128, pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
    // `smithy_query(query="shape",shape="UsernameType")`): the length maximum, a
    // non-ASCII name, an email-shaped name, a name that looks like a generated
    // `sub` (which `Username` also accepts), and the length minimum.
    let longest = "u".repeat(128);
    let usernames = [
        ("a plain name", "sim-absent-user".to_string()),
        ("the 128-character length maximum", longest),
        ("a non-ASCII name", "ユーザー-不在①".to_string()),
        ("an email-shaped name", "absent@example.test".to_string()),
        (
            "a name shaped like a generated sub",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("the one-character length minimum", "z".to_string()),
    ];

    for (name_case, username) in &usernames {
        // Every implemented operation that resolves a `Username` against stored
        // users; each row is (case, operation, request).
        let requests = [
            (
                "AdminGetUser",
                json!({ "UserPoolId": user_pool_id, "Username": username }),
            ),
            (
                "AdminAddUserToGroup",
                json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "GroupName": "staff",
                }),
            ),
            (
                "AdminListGroupsForUser",
                json!({ "UserPoolId": user_pool_id, "Username": username }),
            ),
            (
                "AdminDeleteUser",
                json!({ "UserPoolId": user_pool_id, "Username": username }),
            ),
            (
                "AdminSetUserPassword",
                json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "Password": GOOD_PASSWORD,
                    "Permanent": true,
                }),
            ),
            (
                // There is no invitation to RESEND to a user that does not exist.
                "AdminCreateUser",
                json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "MessageAction": "RESEND",
                }),
            ),
            (
                "AdminInitiateAuth",
                json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            ),
            (
                "InitiateAuth",
                json!({
                    "ClientId": client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            ),
        ];

        for (operation, body) in &requests {
            let case = format!("{operation} with {name_case}");
            let (status, payload) = call(&router, operation, body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (404, "UserNotFoundException"),
                "case {case:?}: expected a 404 UserNotFoundException, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, USER_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {USER_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(username.as_str()),
                "case {case:?}: the refusal must not name the username asked for, got {message:?}"
            );
            assert!(
                !message.contains(user_pool_id.as_str()),
                "case {case:?}: the refusal must not name the pool, got {message:?}"
            );
        }
    }

    // The same lookups still SERVE the user that exists, so the fixed message is
    // the answer to an absent user and not to every lookup.
    let found = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "present" }),
    )
    .await;
    assert_eq!(
        found["Username"],
        json!("present"),
        "a stored user is still read back, got {found}"
    );
}

/// The rule: the `ResourceNotFoundException` of a group lookup carries ONE
/// fixed sentence, whatever group name was asked for and whichever operation
/// resolved it, so the refusal reports that the lookup found nothing without
/// naming the group or the pool it looked in.
#[tokio::test]
async fn an_absent_group_is_refused_with_one_fixed_message() {
    let router = router();
    let user_pool_id = pool(&router, "absent-group-pool", json!({})).await;
    // A group and a user that DO exist, so each lookup below fails on the group
    // name it is given rather than on an empty pool.
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "present" }),
    )
    .await;
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "member" }),
    )
    .await;

    // The names span what `GroupNameType` admits (length 1..128, pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
    // `smithy_query(query="shape",shape="GroupNameType")`): a plain name, both
    // published length bounds, a non-ASCII name, a punctuation-heavy name, and
    // the stored name re-cased, which names a different group.
    let longest = "g".repeat(128);
    let group_names = [
        ("a plain name", "sim-absent-group".to_string()),
        ("the 128-character length maximum", longest),
        ("the one-character length minimum", "z".to_string()),
        ("a non-ASCII name", "グループ①".to_string()),
        ("a punctuation-heavy name", "ops.team-1_a/x".to_string()),
        ("the stored name re-cased", "Present".to_string()),
    ];

    for (name_case, group_name) in &group_names {
        // Every implemented operation that resolves a `GroupName` against a
        // pool's stored groups; each row is (operation, request).
        let requests = [
            (
                "GetGroup",
                json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
            ),
            (
                "UpdateGroup",
                json!({
                    "UserPoolId": user_pool_id,
                    "GroupName": group_name,
                    "Description": "a description for a group that is not there",
                }),
            ),
            (
                "AdminAddUserToGroup",
                json!({
                    "UserPoolId": user_pool_id,
                    "Username": "member",
                    "GroupName": group_name,
                }),
            ),
            (
                "DeleteGroup",
                json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
            ),
        ];

        for (operation, body) in &requests {
            let case = format!("{operation} with {name_case}");
            let (status, payload) = call(&router, operation, body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected a 404 ResourceNotFoundException, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, GROUP_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {GROUP_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(group_name.as_str()),
                "case {case:?}: the refusal must not name the group asked for, got {message:?}"
            );
            assert!(
                !message.contains(user_pool_id.as_str()),
                "case {case:?}: the refusal must not name the pool, got {message:?}"
            );
        }
    }

    // The same lookup still SERVES the group that exists, so the fixed message is
    // the answer to an absent group and not to every lookup.
    let found = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "present" }),
    )
    .await;
    assert_eq!(
        found["Group"]["GroupName"],
        json!("present"),
        "a stored group is still read back, got {found}"
    );
}

/// One password-policy row: the pool policy it creates, the password it offers
/// and the clause the refusal must carry (`None` where the password conforms).
struct PasswordCase {
    case: &'static str,
    /// The `Policies.PasswordPolicy` the pool is created with, or `Value::Null`
    /// for a pool that configures none (the documented default policy).
    policy: Value,
    password: String,
    /// The clause behind [`PASSWORD_POLICY_PREFIX`], or `None` when the password
    /// satisfies the policy.
    requirement: Option<&'static str>,
}

/// The rows. `MinimumLength` is "range 6..99" and every `Require*` member
/// "defaults to false" per
/// `smithy_query(query="shape",shape="PasswordPolicyType")`, so a policy that
/// names only `MinimumLength` tests length alone, and the published bounds (6
/// and 99) are each exercised. The rows differ along which requirement is
/// missed, the configured minimum, and the LENGTH and character content of the
/// password offered.
fn password_cases() -> Vec<PasswordCase> {
    vec![
        PasswordCase {
            case: "a 4-character password against the default policy",
            policy: Value::Null,
            password: "Ab1!".to_string(),
            requirement: Some("Password not long enough"),
        },
        PasswordCase {
            case: "a 5-character password against the minimum-length bound of 6",
            policy: json!({ "MinimumLength": 6 }),
            password: "Ab1!c".to_string(),
            requirement: Some("Password not long enough"),
        },
        PasswordCase {
            case: "a 6-character password against the minimum-length bound of 6",
            policy: json!({ "MinimumLength": 6 }),
            password: "Ab1!cd".to_string(),
            requirement: None,
        },
        PasswordCase {
            case: "a 98-character password against the maximum-length bound of 99",
            policy: json!({ "MinimumLength": 99 }),
            password: "q".repeat(98),
            requirement: Some("Password not long enough"),
        },
        PasswordCase {
            case: "a 99-character password against the maximum-length bound of 99",
            policy: json!({ "MinimumLength": 99 }),
            password: "q".repeat(99),
            requirement: None,
        },
        PasswordCase {
            case: "a password with no uppercase letter where the policy requires one",
            policy: json!({ "MinimumLength": 8, "RequireUppercase": true }),
            password: "passw0rd!2024".to_string(),
            requirement: Some("Password must have uppercase characters"),
        },
        PasswordCase {
            case: "a password with no lowercase letter where the policy requires one",
            policy: json!({ "MinimumLength": 8, "RequireLowercase": true }),
            password: "PASSW0RD!2024".to_string(),
            requirement: Some("Password must have lowercase characters"),
        },
        PasswordCase {
            case: "a password with no digit where the policy requires one",
            policy: json!({ "MinimumLength": 8, "RequireNumbers": true }),
            password: "Password!abc".to_string(),
            requirement: Some("Password must have numeric characters"),
        },
        PasswordCase {
            case: "a password with no symbol where the policy requires one",
            policy: json!({ "MinimumLength": 8, "RequireSymbols": true }),
            password: "Passw0rdabcd".to_string(),
            requirement: Some("Password must have symbol characters"),
        },
        PasswordCase {
            case: "a password satisfying every requirement the policy sets",
            policy: json!({
                "MinimumLength": 12,
                "RequireUppercase": true,
                "RequireLowercase": true,
                "RequireNumbers": true,
                "RequireSymbols": true,
            }),
            password: GOOD_PASSWORD.to_string(),
            requirement: None,
        },
    ]
}

#[tokio::test]
async fn a_password_policy_refusal_names_the_requirement_and_not_the_policy_value() {
    let router = router();

    for (index, row) in password_cases().iter().enumerate() {
        let policies = if row.policy.is_null() {
            json!({})
        } else {
            json!({ "Policies": { "PasswordPolicy": row.policy } })
        };
        let user_pool_id = pool(&router, &format!("policy-pool-{index}"), policies).await;
        // A user created with NO temporary password, so the password under test
        // is the only one this pool's policy ever sees on it.
        ok(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": "target" }),
        )
        .await;

        // Both implemented callers of the policy check: the create that carries a
        // `TemporaryPassword` and the admin password set.
        let requests = [
            (
                "AdminCreateUser",
                json!({
                    "UserPoolId": user_pool_id,
                    "Username": format!("fresh-{index}"),
                    "TemporaryPassword": row.password,
                }),
            ),
            (
                "AdminSetUserPassword",
                json!({
                    "UserPoolId": user_pool_id,
                    "Username": "target",
                    "Password": row.password,
                    "Permanent": true,
                }),
            ),
        ];

        for (operation, body) in &requests {
            let case = format!("{operation}: {}", row.case);
            let (status, payload) = call(&router, operation, body).await;
            let Some(requirement) = row.requirement else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a conforming password is accepted, got {status} with {payload}"
                );
                continue;
            };
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidPasswordException"),
                "case {case:?}: expected a 400 InvalidPasswordException, got {status} with \
                 {payload}"
            );
            let message = error_message(&payload);
            let expected = format!("{PASSWORD_POLICY_PREFIX}{requirement}");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The clause reports the requirement, never the value configured for
            // it: the only digits a policy value could leak here are the
            // configured minimum length.
            if let Some(minimum) = row.policy.get("MinimumLength").and_then(Value::as_i64) {
                assert!(
                    !message.contains(&minimum.to_string()),
                    "case {case:?}: the refusal must not report the configured minimum \
                     {minimum}, got {message:?}"
                );
            }
        }
    }

    // A `MinimumLength` outside the modelled 6..99 range is not a policy at all,
    // so the pool is never created and no password is ever judged against it.
    for (case, minimum) in [("below the range", 5), ("past the range", 100)] {
        let (status, payload) = call(
            &router,
            "CreateUserPool",
            &json!({
                "PoolName": format!("out-of-range-{minimum}"),
                "Policies": { "PasswordPolicy": { "MinimumLength": minimum } },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: a MinimumLength of {minimum} is refused, got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_feature_the_pools_plan_excludes_is_refused_with_the_tier_template() {
    let router = router();

    // `UserPoolTierType` is the enum `LITE | ESSENTIALS | PLUS`
    // (`smithy_query(query="shape",shape="UserPoolTierType")`), plus the pool
    // that names no tier and so carries the documented `ESSENTIALS` default.
    // `ManagedLoginVersion` names only 1 (hosted UI classic) and 2 (managed
    // login), and only managed login "requires that your user pool be configured
    // for any feature plan other than Lite".
    let tiers = [
        ("LITE", Some("LITE"), "LITE"),
        ("ESSENTIALS", Some("ESSENTIALS"), "ESSENTIALS"),
        ("PLUS", Some("PLUS"), "PLUS"),
        ("the default tier", None, "ESSENTIALS"),
    ];

    for (tier_case, requested_tier, effective_tier) in tiers {
        let slug = requested_tier.unwrap_or("default").to_lowercase();

        for version in [1, 2] {
            // A pool holds at most one prefix domain, so each row gets its own
            // pool in the same tier.
            let extra = match requested_tier {
                Some(tier) => json!({ "UserPoolTier": tier }),
                None => json!({}),
            };
            let user_pool_id =
                pool(&router, &format!("tier-pool-{slug}-v{version}"), extra).await;
            let case = format!("ManagedLoginVersion {version} on a pool in {tier_case}");
            let (status, payload) = call(
                &router,
                "CreateUserPoolDomain",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Domain": format!("tier-{slug}-v{version}"),
                    "ManagedLoginVersion": version,
                }),
            )
            .await;

            // Only managed login on the Lite plan is outside the plan.
            if version != 2 || effective_tier != "LITE" {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the plan includes this feature, got {status} with {payload}"
                );
                continue;
            }

            assert_eq!(
                (status, error_code(&payload)),
                (403, "FeatureUnavailableInTierException"),
                "case {case:?}: expected a 403 FeatureUnavailableInTierException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            let expected = format!(
                "The following feature is not available for the {effective_tier} pricing tier \
                 configured: Managed Login Version {version}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The template is what makes the feature machine-readable: a caller
            // reads it back out of the tail of the message.
            let feature = message.rsplit(": ").next().unwrap_or("<no feature>");
            assert_eq!(
                feature,
                format!("Managed Login Version {version}"),
                "case {case:?}: the feature must be parsable out of the message, got {message:?}"
            );
            assert!(
                message.contains(effective_tier),
                "case {case:?}: the refusal must name the configured plan {effective_tier:?}, \
                 got {message:?}"
            );
        }
    }
}
