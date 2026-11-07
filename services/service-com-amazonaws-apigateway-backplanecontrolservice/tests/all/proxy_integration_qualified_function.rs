// SPDX-License-Identifier: CC0-1.0

//! An `AWS_PROXY` integration whose `uri` carries a QUALIFIER: which code the deployed
//! method is served by, and what a qualifier that resolves to nothing earns the caller.
//!
//! RULE: an `AWS_PROXY` integration's `uri` names ONE Lambda function including whatever
//! qualifier it carries — `Integration$uri` gives the value as
//! `arn:aws:apigateway:{region}:lambda:path/{service_api}` over the function-invoking
//! action's path `2015-03-31/functions/{function-arn}/invocations`, and a Lambda function is
//! addressed "with alias: `arn:aws:lambda:us-west-2:123456789012:function:my-function:PROD`
//! [or] with version: `…:my-function:1`" (`Invoke$FunctionName`) — so the invocation is made
//! against the code and configuration THAT qualifier resolves to, whatever the unpublished
//! version now holds, and a qualifier that resolves to no function at all is an integration
//! the call cannot be performed through, answered through the api's own
//! `API_CONFIGURATION_ERROR` gateway response (`500` with `{"message":"Internal server
//! error"}`) rather than by guessing at another target.
//!
//! Two services have to agree for the rule to hold — API Gateway holds the api, the stage and
//! the integration, Lambda holds the versions and the aliases and RUNS the code each of them
//! froze — so both are mounted on this suite's own engine, and every row drives the request
//! through the api's own invoke host
//! (`{rest-api-id}.execute-api.{region}.amazonaws.com/{stage}/{path}`, the endpoint
//! `RestApi$disableExecuteApiEndpoint` documents), so the answer is the one a client of the
//! deployed stage gets rather than a simulated one.
//!
//! The rows differ along what the rule generalises over: whether the `uri` carries a
//! qualifier at all, whether that qualifier is an ALIAS or a published VERSION number,
//! whether the function ahead of it is spelled as a full ARN or as a bare name, WHICH alias
//! is named (two aliases on one function, pointing at two versions published from two
//! different packages), and whether the qualifier — or the function — resolves to anything.

use std::io::Write as _;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the Region the integration ARNs name.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to. Deliberately NOT one of the alias names below, so a row
/// that resolved the stage segment as a qualifier would be visible.
const STAGE: &str = "prod";

/// The function every row's `uri` addresses.
const FUNCTION: &str = "apigw-qualified-handler";

/// A function name this emulator holds no record of, for the row whose `uri` names one.
const ABSENT_FUNCTION: &str = "apigw-qualified-absent";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The two aliases the rows name, and the marker baked into the package each one's version
/// was published from.
const FIRST_ALIAS: &str = "stable";
const FIRST_MARKER: &str = "published-one";
const SECOND_ALIAS: &str = "candidate";
const SECOND_MARKER: &str = "published-two";

/// The marker in the package the UNPUBLISHED version holds, which no published version does.
const LATEST_MARKER: &str = "unpublished";

/// An alias name no `CreateAlias` ever made, for the row whose `uri` qualifies the function
/// with it.
const ABSENT_ALIAS: &str = "ghost";

/// How long one invocation is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
/// The spans belong to Lambda, so nothing here sleeps a fixed span for it.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// What a refused integration answers: the sentence API Gateway words an
/// `API_CONFIGURATION_ERROR` with.
const CONFIGURATION_ERROR_BODY: &str = "Internal server error";

/// One answered request: its status and its body verbatim.
struct Answer {
    status: u16,
    body: Bytes,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// API Gateway and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the integration's own invocation is dispatched back into this same router, so the function
/// this suite created is the function the stage invokes.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call that has to answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answer.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One Lambda `restJson1` call that has to answer `expected`.
async fn lambda(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answer.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One request to the api's own invoke endpoint, carrying no credential at all, as a client
/// of a public method sends it.
async fn invoke(router: &MultiServiceRouter, api: &str, path: &str) -> Answer {
    dispatch(
        router,
        "GET",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}/{path}"),
        &[],
        Bytes::new(),
    )
    .await
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// A .zip deployment package holding a handler that answers `marker`, written in memory the
/// way a caller uploads one.
fn package(marker: &str) -> Vec<u8> {
    let source = format!(
        "import json\n\
         \n\
         MARKER = {}\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   return {{\n\
         \x20       \"statusCode\": 200,\n\
         \x20       \"headers\": {{\"Content-Type\": \"application/json\"}},\n\
         \x20       \"body\": json.dumps({{\"marker\": MARKER, \"path\": event.get(\"path\")}}),\n\
         \x20   }}\n",
        Value::String(marker.to_string())
    );
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

/// Poll `GetFunctionConfiguration` until Lambda reports the function `Active` with no update
/// still being applied — the two clock-owed transitions a created or re-coded function has.
async fn wait_until_settled(router: &MultiServiceRouter, case: &str) {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            case,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
            200,
        )
        .await;
        let active = reported.get("State").and_then(Value::as_str) == Some("Active");
        let applied = reported
            .get("LastUpdateStatus")
            .and_then(Value::as_str)
            .is_none_or(|status| status == "Successful");
        if active && applied {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup {case:?}: Lambda never reported {FUNCTION} settled");
}

/// Replace the function's code with the package answering `marker`, then publish a version of
/// it, answering the version label `PublishVersion` minted.
async fn published(router: &MultiServiceRouter, case: &str, marker: &str) -> String {
    lambda(
        router,
        case,
        "PUT",
        &format!("/2015-03-31/functions/{FUNCTION}/code"),
        Some(json!({
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(marker)),
        })),
        200,
    )
    .await;
    wait_until_settled(router, case).await;
    let version = lambda(
        router,
        case,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/versions"),
        Some(json!({})),
        201,
    )
    .await;
    member(case, &version, "Version")
}

/// A child resource of the api's root named `part`, answering its id.
async fn resource(router: &MultiServiceRouter, api: &str, root: &str, part: &str) -> String {
    let created = apigw(
        router,
        &format!("CreateResource({part})"),
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": part })),
        201,
    )
    .await;
    member(&format!("CreateResource({part})"), &created, "id")
}

/// A `GET` method on `resource` behind an `AWS_PROXY` integration whose `uri` names
/// `function` — whatever qualifier that spelling carries.
async fn proxy_method(router: &MultiServiceRouter, api: &str, resource: &str, function: &str) {
    apigw(
        router,
        &format!("PutMethod({resource})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        &format!("PutIntegration({resource})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function}/invocations"
            ),
        })),
        201,
    )
    .await;
}

/// One row: the resource path the client calls, the function spelling its integration's `uri`
/// carries, and the answer the records earn it.
struct Row {
    case: &'static str,
    /// The resource path part, which is also the path the client requests.
    part: &'static str,
    /// The function (and qualifier) the integration's `uri` names.
    function: String,
    status: u16,
    /// The marker the invoked package answers, for a row the records let through.
    marker: Option<&'static str>,
}

#[tokio::test]
async fn an_aws_proxy_uri_is_served_by_the_qualifier_it_names() {
    let router = emulator();

    // The function, created from the package the FIRST alias's version is published from.
    let created = lambda(
        &router,
        "CreateFunction",
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(FIRST_MARKER)),
            },
        })),
        201,
    )
    .await;
    let function_arn = member("CreateFunction", &created, "FunctionArn");
    wait_until_settled(&router, "CreateFunction").await;

    // Two published versions, from two different packages, and one alias on each — then the
    // unpublished version is re-coded a third time, so NO alias answers what `$LATEST` does.
    let first_version = lambda(
        &router,
        "PublishVersion(first)",
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/versions"),
        Some(json!({})),
        201,
    )
    .await;
    let first_version = member("PublishVersion(first)", &first_version, "Version");
    let second_version = published(&router, "PublishVersion(second)", SECOND_MARKER).await;
    assert_ne!(
        first_version, second_version,
        "setup \"two published versions\": expected distinct version labels, got \
         {first_version:?} twice"
    );
    for (alias, version) in [
        (FIRST_ALIAS, &first_version),
        (SECOND_ALIAS, &second_version),
    ] {
        lambda(
            &router,
            &format!("CreateAlias({alias})"),
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/aliases"),
            Some(json!({ "Name": alias, "FunctionVersion": version })),
            201,
        )
        .await;
    }
    lambda(
        &router,
        "UpdateFunctionCode(unpublished)",
        "PUT",
        &format!("/2015-03-31/functions/{FUNCTION}/code"),
        Some(json!({
            "ZipFile": base64::engine::general_purpose::STANDARD
                .encode(package(LATEST_MARKER)),
        })),
        200,
    )
    .await;
    wait_until_settled(&router, "UpdateFunctionCode(unpublished)").await;

    // The api, one resource per row.
    let api = member(
        "CreateRestApi",
        &apigw(
            &router,
            "CreateRestApi",
            "POST",
            "/restapis",
            Some(json!({ "name": "aws-proxy-qualified-target" })),
            201,
        )
        .await,
        "id",
    );
    let root = member(
        "GetResources",
        &apigw(
            &router,
            "GetResources",
            "GET",
            &format!("/restapis/{api}/resources"),
            None,
            200,
        )
        .await
        .get("item")
        .and_then(|items| items.get(0).cloned())
        .unwrap_or(Value::Null),
        "id",
    );

    let rows = [
        Row {
            case: "a uri carrying no qualifier: the unpublished version's own code",
            part: "unqualified",
            function: function_arn.clone(),
            status: 200,
            marker: Some(LATEST_MARKER),
        },
        Row {
            case: "a uri qualified with one alias: the code that alias's version froze",
            part: "first-alias",
            function: format!("{function_arn}:{FIRST_ALIAS}"),
            status: 200,
            marker: Some(FIRST_MARKER),
        },
        Row {
            case: "a uri qualified with the other alias, on the same function",
            part: "second-alias",
            function: format!("{function_arn}:{SECOND_ALIAS}"),
            status: 200,
            marker: Some(SECOND_MARKER),
        },
        Row {
            case: "a uri qualified with a published version number",
            part: "first-version",
            function: format!("{function_arn}:{first_version}"),
            status: 200,
            marker: Some(FIRST_MARKER),
        },
        Row {
            case: "a uri naming the function by name alone, qualified with an alias",
            part: "name-and-alias",
            function: format!("{FUNCTION}:{SECOND_ALIAS}"),
            status: 200,
            marker: Some(SECOND_MARKER),
        },
        Row {
            case: "a uri qualified with an alias no CreateAlias made: nothing to invoke",
            part: "absent-alias",
            function: format!("{function_arn}:{ABSENT_ALIAS}"),
            status: 500,
            marker: None,
        },
        Row {
            case: "a uri naming a function this account holds no record of",
            part: "absent-function",
            function: ABSENT_FUNCTION.to_string(),
            status: 500,
            marker: None,
        },
    ];

    for row in &rows {
        let resource = resource(&router, &api, &root, row.part).await;
        proxy_method(&router, &api, &resource, &row.function).await;
    }
    apigw(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    // The grant that admits the invocation, on EACH qualifier the rows address: a resource
    // policy "is attached to the ARN the permission names", so the unpublished version, each
    // published version and each alias carries its own.
    let account = engine::emulator_account_id();
    let source_arn = format!("arn:aws:execute-api:{REGION}:{account}:{api}/*/*/*");
    let qualifiers: [Option<&str>; 5] = [
        None,
        Some(first_version.as_str()),
        Some(second_version.as_str()),
        Some(FIRST_ALIAS),
        Some(SECOND_ALIAS),
    ];
    for (index, qualifier) in qualifiers.iter().enumerate() {
        let query = qualifier
            .map(|qualifier| format!("?Qualifier={qualifier}"))
            .unwrap_or_default();
        lambda(
            &router,
            &format!("AddPermission({qualifier:?})"),
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/policy{query}"),
            Some(json!({
                "StatementId": format!("apigw-{index}"),
                "Action": "lambda:InvokeFunction",
                "Principal": "apigateway.amazonaws.com",
                "SourceArn": source_arn,
            })),
            201,
        )
        .await;
    }

    for row in &rows {
        let case = row.case;
        let answered = invoke(&router, &api, row.part).await;
        assert_eq!(
            answered.status, row.status,
            "case {case:?} (uri naming {:?}): expected HTTP {}, got {} with {:?}",
            row.function,
            row.status,
            answered.status,
            answered.text()
        );
        match row.marker {
            Some(marker) => assert_eq!(
                answered.json(),
                json!({ "marker": marker, "path": format!("/{}", row.part) }),
                "case {case:?}: expected the document the {marker:?} package returns, got {:?}",
                answered.text()
            ),
            None => assert_eq!(
                answered.json(),
                json!({ "message": CONFIGURATION_ERROR_BODY }),
                "case {case:?}: expected the API_CONFIGURATION_ERROR sentence, got {:?}",
                answered.text()
            ),
        }
    }
}
