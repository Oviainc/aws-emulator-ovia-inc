// SPDX-License-Identifier: CC0-1.0

//! The AUTHENTICATION TOKEN a running handler mints for a database login, end to end over
//! the wire: a `python3.x` function whose code does `import boto3` and calls
//! `generate_db_auth_token` on an RDS client, and the token the invocation answers with.
//!
//! The rule pinned here: `generate_db_auth_token` is not an API call — the SDKs build the
//! token in the CLIENT, by presigning a request to `connect` as one database user on one
//! endpoint — so a handler that asks for one is answered with a token for exactly the
//! endpoint, port and user it named, signed in the Region its client was built for, for the
//! lifetime AWS issues ("the token has a lifetime of 15 minutes", Amazon RDS User Guide).
//! It is a method of the RDS client alone: on any other client the attribute does not
//! exist, exactly as it does not in the SDK.
//!
//! The rows differ along what the rule generalises over: the endpoint and port the token is
//! for, the database user, whether the client names a Region of its own, and the service
//! the client was built for.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the Region a client that names none
/// is built for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(150);

/// Lambda on its own engine: the token is minted by the RUNTIME, so no other service takes
/// part in it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
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

/// A .zip deployment package holding `source` as `lambda_function.py`.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler every row runs: it builds the client the event names and asks it for a
/// token, reporting what it got back.
const HANDLER_SOURCE: &str = r#"
import boto3


def lambda_handler(event, context):
    arguments = {}
    if event.get("region"):
        arguments["region_name"] = event["region"]
    client = boto3.client(event["service"], **arguments)
    try:
        minter = client.generate_db_auth_token
    except AttributeError as error:
        return {"ok": False, "attribute_error": str(error)}
    token = minter(
        DBHostname=event["host"],
        Port=event["port"],
        DBUsername=event["user"],
    )
    return {"ok": True, "token": token}
"#;

/// Create the function and wait out its provisioning window.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HANDLER_SOURCE)),
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
}

/// Invoke the function and answer with the payload it produced.
async fn invoke(router: &MultiServiceRouter, case: &str, name: &str, event: &Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    assert_eq!(
        headers.get("x-amz-function-error"), None,
        "case {case:?}: the handler must run to completion, got {payload}"
    );
    payload
}

/// The fields a token's query string carries, by name.
fn fields(token: &str) -> Vec<(String, String)> {
    let query = token.split_once("/?").map(|(_, query)| query).unwrap_or("");
    form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// The one value of the field `name`.
fn field(token: &str, name: &str) -> Option<String> {
    fields(token)
        .into_iter()
        .find(|(held, _)| held == name)
        .map(|(_, value)| value)
}

/// One row: the endpoint, port, user and Region the handler asks a token for.
struct Row {
    case: &'static str,
    host: &'static str,
    port: u32,
    user: &'static str,
    /// The Region the client is built for; `None` for a client that names none, which is
    /// built for the Region the invocation was addressed to.
    region: Option<&'static str>,
}

#[tokio::test]
async fn a_handler_mints_an_authentication_token_for_the_login_it_names() {
    let rows = [
        Row {
            case: "an endpoint on the default port",
            host: "orders.cluster-abc.us-east-1.rds.amazonaws.com",
            port: 5432,
            user: "iamuser",
            region: None,
        },
        Row {
            case: "another endpoint, another port and another user",
            host: "127.0.0.1",
            port: 15_432,
            user: "reporting",
            region: None,
        },
        Row {
            case: "a client built for a Region of its own",
            host: "orders.cluster-def.eu-west-1.rds.amazonaws.com",
            port: 3306,
            user: "appuser",
            region: Some("eu-west-1"),
        },
        Row {
            case: "a user name with a character a query string escapes",
            host: "127.0.0.1",
            port: 5432,
            user: "iam user+1",
            region: None,
        },
    ];
    let router = router();
    create_function(&router, "token-minter").await;
    for row in &rows {
        let case = row.case;
        let payload = invoke(
            &router,
            case,
            "token-minter",
            &json!({
                "service": "rds",
                "host": row.host,
                "port": row.port,
                "user": row.user,
                "region": row.region,
            }),
        )
        .await;
        assert_eq!(
            payload["ok"],
            json!(true),
            "case {case:?}: the handler must mint a token, got {payload}"
        );
        let token = payload["token"].as_str().unwrap_or_default().to_string();

        // The token addresses the endpoint, the port and the user the handler named.
        assert!(
            token.starts_with(&format!("{}:{}/?", row.host, row.port)),
            "case {case:?}: expected the token to address {}:{}, got {token:?}",
            row.host,
            row.port
        );
        assert_eq!(
            field(&token, "DBUser").as_deref(),
            Some(row.user),
            "case {case:?}: expected the token to name the database user, got {token:?}"
        );
        assert_eq!(
            field(&token, "Action").as_deref(),
            Some("connect"),
            "case {case:?}: expected a token for the connect action, got {token:?}"
        );

        // And it is a presigned SigV4 request: the algorithm, a credential scope naming the
        // `rds-db` signing service in the client's own Region, the lifetime AWS issues, and
        // a signature.
        assert_eq!(
            field(&token, "X-Amz-Algorithm").as_deref(),
            Some("AWS4-HMAC-SHA256"),
            "case {case:?}: expected a SigV4 presigned request, got {token:?}"
        );
        let credential = field(&token, "X-Amz-Credential").unwrap_or_default();
        let scope: Vec<&str> = credential.split('/').collect();
        assert_eq!(
            scope.len(),
            5,
            "case {case:?}: expected a five-field credential scope, got {credential:?}"
        );
        assert_eq!(
            scope.get(2).copied(),
            Some(row.region.unwrap_or(REGION)),
            "case {case:?}: expected the scope to name the client's Region, got \
             {credential:?}"
        );
        assert_eq!(
            scope.get(3).copied(),
            Some("rds-db"),
            "case {case:?}: expected the scope to name the rds-db signing service, got \
             {credential:?}"
        );
        assert_eq!(
            field(&token, "X-Amz-Expires").as_deref(),
            Some("900"),
            "case {case:?}: expected the 15-minute lifetime AWS issues, got {token:?}"
        );
        let signature = field(&token, "X-Amz-Signature").unwrap_or_default();
        assert!(
            signature.len() == 64 && signature.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a SigV4 signature, got {signature:?}"
        );
        // The token carries no secret: a presigned request's signing values are the key
        // id, the scope and the signature over the one request — so there is no session
        // token and nothing a holder could sign another request with.
        assert_eq!(
            field(&token, "X-Amz-Security-Token"),
            None,
            "case {case:?}: expected no session token in the token, got {token:?}"
        );
        assert_eq!(
            fields(&token)
                .into_iter()
                .map(|(name, _)| name)
                .filter(|name| !name.starts_with("X-Amz-")
                    && name != "Action"
                    && name != "DBUser")
                .collect::<Vec<_>>(),
            Vec::<String>::new(),
            "case {case:?}: expected only the request's own and SigV4's own fields, got \
             {token:?}"
        );
    }
}

/// `generate_db_auth_token` is the RDS client's: on any other client the attribute does not
/// exist at all, which is what the SDK does.
#[tokio::test]
async fn only_the_rds_client_mints_an_authentication_token() {
    let router = router();
    create_function(&router, "token-minter-wrong-client").await;
    for (case, service) in [
        ("a queue client", "sqs"),
        ("a secrets client", "secretsmanager"),
    ] {
        let payload = invoke(
            &router,
            case,
            "token-minter-wrong-client",
            &json!({
                "service": service,
                "host": "127.0.0.1",
                "port": 5432,
                "user": "iamuser",
                "region": Value::Null,
            }),
        )
        .await;
        assert_eq!(
            payload["ok"],
            json!(false),
            "case {case:?}: expected the attribute to be absent, got {payload}"
        );
        let reported = payload["attribute_error"].as_str().unwrap_or_default();
        assert!(
            reported.contains("generate_db_auth_token"),
            "case {case:?}: expected the AttributeError to name the method, got {reported:?}"
        );
    }
}
