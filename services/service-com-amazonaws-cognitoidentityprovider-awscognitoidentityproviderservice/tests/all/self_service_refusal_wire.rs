// SPDX-License-Identifier: CC0-1.0

//! How this endpoint answers a refusal raised by a SELF-SERVICE call — one whose only
//! subject is a signed-in user's `AccessToken` — and by a read that resolves ONE named
//! resource of a pool, over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test is the one [`super::error_wire`] states for this endpoint, asserted
//! here over the half of the surface its own table does not drive: a refusal of the CALLER
//! is answered with HTTP 400 and names its error shape in the `x-amzn-errortype` response
//! header, as the shape's simple name followed by a colon, whatever `@httpError` the model
//! annotates that shape with. The model gives `ResourceNotFoundException` and
//! `UserNotFoundException` 404 and `NotAuthorizedException` 403
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,UserNotFoundException,NotAuthorizedException")`),
//! and this endpoint applies none of them: the error class travels in the body's `__type`
//! and in the header an AWS SDK resolves the exception type from first, while the status
//! says only that the request was refused. That matters for a client that branches on the
//! status: a 404 from an RPC endpoint reads as "no such route" rather than "no such
//! resource", and a 403 from a token-addressed call reads as a credential failure, so
//! credential-refresh logic fires on what is really a bad-token refusal.
//!
//! The rows span what the rule generalises over, so no row is the rule: the error CLASS —
//! and so the modelled status — the refusal is raised with (404, 403 and 400), the KIND of
//! refusal (a resource a delete removed, a resource nothing minted, a token that names no
//! signed-in user, a token whose user is gone, a value that is not valid for the request),
//! the thing the lookup RESOLVES NOTHING for (a user pool, a terms document, a managed
//! login branding, a Region replica, a remembered device, the signed-in user itself), and
//! the LAYER the refusal is raised at (a token-addressed self-service call and an
//! administrative read). The control is the same operations against resources this service
//! HOLDS: they are served, keep their own status and name no error shape.
//!
//! Every assertion names its case and reports the value it saw.
//!
//! The fixture below (a pool that remembers devices, a signed-in user of it, and the
//! device its sign-in minted) is `pub(crate)` so a sibling suite asserting the SAME rule
//! over another part of this surface drives it through this one, rather than through a
//! second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::error_wire::{
    call, error_code, ok, router, Answered, ERRORTYPE, REFUSED, UNMINTED_POOL,
};

/// A password satisfying a default pool's password policy, so a created user can be given
/// a permanent one and sign in.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A value submitted as an `AccessToken` that no sign-in of this emulator issued, in the
/// form `TokenModelType`'s pattern demands (`^[A-Za-z0-9-_=.]+$`, from
/// `smithy_query(query="operation",operation="GetUser")`).
pub(crate) const UNISSUED_TOKEN: &str = "not.an.access.token";

/// A terms-document id nothing created, in the form `TermsIdType`'s pattern demands (a
/// version-4 UUID, from `smithy_query(query="operation",operation="DescribeTerms")`).
const UNMINTED_TERMS: &str = "1dd6a7b2-6f1a-4f3c-8b9d-2c4e6f8a0b1d";

/// A managed-login-branding id nothing created, in the form
/// `ManagedLoginBrandingIdType`'s pattern demands (a version-4 UUID, from
/// `smithy_query(query="operation",operation="DescribeManagedLoginBranding")`).
const UNMINTED_BRANDING: &str = "2ee7b8c3-7a2b-4a4d-9c0e-3d5f7a9b1c2e";

/// A device key no sign-in minted, in the form `DeviceKeyType`'s pattern demands
/// (`^[\w-]+_[0-9a-f-]+$`, from `smithy_query(query="operation",operation="AdminGetDevice")`).
pub(crate) const UNMINTED_DEVICE: &str = "us-east-1_00000000-0000-4000-8000-000000000000";

/// The device credentials a confirm REGISTERS: "a Secure Remote Password (SRP)
/// value that your application generates when you register a user's device"
/// (`smithy_query(query="shape",shape="DeviceSecretVerifierConfigType")`), which
/// `ConfirmDevice` is served only with.
pub(crate) fn device_credentials() -> Value {
    json!({
        "PasswordVerifier": "ZGV2aWNlLXBhc3N3b3JkLXZlcmlmaWVy",
        "Salt": "ZGV2aWNlLXNhbHQ",
    })
}

/// A Region of this emulator's Region catalog that no pool below is replicated to, so it
/// names no replica. `RegionNameType` bounds only the length (5..32,
/// `smithy_query(query="operation",operation="DeleteUserPoolReplica")`), so which values
/// name a Region at all is the service's own catalog.
const UNREPLICATED_REGION: &str = "eu-west-1";

/// The `DeviceConfiguration` that activates device remembering for a pool, so its device
/// operations have a surface to serve: "when you provide a value for any property of
/// `DeviceConfiguration`, you activate the device remembering for the user pool" (this
/// service's model). `DeviceOnlyRememberedOnUserPrompt` is false, so a confirmed device is
/// remembered straight away and the readers below have one to answer for.
fn device_configuration() -> Value {
    json!({ "ChallengeRequiredOnNewDevice": true, "DeviceOnlyRememberedOnUserPrompt": false })
}

/// A pool that remembers devices, with one app client that allows the password sign-in
/// these tests drive: the minted pool id and app client id.
pub(crate) async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
) -> (String, String) {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({ "PoolName": pool_name, "DeviceConfiguration": device_configuration() }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientName": "self-service-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted app client id: {client}"))
        .to_string();
    (user_pool_id, client_id)
}

/// Create `username` with a permanent password, so it is confirmed and can sign in, then
/// sign it in and return the `AuthenticationResult` the sign-in issued.
pub(crate) async fn signed_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Value {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
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
    authenticated["AuthenticationResult"].clone()
}

/// The access token a sign-in issued.
pub(crate) fn access_token(authenticated: &Value) -> String {
    authenticated["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in issues an access token: {authenticated}"))
        .to_string()
}

/// The device key a sign-in against a pool that remembers devices minted for it.
pub(crate) fn device_key(authenticated: &Value) -> String {
    authenticated["NewDeviceMetadata"]["DeviceKey"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in mints a device key: {authenticated}"))
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
async fn every_self_service_and_lookup_refusal_is_a_400_naming_its_error_shape() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "self-service-refusal").await;

    // A signed-in user of that pool, so a row carrying a LIVE token is refused for what it
    // names rather than for the token.
    let alice = signed_in(&router, &user_pool_id, &client_id, "alice").await;
    let alice_token = access_token(&alice);

    // A signed-in user that then deleted its own profile: its token is still live and
    // still names it, so the next call made with it is refused for the user that is gone.
    let bob = signed_in(&router, &user_pool_id, &client_id, "bob").await;
    let bob_token = access_token(&bob);
    ok(&router, "DeleteUser", &json!({ "AccessToken": &bob_token })).await;

    // A pool a DELETE removed, which is the state a teardown path re-runs against.
    let removed = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "self-service-removed" }),
    )
    .await;
    let removed_pool = removed["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {removed}"))
        .to_string();
    ok(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": &removed_pool }),
    )
    .await;

    let rows = vec![
        // A pool whose lookup resolves nothing, read back through the operation a teardown
        // ends on: the model gives this class 404, the wire gives it 400.
        Refusal {
            case: "DescribeUserPool on a pool a delete removed",
            operation: "DescribeUserPool",
            body: json!({ "UserPoolId": &removed_pool }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeUserPool on a pool nothing created",
            operation: "DescribeUserPool",
            body: json!({ "UserPoolId": UNMINTED_POOL }),
            class: "ResourceNotFoundException",
        },
        // One named resource OF a pool this service holds, over the kinds a pool holds.
        Refusal {
            case: "DescribeTerms for a terms id the pool does not hold",
            operation: "DescribeTerms",
            body: json!({ "UserPoolId": &user_pool_id, "TermsId": UNMINTED_TERMS }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeManagedLoginBranding for a branding id the pool does not hold",
            operation: "DescribeManagedLoginBranding",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ManagedLoginBrandingId": UNMINTED_BRANDING,
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DeleteUserPoolReplica for a Region the pool holds no replica in",
            operation: "DeleteUserPoolReplica",
            body: json!({ "UserPoolId": &user_pool_id, "RegionName": UNREPLICATED_REGION }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminGetDevice for a device key the user has not confirmed",
            operation: "AdminGetDevice",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "DeviceKey": UNMINTED_DEVICE,
            }),
            class: "ResourceNotFoundException",
        },
        // A token-addressed call whose token names no signed-in user: modelled at 403.
        Refusal {
            case: "ListWebAuthnCredentials on a token no sign-in issued",
            operation: "ListWebAuthnCredentials",
            body: json!({ "AccessToken": UNISSUED_TOKEN }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "ConfirmDevice on a token no sign-in issued",
            operation: "ConfirmDevice",
            body: json!({ "AccessToken": UNISSUED_TOKEN, "DeviceKey": UNMINTED_DEVICE }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "SetUserMFAPreference on a token no sign-in issued",
            operation: "SetUserMFAPreference",
            body: json!({
                "AccessToken": UNISSUED_TOKEN,
                "SMSMfaSettings": { "Enabled": true, "PreferredMfa": true },
            }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "GetDevice on a token no sign-in issued",
            operation: "GetDevice",
            body: json!({ "AccessToken": UNISSUED_TOKEN, "DeviceKey": UNMINTED_DEVICE }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "DeleteUser on a token no sign-in issued",
            operation: "DeleteUser",
            body: json!({ "AccessToken": UNISSUED_TOKEN }),
            class: "NotAuthorizedException",
        },
        // A live token whose USER is gone: its own class, also modelled at 404.
        Refusal {
            case: "GetUser on the token of a user that deleted its own profile",
            operation: "GetUser",
            body: json!({ "AccessToken": &bob_token }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "DeleteUser on the token of a user that deleted its own profile",
            operation: "DeleteUser",
            body: json!({ "AccessToken": &bob_token }),
            class: "UserNotFoundException",
        },
        // And a refusal the model annotates 400 already, so only the header is new: a live
        // token presenting a device key no sign-in minted.
        Refusal {
            case: "ConfirmDevice on a live token naming a device key no sign-in minted",
            operation: "ConfirmDevice",
            body: json!({
                "AccessToken": &alice_token,
                "DeviceKey": UNMINTED_DEVICE,
                "DeviceSecretVerifierConfig": device_credentials(),
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

/// The control: the same operations, against the resources this service HOLDS and with the
/// token a sign-in really issued, are served — they keep their own status and name no
/// error shape — so the 400s above are computed from state rather than answered to every
/// call of this surface.
#[tokio::test]
async fn the_same_calls_are_served_for_a_token_and_resources_this_service_holds() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "self-service-served").await;
    let alice = signed_in(&router, &user_pool_id, &client_id, "alice").await;
    let alice_token = access_token(&alice);
    let alice_device = device_key(&alice);

    // The device the sign-in minted, confirmed through the self-service writer, so the
    // administrative read has a remembered device to answer for.
    let confirmed = call(
        &router,
        "ConfirmDevice",
        &json!({
            "AccessToken": &alice_token,
            "DeviceKey": &alice_device,
            "DeviceName": "a-laptop",
            "DeviceSecretVerifierConfig": device_credentials(),
        }),
    )
    .await;
    assert_eq!(
        confirmed.status, 200,
        "ConfirmDevice on the device key its own sign-in minted must be served, got {} with {}",
        confirmed.status, confirmed.payload
    );

    // (case, operation, body) — one call per surface the table above refuses on.
    let rows = vec![
        (
            "the pool read back by id",
            "DescribeUserPool",
            json!({ "UserPoolId": &user_pool_id }),
        ),
        (
            "the signed-in user read through its own token",
            "GetUser",
            json!({ "AccessToken": &alice_token }),
        ),
        (
            "the signed-in user's passkeys listed through its own token",
            "ListWebAuthnCredentials",
            json!({ "AccessToken": &alice_token }),
        ),
        (
            "the signed-in user's devices listed through its own token",
            "ListDevices",
            json!({ "AccessToken": &alice_token }),
        ),
        (
            "the remembered device read through its own token",
            "GetDevice",
            json!({ "AccessToken": &alice_token, "DeviceKey": &alice_device }),
        ),
        (
            "the remembered device read administratively",
            "AdminGetDevice",
            json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "DeviceKey": &alice_device,
            }),
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
