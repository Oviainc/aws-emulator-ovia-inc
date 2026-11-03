// SPDX-License-Identifier: CC0-1.0

//! Device remembering over the wire: what a sign-in answers for the device it came
//! from, and what the device the caller then confirms reads back as.
//!
//! Three rules are under test.
//!
//! * MINTING. A completed sign-in against a pool that carries a `DeviceConfiguration`
//!   answers `AuthenticationResult.NewDeviceMetadata` for a device the pool does not
//!   recognise, and nothing for a device it does. The pool's device remembering is
//!   activated by the PRESENCE of `DeviceConfiguration` — "When you provide a value for
//!   any property of `DeviceConfiguration`, you activate the device remembering for the
//!   user pool" — so the tables below span its property combinations, the empty
//!   structure included, and a pool that configures none at all.
//! * CONFIRMING. `ConfirmDevice` confirms the device that key names and remembers it
//!   unless `DeviceOnlyRememberedOnUserPrompt` asks for the user's own answer first,
//!   which is what `UserConfirmationNecessary` reports.
//! * READING BACK. `GetDevice` and `AdminGetDevice` render the STORED device, and refuse
//!   a key no sign-in minted, a device nothing confirmed, and a pool with no device
//!   remembering at all.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for, which is the region the pools
/// they create live in.
const DEFAULT_REGION: &str = "us-east-1";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The `DeviceAttributes` entry a confirmed device's `DeviceName` is reported under.
const DEVICE_NAME_ATTRIBUTE: &str = "device_name";

/// The modelled maximum length of a `DeviceKey` (`DeviceKeyType`, length 1..55).
const MAX_DEVICE_KEY_LENGTH: usize = 55;

/// The device credentials a confirm REGISTERS: "a Secure Remote Password (SRP)
/// value that your application generates when you register a user's device"
/// (`smithy_query(query="shape",shape="DeviceSecretVerifierConfigType")`), which
/// `ConfirmDevice` is served only with.
fn device_credentials() -> Value {
    json!({
        "PasswordVerifier": "ZGV2aWNlLXBhc3N3b3JkLXZlcmlmaWVy",
        "Salt": "ZGV2aWNlLXNhbHQ",
    })
}

/// One emulator with just this service mounted over `engine`.
fn router_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router
}

/// One emulator with just this service mounted over a fresh engine.
fn router() -> MultiServiceRouter {
    router_over(&Arc::new(engine::InMemoryEngine::new()))
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    dispatch_in_region(router, DEFAULT_REGION, operation, body).await
}

/// Dispatch `operation` signed for `region` — the credential scope is what a pool
/// created by the call is placed in — and return the status and JSON response body.
async fn dispatch_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
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
    let (status, payload) = dispatch(router, operation, body).await;
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

/// Call `operation` expecting a refusal, and assert its status and wire error code.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (expected_status, expected_code),
        "case {case:?}: expected ({expected_status}, {expected_code:?}), \
         got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
}

/// A created pool and one of its app clients.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool with `device_configuration` (when given) and one app client that
/// allows the three password flows these tests drive, and return both ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    device_configuration: Option<Value>,
) -> Fixture {
    pool_with_client_in_region(router, DEFAULT_REGION, pool_name, device_configuration).await
}

/// [`pool_with_client`], with the pool created in `region` — the region the
/// creating call is signed for, which is the region the pool and its device keys
/// name.
async fn pool_with_client_in_region(
    router: &MultiServiceRouter,
    region: &str,
    pool_name: &str,
    device_configuration: Option<Value>,
) -> Fixture {
    let mut pool_body = Map::new();
    pool_body.insert("PoolName".to_string(), json!(pool_name));
    if let Some(configuration) = device_configuration {
        pool_body.insert("DeviceConfiguration".to_string(), configuration);
    }
    let (status, created) =
        dispatch_in_region(router, region, "CreateUserPool", &Value::Object(pool_body)).await;
    assert_eq!(
        status, 200,
        "CreateUserPool in {region} must succeed, got {status} with {created}"
    );
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "device-client",
            "ExplicitAuthFlows": [
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Create `username` in `user_pool_id` with a PERMANENT password, so it is `CONFIRMED`
/// and can sign in.
async fn create_confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
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
}

/// Which sign-in operation a case drives, with the `AuthFlow` it names.
#[derive(Clone, Copy, Debug)]
struct SignIn {
    operation: &'static str,
    auth_flow: &'static str,
}

/// The self-service sign-in and the two admin ones, which must agree about the device
/// they answer for.
const SIGN_INS: [SignIn; 3] = [
    SignIn {
        operation: "InitiateAuth",
        auth_flow: "USER_PASSWORD_AUTH",
    },
    SignIn {
        operation: "AdminInitiateAuth",
        auth_flow: "ADMIN_USER_PASSWORD_AUTH",
    },
    SignIn {
        operation: "AdminInitiateAuth",
        auth_flow: "ADMIN_NO_SRP_AUTH",
    },
];

/// Sign `username` in through `sign_in`, presenting `device_key` when one is given, and
/// return the `AuthenticationResult`.
async fn sign_in(
    router: &MultiServiceRouter,
    fixture: &Fixture,
    sign_in: SignIn,
    username: &str,
    device_key: Option<&str>,
) -> Value {
    let mut parameters = Map::new();
    parameters.insert("USERNAME".to_string(), json!(username));
    parameters.insert("PASSWORD".to_string(), json!(GOOD_PASSWORD));
    if let Some(device_key) = device_key {
        parameters.insert("DEVICE_KEY".to_string(), json!(device_key));
    }
    let mut body = Map::new();
    body.insert("ClientId".to_string(), json!(fixture.client_id));
    body.insert("AuthFlow".to_string(), json!(sign_in.auth_flow));
    body.insert("AuthParameters".to_string(), Value::Object(parameters));
    if sign_in.operation == "AdminInitiateAuth" {
        body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
    }
    let payload = ok(router, sign_in.operation, &Value::Object(body)).await;
    payload["AuthenticationResult"].clone()
}

/// Whether `device_key` satisfies the modelled `DeviceKeyType`: pattern
/// `^[\w-]+_[0-9a-f-]+$` (the first group is greedy, so the LAST underscore splits it)
/// and length 1..55.
fn is_modelled_device_key(device_key: &str) -> bool {
    let Some((group, identifier)) = device_key.rsplit_once('_') else {
        return false;
    };
    !device_key.is_empty()
        && device_key.len() <= MAX_DEVICE_KEY_LENGTH
        && !group.is_empty()
        && group
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && !identifier.is_empty()
        && identifier
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase() || c == '-')
}

/// The `DeviceKey` of an `AuthenticationResult`'s `NewDeviceMetadata`, when it carries
/// one, asserting the member's shape wherever it is present.
fn new_device_key(case: &str, result: &Value) -> Option<String> {
    let metadata = result.get("NewDeviceMetadata")?;
    let device_key = metadata["DeviceKey"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: NewDeviceMetadata must carry a DeviceKey, got {metadata}"))
        .to_string();
    assert!(
        is_modelled_device_key(&device_key),
        "case {case:?}: the DeviceKey must satisfy DeviceKeyType, got {device_key:?}"
    );
    assert!(
        metadata["DeviceGroupKey"]
            .as_str()
            .is_some_and(|key| !key.is_empty()),
        "case {case:?}: NewDeviceMetadata must carry a DeviceGroupKey, got {metadata}"
    );
    Some(device_key)
}

/// One row of the device-configuration table: the `DeviceConfiguration` a pool was
/// created with, and whether a first sign-in on that pool owes `NewDeviceMetadata`.
struct ConfigurationRow {
    case: &'static str,
    device_configuration: Option<Value>,
    mints: bool,
}

/// Device remembering is activated by the PRESENCE of `DeviceConfiguration`, whatever
/// its properties say, and only then does a sign-in from an unrecognized device answer
/// `NewDeviceMetadata` — on the self-service flow and on both admin flows alike.
#[tokio::test]
async fn new_device_metadata_follows_the_pools_device_configuration() {
    let rows = [
        ConfigurationRow {
            case: "no DeviceConfiguration at all",
            device_configuration: None,
            mints: false,
        },
        ConfigurationRow {
            case: "DeviceConfiguration with no property set",
            device_configuration: Some(json!({})),
            mints: true,
        },
        ConfigurationRow {
            case: "ChallengeRequiredOnNewDevice alone",
            device_configuration: Some(json!({ "ChallengeRequiredOnNewDevice": true })),
            mints: true,
        },
        ConfigurationRow {
            case: "DeviceOnlyRememberedOnUserPrompt alone",
            device_configuration: Some(json!({ "DeviceOnlyRememberedOnUserPrompt": true })),
            mints: true,
        },
        ConfigurationRow {
            case: "both properties false",
            device_configuration: Some(json!({
                "ChallengeRequiredOnNewDevice": false,
                "DeviceOnlyRememberedOnUserPrompt": false,
            })),
            mints: true,
        },
        ConfigurationRow {
            case: "both properties true",
            device_configuration: Some(json!({
                "ChallengeRequiredOnNewDevice": true,
                "DeviceOnlyRememberedOnUserPrompt": true,
            })),
            mints: true,
        },
    ];

    for row in rows {
        for flow in SIGN_INS {
            let case = format!("{} via {}", row.case, flow.auth_flow);
            let router = router();
            let fixture =
                pool_with_client(&router, "device-pool", row.device_configuration.clone()).await;
            create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;

            let result = sign_in(&router, &fixture, flow, "alice", None).await;
            assert!(
                result["AccessToken"].as_str().is_some_and(|t| !t.is_empty()),
                "case {case:?}: the sign-in must complete, got {result}"
            );
            let minted = new_device_key(&case, &result);
            assert_eq!(
                minted.is_some(),
                row.mints,
                "case {case:?}: expected NewDeviceMetadata present={}, got {result}",
                row.mints
            );
        }
    }
}

/// One row of the presented-device table: which `DEVICE_KEY` the second sign-in carries,
/// and whether that sign-in owes a new device.
struct PresentedRow {
    case: &'static str,
    /// Built from the keys the setup minted: the device of this user, the device of
    /// another user, or a key nothing minted.
    presented: fn(&str, &str) -> Option<String>,
    mints: bool,
}

/// A sign-in answers `NewDeviceMetadata` for a device the user pool does not recognise
/// and nothing for one it does: the key it already holds for THAT user is recognized,
/// and every other key — another user's, or one no sign-in minted — is not.
#[tokio::test]
async fn a_recognized_device_key_is_not_minted_again() {
    let rows = [
        PresentedRow {
            case: "no DEVICE_KEY presented",
            presented: |_, _| None,
            mints: true,
        },
        PresentedRow {
            case: "the DEVICE_KEY this user's first sign-in minted",
            presented: |own, _| Some(own.to_string()),
            mints: false,
        },
        PresentedRow {
            case: "a DEVICE_KEY minted for another user of the same pool",
            presented: |_, other| Some(other.to_string()),
            mints: true,
        },
        PresentedRow {
            case: "a well-formed DEVICE_KEY no sign-in minted",
            presented: |_, _| Some("us-east-1_00000000-0000-0000-0000-000000000000".to_string()),
            mints: true,
        },
    ];

    for row in rows {
        let router = router();
        let fixture = pool_with_client(&router, "recognize-pool", Some(json!({}))).await;
        create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
        create_confirmed_user(&router, &fixture.user_pool_id, "bob").await;

        let first = sign_in(&router, &fixture, SIGN_INS[0], "alice", None).await;
        let own = new_device_key(row.case, &first).expect("a first sign-in mints a device");
        let others = sign_in(&router, &fixture, SIGN_INS[0], "bob", None).await;
        let other = new_device_key(row.case, &others).expect("bob's own device");
        assert_ne!(
            own, other,
            "case {:?}: two devices must carry different keys, got {own:?} twice",
            row.case
        );

        let presented = (row.presented)(&own, &other);
        let second = sign_in(&router, &fixture, SIGN_INS[0], "alice", presented.as_deref()).await;
        let minted = new_device_key(row.case, &second);
        assert_eq!(
            minted.is_some(),
            row.mints,
            "case {:?}: expected NewDeviceMetadata present={}, got {second}",
            row.case,
            row.mints
        );
        if let Some(minted) = minted {
            assert_ne!(
                minted, own,
                "case {:?}: an unrecognized device gets its OWN key, got the stored one",
                row.case
            );
        }
    }
}

/// A minted `DeviceKey` satisfies `DeviceKeyType` (length 1..55, pattern
/// `^[\w-]+_[0-9a-f-]+$`) whatever region the pool lives in: the region comes from the
/// caller's credential scope, so the key's prefix is clamped rather than assumed short.
#[tokio::test]
async fn a_minted_device_key_fits_the_model_in_every_region() {
    // A short region, two of the longest names AWS publishes, and one longer than any
    // of them, which the prefix clamp is what keeps inside the modelled length.
    let regions = [
        "us-east-1",
        "ap-southeast-7",
        "us-gov-east-1",
        "zz-extra-long-region-1",
    ];

    for region in regions {
        let case = format!("pool created in {region:?}");
        let router = router();
        let fixture =
            pool_with_client_in_region(&router, region, "region-pool", Some(json!({}))).await;
        create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;

        let result = sign_in(&router, &fixture, SIGN_INS[0], "alice", None).await;
        // `new_device_key` asserts the member's modelled shape for every case.
        let device_key = new_device_key(&case, &result).expect("a minted device");
        // The device the key names round-trips through the reader, so the key a pool
        // in any region mints is one its own device surface accepts.
        let access_token = result["AccessToken"].as_str().expect("an access token");
        ok(
            &router,
            "ConfirmDevice",
            &json!({
                "AccessToken": access_token,
                "DeviceKey": device_key,
                "DeviceSecretVerifierConfig": device_credentials(),
            }),
        )
        .await;
        let read = ok(
            &router,
            "GetDevice",
            &json!({ "AccessToken": access_token, "DeviceKey": device_key }),
        )
        .await;
        assert_eq!(
            read["Device"]["DeviceKey"].as_str(),
            Some(device_key.as_str()),
            "case {case:?}: the reader must report the device that key names, got {read}"
        );
    }
}

/// Every device of one pool reports that pool's `DeviceGroupKey`, and two pools report
/// different ones: the group key belongs to the pool, not to the sign-in.
#[tokio::test]
async fn the_device_group_key_belongs_to_the_pool() {
    let router = router();
    let first = pool_with_client(&router, "group-key-pool-1", Some(json!({}))).await;
    let second = pool_with_client(&router, "group-key-pool-2", Some(json!({}))).await;
    create_confirmed_user(&router, &first.user_pool_id, "alice").await;
    create_confirmed_user(&router, &first.user_pool_id, "bob").await;
    create_confirmed_user(&router, &second.user_pool_id, "alice").await;

    let group_key = |result: &Value| {
        result["NewDeviceMetadata"]["DeviceGroupKey"]
            .as_str()
            .expect("a device group key")
            .to_string()
    };
    let alice = group_key(&sign_in(&router, &first, SIGN_INS[0], "alice", None).await);
    let bob = group_key(&sign_in(&router, &first, SIGN_INS[0], "bob", None).await);
    let elsewhere = group_key(&sign_in(&router, &second, SIGN_INS[0], "alice", None).await);

    assert_eq!(
        alice, bob,
        "case \"two users of one pool\": the group key is the pool's, got {alice:?} and {bob:?}"
    );
    assert_ne!(
        alice, elsewhere,
        "case \"two pools\": each pool has its own group key, got {alice:?} twice"
    );
}

/// One row of the confirmation table: what the pool configured, and what
/// `ConfirmDevice` owes for it.
struct ConfirmRow {
    case: &'static str,
    device_configuration: Value,
    /// `UserConfirmationNecessary`: true exactly when the pool asks for the user's own
    /// answer before remembering the device.
    user_confirmation_necessary: bool,
    /// The `DeviceName` the confirming request registers, when it registers one.
    device_name: Option<&'static str>,
}

/// `ConfirmDevice` confirms the device a sign-in minted the key for — which is what
/// makes the pool's readers report it — and reports whether the user still has to be
/// asked, which is the pool's `DeviceOnlyRememberedOnUserPrompt`. A device the user has
/// not been asked about yet is registered, so the admin reader reports it, but is not
/// remembered, so the self-service `GetDevice` does not.
#[tokio::test]
async fn confirm_device_registers_the_device_a_sign_in_minted() {
    let rows = [
        ConfirmRow {
            case: "pool remembers devices immediately, device named",
            device_configuration: json!({ "DeviceOnlyRememberedOnUserPrompt": false }),
            user_confirmation_necessary: false,
            device_name: Some("Alice's laptop"),
        },
        ConfirmRow {
            case: "pool prompts the user first, device named",
            device_configuration: json!({ "DeviceOnlyRememberedOnUserPrompt": true }),
            user_confirmation_necessary: true,
            device_name: Some("Alice's phone"),
        },
        ConfirmRow {
            case: "pool with no property set, device unnamed",
            device_configuration: json!({}),
            user_confirmation_necessary: false,
            device_name: None,
        },
    ];

    for row in rows {
        let router = router();
        let fixture =
            pool_with_client(&router, "confirm-pool", Some(row.device_configuration.clone())).await;
        create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;

        let result = sign_in(&router, &fixture, SIGN_INS[0], "alice", None).await;
        let device_key = new_device_key(row.case, &result).expect("a minted device");
        let access_token = result["AccessToken"].as_str().expect("an access token");

        // Before the confirm, the device the sign-in minted is not one the pool
        // reports: nothing has confirmed it.
        refused(
            &router,
            row.case,
            "AdminGetDevice",
            &json!({
                "UserPoolId": fixture.user_pool_id,
                "Username": "alice",
                "DeviceKey": device_key,
            }),
            400,
            "ResourceNotFoundException",
        )
        .await;

        let mut confirm = Map::new();
        confirm.insert("AccessToken".to_string(), json!(access_token));
        confirm.insert("DeviceKey".to_string(), json!(device_key));
        confirm.insert(
            "DeviceSecretVerifierConfig".to_string(),
            device_credentials(),
        );
        if let Some(device_name) = row.device_name {
            confirm.insert("DeviceName".to_string(), json!(device_name));
        }
        let confirmed = ok(&router, "ConfirmDevice", &Value::Object(confirm.clone())).await;
        assert_eq!(
            confirmed["UserConfirmationNecessary"].as_bool(),
            Some(row.user_confirmation_necessary),
            "case {:?}: expected UserConfirmationNecessary={}, got {confirmed}",
            row.case,
            row.user_confirmation_necessary
        );

        // The stored device, read back through the admin reader, which reports any
        // device registered to the user. The self-service reader answers only for a
        // REMEMBERED device, so it serves this one once the user's answer has made it
        // remembered — immediately when the pool asks for no answer.
        let get_device = json!({ "AccessToken": access_token, "DeviceKey": device_key });
        let admin_get_device = json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": "alice",
            "DeviceKey": device_key,
        });
        let mut readers = vec![("AdminGetDevice", admin_get_device)];
        if row.user_confirmation_necessary {
            refused(
                &router,
                row.case,
                "GetDevice",
                &get_device,
                400,
                "ResourceNotFoundException",
            )
            .await;
            ok(
                &router,
                "UpdateDeviceStatus",
                &json!({
                    "AccessToken": access_token,
                    "DeviceKey": device_key,
                    "DeviceRememberedStatus": "remembered",
                }),
            )
            .await;
        }
        readers.push(("GetDevice", get_device));

        for (reader, body) in readers {
            let case = format!("{} through {reader}", row.case);
            let read = ok(&router, reader, &body).await;
            let device = &read["Device"];
            assert_eq!(
                device["DeviceKey"].as_str(),
                Some(device_key.as_str()),
                "case {case:?}: the device must be the one confirmed, got {read}"
            );
            for member in [
                "DeviceCreateDate",
                "DeviceLastModifiedDate",
                "DeviceLastAuthenticatedDate",
            ] {
                assert!(
                    device[member].as_f64().is_some_and(|at| at > 0.0),
                    "case {case:?}: {member} must be the stored instant, got {read}"
                );
            }
            let name = device["DeviceAttributes"]
                .as_array()
                .and_then(|attributes| {
                    attributes
                        .iter()
                        .find(|attribute| attribute["Name"] == DEVICE_NAME_ATTRIBUTE)
                })
                .and_then(|attribute| attribute["Value"].as_str());
            assert_eq!(
                name, row.device_name,
                "case {case:?}: expected the registered device name {:?}, got {read}",
                row.device_name
            );
        }

        // Confirming the same device twice presents a `DeviceKey` that is no
        // longer an unconfirmed key of this user, which is an invalid parameter
        // of the request.
        refused(
            &router,
            row.case,
            "ConfirmDevice",
            &Value::Object(confirm),
            400,
            "InvalidParameterException",
        )
        .await;
    }
}

/// One row of the device-refusal table: a device call that cannot be served, and the
/// refusal it owes.
struct RefusalRow {
    case: &'static str,
    operation: &'static str,
    /// Built from the pool id and the access token of the signed-in user.
    body: fn(&str, &str) -> Value,
    status: u16,
    code: &'static str,
}

/// What the device surface refuses: a key no sign-in minted, a request that names no
/// signed-in user, and a pool that activated no device remembering at all.
#[tokio::test]
async fn the_device_surface_refuses_what_it_cannot_serve() {
    /// A well-formed `DeviceKey` no sign-in of these pools minted.
    const UNKNOWN_DEVICE_KEY: &str = "us-east-1_11111111-2222-3333-4444-555555555555";

    let rows = [
        RefusalRow {
            case: "GetDevice with a device key nothing minted",
            operation: "GetDevice",
            body: |_, token| json!({ "AccessToken": token, "DeviceKey": UNKNOWN_DEVICE_KEY }),
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "AdminGetDevice with a device key nothing minted",
            operation: "AdminGetDevice",
            body: |pool, _| {
                json!({
                    "UserPoolId": pool,
                    "Username": "alice",
                    "DeviceKey": UNKNOWN_DEVICE_KEY,
                })
            },
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "AdminGetDevice for a user the pool does not hold",
            operation: "AdminGetDevice",
            body: |pool, _| {
                json!({
                    "UserPoolId": pool,
                    "Username": "nobody",
                    "DeviceKey": UNKNOWN_DEVICE_KEY,
                })
            },
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "GetDevice with no access token, so no signed-in user",
            operation: "GetDevice",
            body: |_, _| json!({ "DeviceKey": UNKNOWN_DEVICE_KEY }),
            status: 400,
            code: "NotAuthorizedException",
        },
        RefusalRow {
            case: "GetDevice with an access token this emulator never issued",
            operation: "GetDevice",
            body: |_, _| {
                json!({ "AccessToken": "not.a.token", "DeviceKey": UNKNOWN_DEVICE_KEY })
            },
            status: 400,
            code: "NotAuthorizedException",
        },
        RefusalRow {
            case: "ConfirmDevice with a device key nothing minted",
            operation: "ConfirmDevice",
            body: |_, token| {
                json!({
                    "AccessToken": token,
                    "DeviceKey": UNKNOWN_DEVICE_KEY,
                    "DeviceSecretVerifierConfig": device_credentials(),
                })
            },
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalRow {
            case: "UpdateDeviceStatus with a device key nothing minted",
            operation: "UpdateDeviceStatus",
            body: |_, token| {
                json!({
                    "AccessToken": token,
                    "DeviceKey": UNKNOWN_DEVICE_KEY,
                    "DeviceRememberedStatus": "remembered",
                })
            },
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "AdminUpdateDeviceStatus with a device key nothing minted",
            operation: "AdminUpdateDeviceStatus",
            body: |pool, _| {
                json!({
                    "UserPoolId": pool,
                    "Username": "alice",
                    "DeviceKey": UNKNOWN_DEVICE_KEY,
                    "DeviceRememberedStatus": "not_remembered",
                })
            },
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "ForgetDevice with a device key nothing minted",
            operation: "ForgetDevice",
            body: |_, token| json!({ "AccessToken": token, "DeviceKey": UNKNOWN_DEVICE_KEY }),
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "AdminForgetDevice with a device key nothing minted",
            operation: "AdminForgetDevice",
            body: |pool, _| {
                json!({
                    "UserPoolId": pool,
                    "Username": "alice",
                    "DeviceKey": UNKNOWN_DEVICE_KEY,
                })
            },
            status: 400,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "ListDevices with a pagination token this listing never issued",
            operation: "ListDevices",
            body: |_, token| json!({ "AccessToken": token, "PaginationToken": "not-a-page" }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalRow {
            case: "ListDevices with an access token this emulator never issued",
            operation: "ListDevices",
            body: |_, _| json!({ "AccessToken": "not.a.token" }),
            status: 400,
            code: "NotAuthorizedException",
        },
        RefusalRow {
            case: "AdminListDevices for a user the pool does not hold",
            operation: "AdminListDevices",
            body: |pool, _| json!({ "UserPoolId": pool, "Username": "nobody" }),
            status: 400,
            code: "ResourceNotFoundException",
        },
    ];

    let router = router();
    let fixture = pool_with_client(&router, "refusal-pool", Some(json!({}))).await;
    create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
    let result = sign_in(&router, &fixture, SIGN_INS[0], "alice", None).await;
    let access_token = result["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();

    for row in rows {
        refused(
            &router,
            row.case,
            row.operation,
            &(row.body)(&fixture.user_pool_id, &access_token),
            row.status,
            row.code,
        )
        .await;
    }

    // A pool that provided no DeviceConfiguration activated no device remembering, so
    // every device operation against it is refused on the pool's configuration — and
    // its sign-ins mint no device at all.
    let bare = pool_with_client(&router, "no-device-pool", None).await;
    create_confirmed_user(&router, &bare.user_pool_id, "alice").await;
    let bare_result = sign_in(&router, &bare, SIGN_INS[0], "alice", None).await;
    let bare_token = bare_result["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();
    for (case, operation, body) in [
        (
            "GetDevice on a pool with no DeviceConfiguration",
            "GetDevice",
            json!({ "AccessToken": bare_token, "DeviceKey": UNKNOWN_DEVICE_KEY }),
        ),
        (
            "AdminGetDevice on a pool with no DeviceConfiguration",
            "AdminGetDevice",
            json!({
                "UserPoolId": bare.user_pool_id,
                "Username": "alice",
                "DeviceKey": UNKNOWN_DEVICE_KEY,
            }),
        ),
        (
            "ConfirmDevice on a pool with no DeviceConfiguration",
            "ConfirmDevice",
            json!({
                "AccessToken": bare_token,
                "DeviceKey": UNKNOWN_DEVICE_KEY,
                "DeviceSecretVerifierConfig": device_credentials(),
            }),
        ),
        (
            "AdminListDevices on a pool with no DeviceConfiguration",
            "AdminListDevices",
            json!({ "UserPoolId": bare.user_pool_id, "Username": "alice" }),
        ),
        (
            "UpdateDeviceStatus on a pool with no DeviceConfiguration",
            "UpdateDeviceStatus",
            json!({
                "AccessToken": bare_token,
                "DeviceKey": UNKNOWN_DEVICE_KEY,
                "DeviceRememberedStatus": "remembered",
            }),
        ),
        (
            "AdminUpdateDeviceStatus on a pool with no DeviceConfiguration",
            "AdminUpdateDeviceStatus",
            json!({
                "UserPoolId": bare.user_pool_id,
                "Username": "alice",
                "DeviceKey": UNKNOWN_DEVICE_KEY,
                "DeviceRememberedStatus": "remembered",
            }),
        ),
        (
            "ForgetDevice on a pool with no DeviceConfiguration",
            "ForgetDevice",
            json!({ "AccessToken": bare_token, "DeviceKey": UNKNOWN_DEVICE_KEY }),
        ),
        (
            "AdminForgetDevice on a pool with no DeviceConfiguration",
            "AdminForgetDevice",
            json!({
                "UserPoolId": bare.user_pool_id,
                "Username": "alice",
                "DeviceKey": UNKNOWN_DEVICE_KEY,
            }),
        ),
    ] {
        refused(
            &router,
            case,
            operation,
            &body,
            400,
            "InvalidUserPoolConfigurationException",
        )
        .await;
    }
}

/// The device keys `ListDevices` reports for the signed-in user `access_token` names,
/// paging with `limit` until the listing is exhausted, asserting on the way that a page
/// is never larger than the limit it asked for.
async fn listed_device_keys(
    router: &MultiServiceRouter,
    case: &str,
    access_token: &str,
    limit: Option<i64>,
) -> Vec<String> {
    let mut keys = Vec::new();
    let mut pagination_token = None;
    loop {
        let mut body = Map::new();
        body.insert("AccessToken".to_string(), json!(access_token));
        if let Some(limit) = limit {
            body.insert("Limit".to_string(), json!(limit));
        }
        if let Some(token) = pagination_token.clone() {
            body.insert("PaginationToken".to_string(), json!(token));
        }
        let page = ok(router, "ListDevices", &Value::Object(body)).await;
        let devices = page["Devices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Devices must be present, got {page}"))
            .clone();
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
            // A submitted `0` is read as "no value" by this service's pagination, as
            // the model already reads a `0` elsewhere, so it caps nothing.
            assert!(
                devices.len() as i64 <= limit,
                "case {case:?}: a page must hold at most Limit={limit} devices, got {page}"
            );
        }
        keys.extend(devices.iter().map(|device| {
            device["DeviceKey"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed device has a key, got {page}"))
                .to_string()
        }));
        pagination_token = page["PaginationToken"].as_str().map(str::to_string);
        if pagination_token.is_none() {
            return keys;
        }
    }
}

/// The device keys `AdminListDevices` reports for `username` in one page.
async fn admin_listed_device_keys(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let page = ok(
        router,
        "AdminListDevices",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    page["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Devices must be present, got {page}"))
        .iter()
        .map(|device| {
            device["DeviceKey"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed device has a key, got {page}"))
                .to_string()
        })
        .collect()
}

/// A listing reports the devices that were CONFIRMED for that user and no others, in
/// ascending device-key order, whatever page size it is asked for — and a device that is
/// forgotten leaves it, through either the self-service or the admin operation.
#[tokio::test]
async fn the_device_listing_follows_what_was_confirmed_and_forgotten() {
    let router = router();
    let fixture = pool_with_client(&router, "listing-pool", Some(json!({}))).await;
    create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
    create_confirmed_user(&router, &fixture.user_pool_id, "bob").await;

    // Three sign-ins from three unrecognized devices, two of them confirmed and the
    // third left as the sign-in minted it.
    let mut minted = Vec::new();
    let mut access_token = String::new();
    for index in 0..3 {
        let case = format!("device {index}");
        let result = sign_in(&router, &fixture, SIGN_INS[0], "alice", None).await;
        access_token = result["AccessToken"]
            .as_str()
            .expect("an access token")
            .to_string();
        minted.push(new_device_key(&case, &result).expect("a minted device"));
    }
    for key in &minted[..2] {
        ok(
            &router,
            "ConfirmDevice",
            &json!({
                "AccessToken": access_token,
                "DeviceKey": key,
                "DeviceSecretVerifierConfig": device_credentials(),
            }),
        )
        .await;
    }
    // Bob's own confirmed device, which must not show up in alice's listing.
    let bobs = sign_in(&router, &fixture, SIGN_INS[0], "bob", None).await;
    let bobs_token = bobs["AccessToken"].as_str().expect("an access token");
    let bobs_key = new_device_key("bob's device", &bobs).expect("a minted device");
    ok(
        &router,
        "ConfirmDevice",
        &json!({
            "AccessToken": bobs_token,
            "DeviceKey": bobs_key,
            "DeviceSecretVerifierConfig": device_credentials(),
        }),
    )
    .await;

    let mut expected: Vec<String> = minted[..2].to_vec();
    expected.sort();

    // Every page size spans the same listing: the modelled `Limit` range is 0..60, so
    // the rows below are its bounds and the sizes around the listing's own length.
    for limit in [None, Some(0), Some(1), Some(2), Some(60)] {
        let case = format!("ListDevices with Limit={limit:?}");
        let listed = listed_device_keys(&router, &case, &access_token, limit).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing is the confirmed devices in key order, got {listed:?}"
        );
    }
    let admin_listed =
        admin_listed_device_keys(&router, "AdminListDevices", &fixture.user_pool_id, "alice").await;
    assert_eq!(
        admin_listed, expected,
        "case \"AdminListDevices\": the admin listing must agree with the self-service one, \
         got {admin_listed:?}"
    );
    assert!(
        !admin_listed.contains(&bobs_key),
        "case \"another user's device\": a listing reports only its own user's devices, \
         got {admin_listed:?}"
    );

    // Forgetting a device takes it out of the listing — and out of what the pool
    // recognises, so a sign-in presenting its key mints a device again.
    ok(
        &router,
        "ForgetDevice",
        &json!({ "AccessToken": access_token, "DeviceKey": expected[0] }),
    )
    .await;
    let after_forget = listed_device_keys(&router, "after ForgetDevice", &access_token, None).await;
    assert_eq!(
        after_forget,
        expected[1..].to_vec(),
        "case \"after ForgetDevice\": the forgotten device leaves the listing, \
         got {after_forget:?}"
    );
    let reminted = sign_in(
        &router,
        &fixture,
        SIGN_INS[0],
        "alice",
        Some(&expected[0]),
    )
    .await;
    assert!(
        new_device_key("sign-in from a forgotten device", &reminted).is_some(),
        "case \"sign-in from a forgotten device\": the pool no longer recognises it, got {reminted}"
    );

    ok(
        &router,
        "AdminForgetDevice",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": "alice",
            "DeviceKey": expected[1],
        }),
    )
    .await;
    let admin_after =
        admin_listed_device_keys(&router, "after AdminForgetDevice", &fixture.user_pool_id, "alice")
            .await;
    assert!(
        !admin_after.contains(&expected[1]),
        "case \"after AdminForgetDevice\": the forgotten device leaves the listing, \
         got {admin_after:?}"
    );
}

/// One row of the remembered-status table: the `DeviceRememberedStatus` a request
/// names, and what the stored device must be remembered as afterwards.
struct StatusRow {
    case: &'static str,
    status: Option<&'static str>,
    remembered: bool,
}

/// `UpdateDeviceStatus` and `AdminUpdateDeviceStatus` record the answer the user gave
/// the "Remember me on this device" prompt, and a request that names no status changes
/// nothing. The answer is observable where it belongs: the self-service reader and the
/// device listing serve only a remembered device, while the admin reader reports the
/// device as registered to the user either way.
#[tokio::test]
async fn updating_a_devices_status_persists_the_remembered_answer() {
    // The modelled `DeviceRememberedStatusType` values, and the optional member left
    // out (`smithy_query(query="shape", shape="DeviceRememberedStatusType")`).
    let rows = [
        StatusRow {
            case: "remembered",
            status: Some("remembered"),
            remembered: true,
        },
        StatusRow {
            case: "not_remembered",
            status: Some("not_remembered"),
            remembered: false,
        },
        StatusRow {
            case: "no status named, so no change from the confirm",
            status: None,
            remembered: true,
        },
    ];

    for admin in [false, true] {
        for row in &rows {
            let case = format!(
                "{} through {}",
                row.case,
                if admin {
                    "AdminUpdateDeviceStatus"
                } else {
                    "UpdateDeviceStatus"
                }
            );
            let engine = Arc::new(engine::InMemoryEngine::new());
            let router = router_over(&engine);
            // A pool that remembers a confirmed device immediately, so the status
            // before the update is the remembered one.
            let fixture = pool_with_client(
                &router,
                "status-pool",
                Some(json!({ "DeviceOnlyRememberedOnUserPrompt": false })),
            )
            .await;
            create_confirmed_user(&router, &fixture.user_pool_id, "alice").await;
            let result = sign_in(&router, &fixture, SIGN_INS[0], "alice", None).await;
            let device_key = new_device_key(&case, &result).expect("a minted device");
            let access_token = result["AccessToken"].as_str().expect("an access token");
            ok(
                &router,
                "ConfirmDevice",
                &json!({
                    "AccessToken": access_token,
                    "DeviceKey": device_key,
                    "DeviceSecretVerifierConfig": device_credentials(),
                }),
            )
            .await;

            let mut body = Map::new();
            if admin {
                body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
                body.insert("Username".to_string(), json!("alice"));
            } else {
                body.insert("AccessToken".to_string(), json!(access_token));
            }
            body.insert("DeviceKey".to_string(), json!(device_key));
            if let Some(status) = row.status {
                body.insert("DeviceRememberedStatus".to_string(), json!(status));
            }
            let operation = if admin {
                "AdminUpdateDeviceStatus"
            } else {
                "UpdateDeviceStatus"
            };
            ok(&router, operation, &Value::Object(body)).await;

            // What the remembered answer changes on the wire: the self-service reader
            // serves only a remembered device, while the admin reader reports the
            // device either way and the listing still holds it.
            let get_device = json!({ "AccessToken": access_token, "DeviceKey": device_key });
            if row.remembered {
                let read = ok(&router, "GetDevice", &get_device).await;
                assert_eq!(
                    read["Device"]["DeviceKey"].as_str(),
                    Some(device_key.as_str()),
                    "case {case:?}: a remembered device is what GetDevice reports, got {read}"
                );
            } else {
                refused(
                    &router,
                    &case,
                    "GetDevice",
                    &get_device,
                    400,
                    "ResourceNotFoundException",
                )
                .await;
            }
            let admin_read = ok(
                &router,
                "AdminGetDevice",
                &json!({
                    "UserPoolId": fixture.user_pool_id,
                    "Username": "alice",
                    "DeviceKey": device_key,
                }),
            )
            .await;
            assert_eq!(
                admin_read["Device"]["DeviceKey"].as_str(),
                Some(device_key.as_str()),
                "case {case:?}: the admin reader reports a registered device whatever its \
                 remembered status, got {admin_read}"
            );
            let listed = listed_device_keys(&router, &case, access_token, None).await;
            let expected_listing = if row.remembered {
                vec![device_key.clone()]
            } else {
                // A device the user no longer remembers leaves the listing:
                // `ListDevices` reports the devices this user remembers.
                Vec::new()
            };
            assert_eq!(
                listed, expected_listing,
                "case {case:?}: expected the listing {expected_listing:?}, got {listed:?}"
            );

            let service_state = state::service_state(&*engine).expect("this service's state");
            let guard = service_state.lock();
            let stored = guard
                .pool(&fixture.user_pool_id)
                .and_then(|pool| pool.user("alice"))
                .and_then(|user| user.confirmed_device(&device_key))
                .unwrap_or_else(|| panic!("case {case:?}: the device must still be stored"));
            assert_eq!(
                stored.remembered, row.remembered,
                "case {case:?}: expected remembered={}, got {stored:?}",
                row.remembered
            );

            // The device is still the one the confirm registered: updating its status
            // neither forgets it nor unconfirms it.
            assert!(
                stored.confirmed,
                "case {case:?}: the device stays confirmed, got {stored:?}"
            );
        }
    }
}
