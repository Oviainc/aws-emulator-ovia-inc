// SPDX-License-Identifier: CC0-1.0

//! The user-pool Lambda triggers a SIGN-IN reaches: the pre-authentication trigger of the
//! sign-in itself, and the pre-token-generation trigger of the tokens it mints.
//!
//! The rule under test: a trigger a pool's `LambdaConfig` configures for a stage is
//! invoked, synchronously, by the operation that reaches that stage — "Amazon Cognito
//! invokes triggers at several possible stages of user pool operations. Triggers can modify
//! the outcome of the operations that invoked them" (`LambdaConfigType`) — with the event of
//! that stage: `triggerSource` naming the trigger and the stage that reached it, the event
//! `version` the configured trigger version asks for, the pool's own id and Region, the
//! user, the caller's app client (or the administrative placeholder for an operation that
//! names none), the `ClientMetadata` the request assigned, and the stage's own request
//! members. Both of these triggers sit BEFORE the outcome they can modify — a pre
//! authentication trigger "can evaluate and modify user sign-in events"
//! (`LambdaConfigType$PreAuthentication`) — so the operation continues only on a function
//! that RETURNED: a trigger whose function cannot be reached refuses the operation with its
//! modelled `UnexpectedLambdaException` and mints no token, a function that does not answer
//! inside Amazon Cognito's trigger budget is refused the same way rather than waited out,
//! and a pool that configures neither trigger invokes nothing and signs in.
//!
//! The first suite is the event each stage builds, one row per thing the rule generalises
//! over: which stage fired it, which trigger version the pool configured, whether an app
//! client is named, the `ClientMetadata` assigned, the pool's Region and the groups the
//! user belongs to. Nothing it asserts is a value baked into the service: the pool id, the
//! username, the `sub` and the role ARNs each come back from the state the setup calls
//! created.
//!
//! The second is the operation half over the wire, across every operation of this service
//! that mints a sign-in's tokens. (No Lambda is mounted on these routers, so the configured
//! function answers nothing — which is exactly the "reached no function" leg of the rule.
//! That the invocation is really issued to Lambda, and the refusal a function Lambda does
//! not hold earns, are pinned across the two services in
//! `core/router/tests/all/cognito_post_confirmation_lambda_trigger.rs` for the trigger that
//! shares this dispatch.)
//!
//! The last is the budget: a stand-in that CLAIMS the invocation and answers it only after
//! a run far longer than the budget, so the sign-in's refusal can be timed against it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, MultiServiceRouter, OperationContext,
    Service,
};

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;
use cognito_idp::state::lambda_triggers::{self, Caller, TokenGeneration};

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The credential scope of a request addressed to AWS Lambda, which is what the trigger
/// invocation this service issues carries.
const LAMBDA_SCOPE: &str = "/lambda/aws4_request";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A second password satisfying the same policy, for the challenge a temporary one leaves.
const CHOSEN_PASSWORD: &str = "Ch0sen!Passw0rd";

/// `callerContext.clientId` of an event fired by an operation that names no app client.
const CLIENT_ID_NOT_APPLICABLE: &str = "CLIENT_ID_NOT_APPLICABLE";

/// The auth flows every app client below admits, so one client serves every row.
const SIGN_IN_FLOWS: [&str; 3] = [
    "ALLOW_USER_PASSWORD_AUTH",
    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
    "ALLOW_REFRESH_TOKEN_AUTH",
];

/// How long the stand-in function of the budget suite "runs": far longer than Amazon
/// Cognito's trigger budget, so a sign-in that waited for its answer would be held well
/// past that budget and the refusal can be timed against it.
const SLOW_FUNCTION_RUN: Duration = Duration::from_secs(20);

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
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

/// The wire error code of an error body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a body.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A Lambda function ARN in `region`, inside `ArnType`'s pattern and length.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:000000000000:function:{name}")
}

/// An IAM role ARN a group can grant, inside `ArnType`'s pattern and length.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// Create a pool in `region` with `extra` merged into its configuration, and return its
/// minted id.
async fn pool(router: &MultiServiceRouter, region: &str, pool_name: &str, extra: Value) -> String {
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

/// Create an app client of `user_pool_id` admitting [`SIGN_IN_FLOWS`], and return its
/// minted id.
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
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": client_name,
            "ExplicitAuthFlows": SIGN_IN_FLOWS,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

/// Create `username` with an email address and a PERMANENT password, so the user is
/// `CONFIRMED` and can sign in.
async fn confirmed_user(
    router: &MultiServiceRouter,
    region: &str,
    user_pool_id: &str,
    username: &str,
) {
    ok(
        router,
        region,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": [{ "Name": "email", "Value": format!("{username}@example.test") }],
        }),
    )
    .await;
    ok(
        router,
        region,
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

/// Put `username` in a group named `group_name` of `user_pool_id`, created with
/// `precedence` and granting `role` when one is given.
async fn group_membership(
    router: &MultiServiceRouter,
    region: &str,
    user_pool_id: &str,
    username: &str,
    group_name: &str,
    precedence: i32,
    role: Option<String>,
) {
    let mut body = json!({
        "UserPoolId": user_pool_id,
        "GroupName": group_name,
        "Precedence": precedence,
    });
    if let Some(role) = role {
        body["RoleArn"] = json!(role);
    }
    ok(router, region, "CreateGroup", &body).await;
    ok(
        router,
        region,
        "AdminAddUserToGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "GroupName": group_name,
        }),
    )
    .await;
}

/// An `InitiateAuth` body for the password flow.
fn password_sign_in(client_id: &str, username: &str, password: &str) -> Value {
    json!({
        "ClientId": client_id,
        "AuthFlow": "USER_PASSWORD_AUTH",
        "AuthParameters": { "USERNAME": username, "PASSWORD": password },
    })
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

/// Which stage of a sign-in a row's trigger sits at.
#[derive(Clone, Copy, Debug)]
enum Stage {
    /// The sign-in itself, which the `PreAuthentication` member is configured for.
    PreAuthentication,
    /// The tokens a completed sign-in mints, which the pre-token-generation members are
    /// configured for.
    TokenGeneration(TokenGeneration),
}

/// How a row's pool configures its pre-token-generation trigger: the legacy
/// `PreTokenGeneration` member alone, or `PreTokenGenerationConfig` at one of the trigger
/// versions `PreTokenGenerationLambdaVersionType` admits (`V1_0 | V2_0 | V3_0`, from
/// `smithy_query(query="shape", shape="PreTokenGenerationLambdaVersionType")`).
#[derive(Clone, Copy, Debug)]
enum Configured {
    Legacy,
    Versioned(&'static str),
}

impl Configured {
    /// The `LambdaConfig` members this configuration writes for the function `arn`.
    fn lambda_config(self, arn: &str) -> Value {
        match self {
            Self::Legacy => json!({ "PreTokenGeneration": arn }),
            Self::Versioned(version) => json!({
                "PreTokenGenerationConfig": { "LambdaVersion": version, "LambdaArn": arn },
            }),
        }
    }

    /// The event `version` this configuration asks for: the major number of its trigger
    /// version, and the first version for the legacy member that predates every later one.
    fn event_version(self) -> &'static str {
        match self {
            Self::Legacy | Self::Versioned("V1_0") => "1",
            Self::Versioned("V2_0") => "2",
            Self::Versioned("V3_0") => "3",
            Self::Versioned(other) => {
                panic!("the test table names only modelled trigger versions, got {other:?}")
            }
        }
    }

    /// Whether an event of this version carries the `scopes` the access token will hold —
    /// the member the second trigger version added.
    fn carries_scopes(self) -> bool {
        !matches!(self, Self::Legacy | Self::Versioned("V1_0"))
    }
}

/// One row of the event table.
struct Row {
    case: &'static str,
    region: &'static str,
    stage: Stage,
    /// The `triggerSource` the event must carry, as Amazon Cognito documents it for this
    /// stage.
    trigger_source: &'static str,
    /// How the pool configures the trigger, for a token-generation row.
    configured: Configured,
    /// Whether the firing operation names an app client at all.
    names_a_client: bool,
    /// The `ClientMetadata` the firing request assigned.
    metadata: &'static [(&'static str, &'static str)],
    /// The groups the user belongs to: name, precedence and the role each grants.
    groups: &'static [(&'static str, i32, Option<&'static str>)],
}

#[tokio::test]
async fn the_sign_in_trigger_events_report_the_pool_the_user_and_the_stage() {
    let rows: [Row; 6] = [
        Row {
            case: "a password sign-in reaching the pre-authentication trigger",
            region: "us-east-1",
            stage: Stage::PreAuthentication,
            trigger_source: "PreAuthentication_Authentication",
            configured: Configured::Legacy,
            names_a_client: true,
            metadata: &[],
            groups: &[],
        },
        Row {
            case: "an administrative sign-in reaching the pre-authentication trigger",
            region: "eu-west-1",
            stage: Stage::PreAuthentication,
            trigger_source: "PreAuthentication_Authentication",
            configured: Configured::Legacy,
            names_a_client: false,
            metadata: &[("source", "console")],
            groups: &[],
        },
        Row {
            case: "a completed authentication minting tokens, legacy trigger member",
            region: "us-east-1",
            stage: Stage::TokenGeneration(TokenGeneration::Authentication),
            trigger_source: "TokenGeneration_Authentication",
            configured: Configured::Legacy,
            names_a_client: true,
            metadata: &[],
            groups: &[("admins", 0, Some("admin-role"))],
        },
        Row {
            case: "a completed authentication minting tokens, second trigger version",
            region: "ap-southeast-2",
            stage: Stage::TokenGeneration(TokenGeneration::Authentication),
            trigger_source: "TokenGeneration_Authentication",
            configured: Configured::Versioned("V2_0"),
            names_a_client: true,
            metadata: &[("tenant", "acme"), ("source", "app")],
            groups: &[("alpha", 0, Some("alpha-role")), ("beta", 1, Some("beta-role"))],
        },
        Row {
            case: "a refresh minting tokens, third trigger version",
            region: "us-west-2",
            stage: Stage::TokenGeneration(TokenGeneration::RefreshTokens),
            trigger_source: "TokenGeneration_RefreshTokens",
            configured: Configured::Versioned("V3_0"),
            names_a_client: true,
            metadata: &[],
            groups: &[("readers", 7, None)],
        },
        Row {
            case: "a new-password challenge minting tokens, first trigger version",
            region: "eu-central-1",
            stage: Stage::TokenGeneration(TokenGeneration::NewPasswordChallenge),
            trigger_source: "TokenGeneration_NewPasswordChallenge",
            configured: Configured::Versioned("V1_0"),
            names_a_client: false,
            metadata: &[("flow", "admin-created")],
            groups: &[],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            region,
            stage,
            trigger_source,
            configured,
            names_a_client,
            metadata,
            groups,
        } = row;
        let engine = engine();
        let router = router(&engine);
        let trigger = function_arn(region, &format!("sign-in-trigger-{index}"));
        let lambda_config = match stage {
            Stage::PreAuthentication => json!({ "PreAuthentication": trigger }),
            Stage::TokenGeneration(_) => configured.lambda_config(&trigger),
        };
        let user_pool_id = pool(
            &router,
            region,
            &format!("triggered{index}"),
            json!({ "LambdaConfig": lambda_config }),
        )
        .await;
        let client_id = app_client(&router, region, &user_pool_id, &format!("app{index}")).await;
        let username = format!("user{index}");
        confirmed_user(&router, region, &user_pool_id, &username).await;
        for (group_name, precedence, role) in groups {
            group_membership(
                &router,
                region,
                &user_pool_id,
                &username,
                group_name,
                *precedence,
                role.map(role_arn),
            )
            .await;
        }

        let client_metadata: HashMap<String, String> = metadata
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let caller = Caller {
            client_id: names_a_client.then_some(client_id.as_str()),
            aws_sdk_version: None,
        };
        let assigned = (!client_metadata.is_empty()).then_some(&client_metadata);

        let service_state = state::service_state(&*engine).expect("this service's state");
        let guard = service_state.lock();
        let dispatch = match stage {
            Stage::PreAuthentication => {
                lambda_triggers::pre_authentication(&guard, &user_pool_id, &username, caller, assigned)
            }
            Stage::TokenGeneration(generation) => lambda_triggers::pre_token_generation(
                &guard,
                &user_pool_id,
                &username,
                generation,
                caller,
                assigned,
            ),
        };
        let dispatch = dispatch.unwrap_or_else(|| {
            panic!("case {case:?}: a pool configuring this trigger owes an invocation")
        });
        assert_eq!(
            dispatch.function_arn(),
            trigger,
            "case {case:?}: expected the invocation of the configured function"
        );
        let event = dispatch.event().clone();
        drop(guard);

        assert_eq!(
            text(&event, "triggerSource"),
            trigger_source,
            "case {case:?}: expected the documented trigger source, got {event}"
        );
        let expected_version = match stage {
            // Every trigger but the pre-token-generation one has the single documented
            // event version.
            Stage::PreAuthentication => "1",
            Stage::TokenGeneration(_) => configured.event_version(),
        };
        assert_eq!(
            text(&event, "version"),
            expected_version,
            "case {case:?}: expected the event version this configuration asks for, got {event}"
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
            "case {case:?}: expected the signing-in user's username, got {event}"
        );
        assert_eq!(
            attribute(&event, "email"),
            format!("{username}@example.test"),
            "case {case:?}: expected the stored email attribute, got {event}"
        );
        let sub = attribute(&event, "sub");
        assert!(
            !sub.is_empty() && sub != "<absent>",
            "case {case:?}: expected the stored sub among the user attributes, got {event}"
        );
        assert_eq!(
            attribute(&event, "cognito:user_status"),
            "CONFIRMED",
            "case {case:?}: expected the stored user status, got {event}"
        );
        let expected_client = if names_a_client {
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
            "case {case:?}: the response half is the function's to fill in, got {event}"
        );

        match stage {
            Stage::PreAuthentication => {
                // The stage resolved a stored user, and the metadata the request assigned
                // is what reaches this trigger as its validation data.
                assert_eq!(
                    event["request"]["userNotFound"],
                    json!(false),
                    "case {case:?}: expected the resolved user to be reported, got {event}"
                );
                let validation = event["request"].get("validationData").cloned();
                assert_eq!(
                    validation,
                    carried,
                    "case {case:?}: expected the assigned ClientMetadata as validation data, \
                     got {event}"
                );
                assert!(
                    event["request"].get("groupConfiguration").is_none(),
                    "case {case:?}: a sign-in event carries no group configuration, got {event}"
                );
            }
            Stage::TokenGeneration(_) => {
                // The group configuration is the claims the minted tokens would carry: the
                // user's groups in stored order, the distinct roles they grant with the
                // best precedence first, and the preferred role where it is unambiguous.
                let expected_groups: Vec<&str> = {
                    let mut names: Vec<&str> =
                        groups.iter().map(|(name, _, _)| *name).collect();
                    names.sort_unstable();
                    names
                };
                assert_eq!(
                    event["request"]["groupConfiguration"]["groupsToOverride"],
                    json!(expected_groups),
                    "case {case:?}: expected the user's stored groups, got {event}"
                );
                let mut by_precedence: Vec<(i32, &str)> = groups
                    .iter()
                    .filter_map(|(_, precedence, role)| role.map(|role| (*precedence, role)))
                    .collect();
                by_precedence.sort_by_key(|(precedence, _)| *precedence);
                let expected_roles: Vec<String> = by_precedence
                    .iter()
                    .map(|(_, role)| role_arn(role))
                    .collect();
                assert_eq!(
                    event["request"]["groupConfiguration"]["iamRolesToOverride"],
                    json!(expected_roles),
                    "case {case:?}: expected the roles the user's groups grant, got {event}"
                );
                let expected_preferred = by_precedence.first().map(|(_, role)| role_arn(role));
                assert_eq!(
                    event["request"]["groupConfiguration"]
                        .get("preferredRole")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    expected_preferred,
                    "case {case:?}: expected the best-precedence group's role, got {event}"
                );
                let scopes = event["request"].get("scopes").cloned();
                if configured.carries_scopes() {
                    assert_eq!(
                        scopes,
                        Some(json!([state::USER_ADMIN_SCOPE])),
                        "case {case:?}: expected the scopes the access token carries, got {event}"
                    );
                } else {
                    assert!(
                        scopes.is_none(),
                        "case {case:?}: the first trigger version has no scopes member, got \
                         {event}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_pool_that_configures_no_sign_in_trigger_invokes_nothing() {
    // (case, the pool's LambdaConfig)
    let rows: [(&str, Value); 3] = [
        ("a pool with no LambdaConfig at all", json!({})),
        (
            "a pool whose LambdaConfig configures another trigger",
            json!({
                "LambdaConfig": {
                    "PostConfirmation": function_arn("us-east-1", "post-confirm"),
                }
            }),
        ),
        (
            "a pool whose LambdaConfig configures two other triggers",
            json!({
                "LambdaConfig": {
                    "PreSignUp": function_arn("us-east-1", "pre-sign-up"),
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
        confirmed_user(&router, "us-east-1", &user_pool_id, &username).await;

        // The lock is taken and released inside this block, so nothing holds it across the
        // sign-in below.
        {
            let service_state = state::service_state(&*engine).expect("this service's state");
            let guard = service_state.lock();
            let caller = Caller {
                client_id: Some(client_id.as_str()),
                aws_sdk_version: None,
            };
            let pre_authentication =
                lambda_triggers::pre_authentication(&guard, &user_pool_id, &username, caller, None);
            assert!(
                pre_authentication.is_none(),
                "case {case:?}: expected no pre-authentication invocation, got {:?}",
                pre_authentication.map(|dispatch| dispatch.event().clone())
            );
            for generation in [
                TokenGeneration::Authentication,
                TokenGeneration::RefreshTokens,
                TokenGeneration::NewPasswordChallenge,
            ] {
                let dispatch = lambda_triggers::pre_token_generation(
                    &guard,
                    &user_pool_id,
                    &username,
                    generation,
                    caller,
                    None,
                );
                assert!(
                    dispatch.is_none(),
                    "case {case:?} ({generation:?}): expected no invocation, got {:?}",
                    dispatch.map(|dispatch| dispatch.event().clone())
                );
            }
        }

        // And the sign-in itself stands: a pool that configures neither trigger waits on
        // nothing, so the tokens are minted and read back through an implemented reader.
        let (status, payload) = dispatch(
            &router,
            None,
            "InitiateAuth",
            &password_sign_in(&client_id, &username, GOOD_PASSWORD),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the sign-in to stand, got {status} with {payload}"
        );
        let access_token = payload["AuthenticationResult"]["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an access token, got {payload}"))
            .to_string();
        let read_back = ok(
            &router,
            "us-east-1",
            "GetUser",
            &json!({ "AccessToken": access_token }),
        )
        .await;
        assert_eq!(
            text(&read_back, "Username"),
            username,
            "case {case:?}: expected the minted token to identify its user, got {read_back}"
        );
    }
}

/// A trigger function ARN in the Region every row of the gating tables uses, which no
/// Lambda mounted here holds — the "reached no function" leg of the rule.
fn unreachable_trigger(name: &str) -> String {
    function_arn("us-east-1", name)
}

#[tokio::test]
async fn a_sign_in_is_gated_by_the_trigger_the_pool_configures() {
    // (case, the pool's LambdaConfig, the operation, the trigger the refusal must name)
    //
    // Every row drives the same stored user and the same password; only what the pool's
    // LambdaConfig names differs, so the configured trigger is the only thing that can
    // decide the answer.
    let rows: [(&str, Value, &str, Option<&str>); 5] = [
        (
            "a password sign-in on a pool configuring a pre-authentication trigger",
            json!({ "PreAuthentication": unreachable_trigger("pre-auth") }),
            "InitiateAuth",
            Some("PreAuthentication"),
        ),
        (
            "a password sign-in on a pool configuring a legacy pre-token-generation trigger",
            json!({ "PreTokenGeneration": unreachable_trigger("pre-token") }),
            "InitiateAuth",
            Some("PreTokenGeneration"),
        ),
        (
            "a password sign-in on a pool configuring a versioned pre-token-generation trigger",
            json!({
                "PreTokenGenerationConfig": {
                    "LambdaVersion": "V2_0",
                    "LambdaArn": unreachable_trigger("pre-token-v2"),
                }
            }),
            "InitiateAuth",
            Some("PreTokenGeneration"),
        ),
        (
            "an administrative sign-in on a pool configuring a pre-authentication trigger",
            json!({ "PreAuthentication": unreachable_trigger("pre-auth-admin") }),
            "AdminInitiateAuth",
            Some("PreAuthentication"),
        ),
        (
            "a sign-in on a pool configuring neither trigger",
            json!({ "PostConfirmation": unreachable_trigger("post-confirm") }),
            "InitiateAuth",
            None,
        ),
    ];

    for (index, (case, lambda_config, operation, refused_by)) in rows.into_iter().enumerate() {
        let engine = engine();
        let router = router(&engine);
        let user_pool_id = pool(
            &router,
            "us-east-1",
            &format!("gated{index}"),
            json!({ "LambdaConfig": lambda_config }),
        )
        .await;
        let client_id =
            app_client(&router, "us-east-1", &user_pool_id, &format!("app{index}")).await;
        let username = format!("user{index}");
        confirmed_user(&router, "us-east-1", &user_pool_id, &username).await;

        let (region, body) = if operation == "AdminInitiateAuth" {
            (
                Some("us-east-1"),
                json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
                }),
            )
        } else {
            (None, password_sign_in(&client_id, &username, GOOD_PASSWORD))
        };
        let (status, payload) = dispatch(&router, region, operation, &body).await;

        match refused_by {
            Some(trigger) => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, "UnexpectedLambdaException"),
                    "case {case:?}: expected the trigger failure, got {status} with {payload}"
                );
                let stated = message(&payload);
                assert!(
                    stated.starts_with(&format!("{trigger} failed with error ")),
                    "case {case:?}: expected the refusal to name the {trigger} trigger, got \
                     {stated:?}"
                );
                assert!(
                    payload.get("AuthenticationResult").is_none(),
                    "case {case:?}: a gated sign-in mints no token, got {payload}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the sign-in to stand, got {status} with {payload}"
                );
                assert!(
                    payload["AuthenticationResult"]["AccessToken"].is_string(),
                    "case {case:?}: expected the minted tokens, got {payload}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_token_minting_operation_is_gated_by_the_pre_token_generation_trigger() {
    let engine = engine();
    let router = router(&engine);
    let region = "us-east-1";
    // The pool starts out configuring NO trigger, so each operation below is first driven
    // to the state it mints tokens from; the trigger is configured once that state exists.
    let user_pool_id = pool(&router, region, "every-generation", json!({})).await;
    let client_id = app_client(&router, region, &user_pool_id, "app").await;
    confirmed_user(&router, region, &user_pool_id, "refreshed").await;

    // A first sign-in, while the pool waits on nothing, hands back the refresh token the
    // two refreshing operations exchange.
    let signed_in = ok(
        &router,
        region,
        "InitiateAuth",
        &password_sign_in(&client_id, "refreshed", GOOD_PASSWORD),
    )
    .await;
    let refresh_token = signed_in["AuthenticationResult"]["RefreshToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first sign-in must mint a refresh token, got {signed_in}"))
        .to_string();

    // A user an administrator created with a temporary password owes a password change, so
    // its sign-in answers the `NEW_PASSWORD_REQUIRED` challenge whose response mints the
    // tokens of the third operation.
    ok(
        &router,
        region,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "challenged",
            "TemporaryPassword": GOOD_PASSWORD,
            "UserAttributes": [{ "Name": "email", "Value": "challenged@example.test" }],
        }),
    )
    .await;
    let challenged = ok(
        &router,
        region,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "challenged", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let session = challenged["Session"]
        .as_str()
        .unwrap_or_else(|| panic!("the challenge must carry a session, got {challenged}"))
        .to_string();

    // Now the pool configures the trigger: every operation that mints a sign-in's tokens
    // from here on has to get past it.
    let trigger = unreachable_trigger("pre-token-every");
    ok(
        &router,
        region,
        "UpdateUserPool",
        &json!({
            "UserPoolId": user_pool_id,
            "LambdaConfig": {
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": trigger },
            },
        }),
    )
    .await;

    // (case, the operation, whether it is signed, its body)
    let calls: [(&str, &str, Option<&str>, Value); 3] = [
        (
            "a refresh through InitiateAuth",
            "InitiateAuth",
            None,
            json!({
                "ClientId": client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
            }),
        ),
        (
            "a refresh through GetTokensFromRefreshToken",
            "GetTokensFromRefreshToken",
            None,
            json!({ "ClientId": client_id, "RefreshToken": refresh_token }),
        ),
        (
            "a new-password challenge through AdminRespondToAuthChallenge",
            "AdminRespondToAuthChallenge",
            Some(region),
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "ChallengeName": "NEW_PASSWORD_REQUIRED",
                "Session": session,
                "ChallengeResponses": {
                    "USERNAME": "challenged",
                    "NEW_PASSWORD": CHOSEN_PASSWORD,
                },
            }),
        ),
    ];

    for (case, operation, signed, body) in calls {
        let (status, payload) = dispatch(&router, signed, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "UnexpectedLambdaException"),
            "case {case:?}: expected the trigger failure, got {status} with {payload}"
        );
        let stated = message(&payload);
        assert!(
            stated.starts_with("PreTokenGeneration failed with error "),
            "case {case:?}: expected the refusal to name the PreTokenGeneration trigger, got \
             {stated:?}"
        );
        assert!(
            payload.get("AuthenticationResult").is_none(),
            "case {case:?}: a gated generation mints no token, got {payload}"
        );
    }
}

/// A stand-in for AWS Lambda that CLAIMS the trigger invocation this service issues and
/// answers it only after [`SLOW_FUNCTION_RUN`] — a function that runs far longer than
/// Amazon Cognito's trigger budget.
struct SlowLambda {
    /// The model-derived half of a `Service` is not an author seam, so this stand-in
    /// borrows this service's own. It is never used for dispatch: every request this
    /// stand-in claims is served by the operation below.
    inner: cognito_idp::EmulatedService<engine::InMemoryEngine>,
    invocation: SlowInvocation,
}

impl Service for SlowLambda {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        self.inner.definition()
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains(LAMBDA_SCOPE))
    }

    fn override_operation(&self, _req: &RtRequest) -> Option<&dyn ErasedOperation> {
        Some(&self.invocation)
    }
}

/// The invocation the stand-in above serves: it waits out [`SLOW_FUNCTION_RUN`] and only
/// then answers, so nothing it returns can reach a caller that stopped waiting.
struct SlowInvocation;

impl ErasedOperation for SlowInvocation {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async {
            tokio::time::sleep(SLOW_FUNCTION_RUN).await;
            RtResponse::try_from(
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from("{}"))
                    .expect("the stand-in answer is well formed"),
            )
            .expect("the stand-in answer is a runtime response")
        })
    }

    /// Served unsigned: the stand-in answers whatever reached it, so no auth scheme of the
    /// borrowed definition is resolved for it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

#[tokio::test]
async fn a_trigger_that_outlives_the_budget_fails_the_sign_in() {
    let engine = engine();
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    router.register_service(SlowLambda {
        inner: cognito_idp::EmulatedService::new(Arc::clone(&engine)),
        invocation: SlowInvocation,
    });

    let region = "us-east-1";
    let trigger = function_arn(region, "slow-trigger");
    let user_pool_id = pool(
        &router,
        region,
        "slow-trigger-pool",
        json!({
            "LambdaConfig": {
                "PreTokenGenerationConfig": { "LambdaVersion": "V2_0", "LambdaArn": trigger },
            }
        }),
    )
    .await;
    let client_id = app_client(&router, region, &user_pool_id, "app").await;
    confirmed_user(&router, region, &user_pool_id, "waiting").await;

    let started = Instant::now();
    let (status, payload) = dispatch(
        &router,
        None,
        "InitiateAuth",
        &password_sign_in(&client_id, "waiting", GOOD_PASSWORD),
    )
    .await;
    let waited = started.elapsed();

    assert_eq!(
        (status, error_code(&payload).as_str()),
        (400, "UnexpectedLambdaException"),
        "a function that does not answer inside the trigger budget fails the sign-in, got \
         {status} with {payload}"
    );
    assert_eq!(
        message(&payload),
        "PreTokenGeneration failed with error Socket timeout while invoking Lambda function.",
        "the refusal states the trigger and that the invocation was not answered, got {payload}"
    );
    assert!(
        payload.get("AuthenticationResult").is_none(),
        "a sign-in whose trigger timed out mints no token, got {payload}"
    );
    assert!(
        waited < SLOW_FUNCTION_RUN,
        "the sign-in is answered on the budget rather than on the function: waited \
         {waited:?} of the function's {SLOW_FUNCTION_RUN:?}"
    );
}
