// SPDX-License-Identifier: CC0-1.0

//! `ListUserPoolClientSecrets` over the wire: the secrets an app client holds.
//!
//! The rule under test. The operation "lists all client secrets associated with a user
//! pool app client. Returns metadata about the secrets. The response does not include
//! pagination tokens as there are only 2 secrets at any given time and we return both
//! with every ListUserPoolClientSecrets call. For security reasons, the response never
//! reveals the actual secret value in ClientSecretValue" (this operation in the service
//! model). So one call reports EVERY secret the named app client of the named pool
//! currently holds — the one its create stored and every one a later add stored — each as
//! the metadata of `ClientSecretDescriptorType` (`ClientSecretId`, "this identifier
//! follows the format <client-id>--<epoch-create-time>", and `ClientSecretCreateDate`)
//! with no `ClientSecretValue`, with no `NextToken`, and without changing what a later
//! caller sees.
//!
//! The rows below differ along what that rule generalises over: how many secrets the app
//! client holds (none, one, the two an app client may hold at once), where each one came
//! from (the create's `GenerateSecret`, a value the create supplied, a generated add, an
//! add that supplied a value across the published bounds of `ClientSecretType`, 24..64),
//! and whether the request presents a `NextToken`.
//!
//! The listing is read back against the operations that STORED those secrets
//! (`CreateUserPoolClient` and `AddUserPoolClientSecret`) and against `GetClientToken`,
//! whose machine-to-machine grant is authorized by each stored secret, so a listing that
//! reported or consumed anything other than the stored set fails here.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The region the suite's calls are signed for.
const DEFAULT_REGION: &str = "us-east-1";

/// The operation under test.
const LIST: &str = "ListUserPoolClientSecrets";

/// The operation that stores one further secret on an app client.
const ADD: &str = "AddUserPoolClientSecret";

/// The `ExplicitAuthFlows` value that configures an app client for machine-to-machine
/// authorization, which is the grant a stored secret authorizes.
const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// How many secrets one app client holds at a time: `AddUserPoolClientSecret` "supports
/// up to 2 active secrets per app client" (that operation in the service model).
const MAX_CLIENT_SECRETS: usize = 2;

/// A `ClientSecretType`-shaped value (pattern `^[\w+]+$`) of the modelled MINIMUM length,
/// 24 (`smithy_query(query="shape",shape="ClientSecretType")`).
const SHORTEST_SECRET: &str = "cccccccccccccccccccccccc";

/// A `ClientSecretType`-shaped value of the modelled MAXIMUM length, 64.
const LONGEST_SECRET: &str = "d123456789_123456789_123456789_123456789_123456789_123456789_12+";

/// A `ClientSecretType`-shaped value no app client of the suite holds.
const UNHELD_SECRET: &str = "nosuchclientsecret000000000";

/// A `UserPoolIdType`-shaped id (pattern `^[\w-]+_[0-9a-zA-Z]+$`) no create minted.
const UNKNOWN_POOL_ID: &str = "us-east-1_nosuchpool";

/// A `ClientIdType`-shaped id (pattern `^[\w+]+$`) no create minted.
const UNKNOWN_CLIENT_ID: &str = "nosuchappclient0000";

/// A `PaginationKey`-shaped token (pattern `^[\S]+$`) a caller may present even though
/// this listing issues none.
const PRESENTED_TOKEN: &str = "cHJlc2VudGVkLXRva2Vu";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{DEFAULT_REGION}/{SIGNING_NAME}/aws4_request, \
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
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {payload}"
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Call `operation` expecting a refusal, assert its status and wire error code, and hand
/// the body back so a caller can read the message too.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (expected_status, expected_code),
        "case {case:?}: expected ({expected_status}, {expected_code:?}), \
         got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    payload
}

/// What the create stored on the app client, which is the first half of the set a listing
/// reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatedWith {
    /// `GenerateSecret: true`, so the create minted the client's first secret.
    Generated,
    /// The create supplied this `ClientSecret` value.
    Supplied(&'static str),
    /// Neither, so the app client is public and holds no secret at all.
    Public,
}

/// A created pool and the app client of it whose secrets are listed.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    /// Every secret VALUE the app client holds, in the order it was stored.
    held: Vec<String>,
    /// Every `ClientSecretId` an add answered for this client.
    added_ids: Vec<String>,
}

/// Create a pool named `pool_name` and one app client of it, configured for the
/// machine-to-machine grant so a stored secret can be read back through it.
async fn fixture(
    router: &MultiServiceRouter,
    case: &str,
    pool_name: &str,
    created_with: CreatedWith,
) -> Fixture {
    let created = ok(router, case, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!("listed-client"));
    body.insert(
        "ExplicitAuthFlows".to_string(),
        json!([CLIENT_TOKEN_AUTH_FLOW]),
    );
    match created_with {
        CreatedWith::Generated => {
            body.insert("GenerateSecret".to_string(), json!(true));
        }
        CreatedWith::Supplied(secret) => {
            body.insert("ClientSecret".to_string(), json!(secret));
        }
        CreatedWith::Public => {}
    }
    let client = ok(router, case, "CreateUserPoolClient", &Value::Object(body)).await;

    Fixture {
        user_pool_id,
        client_id: client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string(),
        held: client["UserPoolClient"]["ClientSecret"]
            .as_str()
            .map(str::to_string)
            .into_iter()
            .collect(),
        added_ids: Vec::new(),
    }
}

/// Store one further secret on the fixture's app client — the value `supplied`, or a
/// generated one — and record what it stored.
async fn add_secret(
    router: &MultiServiceRouter,
    case: &str,
    fixture: &mut Fixture,
    supplied: Option<&str>,
) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
    body.insert("ClientId".to_string(), json!(fixture.client_id));
    if let Some(secret) = supplied {
        body.insert("ClientSecret".to_string(), json!(secret));
    }
    let payload = ok(router, case, ADD, &Value::Object(body)).await;
    let descriptor = &payload["ClientSecretDescriptor"];
    fixture.added_ids.push(
        descriptor["ClientSecretId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the add must answer a ClientSecretId, got {payload}"))
            .to_string(),
    );
    // "The ClientSecretValue is only returned when Amazon Cognito generates the secret"
    // (`AddUserPoolClientSecret.ClientSecretDescriptor`), so a generated value is only
    // knowable from the add that minted it.
    let stored = match supplied {
        Some(secret) => secret.to_string(),
        None => descriptor["ClientSecretValue"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a generated secret comes back in ClientSecretValue, got {payload}"))
            .to_string(),
    };
    fixture.held.push(stored);
}

/// Whether the machine-to-machine grant of `client_id` accepts `secret`: the grant "must
/// configure the app client with a client secret" and is authorized by the STORED one
/// (`GetClientToken` in the service model).
async fn grant_accepts(router: &MultiServiceRouter, client_id: &str, secret: &str) -> bool {
    let (status, payload) = dispatch(
        router,
        "GetClientToken",
        &json!({ "ClientId": client_id, "Secret": secret }),
    )
    .await;
    if status == 200 {
        return true;
    }
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "a refused grant is a 400 NotAuthorizedException, got ({status}, {:?}) with {payload}",
        error_code(&payload)
    );
    false
}

/// The list request's body: `NextToken` only when the row presents one.
fn list_body(fixture: &Fixture, next_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(fixture.user_pool_id));
    body.insert("ClientId".to_string(), json!(fixture.client_id));
    if let Some(token) = next_token {
        body.insert("NextToken".to_string(), json!(token));
    }
    Value::Object(body)
}

/// Assert the shape of one listed descriptor and return its `ClientSecretId` and
/// `ClientSecretCreateDate`.
fn descriptor_metadata(case: &str, client_id: &str, descriptor: &Value) -> (String, f64) {
    // "This identifier follows the format <client-id>--<epoch-create-time>"
    // (`ClientSecretDescriptorType.ClientSecretId`).
    let id = descriptor
        .get("ClientSecretId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a listed secret must carry a ClientSecretId, got {descriptor}"));
    let suffix = id.strip_prefix(&format!("{client_id}--")).unwrap_or_else(|| {
        panic!("case {case:?}: expected a ClientSecretId of the form {client_id}--<epoch-create-time>, got {id:?}")
    });
    assert!(
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: the ClientSecretId's create time must be an epoch time, got {id:?}"
    );
    // The modelled `ClientSecretIdType`: length 1..128.
    assert!(
        (1..=128).contains(&id.len()),
        "case {case:?}: a ClientSecretId is 1..128 characters, got {} in {id:?}",
        id.len()
    );

    let create_date = descriptor
        .get("ClientSecretCreateDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: a listed secret must carry a ClientSecretCreateDate, got {descriptor}"));
    assert!(
        create_date > 0.0,
        "case {case:?}: the ClientSecretCreateDate must be an epoch timestamp, got {create_date}"
    );

    // "For security reasons, the response never reveals the actual secret value in
    // ClientSecretValue" (this operation in the service model).
    assert!(
        descriptor.get("ClientSecretValue").is_none(),
        "case {case:?}: a listed secret reveals no ClientSecretValue, got {descriptor}"
    );

    (id.to_string(), create_date)
}

/// Assert one listing of `fixture`'s app client and return the ids it reported.
fn listed_ids(case: &str, fixture: &Fixture, payload: &Value) -> Vec<String> {
    // "The response does not include pagination tokens ... we return both with every
    // ListUserPoolClientSecrets call" (this operation in the service model).
    assert!(
        payload.get("NextToken").is_none(),
        "case {case:?}: the listing issues no pagination token, got {payload}"
    );

    // A listing's result member is PRESENT and empty when the app client holds no
    // secret, which is what a reading client parses.
    let secrets = payload
        .get("ClientSecrets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the listing must answer a ClientSecrets list, got {payload}"));
    assert_eq!(
        secrets.len(),
        fixture.held.len(),
        "case {case:?}: expected {} listed secret(s), got {} in {payload}",
        fixture.held.len(),
        secrets.len()
    );

    let mut ids = Vec::new();
    let mut previous_date = f64::MIN;
    for descriptor in secrets {
        let (id, create_date) = descriptor_metadata(case, &fixture.client_id, descriptor);
        assert!(
            create_date >= previous_date,
            "case {case:?}: the listing reports the secrets oldest first, got {create_date} after {previous_date} in {payload}"
        );
        previous_date = create_date;
        assert!(
            !ids.contains(&id),
            "case {case:?}: each listed secret carries its own id, got {id:?} twice in {payload}"
        );
        ids.push(id);
    }

    // No secret VALUE the app client holds appears anywhere in the response.
    let rendered = payload.to_string();
    for secret in &fixture.held {
        assert!(
            !rendered.contains(secret.as_str()),
            "case {case:?}: the listing must not reveal the secret value {secret:?}, got {rendered}"
        );
    }

    // Every id an add minted is one of the ids the listing reports.
    for added in &fixture.added_ids {
        assert!(
            ids.contains(added),
            "case {case:?}: the listing must report the added secret {added:?}, got {ids:?}"
        );
    }
    ids
}

/// One row of the listing table: what the create stored and what each later add stores.
struct ListCase {
    case: &'static str,
    created_with: CreatedWith,
    /// One entry per add, each the `ClientSecret` it supplies or `None` for a generated
    /// one.
    adds: &'static [Option<&'static str>],
}

#[tokio::test]
async fn a_listing_reports_every_client_secret_the_app_client_holds() {
    // The two supplied values below sit on the published bounds of `ClientSecretType`
    // (length 24..64), so a typo in either is a failure of this suite rather than a
    // surprise refusal by the framework's constraint checks.
    assert_eq!(
        (SHORTEST_SECRET.len(), LONGEST_SECRET.len()),
        (24, 64),
        "the bound rows must sit on the modelled bounds, got ({}, {})",
        SHORTEST_SECRET.len(),
        LONGEST_SECRET.len()
    );

    let rows = [
        ListCase {
            case: "an app client that holds no secret at all",
            created_with: CreatedWith::Public,
            adds: &[],
        },
        ListCase {
            case: "the one secret the create generated",
            created_with: CreatedWith::Generated,
            adds: &[],
        },
        ListCase {
            case: "the one secret the create supplied",
            created_with: CreatedWith::Supplied("createsecret000000000000011"),
            adds: &[],
        },
        ListCase {
            case: "a generated add beside the generated create secret",
            created_with: CreatedWith::Generated,
            adds: &[None],
        },
        ListCase {
            case: "an add supplying the shortest modelled secret",
            created_with: CreatedWith::Supplied("createsecret000000000000012"),
            adds: &[Some(SHORTEST_SECRET)],
        },
        ListCase {
            case: "both secrets of a client created public",
            created_with: CreatedWith::Public,
            adds: &[None, Some(LONGEST_SECRET)],
        },
    ];

    for (at, row) in rows.iter().enumerate() {
        let router = router();
        let case = row.case;
        let mut fixture =
            fixture(&router, case, &format!("listing-pool-{at}"), row.created_with).await;

        // Before any add, the listing already reports what the create stored.
        let payload = ok(&router, case, LIST, &list_body(&fixture, None)).await;
        let before = listed_ids(case, &fixture, &payload);

        for supplied in row.adds {
            add_secret(&router, case, &mut fixture, *supplied).await;
        }
        assert!(
            fixture.held.len() <= MAX_CLIENT_SECRETS,
            "case {case:?}: a row may not ask for more than {MAX_CLIENT_SECRETS} secrets, asked for {}",
            fixture.held.len()
        );

        // After the adds, the listing reports the stored set — the create's secret and
        // every added one.
        let payload = ok(&router, case, LIST, &list_body(&fixture, None)).await;
        let after = listed_ids(case, &fixture, &payload);
        assert!(
            before.iter().all(|id| after.contains(id)),
            "case {case:?}: an add leaves the secrets already listed in place, had {before:?} and got {after:?}"
        );

        // The listing is a report: a second call answers the same set, and every secret
        // the app client held before it still authorizes the grant afterwards.
        let repeated = ok(&router, case, LIST, &list_body(&fixture, None)).await;
        assert_eq!(
            listed_ids(case, &fixture, &repeated),
            after,
            "case {case:?}: a second listing reports the same secrets"
        );
        for secret in &fixture.held {
            assert!(
                grant_accepts(&router, &fixture.client_id, secret).await,
                "case {case:?}: the listed secret {secret:?} must still authenticate the app client"
            );
        }
        assert!(
            !grant_accepts(&router, &fixture.client_id, UNHELD_SECRET).await,
            "case {case:?}: the listing stores no secret of its own"
        );

        // "The response does not include pagination tokens ... we return both with every
        // ListUserPoolClientSecrets call", so a presented token narrows nothing.
        let paged = ok(
            &router,
            case,
            LIST,
            &list_body(&fixture, Some(PRESENTED_TOKEN)),
        )
        .await;
        assert_eq!(
            listed_ids(case, &fixture, &paged),
            after,
            "case {case:?}: a presented NextToken still reports the whole set"
        );
    }
}

/// Which part of a listing's request names nothing the state holds.
#[derive(Debug, Clone, Copy)]
enum Absent {
    /// A `UserPoolId` no create minted.
    UserPool,
    /// A `ClientId` no create minted.
    AppClient,
    /// A `ClientId` that names an app client of ANOTHER user pool, which the named pool
    /// holds no app client for.
    AppClientOfAnotherPool,
}

#[tokio::test]
async fn a_listing_for_a_resource_no_pool_holds_is_refused() {
    for (at, absent) in [
        Absent::UserPool,
        Absent::AppClient,
        Absent::AppClientOfAnotherPool,
    ]
    .iter()
    .enumerate()
    {
        let router = router();
        let case = format!("{absent:?} named by the listing");
        let named = fixture(
            &router,
            &case,
            &format!("not-found-pool-{at}"),
            CreatedWith::Generated,
        )
        .await;
        let elsewhere = fixture(
            &router,
            &case,
            &format!("other-pool-{at}"),
            CreatedWith::Generated,
        )
        .await;

        let (user_pool_id, client_id, expected) = match absent {
            // This operation resolves ONE resource, the app client of the pool it
            // names, so a pool nothing minted fails that lookup rather than being
            // reported as a missing resource of its own: the sentence names both
            // ids the request carried.
            Absent::UserPool => (
                UNKNOWN_POOL_ID.to_string(),
                named.client_id.clone(),
                format!(
                    "User pool client {} does not exist in user pool {}.",
                    named.client_id, UNKNOWN_POOL_ID
                ),
            ),
            Absent::AppClient => (
                named.user_pool_id.clone(),
                UNKNOWN_CLIENT_ID.to_string(),
                format!("User pool client {UNKNOWN_CLIENT_ID} does not exist."),
            ),
            Absent::AppClientOfAnotherPool => (
                named.user_pool_id.clone(),
                elsewhere.client_id.clone(),
                format!(
                    "User pool client {} does not exist in user pool {}.",
                    elsewhere.client_id, named.user_pool_id
                ),
            ),
        };
        let body = json!({ "UserPoolId": user_pool_id, "ClientId": client_id });
        let payload = refused(&router, &case, LIST, &body, 400, "ResourceNotFoundException").await;
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&payload)
        );

        // The refusal changed nothing: each app client still lists just the secret its
        // create stored.
        for client in [&named, &elsewhere] {
            let listing = ok(&router, &case, LIST, &list_body(client, None)).await;
            assert_eq!(
                listed_ids(&case, client, &listing).len(),
                client.held.len(),
                "case {case:?}: a refused listing leaves an app client's secrets alone, got {listing}"
            );
        }
    }
}
