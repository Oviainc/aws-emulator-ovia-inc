// SPDX-License-Identifier: CC0-1.0

//! What the AWS SDK a running handler calls another service through fills into a call's input
//! ITSELF: the members the called service's model marks as an idempotency token — end to end
//! over the wire, with Lambda and AWS Secrets Manager on ONE engine and a `nodejs*` function
//! whose code imports the bundled `@aws-sdk/client-secrets-manager`.
//!
//! The rule pinned here: an AWS SDK client generates a value for an input member its service's
//! model marks `@idempotencyToken` when the caller leaves it out, and sends it with the request
//! — "If you use the Amazon Web Services CLI or one of the Amazon Web Services SDKs to call this
//! operation, then you can leave this parameter empty. The CLI or SDK generates a random UUID
//! for you and includes it as the value for this parameter in the request"
//! (`PutSecretValueRequest$ClientRequestToken`, `smithy_query(query="shape",
//! shape="PutSecretValueRequest")`) — so the service never sees such a member absent from a call
//! an SDK made, and a value the caller DID state is sent as the caller wrote it. The fill is the
//! same for every SDK this runtime supplies, because it is the one call seam's and not any one
//! language's.
//!
//! The rows span what the rule generalises over: WHICH operation of the service is called (a
//! create that stakes the first version, a write that appends one, an update that appends one,
//! and two reads whose input carries no such member), whether the handler states the token or
//! leaves it out, and — across the calls that leave it out — that each gets a value of its own.
//! The versions those calls staked are read back through the service's OWN API afterwards, so a
//! token that was generated but never reached the service fails here.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's client is built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` the function carries: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The secret every row below addresses.
const SECRET: &str = "sdk-filled-token";

/// A token a handler generates ITSELF, within `ClientRequestTokenType`'s own length (32..64).
const OWN_TOKEN: &str = "a-token-this-handler-generated-itself";

/// Lambda and AWS Secrets Manager over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the secret it
/// writes is the secret this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Secrets Manager call (`awsJson1_1`) that must answer 200.
async fn secrets(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("authorization", authorization("secretsmanager"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Secrets Manager {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` as `index.mjs` — one ES module, and nothing else:
/// no `node_modules`, so the `@aws-sdk/*` specifier in it is the runtime's to resolve.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("index.mjs", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler this suite runs: it sends the commands the event lists, in order, and reports
/// what the SDK answered for each, so the invocation's payload IS what the guest read back.
///
/// Every row's call is made in the ONE run, because what the rule is about is the input the
/// client built, which does not depend on the run it was built in.
const HANDLER_SOURCE: &str = r#"
import {
    SecretsManagerClient,
    CreateSecretCommand,
    PutSecretValueCommand,
    UpdateSecretCommand,
    GetSecretValueCommand,
    DescribeSecretCommand,
} from "@aws-sdk/client-secrets-manager";

const COMMANDS = {
    CreateSecret: CreateSecretCommand,
    PutSecretValue: PutSecretValueCommand,
    UpdateSecret: UpdateSecretCommand,
    GetSecretValue: GetSecretValueCommand,
    DescribeSecret: DescribeSecretCommand,
};

export const handler = async (event) => {
    const client = new SecretsManagerClient({ region: event.region });
    const answers = [];
    for (const call of event.calls) {
        const Command = COMMANDS[call.operation];
        try {
            const answered = await client.send(new Command(call.input));
            answers.push({
                ok: true,
                versionId: answered.VersionId === undefined ? null : answered.VersionId,
            });
        } catch (error) {
            answers.push({ ok: false, name: error.name, message: error.message });
        }
    }
    return { answers };
};
"#;

/// One call the handler is asked to make.
struct Row {
    case: &'static str,
    /// The operation it addresses, which is the command class it sends.
    operation: &'static str,
    /// The command's input, which IS the operation's own input document.
    input: Value,
    /// The token the call must be recorded under: `Some` when the handler stated one itself,
    /// `None` when it left the member out and the SDK had to generate one.
    stated: Option<&'static str>,
    /// Whether the call stakes a version of the secret, which is what the answer names.
    stakes_version: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create that stakes the secret's first version, with no token stated",
            operation: "CreateSecret",
            input: json!({ "Name": SECRET, "SecretString": "first" }),
            stated: None,
            stakes_version: true,
        },
        Row {
            case: "a write that appends a version, with no token stated",
            operation: "PutSecretValue",
            input: json!({ "SecretId": SECRET, "SecretString": "second" }),
            stated: None,
            stakes_version: true,
        },
        Row {
            case: "another write with no token stated, which gets its own value",
            operation: "PutSecretValue",
            input: json!({ "SecretId": SECRET, "SecretString": "third" }),
            stated: None,
            stakes_version: true,
        },
        Row {
            case: "a write whose own token the handler generated",
            operation: "PutSecretValue",
            input: json!({
                "SecretId": SECRET,
                "SecretString": "fourth",
                "ClientRequestToken": OWN_TOKEN,
            }),
            stated: Some(OWN_TOKEN),
            stakes_version: true,
        },
        Row {
            case: "an update that appends a version, with no token stated",
            operation: "UpdateSecret",
            input: json!({ "SecretId": SECRET, "SecretString": "fifth" }),
            stated: None,
            stakes_version: true,
        },
        Row {
            case: "a read whose input carries no such member",
            operation: "GetSecretValue",
            input: json!({ "SecretId": SECRET }),
            stated: None,
            stakes_version: false,
        },
        Row {
            case: "a report whose input carries no such member",
            operation: "DescribeSecret",
            input: json!({ "SecretId": SECRET }),
            stated: None,
            stakes_version: false,
        },
    ]
}

/// RULE: the SDK a running handler calls a service through fills in the input members that
/// service's model marks as an idempotency token, so a handler that leaves one out is served —
/// and each call that leaves it out gets a value of its own, while a value the handler stated is
/// the one the service records.
#[tokio::test]
async fn the_sdk_fills_in_the_idempotency_token_a_handler_leaves_out() {
    let router = router();
    let name = "sdk-token-fill";
    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    let rows = rows();
    let calls: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "operation": row.operation, "input": row.input }))
        .collect();
    let (status, headers, payload) = lambda(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        json!({ "region": REGION, "calls": calls })
            .to_string()
            .as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok());
    assert!(
        signal.is_none(),
        "expected the run to carry no function error, got {signal:?} with {payload}"
    );
    let answers = payload["answers"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        answers.len(),
        rows.len(),
        "expected one answer per call, got {payload}"
    );

    // Every version the rows staked, in the order they staked them: the last two are read back
    // through the service's own API below.
    let mut staked: Vec<String> = Vec::new();
    for (row, answered) in rows.iter().zip(&answers) {
        let case = row.case;
        assert_eq!(
            answered["ok"],
            json!(true),
            "case {case:?}: expected the call to be served, got {answered}"
        );
        if !row.stakes_version {
            continue;
        }
        let version = answered["versionId"].as_str().unwrap_or_default();
        match row.stated {
            // The handler's own token is what the version is recorded under: the SDK sends
            // what the caller stated and generates nothing over it.
            Some(token) => assert_eq!(
                version, token,
                "case {case:?}: expected the stated token to name the version, got {answered}"
            ),
            // A generated one is the random UUID the SDKs generate, inside
            // `ClientRequestTokenType`'s own length (32..64), and fresh on every call.
            None => {
                assert!(
                    (32..=64).contains(&version.len()) && version.contains('-'),
                    "case {case:?}: expected a generated UUID to name the version, got \
                     {answered}"
                );
                assert!(
                    !staked.iter().any(|held| held == version),
                    "case {case:?}: expected a value of its own, got {version:?} again among \
                     {staked:?}"
                );
            }
        }
        staked.push(version.to_string());
    }

    // What the SERVICE holds: the versions those calls staked, under the ids the calls were
    // recorded with — so a token the SDK generated but never sent would leave the current
    // version named by something else here. A version Secrets Manager has since deprecated and
    // evicted ("Secrets Manager removes a version with no staging labels") is no longer
    // reported, so what is read back is the two the labels still name: the last version staked,
    // which is `AWSCURRENT`, and the one before it, which that write demoted to `AWSPREVIOUS`.
    assert_eq!(
        staked.len(),
        5,
        "expected the five version-staking rows to have reported a version, got {staked:?}"
    );
    let described = secrets(
        &router,
        "the versions the calls staked",
        "DescribeSecret",
        &json!({ "SecretId": SECRET }),
    )
    .await;
    let held = described["VersionIdsToStages"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (case, version, stage) in [
        ("the last version staked", &staked[4], "AWSCURRENT"),
        ("the one it demoted", &staked[3], "AWSPREVIOUS"),
    ] {
        let stages = held.get(version).cloned().unwrap_or(Value::Null);
        assert!(
            stages
                .as_array()
                .is_some_and(|stages| stages.iter().any(|held| held == stage)),
            "case {case:?}: expected the secret to hold {version:?} at {stage}, got {described}"
        );
    }
}
