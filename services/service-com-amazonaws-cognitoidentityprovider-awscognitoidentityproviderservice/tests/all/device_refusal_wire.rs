// SPDX-License-Identifier: CC0-1.0

//! How this endpoint answers a refusal raised by a DEVICE call — the self-service
//! `GetDevice` / `UpdateDeviceStatus` / `ForgetDevice`, addressed by a signed-in user's
//! access token, and the administrative `AdminUpdateDeviceStatus` / `AdminForgetDevice`,
//! addressed by a pool id and a username — over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test is the one [`super::error_wire`] states for this endpoint, asserted
//! here over the device surface: a refusal of the CALLER is answered with HTTP 400 and
//! names its error shape in the `x-amzn-errortype` response header, as the shape's simple
//! name followed by a colon, whatever `@httpError` the model annotates that shape with.
//! This model gives `ResourceNotFoundException` and `UserNotFoundException` 404,
//! `NotAuthorizedException` 403, and `InvalidParameterException` and
//! `InvalidUserPoolConfigurationException` 400
//! (`smithy_query(query="operation",operation="UpdateDeviceStatus,ForgetDevice,AdminForgetDevice,AdminUpdateDeviceStatus")`),
//! and this endpoint applies none of the first three: the error class travels in the
//! body's `__type` and in the header an AWS SDK resolves the exception type from first,
//! while the status says only that the request was refused. That matters for a client that
//! branches on the status: a 404 from an RPC endpoint reads as "no such route" rather than
//! "no such device", so the handling for a device that is missing or forgotten never runs,
//! and a 403 from a token-addressed call reads as a credential failure, so
//! credential-refresh logic fires on what is really a bad-token refusal.
//!
//! The rows span what the rule generalises over, so no row is the rule: the error CLASS —
//! and so the modelled status — the refusal is raised with (404, 403 and 400), the
//! OPERATION it is raised by (the three token-addressed device calls and the two
//! administrative ones), the KIND of refusal (a device key nothing minted, a device a
//! sign-in minted that nothing confirmed, a device a forget removed, a value no sign-in
//! issued as an access token, no access token at all, a live token whose user is gone, a
//! user the pool does not hold, a pool nothing created, a pool that activated no device
//! remembering, and a value outside a modelled constraint), and the LAYER the refusal is
//! raised at (a handler's own modelled error, and the framework's refusal of a value no
//! handler sees). The control is the same operations against the device this service
//! HOLDS: they are served, keep their own status and name no error shape.
//!
//! Every assertion names its case and reports the value it saw.

use serde_json::{json, Value};

use super::error_wire::{call, error_code, ok, router, Answered, ERRORTYPE, REFUSED, UNMINTED_POOL};
use super::self_service_refusal_wire::{
    access_token, device_credentials, device_key, pool_with_client, signed_in, UNISSUED_TOKEN,
    UNMINTED_DEVICE,
};

/// A username no pool below holds, in the form `UsernameType`'s pattern allows
/// (`^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
/// `smithy_query(query="operation",operation="AdminForgetDevice")`).
const UNHELD_USERNAME: &str = "nobody";

/// One row: a device call this service refuses, and the error class it refuses it with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: Value,
    class: &'static str,
}

#[tokio::test]
async fn every_device_refusal_is_a_400_naming_its_error_shape_in_the_errortype_header() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "device-refusal-wire").await;

    // A signed-in user whose sign-in minted a device key that nothing confirmed, so the
    // key is well formed and names no device of this user pool yet.
    let alice = signed_in(&router, &user_pool_id, &client_id, "alice").await;
    let alice_token = access_token(&alice);
    let alice_unconfirmed = device_key(&alice);

    // A signed-in user that confirmed its device and then forgot it, so its key names a
    // device this service once held: the state a retry of a teardown path runs against.
    let bob = signed_in(&router, &user_pool_id, &client_id, "bob").await;
    let bob_token = access_token(&bob);
    let bob_device = device_key(&bob);
    ok(
        &router,
        "ConfirmDevice",
        &json!({
            "AccessToken": &bob_token,
            "DeviceKey": &bob_device,
            "DeviceName": "a-forgotten-laptop",
            "DeviceSecretVerifierConfig": device_credentials(),
        }),
    )
    .await;
    ok(
        &router,
        "ForgetDevice",
        &json!({ "AccessToken": &bob_token, "DeviceKey": &bob_device }),
    )
    .await;

    // A signed-in user that then deleted its own profile: its token is still live and
    // still names it, so the next device call made with it is refused for the user gone.
    let carol = signed_in(&router, &user_pool_id, &client_id, "carol").await;
    let carol_token = access_token(&carol);
    let carol_device = device_key(&carol);
    ok(&router, "DeleteUser", &json!({ "AccessToken": &carol_token })).await;

    // A pool that provided no `DeviceConfiguration` activated no device remembering at
    // all, so a device call against it is refused on the pool's configuration — a class
    // the model already annotates 400, where only the header is new.
    let bare = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "device-refusal-wire-bare" }),
    )
    .await;
    let bare_pool = bare["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {bare}"))
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": &bare_pool, "Username": "alice" }),
    )
    .await;

    let rows = vec![
        // A device key nothing minted, over every device operation of this surface: the
        // model gives this class 404, the wire gives it 400.
        Refusal {
            case: "GetDevice on a device key no sign-in minted",
            operation: "GetDevice",
            body: json!({ "AccessToken": &alice_token, "DeviceKey": UNMINTED_DEVICE }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateDeviceStatus on a device key no sign-in minted",
            operation: "UpdateDeviceStatus",
            body: json!({
                "AccessToken": &alice_token,
                "DeviceKey": UNMINTED_DEVICE,
                "DeviceRememberedStatus": "remembered",
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "ForgetDevice on a device key no sign-in minted",
            operation: "ForgetDevice",
            body: json!({ "AccessToken": &alice_token, "DeviceKey": UNMINTED_DEVICE }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminUpdateDeviceStatus on a device key no sign-in minted",
            operation: "AdminUpdateDeviceStatus",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "DeviceKey": UNMINTED_DEVICE,
                "DeviceRememberedStatus": "not_remembered",
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminForgetDevice on a device key no sign-in minted",
            operation: "AdminForgetDevice",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "DeviceKey": UNMINTED_DEVICE,
            }),
            class: "ResourceNotFoundException",
        },
        // A device key a sign-in minted that nothing confirmed, which the self-service
        // calls do not act on.
        Refusal {
            case: "GetDevice on a device key its sign-in minted and nothing confirmed",
            operation: "GetDevice",
            body: json!({ "AccessToken": &alice_token, "DeviceKey": &alice_unconfirmed }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateDeviceStatus on a device key its sign-in minted and nothing confirmed",
            operation: "UpdateDeviceStatus",
            body: json!({
                "AccessToken": &alice_token,
                "DeviceKey": &alice_unconfirmed,
                "DeviceRememberedStatus": "remembered",
            }),
            class: "ResourceNotFoundException",
        },
        // A device a forget removed, read and acted on again.
        Refusal {
            case: "GetDevice on the device key a forget removed",
            operation: "GetDevice",
            body: json!({ "AccessToken": &bob_token, "DeviceKey": &bob_device }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "ForgetDevice on the device key a forget already removed",
            operation: "ForgetDevice",
            body: json!({ "AccessToken": &bob_token, "DeviceKey": &bob_device }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AdminForgetDevice on the device key a forget removed",
            operation: "AdminForgetDevice",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "bob",
                "DeviceKey": &bob_device,
            }),
            class: "ResourceNotFoundException",
        },
        // A pool whose lookup resolves nothing, named by an administrative device call.
        Refusal {
            case: "AdminUpdateDeviceStatus in a pool nothing created",
            operation: "AdminUpdateDeviceStatus",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "Username": "alice",
                "DeviceKey": UNMINTED_DEVICE,
                "DeviceRememberedStatus": "remembered",
            }),
            class: "ResourceNotFoundException",
        },
        // A token-addressed device call whose token names no signed-in user: modelled 403.
        Refusal {
            case: "GetDevice on a value no sign-in issued as an access token",
            operation: "GetDevice",
            body: json!({ "AccessToken": UNISSUED_TOKEN, "DeviceKey": UNMINTED_DEVICE }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "UpdateDeviceStatus on a value no sign-in issued as an access token",
            operation: "UpdateDeviceStatus",
            body: json!({
                "AccessToken": UNISSUED_TOKEN,
                "DeviceKey": UNMINTED_DEVICE,
                "DeviceRememberedStatus": "remembered",
            }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "ForgetDevice on a value no sign-in issued as an access token",
            operation: "ForgetDevice",
            body: json!({ "AccessToken": UNISSUED_TOKEN, "DeviceKey": UNMINTED_DEVICE }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "ForgetDevice with no access token at all, so no signed-in user",
            operation: "ForgetDevice",
            body: json!({ "DeviceKey": UNMINTED_DEVICE }),
            class: "NotAuthorizedException",
        },
        // A live token whose USER is gone, and a username the pool does not hold: their
        // own class, also modelled at 404.
        Refusal {
            case: "GetDevice on the token of a user that deleted its own profile",
            operation: "GetDevice",
            body: json!({ "AccessToken": &carol_token, "DeviceKey": &carol_device }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "UpdateDeviceStatus on the token of a user that deleted its own profile",
            operation: "UpdateDeviceStatus",
            body: json!({
                "AccessToken": &carol_token,
                "DeviceKey": &carol_device,
                "DeviceRememberedStatus": "not_remembered",
            }),
            class: "UserNotFoundException",
        },
        Refusal {
            case: "AdminForgetDevice for a user the pool does not hold",
            operation: "AdminForgetDevice",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": UNHELD_USERNAME,
                "DeviceKey": UNMINTED_DEVICE,
            }),
            class: "UserNotFoundException",
        },
        // A pool that activated no device remembering: modelled at 400 already, so only
        // the header is new.
        Refusal {
            case: "AdminForgetDevice on a pool that activated no device remembering",
            operation: "AdminForgetDevice",
            body: json!({
                "UserPoolId": &bare_pool,
                "Username": "alice",
                "DeviceKey": UNMINTED_DEVICE,
            }),
            class: "InvalidUserPoolConfigurationException",
        },
        Refusal {
            case: "AdminUpdateDeviceStatus on a pool that activated no device remembering",
            operation: "AdminUpdateDeviceStatus",
            body: json!({
                "UserPoolId": &bare_pool,
                "Username": "alice",
                "DeviceKey": UNMINTED_DEVICE,
                "DeviceRememberedStatus": "remembered",
            }),
            class: "InvalidUserPoolConfigurationException",
        },
        // A value outside a modelled constraint, refused before any handler runs: the
        // layer no device handler sees. `DeviceKey` is required and bounded by
        // `^[\w-]+_[0-9a-f-]+$` on every one of these operations
        // (`smithy_query(query="operation",operation="UpdateDeviceStatus,AdminForgetDevice")`).
        Refusal {
            case: "UpdateDeviceStatus with no device key, which the model requires",
            operation: "UpdateDeviceStatus",
            body: json!({ "AccessToken": &alice_token, "DeviceRememberedStatus": "remembered" }),
            class: "InvalidParameterException",
        },
        Refusal {
            case: "AdminForgetDevice with a device key outside the modelled pattern",
            operation: "AdminForgetDevice",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "DeviceKey": "not a device key",
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

/// The control: the same device operations, against the device this service HOLDS and with
/// the token its own sign-in issued, are served — they keep their own status and name no
/// error shape — so the 400s above are computed from state rather than answered to every
/// call of this surface.
#[tokio::test]
async fn the_same_device_calls_are_served_for_the_device_this_service_holds() {
    let router = router();
    let (user_pool_id, client_id) = pool_with_client(&router, "device-refusal-wire-served").await;
    let alice = signed_in(&router, &user_pool_id, &client_id, "alice").await;
    let alice_token = access_token(&alice);
    let alice_device = device_key(&alice);
    ok(
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

    // (case, operation, body) — the remembered device read, then updated through both the
    // self-service and the administrative writer, then forgotten.
    let rows = vec![
        (
            "the remembered device read through its own token",
            "GetDevice",
            json!({ "AccessToken": &alice_token, "DeviceKey": &alice_device }),
        ),
        (
            "the device's remembered status set through its own token",
            "UpdateDeviceStatus",
            json!({
                "AccessToken": &alice_token,
                "DeviceKey": &alice_device,
                "DeviceRememberedStatus": "not_remembered",
            }),
        ),
        (
            "the device's remembered status set administratively",
            "AdminUpdateDeviceStatus",
            json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "DeviceKey": &alice_device,
                "DeviceRememberedStatus": "remembered",
            }),
        ),
        (
            "the device forgotten through its own token",
            "ForgetDevice",
            json!({ "AccessToken": &alice_token, "DeviceKey": &alice_device }),
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
