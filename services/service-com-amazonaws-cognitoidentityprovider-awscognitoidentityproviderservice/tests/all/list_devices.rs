// SPDX-License-Identifier: CC0-1.0

//! `ListDevices` over the wire, driven exactly as an SDK drives it (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.ListDevices`).
//!
//! The rule under test: a self-service read whose only subject is the
//! `AccessToken` REPORTS, from stored state, the devices registered to the user
//! that access token was issued to — each entry rendered from the STORED device,
//! so it carries that device's key, attributes and timestamps — in ascending
//! device-key order, a page of at most `Limit` entries at a time, where the
//! `PaginationToken` of a non-final page resumes the listing strictly after the
//! last device that page served and the final page carries none. It writes
//! nothing: a repeated call answers the same. A token this emulator never
//! issued, one that is not an ACCESS token, a revoked one and one the clock has
//! expired are all refused with 403 `NotAuthorizedException`; a token whose pool
//! or user is gone with 404 `ResourceNotFoundException` / `UserNotFoundException`;
//! and a `PaginationToken` this listing never minted with 400
//! `InvalidParameterException`.
//!
//! Devices come into existence through `ConfirmDevice` against a device key a
//! sign-in minted, and both of those are still scaffolds in this emulator (the
//! filed issue "device remembering is inert — AuthenticationResult never carries
//! NewDeviceMetadata"), so this suite PLANTS the device state they will write,
//! the way the other suites of this crate plant users and token records for
//! writers that are not implemented yet. The access tokens, pools, app clients
//! and users are all real: they come from `CreateUserPool`,
//! `CreateUserPoolClient`, `AdminCreateUser` and `InitiateAuth`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// `QueryLimitType`'s modelled range is 0..60
/// (`smithy_query(query="operation",operation="ListDevices")`), so 60 is the
/// most one page can hold and what a request naming no usable `Limit` gets.
const MAX_LIMIT: usize = 60;

/// One emulator with just this service mounted, and the engine its state lives
/// in, so a suite can plant the state a writer that is still a scaffold will
/// write.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    Emulator { engine, router }
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls `ListDevices` — the
/// model marks the operation `@optionalAuth`.
async fn dispatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
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

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client that allows the password sign-in this suite uses, and
/// return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "device-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` with a PERMANENT password, so the user is `CONFIRMED` and
/// can sign in.
async fn confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
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

/// Sign `username` in through an unsigned `InitiateAuth` and return the
/// `AuthenticationResult` it answered.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, username: &str) -> Value {
    let (status, payload) = dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"].clone()
}

/// A pool with one app client, and the users this suite signs in.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Create a pool with a sign-in client and the users `usernames`.
async fn fixture(router: &MultiServiceRouter, name: &str, usernames: &[&str]) -> Fixture {
    let user_pool_id = pool(router, name).await;
    let client_id = client(router, &user_pool_id).await;
    for username in usernames {
        confirmed_user(router, &user_pool_id, username).await;
    }
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// One device to plant in the store. The rows of every table below differ along
/// the COUNT of devices, the COUNT of attributes each carries and whether the
/// device has ever authenticated.
#[derive(Clone, Copy, Debug)]
struct PlantedDevice {
    /// A `DeviceKeyType` value (length 1..55, pattern `^[\w-]+_[0-9a-f-]+$`,
    /// from `smithy_query(query="operation",operation="ConfirmDevice")`).
    key: &'static str,
    attributes: &'static [(&'static str, &'static str)],
    /// When the device was created, in seconds since the epoch.
    created_at: u64,
    /// When the device last authenticated, if it ever has.
    last_authenticated_at: Option<u64>,
}

impl PlantedDevice {
    /// This device as the store holds one.
    fn stored(self) -> state::Device {
        state::Device {
            device_key: self.key.to_string(),
            attributes: self
                .attributes
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            secret_verifier: None,
            // A planted device stands for one a `ConfirmDevice` registered and
            // the user chose to remember, which is what a listing reports.
            confirmed: true,
            remembered: true,
            create_date: UNIX_EPOCH + Duration::from_secs(self.created_at),
            last_modified_date: UNIX_EPOCH + Duration::from_secs(self.created_at + 60),
            last_authenticated_date: self
                .last_authenticated_at
                .map(|at| UNIX_EPOCH + Duration::from_secs(at)),
        }
    }
}

/// Plant `devices` on the stored user `username` of `user_pool_id`.
fn plant(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    devices: &[PlantedDevice],
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let pool = guard.pool_mut(user_pool_id).expect("the created pool");
    let user = pool.user_mut(username).expect("the created user");
    for device in devices {
        user.devices.insert(device.key.to_string(), device.stored());
    }
}

/// The device keys the store holds for `username`, in the order it holds them.
fn stored_keys(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .expect("the created pool")
        .user(username)
        .expect("the created user")
        .device_keys()
}

/// One `ListDevices` request body: the optional members a case leaves unset are
/// left OUT rather than sent as null.
fn list_request(access_token: &str, limit: Option<i32>, pagination_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("AccessToken".to_string(), json!(access_token));
    if let Some(limit) = limit {
        body.insert("Limit".to_string(), json!(limit));
    }
    if let Some(token) = pagination_token {
        body.insert("PaginationToken".to_string(), json!(token));
    }
    Value::Object(body)
}

/// Call `ListDevices` unsigned and return its status and body.
async fn list(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    dispatch(router, "ListDevices", body, false).await
}

/// Call `ListDevices` unsigned, requiring success, and return its body.
async fn list_ok(router: &MultiServiceRouter, body: &Value, label: &str) -> Value {
    let (status, payload) = list(router, body).await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListDevices must succeed, got {status} with {payload}"
    );
    payload
}

/// The `Devices` entries of one response, requiring the member to be present.
fn devices_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Devices must be a list, got {payload}"))
        .clone()
}

/// The `DeviceKey`s of one response, in the order it served them.
fn keys_of(payload: &Value, label: &str) -> Vec<String> {
    devices_of(payload, label)
        .iter()
        .map(|entry| {
            entry["DeviceKey"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: an entry must name a device, got {entry}")
                })
                .to_string()
        })
        .collect()
}

/// Assert that `entry` is `planted` rendered as `DeviceType`.
fn assert_entry(label: &str, entry: &Value, planted: PlantedDevice) {
    assert_eq!(
        entry["DeviceKey"],
        json!(planted.key),
        "case {label:?}: the entry carries the stored device key, got {entry}"
    );
    assert_eq!(
        entry["DeviceCreateDate"].as_f64(),
        Some(planted.created_at as f64),
        "case {label:?}: the entry carries the stored create date, got {entry}"
    );
    assert_eq!(
        entry["DeviceLastModifiedDate"].as_f64(),
        Some((planted.created_at + 60) as f64),
        "case {label:?}: the entry carries the stored last-modified date, got {entry}"
    );
    assert_eq!(
        entry.get("DeviceLastAuthenticatedDate").and_then(Value::as_f64),
        planted.last_authenticated_at.map(|at| at as f64),
        "case {label:?}: the entry carries the stored last-authenticated date, and none at \
         all for a device that never authenticated, got {entry}"
    );
    let expected_attributes: Vec<Value> = planted
        .attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    let actual_attributes: Vec<Value> = entry
        .get("DeviceAttributes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        actual_attributes, expected_attributes,
        "case {label:?}: the entry carries the stored attributes in name order, got {entry}"
    );
}

/// Walk every page of the listing `access_token` names with `limit`, and return
/// the entries in the order they were served and how many pages it took,
/// asserting on the way that no page holds more than `limit` entries and that
/// only a non-final page carries a `PaginationToken`.
async fn walk(
    router: &MultiServiceRouter,
    access_token: &str,
    limit: Option<i32>,
    label: &str,
) -> (Vec<Value>, usize) {
    // A request naming no `Limit`, or naming `0` — a page that may hold no item
    // terminates no pagination loop — takes the modelled maximum.
    let page_size = match limit {
        Some(limit) if limit > 0 => (limit as usize).min(MAX_LIMIT),
        _ => MAX_LIMIT,
    };
    let mut served: Vec<Value> = Vec::new();
    let mut pagination_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let payload = list_ok(
            router,
            &list_request(access_token, limit, pagination_token.as_deref()),
            label,
        )
        .await;
        pages += 1;
        let page = devices_of(&payload, label);
        assert!(
            page.len() <= page_size,
            "case {label:?}: page {pages} holds at most {page_size} entries, got {}",
            page.len()
        );
        served.extend(page);
        pagination_token = payload
            .get("PaginationToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &pagination_token {
            None => break,
            Some(token) => assert!(
                !token.is_empty() && !token.contains(char::is_whitespace),
                "case {label:?}: a SearchPaginationTokenType matches ^[\\S]+$, got {token:?}"
            ),
        }
        assert!(
            pages <= 1 + MAX_LIMIT,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// The seven devices the paging table pages over, in no particular order: the
/// listing's order is the ascending device-key order, not the order they were
/// stored in.
const SEVEN_DEVICES: [PlantedDevice; 7] = [
    PlantedDevice {
        key: "us-east-1_3f55",
        attributes: &[("device_name", "tablet")],
        created_at: 1_700_000_300,
        last_authenticated_at: None,
    },
    PlantedDevice {
        key: "us-east-1_000a",
        attributes: &[("device_name", "laptop"), ("device_status", "valid")],
        created_at: 1_700_000_000,
        last_authenticated_at: Some(1_700_000_900),
    },
    PlantedDevice {
        key: "us-east-1_4066",
        attributes: &[],
        created_at: 1_700_000_400,
        last_authenticated_at: Some(1_700_000_500),
    },
    PlantedDevice {
        key: "us-east-1_00b1",
        attributes: &[("device_name", "phone")],
        created_at: 1_700_000_100,
        last_authenticated_at: None,
    },
    PlantedDevice {
        key: "us-east-1_2e44",
        attributes: &[("device_name", "desktop")],
        created_at: 1_700_000_200,
        last_authenticated_at: Some(1_700_000_700),
    },
    PlantedDevice {
        key: "us-east-1_1d33",
        attributes: &[("last_ip_used", "203.0.113.7")],
        created_at: 1_700_000_600,
        last_authenticated_at: None,
    },
    PlantedDevice {
        key: "us-east-1_0c22",
        attributes: &[("device_name", "watch"), ("last_ip_used", "203.0.113.9")],
        created_at: 1_700_000_800,
        last_authenticated_at: Some(1_700_000_950),
    },
];

/// The keys of `devices` in the ascending order the listing serves them in.
fn sorted_keys(devices: &[PlantedDevice]) -> Vec<String> {
    let mut keys: Vec<String> = devices.iter().map(|device| device.key.to_string()).collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn a_listing_reports_every_stored_device_rendered_from_the_stored_device() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(
        &router,
        "listing-pool",
        &["nobody", "single", "several", "seven"],
    )
    .await;

    // The rows differ along the COUNT of stored devices: none, one, a few and
    // more than any one page of the table below holds.
    for (label, username, devices) in [
        ("a user with no remembered device", "nobody", &[][..]),
        ("a user with one device", "single", &SEVEN_DEVICES[..1]),
        ("a user with three devices", "several", &SEVEN_DEVICES[..3]),
        ("a user with seven devices", "seven", &SEVEN_DEVICES[..]),
    ] {
        plant(&engine, &fixture.user_pool_id, username, devices);
        let result = sign_in(&router, &fixture.client_id, username).await;
        let access_token = result["AccessToken"].as_str().expect("an access token");

        let payload = list_ok(&router, &list_request(access_token, None, None), label).await;
        let actual = keys_of(&payload, label);
        let expected = sorted_keys(devices);
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored device once, in device-key \
             order: expected {expected:?}, got {actual:?}"
        );
        assert!(
            payload.get("PaginationToken").is_none(),
            "case {label:?}: a listing that served everything carries no PaginationToken, \
             got {payload}"
        );

        // Each entry is the STORED device, member for member.
        for entry in devices_of(&payload, label) {
            let key = entry["DeviceKey"].as_str().expect("an entry names a device");
            let planted = devices
                .iter()
                .find(|device| device.key == key)
                .unwrap_or_else(|| panic!("case {label:?}: {key:?} was planted above"));
            assert_entry(label, &entry, *planted);
        }

        // The listing is a READ: calling it again answers the same, and the
        // devices the store holds are unchanged.
        let again = list_ok(&router, &list_request(access_token, None, None), label).await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated listing answers the same, got {again}"
        );
        let stored = stored_keys(&engine, &fixture.user_pool_id, username);
        assert_eq!(
            stored, expected,
            "case {label:?}: the listing changed the stored devices: expected {expected:?}, \
             got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_over_the_stored_order_under_every_limit() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "paging-pool", &["paged"]).await;
    plant(&engine, &fixture.user_pool_id, "paged", &SEVEN_DEVICES);
    let result = sign_in(&router, &fixture.client_id, "paged").await;
    let access_token = result["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();
    let expected = sorted_keys(&SEVEN_DEVICES);

    // The rows span the published bounds of `Limit` (0 and 60), the sizes that
    // divide and do not divide the seven devices, the size that holds them all
    // exactly, and an absent `Limit`.
    for (label, limit, expected_pages) in [
        ("no Limit at all", None, 1),
        ("Limit 1, the smallest page that makes progress", Some(1), 7),
        ("Limit 2, which does not divide seven", Some(2), 4),
        ("Limit 7, exactly the device count", Some(7), 1),
        ("Limit 6, one short of the device count", Some(6), 2),
        ("Limit 60, the modelled maximum", Some(60), 1),
        ("Limit 0, the modelled minimum", Some(0), 1),
    ] {
        let (served, pages) = walk(&router, &access_token, limit, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| entry["DeviceKey"].as_str().expect("a key").to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every device exactly once in \
             device-key order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
        // Every entry a page served is the stored device, whatever page it came
        // on.
        for entry in &served {
            let key = entry["DeviceKey"].as_str().expect("an entry names a device");
            let planted = SEVEN_DEVICES
                .iter()
                .find(|device| device.key == key)
                .unwrap_or_else(|| panic!("case {label:?}: {key:?} was planted above"));
            assert_entry(label, entry, *planted);
        }
    }
}

#[tokio::test]
async fn a_listing_reports_the_devices_of_the_tokens_own_user_alone() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "one-user-pool", &["alice", "bob", "carol"]).await;
    // `alice` and `bob` hold different devices; `carol` holds none.
    plant(&engine, &fixture.user_pool_id, "alice", &SEVEN_DEVICES[..3]);
    plant(&engine, &fixture.user_pool_id, "bob", &SEVEN_DEVICES[3..]);

    let mut tokens: Vec<(&str, String)> = Vec::new();
    for username in ["alice", "bob", "carol"] {
        let result = sign_in(&router, &fixture.client_id, username).await;
        tokens.push((
            username,
            result["AccessToken"]
                .as_str()
                .expect("an access token")
                .to_string(),
        ));
    }

    for (label, username, expected) in [
        (
            "the user the first devices were planted on",
            "alice",
            sorted_keys(&SEVEN_DEVICES[..3]),
        ),
        (
            "another user of the same pool",
            "bob",
            sorted_keys(&SEVEN_DEVICES[3..]),
        ),
        ("a user with no device at all", "carol", Vec::new()),
    ] {
        let access_token = &tokens
            .iter()
            .find(|(name, _)| *name == username)
            .expect("the signed-in user")
            .1;
        let payload = list_ok(&router, &list_request(access_token, None, None), label).await;
        let actual = keys_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // A `PaginationToken` is a page of ONE user's listing: `alice`'s names no
    // page of `bob`'s.
    let alices_token = &tokens[0].1;
    let bobs_token = &tokens[1].1;
    let first_page = list_ok(
        &router,
        &list_request(alices_token, Some(1), None),
        "alice's first page",
    )
    .await;
    let cursor = first_page["PaginationToken"]
        .as_str()
        .expect("a non-final page carries a PaginationToken")
        .to_string();
    let (status, payload) = list(&router, &list_request(bobs_token, Some(1), Some(&cursor))).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "a PaginationToken minted for another user's listing names no page of this one, got \
         {status} with {payload}"
    );
}

#[tokio::test]
async fn an_access_token_that_names_no_signed_in_user_is_refused() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "refusal-pool", &["dave"]).await;
    plant(&engine, &fixture.user_pool_id, "dave", &SEVEN_DEVICES[..3]);
    let result = sign_in(&router, &fixture.client_id, "dave").await;
    let access_token = result["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();
    let id_token = result["IdToken"]
        .as_str()
        .expect("an id token")
        .to_string();
    let refresh_token = result["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();

    // A second sign-in, whose access token this test then revokes, so revoking
    // it does not disturb the token the rest of the table uses.
    let revoked = sign_in(&router, &fixture.client_id, "dave").await["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();

    // Three token records planted directly: an access token the clock has
    // already expired, one naming a pool this emulator does not hold, and one
    // naming a user its pool does not hold. None of those can be reached by
    // signing in, and each is one of the rejections the operation models an
    // error for.
    const EXPIRED: &str = "expired.access.token";
    const POOL_GONE: &str = "pool-gone.access.token";
    const USER_GONE: &str = "user-gone.access.token";
    {
        let service_state = state::service_state(&*engine).expect("this service's state");
        let mut guard = service_state.lock();
        guard
            .issued_tokens
            .get_mut(&revoked)
            .expect("the recorded access token")
            .revoked = true;
        let now = SystemTime::now();
        let record = |user_pool_id: &str, username: &str, expires_at: SystemTime| {
            state::TokenRecord {
                user_pool_id: user_pool_id.to_string(),
                client_id: fixture.client_id.clone(),
                username: username.to_string(),
                kind: state::TokenKind::Access,
                issued_at: now - Duration::from_secs(60),
                expires_at,
                revoked: false,
            }
        };
        let live = now + Duration::from_secs(3_600);
        for (token, planted) in [
            (
                EXPIRED,
                record(&fixture.user_pool_id, "dave", now - Duration::from_secs(1)),
            ),
            (POOL_GONE, record("us-east-1_absent", "dave", live)),
            (
                USER_GONE,
                record(&fixture.user_pool_id, "absent-user", live),
            ),
        ] {
            guard.issued_tokens.insert(token.to_string(), planted);
        }
    }

    for (label, token, expected) in [
        (
            "a token this emulator never issued",
            "not-an-issued-token".to_string(),
            (403, "NotAuthorizedException"),
        ),
        (
            "the id token of the same sign-in, which is not an access token",
            id_token,
            (403, "NotAuthorizedException"),
        ),
        (
            "the refresh token of the same sign-in",
            refresh_token,
            (403, "NotAuthorizedException"),
        ),
        (
            "a revoked access token",
            revoked.clone(),
            (403, "NotAuthorizedException"),
        ),
        (
            "an access token the clock has expired",
            EXPIRED.to_string(),
            (403, "NotAuthorizedException"),
        ),
        (
            "an access token naming a user pool this emulator does not hold",
            POOL_GONE.to_string(),
            (404, "ResourceNotFoundException"),
        ),
        (
            "an access token naming a user its pool does not hold",
            USER_GONE.to_string(),
            (404, "UserNotFoundException"),
        ),
    ] {
        let (status, payload) = list(&router, &list_request(&token, None, None)).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // A `PaginationToken` this listing never minted names no page of it.
    for (label, cursor) in [
        ("a PaginationToken no listing of this service minted", "not-a-token"),
        (
            "a PaginationToken that is a device key rather than a cursor",
            SEVEN_DEVICES[0].key,
        ),
    ] {
        let (status, payload) = list(&router, &list_request(&access_token, None, Some(cursor))).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: the stored devices still list.
    let payload = list_ok(
        &router,
        &list_request(&access_token, None, None),
        "after the refusals",
    )
    .await;
    let actual = keys_of(&payload, "after the refusals");
    assert_eq!(
        actual,
        sorted_keys(&SEVEN_DEVICES[..3]),
        "a refused listing changes no stored device, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "constraint-pool", &["erin"]).await;
    plant(&engine, &fixture.user_pool_id, "erin", &SEVEN_DEVICES[..2]);
    let result = sign_in(&router, &fixture.client_id, "erin").await;
    let access_token = result["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListDevices")`): `Limit`
    // range 0..60, `AccessToken` pattern `^[A-Za-z0-9-_=.]+$`, and
    // `PaginationToken` length 1.. with pattern `^[\S]+$`.
    for (label, body) in [
        (
            "Limit 61, one past the range maximum 60",
            list_request(&access_token, Some(61), None),
        ),
        (
            "Limit -1, one below the range minimum 0",
            list_request(&access_token, Some(-1), None),
        ),
        (
            "an access token holding a character the pattern excludes",
            list_request("two words", None, None),
        ),
        (
            "an empty PaginationToken, below the length minimum 1",
            list_request(&access_token, None, Some("")),
        ),
        (
            "a PaginationToken holding a space, which the pattern excludes",
            list_request(&access_token, None, Some("two words")),
        ),
        ("a request missing the required AccessToken", json!({})),
    ] {
        let (status, payload) = list(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} \
             with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got \
             {payload}"
        );
    }
}
