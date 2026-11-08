// SPDX-License-Identifier: CC0-1.0

//! How this endpoint answers a refusal raised on the AUTHENTICATION and ADMIN-USER
//! surface — an administrative sign-in, self-service password recovery, and the
//! create/read/confirm/settings/delete calls that address one user of a pool — over the
//! wire (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test is the one [`super::error_wire`] states for this endpoint, asserted
//! here over the part of the surface neither its own table nor
//! [`super::self_service_refusal_wire`] drives: a refusal of the CALLER is answered with
//! HTTP 400 and names its error shape in the `x-amzn-errortype` response header, as the
//! shape's simple name followed by a colon, whatever `@httpError` the model annotates that
//! shape with. The model gives `UserNotFoundException` and `ResourceNotFoundException` 404
//! and `NotAuthorizedException` 403
//! (`smithy_query(query="operation",operation="AdminInitiateAuth,AdminConfirmSignUp,AdminSetUserSettings,SetUserSettings,ForgotPassword,AdminCreateUser")`),
//! and this endpoint applies none of them: the error class travels in the body's `__type`
//! and in the header an AWS SDK resolves the exception type from first, while the status
//! says only that the request was refused.
//!
//! That matters for a sign-in or user-administration client, which branches on the status
//! before it reads the body: a 403 from a sign-in is read as a credential or IAM failure,
//! so credential-refresh and retry middleware fires instead of surfacing the
//! authentication refusal, and a 404 is read as "no such route", so unknown-user handling
//! never runs.
//!
//! The rows span what the rule generalises over, so no row is the rule: the error CLASS —
//! and so the modelled status — the refusal is raised with (404, 403 and the 400 the model
//! already annotates), the LAYER the call addresses (an administrative sign-in, a
//! self-service recovery start, an administrative write, an administrative read, a
//! token-addressed self-service write), the KIND of refusal (a pool nothing created, an app
//! client nothing minted, a username the pool holds no user for, a credential the user does
//! not hold, a user whose status forbids the call, a token no sign-in issued, a user that
//! deleted itself, and a member value the operation cannot set), and — for the sign-in and
//! the recovery start — whether the app client REPORTS or HIDES user existence, which
//! decides the class but not the status. The control is the same operations against the
//! pool, the client, the user and the token this service HOLDS: they are served, keep their
//! own status and name no error shape.
//!
//! Every assertion names its case and reports the value it saw.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::error_wire::{
    call, error_code, ok, router, Answered, ERRORTYPE, REFUSED, UNMINTED_CLIENT, UNMINTED_POOL,
};

/// A password satisfying a default pool's password policy, so a created user can be given
/// a permanent one and sign in.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A password no user of this suite holds, satisfying the same policy so nothing refuses it
/// for its shape.
const WRONG_PASSWORD: &str = "NotTheP4ssword!";

/// A sign-in name no user of the pools below answers to.
const GHOST: &str = "nobody";

/// A value submitted as an `AccessToken` that no sign-in of this emulator issued, in the
/// form `TokenModelType`'s pattern demands (`^[A-Za-z0-9-_=.]+$`, from
/// `smithy_query(query="operation",operation="SetUserSettings")`).
const UNISSUED_TOKEN: &str = "not.an.access.token";

/// The one MFA configuration `MFAOptions` can set: `MFAOptions` is a parameter "you can
/// use only to set an SMS configuration that uses SMS for delivery"
/// (`smithy_query(query="operation",operation="SetUserSettings")`).
fn sms_mfa_options() -> Value {
    json!([{ "DeliveryMedium": "SMS", "AttributeName": "phone_number" }])
}

/// An `MFAOptions` naming a delivery medium these operations cannot set — `DeliveryMedium`
/// takes `SMS` or `EMAIL` (`smithy_query(query="shape",shape="DeliveryMediumType")`) and
/// only the first is a configuration `MFAOptions` sets — so the request is refused with the
/// class the model already annotates 400.
fn unsettable_mfa_options() -> Value {
    json!([{ "DeliveryMedium": "EMAIL", "AttributeName": "email" }])
}

/// One pool and the two app clients the rows below need: one that REPORTS whether a user
/// exists (`PreventUserExistenceErrors` `LEGACY`) and one that HIDES it (`ENABLED`), both
/// allowing the administrative and the client-side password sign-in.
///
/// Returns the minted pool id, the reporting client's id and the hiding client's id.
async fn pool_with_clients(
    router: &MultiServiceRouter,
    pool_name: &str,
) -> (String, String, String) {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string();

    let mut client_ids = Vec::new();
    for (client_name, existence) in [("reporting", "LEGACY"), ("hiding", "ENABLED")] {
        let client = ok(
            router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": &user_pool_id,
                "ClientName": client_name,
                "PreventUserExistenceErrors": existence,
                "ExplicitAuthFlows": [
                    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                    "ALLOW_USER_PASSWORD_AUTH",
                    "ALLOW_REFRESH_TOKEN_AUTH",
                ],
            }),
        )
        .await;
        client_ids.push(
            client["UserPoolClient"]["ClientId"]
                .as_str()
                .unwrap_or_else(|| panic!("a minted app client id: {client}"))
                .to_string(),
        );
    }
    let hiding = client_ids.pop().expect("the hiding client");
    let reporting = client_ids.pop().expect("the reporting client");
    (user_pool_id, reporting, hiding)
}

/// Create `username` with a verified email and no permanent password, so it is left owing
/// the temporary password an administrative create hands out — the one status a `RESEND` of
/// the invitation message is possible in.
async fn invited_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [
                { "Name": "email", "Value": format!("{username}@example.com") },
                { "Name": "email_verified", "Value": "true" },
            ],
        }),
    )
    .await;
}

/// Create `username` with a verified email and a permanent password, so it is confirmed and
/// can both sign in and start password recovery — recovery needs "a registered/verified
/// email or phone_number"
/// (`smithy_query(query="operation",operation="ForgotPassword")`).
async fn confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    invited_user(router, user_pool_id, username).await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Sign `username` in through the client-side password flow and return the access token the
/// sign-in issued.
async fn access_token(router: &MultiServiceRouter, client_id: &str, username: &str) -> String {
    let authenticated = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    authenticated["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in issues an access token: {authenticated}"))
        .to_string()
}

/// One row: a call this service refuses, and the error class it refuses it with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: Value,
    class: &'static str,
}

#[tokio::test]
async fn every_authentication_and_admin_user_refusal_is_a_400_naming_its_error_shape() {
    let router = router();
    let (user_pool_id, reporting, hiding) = pool_with_clients(&router, "auth-refusal-wire").await;

    // A confirmed user of that pool, so a row presenting a credential is refused for the
    // credential rather than for a user that was never there.
    confirmed_user(&router, &user_pool_id, "alice").await;
    let alice_token = access_token(&router, &reporting, "alice").await;

    // A signed-in user that then deleted its own profile: its token is still live and still
    // names it, so the next call made with it is refused for the user that is gone.
    confirmed_user(&router, &user_pool_id, "bob").await;
    let bob_token = access_token(&router, &reporting, "bob").await;
    ok(&router, "DeleteUser", &json!({ "AccessToken": &bob_token })).await;

    let rows = vec![
        // The administrative sign-in, over everything its lookups can fail to resolve.
        Refusal {
            case: "AdminInitiateAuth on a pool nothing created",
            operation: "AdminInitiateAuth",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "ClientId": &reporting,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminInitiateAuth on an app client nothing minted",
            operation: "AdminInitiateAuth",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": UNMINTED_CLIENT,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminInitiateAuth for a username the pool holds no user for, \
                   on a client that reports user existence",
            operation: "AdminInitiateAuth",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &reporting,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": GHOST, "PASSWORD": GOOD_PASSWORD },
            }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "AdminInitiateAuth for a username the pool holds no user for, \
                   on a client that hides user existence",
            operation: "AdminInitiateAuth",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &hiding,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": GHOST, "PASSWORD": GOOD_PASSWORD },
            }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "AdminInitiateAuth with a password the user does not hold",
            operation: "AdminInitiateAuth",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &reporting,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": WRONG_PASSWORD },
            }),
            class: "NotAuthorizedException",
        },
        // Self-service password recovery, which resolves an app client and then a user.
        Refusal {
            case: "ForgotPassword on an app client nothing minted",
            operation: "ForgotPassword",
            body: json!({ "ClientId": UNMINTED_CLIENT, "Username": "alice" }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "ForgotPassword for a username the pool holds no user for",
            operation: "ForgotPassword",
            body: json!({ "ClientId": &reporting, "Username": GHOST }),
            class: "UserNotFoundException",
        },
        // The administrative writes and reads that address one user of a pool.
        Refusal {
            case: "AdminCreateUser with MessageAction RESEND for a username the pool \
                   holds no user for",
            operation: "AdminCreateUser",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": GHOST,
                "MessageAction": "RESEND",
            }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "AdminCreateUser with MessageAction RESEND for a user that owes no \
                   temporary password",
            operation: "AdminCreateUser",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "MessageAction": "RESEND",
            }),
            class: "UnsupportedUserStateException",
        },
        Refusal {
            case: "AdminGetUser for a username the pool holds no user for",
            operation: "AdminGetUser",
            body: json!({ "UserPoolId": &user_pool_id, "Username": GHOST }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "AdminConfirmSignUp on a pool nothing created",
            operation: "AdminConfirmSignUp",
            body: json!({ "UserPoolId": UNMINTED_POOL, "Username": "alice" }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminConfirmSignUp for a user whose status is not UNCONFIRMED",
            operation: "AdminConfirmSignUp",
            body: json!({ "UserPoolId": &user_pool_id, "Username": "alice" }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "AdminConfirmSignUp for a username the pool holds no user for",
            operation: "AdminConfirmSignUp",
            body: json!({ "UserPoolId": &user_pool_id, "Username": GHOST }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "AdminSetUserSettings on a pool nothing created",
            operation: "AdminSetUserSettings",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "Username": "alice",
                "MFAOptions": sms_mfa_options(),
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminSetUserSettings for a username the pool holds no user for",
            operation: "AdminSetUserSettings",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": GHOST,
                "MFAOptions": sms_mfa_options(),
            }),
            class: "UserNotFoundException",
        },
        // The token-addressed self-service writes on the same user.
        Refusal {
            case: "SetUserSettings on a token no sign-in issued",
            operation: "SetUserSettings",
            body: json!({ "AccessToken": UNISSUED_TOKEN, "MFAOptions": sms_mfa_options() }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "SetUserSettings on the token of a user that deleted its own profile",
            operation: "SetUserSettings",
            body: json!({ "AccessToken": &bob_token, "MFAOptions": sms_mfa_options() }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "DeleteUser on the token of a user that deleted its own profile",
            operation: "DeleteUser",
            body: json!({ "AccessToken": &bob_token }),
            class: "UserNotFoundException",
        },
        // And the class the model already annotates 400, so only the header is new: a
        // member value these operations cannot set, administratively and self-service.
        Refusal {
            case: "AdminSetUserSettings with a delivery medium MFAOptions cannot set",
            operation: "AdminSetUserSettings",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "MFAOptions": unsettable_mfa_options(),
            }),
            class: "InvalidParameterException",
        },
        Refusal {
            case: "SetUserSettings with a delivery medium MFAOptions cannot set",
            operation: "SetUserSettings",
            body: json!({
                "AccessToken": &alice_token,
                "MFAOptions": unsettable_mfa_options(),
            }),
            class: "InvalidParameterException",
        },
    ];

    for Refusal {
        case,
        operation,
        body,
        class,
    } in rows
    {
        let answered = call(&router, operation, &body).await;
        assert_eq!(
            answered.status, REFUSED,
            "case {case:?}: a refusal of the caller is HTTP {REFUSED}, got {} with {}",
            answered.status, answered.payload
        );
        assert_eq!(
            error_code(&answered.payload),
            class,
            "case {case:?}: expected the body to name {class}, got {}",
            answered.payload
        );
        let expected = format!("{class}:");
        assert_eq!(
            answered.errortype.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {ERRORTYPE} {expected:?}, got {:?}",
            answered.errortype
        );
    }
}

/// The control: the same operations, against the pool, the app client, the user and the
/// token this service HOLDS, are served — they keep their own status and name no error
/// shape — so the 400s above are computed from stored state rather than answered to every
/// call of this surface.
#[tokio::test]
async fn the_same_calls_are_served_for_the_pool_user_and_token_this_service_holds() {
    let router = router();
    let (user_pool_id, reporting, _hiding) = pool_with_clients(&router, "auth-served-wire").await;
    confirmed_user(&router, &user_pool_id, "alice").await;
    invited_user(&router, &user_pool_id, "invited").await;
    let alice_token = access_token(&router, &reporting, "alice").await;

    // (case, operation, body) — one call per surface the table above refuses on.
    let rows = vec![
        (
            "the user signed in administratively with the password it holds",
            "AdminInitiateAuth",
            json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &reporting,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        ),
        (
            "recovery started for the user the pool holds",
            "ForgotPassword",
            json!({ "ClientId": &reporting, "Username": "alice" }),
        ),
        (
            "the invitation resent to the user that still owes its temporary password",
            "AdminCreateUser",
            json!({
                "UserPoolId": &user_pool_id,
                "Username": "invited",
                "MessageAction": "RESEND",
            }),
        ),
        (
            "the user read back administratively",
            "AdminGetUser",
            json!({ "UserPoolId": &user_pool_id, "Username": "alice" }),
        ),
        (
            "the user's MFA options set administratively",
            "AdminSetUserSettings",
            json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "MFAOptions": sms_mfa_options(),
            }),
        ),
        (
            "the user's MFA options set through its own token",
            "SetUserSettings",
            json!({ "AccessToken": &alice_token, "MFAOptions": sms_mfa_options() }),
        ),
        (
            "the user deleting its own profile through its own token",
            "DeleteUser",
            json!({ "AccessToken": &alice_token }),
        ),
    ];

    for (case, operation, body) in rows {
        let Answered {
            status,
            errortype,
            payload,
        } = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {payload}"
        );
        assert_eq!(
            errortype, None,
            "case {case:?}: a served call names no error shape, got {errortype:?}"
        );
    }
}

/// A `RESEND` sends "the invitation message to a user that already exists, and ... reset[s]
/// the temporary-password duration with a new temporary password"
/// (`smithy_query(query="operation",operation="AdminCreateUser")`), so it is possible only
/// for the user that still owes the temporary password an administrative create handed it —
/// the status `FORCE_CHANGE_PASSWORD`, "the user must change their password to complete
/// sign-in" (`smithy_query(query="shape",shape="UserStatusType")`). A user in any other
/// status has no outstanding invitation, so the call is refused with the state class the
/// model declares for it, and nothing about that user changes.
///
/// The rows are the statuses this surface can put a user in, one per way it gets there, so
/// the gate is a statement about the status and not about one recorded account.
#[tokio::test]
async fn a_resend_is_possible_only_while_the_user_still_owes_its_temporary_password() {
    let router = router();
    let (user_pool_id, reporting, _hiding) = pool_with_clients(&router, "resend-state").await;

    // The user an administrative create left owing its temporary password.
    invited_user(&router, &user_pool_id, "invited").await;

    // The same create, then a permanent password: the user owes nothing.
    confirmed_user(&router, &user_pool_id, "confirmed").await;

    // A confirmed user an administrator then sent back to a password reset.
    confirmed_user(&router, &user_pool_id, "resetting").await;
    ok(
        &router,
        "AdminResetUserPassword",
        &json!({ "UserPoolId": &user_pool_id, "Username": "resetting" }),
    )
    .await;

    // A user that signed itself up and has not confirmed: it was never invited.
    ok(
        &router,
        "SignUp",
        &json!({
            "ClientId": &reporting,
            "Username": "signedup",
            "Password": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": "signedup@example.com" }],
        }),
    )
    .await;

    // (case, username, the status the user is in, whether a resend is possible)
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "the user an administrative create invited",
            "invited",
            "FORCE_CHANGE_PASSWORD",
            true,
        ),
        (
            "the user given a permanent password",
            "confirmed",
            "CONFIRMED",
            false,
        ),
        (
            "the user an administrator sent back to a password reset",
            "resetting",
            "RESET_REQUIRED",
            false,
        ),
        (
            "the user that signed itself up and has not confirmed",
            "signedup",
            "UNCONFIRMED",
            false,
        ),
    ];

    for (case, username, status, possible) in rows {
        let before = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": &user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            before["UserStatus"].as_str(),
            Some(*status),
            "case {case:?}: the row's premise is the status, got {before}"
        );

        let answered = call(
            &router,
            "AdminCreateUser",
            &json!({
                "UserPoolId": &user_pool_id,
                "Username": username,
                "MessageAction": "RESEND",
            }),
        )
        .await;

        if *possible {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: a resend to a {status} user is served, got {} with {}",
                answered.status, answered.payload
            );
            continue;
        }

        assert_eq!(
            answered.status, REFUSED,
            "case {case:?}: a resend to a {status} user is refused HTTP {REFUSED}, \
             got {} with {}",
            answered.status, answered.payload
        );
        assert_eq!(
            error_code(&answered.payload),
            "UnsupportedUserStateException",
            "case {case:?}: expected the state class, got {}",
            answered.payload
        );
        let expected =
            format!("Resend not possible. {username} status is not FORCE_CHANGE_PASSWORD");
        assert_eq!(
            answered.payload["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {}",
            answered.payload
        );

        // A refused request writes nothing: the user is where it was.
        let after = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": &user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            after["UserStatus"].as_str(),
            Some(*status),
            "case {case:?}: a refused resend leaves the status alone, got {after}"
        );
        assert_eq!(
            after["UserLastModifiedDate"], before["UserLastModifiedDate"],
            "case {case:?}: a refused resend modifies nothing, got {after}"
        );
    }
}
