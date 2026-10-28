// SPDX-License-Identifier: CC0-1.0

//! The post-confirmation Lambda trigger of a user pool: the event a confirming operation
//! hands the function the pool's `LambdaConfig` names.
//!
//! The rule under test: when an operation completes a confirmation of a user in a pool
//! whose `LambdaConfig` names a `PostConfirmation` function, that function is invoked with
//! the trigger event for the confirming operation — `triggerSource` naming the trigger and
//! the operation that reached it, the pool's own id and Region, the confirmed user's
//! username, the caller's app client (or the administrative placeholder for an operation
//! that names none), the `ClientMetadata` the request assigned, and
//! `request.userAttributes` holding the attributes the pool stores for that user. A pool
//! that configures no `PostConfirmation` function invokes nothing.
//!
//! Each row of the table spans what the rule generalises over: which confirming operation
//! fired it, whether an app client is named, whether client metadata was assigned, the
//! pool's Region, and which attributes the user signed up with. Nothing here asserts a
//! value baked into the service: the pool id, the username, the `sub` and the email
//! address each come back from the state the setup calls created.
//!
//! The second suite is the operation half over the wire: a confirmation whose configured
//! trigger function cannot be reached is still a confirmation — the trigger "can take
//! custom actions after a user confirms their user account", so it runs once the
//! confirmation is settled, and no verdict is invented for a function that answered none.
//! (The invocation reaching Lambda, and the refusal a function Lambda does not hold earns,
//! are pinned across the two services in
//! `core/router/tests/all/cognito_post_confirmation_lambda_trigger.rs`. The event a
//! function actually RECEIVES is not observable from outside while Lambda cannot execute a
//! deployment package, so the table below pins it through the dispatch each confirming
//! operation builds.)

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;
use cognito_idp::state::lambda_triggers::{self, Caller, Confirmation};

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// `callerContext.clientId` of an event fired by an operation that names no app client.
const CLIENT_ID_NOT_APPLICABLE: &str = "CLIENT_ID_NOT_APPLICABLE";

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`. No Lambda is mounted, so
/// nothing here can reach a trigger function: what these suites assert is the event a
/// trigger is handed and the answer a confirmation gives.
fn router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(false);
    router
}

/// Dispatch `operation` with `body`, signed for `region` when one is given and unsigned
/// otherwise, and return the status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    region: Option<&str>,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match region {
        Some(region) => format!("{SIGNING_NAME}.{region}.amazonaws.com"),
        None => EMULATOR_HOST.to_string(),
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if let Some(region) = region {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed for `region`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, region: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Some(region), operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call `operation` unsigned, requiring success, and return its response body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, None, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Create a pool in `region` with `extra` merged into its configuration, and return its
/// minted id.
async fn pool(
    router: &MultiServiceRouter,
    region: &str,
    pool_name: &str,
    extra: Value,
) -> String {
    let mut body = json!({ "PoolName": pool_name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, region, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

/// Create an app client of `user_pool_id` and return its minted id.
async fn app_client(
    router: &MultiServiceRouter,
    region: &str,
    user_pool_id: &str,
    client_name: &str,
) -> String {
    let created = ok(
        router,
        region,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": client_name }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

/// Register `username` through `client_id` with `attributes`, leaving the user where a
/// sign-up leaves it: unconfirmed, with a code outstanding.
async fn sign_up(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    attributes: &[(&str, String)],
) {
    let listed: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    ok_anonymous(
        router,
        "SignUp",
        &json!({
            "ClientId": client_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "UserAttributes": listed,
        }),
    )
    .await;
}

/// Confirm `username`'s sign-up as an administrator.
async fn admin_confirm(
    router: &MultiServiceRouter,
    region: &str,
    user_pool_id: &str,
    username: &str,
) {
    ok(
        router,
        region,
        "AdminConfirmSignUp",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
}

/// The event the pool's post-confirmation trigger would be handed for `username`, read
/// from the state the setup calls created, or `None` when the pool configures no such
/// trigger.
fn trigger_event(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    confirmation: Confirmation,
    client_id: Option<&str>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Option<Value> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    let pool = guard.pool(user_pool_id).expect("the created pool is stored");
    let user = pool.user(username).expect("the signed-up user is stored");
    lambda_triggers::post_confirmation(
        pool,
        user,
        confirmation,
        Caller {
            client_id,
            aws_sdk_version: None,
        },
        client_metadata,
    )
    .map(|dispatch| dispatch.event().clone())
}

/// The value of `member` of `event`, or `"<absent>"`.
fn text(event: &Value, member: &str) -> String {
    event
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// The attribute `name` of an event's `request.userAttributes`, or `"<absent>"`.
fn attribute(event: &Value, name: &str) -> String {
    event["request"]["userAttributes"]
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// A Lambda function ARN in `region`, inside `ArnType`'s pattern and length.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:000000000000:function:{name}")
}

/// Which operation confirmed the user in a row, which decides both the `triggerSource`
/// and whether an app client is named at all.
#[derive(Clone, Copy, Debug)]
enum FiredBy {
    /// `ConfirmSignUp`, which names the app client the user signed up through.
    ClientConfirmSignUp,
    /// `AdminConfirmSignUp`, authorized by IAM credentials and naming no app client.
    AdminConfirmSignUp,
    /// `ConfirmForgotPassword`, which names the app client the reset was asked through.
    ClientConfirmForgotPassword,
}

impl FiredBy {
    fn confirmation(self) -> Confirmation {
        match self {
            Self::ClientConfirmSignUp | Self::AdminConfirmSignUp => Confirmation::SignUp,
            Self::ClientConfirmForgotPassword => Confirmation::ForgotPassword,
        }
    }

    /// The `triggerSource` the event must carry for this operation.
    fn trigger_source(self) -> &'static str {
        match self {
            Self::ClientConfirmSignUp | Self::AdminConfirmSignUp => {
                "PostConfirmation_ConfirmSignUp"
            }
            Self::ClientConfirmForgotPassword => "PostConfirmation_ConfirmForgotPassword",
        }
    }

    /// Whether the operation names an app client in the event's caller context.
    fn names_a_client(self) -> bool {
        !matches!(self, Self::AdminConfirmSignUp)
    }
}

/// One row of the event table: what it is called, the Region its pool lives in, which
/// operation fired the trigger, the attributes the user carries beside its email address
/// and the `ClientMetadata` the confirming request assigned.
struct Row {
    case: &'static str,
    region: &'static str,
    fired_by: FiredBy,
    extra: &'static [(&'static str, &'static str)],
    metadata: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn the_post_confirmation_event_reports_the_pool_the_user_and_the_confirmation() {
    let rows: [Row; 5] = [
        Row {
            case: "a sign-up confirmed through the app client",
            region: "us-east-1",
            fired_by: FiredBy::ClientConfirmSignUp,
            extra: &[],
            metadata: &[],
        },
        Row {
            case: "a sign-up confirmed by an administrator",
            region: "us-east-1",
            fired_by: FiredBy::AdminConfirmSignUp,
            extra: &[],
            metadata: &[],
        },
        Row {
            case: "a reset confirmed through the app client",
            region: "eu-west-1",
            fired_by: FiredBy::ClientConfirmForgotPassword,
            extra: &[],
            metadata: &[],
        },
        Row {
            case: "a user carrying more attributes than the email",
            region: "ap-southeast-2",
            fired_by: FiredBy::ClientConfirmSignUp,
            extra: &[("given_name", "Ada"), ("family_name", "Lovelace")],
            metadata: &[],
        },
        Row {
            case: "a confirmation that assigned client metadata",
            region: "us-west-2",
            fired_by: FiredBy::AdminConfirmSignUp,
            extra: &[("given_name", "Grace")],
            metadata: &[("source", "onboarding"), ("tenant", "acme")],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            region,
            fired_by,
            extra,
            metadata,
        } = row;
        let engine = engine();
        let router = router(&engine);
        let trigger = function_arn(region, &format!("post-confirm-{index}"));
        let user_pool_id = pool(
            &router,
            region,
            &format!("triggered{index}"),
            json!({ "LambdaConfig": { "PostConfirmation": trigger } }),
        )
        .await;
        let client_id = app_client(&router, region, &user_pool_id, &format!("app{index}")).await;

        let username = format!("user{index}");
        let email = format!("{username}@example.test");
        let mut attributes: Vec<(&str, String)> = vec![("email", email.clone())];
        attributes.extend(extra.iter().map(|(name, value)| (*name, (*value).to_string())));
        sign_up(&router, &client_id, &username, &attributes).await;
        // Every row confirms the user, so the event is read from the state a COMPLETED
        // confirmation left behind, as each confirming operation builds it.
        admin_confirm(&router, region, &user_pool_id, &username).await;

        let client_metadata: HashMap<String, String> = metadata
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let event = trigger_event(
            &engine,
            &user_pool_id,
            &username,
            fired_by.confirmation(),
            fired_by.names_a_client().then_some(client_id.as_str()),
            (!client_metadata.is_empty()).then_some(&client_metadata),
        )
        .unwrap_or_else(|| {
            panic!("case {case:?}: a pool configuring a PostConfirmation function owes an event")
        });

        assert_eq!(
            text(&event, "triggerSource"),
            fired_by.trigger_source(),
            "case {case:?}: expected the trigger source of {fired_by:?}, got {event}"
        );
        assert_eq!(
            text(&event, "version"),
            "1",
            "case {case:?}: expected the documented event version, got {event}"
        );
        assert_eq!(
            text(&event, "userPoolId"),
            user_pool_id,
            "case {case:?}: expected the created pool's id, got {event}"
        );
        assert_eq!(
            text(&event, "region"),
            region,
            "case {case:?}: expected the pool's own Region, got {event}"
        );
        assert_eq!(
            text(&event, "userName"),
            username,
            "case {case:?}: expected the confirmed user's username, got {event}"
        );
        assert_eq!(
            attribute(&event, "email"),
            email,
            "case {case:?}: expected the email the sign-up supplied, got {event}"
        );
        for (name, value) in extra {
            assert_eq!(
                attribute(&event, name),
                *value,
                "case {case:?}: expected the stored {name} attribute, got {event}"
            );
        }
        // `sub` is the user's own generated id, and the status is the one the pool holds
        // for it after the confirmation: both are read back, never assumed.
        let sub = attribute(&event, "sub");
        assert!(
            !sub.is_empty() && sub != "<absent>",
            "case {case:?}: expected the stored sub among the user attributes, got {event}"
        );
        assert_eq!(
            attribute(&event, "cognito:user_status"),
            "CONFIRMED",
            "case {case:?}: expected the confirmed user's status, got {event}"
        );
        let expected_client = if fired_by.names_a_client() {
            client_id.clone()
        } else {
            CLIENT_ID_NOT_APPLICABLE.to_string()
        };
        assert_eq!(
            text(&event["callerContext"], "clientId"),
            expected_client,
            "case {case:?}: expected the caller's client id, got {event}"
        );
        let carried = event["request"].get("clientMetadata").cloned();
        if client_metadata.is_empty() {
            assert!(
                carried.is_none(),
                "case {case:?}: a request that assigned no ClientMetadata carries none, got \
                 {event}"
            );
        } else {
            let expected: Value = json!(client_metadata
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect::<serde_json::Map<String, Value>>());
            assert_eq!(
                carried,
                Some(expected),
                "case {case:?}: expected the assigned ClientMetadata, got {event}"
            );
        }
        assert_eq!(
            event["response"],
            json!({}),
            "case {case:?}: a post-confirmation function is handed nothing to amend, got {event}"
        );
    }
}

#[tokio::test]
async fn a_pool_that_configures_no_post_confirmation_function_invokes_nothing() {
    // (case, the pool's LambdaConfig)
    let rows: [(&str, Value); 3] = [
        ("a pool with no LambdaConfig at all", json!({})),
        (
            "a pool whose LambdaConfig configures another trigger",
            json!({
                "LambdaConfig": {
                    "PreSignUp": function_arn("us-east-1", "pre-sign-up"),
                }
            }),
        ),
        (
            "a pool whose LambdaConfig configures two other triggers",
            json!({
                "LambdaConfig": {
                    "PreAuthentication": function_arn("us-east-1", "pre-auth"),
                    "PostAuthentication": function_arn("us-east-1", "post-auth"),
                }
            }),
        ),
    ];

    for (index, (case, lambda_config)) in rows.into_iter().enumerate() {
        let engine = engine();
        let router = router(&engine);
        let user_pool_id =
            pool(&router, "us-east-1", &format!("untriggered{index}"), lambda_config).await;
        let client_id =
            app_client(&router, "us-east-1", &user_pool_id, &format!("app{index}")).await;
        let username = format!("user{index}");
        sign_up(
            &router,
            &client_id,
            &username,
            &[("email", format!("{username}@example.test"))],
        )
        .await;
        admin_confirm(&router, "us-east-1", &user_pool_id, &username).await;

        for confirmation in [Confirmation::SignUp, Confirmation::ForgotPassword] {
            let event = trigger_event(
                &engine,
                &user_pool_id,
                &username,
                confirmation,
                Some(&client_id),
                None,
            );
            assert!(
                event.is_none(),
                "case {case:?} ({confirmation:?}): expected no invocation, got {event:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_confirmation_stands_when_its_trigger_function_cannot_be_reached() {
    // No Lambda is mounted on this router, so the configured function answers nothing.
    // The trigger "can take custom actions AFTER a user confirms their user account", so
    // the confirmation it follows is already settled and stands — and no verdict is
    // invented for a function that gave none.
    //
    // (case, the pool's LambdaConfig)
    let rows: [(&str, Value); 2] = [
        (
            "a pool configuring a post-confirmation trigger",
            json!({
                "LambdaConfig": {
                    "PostConfirmation": function_arn("us-east-1", "unreachable"),
                }
            }),
        ),
        ("a pool configuring none", json!({})),
    ];

    for (index, (case, lambda_config)) in rows.into_iter().enumerate() {
        let engine = engine();
        let router = router(&engine);
        let user_pool_id =
            pool(&router, "us-east-1", &format!("unreached{index}"), lambda_config).await;
        let client_id =
            app_client(&router, "us-east-1", &user_pool_id, &format!("app{index}")).await;
        let username = format!("user{index}");
        sign_up(
            &router,
            &client_id,
            &username,
            &[("email", format!("{username}@example.test"))],
        )
        .await;

        let (status, answer) = dispatch(
            &router,
            Some("us-east-1"),
            "AdminConfirmSignUp",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the confirmation to stand, got {status} with {answer}"
        );

        // The round trip: the user reads back CONFIRMED through an implemented reader.
        let read_back = ok(
            &router,
            "us-east-1",
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            text(&read_back, "UserStatus"),
            "CONFIRMED",
            "case {case:?}: expected the confirmed user to read back CONFIRMED, got {read_back}"
        );
    }
}
