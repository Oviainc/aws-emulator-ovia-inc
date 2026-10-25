// SPDX-License-Identifier: CC0-1.0

//! Deleting a user pool app client over the wire, driven through the router
//! exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `DeleteUserPoolClient` removes the app client the NAMED
//! pool holds under the given `ClientId`, whatever that client was configured
//! with, so afterwards no read of the pool reports it and no sign-in names it
//! ("Deletes a user pool app client. After you delete an app client, users can
//! no longer sign in to the associated application", this operation's
//! documentation in the service model); everything else the pool holds — its
//! other app clients, their terms documents, its users — is untouched, and a
//! request naming a pool that does not exist, or a client that pool does not
//! hold, deletes nothing and is refused as a missing resource.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password that satisfies a new pool's default password policy.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The two values `CreateTerms.TermsName` admits
/// (`^(terms-of-use|privacy-policy)$`, from
/// `smithy_query(query="operation",operation="CreateTerms")`).
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

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

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client from `request` and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateUserPoolClient", request).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Attach the terms document `terms_name` to `client_id` and return its minted
/// `TermsId`.
async fn terms(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    terms_name: &str,
) -> String {
    let created = ok(
        router,
        "CreateTerms",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": terms_name,
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": { "cognito:default": "https://example.com/terms" },
        }),
    )
    .await;
    created["Terms"]["TermsId"]
        .as_str()
        .expect("a minted terms id")
        .to_string()
}

/// Create `username` in `user_pool_id` with a PERMANENT password, so the user is
/// `CONFIRMED` and can sign in.
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

/// A `USER_PASSWORD_AUTH` sign-in request for `client_id`.
fn sign_in_request(client_id: &str, username: &str) -> Value {
    json!({
        "ClientId": client_id,
        "AuthFlow": "USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
    })
}

/// One row of the delete table: the configuration the deleted app client was
/// created with, and the terms documents it carries when the delete arrives.
struct DeleteCase {
    case: &'static str,
    client_name: String,
    /// Members beyond `ClientName` the create request carries, so a row can
    /// delete a client configured very differently from the next row's.
    extra: Vec<(&'static str, Value)>,
    /// The terms documents attached to this row's client before the delete.
    terms: &'static [&'static str],
}

impl DeleteCase {
    /// This row's `CreateUserPoolClient` request for `user_pool_id`.
    fn create_request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ClientName".to_string(), json!(self.client_name));
        for (member, value) in &self.extra {
            body.insert((*member).to_string(), value.clone());
        }
        Value::Object(body)
    }
}

/// The table. The rows differ along what the rule generalises over: whether the
/// client holds a secret, which authentication flows it allows, how much of its
/// optional configuration is set, the length of its name across both ends of
/// `ClientNameType`'s modelled length 1..128, and how many terms documents hang
/// off it (none, one, and both names `TermsNameType` admits). Members and
/// constraints from
/// `smithy_query(query="operation",operation="CreateUserPoolClient,CreateTerms,DeleteUserPoolClient")`.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a client created with nothing but the required name",
            client_name: "plain-client".to_string(),
            extra: Vec::new(),
            terms: &[],
        },
        DeleteCase {
            case: "a client holding a generated secret",
            client_name: "secret-client".to_string(),
            extra: vec![("GenerateSecret", json!(true))],
            terms: &[],
        },
        DeleteCase {
            case: "a client with the OAuth and token-validity configuration set",
            client_name: "configured-client".to_string(),
            extra: vec![
                ("RefreshTokenValidity", json!(10)),
                ("AccessTokenValidity", json!(5)),
                ("IdTokenValidity", json!(6)),
                (
                    "TokenValidityUnits",
                    json!({ "AccessToken": "minutes", "IdToken": "minutes", "RefreshToken": "days" }),
                ),
                ("CallbackURLs", json!(["https://example.com/callback"])),
                ("LogoutURLs", json!(["https://example.com/logout"])),
                ("AllowedOAuthFlows", json!(["code"])),
                ("AllowedOAuthScopes", json!(["openid"])),
                ("AllowedOAuthFlowsUserPoolClient", json!(true)),
                ("SupportedIdentityProviders", json!(["COGNITO"])),
                ("EnableTokenRevocation", json!(false)),
            ],
            terms: &["terms-of-use"],
        },
        DeleteCase {
            case: "a client that allows password sign-in and carries both terms documents",
            client_name: "sign-in-client".to_string(),
            extra: vec![(
                "ExplicitAuthFlows",
                json!(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]),
            )],
            terms: &TERMS_NAMES,
        },
        DeleteCase {
            case: "a client whose name is one character, the length minimum",
            client_name: "c".to_string(),
            extra: Vec::new(),
            terms: &["privacy-policy"],
        },
        DeleteCase {
            case: "a client whose name is 128 characters, the length maximum",
            client_name: "n".repeat(128),
            extra: vec![("GenerateSecret", json!(true))],
            terms: &[],
        },
    ]
}

#[tokio::test]
async fn a_deleted_app_client_is_gone_from_the_pool_and_from_every_read() {
    let router = router();

    for (index, case) in delete_cases().into_iter().enumerate() {
        let label = case.case;
        let user_pool_id = pool(&router, &format!("delete-client-pool-{index}")).await;
        let client_id = client(&router, &case.create_request(&user_pool_id)).await;
        // A second client of the same pool, so each row also measures what the
        // delete must NOT touch.
        let bystander_id = client(
            &router,
            &json!({ "UserPoolId": user_pool_id, "ClientName": "bystander" }),
        )
        .await;
        let bystander_terms = terms(&router, &user_pool_id, &bystander_id, "terms-of-use").await;
        let mut case_terms = Vec::new();
        for terms_name in case.terms {
            case_terms.push(terms(&router, &user_pool_id, &client_id, terms_name).await);
        }

        // The client really was stored before the delete, so what follows
        // measures the delete and not a create that never took.
        ok(
            &router,
            "DescribeUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await;

        let deleted = ok(
            &router,
            "DeleteUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {label:?}: DeleteUserPoolClient answers an empty body, got {deleted}"
        );

        // No read of the pool reports the deleted client any more.
        let (status, payload) = call(
            &router,
            "DescribeUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a deleted client is no longer readable, got {status} with {payload}"
        );

        // The pool's other client, and its terms document, are untouched.
        // (`ListUserPoolClients` is still a scaffold stub with its own issue, so
        // the per-client reader is what this suite reads the pool back through.)
        let bystander = ok(
            &router,
            "DescribeUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": bystander_id }),
        )
        .await;
        assert_eq!(
            bystander["UserPoolClient"]["ClientName"],
            json!("bystander"),
            "case {label:?}: the pool's other client survives the delete, got {bystander}"
        );
        let survivor = ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": bystander_terms }),
        )
        .await;
        assert_eq!(
            survivor["Terms"]["ClientId"],
            json!(bystander_id),
            "case {label:?}: another client's terms document survives, got {survivor}"
        );

        // The deleted client's terms documents went with the client they name.
        for terms_id in &case_terms {
            let (status, payload) = call(
                &router,
                "DescribeTerms",
                &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (404, "ResourceNotFoundException"),
                "case {label:?}: the deleted client's terms document {terms_id} is gone, \
                 got {status} with {payload}"
            );
        }

        // The pool itself is still there, and a client created after the delete
        // takes the place of the deleted one.
        let pool_read = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            pool_read["UserPool"]["Id"],
            json!(user_pool_id),
            "case {label:?}: deleting a client leaves its pool in place, got {pool_read}"
        );
        let replacement = client(&router, &case.create_request(&user_pool_id)).await;
        assert_ne!(
            replacement, client_id,
            "case {label:?}: a client created after the delete gets its own id, got {replacement}"
        );

        // A delete is not idempotent: the second one names a client the pool no
        // longer holds.
        let (status, payload) = call(
            &router,
            "DeleteUserPoolClient",
            &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: deleting a client twice is refused the second time, \
             got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_deleted_app_client_can_no_longer_sign_a_user_in() {
    let router = router();
    let user_pool_id = pool(&router, "sign-in-delete-pool").await;
    let flows = json!(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"]);
    let deleted_id = client(
        &router,
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "to-delete",
            "ExplicitAuthFlows": flows,
        }),
    )
    .await;
    let kept_id = client(
        &router,
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "to-keep",
            "ExplicitAuthFlows": flows,
        }),
    )
    .await;
    confirmed_user(&router, &user_pool_id, "alice").await;

    // Both clients sign the stored user in before the delete.
    for (label, client_id) in [("the client to delete", &deleted_id), ("the kept client", &kept_id)]
    {
        let signed_in = ok(&router, "InitiateAuth", &sign_in_request(client_id, "alice")).await;
        assert!(
            signed_in["AuthenticationResult"]["AccessToken"].is_string(),
            "case {label:?}: the sign-in before the delete must issue tokens, got {signed_in}"
        );
    }

    ok(
        &router,
        "DeleteUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": deleted_id }),
    )
    .await;

    // "After you delete an app client, users can no longer sign in to the
    // associated application": the deleted `ClientId` names nothing.
    let (status, payload) = call(
        &router,
        "InitiateAuth",
        &sign_in_request(&deleted_id, "alice"),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (404, "ResourceNotFoundException"),
        "a sign-in naming a deleted app client is refused, got {status} with {payload}"
    );

    // The pool's other client still signs the same user in, and the user itself
    // survived the delete: a client is not its pool's users.
    let signed_in = ok(&router, "InitiateAuth", &sign_in_request(&kept_id, "alice")).await;
    assert!(
        signed_in["AuthenticationResult"]["AccessToken"].is_string(),
        "the surviving client still signs the user in, got {signed_in}"
    );
    let user = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    assert_eq!(
        user["Username"],
        json!("alice"),
        "deleting an app client leaves the pool's users in place, got {user}"
    );
}

#[tokio::test]
async fn a_delete_that_names_no_stored_app_client_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-client-pool").await;
    let other_pool_id = pool(&router, "refusal-other-pool").await;
    let client_id = client(
        &router,
        &json!({ "UserPoolId": user_pool_id, "ClientName": "held-client" }),
    )
    .await;

    // (case, the request, the refusal message it owes)
    for (label, body, expected) in [
        (
            "a client on a user pool that does not exist",
            json!({ "UserPoolId": "us-east-1_absent", "ClientId": client_id }),
            "User pool us-east-1_absent does not exist.".to_string(),
        ),
        (
            "a client id the named pool never held",
            json!({ "UserPoolId": user_pool_id, "ClientId": "neverminted1234" }),
            "User pool client neverminted1234 does not exist.".to_string(),
        ),
        (
            "a client id another pool holds",
            json!({ "UserPoolId": other_pool_id, "ClientId": client_id }),
            format!("User pool client {client_id} does not exist."),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteUserPoolClient", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: expected a 404 ResourceNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Every refusal left the stored client where it was.
    let read = ok(
        &router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    assert_eq!(
        read["UserPoolClient"]["ClientName"],
        json!("held-client"),
        "a refused delete must not remove the stored client, got {read}"
    );
}

#[tokio::test]
async fn a_delete_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-client-pool").await;

    // One value past each published bound of `DeleteUserPoolClient`'s input
    // (`smithy_query(query="operation",operation="DeleteUserPoolClient")`): the
    // client id's length 1..128 and its pattern `^[\w+]+$`, and the user pool
    // id's length 1..55 and its pattern `^[\w-]+_[0-9a-zA-Z]+$`.
    for (label, body) in [
        (
            "an empty client id, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "ClientId": "" }),
        ),
        (
            "a client id of 129 characters, past the length maximum 128",
            json!({ "UserPoolId": user_pool_id, "ClientId": "c".repeat(129) }),
        ),
        (
            "a client id carrying a hyphen, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "ClientId": "client-id" }),
        ),
        (
            "a user pool id without the underscore the pattern requires",
            json!({ "UserPoolId": "nounderscore", "ClientId": "someclientid" }),
        ),
        (
            "an empty user pool id, below the length minimum 1",
            json!({ "UserPoolId": "", "ClientId": "someclientid" }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteUserPoolClient", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, \
             got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, \
             got {payload}"
        );
    }
}
